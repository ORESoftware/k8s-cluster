use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path as AxumPath, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
#[path = "../executor_router.rs"]
mod executor_router;
use executor_router::{
    bounded_text, digest_eq, materialize_executors, namespace_build_id, parse_executor_specs,
    parse_namespaced_build_id, validate_build_request, validate_secret_root, Executor, Provider,
    MAX_ERROR_CHARS_DEFAULT, MAX_EXECUTORS_DEFAULT, MAX_REQUEST_BYTES_DEFAULT, MAX_SECRET_BYTES,
    MIN_SECRET_BYTES, ROUTER_SERVICE_NAME,
};
use serde_json::{json, Value};
use tokio::{net::TcpListener, time::Duration};
use tracing::info;

const DEFAULT_PORT: u16 = 8126;
const DEFAULT_SECRET_ROOT: &str = "/var/run/secrets/gha-executor-router";

#[derive(Clone)]
struct Config {
    host: String,
    port: u16,
    inbound_auth: String,
    execution_enabled: bool,
    executor_specs: usize,
    executors: Vec<Executor>,
    max_request_bytes: usize,
    max_upstream_body_bytes: usize,
    max_error_chars: usize,
    probe_timeout: Duration,
    upstream_timeout: Duration,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let secret_root = PathBuf::from(
            env_optional("GHA_EXECUTOR_ROUTER_SECRET_ROOT")
                .unwrap_or_else(|| DEFAULT_SECRET_ROOT.to_string()),
        );
        validate_secret_root(&secret_root)?;
        let max_executors = env_usize("GHA_EXECUTOR_ROUTER_MAX_EXECUTORS", MAX_EXECUTORS_DEFAULT)?;
        let raw_specs = env_required("GHA_EXECUTOR_ROUTER_EXECUTORS_JSON")?;
        let specs = parse_executor_specs(&raw_specs, max_executors)?;
        let executors = materialize_executors(&specs, &secret_root)?;
        let auth_path = PathBuf::from(env_required("GHA_EXECUTOR_ROUTER_AUTH_PATH")?);
        let inbound_auth = read_secret(&auth_path, &secret_root, "router inbound authentication")?;
        let execution_enabled = env_bool("GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED", false)?;
        if execution_enabled && executors.is_empty() {
            return Err(
                "execution is enabled but no executor entry is enabled and fully materialized"
                    .to_string(),
            );
        }
        let max_request_bytes = env_usize(
            "GHA_EXECUTOR_ROUTER_MAX_REQUEST_BYTES",
            MAX_REQUEST_BYTES_DEFAULT,
        )?;
        let max_upstream_body_bytes = env_usize(
            "GHA_EXECUTOR_ROUTER_MAX_UPSTREAM_BODY_BYTES",
            MAX_REQUEST_BYTES_DEFAULT,
        )?;
        let max_error_chars = env_usize(
            "GHA_EXECUTOR_ROUTER_MAX_ERROR_CHARS",
            MAX_ERROR_CHARS_DEFAULT,
        )?;
        if max_request_bytes == 0 || max_upstream_body_bytes == 0 || max_error_chars == 0 {
            return Err("request, upstream-body, and error bounds must be positive".to_string());
        }
        Ok(Self {
            host: env_optional("HOST").unwrap_or_else(|| "0.0.0.0".to_string()),
            port: env_u16("PORT", DEFAULT_PORT)?,
            inbound_auth,
            execution_enabled,
            executor_specs: specs.len(),
            executors,
            max_request_bytes,
            max_upstream_body_bytes,
            max_error_chars,
            probe_timeout: Duration::from_millis(env_u64(
                "GHA_EXECUTOR_ROUTER_PROBE_TIMEOUT_MS",
                2_000,
            )?),
            upstream_timeout: Duration::from_secs(env_u64(
                "GHA_EXECUTOR_ROUTER_UPSTREAM_TIMEOUT_SECONDS",
                60,
            )?),
        })
    }
}

