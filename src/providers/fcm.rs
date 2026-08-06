use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::header::RETRY_AFTER;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;
use tokio::sync::Mutex;
use url::Url;

use crate::contracts::{
    ContractVersion, OutcomeClass, ProviderKind, PushJob, PushOutcome, PushPriority, PushTarget,
};
use crate::provider::{ProviderError, ProviderReadiness, PushProvider};
use crate::redaction::truncate_utf8;
use crate::retry::{classify_http_status, parse_retry_after};
use crate::validation::validate_push_job;

const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const DEFAULT_API_BASE_URL: &str = "https://fcm.googleapis.com/";
const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const JWT_LIFETIME_SECONDS: u64 = 3_600;
const ACCESS_TOKEN_REFRESH_MARGIN_SECONDS: u64 = 60;
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_SAFE_DETAIL_BYTES: usize = 512;

#[derive(Debug, Error)]
pub enum FcmConfigError {
    #[error("FCM service-account JSON is invalid")]
    InvalidJson(#[source] serde_json::Error),

    #[error("FCM service-account field '{0}' is required")]
    MissingField(&'static str),

    #[error("FCM project_id contains invalid characters")]
    InvalidProjectId,

    #[error("FCM token_uri must be an HTTPS URL without embedded credentials")]
    InvalidTokenUri,

    #[error("FCM HTTP client could not be constructed")]
    HttpClient(#[source] reqwest::Error),
}

#[derive(Clone)]
enum FcmCredentialSource {
    ServiceAccount {
        client_email: String,
        private_key: String,
        token_uri: Url,
    },
}

#[derive(Clone)]
pub struct FcmConfig {
    project_id: String,
    credential: FcmCredentialSource,
    api_base_url: Url,
    request_timeout: Duration,
}

impl FcmConfig {
    pub fn from_service_account_json(
        raw: &str,
        project_id_override: Option<&str>,
    ) -> Result<Self, FcmConfigError> {
        let document: FcmServiceAccountDocument =
            serde_json::from_str(raw).map_err(FcmConfigError::InvalidJson)?;

        let project_id = project_id_override
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or(document.project_id)
            .ok_or(FcmConfigError::MissingField("project_id"))?;
        validate_project_id(&project_id)?;

        let client_email = required(document.client_email, "client_email")?;
        let private_key = required(document.private_key, "private_key")?;
        let token_uri = validate_token_uri(&document.token_uri)?;

        Ok(Self {
            project_id,
            credential: FcmCredentialSource::ServiceAccount {
                client_email,
                private_key,
                token_uri,
            },
            api_base_url: Url::parse(DEFAULT_API_BASE_URL).expect("constant FCM API URL is valid"),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }
}

#[derive(Deserialize)]
struct FcmServiceAccountDocument {
    #[serde(default)]
    client_email: String,
    #[serde(default)]
    private_key: String,
    #[serde(default = "default_token_uri")]
    token_uri: String,
    project_id: Option<String>,
}

fn default_token_uri() -> String {
    DEFAULT_TOKEN_URI.to_owned()
}

fn required(value: String, field: &'static str) -> Result<String, FcmConfigError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        Err(FcmConfigError::MissingField(field))
    } else {
        Ok(value)
    }
}

fn validate_project_id(project_id: &str) -> Result<(), FcmConfigError> {
    if project_id.is_empty()
        || project_id.len() > 128
        || project_id.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':'))
        })
    {
        return Err(FcmConfigError::InvalidProjectId);
    }
    Ok(())
}

fn validate_token_uri(value: &str) -> Result<Url, FcmConfigError> {
    let url = Url::parse(value).map_err(|_| FcmConfigError::InvalidTokenUri)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(FcmConfigError::InvalidTokenUri);
    }
    Ok(url)
}

#[derive(Clone)]
pub struct FcmProvider {
    config: FcmConfig,
    client: Client,
    token: Arc<Mutex<Option<CachedAccessToken>>>,
}

struct CachedAccessToken {
    value: String,
    expires_at: Instant,
}

impl FcmProvider {
    pub fn new(config: FcmConfig) -> Result<Self, FcmConfigError> {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(FcmConfigError::HttpClient)?;
        Ok(Self::with_client(config, client))
    }

    pub fn with_client(config: FcmConfig, client: Client) -> Self {
        Self {
            config,
            client,
            token: Arc::new(Mutex::new(None)),
        }
    }

