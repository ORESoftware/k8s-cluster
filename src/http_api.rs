use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

#[cfg(test)]
use crate::contracts::ProviderKind;
use crate::contracts::{ContractVersion, OutcomeClass, PushJob, PushOutcome};
use crate::dispatch::{ProviderRegistry, RegistryReadiness};
use crate::provider::ProviderError;
use crate::redaction::truncate_utf8;
use crate::validation::validate_push_job;

pub(crate) const MAX_HTTP_BODY_BYTES: usize = 512 * 1024;
const MAX_BATCH_JOBS: usize = 100;
const MAX_SAFE_DETAIL_BYTES: usize = 512;

pub trait RequestAuthenticator: Send + Sync {
    fn configured(&self) -> bool;
    fn mode(&self) -> &'static str;
    fn authenticate(&self, headers: &HeaderMap) -> bool;
}

#[derive(Debug, Default)]
pub struct DenyAllAuthenticator;

impl RequestAuthenticator for DenyAllAuthenticator {
    fn configured(&self) -> bool {
        false
    }

    fn mode(&self) -> &'static str {
        "disabled"
    }

    fn authenticate(&self, _headers: &HeaderMap) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct SharedSecretAuthenticator {
    secret: Arc<[u8]>,
}

impl SharedSecretAuthenticator {
    pub fn new(secret: impl Into<String>) -> Result<Self, AuthConfigError> {
        let secret = secret.into();
        let secret = secret.trim();
        if secret.len() < 32 || secret.len() > 4096 || secret.chars().any(char::is_control) {
            return Err(AuthConfigError::InvalidSharedSecret);
        }
        Ok(Self {
            secret: Arc::from(secret.as_bytes()),
        })
    }
}

impl RequestAuthenticator for SharedSecretAuthenticator {
    fn configured(&self) -> bool {
        true
    }

    fn mode(&self) -> &'static str {
        "migration_shared_secret"
    }

    fn authenticate(&self, headers: &HeaderMap) -> bool {
        let Some(value) = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
        else {
            return false;
        };
        constant_time_eq(value.as_bytes(), &self.secret)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthConfigError {
    #[error("migration shared secret must contain 32 to 4096 non-control characters")]
    InvalidSharedSecret,
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

#[derive(Clone)]
pub struct ApiState {
    registry: ProviderRegistry,
    authenticator: Arc<dyn RequestAuthenticator>,
}

impl ApiState {
    pub fn new(registry: ProviderRegistry, authenticator: Arc<dyn RequestAuthenticator>) -> Self {
        Self {
            registry,
            authenticator,
        }
    }
}

pub(crate) fn push_openapi_router() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .routes(routes!(healthz))
        .routes(routes!(readyz))
        .routes(routes!(submit_job))
        .routes(routes!(submit_batch))
}

pub fn router(state: ApiState) -> Router {
    let (router, _) = push_openapi_router().split_for_parts();
    router
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
}

#[utoipa::path(
    get,
    path = "/healthz",
    operation_id = "getPushServiceHealth",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Process liveness", body = ServiceHealth))
)]
async fn healthz() -> Json<ServiceHealth> {
    Json(ServiceHealth {
        ok: true,
        service: "push-notification-server",
    })
}

#[utoipa::path(
    get,
    path = "/readyz",
    operation_id = "getPushServiceReadiness",
    tag = "operations",
    security(()),
    responses(
        (status = 200, description = "Authentication and at least one push provider are configured", body = ReadinessResponse),
        (status = 503, description = "Authentication or push providers are unavailable", body = ReadinessResponse)
    )
)]
async fn readyz(State(state): State<ApiState>) -> Response {
    let providers = state.registry.readiness();
    let auth_configured = state.authenticator.configured();
    let provider_configured = state.registry.has_configured_provider();
    let status = if auth_configured && provider_configured {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(ReadinessResponse {
            ok: status == StatusCode::OK,
            authentication: AuthenticationReadiness {
                configured: auth_configured,
                mode: state.authenticator.mode(),
            },
            providers,
        }),
    )
        .into_response()
}

#[utoipa::path(
    post,
    path = "/v1/push/jobs",
    operation_id = "submitPushJob",
    tag = "push",
    request_body = PushJob,
    security(("bearer_auth" = [])),
    responses(
        (status = 202, description = "Provider accepted the push job", body = PushOutcome),
        (status = 400, description = "Contract validation failed", body = ErrorEnvelope),
        (status = 401, description = "Authentication failed", body = ErrorEnvelope),
        (status = 422, description = "Provider rejected the target", body = PushOutcome),
        (status = 429, description = "Provider throttled the request", body = PushOutcome),
        (status = 502, description = "Permanent provider failure", body = PushOutcome),
        (status = 503, description = "Transient or internal provider failure", body = PushOutcome)
    )
)]
async fn submit_job(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(job): Json<PushJob>,
) -> Response {
    if !state.authenticator.authenticate(&headers) {
        return unauthorized();
    }

    let outcome = dispatch_job(&state.registry, &job).await;
    (status_for_outcome(outcome.class), Json(outcome)).into_response()
}