#[derive(Default)]
struct Metrics {
    requests_total: AtomicU64,
    rejected_total: AtomicU64,
    readiness_failures_total: AtomicU64,
    submissions_total: AtomicU64,
    submissions_accepted_total: AtomicU64,
    ambiguous_submissions_total: AtomicU64,
    status_requests_total: AtomicU64,
    aws_selections_total: AtomicU64,
    hetzner_selections_total: AtomicU64,
}

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    client: reqwest::Client,
    metrics: Arc<Metrics>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gha_executor_router=info,tower_http=info".into()),
        )
        .init();
    let config = Config::from_env().unwrap_or_else(|error| {
        eprintln!("{ROUTER_SERVICE_NAME}: configuration error: {error}");
        std::process::exit(2);
    });
    let address = format!("{}:{}", config.host, config.port);
    let max_request_bytes = config.max_request_bytes;
    let state = AppState {
        client: reqwest::Client::builder()
            .connect_timeout(config.probe_timeout)
            .timeout(config.upstream_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("gha-executor-router/0.1")
            .build()
            .expect("build executor-router HTTP client"),
        config: Arc::new(config),
        metrics: Arc::new(Metrics::default()),
    };
    let app = router(state).layer(DefaultBodyLimit::max(max_request_bytes));
    let listener = TcpListener::bind(&address)
        .await
        .unwrap_or_else(|error| panic!("failed to bind {address}: {error}"));
    info!(%address, "gha executor router listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("serve gha executor router");
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(descriptor))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v1/capabilities", get(capabilities))
        .route("/metrics", get(metrics))
        .route("/builds", post(submit_build))
        .route("/builds/:id", get(get_build))
        .with_state(state)
}

async fn descriptor() -> Json<Value> {
    Json(json!({
        "service": ROUTER_SERVICE_NAME,
        "purpose": "fail-closed placement of fixed-profile dd-build-server jobs across reviewed AWS and Hetzner executors",
        "endpoints": {
            "health": "GET /healthz",
            "ready": "GET /readyz",
            "capabilities": "GET /v1/capabilities",
            "metrics": "GET /metrics",
            "submit": "POST /builds",
            "status": "GET /builds/<executor~job>"
        }
    }))
}

async fn healthz(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": ROUTER_SERVICE_NAME,
        "executionEnabled": state.config.execution_enabled,
        "configuredExecutors": state.config.executor_specs,
        "enabledExecutors": state.config.executors.len(),
        "authConfigured": !state.config.inbound_auth.is_empty()
    }))
}

async fn readyz(State(state): State<AppState>) -> Response {
    if !state.config.execution_enabled {
        return (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "service": ROUTER_SERVICE_NAME,
                "executionEnabled": false,
                "executionReady": true,
                "readyExecutors": []
            })),
        )
            .into_response();
    }
    let mut ready = Vec::new();
    for executor in &state.config.executors {
        if executor_ready(&state, executor).await {
            ready.push(json!({
                "id": executor.id,
                "provider": executor.provider.as_str()
            }));
        }
    }
    let ok = !ready.is_empty();
    (
        if ok {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "ok": ok,
            "service": ROUTER_SERVICE_NAME,
            "executionEnabled": true,
            "executionReady": ok,
            "readyExecutors": ready
        })),
    )
        .into_response()
}

