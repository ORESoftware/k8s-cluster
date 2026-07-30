use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use reqwest::header::RETRY_AFTER;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use thiserror::Error;
use url::Url;

use super::contracts::{
    ContactContent, ContactJob, ContactOutcome, ContactOutcomeClass, ContactProviderKind,
    ContactTarget,
};
use super::provider::{ContactProvider, ContactProviderError};
use super::validation::{valid_email_address, validate_contact_job};
use crate::provider::ProviderReadiness;
use crate::redaction::truncate_utf8;
use crate::retry::{classify_http_status, parse_retry_after};

const GLOBAL_API_BASE: &str = "https://api.sendgrid.com/";
const EUROPE_API_BASE: &str = "https://api.eu.sendgrid.com/";
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_SAFE_DETAIL_BYTES: usize = 512;
const MAX_PROVIDER_CODE_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendGridRegion {
    Global,
    Europe,
}

impl SendGridRegion {
    fn api_base_url(self) -> Url {
        Url::parse(match self {
            Self::Global => GLOBAL_API_BASE,
            Self::Europe => EUROPE_API_BASE,
        })
        .expect("constant SendGrid API URL is valid")
    }
}

#[derive(Debug, Error)]
pub enum SendGridConfigError {
    #[error("SENDGRID_API_KEY must contain 20 to 512 non-control characters")]
    InvalidApiKey,

    #[error("SENDGRID_FROM_EMAIL must be a valid email address")]
    InvalidFromEmail,

    #[error("SENDGRID_FROM_NAME must be at most 256 bytes and contain no control characters")]
    InvalidFromName,

    #[error("SendGrid API base URL must be HTTPS without embedded credentials")]
    InvalidApiBaseUrl,

