use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use reqwest::header::RETRY_AFTER;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use thiserror::Error;
use url::Url;

use super::contracts::{
    ContactContent, ContactJob, ContactOutcome, ContactOutcomeClass, ContactProviderKind,
    ContactTarget,
};
use super::provider::{ContactProvider, ContactProviderError};
use super::validation::{valid_e164, validate_contact_job};
use crate::provider::ProviderReadiness;
use crate::redaction::truncate_utf8;
use crate::retry::{classify_http_status, parse_retry_after};

const DEFAULT_API_BASE: &str = "https://api.twilio.com/";
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_SAFE_DETAIL_BYTES: usize = 512;
const MAX_PROVIDER_CODE_BYTES: usize = 128;

#[derive(Debug, Clone)]
pub enum TwilioCredentials {
    AuthToken { token: String },
    ApiKey { sid: String, secret: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TwilioSender {
    MessagingService { sid: String },
    PhoneNumber { e164: String },
}

#[derive(Debug, Error)]
pub enum TwilioConfigError {
    #[error("TWILIO_ACCOUNT_SID must be AC followed by 32 hexadecimal characters")]
    InvalidAccountSid,

    #[error("Twilio Auth Token must contain 16 to 512 non-control characters")]
    InvalidAuthToken,

    #[error("TWILIO_API_KEY_SID must be SK followed by 32 hexadecimal characters")]
    InvalidApiKeySid,

    #[error("Twilio API Key secret must contain 16 to 512 non-control characters")]
    InvalidApiKeySecret,

    #[error("TWILIO_MESSAGING_SERVICE_SID must be MG followed by 32 hexadecimal characters")]
    InvalidMessagingServiceSid,

    #[error("TWILIO_FROM_NUMBER must be an E.164 number")]
    InvalidFromNumber,

    #[error("TWILIO_STATUS_CALLBACK_URL must be HTTPS without credentials or a fragment")]
    InvalidStatusCallbackUrl,

    #[error("TWILIO_VALIDITY_PERIOD_SECONDS must be between 1 and 36000")]
    InvalidValidityPeriod,

    #[error("Twilio API base URL must be HTTPS without embedded credentials")]
    InvalidApiBaseUrl,

    #[error("Twilio HTTP client could not be constructed")]
    HttpClient(#[source] reqwest::Error),
}

#[derive(Debug, Clone)]
pub struct TwilioConfig {
    account_sid: String,
    credentials: TwilioCredentials,
    sender: TwilioSender,
    status_callback_url: Option<Url>,
    validity_period_seconds: Option<u32>,
    api_base_url: Url,
    request_timeout: Duration,
}

impl TwilioConfig {
    pub fn new(
        account_sid: impl Into<String>,
        credentials: TwilioCredentials,
        sender: TwilioSender,
    ) -> Result<Self, TwilioConfigError> {
        let account_sid = account_sid.into();
        let account_sid = account_sid.trim().to_owned();
        if !valid_twilio_sid(&account_sid, "AC") {
            return Err(TwilioConfigError::InvalidAccountSid);
        }
        validate_credentials(&credentials)?;
        validate_sender(&sender)?;
        Ok(Self {
            account_sid,
            credentials,
            sender,
            status_callback_url: None,
            validity_period_seconds: None,
            api_base_url: Url::parse(DEFAULT_API_BASE).expect("constant Twilio API URL is valid"),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }

    pub fn with_status_callback_url(mut self, url: Url) -> Result<Self, TwilioConfigError> {
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(TwilioConfigError::InvalidStatusCallbackUrl);
        }
        self.status_callback_url = Some(url);
        Ok(self)
    }

    pub fn with_validity_period_seconds(
        mut self,
        validity_period_seconds: u32,
    ) -> Result<Self, TwilioConfigError> {
        if !(1..=36_000).contains(&validity_period_seconds) {
            return Err(TwilioConfigError::InvalidValidityPeriod);
        }
        self.validity_period_seconds = Some(validity_period_seconds);
        Ok(self)
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub fn with_api_base_url(mut self, api_base_url: Url) -> Result<Self, TwilioConfigError> {
        validate_api_base_url(&api_base_url)?;
        self.api_base_url = api_base_url;
        Ok(self)
    }
}

fn validate_credentials(credentials: &TwilioCredentials) -> Result<(), TwilioConfigError> {
    match credentials {
        TwilioCredentials::AuthToken { token } => {
            if !valid_secret(token) {
                return Err(TwilioConfigError::InvalidAuthToken);
            }
        }
        TwilioCredentials::ApiKey { sid, secret } => {
            if !valid_twilio_sid(sid, "SK") {
                return Err(TwilioConfigError::InvalidApiKeySid);
            }
            if !valid_secret(secret) {
                return Err(TwilioConfigError::InvalidApiKeySecret);
            }
        }
    }
    Ok(())
}

fn validate_sender(sender: &TwilioSender) -> Result<(), TwilioConfigError> {
    match sender {
        TwilioSender::MessagingService { sid } if !valid_twilio_sid(sid, "MG") => {
            Err(TwilioConfigError::InvalidMessagingServiceSid)
        }
        TwilioSender::PhoneNumber { e164 } if !valid_e164(e164) => {
            Err(TwilioConfigError::InvalidFromNumber)
        }
        _ => Ok(()),
    }
}

fn valid_twilio_sid(value: &str, prefix: &str) -> bool {
    value.len() == 34
        && value.starts_with(prefix)
        && value[prefix.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

fn valid_secret(value: &str) -> bool {
    (16..=512).contains(&value.len()) && !value.chars().any(char::is_control)
}

fn validate_api_base_url(url: &Url) -> Result<(), TwilioConfigError> {
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(TwilioConfigError::InvalidApiBaseUrl);
    }
    Ok(())
}

#[derive(Clone)]
pub struct TwilioProvider {
    config: TwilioConfig,
    client: Client,
}

impl TwilioProvider {
    pub fn new(config: TwilioConfig) -> Result<Self, TwilioConfigError> {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(TwilioConfigError::HttpClient)?;
        Ok(Self { config, client })
    }

    pub fn with_client(config: TwilioConfig, client: Client) -> Self {
        Self { config, client }
    }

    fn messages_url(&self) -> Result<Url, ContactProviderError> {
        self.config
            .api_base_url
            .join(&format!(
                "2010-04-01/Accounts/{}/Messages.json",
                self.config.account_sid
            ))
            .map_err(|_| ContactProviderError::internal("Twilio endpoint could not be constructed"))
    }

    async fn send_sms(&self, job: &ContactJob) -> Result<ContactOutcome, ContactProviderError> {
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

        let form = build_twilio_form(job, &self.config)?;
        let mut request = self.client.post(self.messages_url()?).form(&form);
        request = match &self.config.credentials {
            TwilioCredentials::AuthToken { token } => {
                request.basic_auth(&self.config.account_sid, Some(token))
            }
            TwilioCredentials::ApiKey { sid, secret } => request.basic_auth(sid, Some(secret)),
        };
        let response = request.send().await.map_err(|error| {
            ContactProviderError::delivery(
                ContactOutcomeClass::TransientProviderFailure,
                format!("Twilio request failed: {error}"),
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
        let response_text = response.text().await.unwrap_or_default();
        let parsed = serde_json::from_str::<TwilioMessageResponse>(&response_text).ok();

        if status.is_success() {
            if let Some(error_code) = parsed.as_ref().and_then(|document| document.error_code) {
                return Ok(outcome_for_twilio_error(
                    job,
                    status,
                    Some(error_code),
                    retry_after,
                ));
            }
            let provider_code = parsed
                .as_ref()
                .and_then(|document| document.sid.as_deref())
                .and_then(valid_provider_code);
            return Ok(ContactOutcome::accepted(job, provider_code));
        }

        let code = parsed.as_ref().and_then(|document| document.code);
        Ok(outcome_for_twilio_error(job, status, code, retry_after))
    }
}

#[async_trait]
impl ContactProvider for TwilioProvider {
    fn kind(&self) -> ContactProviderKind {
        ContactProviderKind::Twilio
    }

    fn readiness(&self) -> ProviderReadiness {
        ProviderReadiness::ready()
    }

    async fn send(&self, job: &ContactJob) -> Result<ContactOutcome, ContactProviderError> {
        self.send_sms(job).await
    }
}

fn build_twilio_form(
    job: &ContactJob,
    config: &TwilioConfig,
) -> Result<BTreeMap<String, String>, ContactProviderError> {
    let e164 = match &job.target {
        ContactTarget::Sms { e164 } => e164,
        _ => {
            return Err(ContactProviderError::delivery(
                ContactOutcomeClass::InvalidPayload,
                "Twilio provider received a non-SMS target",
                None,
                Some("provider_target_mismatch".to_owned()),
            ));
        }
    };
    let ContactContent::Sms { body } = &job.content else {
        return Err(ContactProviderError::delivery(
            ContactOutcomeClass::InvalidPayload,
            "Twilio provider received non-SMS content",
            None,
            Some("provider_content_mismatch".to_owned()),
        ));
    };

    let mut form = BTreeMap::from([
        ("To".to_owned(), e164.clone()),
        ("Body".to_owned(), body.clone()),
    ]);
    match &config.sender {
        TwilioSender::MessagingService { sid } => {
            form.insert("MessagingServiceSid".to_owned(), sid.clone());
        }
        TwilioSender::PhoneNumber { e164 } => {
            form.insert("From".to_owned(), e164.clone());
        }
    }
    if let Some(url) = &config.status_callback_url {
        form.insert("StatusCallback".to_owned(), url.as_str().to_owned());
    }
    if let Some(validity_period_seconds) = config.validity_period_seconds {
        form.insert(
            "ValidityPeriod".to_owned(),
            validity_period_seconds.to_string(),
        );
    }
    Ok(form)
}

#[derive(Debug, Deserialize)]
struct TwilioMessageResponse {
    sid: Option<String>,
    code: Option<u32>,
    error_code: Option<u32>,
}

fn outcome_for_twilio_error(
    job: &ContactJob,
    status: StatusCode,
    error_code: Option<u32>,
    retry_after: Option<Duration>,
) -> ContactOutcome {
    let decision = classify_http_status(status.as_u16(), retry_after);
    let class = match error_code {
        Some(21_211 | 21_612 | 21_614 | 21_610 | 30_003 | 30_005 | 30_006) => {
            ContactOutcomeClass::InvalidTarget
        }
        Some(21_602 | 21_617) => ContactOutcomeClass::InvalidPayload,
        Some(20_003 | 20_404 | 21_606 | 21_608) => ContactOutcomeClass::PermanentProviderFailure,
        _ if status == StatusCode::TOO_MANY_REQUESTS => ContactOutcomeClass::Throttled,
        _ if decision.retryable => ContactOutcomeClass::TransientProviderFailure,
        _ if status == StatusCode::BAD_REQUEST => ContactOutcomeClass::InvalidPayload,
        _ => ContactOutcomeClass::PermanentProviderFailure,
    };
    let provider_code = error_code
        .map(|code| format!("twilio_{code}"))
        .or_else(|| Some(format!("http_{}", status.as_u16())));
    let safe_detail = match error_code {
        Some(code) => format!(
            "Twilio rejected the request with provider code {code} and HTTP {}",
            status.as_u16()
        ),
        None => format!("Twilio rejected the request with HTTP {}", status.as_u16()),
    };
    ContactOutcome {
        version: job.version,
        job_id: job.job_id.clone(),
        provider: ContactProviderKind::Twilio,
        target_fingerprint: job.target.fingerprint(),
        class,
        provider_code,
        retry_after_ms: retry_after.map(duration_to_millis),
        safe_detail: Some(truncate_utf8(&safe_detail, MAX_SAFE_DETAIL_BYTES)),
    }
}

fn valid_provider_code(value: &str) -> Option<String> {
    if value.len() > MAX_PROVIDER_CODE_BYTES
        || value.len() < 2
        || value
            .chars()
            .any(|character| !character.is_ascii_alphanumeric())
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
    use std::sync::Arc;

    use axum::extract::{Form, State};
    use axum::http::{HeaderMap, StatusCode as AxumStatusCode};
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::json;
    use tokio::sync::Mutex;

    use super::*;
    use crate::contracts::{ContractVersion, TraceMetadata};

    fn fixture_sid(prefix: &str) -> String {
        format!("{prefix}{}", "0".repeat(32))
    }

    fn sms_job() -> ContactJob {
        ContactJob {
            version: ContractVersion::V1,
            job_id: "job-1".to_owned(),
            tenant_id: "tenant-1".to_owned(),
            application_id: "app-1".to_owned(),
            idempotency_key: "event-1".to_owned(),
            provider: ContactProviderKind::Twilio,
            target: ContactTarget::Sms {
                e164: "+15551234567".to_owned(),
            },
            content: ContactContent::Sms {
                body: "Hello".to_owned(),
            },
            trace: TraceMetadata::default(),
        }
    }

    #[test]
    fn supports_messaging_services_status_callbacks_and_validity_periods() {
        let messaging_service_sid = fixture_sid("MG");
        let config = TwilioConfig::new(
            fixture_sid("AC"),
            TwilioCredentials::AuthToken {
                token: "fixture-auth-token-that-is-long-enough".to_owned(),
            },
            TwilioSender::MessagingService {
                sid: messaging_service_sid.clone(),
            },
        )
        .expect("config")
        .with_status_callback_url(
            Url::parse("https://callbacks.example.com/twilio/status").expect("callback URL"),
        )
        .expect("callback")
        .with_validity_period_seconds(600)
        .expect("validity period");
        let form = build_twilio_form(&sms_job(), &config).expect("form");
        assert_eq!(
            form.get("MessagingServiceSid").map(String::as_str),
            Some(messaging_service_sid.as_str())
        );
        assert_eq!(form.get("ValidityPeriod").map(String::as_str), Some("600"));
        assert_eq!(
            form.get("StatusCallback").map(String::as_str),
            Some("https://callbacks.example.com/twilio/status")
        );
        assert!(!form.contains_key("From"));
    }

    #[test]
    fn classifies_invalid_targets_and_throttling_without_recipient_leakage() {
        let job = sms_job();
        let invalid = outcome_for_twilio_error(&job, StatusCode::BAD_REQUEST, Some(21_211), None);
        assert_eq!(invalid.class, ContactOutcomeClass::InvalidTarget);
        assert!(
            !serde_json::to_string(&invalid)
                .expect("JSON")
                .contains("+15551234567")
        );

        let throttled = outcome_for_twilio_error(
            &job,
            StatusCode::TOO_MANY_REQUESTS,
            None,
            Some(Duration::from_secs(3)),
        );
        assert_eq!(throttled.class, ContactOutcomeClass::Throttled);
        assert_eq!(throttled.retry_after_ms, Some(3_000));
    }

    #[derive(Clone, Default)]
    struct Capture {
        authorization: Arc<Mutex<Option<String>>>,
        form: Arc<Mutex<Option<BTreeMap<String, String>>>>,
    }

    async fn capture_send(
        State(capture): State<Capture>,
        headers: HeaderMap,
        Form(form): Form<BTreeMap<String, String>>,
    ) -> (AxumStatusCode, Json<serde_json::Value>) {
        *capture.authorization.lock().await = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        *capture.form.lock().await = Some(form);
        (
            AxumStatusCode::CREATED,
            Json(json!({
                "sid": fixture_sid("SM"),
                "status": "queued",
                "error_code": null
            })),
        )
    }

    #[tokio::test]
    async fn sends_to_mock_endpoint_with_basic_auth_and_redacted_outcome() {
        let capture = Capture::default();
        let app = Router::new()
            .route(
                "/2010-04-01/Accounts/{account_sid}/Messages.json",
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

        let mut config = TwilioConfig::new(
            fixture_sid("AC"),
            TwilioCredentials::ApiKey {
                sid: fixture_sid("SK"),
                secret: "fixture-api-key-secret-that-is-long-enough".to_owned(),
            },
            TwilioSender::MessagingService {
                sid: fixture_sid("MG"),
            },
        )
        .expect("config");
        config.api_base_url = Url::parse(&format!("http://{address}/")).expect("mock URL");
        let provider = TwilioProvider::new(config).expect("provider");
        let job = sms_job();
        let outcome = provider.send(&job).await.expect("send succeeds");
        assert_eq!(outcome.class, ContactOutcomeClass::Accepted);
        let expected_message_sid = fixture_sid("SM");
        assert_eq!(
            outcome.provider_code.as_deref(),
            Some(expected_message_sid.as_str())
        );
        assert!(
            capture
                .authorization
                .lock()
                .await
                .as_deref()
                .is_some_and(|value| value.starts_with("Basic "))
        );
        assert_eq!(
            capture
                .form
                .lock()
                .await
                .as_ref()
                .and_then(|form| form.get("To"))
                .map(String::as_str),
            Some("+15551234567")
        );
        assert!(
            !serde_json::to_string(&outcome)
                .expect("outcome JSON")
                .contains("+15551234567")
        );
        server.abort();
    }
}
