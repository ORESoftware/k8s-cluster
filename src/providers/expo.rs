use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use reqwest::header::RETRY_AFTER;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{Map, Value};
use thiserror::Error;
use url::Url;

use crate::contracts::{
    ContractVersion, OutcomeClass, ProviderKind, PushJob, PushOutcome, PushPriority, PushTarget,
};
use crate::provider::{ProviderError, ProviderReadiness, PushProvider};
use crate::redaction::{TargetFingerprint, truncate_utf8};
use crate::retry::{classify_http_status, parse_retry_after};
use crate::validation::validate_push_job;

const DEFAULT_SEND_URL: &str = "https://exp.host/--/api/v2/push/send";
const DEFAULT_RECEIPTS_URL: &str = "https://exp.host/--/api/v2/push/getReceipts";
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_BATCH_MESSAGES: usize = 100;
const MAX_RECEIPT_IDS: usize = 1_000;
const MAX_SAFE_DETAIL_BYTES: usize = 512;
const MAX_TICKET_ID_BYTES: usize = 256;

#[derive(Debug, Error)]
pub enum ExpoConfigError {
    #[error("Expo endpoint must be an HTTPS URL without embedded credentials")]
    InvalidEndpoint,

    #[error("Expo HTTP client could not be constructed")]
    HttpClient(#[source] reqwest::Error),
}

#[derive(Clone)]
pub struct ExpoConfig {
    access_token: Option<String>,
    send_url: Url,
    receipts_url: Url,
    request_timeout: Duration,
}

impl ExpoConfig {
    pub fn new(access_token: Option<String>) -> Result<Self, ExpoConfigError> {
        Ok(Self {
            access_token: access_token.filter(|value| !value.trim().is_empty()),
            send_url: validate_endpoint(DEFAULT_SEND_URL)?,
            receipts_url: validate_endpoint(DEFAULT_RECEIPTS_URL)?,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub fn access_token_configured(&self) -> bool {
        self.access_token.is_some()
    }

    #[cfg(test)]
    fn for_test(send_url: Url, receipts_url: Url, access_token: Option<String>) -> Self {
        Self {
            access_token,
            send_url,
            receipts_url,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

fn validate_endpoint(value: &str) -> Result<Url, ExpoConfigError> {
    let url = Url::parse(value).map_err(|_| ExpoConfigError::InvalidEndpoint)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ExpoConfigError::InvalidEndpoint);
    }
    Ok(url)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpoReceiptRequest {
    pub job_id: String,
    pub target_fingerprint: TargetFingerprint,
    pub ticket_id: String,
}

impl ExpoReceiptRequest {
    pub fn new(
        job_id: impl Into<String>,
        target_fingerprint: TargetFingerprint,
        ticket_id: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        let ticket_id = ticket_id.into();
        validate_ticket_id(&ticket_id)?;
        Ok(Self {
            job_id: job_id.into(),
            target_fingerprint,
            ticket_id,
        })
    }

    pub fn from_ticket(job: &PushJob, ticket: &PushOutcome) -> Result<Self, ProviderError> {
        if ticket.class != OutcomeClass::Accepted || ticket.provider != ProviderKind::Expo {
            return Err(invalid_payload(
                "Expo receipt requests require an accepted Expo push ticket",
                "invalid_ticket_outcome",
            ));
        }
        let ticket_id = ticket.provider_code.clone().ok_or_else(|| {
            invalid_payload(
                "Expo accepted ticket omitted its receipt ID",
                "missing_ticket_id",
            )
        })?;
        Self::new(job.job_id.clone(), job.target.fingerprint(), ticket_id)
    }
}

#[derive(Clone)]
pub struct ExpoProvider {
    config: ExpoConfig,
    client: Client,
}

impl ExpoProvider {
    pub fn new(config: ExpoConfig) -> Result<Self, ExpoConfigError> {
        let client = Client::builder()
            .http2_adaptive_window(true)
            .timeout(config.request_timeout)
            .build()
            .map_err(ExpoConfigError::HttpClient)?;
        Ok(Self::with_client(config, client))
    }

    pub fn with_client(config: ExpoConfig, client: Client) -> Self {
        Self { config, client }
    }

    pub async fn send_batch(&self, jobs: &[PushJob]) -> Result<Vec<PushOutcome>, ProviderError> {
        validate_batch(jobs)?;
        let messages = jobs
            .iter()
            .map(build_message)
            .collect::<Result<Vec<_>, _>>()?;

        let mut request = self
            .client
            .post(self.config.send_url.clone())
            .json(&messages);
        if let Some(access_token) = &self.config.access_token {
            request = request.bearer_auth(access_token);
        }
        let response = request.send().await.map_err(|error| {
            ProviderError::delivery(
                OutcomeClass::TransientProviderFailure,
                format!("Expo push-ticket request failed: {error}"),
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
        let body = response.text().await.map_err(|_| {
            ProviderError::delivery(
                OutcomeClass::TransientProviderFailure,
                "Expo push-ticket response could not be read",
                retry_after,
                Some("response_read_error".to_owned()),
            )
        })?;

        if !status.is_success() {
            return Err(classify_request_failure(status, &body, retry_after));
        }

        let document: ExpoSendResponse = serde_json::from_str(&body).map_err(|_| {
            ProviderError::delivery(
                OutcomeClass::PermanentProviderFailure,
                "Expo returned a non-JSON success response",
                None,
                Some("invalid_success_response".to_owned()),
            )
        })?;
        if let Some(error) = first_request_error(document.errors.as_deref()) {
            return Err(error);
        }
        let tickets = document.data.ok_or_else(|| {
            ProviderError::delivery(
                OutcomeClass::PermanentProviderFailure,
                "Expo success response omitted push tickets",
                None,
                Some("missing_tickets".to_owned()),
            )
        })?;
        if tickets.len() != jobs.len() {
            return Err(ProviderError::internal(format!(
                "Expo returned {} tickets for {} messages",
                tickets.len(),
                jobs.len()
            )));
        }

        Ok(jobs
            .iter()
            .zip(tickets)
            .map(|(job, ticket)| outcome_from_ticket(job, ticket))
            .collect())
    }

    pub async fn get_receipts(
        &self,
        requests: &[ExpoReceiptRequest],
    ) -> Result<Vec<PushOutcome>, ProviderError> {
        if requests.is_empty() || requests.len() > MAX_RECEIPT_IDS {
            return Err(invalid_payload(
                format!("Expo receipt lookup requires 1 to {MAX_RECEIPT_IDS} ticket IDs"),
                "invalid_receipt_batch_size",
            ));
        }
        for request in requests {
            validate_ticket_id(&request.ticket_id)?;
        }

        let body = Value::Object(Map::from_iter([(
            "ids".to_owned(),
            Value::Array(
                requests
                    .iter()
                    .map(|request| Value::String(request.ticket_id.clone()))
                    .collect(),
            ),
        )]));
        let mut http_request = self
            .client
            .post(self.config.receipts_url.clone())
            .json(&body);
        if let Some(access_token) = &self.config.access_token {
            http_request = http_request.bearer_auth(access_token);
        }
        let response = http_request.send().await.map_err(|error| {
            ProviderError::delivery(
                OutcomeClass::TransientProviderFailure,
                format!("Expo receipt request failed: {error}"),
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
        let response_body = response.text().await.map_err(|_| {
            ProviderError::delivery(
                OutcomeClass::TransientProviderFailure,
                "Expo receipt response could not be read",
                retry_after,
                Some("response_read_error".to_owned()),
            )
        })?;
        if !status.is_success() {
            return Err(classify_request_failure(
                status,
                &response_body,
                retry_after,
            ));
        }

        let document: ExpoReceiptResponse = serde_json::from_str(&response_body).map_err(|_| {
            ProviderError::delivery(
                OutcomeClass::PermanentProviderFailure,
                "Expo returned a non-JSON receipt response",
                None,
                Some("invalid_receipt_response".to_owned()),
            )
        })?;
        if let Some(error) = first_request_error(document.errors.as_deref()) {
            return Err(error);
        }
        let receipts = document.data.unwrap_or_default();
        Ok(requests
            .iter()
            .map(|request| match receipts.get(&request.ticket_id) {
                Some(receipt) => outcome_from_receipt(request, receipt),
                None => PushOutcome {
                    version: ContractVersion::V1,
                    job_id: request.job_id.clone(),
                    provider: ProviderKind::Expo,
                    target_fingerprint: request.target_fingerprint.clone(),
                    class: OutcomeClass::TransientProviderFailure,
                    provider_code: Some(request.ticket_id.clone()),
                    retry_after_ms: None,
                    safe_detail: Some("Expo receipt is not available yet".to_owned()),
                },
            })
            .collect())
    }
}

#[async_trait]
impl PushProvider for ExpoProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Expo
    }

    fn readiness(&self) -> ProviderReadiness {
        ProviderReadiness::ready()
    }

    async fn send(&self, job: &PushJob) -> Result<PushOutcome, ProviderError> {
        let mut outcomes = self.send_batch(std::slice::from_ref(job)).await?;
        outcomes
            .pop()
            .ok_or_else(|| ProviderError::internal("Expo single-send returned no outcome"))
    }
}

fn validate_batch(jobs: &[PushJob]) -> Result<(), ProviderError> {
    if jobs.is_empty() || jobs.len() > MAX_BATCH_MESSAGES {
        return Err(invalid_payload(
            format!("Expo batches require 1 to {MAX_BATCH_MESSAGES} messages"),
            "invalid_batch_size",
        ));
    }
    let application_id = &jobs[0].application_id;
    for job in jobs {
        validate_push_job(job).map_err(|errors| {
            invalid_payload(
                errors
                    .into_iter()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
                "contract_validation_failed",
            )
        })?;
        if job.provider != ProviderKind::Expo || !matches!(job.target, PushTarget::Expo { .. }) {
            return Err(invalid_payload(
                "Expo batch contains a non-Expo provider or target",
                "provider_target_mismatch",
            ));
        }
        if &job.application_id != application_id {
            return Err(invalid_payload(
                "Expo batches may contain messages for only one application/project",
                "mixed_applications",
            ));
        }
        if job.options.dry_run {
            return Err(invalid_payload(
                "Expo does not provide a validate-only delivery mode",
                "dry_run_unsupported",
            ));
        }
    }
    Ok(())
}

fn build_message(job: &PushJob) -> Result<Value, ProviderError> {
    let token = match &job.target {
        PushTarget::Expo { token } => token,
        _ => {
            return Err(invalid_payload(
                "Expo provider received a non-Expo target",
                "provider_target_mismatch",
            ));
        }
    };
    if !valid_expo_push_token(token) {
        return Err(ProviderError::delivery(
            OutcomeClass::InvalidToken,
            "Expo push token has an invalid shape",
            None,
            Some("invalid_expo_push_token".to_owned()),
        ));
    }

    let mut message = Map::new();
    message.insert("to".to_owned(), Value::String(token.clone()));
    if let Some(title) = &job.notification.title {
        message.insert("title".to_owned(), Value::String(title.clone()));
    }
    if let Some(body) = &job.notification.body {
        message.insert("body".to_owned(), Value::String(body.clone()));
    }
    if !job.notification.data.is_empty() {
        message.insert(
            "data".to_owned(),
            Value::Object(
                job.notification
                    .data
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        );
    }
    if let Some(image_url) = &job.notification.image_url {
        message.insert(
            "richContent".to_owned(),
            Value::Object(Map::from_iter([(
                "image".to_owned(),
                Value::String(image_url.clone()),
            )])),
        );
    }
    if let Some(ttl_seconds) = job.options.ttl_seconds {
        message.insert("ttl".to_owned(), Value::from(ttl_seconds));
    }
    message.insert(
        "priority".to_owned(),
        Value::String(match job.options.priority {
            PushPriority::Normal => "normal".to_owned(),
            PushPriority::High => "high".to_owned(),
        }),
    );
    if let Some(collapse_key) = &job.options.collapse_key {
        message.insert("collapseId".to_owned(), Value::String(collapse_key.clone()));
    }
    Ok(Value::Object(message))
}

fn valid_expo_push_token(token: &str) -> bool {
    let inner = token
        .strip_prefix("ExpoPushToken[")
        .or_else(|| token.strip_prefix("ExponentPushToken["))
        .and_then(|value| value.strip_suffix(']'));
    inner.is_some_and(|value| {
        (8..=256).contains(&value.len())
            && value.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
    })
}

fn validate_ticket_id(ticket_id: &str) -> Result<(), ProviderError> {
    if ticket_id.is_empty()
        || ticket_id.len() > MAX_TICKET_ID_BYTES
        || ticket_id
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(invalid_payload(
            "Expo ticket ID has an invalid shape",
            "invalid_ticket_id",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct ExpoSendResponse {
    data: Option<Vec<ExpoTicket>>,
    errors: Option<Vec<ExpoRequestError>>,
}

#[derive(Debug, Deserialize)]
struct ExpoTicket {
    status: String,
    id: Option<String>,
    message: Option<String>,
    details: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ExpoReceiptResponse {
    data: Option<BTreeMap<String, ExpoReceipt>>,
    errors: Option<Vec<ExpoRequestError>>,
}

#[derive(Debug, Deserialize)]
struct ExpoReceipt {
    status: String,
    message: Option<String>,
    details: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ExpoRequestError {
    code: Option<String>,
    message: Option<String>,
}

fn outcome_from_ticket(job: &PushJob, ticket: ExpoTicket) -> PushOutcome {
    if ticket.status == "ok" {
        return match ticket.id {
            Some(ticket_id) if validate_ticket_id(&ticket_id).is_ok() => PushOutcome {
                version: ContractVersion::V1,
                job_id: job.job_id.clone(),
                provider: ProviderKind::Expo,
                target_fingerprint: job.target.fingerprint(),
                class: OutcomeClass::Accepted,
                provider_code: Some(ticket_id),
                retry_after_ms: None,
                safe_detail: None,
            },
            _ => internal_outcome(job, "Expo accepted ticket omitted a valid receipt ID"),
        };
    }
    let code = expo_error_code(ticket.details.as_ref());
    outcome_for_expo_error(
        job.job_id.clone(),
        job.target.fingerprint(),
        code,
        ticket.message.as_deref(),
        None,
    )
}

fn outcome_from_receipt(request: &ExpoReceiptRequest, receipt: &ExpoReceipt) -> PushOutcome {
    if receipt.status == "ok" {
        return PushOutcome {
            version: ContractVersion::V1,
            job_id: request.job_id.clone(),
            provider: ProviderKind::Expo,
            target_fingerprint: request.target_fingerprint.clone(),
            class: OutcomeClass::Accepted,
            provider_code: Some(request.ticket_id.clone()),
            retry_after_ms: None,
            safe_detail: None,
        };
    }
    outcome_for_expo_error(
        request.job_id.clone(),
        request.target_fingerprint.clone(),
        expo_error_code(receipt.details.as_ref()),
        receipt.message.as_deref(),
        Some(request.ticket_id.clone()),
    )
}

fn outcome_for_expo_error(
    job_id: String,
    target_fingerprint: TargetFingerprint,
    code: Option<&str>,
    message: Option<&str>,
    fallback_provider_code: Option<String>,
) -> PushOutcome {
    let class = match code {
        Some("DeviceNotRegistered") => OutcomeClass::InvalidToken,
        Some("MessageTooBig") => OutcomeClass::InvalidPayload,
        Some("MessageRateExceeded") => OutcomeClass::Throttled,
        Some("MismatchSenderId" | "InvalidCredentials") => OutcomeClass::PermanentProviderFailure,
        _ => OutcomeClass::InvalidPayload,
    };
    PushOutcome {
        version: ContractVersion::V1,
        job_id,
        provider: ProviderKind::Expo,
        target_fingerprint,
        class,
        provider_code: code.map(ToOwned::to_owned).or(fallback_provider_code),
        retry_after_ms: None,
        safe_detail: message.map(|value| truncate_utf8(value, MAX_SAFE_DETAIL_BYTES)),
    }
}

fn expo_error_code(details: Option<&Value>) -> Option<&str> {
    details
        .and_then(|value| value.get("error"))
        .and_then(Value::as_str)
}

fn internal_outcome(job: &PushJob, detail: &str) -> PushOutcome {
    PushOutcome {
        version: ContractVersion::V1,
        job_id: job.job_id.clone(),
        provider: ProviderKind::Expo,
        target_fingerprint: job.target.fingerprint(),
        class: OutcomeClass::InternalFailure,
        provider_code: Some("invalid_ticket_response".to_owned()),
        retry_after_ms: None,
        safe_detail: Some(truncate_utf8(detail, MAX_SAFE_DETAIL_BYTES)),
    }
}

fn first_request_error(errors: Option<&[ExpoRequestError]>) -> Option<ProviderError> {
    let error = errors?.first()?;
    let code = error.code.as_deref().unwrap_or("request_error");
    let class = classify_expo_code(code);
    Some(ProviderError::delivery(
        class,
        error
            .message
            .as_deref()
            .unwrap_or("Expo rejected the request"),
        None,
        Some(code.to_owned()),
    ))
}

fn classify_request_failure(
    status: StatusCode,
    body: &str,
    retry_after: Option<Duration>,
) -> ProviderError {
    let parsed = serde_json::from_str::<ExpoSendResponse>(body).ok();
    if let Some(error) = parsed
        .as_ref()
        .and_then(|document| document.errors.as_deref())
        .and_then(|errors| first_request_error(Some(errors)))
    {
        return error;
    }
    let decision = classify_http_status(status.as_u16(), retry_after);
    let class = if status == StatusCode::TOO_MANY_REQUESTS {
        OutcomeClass::Throttled
    } else if decision.retryable {
        OutcomeClass::TransientProviderFailure
    } else if matches!(
        status,
        StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE
    ) {
        OutcomeClass::InvalidPayload
    } else {
        OutcomeClass::PermanentProviderFailure
    };
    ProviderError::delivery(
        class,
        format!("Expo request failed with HTTP {}", status.as_u16()),
        decision.retry_after,
        Some(format!("http_{}", status.as_u16())),
    )
}

fn classify_expo_code(code: &str) -> OutcomeClass {
    match code {
        "TOO_MANY_REQUESTS" => OutcomeClass::Throttled,
        "PUSH_TOO_MANY_EXPERIENCE_IDS"
        | "PUSH_TOO_MANY_NOTIFICATIONS"
        | "PUSH_TOO_MANY_RECEIPTS" => OutcomeClass::InvalidPayload,
        "UNAUTHORIZED" => OutcomeClass::PermanentProviderFailure,
        _ => OutcomeClass::PermanentProviderFailure,
    }
}

fn invalid_payload(detail: impl AsRef<str>, code: &'static str) -> ProviderError {
    ProviderError::delivery(
        OutcomeClass::InvalidPayload,
        detail,
        None,
        Some(code.to_owned()),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode as AxumStatusCode};
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::json;
    use tokio::sync::Mutex;

    use super::*;
    use crate::contracts::{Notification, PushOptions, TraceMetadata};

    fn expo_token(suffix: &str) -> String {
        format!("ExpoPushToken[fixture_{suffix}]")
    }

    fn test_job(suffix: &str) -> PushJob {
        PushJob {
            version: ContractVersion::V1,
            job_id: format!("job-{suffix}"),
            tenant_id: "tenant-1".to_owned(),
            application_id: "app-1".to_owned(),
            idempotency_key: format!("event-{suffix}"),
            provider: ProviderKind::Expo,
            target: PushTarget::Expo {
                token: expo_token(suffix),
            },
            notification: Notification {
                title: Some("Hello".to_owned()),
                body: Some("World".to_owned()),
                image_url: Some("https://cdn.example.invalid/image.jpg".to_owned()),
                data: BTreeMap::from([("count".to_owned(), json!(3))]),
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
    fn validates_expo_token_shapes() {
        assert!(valid_expo_push_token(&expo_token("abcdefgh")));
        assert!(valid_expo_push_token("ExponentPushToken[abcdefgh]"));
        assert!(!valid_expo_push_token("ExpoPushToken[bad space]"));
        assert!(!valid_expo_push_token("https://example.invalid/token"));
    }

    #[test]
    fn builds_message_fields() {
        let message = build_message(&test_job("one")).expect("message");
        assert_eq!(message["to"], expo_token("one"));
        assert_eq!(message["title"], "Hello");
        assert_eq!(message["body"], "World");
        assert_eq!(message["data"]["count"], 3);
        assert_eq!(
            message["richContent"]["image"],
            "https://cdn.example.invalid/image.jpg"
        );
        assert_eq!(message["ttl"], 120);
        assert_eq!(message["priority"], "high");
        assert_eq!(message["collapseId"], "inbox");
    }

    #[test]
    fn parses_http_200_ticket_errors() {
        let job = test_job("one");
        let ticket: ExpoTicket = serde_json::from_value(json!({
            "status": "error",
            "message": "device is no longer registered",
            "details": {"error": "DeviceNotRegistered"}
        }))
        .expect("ticket");
        let outcome = outcome_from_ticket(&job, ticket);
        assert_eq!(outcome.class, OutcomeClass::InvalidToken);
        assert_eq!(
            outcome.provider_code.as_deref(),
            Some("DeviceNotRegistered")
        );
    }

    #[test]
    fn classifies_receipt_errors() {
        for (code, expected) in [
            ("DeviceNotRegistered", OutcomeClass::InvalidToken),
            ("MessageTooBig", OutcomeClass::InvalidPayload),
            ("MessageRateExceeded", OutcomeClass::Throttled),
            ("MismatchSenderId", OutcomeClass::PermanentProviderFailure),
            ("InvalidCredentials", OutcomeClass::PermanentProviderFailure),
        ] {
            let request =
                ExpoReceiptRequest::new("job-1", test_job("one").target.fingerprint(), "ticket-1")
                    .expect("request");
            let receipt: ExpoReceipt = serde_json::from_value(json!({
                "status": "error",
                "message": "fixture error",
                "details": {"error": code}
            }))
            .expect("receipt");
            assert_eq!(outcome_from_receipt(&request, &receipt).class, expected);
        }
    }

    #[derive(Clone, Default)]
    struct Capture {
        authorization: Arc<Mutex<Option<String>>>,
        send_body: Arc<Mutex<Option<Value>>>,
        receipt_body: Arc<Mutex<Option<Value>>>,
    }

    async fn send_handler(
        State(capture): State<Capture>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        *capture.authorization.lock().await = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        *capture.send_body.lock().await = Some(body);
        Json(json!({
            "data": [
                {"status": "ok", "id": "ticket-one"},
                {"status": "error", "message": "not registered", "details": {"error": "DeviceNotRegistered"}}
            ]
        }))
    }

    async fn receipt_handler(
        State(capture): State<Capture>,
        Json(body): Json<Value>,
    ) -> (AxumStatusCode, Json<Value>) {
        *capture.receipt_body.lock().await = Some(body);
        (
            AxumStatusCode::OK,
            Json(json!({
                "data": {
                    "ticket-one": {"status": "ok"},
                    "ticket-two": {"status": "error", "message": "too fast", "details": {"error": "MessageRateExceeded"}}
                }
            })),
        )
    }

    #[tokio::test]
    async fn sends_batches_and_processes_receipts_with_optional_auth() {
        let capture = Capture::default();
        let app = Router::new()
            .route("/send", post(send_handler))
            .route("/receipts", post(receipt_handler))
            .with_state(capture.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let address = listener.local_addr().expect("mock address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });

        let config = ExpoConfig::for_test(
            Url::parse(&format!("http://{address}/send")).expect("send URL"),
            Url::parse(&format!("http://{address}/receipts")).expect("receipt URL"),
            Some("fixture-access".to_owned()),
        );
        let provider = ExpoProvider::new(config).expect("provider");
        let jobs = vec![test_job("one"), test_job("two")];
        let tickets = provider.send_batch(&jobs).await.expect("tickets");
        assert_eq!(tickets[0].class, OutcomeClass::Accepted);
        assert_eq!(tickets[0].provider_code.as_deref(), Some("ticket-one"));
        assert_eq!(tickets[1].class, OutcomeClass::InvalidToken);
        assert_eq!(
            capture.authorization.lock().await.as_deref(),
            Some("Bearer fixture-access")
        );
        assert_eq!(
            capture.send_body.lock().await.as_ref().expect("send body")[0]["to"],
            expo_token("one")
        );

        let receipt_requests = vec![
            ExpoReceiptRequest::new("job-one", jobs[0].target.fingerprint(), "ticket-one")
                .expect("receipt one"),
            ExpoReceiptRequest::new("job-two", jobs[1].target.fingerprint(), "ticket-two")
                .expect("receipt two"),
        ];
        let receipts = provider
            .get_receipts(&receipt_requests)
            .await
            .expect("receipts");
        assert_eq!(receipts[0].class, OutcomeClass::Accepted);
        assert_eq!(receipts[1].class, OutcomeClass::Throttled);
        assert_eq!(
            capture
                .receipt_body
                .lock()
                .await
                .as_ref()
                .expect("receipt body")["ids"],
            json!(["ticket-one", "ticket-two"])
        );

        server.abort();
    }
}