async fn capabilities(State(state): State<AppState>) -> Json<Value> {
    let executors = state
        .config
        .executors
        .iter()
        .map(|executor| {
            json!({
                "id": executor.id,
                "provider": executor.provider.as_str()
            })
        })
        .collect::<Vec<_>>();
    Json(json!({
        "service": ROUTER_SERVICE_NAME,
        "schemaVersion": "gha-executor-router.v1",
        "executionEnabled": state.config.execution_enabled,
        "executors": executors,
        "acceptedJobKind": "run-profile",
        "immutableRevisionRequired": true,
        "routing": {
            "order": "configured executor order",
            "preSubmissionFailover": "only executors whose /readyz probe succeeds are eligible",
            "postSubmissionFailover": false,
            "reason": "a transport or HTTP failure after POST may be an ambiguous acceptance; automatic resubmission could duplicate work",
            "statusPinning": "executor id is namespaced into every accepted build id"
        },
        "coordination": {
            "requestIdForwardedUnchanged": true,
            "crossProviderResubmissionRequires": "shared Fiducia-fenced claim plus shared durable job/artifact state"
        }
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let metrics = &state.metrics;
    let body = format!(
        "# HELP gha_executor_router_requests_total Authenticated and unauthenticated HTTP build/status requests.\n\
         # TYPE gha_executor_router_requests_total counter\n\
         gha_executor_router_requests_total {}\n\
         # HELP gha_executor_router_rejected_total Requests rejected before upstream acceptance.\n\
         # TYPE gha_executor_router_rejected_total counter\n\
         gha_executor_router_rejected_total {}\n\
         # HELP gha_executor_router_readiness_failures_total Executor readiness probes that failed or returned non-success.\n\
         # TYPE gha_executor_router_readiness_failures_total counter\n\
         gha_executor_router_readiness_failures_total {}\n\
         # HELP gha_executor_router_submissions_total Upstream build submissions attempted after readiness selection.\n\
         # TYPE gha_executor_router_submissions_total counter\n\
         gha_executor_router_submissions_total {}\n\
         # HELP gha_executor_router_submissions_accepted_total Upstream build submissions accepted.\n\
         # TYPE gha_executor_router_submissions_accepted_total counter\n\
         gha_executor_router_submissions_accepted_total {}\n\
         # HELP gha_executor_router_ambiguous_submissions_total Submission attempts that could not be proven accepted or rejected and were not failed over.\n\
         # TYPE gha_executor_router_ambiguous_submissions_total counter\n\
         gha_executor_router_ambiguous_submissions_total {}\n\
         # HELP gha_executor_router_status_requests_total Namespaced build status requests.\n\
         # TYPE gha_executor_router_status_requests_total counter\n\
         gha_executor_router_status_requests_total {}\n\
         # HELP gha_executor_router_executor_selections_total Selected executors by provider.\n\
         # TYPE gha_executor_router_executor_selections_total counter\n\
         gha_executor_router_executor_selections_total{{provider=\"aws\"}} {}\n\
         gha_executor_router_executor_selections_total{{provider=\"hetzner\"}} {}\n",
        metrics.requests_total.load(Ordering::Relaxed),
        metrics.rejected_total.load(Ordering::Relaxed),
        metrics.readiness_failures_total.load(Ordering::Relaxed),
        metrics.submissions_total.load(Ordering::Relaxed),
        metrics
            .submissions_accepted_total
            .load(Ordering::Relaxed),
        metrics
            .ambiguous_submissions_total
            .load(Ordering::Relaxed),
        metrics.status_requests_total.load(Ordering::Relaxed),
        metrics.aws_selections_total.load(Ordering::Relaxed),
        metrics.hetzner_selections_total.load(Ordering::Relaxed),
    );
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

async fn submit_build(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    state.metrics.requests_total.fetch_add(1, Ordering::Relaxed);
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if !state.config.execution_enabled {
        state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "executor routing is disabled",
                "hint": "enable only after at least one executor and its mounted credential have passed readiness and no-duplicate smoke tests"
            })),
        )
            .into_response();
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "build request must be valid JSON" })),
            )
                .into_response();
        }
    };
    let validated = match validate_build_request(&request) {
        Ok(validated) => validated,
        Err(error) => {
            state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "error": bounded_text(&error, state.config.max_error_chars)
                })),
            )
                .into_response();
        }
    };
    info!(
        request_id = %validated.request_id,
        repository = %validated.repository,
        revision = %validated.revision,
        profile = %validated.profile,
        "validated fixed-profile executor request"
    );

    let Some(executor) = first_ready_executor(&state).await else {
        state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "no reviewed executor is ready",
                "retryable": true,
                "submissionAttempted": false
            })),
        )
            .into_response();
    };
    record_selection(&state.metrics, executor.provider);
    state
        .metrics
        .submissions_total
        .fetch_add(1, Ordering::Relaxed);

    let response = match state
        .client
        .post(format!("{}/builds", executor.base_url))
        .header("x-build-server-auth", &executor.auth)
        .json(&request)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            state
                .metrics
                .ambiguous_submissions_total
                .fetch_add(1, Ordering::Relaxed);
            return ambiguous_submission(&executor.id, None);
        }
    };
    let status = response.status();
    if status != StatusCode::ACCEPTED {
        if status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS {
            state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
            return (
                status,
                Json(json!({
                    "error": "selected executor rejected the fixed-profile request",
                    "executorId": executor.id,
                    "upstreamStatus": status.as_u16(),
                    "automaticFailover": false
                })),
            )
                .into_response();
        }
        state
            .metrics
            .ambiguous_submissions_total
            .fetch_add(1, Ordering::Relaxed);
        return ambiguous_submission(&executor.id, Some(status));
    }

    let body = match read_bounded_body(response, state.config.max_upstream_body_bytes).await {
        Ok(body) => body,
        Err(_) => {
            state
                .metrics
                .ambiguous_submissions_total
                .fetch_add(1, Ordering::Relaxed);
            return ambiguous_submission(&executor.id, Some(StatusCode::ACCEPTED));
        }
    };
    let mut value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            state
                .metrics
                .ambiguous_submissions_total
                .fetch_add(1, Ordering::Relaxed);
            return ambiguous_submission(&executor.id, Some(StatusCode::ACCEPTED));
        }
    };
    let Some(object) = value.as_object_mut() else {
        state
            .metrics
            .ambiguous_submissions_total
            .fetch_add(1, Ordering::Relaxed);
        return ambiguous_submission(&executor.id, Some(StatusCode::ACCEPTED));
    };
    let Some(upstream_id) = object.get("id").and_then(Value::as_str) else {
        state
            .metrics
            .ambiguous_submissions_total
            .fetch_add(1, Ordering::Relaxed);
        return ambiguous_submission(&executor.id, Some(StatusCode::ACCEPTED));
    };
    let route_id = match namespace_build_id(&executor.id, upstream_id) {
        Ok(route_id) => route_id,
        Err(_) => {
            state
                .metrics
                .ambiguous_submissions_total
                .fetch_add(1, Ordering::Relaxed);
            return ambiguous_submission(&executor.id, Some(StatusCode::ACCEPTED));
        }
    };
    object.insert("id".into(), Value::String(route_id));
    object.insert("executorId".into(), Value::String(executor.id.clone()));
    object.insert(
        "provider".into(),
        Value::String(executor.provider.as_str().to_string()),
    );
    state
        .metrics
        .submissions_accepted_total
        .fetch_add(1, Ordering::Relaxed);
    (StatusCode::ACCEPTED, Json(value)).into_response()
}

