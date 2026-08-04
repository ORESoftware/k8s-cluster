use std::path::Path;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::header::{HeaderMap as ReqwestHeaderMap, HeaderValue};
use serde::Deserialize;
use time::OffsetDateTime;

use crate::config::{first_env, Config};
use crate::exec::{append_log, run_logged_command_with_input};
use crate::state::{AppState, SERVICE_NAME};
use crate::util::{hmac_sha256, sha256_hex};

#[derive(Debug, Clone)]
pub(crate) struct EcrImage {
    pub(crate) registry: String,
    pub(crate) region: String,
}

// No `#[derive(Debug)]`: this holds a live AWS secret access key and session
// token. A stray `{:?}`/`dbg!` must not be able to print it.
pub(crate) struct AwsCredentials {
    pub(crate) access_key_id: String,
    pub(crate) secret_access_key: String,
    pub(crate) session_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EcrAuthResponse {
    pub(crate) authorization_data: Vec<EcrAuthorizationData>,
}

// No `#[derive(Debug)]`: `authorization_token` is a usable ECR credential.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EcrAuthorizationData {
    pub(crate) authorization_token: String,
    pub(crate) proxy_endpoint: String,
}

pub(crate) fn image_registry(image: &str) -> Option<&str> {
    let first = image.split('/').next().unwrap_or_default();
    if first.contains('.') || first.contains(':') || first == "localhost" {
        Some(first)
    } else {
        None
    }
}

pub(crate) fn ecr_image(image: &str) -> Option<EcrImage> {
    let registry = image_registry(image)?;
    let parts = registry.split('.').collect::<Vec<_>>();
    if parts.len() >= 6
        && parts[1] == "dkr"
        && parts[2] == "ecr"
        && parts[4] == "amazonaws"
        && (parts[5] == "com" || parts[5] == "com.cn")
    {
        return Some(EcrImage {
            registry: registry.to_string(),
            region: parts[3].to_string(),
        });
    }
    None
}

pub(crate) fn aws_timestamp() -> (String, String) {
    let now = OffsetDateTime::now_utc();
    let date = format!(
        "{:04}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    );
    let timestamp = format!(
        "{}T{:02}{:02}{:02}Z",
        date,
        now.hour(),
        now.minute(),
        now.second()
    );
    (date, timestamp)
}

pub(crate) fn aws_credentials_from_env() -> Result<AwsCredentials, String> {
    let access_key_id = first_env(&["AWS_ACCESS_KEY_ID"])
        .ok_or_else(|| "AWS_ACCESS_KEY_ID is required for ECR push".to_string())?;
    let secret_access_key = first_env(&["AWS_SECRET_ACCESS_KEY"])
        .ok_or_else(|| "AWS_SECRET_ACCESS_KEY is required for ECR push".to_string())?;
    let session_token = first_env(&["AWS_SESSION_TOKEN"]);
    Ok(AwsCredentials {
        access_key_id,
        secret_access_key,
        session_token,
    })
}

pub(crate) fn ecr_headers(
    config: &Config,
    credentials: &AwsCredentials,
    region: &str,
    host: &str,
    body: &str,
) -> Result<ReqwestHeaderMap, String> {
    let target = "AmazonEC2ContainerRegistry_V20150921.GetAuthorizationToken";
    let content_type = "application/x-amz-json-1.1";
    let (date, timestamp) = aws_timestamp();
    let session_token = credentials.session_token.as_deref().unwrap_or("");
    let (canonical_headers, signed_headers) = if session_token.is_empty() {
        (
            format!("content-type:{content_type}\nhost:{host}\nx-amz-date:{timestamp}\nx-amz-target:{target}\n"),
            "content-type;host;x-amz-date;x-amz-target",
        )
    } else {
        (
            format!(
                "content-type:{content_type}\nhost:{host}\nx-amz-date:{timestamp}\nx-amz-security-token:{session_token}\nx-amz-target:{target}\n"
            ),
            "content-type;host;x-amz-date;x-amz-security-token;x-amz-target",
        )
    };
    let canonical_request = format!(
        "POST\n/\n\n{canonical_headers}\n{signed_headers}\n{}",
        sha256_hex(body)
    );
    let credential_scope = format!("{date}/{region}/ecr/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{timestamp}\n{credential_scope}\n{}",
        sha256_hex(&canonical_request)
    );
    let date_key = hmac_sha256(
        format!("AWS4{}", credentials.secret_access_key).as_bytes(),
        &date,
    );
    let region_key = hmac_sha256(&date_key, region);
    let service_key = hmac_sha256(&region_key, "ecr");
    let signing_key = hmac_sha256(&service_key, "aws4_request");
    let signature = hex::encode(hmac_sha256(&signing_key, &string_to_sign));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
        credentials.access_key_id
    );

    let mut headers = ReqwestHeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static(content_type));
    headers.insert(
        "x-amz-date",
        HeaderValue::from_str(&timestamp).map_err(|error| error.to_string())?,
    );
    headers.insert("x-amz-target", HeaderValue::from_static(target));
    headers.insert(
        "authorization",
        HeaderValue::from_str(&authorization).map_err(|error| error.to_string())?,
    );
    if !session_token.is_empty() {
        headers.insert(
            "x-amz-security-token",
            HeaderValue::from_str(session_token).map_err(|error| error.to_string())?,
        );
    }
    headers.insert(
        "user-agent",
        HeaderValue::from_str(&format!("{SERVICE_NAME}/0.1 ({})", config.aws_region))
            .map_err(|error| error.to_string())?,
    );
    Ok(headers)
}

