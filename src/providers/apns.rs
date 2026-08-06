use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::Client;
use reqwest::header::RETRY_AFTER;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;
use tokio::sync::Mutex;
use url::Url;

use crate::contracts::{
    ContractVersion, OutcomeClass, ProviderEnvironment, ProviderKind, PushJob, PushOutcome,
    PushPriority, PushTarget,
};
use crate::provider::{ProviderError, ProviderReadiness, PushProvider};
use crate::redaction::truncate_utf8;
use crate::retry::parse_retry_after;
use crate::validation::validate_push_job;

const PRODUCTION_API_BASE_URL: &str = "https://api.push.apple.com/";
const SANDBOX_API_BASE_URL: &str = "https://api.sandbox.push.apple.com/";
const PROVIDER_TOKEN_CACHE_SECONDS: u64 = 50 * 60;
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_APNS_PAYLOAD_BYTES: usize = 4_096;
const MAX_COLLAPSE_ID_BYTES: usize = 64;
const MAX_SAFE_DETAIL_BYTES: usize = 512;

#[derive(Debug, Error)]
pub enum ApnsConfigError {
    #[error("APNs configuration field '{0}' is required")]
    MissingField(&'static str),
    #[error("APNs configuration field '{0}' is invalid")]
    InvalidIdentifier(&'static str),
    #[error("APNs topic is invalid")]
    InvalidTopic,
    #[error("APNs .p8 private key is invalid")]
    InvalidPrivateKey,
    #[error("APNs HTTP client could not be constructed")]
    HttpClient(#[source] reqwest::Error),
}

#[derive(Clone)]
pub struct ApnsConfig {
    key_id: String,
    team_id: String,
    topic: String,
    environment: ProviderEnvironment,
    signing_key: Arc<EncodingKey>,
    api_base_url: Url,
    request_timeout: Duration,
}

impl ApnsConfig {
    pub fn new(
        key_id: impl Into<String>,
        team_id: impl Into<String>,
        topic: impl Into<String>,
        private_key_p8: &str,
        environment: ProviderEnvironment,
    ) -> Result<Self, ApnsConfigError> {
        let key_id = required(key_id.into(), "key_id")?;
        let team_id = required(team_id.into(), "team_id")?;
        let topic = required(topic.into(), "topic")?;
        validate_apple_identifier(&key_id, "key_id")?;
        validate_apple_identifier(&team_id, "team_id")?;
        validate_topic(&topic)?;
        let signing_key = EncodingKey::from_ec_pem(private_key_p8.as_bytes())
            .map_err(|_| ApnsConfigError::InvalidPrivateKey)?;
        let api_base_url = default_api_base_url(environment);

        Ok(Self {
            key_id,
            team_id,
            topic,
            environment,
            signing_key: Arc::new(signing_key),
            api_base_url,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }

    pub fn topic(&self) -> &str {
        &self.topic
    }

    pub const fn environment(&self) -> ProviderEnvironment {
        self.environment
    }

    pub fn endpoint_host(&self) -> &str {
        self.api_base_url
            .host_str()
            .expect("validated APNs base URL has a host")
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    #[cfg(test)]
    fn for_test(topic: &str, environment: ProviderEnvironment, api_base_url: Url) -> Self {
        Self {
            key_id: "KEY1234567".to_owned(),
            team_id: "TEAM123456".to_owned(),
            topic: topic.to_owned(),
            environment,
            signing_key: Arc::new(EncodingKey::from_secret(b"fixture")),
            api_base_url,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

fn required(value: String, field: &'static str) -> Result<String, ApnsConfigError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        Err(ApnsConfigError::MissingField(field))
    } else {
        Ok(value)
    }
}

fn validate_apple_identifier(value: &str, field: &'static str) -> Result<(), ApnsConfigError> {
    if value.len() != 10
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err(ApnsConfigError::InvalidIdentifier(field));
    }
    Ok(())
}

fn validate_topic(topic: &str) -> Result<(), ApnsConfigError> {
    if topic.len() > 255
        || topic.starts_with('.')
        || topic.ends_with('.')
        || topic.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        })
    {
        return Err(ApnsConfigError::InvalidTopic);
    }
    Ok(())
}

fn default_api_base_url(environment: ProviderEnvironment) -> Url {
    Url::parse(match environment {
        ProviderEnvironment::Production => PRODUCTION_API_BASE_URL,
        ProviderEnvironment::Sandbox => SANDBOX_API_BASE_URL,
    })
    .expect("constant APNs API URL is valid")
}

#[derive(Clone)]
pub struct ApnsProvider {
    config: ApnsConfig,
    client: Client,
    provider_token: Arc<Mutex<Option<CachedProviderToken>>>,
}

struct CachedProviderToken {
    value: String,
    expires_at: Instant,
}

impl ApnsProvider {
    pub fn new(config: ApnsConfig) -> Result<Self, ApnsConfigError> {
        let client = Client::builder()
            .http2_adaptive_window(true)
            .timeout(config.request_timeout)
            .build()
            .map_err(ApnsConfigError::HttpClient)?;
        Ok(Self::with_client(config, client))
    }

    pub fn with_client(config: ApnsConfig, client: Client) -> Self {
        Self {
            config,
            client,
            provider_token: Arc::new(Mutex::new(None)),
        }
    }

    async fn token(&self) -> Result<String, ProviderError> {
        let mut guard = self.provider_token.lock().await;
        if let Some(cached) = guard.as_ref()
            && cached.expires_at > Instant::now()
        {
            return Ok(cached.value.clone());
        }

        let token = encode_provider_token(&self.config, unix_now()?)?;
        *guard = Some(CachedProviderToken {
            value: token.clone(),
            expires_at: Instant::now() + Duration::from_secs(PROVIDER_TOKEN_CACHE_SECONDS),
        });
        Ok(token)
    }

    fn message_url(&self, device_token: &str) -> Result<Url, ProviderError> {
        if !valid_device_token(device_token) {
            return Err(ProviderError::delivery(
                OutcomeClass::InvalidToken,
                "APNs device token is not valid hexadecimal capability data",
                None,
                Some("invalid_device_token".to_owned()),
            ));
        }
        self.config
            .api_base_url
            .join(&format!("3/device/{device_token}"))
            .map_err(|_| ProviderError::internal("APNs message endpoint could not be constructed"))
    }

    async fn send_message(&self, job: &PushJob) -> Result<PushOutcome, ProviderError> {
        validate_job(job)?;
        let (device_token, target_environment) = apns_target(job)?;
        if target_environment != self.config.environment {
            return Err(invalid_payload(
                "APNs target environment does not match provider configuration",
                "environment_mismatch",
            ));
        }

        let request = build_request(job, unix_now()?)?;
        let provider_token = self.token().await?;
        let mut builder = self
            .client
            .post(self.message_url(device_token)?)
            .bearer_auth(provider_token)
            .header("apns-topic", &self.config.topic)
            .header("apns-push-type", request.push_type)
            .header("apns-priority", request.priority)
            .header("apns-expiration", request.expiration.to_string());
        if let Some(collapse_id) = request.collapse_id.as_deref() {
            builder = builder.header("apns-collapse-id", collapse_id);
        }
        if canonical_uuid(&job.idempotency_key) {
            builder = builder.header("apns-id", &job.idempotency_key);
        }

        let response = builder
            .json(&request.payload)
            .send()
            .await
            .map_err(|error| {
                ProviderError::delivery(
                    OutcomeClass::TransientProviderFailure,
                    format!("APNs send request failed: {error}"),
                    None,
                    Some("transport_error".to_owned()),
                )
            })?;
        let status = response.status();
        let apns_id = response
            .headers()
            .get("apns-id")
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| parse_retry_after(value, SystemTime::now()));
        let body = response.text().await.map_err(|_| {
            ProviderError::delivery(
                OutcomeClass::TransientProviderFailure,
                "APNs response could not be read",
                retry_after,
                Some("response_read_error".to_owned()),
            )
        })?;

        if status.is_success() {
            let mut outcome = PushOutcome::accepted(job);
            outcome.provider_code = apns_id;
            return Ok(outcome);
        }

        let error = serde_json::from_str::<ApnsErrorResponse>(&body).ok();
        let reason = error.as_ref().map(|error| error.reason.as_str());
        Ok(PushOutcome {
            version: ContractVersion::V1,
            job_id: job.job_id.clone(),
            provider: ProviderKind::Apns,
            target_fingerprint: job.target.fingerprint(),
            class: classify_apns_response(status.as_u16(), reason),
            provider_code: reason
                .map(ToOwned::to_owned)
                .or_else(|| Some(format!("http_{}", status.as_u16()))),
            retry_after_ms: retry_after.map(duration_to_millis),
            safe_detail: reason.map(|value| truncate_utf8(value, MAX_SAFE_DETAIL_BYTES)),
        })
    }
}

#[async_trait]
impl PushProvider for ApnsProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Apns
    }

    fn readiness(&self) -> ProviderReadiness {
        ProviderReadiness::ready()
    }

    async fn send(&self, job: &PushJob) -> Result<PushOutcome, ProviderError> {
        self.send_message(job).await
    }
}

fn validate_job(job: &PushJob) -> Result<(), ProviderError> {
    validate_push_job(job).map_err(|errors| {
        invalid_payload(
            errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; "),
            "contract_validation_failed",
        )
    })
}

fn apns_target(job: &PushJob) -> Result<(&str, ProviderEnvironment), ProviderError> {
    match &job.target {
        PushTarget::Apns { token, environment } => Ok((token.as_str(), *environment)),
        _ => Err(invalid_payload(
            "APNs provider received a non-APNs target",
            "provider_target_mismatch",
        )),
    }
}

#[derive(Serialize)]
struct ApnsClaims<'a> {
    iss: &'a str,
    iat: u64,
}

fn provider_token_header(key_id: &str) -> Header {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(key_id.to_owned());
    header
}

fn provider_token_claims(team_id: &str, now: u64) -> ApnsClaims<'_> {
    ApnsClaims {
        iss: team_id,
        iat: now,
    }
}