    async fn access_token(&self) -> Result<String, ProviderError> {
        // Hold the lock through refresh so concurrent callers share one token mint.
        let mut guard = self.token.lock().await;
        if let Some(cached) = guard.as_ref()
            && cached.expires_at > Instant::now()
        {
            return Ok(cached.value.clone());
        }

        let refreshed = self.refresh_access_token().await?;
        let value = refreshed.value.clone();
        *guard = Some(refreshed);
        Ok(value)
    }

    async fn refresh_access_token(&self) -> Result<CachedAccessToken, ProviderError> {
        let FcmCredentialSource::ServiceAccount {
            client_email,
            private_key,
            token_uri,
        } = &self.config.credential;

        let now = unix_now()?;
        let claims = build_oauth_claims(client_email, token_uri.as_str(), now);
        let key = EncodingKey::from_rsa_pem(private_key.as_bytes()).map_err(|_| {
            ProviderError::not_configured("FCM service-account private key is invalid")
        })?;
        let assertion = encode(&Header::new(Algorithm::RS256), &claims, &key)
            .map_err(|_| ProviderError::internal("FCM OAuth assertion signing failed"))?;

        let response = self
            .client
            .post(token_uri.clone())
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", assertion.as_str()),
            ])
            .send()
            .await
            .map_err(|error| {
                ProviderError::delivery(
                    OutcomeClass::TransientProviderFailure,
                    format!("FCM OAuth token request failed: {error}"),
                    None,
                    Some("oauth_transport_error".to_owned()),
                )
            })?;

        let status = response.status();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| parse_retry_after(value, SystemTime::now()));
        let text = response.text().await.map_err(|_| {
            ProviderError::delivery(
                OutcomeClass::TransientProviderFailure,
                "FCM OAuth token response could not be read",
                retry_after,
                Some("oauth_response_error".to_owned()),
            )
        })?;

        if !status.is_success() {
            let parsed = serde_json::from_str::<FcmOauthError>(&text).ok();
            let detail = parsed
                .as_ref()
                .and_then(|error| error.error_description.as_deref())
                .unwrap_or("FCM OAuth token endpoint rejected the request")
                .to_owned();
            let provider_code = parsed
                .as_ref()
                .and_then(|error| error.error.clone())
                .or_else(|| Some(format!("oauth_http_{}", status.as_u16())));
            let decision = classify_http_status(status.as_u16(), retry_after);
            let class = if status == StatusCode::TOO_MANY_REQUESTS {
                OutcomeClass::Throttled
            } else if decision.retryable {
                OutcomeClass::TransientProviderFailure
            } else {
                OutcomeClass::PermanentProviderFailure
            };
            return Err(ProviderError::delivery(
                class,
                detail,
                decision.retry_after,
                provider_code,
            ));
        }

        let token_response: FcmOauthTokenResponse = serde_json::from_str(&text).map_err(|_| {
            ProviderError::delivery(
                OutcomeClass::PermanentProviderFailure,
                "FCM OAuth token response was not valid JSON",
                None,
                Some("oauth_invalid_response".to_owned()),
            )
        })?;
        if token_response.access_token.trim().is_empty() {
            return Err(ProviderError::delivery(
                OutcomeClass::PermanentProviderFailure,
                "FCM OAuth token response omitted access_token",
                None,
                Some("oauth_missing_access_token".to_owned()),
            ));
        }
        if token_response
            .token_type
            .as_deref()
            .is_some_and(|value| !value.eq_ignore_ascii_case("bearer"))
        {
            return Err(ProviderError::delivery(
                OutcomeClass::PermanentProviderFailure,
                "FCM OAuth token response returned an unsupported token type",
                None,
                Some("oauth_invalid_token_type".to_owned()),
            ));
        }

        let expires_in = token_response.expires_in.unwrap_or(JWT_LIFETIME_SECONDS);
        let usable_seconds = expires_in
            .saturating_sub(ACCESS_TOKEN_REFRESH_MARGIN_SECONDS)
            .max(1);
        Ok(CachedAccessToken {
            value: token_response.access_token,
            expires_at: Instant::now() + Duration::from_secs(usable_seconds),
        })
    }

    fn message_url(&self) -> Result<Url, ProviderError> {
        self.config
            .api_base_url
            .join(&format!(
                "v1/projects/{}/messages:send",
                self.config.project_id
            ))
            .map_err(|_| ProviderError::internal("FCM message endpoint could not be constructed"))
    }

    async fn send_message(&self, job: &PushJob) -> Result<PushOutcome, ProviderError> {
        if let Err(errors) = validate_push_job(job) {
            let detail = errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ProviderError::delivery(
                OutcomeClass::InvalidPayload,
                detail,
                None,
                Some("contract_validation_failed".to_owned()),
            ));
        }

        let device_token = match &job.target {
            PushTarget::Fcm { token } => token,
            _ => {
                return Err(ProviderError::delivery(
                    OutcomeClass::InvalidPayload,
                    "FCM provider received a non-FCM target",
                    None,
                    Some("provider_target_mismatch".to_owned()),
                ));
            }
        };

        let access_token = self.access_token().await?;
        let body = build_send_body(job, device_token);
        let response = self
            .client
            .post(self.message_url()?)
            .bearer_auth(access_token)
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                ProviderError::delivery(
                    OutcomeClass::TransientProviderFailure,
                    format!("FCM send request failed: {error}"),
                    None,
                    Some("transport_error".to_owned()),
                )
            })?;

        let status = response.status();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| parse_retry_after(value, SystemTime::now()));
        let text = response.text().await.map_err(|_| {
            ProviderError::delivery(
                OutcomeClass::TransientProviderFailure,
                "FCM response could not be read",
                retry_after,
                Some("response_read_error".to_owned()),
            )
        })?;
        let response_json = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);

        if status.is_success() {
            let mut outcome = PushOutcome::accepted(job);
            outcome.provider_code = response_json
                .get("name")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            return Ok(outcome);
        }

        let classified = classify_fcm_response(status.as_u16(), &response_json);
        Ok(PushOutcome {
            version: ContractVersion::V1,
            job_id: job.job_id.clone(),
            provider: ProviderKind::Fcm,
            target_fingerprint: job.target.fingerprint(),
            class: classified.class,
            provider_code: classified.provider_code,
            retry_after_ms: retry_after.map(duration_to_millis),
            safe_detail: classified.safe_detail,
        })
    }
}

