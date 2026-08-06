use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{sse::Event, IntoResponse, Response, Sse},
    Extension, Json, Router,
};
use futures_util::StreamExt;
use serde::Serialize;
use tokio::time::Instant;
use utoipa::{openapi::OpenApi, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    config::ServerConfig,
    docs::{self, ApiDocs, SharedApiDocs, OPENAPI_CONTENT_TYPE},
    engine::{Engine, EngineError},
    model::{
        CompleteStepRequest, ErrorResponse, FailStepRequest, LeaseCommand, MutationResponse,
        PollQuery, PollResponse, RunSnapshot, SignalRequest, SignalResponse, StepOutputRequest,
        SubmitRunRequest, SubmitRunResponse, SubmitTaskRequest, WorkerHeartbeatRequest,
        WorkerRecord, WorkerRegistration,
    },
};

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<Engine>,
    pub nats: async_nats::Client,
    pub config: Arc<ServerConfig>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfo {
    pub service: String,
    pub version: String,
    pub architecture: String,
    pub shadow_mode: bool,
    pub state_backend: String,
    pub event_journal: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessResponse {
    pub status: String,
    pub state_backend_ready: bool,
    pub shadow_mode: bool,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    body: ErrorResponse,
}

impl ApiError {
    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            body: ErrorResponse {
                code: "unauthorized".to_string(),
                message: "a valid X-Worker-Auth or X-Server-Auth header is required".to_string(),
                retryable: false,
            },
        }
    }

    fn backend(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            body: ErrorResponse {
                code: "backend_unavailable".to_string(),
                message: message.into(),
                retryable: true,
            },
        }
    }
}

impl From<EngineError> for ApiError {
    fn from(error: EngineError) -> Self {
        let (status, code, retryable) = match &error {
            EngineError::InvalidGraph(_) | EngineError::InvalidRequest(_) => {
                (StatusCode::BAD_REQUEST, "invalid_request", false)
            }
            EngineError::NotFound { .. } => (StatusCode::NOT_FOUND, "not_found", false),
            EngineError::Conflict(_) => (StatusCode::CONFLICT, "state_conflict", true),
            EngineError::IdempotencyMismatch => {
                (StatusCode::CONFLICT, "idempotency_mismatch", false)
            }
            EngineError::WorkerUnavailable(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "worker_unavailable", true)
            }
            EngineError::Store(_) => (StatusCode::SERVICE_UNAVAILABLE, "state_backend_error", true),
        };
        Self {
            status,
            body: ErrorResponse {
                code: code.to_string(),
                message: error.to_string(),
                retryable,
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

pub fn local_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(root))
        .routes(routes!(healthz))
        .routes(routes!(readyz))
        .routes(routes!(metrics))
        .routes(routes!(openapi_json))
        .routes(routes!(api_docs_json))
        .routes(routes!(api_docs_ui))
        .routes(routes!(docs_api_ui))
        .routes(routes!(internal_openapi_json))
        .routes(routes!(internal_docs_ui))
        .routes(routes!(submit_task))
        .routes(routes!(submit_run))
        .routes(routes!(get_run))
        .routes(routes!(stream_run_events))
        .routes(routes!(signal_run))
        .routes(routes!(pause_run))
        .routes(routes!(resume_run))
        .routes(routes!(cancel_run))
        .routes(routes!(register_worker))
        .routes(routes!(heartbeat_worker))
        .routes(routes!(poll_worker))
        .routes(routes!(start_step))
        .routes(routes!(heartbeat_step))
        .routes(routes!(append_step_output))
        .routes(routes!(complete_step))
        .routes(routes!(fail_step))
}

pub fn openapi_document() -> OpenApi {
    docs::finalize(local_router().into_openapi())
}

pub fn app_router(state: AppState) -> Result<Router, serde_json::Error> {
    let (router, openapi) = local_router().split_for_parts();
    let docs = Arc::new(ApiDocs::new(&docs::finalize(openapi))?);
    Ok(router.with_state(state).layer(Extension(docs)))
}

#[utoipa::path(
    get,
    path = "/",
    operation_id = "getDurableWorkerServiceInfo",
    tag = "public",
    security(()),
    responses((status = 200, description = "Service identity and architecture summary", body = ServiceInfo))
)]
pub async fn root(State(state): State<AppState>) -> Json<ServiceInfo> {
    Json(ServiceInfo {
        service: "dd-durable-worker-server".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        architecture: "JetStream KV state + CAS leases + append-only event journal".to_string(),
        shadow_mode: state.config.shadow_mode,
        state_backend: state.config.state_bucket.clone(),
        event_journal: state.config.event_stream.clone(),
    })
}