async fn get_build(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(route_id): AxumPath<String>,
) -> Response {
    state.metrics.requests_total.fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .status_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let (executor_id, upstream_id) = match parse_namespaced_build_id(&route_id) {
        Ok(parts) => parts,
        Err(error) => {
            state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
        }
    };
    let Some(executor) = state
        .config
        .executors
        .iter()
        .find(|executor| executor.id == executor_id)
    else {
        state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "executor route is not configured" })),
        )
            .into_response();
    };
    let response = match state
        .client
        .get(format!("{}/builds/{upstream_id}", executor.base_url))
        .header("x-build-server-auth", &executor.auth)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "status request to the accepting executor failed",
                    "executorId": executor.id,
                    "automaticFailover": false
                })),
            )
                .into_response()
        }
    };
    let status = response.status();
    if status != StatusCode::OK {
        return (
            if status == StatusCode::NOT_FOUND {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_GATEWAY
            },
            Json(json!({
                "error": "accepting executor did not return build status",
                "executorId": executor.id,
                "upstreamStatus": status.as_u16(),
                "automaticFailover": false
            })),
        )
            .into_response();
    }
    let body = match read_bounded_body(response, state.config.max_upstream_body_bytes).await {
        Ok(body) => body,
        Err(_) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "accepting executor returned an invalid bounded status response",
                    "executorId": executor.id
                })),
            )
                .into_response()
        }
    };
    let mut value: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "accepting executor returned invalid status JSON",
                    "executorId": executor.id
                })),
            )
                .into_response()
        }
    };
    let Some(object) = value.as_object_mut() else {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": "accepting executor returned a non-object status",
                "executorId": executor.id
            })),
        )
            .into_response();
    };
    if object.get("id").and_then(Value::as_str) != Some(upstream_id) {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": "accepting executor returned a mismatched build id",
                "executorId": executor.id
            })),
        )
            .into_response();
    }
    object.insert("id".into(), Value::String(route_id));
    object.insert("executorId".into(), Value::String(executor.id.clone()));
    object.insert(
        "provider".into(),
        Value::String(executor.provider.as_str().to_string()),
    );
    (StatusCode::OK, Json(value)).into_response()
}

async fn first_ready_executor(state: &AppState) -> Option<&Executor> {
    for executor in &state.config.executors {
        if executor_ready(state, executor).await {
            return Some(executor);
        }
    }
    None
}

async fn executor_ready(state: &AppState, executor: &Executor) -> bool {
    match state
        .client
        .get(format!("{}/readyz", executor.base_url))
        .timeout(state.config.probe_timeout)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => true,
        Ok(_) | Err(_) => {
            state
                .metrics
                .readiness_failures_total
                .fetch_add(1, Ordering::Relaxed);
            false
        }
    }
}

fn record_selection(metrics: &Metrics, provider: Provider) {
    match provider {
        Provider::Aws => metrics.aws_selections_total.fetch_add(1, Ordering::Relaxed),
        Provider::Hetzner => metrics
            .hetzner_selections_total
            .fetch_add(1, Ordering::Relaxed),
    };
}