#[async_trait]
impl PushProvider for FcmProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Fcm
    }

    fn readiness(&self) -> ProviderReadiness {
        ProviderReadiness::ready()
    }

    async fn send(&self, job: &PushJob) -> Result<PushOutcome, ProviderError> {
        self.send_message(job).await
    }
}

#[derive(Serialize)]
struct FcmOauthClaims<'a> {
    iss: &'a str,
    scope: &'static str,
    aud: &'a str,
    iat: u64,
    exp: u64,
}

fn build_oauth_claims<'a>(
    client_email: &'a str,
    token_uri: &'a str,
    now: u64,
) -> FcmOauthClaims<'a> {
    FcmOauthClaims {
        iss: client_email,
        scope: FCM_SCOPE,
        aud: token_uri,
        iat: now,
        exp: now + JWT_LIFETIME_SECONDS,
    }
}

fn unix_now() -> Result<u64, ProviderError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| ProviderError::internal("system clock is before the Unix epoch"))
}

#[derive(Deserialize)]
struct FcmOauthTokenResponse {
    access_token: String,
    expires_in: Option<u64>,
    token_type: Option<String>,
}

#[derive(Deserialize)]
struct FcmOauthError {
    error: Option<String>,
    error_description: Option<String>,
}

fn build_send_body(job: &PushJob, device_token: &str) -> Value {
    let mut message = Map::new();
    message.insert("token".to_owned(), Value::String(device_token.to_owned()));

    let mut notification = Map::new();
    if let Some(title) = &job.notification.title {
        notification.insert("title".to_owned(), Value::String(title.clone()));
    }
    if let Some(body) = &job.notification.body {
        notification.insert("body".to_owned(), Value::String(body.clone()));
    }
    if let Some(image) = &job.notification.image_url {
        notification.insert("image".to_owned(), Value::String(image.clone()));
    }
    if !notification.is_empty() {
        message.insert("notification".to_owned(), Value::Object(notification));
    }

    if !job.notification.data.is_empty() {
        let data = job
            .notification
            .data
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(stringify_value(value))))
            .collect::<Map<String, Value>>();
        message.insert("data".to_owned(), Value::Object(data));
    }

    let mut android = Map::new();
    android.insert(
        "priority".to_owned(),
        Value::String(match job.options.priority {
            PushPriority::Normal => "NORMAL".to_owned(),
            PushPriority::High => "HIGH".to_owned(),
        }),
    );
    if let Some(ttl_seconds) = job.options.ttl_seconds {
        android.insert("ttl".to_owned(), Value::String(format!("{ttl_seconds}s")));
    }
    if let Some(collapse_key) = &job.options.collapse_key {
        android.insert(
            "collapse_key".to_owned(),
            Value::String(collapse_key.clone()),
        );
    }
    message.insert("android".to_owned(), Value::Object(android));

    let mut request = Map::new();
    request.insert("message".to_owned(), Value::Object(message));
    if job.options.dry_run {
        request.insert("validate_only".to_owned(), Value::Bool(true));
    }
    Value::Object(request)
}

