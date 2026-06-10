use std::{
    env,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dd_nats_subject_defs::{
    thread_tasks_subject, DD_REMOTE_TASKS_STREAM_NAME, ORCHESTRATOR_WAKEUP_SUBJECT,
    THREAD_TASKS_WILDCARD,
};
use dd_shared_interfaces::AgentTaskQueueMessage;
use serde::{Deserialize, Serialize};
use serde_json::json;

const MAX_HTTP_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_IDENTIFIER_LEN: usize = 200;
const MAX_PROMPT_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
struct AppState {
    config: Config,
    http: reqwest::Client,
    // A single shared NATS client. Connected once at startup and reused for
    // every dispatch; the previous code opened a fresh TCP connection per
    // request, which is both slow and unauthenticated.
    nats: async_nats::Client,
    metrics: Arc<Metrics>,
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    dispatch_requests_total: AtomicU64,
    dispatch_failures_total: AtomicU64,
    nats_publish_failures_total: AtomicU64,
}

#[derive(Clone)]
struct Config {
    namespace: String,
    nats_url: String,
    nats_task_stream: String,
    nats_task_subject: String,
    nats_wakeup_subject: String,
    direct_dispatch_enabled: bool,
    worker_health_timeout: Duration,
    worker_task_timeout: Duration,
    server_auth_secret: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DispatchTaskRequest {
    task_id: String,
    thread_id: Option<String>,
    repo: Option<String>,
    base_branch: Option<String>,
    prompt: String,
    provider: Option<String>,
    thread_title: Option<String>,
    context_mode: Option<String>,
    context_ids: Option<Vec<String>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BrokerHealth {
    ok: bool,
    service: &'static str,
    direct_dispatch_enabled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NatsPublishResult {
    published: bool,
    subject: String,
    wakeup_subject: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerDispatchResult {
    attempted: bool,
    worker_awake: bool,
    sent: bool,
    status: Option<u16>,
    body: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct K8sWakeResult {
    attempted: bool,
    ok: bool,
    resource: String,
    status: Option<u16>,
    body: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BrokerResponse {
    ok: bool,
    mode: &'static str,
    thread_id: String,
    task_id: String,
    worker_name: String,
    nats: NatsPublishResult,
    direct_dispatch: WorkerDispatchResult,
    wake: K8sWakeResult,
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn env_value(key: &str, fallback: &str) -> String {
    first_env(&[key]).unwrap_or_else(|| fallback.to_string())
}

fn env_bool(key: &str, fallback: bool) -> bool {
    first_env(&[key])
        .map(|value| {
            matches!(
                value.as_str(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(fallback)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    first_env(&[key])
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn required_repo(request: &DispatchTaskRequest) -> Result<String, String> {
    let repo = request
        .repo
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "repo is required".to_string())?;
    if repo.len() > 2048 {
        return Err("repo must be 2048 characters or fewer".to_string());
    }
    Ok(repo.to_string())
}

fn requested_base_branch(request: &DispatchTaskRequest) -> Result<String, String> {
    let base_branch = request.base_branch.as_deref().unwrap_or("dev").trim();
    if base_branch.is_empty() {
        return Err("baseBranch must not be empty".to_string());
    }
    if base_branch.len() > 120 {
        return Err("baseBranch must be 120 characters or fewer".to_string());
    }
    Ok(base_branch.to_string())
}

fn thread_resource_name(thread_id: &str) -> String {
    let short = thread_id
        .chars()
        .filter(|value| value.is_ascii_alphanumeric())
        .take(12)
        .collect::<String>()
        .to_lowercase();
    format!("dd-thread-{short}")
}

fn thread_worker_url(namespace: &str, name: &str, path: &str) -> String {
    format!("http://{name}.{namespace}.svc.cluster.local:8080{path}")
}

fn config_from_env() -> Config {
    Config {
        namespace: env_value("THREAD_RUNTIME_NAMESPACE", "default"),
        nats_url: env_value(
            "NATS_URL",
            "nats://dd-nats.messaging.svc.cluster.local:4222",
        ),
        nats_task_stream: env_value("NATS_TASK_STREAM", DD_REMOTE_TASKS_STREAM_NAME),
        nats_task_subject: env_value("NATS_TASK_SUBJECT", THREAD_TASKS_WILDCARD),
        nats_wakeup_subject: env_value("NATS_WAKEUP_SUBJECT", ORCHESTRATOR_WAKEUP_SUBJECT),
        direct_dispatch_enabled: env_bool("DIRECT_DISPATCH_ENABLED", true),
        worker_health_timeout: Duration::from_millis(env_u64("WORKER_HEALTH_TIMEOUT_MS", 800)),
        worker_task_timeout: Duration::from_millis(env_u64("WORKER_TASK_TIMEOUT_MS", 30_000)),
        server_auth_secret: first_env(&["REMOTE_DEV_SERVER_SECRET", "SERVER_AUTH_SECRET"]),
    }
}

/// Compare two secrets without leaking length-independent timing. A plain `==`
/// short-circuits on the first differing byte, which is a (small) side channel
/// on the shared auth secret.
fn constant_time_equals(candidate: &str, expected: &str) -> bool {
    let candidate = candidate.as_bytes();
    let expected = expected.as_bytes();
    if candidate.len() != expected.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in candidate.iter().zip(expected.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

fn request_is_authorized(headers: &HeaderMap, secret: &str) -> bool {
    headers
        .get("x-server-auth")
        .or_else(|| headers.get("x-agent-auth"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| constant_time_equals(value, secret))
}

/// Validate a `thread_id`/`task_id` against a strict allowlist. `thread_id` is
/// interpolated **raw** into the NATS subject `dd.remote.thread.{id}.tasks`, so
/// a value containing `.`, `*`, or `>` would inject extra subject tokens or
/// wildcards and break per-thread isolation. Allow only ASCII alphanumerics,
/// `-`, and `_` (UUIDs qualify); notably `.` is rejected because it is the NATS
/// token separator.
fn validate_identifier(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if value.len() > MAX_IDENTIFIER_LEN {
        return Err(format!("{label} must be at most {MAX_IDENTIFIER_LEN} bytes"));
    }
    if let Some(bad) = value
        .chars()
        .find(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')))
    {
        return Err(format!(
            "{label} must contain only ASCII alphanumerics, '-', or '_' (found {bad:?})"
        ));
    }
    Ok(())
}

fn validate_prompt(prompt: &str) -> Result<(), String> {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return Err("prompt must not be empty".to_string());
    }
    if prompt.len() > MAX_PROMPT_BYTES {
        return Err(format!("prompt must be at most {MAX_PROMPT_BYTES} bytes"));
    }
    Ok(())
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(BrokerHealth {
        ok: true,
        service: "dd-agent-worker-broker",
        direct_dispatch_enabled: state.config.direct_dispatch_enabled,
    })
}

async fn metrics(State(state): State<AppState>) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let body = format!(
        concat!(
            "# HELP dd_agent_worker_broker_build_info Agent worker broker build metadata.\n",
            "# TYPE dd_agent_worker_broker_build_info gauge\n",
            "dd_agent_worker_broker_build_info{{service=\"dd-agent-worker-broker\"}} 1\n",
            "# HELP dd_agent_worker_broker_http_requests_total HTTP requests handled by the broker.\n",
            "# TYPE dd_agent_worker_broker_http_requests_total counter\n",
            "dd_agent_worker_broker_http_requests_total {}\n",
            "# HELP dd_agent_worker_broker_dispatch_requests_total Task dispatch requests handled by the broker.\n",
            "# TYPE dd_agent_worker_broker_dispatch_requests_total counter\n",
            "dd_agent_worker_broker_dispatch_requests_total {}\n",
            "# HELP dd_agent_worker_broker_dispatch_failures_total Task dispatch requests rejected or failed by the broker.\n",
            "# TYPE dd_agent_worker_broker_dispatch_failures_total counter\n",
            "dd_agent_worker_broker_dispatch_failures_total {}\n",
            "# HELP dd_agent_worker_broker_nats_publish_failures_total NATS publish failures while dispatching tasks.\n",
            "# TYPE dd_agent_worker_broker_nats_publish_failures_total counter\n",
            "dd_agent_worker_broker_nats_publish_failures_total {}\n",
            "# HELP dd_agent_worker_broker_direct_dispatch_enabled Direct worker dispatch setting.\n",
            "# TYPE dd_agent_worker_broker_direct_dispatch_enabled gauge\n",
            "dd_agent_worker_broker_direct_dispatch_enabled {}\n"
        ),
        state.metrics.http_requests_total.load(Ordering::Relaxed),
        state.metrics.dispatch_requests_total.load(Ordering::Relaxed),
        state.metrics.dispatch_failures_total.load(Ordering::Relaxed),
        state.metrics.nats_publish_failures_total.load(Ordering::Relaxed),
        u8::from(state.config.direct_dispatch_enabled)
    );

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

async fn ensure_task_stream(config: &Config, client: async_nats::Client) -> Result<(), String> {
    let jetstream = async_nats::jetstream::new(client);
    jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: config.nats_task_stream.clone(),
            subjects: vec![config.nats_task_subject.clone()],
            retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
            max_age: Duration::from_secs(60 * 60 * 24 * 14),
            max_message_size: 8 * 1024 * 1024,
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn publish_task_to_nats(
    config: &Config,
    thread_id: &str,
    request: &DispatchTaskRequest,
    repo: &str,
    base_branch: &str,
) -> Result<NatsPublishResult, String> {
    let client = async_nats::connect(config.nats_url.clone())
        .await
        .map_err(|error| error.to_string())?;
    ensure_task_stream(config, client.clone()).await?;

    let subject = thread_tasks_subject(thread_id);
    let payload = serde_json::to_vec(&AgentTaskQueueMessage {
        version: Some(1),
        message_kind: Some("task.dispatch".to_string()),
        task_kind: Some("agent.prompt".to_string()),
        shadow: Some(false),
        direct_dispatch: Some(false),
        dispatch_mode: None,
        container_pool_dispatch: None,
        thread_id: thread_id.to_string(),
        task_id: request.task_id.clone(),
        provider: request.provider.clone(),
        repo: Some(repo.to_string()),
        base_branch: Some(base_branch.to_string()),
        feature_branch: None,
        prompt: Some(request.prompt.clone()),
        thread_title: None,
        context_mode: request.context_mode.clone(),
        context_ids: request.context_ids.clone(),
        created_at_ms: Some(now_ms() as i64),
    })
    .map_err(|error| error.to_string())?;

    let jetstream = async_nats::jetstream::new(client.clone());
    jetstream
        .publish(subject.clone(), payload.clone().into())
        .await
        .map_err(|error| error.to_string())?
        .await
        .map_err(|error| error.to_string())?;
    client
        .publish(config.nats_wakeup_subject.clone(), payload.into())
        .await
        .map_err(|error| error.to_string())?;
    client.flush().await.map_err(|error| error.to_string())?;

    return Ok(NatsPublishResult {
        published: true,
        subject,
        wakeup_subject: config.nats_wakeup_subject.clone(),
    })
}

fn skipped_nats_result(config: &Config, thread_id: &str) -> NatsPublishResult {
    return NatsPublishResult {
        published: false,
        subject: thread_tasks_subject(thread_id),
        wakeup_subject: config.nats_wakeup_subject.clone(),
    }
}

fn skipped_wake_result() -> K8sWakeResult {
    K8sWakeResult {
        attempted: false,
        ok: false,
        resource: String::new(),
        status: None,
        body: None,
        error: None,
    }
}

async fn worker_is_awake(state: &AppState, worker_name: &str) -> bool {
    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        return false;
    };
    let url = thread_worker_url(&state.config.namespace, worker_name, "/healthz");
    state
        .http
        .get(url)
        .timeout(state.config.worker_health_timeout)
        .header("X-Server-Auth", secret)
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

async fn direct_dispatch(
    state: &AppState,
    thread_id: &str,
    worker_name: &str,
    request: &DispatchTaskRequest,
    repo: &str,
    base_branch: &str,
) -> WorkerDispatchResult {
    if !state.config.direct_dispatch_enabled {
        return WorkerDispatchResult {
            attempted: false,
            worker_awake: false,
            sent: false,
            status: None,
            body: None,
            error: None,
        };
    }

    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        return WorkerDispatchResult {
            attempted: true,
            worker_awake: false,
            sent: false,
            status: None,
            body: None,
            error: Some("REMOTE_DEV_SERVER_SECRET or SERVER_AUTH_SECRET is not set".to_string()),
        };
    };

    if !worker_is_awake(state, worker_name).await {
        return WorkerDispatchResult {
            attempted: true,
            worker_awake: false,
            sent: false,
            status: None,
            body: None,
            error: None,
        };
    }

    let worker_body = json!({
        "taskId": &request.task_id,
        "threadId": thread_id,
        "repo": repo,
        "baseBranch": base_branch,
        "prompt": &request.prompt,
        "provider": &request.provider,
        "threadTitle": &request.thread_title,
        "contextMode": &request.context_mode,
        "contextIds": &request.context_ids,
    });
    let url = thread_worker_url(&state.config.namespace, worker_name, "/tasks");
    match state
        .http
        .post(url)
        .timeout(state.config.worker_task_timeout)
        .header("X-Server-Auth", secret)
        .json(&worker_body)
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            WorkerDispatchResult {
                attempted: true,
                worker_awake: true,
                sent: status.is_success(),
                status: Some(status.as_u16()),
                body: Some(body.chars().take(2_000).collect()),
                error: None,
            }
        }
        Err(error) => WorkerDispatchResult {
            attempted: true,
            worker_awake: true,
            sent: false,
            status: None,
            body: None,
            error: Some(error.to_string()),
        },
    }
}

async fn dispatch_task(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<DispatchTaskRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .dispatch_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        state
            .metrics
            .dispatch_failures_total
            .fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "SERVER_AUTH_SECRET is not configured" })),
        )
            .into_response();
    };
    if !request_is_authorized(&headers, secret) {
        state
            .metrics
            .dispatch_failures_total
            .fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthorized",
                "errMessage": "missing required worker broker auth header",
            })),
        )
            .into_response();
    }
    if let Some(body_thread_id) = request.thread_id.as_deref() {
        if body_thread_id != thread_id {
            state
                .metrics
                .dispatch_failures_total
                .fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "threadId path/body mismatch" })),
            )
                .into_response();
        }
    }
    // Validate identifiers before they reach the NATS subject / worker URL.
    // threadId in particular is interpolated into the task subject.
    for (value, label) in [(&thread_id, "threadId"), (&request.task_id, "taskId")] {
        if let Err(error) = validate_identifier(value, label) {
            state
                .metrics
                .dispatch_failures_total
                .fetch_add(1, Ordering::Relaxed);
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
        }
    }
    if let Err(error) = validate_prompt(&request.prompt) {
        state
            .metrics
            .dispatch_failures_total
            .fetch_add(1, Ordering::Relaxed);
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    let repo = match required_repo(&request) {
        Ok(value) => value,
        Err(error) => {
            state
                .metrics
                .dispatch_failures_total
                .fetch_add(1, Ordering::Relaxed);
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
        }
    };
    let base_branch = match requested_base_branch(&request) {
        Ok(value) => value,
        Err(error) => {
            state
                .metrics
                .dispatch_failures_total
                .fetch_add(1, Ordering::Relaxed);
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
        }
    };

    let worker_name = thread_resource_name(&thread_id);
    let direct_dispatch = direct_dispatch(
        &state,
        &thread_id,
        &worker_name,
        &request,
        &repo,
        &base_branch,
    )
    .await;
    if direct_dispatch.worker_awake {
        let ok = direct_dispatch.sent;
        let nats = skipped_nats_result(&state.config, &thread_id);
        if !ok {
            state
                .metrics
                .dispatch_failures_total
                .fetch_add(1, Ordering::Relaxed);
        }
        let status = if ok {
            StatusCode::OK
        } else {
            StatusCode::BAD_GATEWAY
        };
        return (
            status,
            Json(BrokerResponse {
                ok,
                mode: "direct",
                thread_id,
                task_id: request.task_id,
                worker_name,
                nats,
                direct_dispatch,
                wake: skipped_wake_result(),
            }),
        )
            .into_response();
    }

    let nats = match publish_task_to_nats(&state.config, &thread_id, &request, &repo, &base_branch)
        .await
    {
        Ok(value) => value,
        Err(error) => {
            state
                .metrics
                .dispatch_failures_total
                .fetch_add(1, Ordering::Relaxed);
            state
                .metrics
                .nats_publish_failures_total
                .fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "failed to publish agent task", "detail": error })),
            )
                .into_response();
        }
    };
    let wake = skipped_wake_result();
    let mode = "queued";
    let status = StatusCode::ACCEPTED;

    (
        status,
        Json(BrokerResponse {
            ok: true,
            mode,
            thread_id,
            task_id: request.task_id,
            worker_name,
            nats,
            direct_dispatch,
            wake,
        }),
    )
        .into_response()
}

async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let config = config_from_env();
    let host = env_value("HOST", "0.0.0.0");
    let port = env_u64("PORT", 8098) as u16;
    let state = AppState {
        config,
        http: reqwest::Client::new(),
        metrics: Arc::new(Metrics::default()),
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route(
            "/api/agent-worker/threads/:thread_id/tasks",
            post(dispatch_task),
        )
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("failed to parse bind address");
    println!("dd-agent-worker-broker listening on http://{address}");

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("failed to bind tcp listener");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("axum server crashed");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
            let _ = sigterm.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