#[utoipa::path(
    post,
    path = "/v1/push/jobs/batch",
    operation_id = "submitPushBatch",
    tag = "push",
    request_body = BatchRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 202, description = "Every job was accepted", body = BatchResponse),
        (status = 207, description = "Batch contains mixed outcomes", body = BatchResponse),
        (status = 400, description = "Batch size is invalid", body = ErrorEnvelope),
        (status = 401, description = "Authentication failed", body = ErrorEnvelope)
    )
)]
async fn submit_batch(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<BatchRequest>,
) -> Response {
    if !state.authenticator.authenticate(&headers) {
        return unauthorized();
    }
    if request.jobs.is_empty() || request.jobs.len() > MAX_BATCH_JOBS {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_batch_size",
            format!("batch must contain 1 to {MAX_BATCH_JOBS} jobs"),
        );
    }

    let mut outcomes = Vec::with_capacity(request.jobs.len());
    for job in &request.jobs {
        outcomes.push(dispatch_job(&state.registry, job).await);
    }
    let accepted = outcomes
        .iter()
        .filter(|outcome| outcome.class == OutcomeClass::Accepted)
        .count();
    let status = if accepted == outcomes.len() {
        StatusCode::ACCEPTED
    } else {
        StatusCode::MULTI_STATUS
    };
    (
        status,
        Json(BatchResponse {
            accepted,
            rejected: outcomes.len() - accepted,
            outcomes,
        }),
    )
        .into_response()
}

async fn dispatch_job(registry: &ProviderRegistry, job: &PushJob) -> PushOutcome {
    if let Err(errors) = validate_push_job(job) {
        return PushOutcome {
            version: ContractVersion::V1,
            job_id: job.job_id.clone(),
            provider: job.provider,
            target_fingerprint: job.target.fingerprint(),
            class: OutcomeClass::InvalidPayload,
            provider_code: Some("contract_validation_failed".to_owned()),
            retry_after_ms: None,
            safe_detail: Some(truncate_utf8(
                &errors
                    .into_iter()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
                MAX_SAFE_DETAIL_BYTES,
            )),
        };
    }

    match registry.dispatch(job).await {
        Ok(outcome) => outcome,
        Err(error) => outcome_from_provider_error(job, error),
    }
}

fn outcome_from_provider_error(job: &PushJob, error: ProviderError) -> PushOutcome {
    let (class, safe_detail, retry_after, provider_code) = match error {
        ProviderError::NotConfigured { safe_reason } => (
            OutcomeClass::PermanentProviderFailure,
            safe_reason,
            None,
            Some("provider_not_configured".to_owned()),
        ),
        ProviderError::Delivery {
            class,
            safe_detail,
            retry_after,
            provider_code,
        } => (class, safe_detail, retry_after, provider_code),
        ProviderError::Internal { safe_detail } => (
            OutcomeClass::InternalFailure,
            safe_detail,
            None,
            Some("internal_provider_error".to_owned()),
        ),
    };
    PushOutcome {
        version: ContractVersion::V1,
        job_id: job.job_id.clone(),
        provider: job.provider,
        target_fingerprint: job.target.fingerprint(),
        class,
        provider_code,
        retry_after_ms: retry_after.map(duration_to_millis),
        safe_detail: Some(truncate_utf8(&safe_detail, MAX_SAFE_DETAIL_BYTES)),
    }
}

fn duration_to_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn status_for_outcome(class: OutcomeClass) -> StatusCode {
    match class {
        OutcomeClass::Accepted => StatusCode::ACCEPTED,
        OutcomeClass::InvalidPayload => StatusCode::BAD_REQUEST,
        OutcomeClass::InvalidToken => StatusCode::UNPROCESSABLE_ENTITY,
        OutcomeClass::Throttled => StatusCode::TOO_MANY_REQUESTS,
        OutcomeClass::TransientProviderFailure | OutcomeClass::InternalFailure => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        OutcomeClass::PermanentProviderFailure => StatusCode::BAD_GATEWAY,
    }
}

fn unauthorized() -> Response {
    api_error(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "request authentication failed",
    )
}

fn api_error(status: StatusCode, code: &'static str, detail: impl AsRef<str>) -> Response {
    (
        status,
        Json(ErrorEnvelope {
            error: ErrorBody {
                code,
                safe_detail: truncate_utf8(detail.as_ref(), MAX_SAFE_DETAIL_BYTES),
            },
        }),
    )
        .into_response()
}