fn encode_provider_token(config: &ApnsConfig, now: u64) -> Result<String, ProviderError> {
    encode(
        &provider_token_header(&config.key_id),
        &provider_token_claims(&config.team_id, now),
        config.signing_key.as_ref(),
    )
    .map_err(|_| ProviderError::not_configured("APNs provider-token signing failed"))
}

fn unix_now() -> Result<u64, ProviderError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| ProviderError::internal("system clock is before the Unix epoch"))
}

struct ApnsRequest {
    payload: Value,
    push_type: &'static str,
    priority: &'static str,
    expiration: u64,
    collapse_id: Option<String>,
}

fn build_request(job: &PushJob, now: u64) -> Result<ApnsRequest, ProviderError> {
    if job.options.dry_run {
        return Err(invalid_payload(
            "APNs does not provide a validate-only delivery mode",
            "dry_run_unsupported",
        ));
    }
    if job.notification.data.contains_key("aps") {
        return Err(invalid_payload(
            "APNs custom data may not replace the reserved aps dictionary",
            "reserved_aps_key",
        ));
    }
    if job
        .options
        .collapse_key
        .as_ref()
        .is_some_and(|value| value.len() > MAX_COLLAPSE_ID_BYTES)
    {
        return Err(invalid_payload(
            "APNs collapse identifier exceeds 64 bytes",
            "collapse_id_too_long",
        ));
    }

    let visible = has_visible_alert(job);
    if job.notification.image_url.is_some() && !visible {
        return Err(invalid_payload(
            "APNs image notifications require an alert title or body",
            "image_requires_alert",
        ));
    }

    let mut aps = Map::new();
    if visible {
        let mut alert = Map::new();
        if let Some(title) = &job.notification.title {
            alert.insert("title".to_owned(), Value::String(title.clone()));
        }
        if let Some(body) = &job.notification.body {
            alert.insert("body".to_owned(), Value::String(body.clone()));
        }
        aps.insert("alert".to_owned(), Value::Object(alert));
    } else {
        aps.insert("content-available".to_owned(), Value::from(1));
    }
    if job.notification.image_url.is_some() {
        aps.insert("mutable-content".to_owned(), Value::from(1));
    }

    let mut payload = job
        .notification
        .data
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<String, Value>>();
    if let Some(image_url) = &job.notification.image_url {
        payload.insert("image_url".to_owned(), Value::String(image_url.clone()));
    }
    payload.insert("aps".to_owned(), Value::Object(aps));
    let payload = Value::Object(payload);
    let payload_size = serde_json::to_vec(&payload)
        .map_err(|_| invalid_payload("APNs payload could not be serialized", "serialization"))?
        .len();
    if payload_size > MAX_APNS_PAYLOAD_BYTES {
        return Err(invalid_payload(
            "APNs payload exceeds 4096 bytes",
            "payload_too_large",
        ));
    }

    Ok(ApnsRequest {
        payload,
        push_type: if visible { "alert" } else { "background" },
        priority: if visible && job.options.priority == PushPriority::High {
            "10"
        } else {
            "5"
        },
        expiration: job
            .options
            .ttl_seconds
            .map(|ttl| now.saturating_add(u64::from(ttl)))
            .unwrap_or(0),
        collapse_id: job.options.collapse_key.clone(),
    })
}

