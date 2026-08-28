use std::{
    collections::HashMap,
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
#[path = "executor_router.rs"]
mod executor_router;
use executor_router::{
    bounded_text, digest_eq, materialize_executors, namespace_build_id, parse_executor_specs,
    parse_namespaced_build_id, validate_build_request, validate_secret_root, Executor, Provider,
    ValidatedBuildRequest, MAX_ERROR_CHARS_DEFAULT, MAX_EXECUTORS_DEFAULT,
    MAX_REQUEST_BYTES_DEFAULT, MAX_SECRET_BYTES, MIN_SECRET_BYTES, ROUTER_SERVICE_NAME,
};
use serde_json::{json, Value};
use tokio::{
    net::TcpListener,
    sync::{Mutex, Notify},
    time::Duration,
};
use tracing::info;

const DEFAULT_PORT: u16 = 8126;
const DEFAULT_SECRET_ROOT: &str = "/var/run/secrets/gha-executor-router";
const DEFAULT_MAX_ASSIGNMENTS: usize = 4096;

#[path = "executor_router_service/assignment.rs"]
mod assignment;
#[path = "executor_router_service/security.rs"]
mod security;
#[path = "executor_router_service/upstream.rs"]
mod upstream;

use assignment::submit_build;
use security::{
    env_bool, env_optional, env_required, env_u16, env_u64, env_usize, read_secret, shutdown_signal,
};
use upstream::get_build;

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
    max_assignments: usize,
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
        let max_assignments = env_usize(
            "GHA_EXECUTOR_ROUTER_MAX_ASSIGNMENTS",
            DEFAULT_MAX_ASSIGNMENTS,
        )?;
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
            max_assignments,
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
    duplicate_hits_total: AtomicU64,
    duplicate_conflicts_total: AtomicU64,
    assignment_capacity_exhausted_total: AtomicU64,
    status_requests_total: AtomicU64,
    aws_selections_total: AtomicU64,
    hetzner_selections_total: AtomicU64,
}

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    client: reqwest::Client,
    metrics: Arc<Metrics>,
    assignments: Arc<Mutex<HashMap<String, Arc<Assignment>>>>,
}

struct Assignment {
    request: ValidatedBuildRequest,
    executor_id: String,
    outcome: Mutex<Option<AssignmentOutcome>>,
    notify: Notify,
}

impl Assignment {
    fn new(request: ValidatedBuildRequest, executor_id: String) -> Self {
        Self {
            request,
            executor_id,
            outcome: Mutex::new(None),
            notify: Notify::new(),
        }
    }
}

#[derive(Clone)]
enum AssignmentOutcome {
    Accepted(Value),
    Rejected { status: StatusCode, body: Value },
    Ambiguous { upstream_status: Option<u16> },
}

pub async fn run() {
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
            .user_agent("gha-executor-router/0.2")
            .build()
            .expect("build executor-router HTTP client"),
        config: Arc::new(config),
        metrics: Arc::new(Metrics::default()),
        assignments: Arc::new(Mutex::new(HashMap::new())),
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
    let assignments = state.assignments.lock().await.len();
    Json(json!({
        "ok": true,
        "service": ROUTER_SERVICE_NAME,
        "executionEnabled": state.config.execution_enabled,
        "configuredExecutors": state.config.executor_specs,
        "enabledExecutors": state.config.executors.len(),
        "authConfigured": !state.config.inbound_auth.is_empty(),
        "retainedAssignments": assignments,
        "maxAssignments": state.config.max_assignments
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
        if upstream::executor_ready(&state, executor).await {
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
            "requestContentBinding": true,
            "ambiguousAssignmentRetention": true,
            "assignmentScope": "bounded in-process single-replica",
            "crossProviderResubmissionRequires": "shared Fiducia-fenced claim plus shared durable job/artifact state"
        }
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let metrics = &state.metrics;
    let assignment_count = state.assignments.lock().await.len();
    let body = format!(
        "# TYPE gha_executor_router_requests_total counter\n\
         gha_executor_router_requests_total {}\n\
         # TYPE gha_executor_router_rejected_total counter\n\
         gha_executor_router_rejected_total {}\n\
         # TYPE gha_executor_router_readiness_failures_total counter\n\
         gha_executor_router_readiness_failures_total {}\n\
         # TYPE gha_executor_router_submissions_total counter\n\
         gha_executor_router_submissions_total {}\n\
         # TYPE gha_executor_router_submissions_accepted_total counter\n\
         gha_executor_router_submissions_accepted_total {}\n\
         # TYPE gha_executor_router_ambiguous_submissions_total counter\n\
         gha_executor_router_ambiguous_submissions_total {}\n\
         # TYPE gha_executor_router_duplicate_hits_total counter\n\
         gha_executor_router_duplicate_hits_total {}\n\
         # TYPE gha_executor_router_duplicate_conflicts_total counter\n\
         gha_executor_router_duplicate_conflicts_total {}\n\
         # TYPE gha_executor_router_assignment_capacity_exhausted_total counter\n\
         gha_executor_router_assignment_capacity_exhausted_total {}\n\
         # TYPE gha_executor_router_assignments gauge\n\
         gha_executor_router_assignments {}\n\
         # TYPE gha_executor_router_status_requests_total counter\n\
         gha_executor_router_status_requests_total {}\n\
         # TYPE gha_executor_router_executor_selections_total counter\n\
         gha_executor_router_executor_selections_total{{provider=\"aws\"}} {}\n\
         gha_executor_router_executor_selections_total{{provider=\"hetzner\"}} {}\n",
        metrics.requests_total.load(Ordering::Relaxed),
        metrics.rejected_total.load(Ordering::Relaxed),
        metrics.readiness_failures_total.load(Ordering::Relaxed),
        metrics.submissions_total.load(Ordering::Relaxed),
        metrics.submissions_accepted_total.load(Ordering::Relaxed),
        metrics.ambiguous_submissions_total.load(Ordering::Relaxed),
        metrics.duplicate_hits_total.load(Ordering::Relaxed),
        metrics.duplicate_conflicts_total.load(Ordering::Relaxed),
        metrics
            .assignment_capacity_exhausted_total
            .load(Ordering::Relaxed),
        assignment_count,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_errors_never_include_an_upstream_body() {
        let body = "secret-token=do-not-return";
        let response =
            upstream::ambiguous_submission("aws-primary", Some(StatusCode::SERVICE_UNAVAILABLE));
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
        upstream::record_selection(&metrics, Provider::Aws);
        upstream::record_selection(&metrics, Provider::Hetzner);
        assert_eq!(metrics.aws_selections_total.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.hetzner_selections_total.load(Ordering::Relaxed), 1);
    }
}