#[derive(Debug, Serialize, ToSchema)]
struct ServiceHealth {
    ok: bool,
    #[schema(value_type = String)]
    service: &'static str,
}

#[derive(Debug, Serialize, ToSchema)]
struct ReadinessResponse {
    ok: bool,
    authentication: AuthenticationReadiness,
    providers: RegistryReadiness,
}

#[derive(Debug, Serialize, ToSchema)]
struct AuthenticationReadiness {
    configured: bool,
    #[schema(value_type = String)]
    mode: &'static str,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BatchRequest {
    pub jobs: Vec<PushJob>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BatchResponse {
    pub accepted: usize,
    pub rejected: usize,
    pub outcomes: Vec<PushOutcome>,
}

#[derive(Debug, Serialize, ToSchema)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, Serialize, ToSchema)]
struct ErrorBody {
    #[schema(value_type = String)]
    code: &'static str,
    safe_detail: String,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use async_trait::async_trait;
    use axum::body::to_bytes;
    use axum::http::{Request, header};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::contracts::{Notification, PushOptions, PushTarget, TraceMetadata};
    use crate::dispatch::ProviderSlot;
    use crate::provider::{ProviderReadiness, PushProvider};

    struct AcceptingProvider;

    #[async_trait]
    impl PushProvider for AcceptingProvider {
        fn kind(&self) -> ProviderKind {
            ProviderKind::Fcm
        }

        fn readiness(&self) -> ProviderReadiness {
            ProviderReadiness::ready()
        }

        async fn send(&self, job: &PushJob) -> Result<PushOutcome, ProviderError> {
            Ok(PushOutcome::accepted(job))
        }
    }

    fn job() -> PushJob {
        PushJob {
            version: ContractVersion::V1,
            job_id: "job-1".to_owned(),
            tenant_id: "tenant-1".to_owned(),
            application_id: "app-1".to_owned(),
            idempotency_key: "event-1".to_owned(),
            provider: ProviderKind::Fcm,
            target: PushTarget::Fcm {
                token: "fixture-token-123".to_owned(),
            },
            notification: Notification {
                title: Some("Hello".to_owned()),
                body: None,
                image_url: None,
                data: BTreeMap::from([("count".to_owned(), json!(1))]),
            },
            options: PushOptions::default(),
            trace: TraceMetadata::default(),
        }
    }

    fn authenticated_router() -> Router {
        let registry = ProviderRegistry::new()
            .with_provider(ProviderSlot::Fcm, Arc::new(AcceptingProvider))
            .expect("registry");
        let auth = SharedSecretAuthenticator::new("x".repeat(32)).expect("auth");
        router(ApiState::new(registry, Arc::new(auth)))
    }

    #[tokio::test]
    async fn fails_closed_without_auth_configuration() {
        let app = router(ApiState::new(
            ProviderRegistry::new(),
            Arc::new(DenyAllAuthenticator),
        ));
        let response = app
            .oneshot(
                Request::post("/v1/push/jobs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&job()).expect("job JSON")))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticates_and_dispatches_a_job() {
        let response = authenticated_router()
            .oneshot(
                Request::post("/v1/push/jobs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(AUTHORIZATION, format!("Bearer {}", "x".repeat(32)))
                    .body(Body::from(serde_json::to_vec(&job()).expect("job JSON")))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let outcome: Value = serde_json::from_slice(&body).expect("outcome");
        assert_eq!(outcome["class"], "accepted");
    }

    #[tokio::test]
    async fn validation_failures_do_not_echo_capability_tokens() {
        let mut invalid = job();
        invalid.notification = Notification::default();
        let secret_token = match &invalid.target {
            PushTarget::Fcm { token } => token.clone(),
            _ => unreachable!(),
        };
        let response = authenticated_router()
            .oneshot(
                Request::post("/v1/push/jobs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(AUTHORIZATION, format!("Bearer {}", "x".repeat(32)))
                    .body(Body::from(
                        serde_json::to_vec(&invalid).expect("invalid job JSON"),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let text = String::from_utf8(body.to_vec()).expect("UTF-8");
        assert!(!text.contains(&secret_token));
    }

    #[tokio::test]
    async fn rejects_oversized_batches_before_dispatch() {
        let response = authenticated_router()
            .oneshot(
                Request::post("/v1/push/jobs/batch")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(AUTHORIZATION, format!("Bearer {}", "x".repeat(32)))
                    .body(Body::from(
                        serde_json::to_vec(&json!({"jobs": vec![job(); MAX_BATCH_JOBS + 1]}))
                            .expect("batch JSON"),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