pub(crate) async fn ecr_authorization_password(
    state: &AppState,
    ecr: &EcrImage,
) -> Result<String, String> {
    let credentials = aws_credentials_from_env()?;
    let body = "{}";
    let host = format!("api.ecr.{}.amazonaws.com", ecr.region);
    let headers = ecr_headers(&state.config, &credentials, &ecr.region, &host, body)?;
    let response = state
        .http
        .post(format!("https://{host}/"))
        .headers(headers)
        .body(body.to_string())
        .send()
        .await
        .map_err(|error| format!("failed to request ECR authorization token: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("failed to read ECR authorization response: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "ECR authorization failed with HTTP {}: {}",
            status.as_u16(),
            text.chars().take(400).collect::<String>()
        ));
    }
    let parsed: EcrAuthResponse = serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse ECR authorization response: {error}"))?;
    let data = parsed
        .authorization_data
        .iter()
        .find(|data| data.proxy_endpoint.trim_start_matches("https://") == ecr.registry)
        .or_else(|| parsed.authorization_data.first())
        .ok_or_else(|| {
            "ECR authorization response did not include authorizationData".to_string()
        })?;
    let decoded = BASE64
        .decode(&data.authorization_token)
        .map_err(|error| format!("failed to decode ECR authorization token: {error}"))?;
    let decoded = String::from_utf8(decoded)
        .map_err(|error| format!("ECR authorization token was not UTF-8: {error}"))?;
    let Some((username, password)) = decoded.split_once(':') else {
        return Err("ECR authorization token did not contain username/password".to_string());
    };
    if username != "AWS" {
        return Err("ECR authorization token had an unexpected username".to_string());
    }
    Ok(password.to_string())
}

pub(crate) async fn login_to_ecr(
    state: &AppState,
    log_path: &Path,
    cwd: &Path,
    ecr: &EcrImage,
) -> Result<(), String> {
    append_log(
        log_path,
        &format!("requesting ECR login token for {}\n", ecr.registry),
        state.config.max_log_bytes,
    )
    .await;
    let password = ecr_authorization_password(state, ecr).await?;
    let args = vec![
        "-n".to_string(),
        state.config.containerd_namespace.clone(),
        "login".to_string(),
        "--username".to_string(),
        "AWS".to_string(),
        "--password-stdin".to_string(),
        ecr.registry.clone(),
    ];
    let display_args = args.clone();
    run_logged_command_with_input(
        &state.config,
        log_path,
        cwd,
        &state.config.nerdctl_bin,
        args,
        display_args,
        format!("{password}\n").into_bytes(),
    )
    .await
}