fn has_visible_alert(job: &PushJob) -> bool {
    job.notification
        .title
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || job
            .notification
            .body
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn invalid_payload(detail: impl AsRef<str>, code: &'static str) -> ProviderError {
    ProviderError::delivery(
        OutcomeClass::InvalidPayload,
        detail,
        None,
        Some(code.to_owned()),
    )
}

fn valid_device_token(token: &str) -> bool {
    (32..=512).contains(&token.len())
        && token.len().is_multiple_of(2)
        && token.chars().all(|character| character.is_ascii_hexdigit())
}

fn canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.char_indices().all(|(index, character)| match index {
            8 | 13 | 18 | 23 => character == '-',
            _ => character.is_ascii_hexdigit() && !character.is_ascii_uppercase(),
        })
}

#[derive(Deserialize)]
struct ApnsErrorResponse {
    reason: String,
    #[serde(rename = "timestamp")]
    _timestamp: Option<u64>,
}

fn classify_apns_response(status: u16, reason: Option<&str>) -> OutcomeClass {
    match reason {
        Some("BadDeviceToken" | "DeviceTokenNotForTopic" | "Unregistered") => {
            OutcomeClass::InvalidToken
        }
        Some(
            "BadCollapseId" | "BadExpirationDate" | "BadMessageId" | "BadPath" | "BadPriority"
            | "BadTopic" | "DuplicateHeaders" | "MethodNotAllowed" | "MissingDeviceToken"
            | "MissingTopic" | "PayloadEmpty" | "TooManyTopics",
        ) => OutcomeClass::InvalidPayload,
        Some("TooManyProviderTokenUpdates" | "TooManyRequests") => OutcomeClass::Throttled,
        Some("IdleTimeout" | "InternalServerError" | "ServiceUnavailable" | "Shutdown") => {
            OutcomeClass::TransientProviderFailure
        }
        Some(
            "BadCertificate"
            | "BadCertificateEnvironment"
            | "CertificateExpired"
            | "CertificateRevoked"
            | "ExpiredProviderToken"
            | "Forbidden"
            | "InvalidProviderToken"
            | "MissingProviderToken"
            | "TopicDisallowed",
        ) => OutcomeClass::PermanentProviderFailure,
        _ => match status {
            400 | 404 | 405 | 413 => OutcomeClass::InvalidPayload,
            410 => OutcomeClass::InvalidToken,
            429 => OutcomeClass::Throttled,
            500..=599 => OutcomeClass::TransientProviderFailure,
            _ => OutcomeClass::PermanentProviderFailure,
        },
    }
}