#[utoipa::path(
    get,
    path = "/healthz",
    operation_id = "getDurableWorkerHealth",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Process liveness", body = HealthResponse))
)]
pub async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        service: "dd-durable-worker-server".to_string(),
    })
}

#[utoipa::path(
    get,
    path = "/readyz",
    operation_id = "getDurableWorkerReadiness",
    tag = "operations",
    security(()),
    responses(
        (status = 200, description = "JetStream state backend is ready", body = ReadinessResponse),
        (status = 503, description = "JetStream state backend is unavailable", body = ReadinessResponse)
    )
)]
pub async fn readyz(State(state): State<AppState>) -> Response {
    let ready = state.engine.ready().await;
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(ReadinessResponse {
            status: if ready { "ready" } else { "not_ready" }.to_string(),
            state_backend_ready: ready,
            shadow_mode: state.config.shadow_mode,
        }),
    )
        .into_response()
}

#[utoipa::path(
    get,
    path = "/metrics",
    operation_id = "getDurableWorkerMetrics",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Prometheus text exposition", body = String, content_type = "text/plain"))
)]
pub async fn metrics(State(state): State<AppState>) -> Response {
    let mut body = concat!(
        "# HELP dd_durable_worker_build_info Durable worker server build metadata.\n",
        "# TYPE dd_durable_worker_build_info gauge\n",
        "dd_durable_worker_build_info{service=\"dd-durable-worker-server\"} 1\n",
        "# HELP dd_durable_worker_shadow_mode Whether the deployment is intentionally additive/shadow-first.\n",
        "# TYPE dd_durable_worker_shadow_mode gauge\n",
    )
    .to_string();
    body.push_str(&format!(
        "dd_durable_worker_shadow_mode {}\n",
        u8::from(state.config.shadow_mode)
    ));
    body.push_str(&state.engine.metrics().render_prometheus());
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

#[utoipa::path(
    get,
    path = "/openapi.json",
    operation_id = "getDurableWorkerPublicOpenApi",
    tag = "public",
    security(()),
    responses((status = 200, description = "Fail-closed public OpenAPI 3.1 document", body = String, content_type = "application/vnd.oai.openapi+json;version=3.1"))
)]
pub async fn openapi_json(Extension(docs): Extension<SharedApiDocs>) -> Response {
    openapi_response(docs.public_json.clone())
}

#[utoipa::path(
    get,
    path = "/api/docs.json",
    operation_id = "getDurableWorkerPublicOpenApiAlias",
    tag = "public",
    security(()),
    responses((status = 200, description = "Compatibility alias for the public OpenAPI 3.1 document", body = String, content_type = "application/vnd.oai.openapi+json;version=3.1"))
)]
pub async fn api_docs_json(Extension(docs): Extension<SharedApiDocs>) -> Response {
    openapi_response(docs.public_json.clone())
}

#[utoipa::path(
    get,
    path = "/api/docs",
    operation_id = "getDurableWorkerPublicDocs",
    tag = "public",
    security(()),
    responses((status = 200, description = "Interactive public API reference", body = String, content_type = "text/html"))
)]
pub async fn api_docs_ui(Extension(docs): Extension<SharedApiDocs>) -> Response {
    html_response(docs.public_html.clone())
}