    #[error("SendGrid HTTP client could not be constructed")]
    HttpClient(#[source] reqwest::Error),
}

#[derive(Debug, Clone)]
pub struct SendGridConfig {
    api_key: String,
    from_email: String,
    from_name: Option<String>,
    api_base_url: Url,
    sandbox_mode: bool,
    request_timeout: Duration,
}

impl SendGridConfig {
    pub fn new(
        api_key: impl Into<String>,
        from_email: impl Into<String>,
        from_name: Option<String>,
        region: SendGridRegion,
    ) -> Result<Self, SendGridConfigError> {
        let api_key = api_key.into();
        let api_key = api_key.trim().to_owned();
        if !(20..=512).contains(&api_key.len()) || api_key.chars().any(char::is_control) {
            return Err(SendGridConfigError::InvalidApiKey);
        }

        let from_email = from_email.into();
        let from_email = from_email.trim().to_ascii_lowercase();
        if !valid_email_address(&from_email) {
            return Err(SendGridConfigError::InvalidFromEmail);
        }

        let from_name = from_name
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if from_name
            .as_ref()
            .is_some_and(|value| value.len() > 256 || value.chars().any(char::is_control))
        {
            return Err(SendGridConfigError::InvalidFromName);
        }

        Ok(Self {
            api_key,
            from_email,
            from_name,
            api_base_url: region.api_base_url(),
            sandbox_mode: false,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }

    pub fn with_sandbox_mode(mut self, enabled: bool) -> Self {
        self.sandbox_mode = enabled;
        self
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub fn with_api_base_url(mut self, api_base_url: Url) -> Result<Self, SendGridConfigError> {
        validate_api_base_url(&api_base_url)?;
        self.api_base_url = api_base_url;
        Ok(self)
    }
}

fn validate_api_base_url(url: &Url) -> Result<(), SendGridConfigError> {
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(SendGridConfigError::InvalidApiBaseUrl);
    }
    Ok(())
}

#[derive(Clone)]
pub struct SendGridProvider {
    config: SendGridConfig,
    client: Client,
}

impl SendGridProvider {
    pub fn new(config: SendGridConfig) -> Result<Self, SendGridConfigError> {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(SendGridConfigError::HttpClient)?;
        Ok(Self { config, client })
    }

    pub fn with_client(config: SendGridConfig, client: Client) -> Self {
        Self { config, client }
    }

    fn mail_send_url(&self) -> Result<Url, ContactProviderError> {
        self.config.api_base_url.join("v3/mail/send").map_err(|_| {
            ContactProviderError::internal("SendGrid endpoint could not be constructed")
        })
    }

    async fn send_email(&self, job: &ContactJob) -> Result<ContactOutcome, ContactProviderError> {
        if let Err(errors) = validate_contact_job(job) {
            return Err(ContactProviderError::delivery(
                ContactOutcomeClass::InvalidPayload,
                errors
                    .into_iter()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
                None,
                Some("contract_validation_failed".to_owned()),
            ));
        }

        let body = build_mail_send_body(job, &self.config)?;
        let response = self
            .client
            .post(self.mail_send_url()?)
            .bearer_auth(&self.config.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                ContactProviderError::delivery(
                    ContactOutcomeClass::TransientProviderFailure,
                    format!("SendGrid request failed: {error}"),
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
        let message_id = response
            .headers()
            .get("x-message-id")
            .and_then(|value| value.to_str().ok())
            .and_then(valid_provider_code);
        let response_text = response.text().await.unwrap_or_default();

        if status.is_success() {
            return Ok(ContactOutcome::accepted(job, message_id));
        }

        let (class, safe_detail, provider_code) =
            classify_sendgrid_failure(status, &response_text, retry_after);
        Ok(ContactOutcome {
            version: job.version,
            job_id: job.job_id.clone(),
            provider: ContactProviderKind::Sendgrid,
            target_fingerprint: job.target.fingerprint(),
            class,
            provider_code,
            retry_after_ms: retry_after.map(duration_to_millis),
            safe_detail: Some(safe_detail),
        })
    }
}

#[async_trait]
impl ContactProvider for SendGridProvider {
    fn kind(&self) -> ContactProviderKind {
        ContactProviderKind::Sendgrid
    }

    fn readiness(&self) -> ProviderReadiness {
        ProviderReadiness::ready()
    }

    async fn send(&self, job: &ContactJob) -> Result<ContactOutcome, ContactProviderError> {
        self.send_email(job).await
    }
}

fn build_mail_send_body(
    job: &ContactJob,
    config: &SendGridConfig,
) -> Result<Value, ContactProviderError> {
    let (address, name) = match &job.target {
        ContactTarget::Email { address, name } => (address, name),
        _ => {
            return Err(ContactProviderError::delivery(
                ContactOutcomeClass::InvalidPayload,
                "SendGrid provider received a non-email target",
                None,
                Some("provider_target_mismatch".to_owned()),
            ));
        }
    };
    let ContactContent::Email {
        subject,
        text,
        html,
        template_id,
        dynamic_template_data,
        reply_to,
    } = &job.content
    else {
        return Err(ContactProviderError::delivery(
            ContactOutcomeClass::InvalidPayload,
            "SendGrid provider received non-email content",
            None,
            Some("provider_content_mismatch".to_owned()),
        ));
    };

    let mut recipient = Map::new();
    recipient.insert("email".to_owned(), Value::String(address.clone()));
    if let Some(name) = name {
        recipient.insert("name".to_owned(), Value::String(name.clone()));
    }

    let mut personalization = Map::new();
    personalization.insert(
        "to".to_owned(),
        Value::Array(vec![Value::Object(recipient)]),
    );
    if !dynamic_template_data.is_empty() {
        personalization.insert(
            "dynamic_template_data".to_owned(),
            Value::Object(
                dynamic_template_data
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        );
    }

    let mut sender = Map::new();
    sender.insert("email".to_owned(), Value::String(config.from_email.clone()));
    if let Some(name) = &config.from_name {
        sender.insert("name".to_owned(), Value::String(name.clone()));
    }

    let mut root = Map::new();
    root.insert(
        "personalizations".to_owned(),
        Value::Array(vec![Value::Object(personalization)]),
    );
    root.insert("from".to_owned(), Value::Object(sender));
    if let Some(reply_to) = reply_to {
        root.insert("reply_to".to_owned(), json!({"email": reply_to}));
    }
    if let Some(template_id) = template_id {
        root.insert("template_id".to_owned(), Value::String(template_id.clone()));
    } else {
        root.insert(
            "subject".to_owned(),
            Value::String(subject.clone().unwrap_or_default()),
        );
        let mut content = Vec::new();
        if let Some(text) = text {
            content.push(json!({"type": "text/plain", "value": text}));
        }
        if let Some(html) = html {
            content.push(json!({"type": "text/html", "value": html}));
        }
        root.insert("content".to_owned(), Value::Array(content));
    }
    if config.sandbox_mode {
        root.insert(
            "mail_settings".to_owned(),
            json!({"sandbox_mode": {"enable": true}}),
        );
    }
    Ok(Value::Object(root))
}

#[derive(Debug, Deserialize)]
struct SendGridErrorEnvelope {
    #[serde(default)]
    errors: Vec<SendGridErrorItem>,
}

#[derive(Debug, Deserialize)]
struct SendGridErrorItem {
    field: Option<String>,
}

fn classify_sendgrid_failure(
    status: StatusCode,
    response_text: &str,
    retry_after: Option<Duration>,
) -> (ContactOutcomeClass, String, Option<String>) {
    let decision = classify_http_status(status.as_u16(), retry_after);
    let class = match status {
        StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE | StatusCode::NOT_ACCEPTABLE => {
            ContactOutcomeClass::InvalidPayload
        }
        StatusCode::TOO_MANY_REQUESTS => ContactOutcomeClass::Throttled,
        _ if decision.retryable => ContactOutcomeClass::TransientProviderFailure,
        _ => ContactOutcomeClass::PermanentProviderFailure,
    };
    let safe_field = serde_json::from_str::<SendGridErrorEnvelope>(response_text)
        .ok()
        .and_then(|document| document.errors.into_iter().find_map(|error| error.field))
        .and_then(sanitize_error_field);
    let safe_detail = match safe_field {
        Some(field) => format!(
            "SendGrid rejected the request with HTTP {} in field {field}",
            status.as_u16()
        ),
        None => format!(
            "SendGrid rejected the request with HTTP {}",
            status.as_u16()
        ),
    };
    (
        class,
        truncate_utf8(&safe_detail, MAX_SAFE_DETAIL_BYTES),
        Some(format!("http_{}", status.as_u16())),
    )
}

fn sanitize_error_field(value: String) -> Option<String> {
    if value.len() > 128
        || value.is_empty()
        || value.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '[' | ']'))
        })
    {
        return None;
    }
    Some(value)
}

fn valid_provider_code(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > MAX_PROVIDER_CODE_BYTES
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return None;
    }
    Some(value.to_owned())
}

fn duration_to_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode as AxumStatusCode};
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::json;
    use tokio::sync::Mutex;

    use super::*;
    use crate::contact::contracts::{ContactTarget, ContactTargetFingerprint};
    use crate::contracts::{ContractVersion, TraceMetadata};

    fn explicit_job() -> ContactJob {
        ContactJob {
            version: ContractVersion::V1,
            job_id: "job-1".to_owned(),
            tenant_id: "tenant-1".to_owned(),
            application_id: "app-1".to_owned(),
            idempotency_key: "event-1".to_owned(),
            provider: ContactProviderKind::Sendgrid,
            target: ContactTarget::Email {
                address: "person@example.com".to_owned(),
                name: Some("Person".to_owned()),
            },
            content: ContactContent::Email {
                subject: Some("Hello".to_owned()),
                text: Some("Plain".to_owned()),
                html: Some("<p>Hello</p>".to_owned()),
                template_id: None,
                dynamic_template_data: BTreeMap::new(),
                reply_to: Some("support@example.com".to_owned()),
            },
            trace: TraceMetadata::default(),
        }
    }

    #[test]
    fn builds_explicit_and_template_payloads_without_sender_override() {
        let config = SendGridConfig::new(
            "SG.fixture-key-that-is-long-enough",
            "verified@example.com",
            Some("Verified Sender".to_owned()),
            SendGridRegion::Global,
        )
        .expect("config")
        .with_sandbox_mode(true);
        let explicit = build_mail_send_body(&explicit_job(), &config).expect("payload");
        assert_eq!(explicit["from"]["email"], "verified@example.com");
        assert_eq!(
            explicit["personalizations"][0]["to"][0]["email"],
            "person@example.com"
        );
        assert_eq!(explicit["mail_settings"]["sandbox_mode"]["enable"], true);

        let mut template = explicit_job();
        template.content = ContactContent::Email {
            subject: None,
            text: None,
            html: None,
            template_id: Some("d-0123456789abcdef".to_owned()),
            dynamic_template_data: BTreeMap::from([("name".to_owned(), json!("Person"))]),
            reply_to: None,
        };
        let payload = build_mail_send_body(&template, &config).expect("template payload");
        assert_eq!(payload["template_id"], "d-0123456789abcdef");
        assert_eq!(
            payload["personalizations"][0]["dynamic_template_data"]["name"],
            "Person"
        );
        assert!(payload.get("content").is_none());
    }

    #[test]
    fn classifies_failures_without_echoing_provider_body() {
        let target = "person@example.com";
        let body = format!(
            r#"{{"errors":[{{"message":"invalid recipient {target}","field":"personalizations.0.to.0.email"}}]}}"#
        );
        let (class, detail, code) = classify_sendgrid_failure(StatusCode::BAD_REQUEST, &body, None);
        assert_eq!(class, ContactOutcomeClass::InvalidPayload);
        assert!(!detail.contains(target));
        assert_eq!(code.as_deref(), Some("http_400"));
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
    ) -> (AxumStatusCode, [(axum::http::HeaderName, &'static str); 1]) {
        *capture.authorization.lock().await = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        *capture.body.lock().await = Some(body);
        (
            AxumStatusCode::ACCEPTED,
            [(
                axum::http::HeaderName::from_static("x-message-id"),
                "message-1",
            )],
        )
    }

    #[tokio::test]
    async fn sends_to_mock_endpoint_and_returns_only_redacted_outcome() {
        let capture = Capture::default();
        let app = Router::new()
            .route("/v3/mail/send", post(capture_send))
            .with_state(capture.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let address = listener.local_addr().expect("mock address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });

        let mut config = SendGridConfig::new(
            "SG.fixture-key-that-is-long-enough",
            "verified@example.com",
            None,
            SendGridRegion::Global,
        )
        .expect("config");
        config.api_base_url = Url::parse(&format!("http://{address}/")).expect("mock URL");
        let provider = SendGridProvider::new(config).expect("provider");
        let job = explicit_job();
        let outcome = provider.send(&job).await.expect("send succeeds");
        assert_eq!(outcome.class, ContactOutcomeClass::Accepted);
        assert_eq!(outcome.provider_code.as_deref(), Some("message-1"));
        assert_eq!(outcome.target_fingerprint, job.target.fingerprint());
        assert_eq!(
            capture.authorization.lock().await.as_deref(),
            Some("Bearer SG.fixture-key-that-is-long-enough")
        );
        let serialized = serde_json::to_string(&outcome).expect("outcome JSON");
        assert!(!serialized.contains("person@example.com"));
        let _ = ContactTargetFingerprint::clone;
        server.abort();
    }
}
