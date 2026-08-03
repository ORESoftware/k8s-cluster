use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

#[cfg(test)]
use super::contracts::ContactProviderKind;
use super::contracts::{ContactJob, ContactOutcome, ContactOutcomeClass};
use super::dispatch::{ContactProviderRegistry, ContactRegistryReadiness};
use super::provider::ContactProviderError;
use super::validation::validate_contact_job;
use crate::contracts::ContractVersion;
use crate::http_api::RequestAuthenticator;
use crate::redaction::truncate_utf8;

pub(crate) const MAX_CONTACT_HTTP_BODY_BYTES: usize = 768 * 1024;
const MAX_CONTACT_BATCH_JOBS: usize = 100;
const MAX_SAFE_DETAIL_BYTES: usize = 512;

#[derive(Clone)]
pub struct ContactApiState {
    registry: ContactProviderRegistry,
    authenticator: Arc<dyn RequestAuthenticator>,
}

impl ContactApiState {
    pub fn new(
        registry: ContactProviderRegistry,
        authenticator: Arc<dyn RequestAuthenticator>,
    ) -> Self {
        Self {
            registry,
            authenticator,
        }
    }
}

pub(crate) fn contact_openapi_router() -> OpenApiRouter<ContactApiState> {
    OpenApiRouter::new()
        .routes(routes!(readyz))
        .routes(routes!(submit_job))
        .routes(routes!(submit_batch))
}

pub fn contact_router(state: ContactApiState) -> Router {
    let (router, _) = contact_openapi_router().split_for_parts();
    router
        .layer(DefaultBodyLimit::max(MAX_CONTACT_HTTP_BODY_BYTES))
        .with_state(state)
}

#[utoipa::path(
    get,
    path = "/v1/contact/readyz",
    operation_id = "getContactServiceReadiness",
    tag = "operations",
    security(()),
    responses(
        (status = 200, description = "Authentication and at least one contact provider are configured", body = ContactReadinessResponse),
        (status = 503, description = "Authentication or contact providers are unavailable", body = ContactReadinessResponse)
    )
)]
async fn readyz(State(state): State<ContactApiState>) -> Response {
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
        Json(ContactReadinessResponse {
            ok: status == StatusCode::OK,
            authentication_configured: auth_configured,
            authentication_mode: state.authenticator.mode(),
            providers,
        }),
    )
        .into_response()
}

#[utoipa::path(
    post,
    path = "/v1/contact/jobs",
    operation_id = "submitContactJob",
    tag = "contact",
    request_body = ContactJob,
    security(("bearer_auth" = [])),
    responses(
        (status = 202, description = "Provider accepted the contact job", body = ContactOutcome),
        (status = 400, description = "Contract validation failed", body = ContactErrorEnvelope),
        (status = 401, description = "Authentication failed", body = ContactErrorEnvelope),
        (status = 422, description = "Provider rejected the target", body = ContactOutcome),
        (status = 429, description = "Provider throttled the request", body = ContactOutcome),
        (status = 502, description = "Permanent provider failure", body = ContactOutcome),
        (status = 503, description = "Transient or internal provider failure", body = ContactOutcome)
    )
)]
async fn submit_job(
    State(state): State<ContactApiState>,
    headers: HeaderMap,
    Json(job): Json<ContactJob>,
) -> Response {
    if !state.authenticator.authenticate(&headers) {
        return unauthorized();
    }
    let outcome = dispatch_job(&state.registry, &job).await;
    (status_for_outcome(outcome.class), Json(outcome)).into_response()
}