#[utoipa::path(
    get,
    path = "/docs/api",
    operation_id = "getDurableWorkerPublicDocsAlias",
    tag = "public",
    security(()),
    responses((status = 200, description = "Compatibility alias for the public API reference", body = String, content_type = "text/html"))
)]
pub async fn docs_api_ui(Extension(docs): Extension<SharedApiDocs>) -> Response {
    html_response(docs.public_html.clone())
}

#[utoipa::path(
    get,
    path = "/internal/openapi.json",
    operation_id = "getDurableWorkerInternalOpenApi",
    tag = "internal-docs",
    security(("workerAuth" = [])),
    responses(
        (status = 200, description = "Complete internal OpenAPI 3.1 document", body = String, content_type = "application/vnd.oai.openapi+json;version=3.1"),
        (status = 401, description = "Missing or invalid worker secret", body = ErrorResponse)
    )
)]
pub async fn internal_openapi_json(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(docs): Extension<SharedApiDocs>,
) -> Result<Response, ApiError> {
    authorize(&headers, &state)?;
    Ok(openapi_response(docs.internal_json.clone()))
}

#[utoipa::path(
    get,
    path = "/internal/docs/api",
    operation_id = "getDurableWorkerInternalDocs",
    tag = "internal-docs",
    security(("workerAuth" = [])),
    responses(
        (status = 200, description = "Interactive complete internal API reference", body = String, content_type = "text/html"),
        (status = 401, description = "Missing or invalid worker secret", body = ErrorResponse)
    )
)]
pub async fn internal_docs_ui(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(docs): Extension<SharedApiDocs>,
) -> Result<Response, ApiError> {
    authorize(&headers, &state)?;
    Ok(html_response(docs.internal_html.clone()))
}

#[utoipa::path(
    post,
    path = "/api/v1/tasks",
    operation_id = "submitDurableTask",
    tag = "runs",
    security(("workerAuth" = [])),
    request_body = SubmitTaskRequest,
    responses(
        (status = 202, description = "One-step durable run accepted", body = SubmitRunResponse),
        (status = 400, description = "Invalid task", body = ErrorResponse),
        (status = 401, description = "Missing or invalid worker secret", body = ErrorResponse),
        (status = 409, description = "Idempotency or state conflict", body = ErrorResponse),
        (status = 503, description = "State backend unavailable", body = ErrorResponse)
    )
)]
pub async fn submit_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SubmitTaskRequest>,
) -> Result<(StatusCode, Json<SubmitRunResponse>), ApiError> {
    authorize(&headers, &state)?;
    let response = state.engine.submit_run(request.into_run()).await?;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

#[utoipa::path(
    post,
    path = "/api/v1/runs",
    operation_id = "submitDurableRun",
    tag = "runs",
    security(("workerAuth" = [])),
    request_body = SubmitRunRequest,
    responses(
        (status = 202, description = "Durable DAG run accepted", body = SubmitRunResponse),
        (status = 400, description = "Invalid DAG or request", body = ErrorResponse),
        (status = 401, description = "Missing or invalid worker secret", body = ErrorResponse),
        (status = 409, description = "Idempotency or state conflict", body = ErrorResponse),
        (status = 503, description = "State backend unavailable", body = ErrorResponse)
    )
)]
pub async fn submit_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SubmitRunRequest>,
) -> Result<(StatusCode, Json<SubmitRunResponse>), ApiError> {
    authorize(&headers, &state)?;
    let response = state.engine.submit_run(request).await?;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/runs/{run_id}",
    operation_id = "getDurableRun",
    tag = "runs",
    security(("workerAuth" = [])),
    params(("run_id" = String, Path, description = "Durable run UUID")),
    responses(
        (status = 200, description = "Run and step snapshot", body = RunSnapshot),
        (status = 401, description = "Missing or invalid worker secret", body = ErrorResponse),
        (status = 404, description = "Run not found", body = ErrorResponse)
    )
)]
pub async fn get_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Json<RunSnapshot>, ApiError> {
    authorize(&headers, &state)?;
    Ok(Json(state.engine.get_run_snapshot(&run_id).await?))
}