fn duration_to_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::json;

    use super::*;
    use crate::contracts::{Notification, PushOptions, TraceMetadata};

    fn fixture_uuid() -> String {
        ["123e4567", "e89b", "12d3", "a456", "426655440000"].join("-")
    }

    fn fixture_device_token() -> String {
        "01".repeat(32)
    }

    fn fixture_provider_token() -> String {
        ["cached", "provider", "fixture"].join("-")
    }

    fn test_job(environment: ProviderEnvironment) -> PushJob {
        PushJob {
            version: ContractVersion::V1,
            job_id: "job-1".to_owned(),
            tenant_id: "tenant-1".to_owned(),
            application_id: "app-1".to_owned(),
            idempotency_key: fixture_uuid(),
            provider: ProviderKind::Apns,
            target: PushTarget::Apns {
                token: fixture_device_token(),
                environment,
            },
            notification: Notification {
                title: Some("Hello".to_owned()),
                body: Some("World".to_owned()),
                image_url: Some("https://cdn.example.invalid/image.jpg".to_owned()),
                data: BTreeMap::from([("screen".to_owned(), json!("inbox"))]),
            },
            options: PushOptions {
                ttl_seconds: Some(120),
                priority: PushPriority::High,
                collapse_key: Some("inbox".to_owned()),
                dry_run: false,
            },
            trace: TraceMetadata::default(),
        }
    }

    #[test]
    fn validates_config_and_selects_environment_host() {
        let invalid = ApnsConfig::new(
            "short",
            "TEAM123456",
            "com.example.app",
            "not-a-key",
            ProviderEnvironment::Sandbox,
        );
        assert!(matches!(
            invalid,
            Err(ApnsConfigError::InvalidIdentifier("key_id"))
        ));
        assert_eq!(
            default_api_base_url(ProviderEnvironment::Sandbox).host_str(),
            Some("api.sandbox.push.apple.com")
        );
        assert_eq!(
            default_api_base_url(ProviderEnvironment::Production).host_str(),
            Some("api.push.apple.com")
        );
    }

    #[test]
    fn constructs_es256_header_and_team_claims() {
        let header = provider_token_header("KEY1234567");
        assert_eq!(header.alg, Algorithm::ES256);
        assert_eq!(header.kid.as_deref(), Some("KEY1234567"));
        let claims = serde_json::to_value(provider_token_claims("TEAM123456", 1_000))
            .expect("serialize claims");
        assert_eq!(claims["iss"], "TEAM123456");
        assert_eq!(claims["iat"], 1_000);
        assert!(claims.get("exp").is_none());
    }

    #[tokio::test]
    async fn cached_provider_token_is_reused() {
        let config = ApnsConfig::for_test(
            "com.example.app",
            ProviderEnvironment::Production,
            default_api_base_url(ProviderEnvironment::Production),
        );
        let provider = ApnsProvider::new(config).expect("provider");
        *provider.provider_token.lock().await = Some(CachedProviderToken {
            value: fixture_provider_token(),
            expires_at: Instant::now() + Duration::from_secs(60),
        });
        assert_eq!(
            provider.token().await.expect("cached token"),
            fixture_provider_token()
        );
    }

    #[test]
    fn builds_alert_and_background_payloads() {
        let alert = build_request(&test_job(ProviderEnvironment::Production), 1_000)
            .expect("alert request");
        assert_eq!(alert.push_type, "alert");
        assert_eq!(alert.priority, "10");
        assert_eq!(alert.expiration, 1_120);
        assert_eq!(alert.payload["aps"]["alert"]["title"], "Hello");
        assert_eq!(alert.payload["aps"]["mutable-content"], 1);
        assert_eq!(alert.payload["screen"], "inbox");

        let mut job = test_job(ProviderEnvironment::Sandbox);
        job.notification.title = None;
        job.notification.body = None;
        job.notification.image_url = None;
        let background = build_request(&job, 1_000).expect("background request");
        assert_eq!(background.push_type, "background");
        assert_eq!(background.priority, "5");
        assert_eq!(background.payload["aps"]["content-available"], 1);
    }

    #[test]
    fn rejects_reserved_aps() {
        let mut job = test_job(ProviderEnvironment::Production);
        job.notification.data.insert("aps".to_owned(), json!({}));
        let error = match build_request(&job, 1_000) {
            Err(error) => error,
            Ok(_) => panic!("reserved aps must fail"),
        };
        assert!(matches!(
            error,
            ProviderError::Delivery {
                class: OutcomeClass::InvalidPayload,
                ..
            }
        ));
    }

    #[test]
    fn classifies_apple_error_reasons() {
        assert_eq!(
            classify_apns_response(410, Some("Unregistered")),
            OutcomeClass::InvalidToken
        );
        assert_eq!(
            classify_apns_response(400, Some("BadPriority")),
            OutcomeClass::InvalidPayload
        );
        assert_eq!(
            classify_apns_response(429, Some("TooManyProviderTokenUpdates")),
            OutcomeClass::Throttled
        );
        assert_eq!(
            classify_apns_response(503, Some("Shutdown")),
            OutcomeClass::TransientProviderFailure
        );
        assert_eq!(
            classify_apns_response(403, Some("ExpiredProviderToken")),
            OutcomeClass::PermanentProviderFailure
        );
    }

    #[derive(Clone, Default)]
    struct Capture {
        headers: Arc<Mutex<Option<HeaderMap>>>,
        body: Arc<Mutex<Option<Value>>>,
        token: Arc<Mutex<Option<String>>>,
    }

    async fn capture_send(
        State(capture): State<Capture>,
        Path(token): Path<String>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> (StatusCode, [(HeaderName, HeaderValue); 1]) {
        *capture.headers.lock().await = Some(headers);
        *capture.body.lock().await = Some(body);
        *capture.token.lock().await = Some(token);
        (
            StatusCode::OK,
            [(
                HeaderName::from_static("apns-id"),
                HeaderValue::from_static("mock-apns-id"),
            )],
        )
    }

    #[tokio::test]
    async fn sends_to_mock_endpoint_with_expected_headers() {
        let capture = Capture::default();
        let app = Router::new()
            .route("/3/device/{token}", post(capture_send))
            .with_state(capture.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let address = listener.local_addr().expect("mock address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });

        let config = ApnsConfig::for_test(
            "com.example.app",
            ProviderEnvironment::Production,
            Url::parse(&format!("http://{address}/")).expect("mock URL"),
        );
        let provider = ApnsProvider::new(config).expect("provider");
        *provider.provider_token.lock().await = Some(CachedProviderToken {
            value: fixture_provider_token(),
            expires_at: Instant::now() + Duration::from_secs(60),
        });
        let outcome = provider
            .send(&test_job(ProviderEnvironment::Production))
            .await
            .expect("send succeeds");

        assert_eq!(outcome.class, OutcomeClass::Accepted);
        assert_eq!(outcome.provider_code.as_deref(), Some("mock-apns-id"));
        let headers = capture.headers.lock().await;
        let headers = headers.as_ref().expect("captured headers");
        assert_eq!(
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some(format!("Bearer {}", fixture_provider_token()).as_str())
        );
        assert_eq!(
            headers
                .get("apns-topic")
                .and_then(|value| value.to_str().ok()),
            Some("com.example.app")
        );
        assert_eq!(
            headers
                .get("apns-push-type")
                .and_then(|value| value.to_str().ok()),
            Some("alert")
        );
        assert_eq!(
            capture.token.lock().await.as_deref(),
            Some(fixture_device_token().as_str())
        );
        assert_eq!(
            capture.body.lock().await.as_ref().expect("captured body")["aps"]["alert"]["body"],
            "World"
        );

        server.abort();
    }

    #[tokio::test]
    async fn rejects_target_environment_mismatch() {
        let config = ApnsConfig::for_test(
            "com.example.app",
            ProviderEnvironment::Production,
            default_api_base_url(ProviderEnvironment::Production),
        );
        let provider = ApnsProvider::new(config).expect("provider");
        let error = provider
            .send(&test_job(ProviderEnvironment::Sandbox))
            .await
            .expect_err("environment mismatch must fail");
        assert!(matches!(
            error,
            ProviderError::Delivery {
                class: OutcomeClass::InvalidPayload,
                ..
            }
        ));
    }
}