#[utoipa::path(
    post,
    path = "/v1/contact/jobs/batch",
    operation_id = "submitContactBatch",
    tag = "contact",
    request_body = ContactBatchRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 202, description = "Every job was accepted", body = ContactBatchResponse),
        (status = 207, description = "Batch contains mixed outcomes", body = ContactBatchResponse),
        (status = 400, description = "Batch size is invalid", body = ContactErrorEnvelope),
        (status = 401, description = "Authentication failed", body = ContactErrorEnvelope)
    )
)]
async fn submit_batch(
    State(state): State<ContactApiState>,
    headers: HeaderMap,
    Json(request): Json<ContactBatchRequest>,
) -> Response {
    if !state.authenticator.authenticate(&headers) {
        return unauthorized();
    }
    if request.jobs.is_empty() || request.jobs.len() > MAX_CONTACT_BATCH_JOBS {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_batch_size",
            format!("batch must contain 1 to {MAX_CONTACT_BATCH_JOBS} jobs"),
        );
    }

    let mut outcomes = Vec::with_capacity(request.jobs.len());
    for job in &request.jobs {
        outcomes.push(dispatch_job(&state.registry, job).await);
    }
    let accepted = outcomes
        .iter()
        .filter(|outcome| outcome.class == ContactOutcomeClass::Accepted)
        .count();
    let status = if accepted == outcomes.len() {
        StatusCode::ACCEPTED
    } else {
        StatusCode::MULTI_STATUS
    };
    (
        status,
        Json(ContactBatchResponse {
            accepted,
            rejected: outcomes.len() - accepted,
            outcomes,
        }),
    )
        .into_response()
}