#[utoipa::path(
    get,
    path = "/api/v1/runs/{run_id}/events",
    operation_id = "streamDurableRunEvents",
    tag = "runs",
    security(("workerAuth" = [])),
    params(("run_id" = String, Path, description = "Durable run UUID")),
    responses(
        (status = 200, description = "Live server-sent event stream. Every event is also persisted in the JetStream event journal.", body = String, content_type = "text/event-stream"),
        (status = 401, description = "Missing or invalid worker secret", body = ErrorResponse),
        (status = 404, description = "Run not found", body = ErrorResponse),
        (status = 503, description = "NATS subscription unavailable", body = ErrorResponse)
    )
)]
pub async fn stream_run_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    authorize(&headers, &state)?;
    state.engine.get_run_snapshot(&run_id).await?;
    let subscriber = state
        .nats
        .subscribe(state.config.event_subject.replace('*', &run_id))
        .await
        .map_err(|error| ApiError::backend(format!("event subscription failed: {error}")))?;
    let stream = subscriber.map(|message| {
        let payload = String::from_utf8_lossy(&message.payload).into_owned();
        Ok::<Event, Infallible>(Event::default().event("durable-event").data(payload))
    });
    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/runs/{run_id}/signals/{signal_name}",
    operation_id = "signalDurableRun",
    tag = "runs",
    security(("workerAuth" = [])),
    params(
        ("run_id" = String, Path, description = "Durable run UUID"),
        ("signal_name" = String, Path, description = "Signal name awaited by one or more steps")
    ),
    request_body = SignalRequest,
    responses(
        (status = 200, description = "Signal durably recorded and eligible steps released", body = SignalResponse),
        (status = 400, description = "Invalid signal", body = ErrorResponse),
        (status = 401, description = "Missing or invalid worker secret", body = ErrorResponse),
        (status = 404, description = "Run not found", body = ErrorResponse),
        (status = 409, description = "Run is terminal", body = ErrorResponse)
    )
)]
pub async fn signal_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((run_id, signal_name)): Path<(String, String)>,
    Json(request): Json<SignalRequest>,
) -> Result<Json<SignalResponse>, ApiError> {
    authorize(&headers, &state)?;
    Ok(Json(
        state
            .engine
            .signal_run(&run_id, &signal_name, request.payload)
            .await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/runs/{run_id}/pause",
    operation_id = "pauseDurableRun",
    tag = "runs",
    security(("workerAuth" = [])),
    params(("run_id" = String, Path, description = "Durable run UUID")),
    responses((status = 200, description = "Run paused; active leases may finish", body = MutationResponse), (status = 401, description = "Unauthorized", body = ErrorResponse), (status = 404, description = "Run not found", body = ErrorResponse), (status = 409, description = "Terminal run conflict", body = ErrorResponse))
)]
pub async fn pause_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Json<MutationResponse>, ApiError> {
    authorize(&headers, &state)?;
    Ok(Json(state.engine.pause_run(&run_id).await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/runs/{run_id}/resume",
    operation_id = "resumeDurableRun",
    tag = "runs",
    security(("workerAuth" = [])),
    params(("run_id" = String, Path, description = "Durable run UUID")),
    responses((status = 200, description = "Run resumed", body = MutationResponse), (status = 401, description = "Unauthorized", body = ErrorResponse), (status = 404, description = "Run not found", body = ErrorResponse), (status = 409, description = "Terminal run conflict", body = ErrorResponse))
)]
pub async fn resume_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Json<MutationResponse>, ApiError> {
    authorize(&headers, &state)?;
    Ok(Json(state.engine.resume_run(&run_id).await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/runs/{run_id}/cancel",
    operation_id = "cancelDurableRun",
    tag = "runs",
    security(("workerAuth" = [])),
    params(("run_id" = String, Path, description = "Durable run UUID")),
    responses((status = 200, description = "Run and non-terminal steps cancelled", body = MutationResponse), (status = 401, description = "Unauthorized", body = ErrorResponse), (status = 404, description = "Run not found", body = ErrorResponse), (status = 409, description = "Terminal run conflict", body = ErrorResponse))
)]
pub async fn cancel_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Json<MutationResponse>, ApiError> {
    authorize(&headers, &state)?;
    Ok(Json(state.engine.cancel_run(&run_id).await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/workers/register",
    operation_id = "registerDurableWorker",
    tag = "workers",
    security(("workerAuth" = [])),
    request_body = WorkerRegistration,
    responses((status = 200, description = "Worker registered or refreshed", body = WorkerRecord), (status = 400, description = "Invalid worker registration", body = ErrorResponse), (status = 401, description = "Unauthorized", body = ErrorResponse), (status = 503, description = "State backend unavailable", body = ErrorResponse))
)]
pub async fn register_worker(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<WorkerRegistration>,
) -> Result<Json<WorkerRecord>, ApiError> {
    authorize(&headers, &state)?;
    Ok(Json(state.engine.register_worker(request).await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/workers/{worker_id}/heartbeat",
    operation_id = "heartbeatDurableWorker",
    tag = "workers",
    security(("workerAuth" = [])),
    params(("worker_id" = String, Path, description = "Stable worker instance identifier")),
    request_body = WorkerHeartbeatRequest,
    responses((status = 200, description = "Worker heartbeat recorded", body = WorkerRecord), (status = 401, description = "Unauthorized", body = ErrorResponse), (status = 404, description = "Worker not registered", body = ErrorResponse), (status = 503, description = "State backend unavailable", body = ErrorResponse))
)]
pub async fn heartbeat_worker(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(worker_id): Path<String>,
    Json(request): Json<WorkerHeartbeatRequest>,
) -> Result<Json<WorkerRecord>, ApiError> {
    authorize(&headers, &state)?;
    Ok(Json(
        state.engine.heartbeat_worker(&worker_id, request).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/workers/{worker_id}/poll",
    operation_id = "pollDurableWorker",
    tag = "workers",
    security(("workerAuth" = [])),
    params(
        ("worker_id" = String, Path, description = "Stable worker instance identifier"),
        PollQuery
    ),
    responses((status = 200, description = "An assignment or an empty long-poll response", body = PollResponse), (status = 401, description = "Unauthorized", body = ErrorResponse), (status = 404, description = "Worker not registered", body = ErrorResponse), (status = 503, description = "Worker or state backend unavailable", body = ErrorResponse))
)]
pub async fn poll_worker(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(worker_id): Path<String>,
    Query(query): Query<PollQuery>,
) -> Result<Json<PollResponse>, ApiError> {
    authorize(&headers, &state)?;
    let wait = Duration::from_millis(
        query
            .wait_ms
            .unwrap_or_default()
            .min(state.config.poll_max_wait.as_millis() as u64),
    );
    let deadline = Instant::now() + wait;
    loop {
        let response = state.engine.poll_once(&worker_id).await?;
        if response.assignment.is_some() || wait.is_zero() || Instant::now() >= deadline {
            return Ok(Json(response));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        tokio::time::sleep(Duration::from_millis(response.retry_after_ms.max(25)).min(remaining))
            .await;
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/steps/{step_id}/start",
    operation_id = "startDurableStep",
    tag = "steps",
    security(("workerAuth" = [])),
    params(("step_id" = String, Path, description = "Durable step UUID")),
    request_body = LeaseCommand,
    responses((status = 200, description = "Lease acknowledged and step marked running", body = MutationResponse), (status = 401, description = "Unauthorized", body = ErrorResponse), (status = 404, description = "Step not found", body = ErrorResponse), (status = 409, description = "Stale or expired lease", body = ErrorResponse))
)]
pub async fn start_step(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(step_id): Path<String>,
    Json(request): Json<LeaseCommand>,
) -> Result<Json<MutationResponse>, ApiError> {
    authorize(&headers, &state)?;
    Ok(Json(state.engine.start_step(&step_id, request).await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/steps/{step_id}/heartbeat",
    operation_id = "heartbeatDurableStep",
    tag = "steps",
    security(("workerAuth" = [])),
    params(("step_id" = String, Path, description = "Durable step UUID")),
    request_body = LeaseCommand,
    responses((status = 200, description = "Lease extended", body = MutationResponse), (status = 401, description = "Unauthorized", body = ErrorResponse), (status = 404, description = "Step not found", body = ErrorResponse), (status = 409, description = "Stale or expired lease", body = ErrorResponse))
)]
pub async fn heartbeat_step(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(step_id): Path<String>,
    Json(request): Json<LeaseCommand>,
) -> Result<Json<MutationResponse>, ApiError> {
    authorize(&headers, &state)?;
    Ok(Json(state.engine.heartbeat_step(&step_id, request).await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/steps/{step_id}/output",
    operation_id = "appendDurableStepOutput",
    tag = "steps",
    security(("workerAuth" = [])),
    params(("step_id" = String, Path, description = "Durable step UUID")),
    request_body = StepOutputRequest,
    responses((status = 200, description = "Output state committed and JetStream acknowledged the stable event ID", body = MutationResponse), (status = 400, description = "Invalid chunk ID, stream, payload reuse, or oversized output", body = ErrorResponse), (status = 401, description = "Unauthorized", body = ErrorResponse), (status = 404, description = "Step not found", body = ErrorResponse), (status = 409, description = "Stale or expired lease", body = ErrorResponse), (status = 503, description = "State or event journal unavailable; retry the same chunkId", body = ErrorResponse))
)]
pub async fn append_step_output(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(step_id): Path<String>,
    Json(request): Json<StepOutputRequest>,
) -> Result<Json<MutationResponse>, ApiError> {
    authorize(&headers, &state)?;
    Ok(Json(state.engine.append_output(&step_id, request).await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/steps/{step_id}/complete",
    operation_id = "completeDurableStep",
    tag = "steps",
    security(("workerAuth" = [])),
    params(("step_id" = String, Path, description = "Durable step UUID")),
    request_body = CompleteStepRequest,
    responses((status = 200, description = "Step completed and dependent DAG nodes advanced", body = MutationResponse), (status = 401, description = "Unauthorized", body = ErrorResponse), (status = 404, description = "Step not found", body = ErrorResponse), (status = 409, description = "Stale or expired lease", body = ErrorResponse), (status = 503, description = "State backend unavailable", body = ErrorResponse))
)]
pub async fn complete_step(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(step_id): Path<String>,
    Json(request): Json<CompleteStepRequest>,
) -> Result<Json<MutationResponse>, ApiError> {
    authorize(&headers, &state)?;
    Ok(Json(state.engine.complete_step(&step_id, request).await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/steps/{step_id}/fail",
    operation_id = "failDurableStep",
    tag = "steps",
    security(("workerAuth" = [])),
    params(("step_id" = String, Path, description = "Durable step UUID")),
    request_body = FailStepRequest,
    responses((status = 200, description = "Step failed or scheduled for retry", body = MutationResponse), (status = 401, description = "Unauthorized", body = ErrorResponse), (status = 404, description = "Step not found", body = ErrorResponse), (status = 409, description = "Stale or expired lease", body = ErrorResponse), (status = 503, description = "State backend unavailable", body = ErrorResponse))
)]
pub async fn fail_step(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(step_id): Path<String>,
    Json(request): Json<FailStepRequest>,
) -> Result<Json<MutationResponse>, ApiError> {
    authorize(&headers, &state)?;
    Ok(Json(state.engine.fail_step(&step_id, request).await?))
}

fn authorize(headers: &HeaderMap, state: &AppState) -> Result<(), ApiError> {
    let candidate = headers
        .get("x-worker-auth")
        .or_else(|| headers.get("x-server-auth"))
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if state.config.auth_secret.verify(candidate) {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}

fn openapi_response(bytes: bytes::Bytes) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, OPENAPI_CONTENT_TYPE)
        .body(Body::from(bytes))
        .expect("valid OpenAPI response")
}

fn html_response(bytes: bytes::Bytes) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(bytes))
        .expect("valid HTML response")
}