fn ambiguous_submission(executor_id: &str, status: Option<StatusCode>) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "error": "executor submission outcome is ambiguous; automatic provider failover is blocked to prevent duplicate work",
            "executorId": executor_id,
            "upstreamStatus": status.map(|value| value.as_u16()),
            "automaticFailover": false,
            "retryGuidance": "reconcile the deterministic requestId through shared build-server/Fiducia state before any operator retry"
        })),
    )
        .into_response()
}

#[allow(clippy::result_large_err)]
fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    let presented = headers
        .get("x-build-server-auth")
        .or_else(|| headers.get("x-server-auth"))
        .and_then(|value| value.to_str().ok());
    if presented.is_some_and(|value| digest_eq(value, &state.config.inbound_auth)) {
        Ok(())
    } else {
        state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
        Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response())
    }
}

async fn read_bounded_body(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err("upstream response exceeds configured body bound".to_string());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "upstream response body could not be read".to_string())?
    {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err("upstream response exceeds configured body bound".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn read_secret(path: &Path, root: &Path, label: &str) -> Result<String, String> {
    if !path.is_absolute() || path.parent() != Some(root) {
        return Err(format!(
            "{label} path must be an absolute direct child of {}",
            root.display()
        ));
    }
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| format!("secret root {} is unavailable: {error}", root.display()))?;
    let canonical_path = fs::canonicalize(path)
        .map_err(|error| format!("{label} file {} is unavailable: {error}", path.display()))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(format!("{label} file escapes the configured secret root"));
    }
    let raw = fs::read_to_string(&canonical_path)
        .map_err(|error| format!("{label} file could not be read: {error}"))?;
    let value = raw.trim().to_string();
    if value.len() < MIN_SECRET_BYTES
        || value.len() > MAX_SECRET_BYTES
        || value.as_bytes().contains(&0)
        || value.contains('\n')
        || value.contains('\r')
    {
        return Err(format!(
            "{label} must contain between {MIN_SECRET_BYTES} and {MAX_SECRET_BYTES} non-NUL bytes"
        ));
    }
    Ok(value)
}

fn env_optional(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_required(name: &str) -> Result<String, String> {
    env_optional(name).ok_or_else(|| format!("{name} is required"))
}

fn env_bool(name: &str, default: bool) -> Result<bool, String> {
    match env_optional(name) {
        Some(value) => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(format!("{name} must be true or false")),
        },
        None => Ok(default),
    }
}

fn env_u16(name: &str, default: u16) -> Result<u16, String> {
    env_optional(name)
        .map(|value| {
            value
                .parse::<u16>()
                .map_err(|error| format!("{name} is invalid: {error}"))
                .and_then(|value| {
                    if value == 0 {
                        Err(format!("{name} must be positive"))
                    } else {
                        Ok(value)
                    }
                })
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_u64(name: &str, default: u64) -> Result<u64, String> {
    env_optional(name)
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| format!("{name} is invalid: {error}"))
                .and_then(|value| {
                    if value == 0 {
                        Err(format!("{name} must be positive"))
                    } else {
                        Ok(value)
                    }
                })
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_usize(name: &str, default: usize) -> Result<usize, String> {
    env_optional(name)
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|error| format!("{name} is invalid: {error}"))
                .and_then(|value| {
                    if value == 0 {
                        Err(format!("{name} must be positive"))
                    } else {
                        Ok(value)
                    }
                })
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl-C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_errors_never_include_an_upstream_body() {
        let body = "secret-token=do-not-return";
        let response = ambiguous_submission("aws-primary", Some(StatusCode::SERVICE_UNAVAILABLE));
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(bounded_text(body, 6), "secret");
    }

    #[test]
    fn malformed_boolean_and_zero_bounds_fail_configuration_helpers() {
        env::set_var("ROUTER_TEST_BOOL", "sometimes");
        assert!(env_bool("ROUTER_TEST_BOOL", false).is_err());
        env::set_var("ROUTER_TEST_U64", "0");
        assert!(env_u64("ROUTER_TEST_U64", 1).is_err());
        env::remove_var("ROUTER_TEST_BOOL");
        env::remove_var("ROUTER_TEST_U64");
    }

    #[test]
    fn provider_metrics_use_a_bounded_known_label_set() {
        let metrics = Metrics::default();
        record_selection(&metrics, Provider::Aws);
        record_selection(&metrics, Provider::Hetzner);
        assert_eq!(metrics.aws_selections_total.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.hetzner_selections_total.load(Ordering::Relaxed), 1);
    }
}