async fn dispatch_job(registry: &ContactProviderRegistry, job: &ContactJob) -> ContactOutcome {
    if let Err(errors) = validate_contact_job(job) {
        return ContactOutcome {
            version: ContractVersion::V1,
            job_id: job.job_id.clone(),
            provider: job.provider,
            target_fingerprint: job.target.fingerprint(),
            class: ContactOutcomeClass::InvalidPayload,
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

fn outcome_from_provider_error(job: &ContactJob, error: ContactProviderError) -> ContactOutcome {
    let (class, safe_detail, retry_after, provider_code) = match error {
        ContactProviderError::NotConfigured { safe_reason } => (
            ContactOutcomeClass::PermanentProviderFailure,
            safe_reason,
            None,
            Some("provider_not_configured".to_owned()),
        ),
        ContactProviderError::Delivery {
            class,
            safe_detail,
            retry_after,
            provider_code,
        } => (class, safe_detail, retry_after, provider_code),
        ContactProviderError::Internal { safe_detail } => (
            ContactOutcomeClass::InternalFailure,
            safe_detail,
            None,
            Some("internal_provider_error".to_owned()),
        ),
    };
    ContactOutcome {
        version: job.version,
        job_id: job.job_id.clone(),
        provider: job.provider,
        target_fingerprint: job.target.fingerprint(),
        class,
        provider_code,
        retry_after_ms: retry_after.map(duration_to_millis),
        safe_detail: Some(truncate_utf8(&safe_detail, MAX_SAFE_DETAIL_BYTES)),
    }
}

fn status_for_outcome(class: ContactOutcomeClass) -> StatusCode {
    match class {
        ContactOutcomeClass::Accepted => StatusCode::ACCEPTED,
        ContactOutcomeClass::InvalidPayload => StatusCode::BAD_REQUEST,
        ContactOutcomeClass::InvalidTarget => StatusCode::UNPROCESSABLE_ENTITY,
        ContactOutcomeClass::Throttled => StatusCode::TOO_MANY_REQUESTS,
        ContactOutcomeClass::TransientProviderFailure | ContactOutcomeClass::InternalFailure => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        ContactOutcomeClass::PermanentProviderFailure => StatusCode::BAD_GATEWAY,
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
        Json(ContactErrorEnvelope {
            error: ContactErrorBody {
                code,
                safe_detail: truncate_utf8(detail.as_ref(), MAX_SAFE_DETAIL_BYTES),
            },
        }),
    )
        .into_response()
}

fn duration_to_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[derive(Debug, Serialize, ToSchema)]
struct ContactReadinessResponse {
    ok: bool,
    authentication_configured: bool,
    #[schema(value_type = String)]
    authentication_mode: &'static str,
    providers: ContactRegistryReadiness,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ContactBatchRequest {
    pub jobs: Vec<ContactJob>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ContactBatchResponse {
    pub accepted: usize,
    pub rejected: usize,
    pub outcomes: Vec<ContactOutcome>,
}

#[derive(Debug, Serialize, ToSchema)]
struct ContactErrorEnvelope {
    error: ContactErrorBody,
}

#[derive(Debug, Serialize, ToSchema)]
struct ContactErrorBody {
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
    use crate::contact::contracts::{ContactContent, ContactTarget};
    use crate::contact::provider::ContactProvider;
    use crate::contracts::TraceMetadata;
    use crate::http_api::{DenyAllAuthenticator, SharedSecretAuthenticator};
    use crate::provider::ProviderReadiness;

    struct AcceptingProvider;

    #[async_trait]
    impl ContactProvider for AcceptingProvider {
        fn kind(&self) -> ContactProviderKind {
            ContactProviderKind::Sendgrid
        }

        fn readiness(&self) -> ProviderReadiness {
            ProviderReadiness::ready()
        }

        async fn send(&self, job: &ContactJob) -> Result<ContactOutcome, ContactProviderError> {
            Ok(ContactOutcome::accepted(job, Some("message-1".to_owned())))
        }
    }

    fn job() -> ContactJob {
        ContactJob {
            version: ContractVersion::V1,
            job_id: "job-1".to_owned(),
            tenant_id: "tenant-1".to_owned(),
            application_id: "app-1".to_owned(),
            idempotency_key: "event-1".to_owned(),
            provider: ContactProviderKind::Sendgrid,
            target: ContactTarget::Email {
                address: "person@example.com".to_owned(),
                name: None,
            },
            content: ContactContent::Email {
                subject: Some("Hello".to_owned()),
                text: Some("World".to_owned()),
                html: None,
                template_id: None,
                dynamic_template_data: BTreeMap::new(),
                reply_to: None,
            },
            trace: TraceMetadata::default(),
        }
    }

    fn authenticated_router() -> Router {
        let registry = ContactProviderRegistry::new().with_provider(Arc::new(AcceptingProvider));
        let auth = SharedSecretAuthenticator::new("x".repeat(32)).expect("auth");
        contact_router(ContactApiState::new(registry, Arc::new(auth)))
    }

    #[tokio::test]
    async fn contact_submission_fails_closed_without_authentication() {
        let app = contact_router(ContactApiState::new(
            ContactProviderRegistry::new(),
            Arc::new(DenyAllAuthenticator),
        ));
        let response = app
            .oneshot(
                Request::post("/v1/contact/jobs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&job()).expect("job JSON")))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_submission_dispatches_and_redacts_recipient() {
        let app = authenticated_router();
        let response = app
            .oneshot(
                Request::post("/v1/contact/jobs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {}", "x".repeat(32)))
                    .body(Body::from(serde_json::to_vec(&job()).expect("job JSON")))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        let value: Value = serde_json::from_slice(&body).expect("response JSON");
        assert_eq!(value["provider_code"], "message-1");
        assert!(!String::from_utf8_lossy(&body).contains("person@example.com"));
    }

    #[tokio::test]
    async fn invalid_jobs_return_bounded_safe_errors() {
        let mut invalid = job();
        invalid.target = ContactTarget::Sms {
            e164: "+15551234567".to_owned(),
        };
        let app = authenticated_router();
        let response = app
            .oneshot(
                Request::post("/v1/contact/jobs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {}", "x".repeat(32)))
                    .body(Body::from(serde_json::to_vec(&invalid).expect("job JSON")))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        assert!(!String::from_utf8_lossy(&body).contains("+15551234567"));
        let value: Value = serde_json::from_slice(&body).expect("response JSON");
        assert_eq!(value["provider_code"], json!("contract_validation_failed"));
    }
}