fn stringify_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned()),
    }
}

struct ClassifiedFcmResponse {
    class: OutcomeClass,
    provider_code: Option<String>,
    safe_detail: Option<String>,
}

fn classify_fcm_response(status: u16, body: &Value) -> ClassifiedFcmResponse {
    let error = body.get("error").unwrap_or(&Value::Null);
    let status_name = error
        .get("status")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let error_code = error
        .get("details")
        .and_then(Value::as_array)
        .and_then(|details| {
            details.iter().find_map(|detail| {
                detail
                    .get("errorCode")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
        });
    let provider_code = error_code.clone().or(status_name.clone());
    let class = match error_code.as_deref().or(status_name.as_deref()) {
        Some("UNREGISTERED" | "SENDER_ID_MISMATCH") => OutcomeClass::InvalidToken,
        Some("INVALID_ARGUMENT") => OutcomeClass::InvalidPayload,
        Some("QUOTA_EXCEEDED" | "RESOURCE_EXHAUSTED") => OutcomeClass::Throttled,
        Some("UNAVAILABLE" | "INTERNAL") => OutcomeClass::TransientProviderFailure,
        Some("THIRD_PARTY_AUTH_ERROR" | "UNAUTHENTICATED" | "PERMISSION_DENIED") => {
            OutcomeClass::PermanentProviderFailure
        }
        _ => match status {
            400 => OutcomeClass::InvalidPayload,
            401 | 403 | 404 => OutcomeClass::PermanentProviderFailure,
            429 => OutcomeClass::Throttled,
            500..=599 => OutcomeClass::TransientProviderFailure,
            _ => OutcomeClass::PermanentProviderFailure,
        },
    };
    let safe_detail = error
        .get("message")
        .and_then(Value::as_str)
        .map(|message| truncate_utf8(message, MAX_SAFE_DETAIL_BYTES));

    ClassifiedFcmResponse {
        class,
        provider_code,
        safe_detail,
    }
}

fn duration_to_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::json;

    use crate::contracts::{Notification, PushOptions, TraceMetadata};

    fn service_account_json(token_uri: &str) -> String {
        json!({
            "client_email": "push-test@example.invalid",
            "private_key": "not-a-real-private-key",
            "token_uri": token_uri,
            "project_id": "test-project"
        })
        .to_string()
    }

    fn test_job() -> PushJob {
        PushJob {
            version: ContractVersion::V1,
            job_id: "job-1".to_owned(),
            tenant_id: "tenant-1".to_owned(),
            application_id: "app-1".to_owned(),
            idempotency_key: "event-1:user-1".to_owned(),
            provider: ProviderKind::Fcm,
            target: PushTarget::Fcm {
                token: "test-device".to_owned(),
            },
            notification: Notification {
                title: Some("Hello".to_owned()),
                body: Some("World".to_owned()),
                image_url: None,
                data: BTreeMap::from([
                    ("enabled".to_owned(), Value::Bool(true)),
                    ("count".to_owned(), json!(3)),
                    ("nested".to_owned(), json!({"a": 1})),
                ]),
            },
            options: PushOptions {
                ttl_seconds: Some(120),
                priority: PushPriority::High,
                collapse_key: Some("status".to_owned()),
                dry_run: true,
            },
            trace: TraceMetadata::default(),
        }
    }

    #[test]
    fn parses_service_account_without_exposing_secrets() {
        let config =
            FcmConfig::from_service_account_json(&service_account_json(DEFAULT_TOKEN_URI), None)
                .expect("valid service account");
        assert_eq!(config.project_id(), "test-project");
    }

    #[test]
    fn project_override_wins_and_invalid_urls_are_rejected() {
        let config = FcmConfig::from_service_account_json(
            &service_account_json(DEFAULT_TOKEN_URI),
            Some("override-project"),
        )
        .expect("valid override");
        assert_eq!(config.project_id(), "override-project");

        let error = match FcmConfig::from_service_account_json(
            &service_account_json("http://oauth.example.invalid/token"),
            None,
        ) {
            Err(error) => error,
            Ok(_) => panic!("non-HTTPS token URI must fail"),
        };
        assert!(matches!(error, FcmConfigError::InvalidTokenUri));
    }

    #[test]
    fn oauth_claims_match_fcm_scope_and_audience() {
        let claims = build_oauth_claims("push-test@example.invalid", DEFAULT_TOKEN_URI, 1_000);
        let encoded = serde_json::to_value(claims).expect("serialize claims");
        assert_eq!(encoded["iss"], "push-test@example.invalid");
        assert_eq!(encoded["scope"], FCM_SCOPE);
        assert_eq!(encoded["aud"], DEFAULT_TOKEN_URI);
        assert_eq!(encoded["iat"], 1_000);
        assert_eq!(encoded["exp"], 1_000 + JWT_LIFETIME_SECONDS);
    }

    #[test]
    fn builds_fcm_message_and_coerces_data_values_to_strings() {
        let body = build_send_body(&test_job(), "test-device");
        assert_eq!(body["message"]["token"], "test-device");
        assert_eq!(body["message"]["notification"]["title"], "Hello");
        assert_eq!(body["message"]["data"]["enabled"], "true");
        assert_eq!(body["message"]["data"]["count"], "3");
        assert_eq!(body["message"]["data"]["nested"], "{\"a\":1}");
        assert_eq!(body["message"]["android"]["ttl"], "120s");
        assert_eq!(body["message"]["android"]["priority"], "HIGH");
        assert_eq!(body["message"]["android"]["collapse_key"], "status");
        assert_eq!(body["validate_only"], true);
    }

    #[test]
    fn classifies_fcm_error_codes() {
        let unregistered = json!({
            "error": {
                "status": "NOT_FOUND",
                "message": "Requested entity was not found.",
                "details": [{"errorCode": "UNREGISTERED"}]
            }
        });
        assert_eq!(
            classify_fcm_response(404, &unregistered).class,
            OutcomeClass::InvalidToken
        );

        let quota = json!({
            "error": {
                "status": "RESOURCE_EXHAUSTED",
                "details": [{"errorCode": "QUOTA_EXCEEDED"}]
            }
        });
        assert_eq!(
            classify_fcm_response(429, &quota).class,
            OutcomeClass::Throttled
        );

        let unavailable = json!({"error": {"status": "UNAVAILABLE"}});
        assert_eq!(
            classify_fcm_response(503, &unavailable).class,
            OutcomeClass::TransientProviderFailure
        );
    }

    #[derive(Clone, Default)]
    struct Capture {
        authorization: Arc<Mutex<Option<String>>>,
        body: Arc<Mutex<Option<Value>>>,
    }

    async fn capture_send(
        State(capture): State<Capture>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        *capture.authorization.lock().await = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        *capture.body.lock().await = Some(body);
        Json(json!({"name": "projects/test-project/messages/message-1"}))
    }

    #[tokio::test]
    async fn sends_to_mock_endpoint_with_cached_bearer_token() {
        let capture = Capture::default();
        let app = Router::new()
            .route(
                "/v1/projects/test-project/messages:send",
                post(capture_send),
            )
            .with_state(capture.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let address = listener.local_addr().expect("mock address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });

        let mut config =
            FcmConfig::from_service_account_json(&service_account_json(DEFAULT_TOKEN_URI), None)
                .expect("config");
        config.api_base_url = Url::parse(&format!("http://{address}/")).expect("mock URL");
        let provider = FcmProvider::new(config).expect("provider");
        *provider.token.lock().await = Some(CachedAccessToken {
            value: "test-access".to_owned(),
            expires_at: Instant::now() + Duration::from_secs(60),
        });

        let outcome = provider.send(&test_job()).await.expect("send succeeds");
        assert_eq!(outcome.class, OutcomeClass::Accepted);
        assert_eq!(
            capture.authorization.lock().await.as_deref(),
            Some("Bearer test-access")
        );
        assert_eq!(
            capture.body.lock().await.as_ref().expect("captured body")["message"]["data"]["enabled"],
            "true"
        );

        server.abort();
    }
}
