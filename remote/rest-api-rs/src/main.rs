#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    collections::HashMap,
    env, fs,
    io::BufReader,
    net::SocketAddr,
    path::{Component, Path as FsPath, PathBuf},
    process::Command,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Body,
    extract::{Path, Query},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounterVec, IntGauge, Opts, TextEncoder};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

static STARTED_AT: Lazy<Instant> = Lazy::new(Instant::now);
static HTTP_REQUESTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "dd_remote_rest_api_http_requests_total",
            "HTTP requests observed by the dd remote REST API.",
        ),
        &["method", "path", "status"],
    )
    .expect("failed to create dd_remote_rest_api_http_requests_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("failed to register dd_remote_rest_api_http_requests_total");
    counter
});
static UPTIME_SECONDS: Lazy<IntGauge> = Lazy::new(|| {
    let gauge = IntGauge::new(
        "dd_remote_rest_api_uptime_seconds",
        "REST API process uptime in seconds.",
    )
    .expect("failed to create dd_remote_rest_api_uptime_seconds");
    prometheus::default_registry()
        .register(Box::new(gauge.clone()))
        .expect("failed to register dd_remote_rest_api_uptime_seconds");
    gauge
});
static RUNTIME_STATE: Lazy<Mutex<RuntimeMemoryState>> =
    Lazy::new(|| Mutex::new(RuntimeMemoryState::default()));

#[derive(Default)]
struct RuntimeMemoryState {
    threads: HashMap<String, AgentThreadRow>,
    tasks: Vec<AgentTaskRow>,
}

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
    service: String,
    mode: String,
}

#[derive(Deserialize)]
struct AgentsQuery {
    limit: Option<i64>,
}

#[derive(Deserialize)]
struct ContextQuery {
    limit: Option<i64>,
}

#[derive(Deserialize)]
struct LambdasQuery {
    limit: Option<i64>,
    search: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentsSnapshot {
    ok: bool,
    source: String,
    generated_at_ms: u128,
    config: AgentsDataConfig,
    summary: AgentsSummary,
    threads: Vec<AgentThreadRow>,
    tasks: Vec<AgentTaskRow>,
    errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadContextResponse {
    ok: bool,
    source: String,
    thread_id: String,
    generated_at_ms: u128,
    tasks: Vec<AgentTaskRow>,
    errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KnownGitReposResponse {
    ok: bool,
    source: String,
    generated_at_ms: u128,
    repos: Vec<KnownGitRepoRow>,
    errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentsDataConfig {
    rds_configured: bool,
    postgres_configured: bool,
    supabase_configured: bool,
    nats_configured: bool,
    nats_url: String,
    postgres_plan: String,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentsSummary {
    thread_count: usize,
    task_count: usize,
    running_count: usize,
    failed_count: usize,
    done_count: usize,
    pr_count: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentThreadRow {
    id: String,
    title: String,
    repo: String,
    base_branch: String,
    archived_at: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    task_count: i64,
    active_task_count: i64,
    latest_task_at: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct KnownGitRepoRow {
    id: String,
    repo_url: String,
    display_name: String,
    provider: String,
    default_branch: String,
    status: String,
    last_verified_at: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentTaskRow {
    id: String,
    thread_id: String,
    thread_title: Option<String>,
    prompt: String,
    status: String,
    branch: Option<String>,
    pr_url: Option<String>,
    pr_state: Option<String>,
    exit_reason: Option<String>,
    error_message: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    last_event_seq: i32,
    event_count: i64,
    latest_event_kind: Option<String>,
    latest_payload: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentEventRow {
    task_id: String,
    seq: i32,
    event_kind: String,
    payload: Value,
    created_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentTaskEventsResponse {
    ok: bool,
    source: String,
    task_id: String,
    generated_at_ms: u128,
    events: Vec<AgentEventRow>,
    errors: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LambdaFunctionRow {
    id: String,
    slug: String,
    display_name: String,
    description: String,
    runtime: String,
    entry_command: String,
    function_body: String,
    reuse_key: Option<String>,
    idle_timeout_seconds: i32,
    max_run_ms: i32,
    containerized: bool,
    container_image: Option<String>,
    container_build_status: String,
    container_build_error: Option<String>,
    container_built_at: Option<String>,
    status: String,
    labels: Value,
    meta_data: Value,
    last_invoked_at: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LambdaFunctionsResponse {
    ok: bool,
    source: String,
    generated_at_ms: u128,
    functions: Vec<LambdaFunctionRow>,
    errors: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LambdaFunctionSaveRequest {
    slug: String,
    display_name: String,
    description: Option<String>,
    runtime: Option<String>,
    entry_command: Option<String>,
    function_body: String,
    reuse_key: Option<String>,
    idle_timeout_seconds: Option<i32>,
    max_run_ms: Option<i32>,
    containerized: Option<bool>,
    status: Option<String>,
    labels: Option<Value>,
    meta_data: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadActionResult {
    resource: String,
    status: u16,
    ok: bool,
    body: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadActionResponse {
    ok: bool,
    action: String,
    thread_id: String,
    k8s_name: String,
    namespace: String,
    results: Vec<ThreadActionResult>,
    errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadRuntimeResponse {
    ok: bool,
    source: String,
    thread_id: String,
    namespace: String,
    k8s_name: String,
    generated_at_ms: u128,
    summary: Value,
    deployment: Option<Value>,
    service: Option<Value>,
    pods: Vec<Value>,
    errors: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DispatchTaskRequest {
    task_id: String,
    thread_id: String,
    repo: String,
    base_branch: Option<String>,
    prompt: String,
    provider: Option<String>,
    thread_title: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KnownGitRepoRequest {
    repo_url: String,
    display_name: Option<String>,
    provider: Option<String>,
    default_branch: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentEventIngestRequest {
    task_id: String,
    seq: i32,
    event: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentFeedbackRequest {
    target_seq: Option<i32>,
    vote: String,
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadControlRequest {
    kind: String,
    action: String,
    thread_id: String,
    task_id: Option<String>,
    requested_by: Option<String>,
    reason: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NatsTaskMessage {
    version: u8,
    message_kind: &'static str,
    task_kind: &'static str,
    shadow: bool,
    direct_dispatch: bool,
    thread_id: String,
    task_id: String,
    provider: Option<String>,
    repo: String,
    base_branch: String,
    feature_branch: Option<String>,
    prompt: String,
    created_at_ms: u128,
}

#[derive(Clone)]
struct ThreadRepoConfig {
    repo: String,
    base_branch: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NatsLambdaFunctionMessage {
    version: u8,
    message_kind: &'static str,
    action: String,
    function_id: String,
    slug: String,
    status: String,
    updated_at_ms: u128,
}

fn record_request(method: &str, path: &str, status: StatusCode) {
    HTTP_REQUESTS
        .with_label_values(&[method, path, status.as_str()])
        .inc();
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn now_label() -> String {
    now_ms().to_string()
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

fn postgres_database_url() -> Option<String> {
    first_env(&[
        "AGENT_TASKS_RDS_DATABASE_URL",
        "RDS_DATABASE_URL",
        "AGENT_TASKS_DATABASE_URL",
        "DATABASE_URL",
    ])
}

fn agent_tasks_admin_user_id() -> Option<String> {
    first_env(&["AGENT_TASKS_ADMIN_USER_ID", "REMOTE_DEV_ADMIN_USER_ID"])
}

fn data_config() -> AgentsDataConfig {
    let rds_configured = first_env(&["AGENT_TASKS_RDS_DATABASE_URL", "RDS_DATABASE_URL"]).is_some();
    let postgres_configured = postgres_database_url().is_some();
    let supabase_configured = first_env(&["SUPABASE_URL", "NEXT_PUBLIC_SUPABASE_URL"]).is_some()
        && first_env(&["SUPABASE_SERVICE_ROLE_KEY", "SUPABASE_KEY"]).is_some();
    let nats_url = first_env(&["NATS_URL"])
        .unwrap_or_else(|| "nats://dd-nats.messaging.svc.cluster.local:4222".to_string());

    AgentsDataConfig {
        rds_configured,
        postgres_configured,
        supabase_configured,
        nats_configured: first_env(&["NATS_URL"]).is_some(),
        nats_url,
        postgres_plan:
            "This REST API is the database boundary. Point AGENT_TASKS_RDS_DATABASE_URL at RDS today, then swap to an in-cluster Postgres service later."
                .to_string(),
    }
}

fn limit_from_query(query: &AgentsQuery) -> i64 {
    query.limit.unwrap_or(50).clamp(1, 200)
}

fn context_limit_from_query(query: &ContextQuery) -> i64 {
    query.limit.unwrap_or(20).clamp(1, 100)
}

fn event_limit_from_query(query: &ContextQuery) -> i64 {
    query.limit.unwrap_or(100).clamp(1, 500)
}

fn public_data_source_error(source: &str) -> String {
    format!("{source} source unavailable; check remote REST API server logs")
}

fn add_rds_root_certificates(root_store: &mut rustls::RootCertStore) -> Result<(), String> {
    let mut reader = BufReader::new(&include_bytes!("../certs/rds-us-east-1-bundle.pem")[..]);
    let mut added = 0usize;

    for cert in rustls_pemfile::certs(&mut reader) {
        let cert = cert.map_err(|error| format!("failed to parse RDS CA certificate: {error}"))?;
        if root_store.add(cert).is_ok() {
            added += 1;
        }
    }

    if added == 0 {
        return Err("no RDS CA certificates loaded".to_string());
    }

    Ok(())
}

fn public_thread_worker_proxy_error(action: &str) -> String {
    format!("thread worker {action} failed; check remote REST API server logs")
}

fn normalize_repo_url(value: &str) -> Result<String, String> {
    let repo = value.trim();
    if repo.is_empty() {
        return Err("repo is required".to_string());
    }
    if repo.len() > 2048 {
        return Err("repo must be 2048 characters or fewer".to_string());
    }
    if !(repo.starts_with("git@") || repo.starts_with("ssh://") || repo.starts_with("https://")) {
        return Err("repo must start with git@, ssh://, or https://".to_string());
    }
    Ok(repo.to_string())
}

fn normalize_base_branch(value: Option<&str>) -> Result<String, String> {
    let branch = value.unwrap_or("dev").trim();
    if branch.is_empty() {
        return Err("baseBranch must not be empty".to_string());
    }
    if branch.len() > 120 {
        return Err("baseBranch must be 120 characters or fewer".to_string());
    }
    if !branch
        .chars()
        .all(|item| item.is_ascii_alphanumeric() || matches!(item, '.' | '_' | '/' | '-'))
    {
        return Err("baseBranch contains unsupported characters".to_string());
    }
    Ok(branch.to_string())
}

fn infer_repo_display_name(repo_url: &str) -> String {
    repo_url
        .trim_end_matches(".git")
        .rsplit(['/', ':'])
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("Git repository")
        .chars()
        .take(200)
        .collect()
}

fn infer_repo_provider(repo_url: &str) -> String {
    if repo_url.contains("github.com") {
        "github".to_string()
    } else if repo_url.contains("gitlab.com") {
        "gitlab".to_string()
    } else if repo_url.contains("bitbucket.org") {
        "bitbucket".to_string()
    } else {
        "generic".to_string()
    }
}

fn normalize_repo_provider(value: Option<&str>, repo_url: &str) -> Result<String, String> {
    let provider = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| infer_repo_provider(repo_url));
    if matches!(
        provider.as_str(),
        "github" | "gitlab" | "bitbucket" | "generic"
    ) {
        Ok(provider)
    } else {
        Err("provider must be github, gitlab, bitbucket, or generic".to_string())
    }
}

fn normalized_repo_config(request: &DispatchTaskRequest) -> Result<ThreadRepoConfig, String> {
    Ok(ThreadRepoConfig {
        repo: normalize_repo_url(&request.repo)?,
        base_branch: normalize_base_branch(request.base_branch.as_deref())?,
    })
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "unauthorized",
            "errMessage": "missing required dd header",
        })),
    )
        .into_response()
}

fn authorized_internal_request(headers: &HeaderMap) -> bool {
    let Some(expected) = worker_auth_secret() else {
        return false;
    };
    headers
        .get("x-agent-auth")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
}

fn thread_short_id(thread_id: &str) -> String {
    thread_id
        .chars()
        .filter(|value| value.is_ascii_alphanumeric())
        .take(12)
        .collect::<String>()
        .to_lowercase()
}

fn thread_resource_name(thread_id: &str) -> String {
    format!("dd-thread-{}", thread_short_id(thread_id))
}

fn thread_terminal_url(thread_id: &str) -> String {
    format!(
        "/dd-thread/{}/terminal?threadId={thread_id}",
        thread_short_id(thread_id)
    )
}

fn worker_auth_secret() -> Option<String> {
    first_env(&["REMOTE_DEV_SERVER_SECRET", "SERVER_AUTH_SECRET"])
}

fn missing_worker_auth_secret_message() -> &'static str {
    "REMOTE_DEV_SERVER_SECRET or SERVER_AUTH_SECRET is not set"
}

fn nats_url() -> String {
    first_env(&["NATS_URL"])
        .unwrap_or_else(|| "nats://dd-nats.messaging.svc.cluster.local:4222".to_string())
}

fn nats_task_subject(thread_id: &str) -> String {
    format!("dd.remote.thread.{thread_id}.tasks")
}

fn nats_task_stream_subject() -> String {
    first_env(&["NATS_TASK_SUBJECT"]).unwrap_or_else(|| "dd.remote.thread.*.tasks".to_string())
}

fn nats_task_stream_name() -> String {
    first_env(&["NATS_TASK_STREAM"]).unwrap_or_else(|| "DD_REMOTE_TASKS".to_string())
}

fn nats_wakeup_subject() -> &'static str {
    "dd.remote.orchestrator.wakeup"
}

fn nats_lambda_functions_subject() -> &'static str {
    "dd.remote.lambdas.functions"
}

async fn publish_thread_runtime_event_to_nats(
    thread_id: &str,
    task_id: Option<&str>,
    action: &str,
    status: &str,
    message: &str,
) -> Result<(), String> {
    let event_task_id = task_id.unwrap_or(thread_id);
    let now = now_ms();
    let payload = json!({
        "type": "task-event",
        "threadId": thread_id,
        "taskId": event_task_id,
        "seq": now,
        "event": {
            "kind": "thread-runtime",
            "action": action,
            "status": status,
            "message": message,
            "atMs": now
        }
    });
    let body = serde_json::to_vec(&payload).map_err(|error| error.to_string())?;
    let client = async_nats::connect(nats_url())
        .await
        .map_err(|error| error.to_string())?;
    client
        .publish("dd.remote.events", body.into())
        .await
        .map_err(|error| error.to_string())?;
    client.flush().await.map_err(|error| error.to_string())?;
    Ok(())
}

async fn ensure_nats_task_stream(jetstream: &async_nats::jetstream::Context) -> Result<(), String> {
    jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: nats_task_stream_name(),
            subjects: vec![nats_task_stream_subject()],
            retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
            max_age: Duration::from_secs(60 * 60 * 24 * 14),
            max_message_size: 8 * 1024 * 1024,
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn jetstream_publish_task(
    client: async_nats::Client,
    subject: String,
    payload: Vec<u8>,
) -> Result<(), String> {
    let jetstream = async_nats::jetstream::new(client);
    ensure_nats_task_stream(&jetstream).await?;
    jetstream
        .publish(subject, payload.into())
        .await
        .map_err(|error| error.to_string())?
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn validate_thread_control_signal(
    path_thread_id: &str,
    expected_action: &str,
    request: &ThreadControlRequest,
) -> Result<(), String> {
    if request.kind != "thread-control" {
        return Err("control payload kind must be thread-control".to_string());
    }
    if !looks_like_uuid(path_thread_id) {
        return Err("threadId must be a UUID".to_string());
    }
    if request.action != expected_action {
        return Err(format!("control payload action must be {expected_action}"));
    }
    if request.thread_id != path_thread_id {
        return Err("threadId path/body mismatch".to_string());
    }
    if let Some(task_id) = request.task_id.as_deref() {
        if !looks_like_uuid(task_id) {
            return Err("taskId must be a UUID".to_string());
        }
    }
    Ok(())
}

fn thread_runtime_namespace() -> String {
    env::var("THREAD_RUNTIME_NAMESPACE").unwrap_or_else(|_| "default".to_string())
}

fn thread_runtime_image() -> String {
    env::var("THREAD_RUNTIME_IMAGE")
        .unwrap_or_else(|_| "docker.io/library/dd-dev-server:dev".to_string())
}

fn thread_worker_url(namespace: &str, name: &str, path: &str) -> String {
    format!("http://{name}.{namespace}.svc.cluster.local:8080{path}")
}

fn remember_runtime_task(request: &DispatchTaskRequest, branch: Option<String>) {
    let now = now_label();
    if let Ok(mut state) = RUNTIME_STATE.lock() {
        let title = request
            .thread_title
            .clone()
            .unwrap_or_else(|| request.prompt.chars().take(80).collect::<String>());
        state.threads.insert(
            request.thread_id.clone(),
            AgentThreadRow {
                id: request.thread_id.clone(),
                title,
                repo: normalize_repo_url(&request.repo).unwrap_or_else(|_| request.repo.clone()),
                base_branch: normalize_base_branch(request.base_branch.as_deref())
                    .unwrap_or_else(|_| "dev".to_string()),
                archived_at: None,
                created_at: Some(now.clone()),
                updated_at: Some(now.clone()),
                task_count: 1,
                active_task_count: 1,
                latest_task_at: Some(now.clone()),
            },
        );
        state.tasks.insert(
            0,
            AgentTaskRow {
                id: request.task_id.clone(),
                thread_id: request.thread_id.clone(),
                thread_title: request.thread_title.clone(),
                prompt: request.prompt.clone(),
                status: "running".to_string(),
                branch,
                pr_url: None,
                pr_state: None,
                exit_reason: None,
                error_message: None,
                started_at: Some(now.clone()),
                finished_at: None,
                created_at: Some(now.clone()),
                updated_at: Some(now),
                last_event_seq: -1,
                event_count: 0,
                latest_event_kind: Some("dispatch".to_string()),
                latest_payload: None,
            },
        );
        if state.tasks.len() > 200 {
            state.tasks.truncate(200);
        }
    }
}

fn runtime_snapshot(
    limit: i64,
    config: AgentsDataConfig,
    mut errors: Vec<String>,
) -> AgentsSnapshot {
    let state = RUNTIME_STATE.lock().ok();
    let (threads, tasks) = if let Some(state) = state {
        let mut threads = state.threads.values().cloned().collect::<Vec<_>>();
        threads.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        let tasks = state
            .tasks
            .iter()
            .take(limit as usize)
            .cloned()
            .collect::<Vec<_>>();
        (threads, tasks)
    } else {
        errors.push("runtime memory state lock unavailable".to_string());
        (Vec::new(), Vec::new())
    };
    AgentsSnapshot {
        ok: true,
        source: "runtime-memory".to_string(),
        generated_at_ms: now_ms(),
        summary: summarize(&threads, &tasks),
        threads,
        tasks,
        errors,
        config,
    }
}

fn runtime_thread_context(
    thread_id: &str,
    limit: i64,
    mut errors: Vec<String>,
) -> ThreadContextResponse {
    let mut tasks = if let Ok(state) = RUNTIME_STATE.lock() {
        state
            .tasks
            .iter()
            .filter(|task| task.thread_id == thread_id)
            .take(limit as usize)
            .cloned()
            .collect::<Vec<_>>()
    } else {
        errors.push("runtime memory state lock unavailable".to_string());
        Vec::new()
    };
    tasks.reverse();
    ThreadContextResponse {
        ok: true,
        source: "runtime-memory".to_string(),
        thread_id: thread_id.to_string(),
        generated_at_ms: now_ms(),
        tasks,
        errors,
    }
}

async fn k8s_http_client() -> Result<(reqwest::Client, String, String), String> {
    let host = env::var("KUBERNETES_SERVICE_HOST")
        .map_err(|_| "KUBERNETES_SERVICE_HOST is not set".to_string())?;
    let port = env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".to_string());
    let token = fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/token")
        .map_err(|error| format!("failed to read serviceaccount token: {error}"))?;
    let mut builder = reqwest::Client::builder();
    if let Ok(ca) = fs::read("/var/run/secrets/kubernetes.io/serviceaccount/ca.crt") {
        if let Ok(cert) = reqwest::Certificate::from_pem(&ca) {
            builder = builder.add_root_certificate(cert);
        }
    }
    let client = builder
        .build()
        .map_err(|error| format!("failed to build k8s http client: {error}"))?;
    Ok((client, format!("https://{host}:{port}"), token))
}

async fn k8s_json_request(
    method: reqwest::Method,
    path: String,
    body: Option<Value>,
    content_type: &'static str,
) -> Result<ThreadActionResult, String> {
    let (client, base_url, token) = k8s_http_client().await?;
    let mut request = client
        .request(method, format!("{base_url}{path}"))
        .bearer_auth(token.trim())
        .header(reqwest::header::ACCEPT, "application/json");
    if let Some(body) = body {
        request = request
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("k8s request failed: {error}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok(ThreadActionResult {
        resource: path,
        status: status.as_u16(),
        ok: status.is_success() || status == reqwest::StatusCode::NOT_FOUND,
        body: body.chars().take(500).collect(),
    })
}

async fn k8s_create_request(path: String, body: Value) -> Result<ThreadActionResult, String> {
    let (client, base_url, token) = k8s_http_client().await?;
    let response = client
        .post(format!("{base_url}{path}"))
        .bearer_auth(token.trim())
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("k8s create failed: {error}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok(ThreadActionResult {
        resource: path,
        status: status.as_u16(),
        ok: status.is_success() || status == reqwest::StatusCode::CONFLICT,
        body: body.chars().take(500).collect(),
    })
}

async fn k8s_get_value(path: String) -> Result<Option<Value>, String> {
    let (client, base_url, token) = k8s_http_client().await?;
    let response = client
        .get(format!("{base_url}{path}"))
        .bearer_auth(token.trim())
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| format!("k8s get failed: {error}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(format!(
            "k8s get {path} failed {}: {}",
            status.as_u16(),
            body.chars().take(300).collect::<String>()
        ));
    }
    serde_json::from_str::<Value>(&body)
        .map(Some)
        .map_err(|error| format!("k8s get {path} returned invalid json: {error}"))
}

fn json_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |cursor, segment| cursor.get(*segment))
}

fn json_at_string(value: &Value, path: &[&str]) -> Option<String> {
    json_at(value, path)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .filter(|text| !text.is_empty())
}

fn json_at_i64(value: &Value, path: &[&str]) -> Option<i64> {
    json_at(value, path).and_then(Value::as_i64)
}

fn summarize_deployment(deployment: &Value) -> Value {
    let conditions = json_at(deployment, &["status", "conditions"])
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|condition| {
                    json!({
                        "type": json_string(condition, "type"),
                        "status": json_string(condition, "status"),
                        "reason": json_string(condition, "reason"),
                        "message": json_string(condition, "message"),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "name": json_at_string(deployment, &["metadata", "name"]),
        "createdAt": json_at_string(deployment, &["metadata", "creationTimestamp"]),
        "desiredReplicas": json_at_i64(deployment, &["spec", "replicas"]).unwrap_or(0),
        "replicas": json_at_i64(deployment, &["status", "replicas"]).unwrap_or(0),
        "readyReplicas": json_at_i64(deployment, &["status", "readyReplicas"]).unwrap_or(0),
        "availableReplicas": json_at_i64(deployment, &["status", "availableReplicas"]).unwrap_or(0),
        "updatedReplicas": json_at_i64(deployment, &["status", "updatedReplicas"]).unwrap_or(0),
        "unavailableReplicas": json_at_i64(deployment, &["status", "unavailableReplicas"]).unwrap_or(0),
        "observedGeneration": json_at_i64(deployment, &["status", "observedGeneration"]),
        "conditions": conditions,
    })
}

fn summarize_service(service: &Value) -> Value {
    json!({
        "name": json_at_string(service, &["metadata", "name"]),
        "createdAt": json_at_string(service, &["metadata", "creationTimestamp"]),
        "clusterIp": json_at_string(service, &["spec", "clusterIP"]),
        "type": json_at_string(service, &["spec", "type"]),
    })
}

fn summarize_pod(pod: &Value) -> Value {
    let containers = json_at(pod, &["status", "containerStatuses"])
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|container| {
                    json!({
                        "name": json_string(container, "name"),
                        "ready": container.get("ready").and_then(Value::as_bool).unwrap_or(false),
                        "restartCount": json_at_i64(container, &["restartCount"]).unwrap_or(0),
                        "state": container.get("state").cloned().unwrap_or_else(|| json!({})),
                        "lastState": container.get("lastState").cloned().unwrap_or_else(|| json!({})),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "name": json_at_string(pod, &["metadata", "name"]),
        "createdAt": json_at_string(pod, &["metadata", "creationTimestamp"]),
        "phase": json_at_string(pod, &["status", "phase"]),
        "podIp": json_at_string(pod, &["status", "podIP"]),
        "hostIp": json_at_string(pod, &["status", "hostIP"]),
        "startTime": json_at_string(pod, &["status", "startTime"]),
        "deletionTimestamp": json_at_string(pod, &["metadata", "deletionTimestamp"]),
        "containers": containers,
    })
}

fn summarize_thread_runtime(deployment: Option<&Value>, pods: &[Value]) -> Value {
    let desired = deployment
        .and_then(|value| json_at_i64(value, &["desiredReplicas"]))
        .unwrap_or(0);
    let available = deployment
        .and_then(|value| json_at_i64(value, &["availableReplicas"]))
        .unwrap_or(0);
    let ready = deployment
        .and_then(|value| json_at_i64(value, &["readyReplicas"]))
        .unwrap_or(0);
    let ready_pods = pods
        .iter()
        .filter(|pod| {
            json_at(pod, &["containers"])
                .and_then(Value::as_array)
                .is_some_and(|containers| {
                    !containers.is_empty()
                        && containers.iter().all(|container| {
                            container.get("ready").and_then(Value::as_bool) == Some(true)
                        })
                })
        })
        .count();
    let phase = if deployment.is_none() {
        "missing"
    } else if desired == 0 {
        "sleeping"
    } else if available > 0 && ready > 0 {
        "ready"
    } else if pods.is_empty() {
        "creating"
    } else {
        "starting"
    };
    json!({
        "phase": phase,
        "desiredReplicas": desired,
        "readyReplicas": ready,
        "availableReplicas": available,
        "podCount": pods.len(),
        "readyPodCount": ready_pods,
    })
}

fn render_thread_service(namespace: &str, name: &str, thread_id: &str) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": {
            "name": name,
            "namespace": namespace,
            "labels": {
                "app.kubernetes.io/part-of": "dd-remote-dev",
                "app.kubernetes.io/component": "thread-pod",
                "dd/threadId": thread_id
            }
        },
        "spec": {
            "type": "ClusterIP",
            "selector": { "dd/threadId": thread_id },
            "ports": [{ "name": "http", "port": 8080, "targetPort": "http" }]
        }
    })
}

fn render_thread_deployment(
    namespace: &str,
    name: &str,
    thread_id: &str,
    repo_url: &str,
    base_branch: &str,
) -> Value {
    let image = thread_runtime_image();
    json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": {
            "name": name,
            "namespace": namespace,
            "labels": {
                "app.kubernetes.io/part-of": "dd-remote-dev",
                "app.kubernetes.io/component": "thread-pod",
                "dd/threadId": thread_id
            }
        },
        "spec": {
            "replicas": 1,
            "strategy": { "type": "Recreate" },
            "selector": { "matchLabels": { "dd/threadId": thread_id } },
            "template": {
                "metadata": {
                    "labels": {
                        "app.kubernetes.io/part-of": "dd-remote-dev",
                        "app.kubernetes.io/component": "thread-pod",
                        "dd/threadId": thread_id
                    }
                },
                "spec": {
                    "terminationGracePeriodSeconds": 30,
                    "initContainers": [{
                        "name": "workspace-permissions",
                        "image": "docker.io/library/busybox:1.36",
                        "imagePullPolicy": "IfNotPresent",
                        "command": ["/bin/sh", "-c"],
                        "args": ["mkdir -p /home/node/workspace /tmp/convos && chown -R 1000:1000 /home/node/workspace /tmp/convos"],
                        "volumeMounts": [
                            { "name": "workspace", "mountPath": "/home/node/workspace" },
                            { "name": "tmp-convos", "mountPath": "/tmp/convos" }
                        ]
                    }],
                    "containers": [{
                        "name": "dev-server",
                        "image": image,
                        "imagePullPolicy": "IfNotPresent",
                        "securityContext": {
                            "runAsNonRoot": true,
                            "runAsUser": 1000,
                            "runAsGroup": 1000
                        },
                        "ports": [{ "containerPort": 8080, "name": "http" }],
                        "env": [
                            { "name": "REMOTE_DEV_THREAD_ID", "value": thread_id },
                            { "name": "DD_REPO_URL", "value": repo_url },
                            { "name": "BASE_BRANCH", "value": base_branch },
                            { "name": "IDLE_TIMEOUT_MS", "value": "0" },
                            { "name": "OTEL_SERVICE_NAME", "value": name },
                            { "name": "OTEL_EXPORTER_OTLP_ENDPOINT", "value": "http://dd-otel-collector.observability.svc.cluster.local:4318" },
                            { "name": "THREAD_CONTEXT_BASE_URL", "value": "http://dd-remote-rest-api.default.svc.cluster.local:8082" },
                            { "name": "EVENT_INGEST_URL", "value": "http://dd-remote-rest-api.default.svc.cluster.local:8082/api/agents/events" },
                            { "name": "EVENT_INGEST_SECRET", "valueFrom": { "secretKeyRef": { "name": "dd-agent-secrets", "key": "SERVER_AUTH_SECRET" } } },
                            { "name": "NATS_URL", "value": "nats://dd-nats.messaging.svc.cluster.local:4222" },
                            { "name": "NATS_EVENT_SUBJECT", "value": "dd.remote.events" }
                        ],
                        "envFrom": [
                            { "secretRef": { "name": "dd-agent-secrets", "optional": true } }
                        ],
                        "volumeMounts": [
                            { "name": "workspace", "mountPath": "/home/node/workspace" },
                            { "name": "tmp-convos", "mountPath": "/tmp/convos" }
                        ],
                        "resources": {
                            "requests": { "cpu": "1", "memory": "2Gi" },
                            "limits": { "cpu": "2", "memory": "4Gi" }
                        },
                        "startupProbe": {
                            "httpGet": { "path": "/healthz", "port": "http" },
                            "periodSeconds": 5,
                            "failureThreshold": 180
                        },
                        "livenessProbe": {
                            "httpGet": { "path": "/healthz", "port": "http" },
                            "periodSeconds": 30,
                            "timeoutSeconds": 5,
                            "failureThreshold": 3
                        },
                        "readinessProbe": {
                            "httpGet": { "path": "/healthz", "port": "http" },
                            "periodSeconds": 10,
                            "timeoutSeconds": 3,
                            "failureThreshold": 2
                        }
                    }],
                    "volumes": [
                        {
                            "name": "workspace",
                            "hostPath": {
                                "path": format!("/home/ec2-user/codes/dd/thread-workspaces/{name}"),
                                "type": "DirectoryOrCreate"
                            }
                        },
                        {
                            "name": "tmp-convos",
                            "emptyDir": { "sizeLimit": "256Mi" }
                        }
                    ]
                }
            }
        }
    })
}

async fn ensure_thread_worker(
    thread_id: &str,
    repo_url: &str,
    base_branch: &str,
) -> Result<(String, String, Vec<ThreadActionResult>), String> {
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(thread_id);
    let mut results = Vec::new();
    let deployment = render_thread_deployment(&namespace, &name, thread_id, repo_url, base_branch);

    results.push(
        k8s_create_request(
            format!("/api/v1/namespaces/{namespace}/services"),
            render_thread_service(&namespace, &name, thread_id),
        )
        .await?,
    );
    results.push(
        k8s_create_request(
            format!("/apis/apps/v1/namespaces/{namespace}/deployments"),
            deployment.clone(),
        )
        .await?,
    );
    results.push(
        k8s_json_request(
            reqwest::Method::PATCH,
            format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}"),
            Some(json!({ "spec": deployment["spec"].clone() })),
            "application/merge-patch+json",
        )
        .await?,
    );
    results.push(
        k8s_json_request(
            reqwest::Method::PATCH,
            format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}/scale"),
            Some(json!({ "spec": { "replicas": 1 } })),
            "application/merge-patch+json",
        )
        .await?,
    );

    Ok((namespace, name, results))
}

async fn prepare_thread_worker(thread_id: &str) -> Result<ThreadActionResponse, String> {
    let repo_config = fetch_thread_repo_config_from_postgres(thread_id)
        .await?
        .ok_or_else(|| "thread repo config is not configured".to_string())?;
    let (namespace, name, results) =
        ensure_thread_worker(thread_id, &repo_config.repo, &repo_config.base_branch).await?;
    let Some(secret) = worker_auth_secret() else {
        return Err(missing_worker_auth_secret_message().to_string());
    };
    wait_thread_worker_ready(&namespace, &name, &secret).await?;

    Ok(ThreadActionResponse {
        ok: true,
        action: "prepare".to_string(),
        thread_id: thread_id.to_string(),
        k8s_name: name,
        namespace,
        results,
        errors: Vec::new(),
    })
}

async fn scale_thread_runtime(
    thread_id: String,
    action: &'static str,
    replicas: i32,
    task_id: Option<String>,
) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/scale",
        StatusCode::OK,
    );
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(&thread_id);
    let path = format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}/scale");
    let mut response = ThreadActionResponse {
        ok: false,
        action: action.to_string(),
        thread_id,
        k8s_name: name,
        namespace,
        results: Vec::new(),
        errors: Vec::new(),
    };
    match k8s_json_request(
        reqwest::Method::PATCH,
        path,
        Some(json!({ "spec": { "replicas": replicas } })),
        "application/merge-patch+json",
    )
    .await
    {
        Ok(result) => {
            response.ok = result.ok;
            response.results.push(result);
        }
        Err(error) => response.errors.push(error),
    }
    if response.ok {
        let status = match action {
            "sleep" => "sleeping",
            "archive" => "archived",
            _ if replicas == 0 => "suspended",
            _ => "awake",
        };
        if let Err(error) = publish_thread_runtime_event_to_nats(
            &response.thread_id,
            task_id.as_deref(),
            action,
            status,
            "thread runtime scaled",
        )
        .await
        {
            eprintln!("failed to publish thread runtime event: {error}");
        }
    }
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_GATEWAY
    };
    (status, Json(response)).into_response()
}

async fn delete_thread_runtime(thread_id: String, task_id: Option<String>) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/hard-delete",
        StatusCode::OK,
    );
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(&thread_id);
    let resources = [
        format!("/apis/networking.k8s.io/v1/namespaces/{namespace}/ingresses/{name}"),
        format!("/api/v1/namespaces/{namespace}/services/{name}"),
        format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}"),
        format!("/api/v1/namespaces/{namespace}/persistentvolumeclaims/{name}"),
    ];
    let mut response = ThreadActionResponse {
        ok: false,
        action: "hard-delete".to_string(),
        thread_id,
        k8s_name: name,
        namespace,
        results: Vec::new(),
        errors: Vec::new(),
    };
    for path in resources {
        match k8s_json_request(reqwest::Method::DELETE, path, None, "application/json").await {
            Ok(result) => response.results.push(result),
            Err(error) => response.errors.push(error),
        }
    }
    response.ok = response.errors.is_empty() && response.results.iter().all(|result| result.ok);
    if response.ok {
        if let Err(error) = publish_thread_runtime_event_to_nats(
            &response.thread_id,
            task_id.as_deref(),
            "hard-delete",
            "deleted",
            "thread runtime resources deleted",
        )
        .await
        {
            eprintln!("failed to publish thread runtime event: {error}");
        }
    }
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_GATEWAY
    };
    (status, Json(response)).into_response()
}

async fn wait_thread_worker_ready(namespace: &str, name: &str, secret: &str) -> Result<(), String> {
    let client = reqwest::Client::new();
    let url = thread_worker_url(namespace, name, "/healthz");
    for _ in 0..100 {
        if let Ok(response) = client
            .get(&url)
            .header("X-Server-Auth", secret)
            .send()
            .await
        {
            if response.status().is_success() {
                return Ok(());
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
    Err("thread worker readiness timed out".to_string())
}

async fn ensure_thread_worker_for_control(
    thread_id: &str,
    action: &'static str,
    task_id: Option<&str>,
    waking_message: &'static str,
    awake_message: &'static str,
) -> Result<(String, String, String), ThreadActionResponse> {
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(thread_id);
    let Some(secret) = worker_auth_secret() else {
        return Err(ThreadActionResponse {
            ok: false,
            action: action.to_string(),
            thread_id: thread_id.to_string(),
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![missing_worker_auth_secret_message().to_string()],
        });
    };

    if let Err(error) =
        publish_thread_runtime_event_to_nats(thread_id, task_id, action, "waking", waking_message)
            .await
    {
        eprintln!("failed to publish thread runtime event: {error}");
    }

    let repo_config = match fetch_thread_repo_config_from_postgres(thread_id).await {
        Ok(Some(repo_config)) => repo_config,
        Ok(None) => {
            return Err(ThreadActionResponse {
                ok: false,
                action: action.to_string(),
                thread_id: thread_id.to_string(),
                k8s_name: name,
                namespace,
                results: Vec::new(),
                errors: vec!["thread repo config is not configured".to_string()],
            });
        }
        Err(error) => {
            return Err(ThreadActionResponse {
                ok: false,
                action: action.to_string(),
                thread_id: thread_id.to_string(),
                k8s_name: name,
                namespace,
                results: Vec::new(),
                errors: vec![error],
            });
        }
    };

    let (namespace, name, _results) =
        match ensure_thread_worker(thread_id, &repo_config.repo, &repo_config.base_branch).await {
            Ok(result) => result,
            Err(error) => {
                return Err(ThreadActionResponse {
                    ok: false,
                    action: action.to_string(),
                    thread_id: thread_id.to_string(),
                    k8s_name: name,
                    namespace,
                    results: Vec::new(),
                    errors: vec![error],
                });
            }
        };

    if let Err(error) = wait_thread_worker_ready(&namespace, &name, &secret).await {
        return Err(ThreadActionResponse {
            ok: false,
            action: action.to_string(),
            thread_id: thread_id.to_string(),
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![error],
        });
    }

    if let Err(error) =
        publish_thread_runtime_event_to_nats(thread_id, task_id, action, "awake", awake_message)
            .await
    {
        eprintln!("failed to publish thread runtime event: {error}");
    }

    Ok((namespace, name, secret))
}

async fn merge_thread_upstream(thread_id: String, request: ThreadControlRequest) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/merge-upstream",
        StatusCode::OK,
    );
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(&thread_id);
    let Some(secret) = worker_auth_secret() else {
        let response = ThreadActionResponse {
            ok: false,
            action: "merge-upstream".to_string(),
            thread_id,
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![missing_worker_auth_secret_message().to_string()],
        };
        return (StatusCode::BAD_GATEWAY, Json(response)).into_response();
    };

    let scale_path = format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}/scale");
    if let Err(error) = publish_thread_runtime_event_to_nats(
        &thread_id,
        request.task_id.as_deref(),
        "merge-upstream",
        "waking",
        "waking thread runtime for merge",
    )
    .await
    {
        eprintln!("failed to publish thread runtime event: {error}");
    }
    if let Err(error) = k8s_json_request(
        reqwest::Method::PATCH,
        scale_path,
        Some(json!({ "spec": { "replicas": 1 } })),
        "application/merge-patch+json",
    )
    .await
    {
        let response = ThreadActionResponse {
            ok: false,
            action: "merge-upstream".to_string(),
            thread_id,
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![error],
        };
        return (StatusCode::BAD_GATEWAY, Json(response)).into_response();
    }

    if let Err(error) = wait_thread_worker_ready(&namespace, &name, &secret).await {
        let response = ThreadActionResponse {
            ok: false,
            action: "merge-upstream".to_string(),
            thread_id,
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![error],
        };
        return (StatusCode::BAD_GATEWAY, Json(response)).into_response();
    }
    if let Err(error) = publish_thread_runtime_event_to_nats(
        &thread_id,
        request.task_id.as_deref(),
        "merge-upstream",
        "awake",
        "thread runtime ready for merge",
    )
    .await
    {
        eprintln!("failed to publish thread runtime event: {error}");
    }

    let client = reqwest::Client::new();
    let worker_response = client
        .post(thread_worker_url(
            &namespace,
            &name,
            "/thread/merge-upstream",
        ))
        .header("X-Server-Auth", secret)
        .json(&json!({
            "kind": "thread-control",
            "action": "merge-upstream",
            "threadId": thread_id.clone(),
            "taskId": request.task_id,
            "requestedBy": request.requested_by,
            "reason": request.reason,
        }))
        .send()
        .await;
    match worker_response {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let public_status =
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            (
                public_status,
                [(header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response()
        }
        Err(error) => {
            let response = ThreadActionResponse {
                ok: false,
                action: "merge-upstream".to_string(),
                thread_id,
                k8s_name: name,
                namespace,
                results: Vec::new(),
                errors: vec![error.to_string()],
            };
            (StatusCode::BAD_GATEWAY, Json(response)).into_response()
        }
    }
}

async fn open_thread_pr(thread_id: String, request: ThreadControlRequest) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/open-pr",
        StatusCode::OK,
    );
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(&thread_id);
    let Some(secret) = worker_auth_secret() else {
        let response = ThreadActionResponse {
            ok: false,
            action: "open-pr".to_string(),
            thread_id,
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![missing_worker_auth_secret_message().to_string()],
        };
        return (StatusCode::BAD_GATEWAY, Json(response)).into_response();
    };

    let scale_path = format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}/scale");
    if let Err(error) = publish_thread_runtime_event_to_nats(
        &thread_id,
        request.task_id.as_deref(),
        "open-pr",
        "waking",
        "waking thread runtime for draft PR",
    )
    .await
    {
        eprintln!("failed to publish thread runtime event: {error}");
    }
    if let Err(error) = k8s_json_request(
        reqwest::Method::PATCH,
        scale_path,
        Some(json!({ "spec": { "replicas": 1 } })),
        "application/merge-patch+json",
    )
    .await
    {
        let response = ThreadActionResponse {
            ok: false,
            action: "open-pr".to_string(),
            thread_id,
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![error],
        };
        return (StatusCode::BAD_GATEWAY, Json(response)).into_response();
    }
    if let Err(error) = wait_thread_worker_ready(&namespace, &name, &secret).await {
        let response = ThreadActionResponse {
            ok: false,
            action: "open-pr".to_string(),
            thread_id,
            k8s_name: name,
            namespace,
            results: Vec::new(),
            errors: vec![error],
        };
        return (StatusCode::BAD_GATEWAY, Json(response)).into_response();
    }
    if let Err(error) = publish_thread_runtime_event_to_nats(
        &thread_id,
        request.task_id.as_deref(),
        "open-pr",
        "awake",
        "thread runtime ready for draft PR",
    )
    .await
    {
        eprintln!("failed to publish thread runtime event: {error}");
    }

    let client = reqwest::Client::new();
    let worker_response = client
        .post(thread_worker_url(&namespace, &name, "/thread/open-pr"))
        .header("X-Server-Auth", secret)
        .json(&json!({
            "kind": "thread-control",
            "action": "open-pr",
            "threadId": thread_id.clone(),
            "taskId": request.task_id,
            "requestedBy": request.requested_by,
            "reason": request.reason,
        }))
        .send()
        .await;
    match worker_response {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let public_status =
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            (
                public_status,
                [(header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response()
        }
        Err(error) => {
            let response = ThreadActionResponse {
                ok: false,
                action: "open-pr".to_string(),
                thread_id,
                k8s_name: name,
                namespace,
                results: Vec::new(),
                errors: vec![error.to_string()],
            };
            (StatusCode::BAD_GATEWAY, Json(response)).into_response()
        }
    }
}

async fn make_thread_commit(thread_id: String, request: ThreadControlRequest) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/make-commit",
        StatusCode::OK,
    );
    let (namespace, name, secret) = match ensure_thread_worker_for_control(
        &thread_id,
        "make-commit",
        request.task_id.as_deref(),
        "waking thread runtime for commit",
        "thread runtime ready for commit",
    )
    .await
    {
        Ok(result) => result,
        Err(response) => return (StatusCode::BAD_GATEWAY, Json(response)).into_response(),
    };

    let client = reqwest::Client::new();
    let worker_response = client
        .post(thread_worker_url(&namespace, &name, "/thread/make-commit"))
        .header("X-Server-Auth", secret)
        .json(&json!({
            "kind": "thread-control",
            "action": "make-commit",
            "threadId": thread_id.clone(),
            "taskId": request.task_id,
            "requestedBy": request.requested_by,
            "reason": request.reason,
        }))
        .send()
        .await;
    match worker_response {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let public_status =
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            (
                public_status,
                [(header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response()
        }
        Err(error) => {
            let response = ThreadActionResponse {
                ok: false,
                action: "make-commit".to_string(),
                thread_id,
                k8s_name: name,
                namespace,
                results: Vec::new(),
                errors: vec![error.to_string()],
            };
            (StatusCode::BAD_GATEWAY, Json(response)).into_response()
        }
    }
}

async fn open_thread_terminal(thread_id: String, request: ThreadControlRequest) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/terminal",
        StatusCode::OK,
    );
    let (namespace, name, _secret) = match ensure_thread_worker_for_control(
        &thread_id,
        "terminal",
        request.task_id.as_deref(),
        "waking thread runtime for terminal",
        "thread runtime ready for terminal",
    )
    .await
    {
        Ok(result) => result,
        Err(response) => return (StatusCode::BAD_GATEWAY, Json(response)).into_response(),
    };

    let terminal_url = thread_terminal_url(&thread_id);
    Json(json!({
        "ok": true,
        "action": "terminal",
        "threadId": thread_id,
        "k8sName": name,
        "namespace": namespace,
        "terminalUrl": terminal_url,
    }))
    .into_response()
}

fn summarize(threads: &[AgentThreadRow], tasks: &[AgentTaskRow]) -> AgentsSummary {
    let mut summary = AgentsSummary {
        thread_count: threads.len(),
        task_count: tasks.len(),
        ..AgentsSummary::default()
    };

    for task in tasks {
        match task.status.as_str() {
            "queued" | "running" | "streaming" => summary.running_count += 1,
            "failed" | "cancelled" => summary.failed_count += 1,
            "done" | "pushed" | "pr_open" | "pr_merged" | "pr_closed" => {
                summary.done_count += 1;
            }
            _ => {}
        }
        if task.pr_url.is_some() {
            summary.pr_count += 1;
        }
    }

    summary
}

fn row_string(row: &tokio_postgres::Row, column: &str) -> String {
    row.try_get::<_, String>(column).unwrap_or_default()
}

fn row_opt_string(row: &tokio_postgres::Row, column: &str) -> Option<String> {
    row.try_get::<_, Option<String>>(column)
        .ok()
        .flatten()
        .filter(|value| !value.is_empty())
}

fn row_i32(row: &tokio_postgres::Row, column: &str) -> i32 {
    row.try_get::<_, i32>(column).unwrap_or_default()
}

fn row_i64(row: &tokio_postgres::Row, column: &str) -> i64 {
    row.try_get::<_, i64>(column).unwrap_or_default()
}

fn row_bool(row: &tokio_postgres::Row, column: &str) -> bool {
    row.try_get::<_, bool>(column).unwrap_or_default()
}

fn row_value(row: &tokio_postgres::Row, column: &str, fallback: Value) -> Value {
    row.try_get::<_, Value>(column).unwrap_or(fallback)
}

fn row_to_lambda_function(row: &tokio_postgres::Row) -> LambdaFunctionRow {
    LambdaFunctionRow {
        id: row_string(row, "id"),
        slug: row_string(row, "slug"),
        display_name: row_string(row, "display_name"),
        description: row_string(row, "description"),
        runtime: row_string(row, "runtime"),
        entry_command: row_string(row, "entry_command"),
        function_body: row_string(row, "function_body"),
        reuse_key: row_opt_string(row, "reuse_key"),
        idle_timeout_seconds: row_i32(row, "idle_timeout_seconds"),
        max_run_ms: row_i32(row, "max_run_ms"),
        containerized: row_bool(row, "containerized"),
        container_image: row_opt_string(row, "container_image"),
        container_build_status: row_string(row, "container_build_status"),
        container_build_error: row_opt_string(row, "container_build_error"),
        container_built_at: row_opt_string(row, "container_built_at"),
        status: row_string(row, "status"),
        labels: row_value(row, "labels", json!([])),
        meta_data: row_value(row, "meta_data", json!({})),
        last_invoked_at: row_opt_string(row, "last_invoked_at"),
        created_at: row_opt_string(row, "created_at"),
        updated_at: row_opt_string(row, "updated_at"),
    }
}

fn lambda_limit_from_query(query: &LambdasQuery) -> i64 {
    query.limit.unwrap_or(100).clamp(1, 250)
}

fn lambda_search_pattern(query: &LambdasQuery) -> String {
    query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{value}%"))
        .unwrap_or_default()
}

fn normalize_lambda_slug(input: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for ch in input.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

fn looks_like_uuid(input: &str) -> bool {
    let bytes = input.as_bytes();
    if bytes.len() != 36 {
        return false;
    }

    bytes.iter().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            *byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}

fn validate_lambda_status(input: Option<&str>) -> String {
    match input.unwrap_or("draft").trim() {
        "draft" => "draft".to_string(),
        "active" => "active".to_string(),
        "paused" => "paused".to_string(),
        "archived" => "archived".to_string(),
        _ => "draft".to_string(),
    }
}

fn normalize_lambda_runtime_alias(input: &str) -> Option<&'static str> {
    match input.trim() {
        "node" | "nodejs" | "javascript" | "typescript" => Some("nodejs"),
        "python" | "python3" => Some("python3"),
        "ruby" => Some("ruby"),
        "bash" | "shell" => Some("bash"),
        _ => None,
    }
}

fn validate_lambda_runtime(input: Option<&str>) -> Result<String, String> {
    let value = input.unwrap_or("javascript");
    normalize_lambda_runtime_alias(value)
        .map(ToString::to_string)
        .ok_or_else(|| "runtime must be one of nodejs, python3, ruby, or bash".to_string())
}

fn lambda_host_runtime_allowed(runtime: &str) -> bool {
    env::var("LAMBDA_ALLOW_HOST_RUNTIMES")
        .unwrap_or_else(|_| "nodejs".to_string())
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter_map(normalize_lambda_runtime_alias)
        .any(|allowed| allowed == runtime)
}

fn validate_lambda_reuse_key(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(reuse_key) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if reuse_key.len() > 120 {
        return Err("reuseKey must be 120 characters or fewer".to_string());
    }
    if !reuse_key
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | ':' | '-'))
        || !reuse_key
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
    {
        return Err(
            "reuseKey may contain only ASCII letters, numbers, '.', '_', ':', and '-' and must start with a letter or number"
                .to_string(),
        );
    }
    Ok(Some(reuse_key.to_string()))
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .filter(|text| !text.is_empty())
}

fn json_i32(value: &Value, key: &str) -> i32 {
    value
        .get(key)
        .and_then(Value::as_i64)
        .and_then(|number| i32::try_from(number).ok())
        .unwrap_or_default()
}

fn json_i64(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or_default()
}

async fn fetch_agents_snapshot(limit: i64) -> AgentsSnapshot {
    let config = data_config();
    let mut errors = Vec::new();

    if config.postgres_configured {
        match fetch_agents_from_postgres(limit).await {
            Ok((threads, tasks)) => {
                return AgentsSnapshot {
                    ok: true,
                    source: if config.rds_configured {
                        "rds-postgres".to_string()
                    } else {
                        "postgres".to_string()
                    },
                    generated_at_ms: now_ms(),
                    summary: summarize(&threads, &tasks),
                    threads,
                    tasks,
                    errors,
                    config,
                };
            }
            Err(error) => {
                eprintln!("agent tasks postgres data source error: {error}");
                errors.push(public_data_source_error("postgres"));
            }
        }
    }

    if config.supabase_configured {
        match fetch_agents_from_supabase(limit).await {
            Ok((threads, tasks)) => {
                return AgentsSnapshot {
                    ok: true,
                    source: "supabase".to_string(),
                    generated_at_ms: now_ms(),
                    summary: summarize(&threads, &tasks),
                    threads,
                    tasks,
                    errors,
                    config,
                };
            }
            Err(error) => {
                eprintln!("agent tasks supabase data source error: {error}");
                errors.push(public_data_source_error("supabase"));
            }
        }
    }

    if !config.postgres_configured && !config.supabase_configured {
        errors.push(
            "agent tasks data source is not configured; showing runtime memory only".to_string(),
        );
    }

    runtime_snapshot(limit, config, errors)
}

async fn connect_postgres() -> Result<tokio_postgres::Client, String> {
    let database_url = postgres_database_url()
        .ok_or_else(|| "postgres database URL not configured".to_string())?;
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    add_rds_root_certificates(&mut root_store)?;
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
    let (client, connection) = tokio_postgres::connect(&database_url, tls)
        .await
        .map_err(|error| error.to_string())?;

    tokio::spawn(async move {
        if let Err(error) = connection.await {
            eprintln!("agent tasks postgres connection error: {error}");
        }
    });
    Ok(client)
}

async fn fetch_agents_from_postgres(
    limit: i64,
) -> Result<(Vec<AgentThreadRow>, Vec<AgentTaskRow>), String> {
    let client = connect_postgres().await?;

    let thread_rows = client
        .query(
            r#"
            select
              th.id::text as id,
              th.title as title,
              th.repo as repo,
              th.base_branch as base_branch,
              to_char(th.archived_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as archived_at,
              to_char(th.created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at,
              to_char(th.updated_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as updated_at,
              count(t.id)::bigint as task_count,
              count(t.id) filter (where t.status in ('queued', 'running', 'streaming'))::bigint as active_task_count,
              to_char(max(t.created_at) at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as latest_task_at
            from agent_remote_dev_threads th
            left join agent_remote_dev_tasks t
              on t.thread_id = th.id and t.is_soft_deleted = false
            where th.is_soft_deleted = false
            group by th.id, th.title, th.repo, th.base_branch, th.archived_at, th.created_at, th.updated_at
            order by coalesce(max(t.created_at), th.updated_at, th.created_at) desc
            limit $1
            "#,
            &[&limit],
        )
        .await
        .map_err(|error| error.to_string())?;

    let task_rows = client
        .query(
            r#"
            select
              t.id::text as id,
              t.thread_id::text as thread_id,
              th.title as thread_title,
              t.prompt as prompt,
              t.status as status,
              t.branch as branch,
              t.pr_url as pr_url,
              t.pr_state as pr_state,
              t.exit_reason as exit_reason,
              t.error_message as error_message,
              to_char(t.started_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as started_at,
              to_char(t.finished_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as finished_at,
              to_char(t.created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at,
              to_char(t.updated_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as updated_at,
              t.last_event_seq as last_event_seq,
              coalesce(e.event_count, 0)::bigint as event_count,
              le.event_kind as latest_event_kind,
              left(le.payload::text, 1200) as latest_payload
            from agent_remote_dev_tasks t
            left join agent_remote_dev_threads th on th.id = t.thread_id
            left join lateral (
              select count(*)::bigint as event_count
              from agent_remote_dev_events ev
              where ev.task_id = t.id
            ) e on true
            left join lateral (
              select ev.event_kind, ev.payload
              from agent_remote_dev_events ev
              where ev.task_id = t.id
              order by ev.seq desc
              limit 1
            ) le on true
            where t.is_soft_deleted = false
            order by t.created_at desc
            limit $1
            "#,
            &[&limit],
        )
        .await
        .map_err(|error| error.to_string())?;

    let threads = thread_rows
        .iter()
        .map(|row| AgentThreadRow {
            id: row_string(row, "id"),
            title: row_string(row, "title"),
            repo: row_string(row, "repo"),
            base_branch: row_string(row, "base_branch"),
            archived_at: row_opt_string(row, "archived_at"),
            created_at: row_opt_string(row, "created_at"),
            updated_at: row_opt_string(row, "updated_at"),
            task_count: row_i64(row, "task_count"),
            active_task_count: row_i64(row, "active_task_count"),
            latest_task_at: row_opt_string(row, "latest_task_at"),
        })
        .collect();

    let tasks = task_rows
        .iter()
        .map(|row| AgentTaskRow {
            id: row_string(row, "id"),
            thread_id: row_string(row, "thread_id"),
            thread_title: row_opt_string(row, "thread_title"),
            prompt: row_string(row, "prompt"),
            status: row_string(row, "status"),
            branch: row_opt_string(row, "branch"),
            pr_url: row_opt_string(row, "pr_url"),
            pr_state: row_opt_string(row, "pr_state"),
            exit_reason: row_opt_string(row, "exit_reason"),
            error_message: row_opt_string(row, "error_message"),
            started_at: row_opt_string(row, "started_at"),
            finished_at: row_opt_string(row, "finished_at"),
            created_at: row_opt_string(row, "created_at"),
            updated_at: row_opt_string(row, "updated_at"),
            last_event_seq: row_i32(row, "last_event_seq"),
            event_count: row_i64(row, "event_count"),
            latest_event_kind: row_opt_string(row, "latest_event_kind"),
            latest_payload: row_opt_string(row, "latest_payload"),
        })
        .collect();

    Ok((threads, tasks))
}

async fn fetch_known_git_repos_from_postgres(limit: i64) -> Result<Vec<KnownGitRepoRow>, String> {
    let client = connect_postgres().await?;
    let rows = client
        .query(
            r#"
            select
              id::text as id,
              repo_url,
              display_name,
              provider,
              default_branch,
              status,
              to_char(last_verified_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as last_verified_at,
              to_char(created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at,
              to_char(updated_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as updated_at
            from known_git_repos
            where is_soft_deleted = false
            order by updated_at desc
            limit $1
            "#,
            &[&limit],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(rows
        .iter()
        .map(|row| KnownGitRepoRow {
            id: row_string(row, "id"),
            repo_url: row_string(row, "repo_url"),
            display_name: row_string(row, "display_name"),
            provider: row_string(row, "provider"),
            default_branch: row_string(row, "default_branch"),
            status: row_string(row, "status"),
            last_verified_at: row_opt_string(row, "last_verified_at"),
            created_at: row_opt_string(row, "created_at"),
            updated_at: row_opt_string(row, "updated_at"),
        })
        .collect())
}

async fn upsert_known_git_repo_to_postgres(
    repo_url: &str,
    display_name: Option<&str>,
    provider: Option<&str>,
    default_branch: Option<&str>,
) -> Result<KnownGitRepoRow, String> {
    let client = connect_postgres().await?;
    let admin_user_id = agent_tasks_admin_user_id();
    let repo_url = normalize_repo_url(repo_url)?;
    let display_name = display_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(200).collect::<String>())
        .unwrap_or_else(|| infer_repo_display_name(&repo_url));
    let provider = provider.map(str::trim).filter(|value| !value.is_empty());
    let provider = normalize_repo_provider(provider, &repo_url)?;
    let default_branch = normalize_base_branch(default_branch)?;

    let row = client
        .query_one(
            r#"
            insert into known_git_repos
              (repo_url, display_name, provider, default_branch, status, is_soft_deleted, created_at, updated_at, created_by, updated_by)
            values
              ($1, $2, $3, $4, 'active', false, now(), now(), $5::text::uuid, $5::text::uuid)
            on conflict (repo_url) where is_soft_deleted = false do update set
              display_name = excluded.display_name,
              provider = excluded.provider,
              default_branch = excluded.default_branch,
              status = 'active',
              updated_by = excluded.updated_by,
              updated_at = now()
            returning
              id::text as id,
              repo_url,
              display_name,
              provider,
              default_branch,
              status,
              to_char(last_verified_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as last_verified_at,
              to_char(created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at,
              to_char(updated_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as updated_at
            "#,
            &[&repo_url, &display_name, &provider, &default_branch, &admin_user_id],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(KnownGitRepoRow {
        id: row_string(&row, "id"),
        repo_url: row_string(&row, "repo_url"),
        display_name: row_string(&row, "display_name"),
        provider: row_string(&row, "provider"),
        default_branch: row_string(&row, "default_branch"),
        status: row_string(&row, "status"),
        last_verified_at: row_opt_string(&row, "last_verified_at"),
        created_at: row_opt_string(&row, "created_at"),
        updated_at: row_opt_string(&row, "updated_at"),
    })
}

async fn fetch_thread_repo_config_from_postgres(
    thread_id: &str,
) -> Result<Option<ThreadRepoConfig>, String> {
    if postgres_database_url().is_none() {
        return Ok(None);
    }
    let client = connect_postgres().await?;
    let row = client
        .query_opt(
            r#"
            select repo, base_branch
            from agent_remote_dev_threads
            where id = $1::text::uuid
              and is_soft_deleted = false
            limit 1
            "#,
            &[&thread_id],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(row.map(|row| ThreadRepoConfig {
        repo: row_string(&row, "repo"),
        base_branch: row_string(&row, "base_branch"),
    }))
}

fn lambda_select_sql() -> &'static str {
    r#"
    select
      id::text as id,
      slug,
      display_name,
      description,
      runtime,
      entry_command,
      function_body,
      reuse_key,
      idle_timeout_seconds,
      max_run_ms,
      containerized,
      container_image,
      container_build_status,
      container_build_error,
      to_char(container_built_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as container_built_at,
      status,
      labels,
      meta_data,
      to_char(last_invoked_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as last_invoked_at,
      to_char(created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at,
      to_char(updated_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as updated_at
    from lambda_functions
    "#
}

fn lambda_entry_command_for_runtime(runtime: &str) -> String {
    match runtime {
        "python3" => {
            "env -i PATH=\"$PATH\" PYTHONUNBUFFERED=1 python3 child-runtimes/python-function-runner.py"
        }
        "ruby" => "env -i PATH=\"$PATH\" ruby child-runtimes/ruby-function-runner.rb",
        "bash" => {
            "env -i PATH=\"$PATH\" node --permission --allow-net --allow-child-process child-runtimes/bash-function-runner.mjs"
        }
        _ => {
            "env -i PATH=\"$PATH\" NODE_ENV=production node --permission --allow-net child-runtimes/js-function-runner.mjs"
        }
    }
    .to_string()
}

fn managed_lambda_entry_command(value: &str) -> bool {
    ["nodejs", "python3", "ruby", "bash"]
        .iter()
        .map(|runtime| lambda_entry_command_for_runtime(runtime))
        .any(|command| command == value)
}

fn validate_lambda_entry_command(value: Option<&str>, runtime: &str) -> Result<String, String> {
    let entry_command = lambda_entry_command_for_runtime(runtime);
    let Some(command) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(entry_command);
    };
    if !managed_lambda_entry_command(command) {
        return Err("entryCommand must use the managed lambda child runtime".to_string());
    }
    Ok(entry_command)
}

fn cleaned_lambda_input(
    request: &LambdaFunctionSaveRequest,
) -> Result<
    (
        String,
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        i32,
        i32,
        bool,
        String,
        Value,
        Value,
    ),
    String,
> {
    let slug = normalize_lambda_slug(&request.slug);
    if slug.len() < 3 || slug.len() > 120 {
        return Err("slug must normalize to 3-120 characters".to_string());
    }

    let display_name = request.display_name.trim().to_string();
    if display_name.is_empty() {
        return Err("displayName is required".to_string());
    }

    let function_body = request.function_body.trim().to_string();
    if function_body.is_empty() {
        return Err("functionBody is required".to_string());
    }
    if function_body.len() > 262_144 {
        return Err("functionBody exceeds configured byte limit".to_string());
    }

    let description = request
        .description
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let runtime = validate_lambda_runtime(request.runtime.as_deref())?;
    let entry_command = validate_lambda_entry_command(request.entry_command.as_deref(), &runtime)?;
    let reuse_key = validate_lambda_reuse_key(request.reuse_key.as_deref())?;
    let idle_timeout_seconds = request.idle_timeout_seconds.unwrap_or(300).clamp(1, 3600);
    let max_run_ms = request.max_run_ms.unwrap_or(30_000).clamp(1_000, 300_000);
    let containerized = request.containerized.unwrap_or(false);
    if !containerized && !lambda_host_runtime_allowed(&runtime) {
        return Err(format!(
            "{runtime} lambdas require containerized=true; host execution is disabled for this runtime"
        ));
    }
    let status = validate_lambda_status(request.status.as_deref());
    let labels = request.labels.clone().unwrap_or_else(|| json!([]));
    if !labels.is_array() {
        return Err("labels must be a JSON array".to_string());
    }
    let meta_data = request.meta_data.clone().unwrap_or_else(|| json!({}));
    if !meta_data.is_object() {
        return Err("metaData must be a JSON object".to_string());
    }

    Ok((
        slug,
        display_name,
        description,
        runtime,
        entry_command,
        function_body,
        reuse_key,
        idle_timeout_seconds,
        max_run_ms,
        containerized,
        status,
        labels,
        meta_data,
    ))
}

async fn fetch_lambda_functions_from_postgres(
    limit: i64,
    search_pattern: &str,
) -> Result<Vec<LambdaFunctionRow>, String> {
    let client = connect_postgres().await?;
    let rows = client
        .query(
            &format!(
                r#"
                {}
                where is_soft_deleted = false
                  and (
                    $2 = ''
                    or slug ilike $2
                    or display_name ilike $2
                    or description ilike $2
                  )
                order by updated_at desc, created_at desc
                limit $1
                "#,
                lambda_select_sql()
            ),
            &[&limit, &search_pattern],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(rows.iter().map(row_to_lambda_function).collect())
}

async fn fetch_lambda_function_by_slug(slug: &str) -> Result<LambdaFunctionRow, String> {
    let client = connect_postgres().await?;
    let row = client
        .query_one(
            &format!(
                r#"
                {}
                where is_soft_deleted = false
                  and slug = $1
                limit 1
                "#,
                lambda_select_sql()
            ),
            &[&slug],
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(row_to_lambda_function(&row))
}

async fn fetch_lambda_function_by_id(id: &str) -> Result<LambdaFunctionRow, String> {
    let client = connect_postgres().await?;
    let row = client
        .query_one(
            &format!(
                r#"
                {}
                where is_soft_deleted = false
                  and id = $1::text::uuid
                limit 1
                "#,
                lambda_select_sql()
            ),
            &[&id],
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(row_to_lambda_function(&row))
}

async fn fetch_lambda_function_by_identifier(
    identifier: &str,
) -> Result<LambdaFunctionRow, String> {
    let identifier = identifier.trim();
    if looks_like_uuid(identifier) {
        fetch_lambda_function_by_id(identifier).await
    } else {
        fetch_lambda_function_by_slug(identifier).await
    }
}

async fn insert_lambda_function_to_postgres(
    request: &LambdaFunctionSaveRequest,
) -> Result<LambdaFunctionRow, String> {
    let (
        slug,
        display_name,
        description,
        runtime,
        entry_command,
        function_body,
        reuse_key,
        idle_timeout_seconds,
        max_run_ms,
        containerized,
        status,
        labels,
        meta_data,
    ) = cleaned_lambda_input(request)?;
    let client = connect_postgres().await?;
    let row = client
        .query_one(
            r#"
                insert into lambda_functions
                  (slug, display_name, description, runtime, entry_command, function_body, reuse_key,
                   idle_timeout_seconds, max_run_ms, containerized, container_build_status,
                   status, labels, meta_data, is_soft_deleted,
                   created_at, updated_at)
                values
                  ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                   case when $10 then 'pending' else 'not_requested' end,
                   $11, $12, $13, false, now(), now())
                returning slug
                "#,
            &[
                &slug,
                &display_name,
                &description,
                &runtime,
                &entry_command,
                &function_body,
                &reuse_key,
                &idle_timeout_seconds,
                &max_run_ms,
                &containerized,
                &status,
                &labels,
                &meta_data,
            ],
        )
        .await
        .map_err(|error| error.to_string())?;

    let returned_slug = row.try_get::<_, String>("slug").unwrap_or(slug);
    let function = fetch_lambda_function_by_slug(&returned_slug).await?;
    maybe_package_lambda_image(function).await
}

async fn update_lambda_function_in_postgres(
    id: &str,
    request: &LambdaFunctionSaveRequest,
) -> Result<LambdaFunctionRow, String> {
    let (
        slug,
        display_name,
        description,
        runtime,
        entry_command,
        function_body,
        reuse_key,
        idle_timeout_seconds,
        max_run_ms,
        containerized,
        status,
        labels,
        meta_data,
    ) = cleaned_lambda_input(request)?;
    let client = connect_postgres().await?;
    let row = client
        .query_one(
            r#"
                update lambda_functions
                set
                  slug = $2,
                  display_name = $3,
                  description = $4,
                  runtime = $5,
                  entry_command = $6,
                  function_body = $7,
                  reuse_key = $8,
                  idle_timeout_seconds = $9,
                  max_run_ms = $10,
                  containerized = $11,
                  container_image = case when $11 then container_image else null end,
                  container_build_status = case when $11 then 'pending' else 'not_requested' end,
                  container_build_error = null,
                  container_built_at = case when $11 then container_built_at else null end,
                  status = $12,
                  labels = $13,
                  meta_data = $14,
                  updated_at = now()
                where id = $1::text::uuid
                  and is_soft_deleted = false
                returning slug
                "#,
            &[
                &id,
                &slug,
                &display_name,
                &description,
                &runtime,
                &entry_command,
                &function_body,
                &reuse_key,
                &idle_timeout_seconds,
                &max_run_ms,
                &containerized,
                &status,
                &labels,
                &meta_data,
            ],
        )
        .await
        .map_err(|error| error.to_string())?;

    let returned_slug = row.try_get::<_, String>("slug").unwrap_or(slug);
    let function = fetch_lambda_function_by_slug(&returned_slug).await?;
    maybe_package_lambda_image(function).await
}

fn lambda_image_repository() -> String {
    env::var("LAMBDA_IMAGE_REPOSITORY")
        .unwrap_or_else(|_| "docker.io/library/dd-lambda-function".to_string())
}

fn lambda_image_tag(function: &LambdaFunctionRow) -> String {
    let short_id = function.id.chars().take(8).collect::<String>();
    format!(
        "{}:{}-{}",
        lambda_image_repository(),
        function.slug,
        short_id
    )
}

fn lambda_image_build_root() -> PathBuf {
    PathBuf::from(
        env::var("LAMBDA_IMAGE_BUILD_ROOT").unwrap_or_else(|_| "/var/lib/dd-lambdas".to_string()),
    )
}

fn validate_lambda_image_build_root(path: &FsPath) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("lambda image build root must be an absolute path".to_string());
    }
    if path.parent().is_none() {
        return Err("lambda image build root must not be filesystem root".to_string());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err("lambda image build root must not contain . or .. path components".to_string());
    }
    Ok(())
}

fn lambda_image_repo_root() -> PathBuf {
    PathBuf::from(
        env::var("LAMBDA_IMAGE_REPO_ROOT").unwrap_or_else(|_| "/opt/dd-next-1".to_string()),
    )
}

fn lambda_image_build_namespace() -> String {
    env::var("LAMBDA_IMAGE_BUILD_NAMESPACE").unwrap_or_else(|_| "k8s.io".to_string())
}

fn lambda_image_build_nerdctl() -> String {
    env::var("LAMBDA_IMAGE_BUILD_NERDCTL").unwrap_or_else(|_| "/usr/local/bin/nerdctl".to_string())
}

fn lambda_runner_source(runtime: &str) -> (&'static str, &'static str) {
    match runtime {
        "python3" => ("python-function-runner.py", "runner.py"),
        "ruby" => ("ruby-function-runner.rb", "runner.rb"),
        "bash" => ("bash-function-runner.mjs", "runner.mjs"),
        _ => ("js-function-runner.mjs", "runner.mjs"),
    }
}

fn lambda_container_dockerfile(runtime: &str, function: &LambdaFunctionRow) -> String {
    let label = format!(
        "LABEL dd.lambda.id=\"{}\" dd.lambda.slug=\"{}\" dd.lambda.runtime=\"{}\"",
        function.id, function.slug, runtime
    );
    match runtime {
        "python3" => format!(
            r#"FROM docker.io/library/python:3.12-alpine
RUN addgroup -S lambda && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/dd-lambda
COPY runner.py ./runner.py
COPY definition.json ./definition.json
{label}
USER 10001:10001
ENTRYPOINT ["python3", "/opt/dd-lambda/runner.py"]
"#
        ),
        "ruby" => format!(
            r#"FROM docker.io/library/ruby:3.3-alpine
RUN addgroup -S lambda && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/dd-lambda
COPY runner.rb ./runner.rb
COPY definition.json ./definition.json
{label}
USER 10001:10001
ENTRYPOINT ["ruby", "/opt/dd-lambda/runner.rb"]
"#
        ),
        "bash" => format!(
            r#"FROM docker.io/library/alpine:edge
RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/main \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community \
  nodejs-current \
  bash \
  && addgroup -S lambda \
  && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/dd-lambda
COPY runner.mjs ./runner.mjs
COPY definition.json ./definition.json
{label}
USER 10001:10001
ENTRYPOINT ["node", "--permission", "--allow-net", "--allow-child-process", "/opt/dd-lambda/runner.mjs"]
"#
        ),
        _ => format!(
            r#"FROM docker.io/library/alpine:edge
RUN apk add --no-cache \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/main \
  --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community \
  nodejs-current \
  && addgroup -S lambda \
  && adduser -S -G lambda -u 10001 lambda
WORKDIR /opt/dd-lambda
COPY runner.mjs ./runner.mjs
COPY definition.json ./definition.json
{label}
USER 10001:10001
ENTRYPOINT ["node", "--permission", "--allow-net", "/opt/dd-lambda/runner.mjs"]
"#
        ),
    }
}

fn copy_lambda_runner(
    repo_root: &FsPath,
    context_dir: &FsPath,
    runtime: &str,
) -> Result<(), String> {
    let (source_name, target_name) = lambda_runner_source(runtime);
    let source = repo_root
        .join("remote")
        .join("gleam-lambda-runner")
        .join("child-runtimes")
        .join(source_name);
    let target = context_dir.join(target_name);
    fs::copy(&source, &target)
        .map(|_| ())
        .map_err(|error| format!("failed to copy lambda runner {}: {error}", source.display()))
}

fn harden_lambda_build_dir(path: &FsPath) -> Result<(), String> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("failed to restrict lambda image context: {error}"))?;
    }
    Ok(())
}

fn write_lambda_build_file(path: &FsPath, content: impl AsRef<[u8]>) -> Result<(), String> {
    fs::write(path, content).map_err(|error| {
        format!(
            "failed to write lambda image build file {}: {error}",
            path.display()
        )
    })?;
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            format!(
                "failed to restrict lambda image build file {}: {error}",
                path.display()
            )
        })?;
    }
    Ok(())
}

fn package_lambda_image_sync(function: &LambdaFunctionRow, image: &str) -> Result<(), String> {
    let runtime = validate_lambda_runtime(Some(&function.runtime))?;
    let build_root = lambda_image_build_root();
    validate_lambda_image_build_root(&build_root)?;
    fs::create_dir_all(&build_root)
        .map_err(|error| format!("failed to create lambda image build root: {error}"))?;
    let context_dir = build_root.join(format!("lambda-{}", function.id));
    if context_dir.exists() {
        fs::remove_dir_all(&context_dir)
            .map_err(|error| format!("failed to reset lambda image context: {error}"))?;
    }
    fs::create_dir_all(&context_dir)
        .map_err(|error| format!("failed to create lambda image context: {error}"))?;
    harden_lambda_build_dir(&context_dir)?;
    copy_lambda_runner(&lambda_image_repo_root(), &context_dir, &runtime)?;
    write_lambda_build_file(
        &context_dir.join("definition.json"),
        serde_json::to_vec_pretty(function).map_err(|error| error.to_string())?,
    )?;
    write_lambda_build_file(
        &context_dir.join("Dockerfile"),
        lambda_container_dockerfile(&runtime, function),
    )?;

    let namespace = lambda_image_build_namespace();
    let mut command = Command::new(lambda_image_build_nerdctl());
    if !namespace.trim().is_empty() {
        command.arg("-n").arg(namespace);
    }
    command.arg("build").arg("-t").arg(image).arg(&context_dir);
    let output = command
        .output()
        .map_err(|error| format!("failed to run lambda image build: {error}"))?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}\n{stderr}");
        return Err(format!(
            "lambda image build failed: {}",
            combined.chars().take(8192).collect::<String>()
        ));
    }
    Ok(())
}

async fn update_lambda_container_build(
    id: &str,
    image: Option<&str>,
    status: &str,
    error: Option<&str>,
    built: bool,
) -> Result<LambdaFunctionRow, String> {
    let client = connect_postgres().await?;
    client
        .execute(
            r#"
            update lambda_functions
            set
              container_image = $2,
              container_build_status = $3,
              container_build_error = $4,
              container_built_at = case when $5 then now() else container_built_at end,
              updated_at = now()
            where id = $1::text::uuid
              and is_soft_deleted = false
            "#,
            &[&id, &image, &status, &error, &built],
        )
        .await
        .map_err(|error| error.to_string())?;
    fetch_lambda_function_by_id(id).await
}

async fn maybe_package_lambda_image(
    function: LambdaFunctionRow,
) -> Result<LambdaFunctionRow, String> {
    if !function.containerized {
        return Ok(function);
    }

    let image = lambda_image_tag(&function);
    if !env_bool("LAMBDA_IMAGE_BUILD_ENABLED", false) {
        return update_lambda_container_build(
            &function.id,
            Some(&image),
            "skipped",
            Some("LAMBDA_IMAGE_BUILD_ENABLED is not true; image build deferred"),
            false,
        )
        .await
        .or(Ok(function));
    }

    let building =
        update_lambda_container_build(&function.id, Some(&image), "building", None, false)
            .await
            .unwrap_or(function);
    let build_input = building.clone();
    let image_for_build = image.clone();
    let result = tokio::task::spawn_blocking(move || {
        package_lambda_image_sync(&build_input, &image_for_build)
    })
    .await
    .map_err(|error| error.to_string())?;

    match result {
        Ok(()) => update_lambda_container_build(&building.id, Some(&image), "built", None, true)
            .await
            .or(Ok(building)),
        Err(error) => {
            let public_error = error.chars().take(8192).collect::<String>();
            update_lambda_container_build(
                &building.id,
                Some(&image),
                "failed",
                Some(&public_error),
                false,
            )
            .await
            .or(Ok(building))
        }
    }
}

async fn persist_runtime_task_to_postgres(
    request: &DispatchTaskRequest,
    branch: Option<&str>,
) -> Result<(), String> {
    let admin_user_id = agent_tasks_admin_user_id().ok_or_else(|| {
        "AGENT_TASKS_ADMIN_USER_ID or REMOTE_DEV_ADMIN_USER_ID is not configured".to_string()
    })?;
    let repo_config = normalized_repo_config(request)?;
    let known_repo = upsert_known_git_repo_to_postgres(
        &repo_config.repo,
        None,
        None,
        Some(&repo_config.base_branch),
    )
    .await?;
    let client = connect_postgres().await?;
    let title = request
        .thread_title
        .clone()
        .unwrap_or_else(|| request.prompt.chars().take(80).collect::<String>());

    let affected_thread_rows = client
        .execute(
            r#"
            insert into agent_remote_dev_threads
              (id, user_id, known_git_repo_id, title, repo, base_branch, is_soft_deleted, created_at, updated_at, created_by, updated_by)
            values
              ($1::text::uuid, $2::text::uuid, $3::text::uuid, $4, $5, $6, false, now(), now(), $2::text::uuid, $2::text::uuid)
            on conflict (id) do update set
              title = coalesce(agent_remote_dev_threads.title, excluded.title),
              known_git_repo_id = coalesce(agent_remote_dev_threads.known_git_repo_id, excluded.known_git_repo_id),
              updated_by = excluded.updated_by,
              updated_at = now()
            where agent_remote_dev_threads.repo = excluded.repo
              and agent_remote_dev_threads.base_branch = excluded.base_branch
            "#,
            &[
                &request.thread_id,
                &admin_user_id,
                &known_repo.id,
                &title,
                &repo_config.repo,
                &repo_config.base_branch,
            ],
        )
        .await
        .map_err(|error| error.to_string())?;
    if affected_thread_rows == 0 {
        return Err("thread already exists with a different repo or baseBranch".to_string());
    }

    client
        .execute(
            r#"
            insert into agent_remote_dev_tasks
              (id, thread_id, user_id, docker_task_id, prompt, status, branch, last_event_seq, is_soft_deleted, started_at, created_at, updated_at, created_by, updated_by)
            values
              ($1::text::uuid, $2::text::uuid, $3::text::uuid, $1::text::uuid, $4, 'running', $5, -1, false, now(), now(), now(), $3::text::uuid, $3::text::uuid)
            on conflict (id) do update set
              prompt = excluded.prompt,
              status = excluded.status,
              branch = excluded.branch,
              updated_by = excluded.updated_by,
              updated_at = now()
            "#,
            &[
                &request.task_id,
                &request.thread_id,
                &admin_user_id,
                &request.prompt,
                &branch,
            ],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(())
}

fn task_status_from_exit_reason(exit_reason: &str) -> &'static str {
    match exit_reason {
        "completed" => "done",
        "cancelled" => "cancelled",
        _ => "failed",
    }
}

async fn persist_agent_event_to_postgres(
    request: &AgentEventIngestRequest,
    event_kind: &str,
) -> Result<(), String> {
    let client = connect_postgres().await?;
    client
        .execute(
            r#"
            insert into agent_remote_dev_events
              (task_id, seq, event_kind, payload, created_at)
            values
              ($1::text::uuid, $2, $3, $4, now())
            on conflict (task_id, seq) do nothing
            "#,
            &[&request.task_id, &request.seq, &event_kind, &request.event],
        )
        .await
        .map_err(|error| error.to_string())?;

    client
        .execute(
            r#"
            update agent_remote_dev_tasks
            set
              last_event_seq = greatest(last_event_seq, $2),
              updated_at = now()
            where id = $1::text::uuid
              and $2 > last_event_seq
            "#,
            &[&request.task_id, &request.seq],
        )
        .await
        .map_err(|error| error.to_string())?;

    if event_kind == "done" {
        let exit_reason =
            json_string(&request.event, "exitReason").unwrap_or_else(|| "failed".to_string());
        let status = task_status_from_exit_reason(&exit_reason);
        let branch = json_string(&request.event, "branch");
        let pr_url = json_string(&request.event, "prUrl");
        let error_message = json_string(&request.event, "errorMessage");
        client
            .execute(
                r#"
                update agent_remote_dev_tasks
                set
                  status = $2,
                  branch = coalesce($3, branch),
                  pr_url = coalesce($4, pr_url),
                  exit_reason = $5,
                  error_message = $6,
                  finished_at = now(),
                  updated_at = now()
                where id = $1::text::uuid
                "#,
                &[
                    &request.task_id,
                    &status,
                    &branch,
                    &pr_url,
                    &exit_reason,
                    &error_message,
                ],
            )
            .await
            .map_err(|error| error.to_string())?;
    }

    if event_kind == "pr_open" {
        let branch = json_string(&request.event, "branch");
        let pr_url = json_string(&request.event, "prUrl");
        client
            .execute(
                r#"
                update agent_remote_dev_tasks
                set
                  status = 'pr_open',
                  branch = coalesce($2, branch),
                  pr_url = coalesce($3, pr_url),
                  pr_state = 'draft',
                  updated_at = now()
                where id = $1::text::uuid
                "#,
                &[&request.task_id, &branch, &pr_url],
            )
            .await
            .map_err(|error| error.to_string())?;
    }

    Ok(())
}

async fn fetch_agent_events_from_postgres(
    task_id: &str,
    limit: i64,
) -> Result<Vec<AgentEventRow>, String> {
    let client = connect_postgres().await?;
    let event_rows = client
        .query(
            r#"
            select
              ev.task_id::text as task_id,
              ev.seq as seq,
              ev.event_kind as event_kind,
              ev.payload as payload,
              to_char(ev.created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at
            from (
              select task_id, seq, event_kind, payload, created_at
              from agent_remote_dev_events
              where task_id = $1::text::uuid
              order by seq desc
              limit $2
            ) ev
            order by ev.seq asc
            "#,
            &[&task_id, &limit],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(event_rows
        .iter()
        .map(|row| AgentEventRow {
            task_id: row_string(row, "task_id"),
            seq: row_i32(row, "seq"),
            event_kind: row_string(row, "event_kind"),
            payload: row.get("payload"),
            created_at: row_opt_string(row, "created_at"),
        })
        .collect())
}

async fn persist_feedback_event_to_postgres(
    task_id: &str,
    request: &AgentFeedbackRequest,
) -> Result<AgentEventRow, String> {
    let client = connect_postgres().await?;
    let vote = request.vote.trim().to_lowercase();
    let seq_row = client
        .query_one(
            r#"
            select coalesce(max(seq), -1) + 1 as next_seq
            from agent_remote_dev_events
            where task_id = $1::text::uuid
            "#,
            &[&task_id],
        )
        .await
        .map_err(|error| error.to_string())?;
    let seq: i32 = seq_row.get("next_seq");
    let payload = json!({
        "kind": "feedback",
        "vote": vote,
        "targetSeq": request.target_seq,
        "note": request.note,
        "source": "agents-threads-ui",
        "createdAtMs": now_ms(),
    });

    let event_row = client
        .query_one(
            r#"
            insert into agent_remote_dev_events
              (task_id, seq, event_kind, payload, created_at)
            values
              ($1::text::uuid, $2, 'feedback', $3, now())
            returning
              task_id::text as task_id,
              seq,
              event_kind,
              payload,
              to_char(created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at
            "#,
            &[&task_id, &seq, &payload],
        )
        .await
        .map_err(|error| error.to_string())?;

    client
        .execute(
            r#"
            update agent_remote_dev_tasks
            set
              last_event_seq = greatest(last_event_seq, $2),
              updated_at = now()
            where id = $1::text::uuid
            "#,
            &[&task_id, &seq],
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(AgentEventRow {
        task_id: row_string(&event_row, "task_id"),
        seq: row_i32(&event_row, "seq"),
        event_kind: row_string(&event_row, "event_kind"),
        payload: event_row.get("payload"),
        created_at: row_opt_string(&event_row, "created_at"),
    })
}

async fn publish_task_shadow_to_nats(
    request: &DispatchTaskRequest,
    branch: Option<&str>,
) -> Result<(), String> {
    let repo_config = normalized_repo_config(request)?;
    let message = NatsTaskMessage {
        version: 1,
        message_kind: "task.shadow",
        task_kind: "agent.prompt",
        shadow: true,
        direct_dispatch: true,
        thread_id: request.thread_id.clone(),
        task_id: request.task_id.clone(),
        provider: request.provider.clone(),
        repo: repo_config.repo,
        base_branch: repo_config.base_branch,
        feature_branch: branch.map(str::to_string),
        prompt: request.prompt.clone(),
        created_at_ms: now_ms(),
    };
    let payload = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
    let client = async_nats::connect(nats_url())
        .await
        .map_err(|error| error.to_string())?;
    let task_subject = nats_task_subject(&request.thread_id);

    jetstream_publish_task(client.clone(), task_subject, payload.clone()).await?;
    client
        .publish(nats_wakeup_subject(), payload.into())
        .await
        .map_err(|error| error.to_string())?;
    client.flush().await.map_err(|error| error.to_string())?;
    Ok(())
}

async fn publish_lambda_function_update_to_nats(
    action: &str,
    function: &LambdaFunctionRow,
) -> Result<(), String> {
    let message = NatsLambdaFunctionMessage {
        version: 1,
        message_kind: "lambda.function.updated",
        action: action.to_string(),
        function_id: function.id.clone(),
        slug: function.slug.clone(),
        status: function.status.clone(),
        updated_at_ms: now_ms(),
    };
    let payload = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
    let client = async_nats::connect(nats_url())
        .await
        .map_err(|error| error.to_string())?;
    client
        .publish(nats_lambda_functions_subject(), payload.into())
        .await
        .map_err(|error| error.to_string())?;
    client.flush().await.map_err(|error| error.to_string())?;
    Ok(())
}

async fn fetch_thread_context_from_postgres(
    thread_id: &str,
    limit: i64,
) -> Result<Vec<AgentTaskRow>, String> {
    let client = connect_postgres().await?;
    let task_rows = client
        .query(
            r#"
            select
              t.id::text as id,
              t.thread_id::text as thread_id,
              th.title as thread_title,
              t.prompt as prompt,
              t.status as status,
              t.branch as branch,
              t.pr_url as pr_url,
              t.pr_state as pr_state,
              t.exit_reason as exit_reason,
              t.error_message as error_message,
              to_char(t.started_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as started_at,
              to_char(t.finished_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as finished_at,
              to_char(t.created_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as created_at,
              to_char(t.updated_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') as updated_at,
              t.last_event_seq as last_event_seq,
              coalesce(e.event_count, 0)::bigint as event_count,
              le.event_kind as latest_event_kind,
              left(le.payload::text, 1200) as latest_payload
            from agent_remote_dev_tasks t
            left join agent_remote_dev_threads th on th.id = t.thread_id
            left join lateral (
              select count(*)::bigint as event_count
              from agent_remote_dev_events ev
              where ev.task_id = t.id
            ) e on true
            left join lateral (
              select ev.event_kind, ev.payload
              from agent_remote_dev_events ev
              where ev.task_id = t.id
              order by ev.seq desc
              limit 1
            ) le on true
            where t.thread_id = $1::text::uuid
              and t.is_soft_deleted = false
            order by t.created_at desc
            limit $2
            "#,
            &[&thread_id, &limit],
        )
        .await
        .map_err(|error| error.to_string())?;

    let mut tasks = task_rows
        .iter()
        .map(|row| AgentTaskRow {
            id: row_string(row, "id"),
            thread_id: row_string(row, "thread_id"),
            thread_title: row_opt_string(row, "thread_title"),
            prompt: row_string(row, "prompt"),
            status: row_string(row, "status"),
            branch: row_opt_string(row, "branch"),
            pr_url: row_opt_string(row, "pr_url"),
            pr_state: row_opt_string(row, "pr_state"),
            exit_reason: row_opt_string(row, "exit_reason"),
            error_message: row_opt_string(row, "error_message"),
            started_at: row_opt_string(row, "started_at"),
            finished_at: row_opt_string(row, "finished_at"),
            created_at: row_opt_string(row, "created_at"),
            updated_at: row_opt_string(row, "updated_at"),
            last_event_seq: row_i32(row, "last_event_seq"),
            event_count: row_i64(row, "event_count"),
            latest_event_kind: row_opt_string(row, "latest_event_kind"),
            latest_payload: row_opt_string(row, "latest_payload"),
        })
        .collect::<Vec<_>>();
    tasks.reverse();
    Ok(tasks)
}

async fn fetch_agents_from_supabase(
    limit: i64,
) -> Result<(Vec<AgentThreadRow>, Vec<AgentTaskRow>), String> {
    let supabase_url = first_env(&["SUPABASE_URL", "NEXT_PUBLIC_SUPABASE_URL"])
        .ok_or_else(|| "SUPABASE_URL not configured".to_string())?;
    let supabase_key = first_env(&["SUPABASE_SERVICE_ROLE_KEY", "SUPABASE_KEY"])
        .ok_or_else(|| "SUPABASE_SERVICE_ROLE_KEY not configured".to_string())?;
    let base = supabase_url.trim_end_matches('/');
    let http = reqwest::Client::new();

    let threads_url = format!(
        "{base}/rest/v1/agent_remote_dev_threads?select=id,title,repo,base_branch,archived_at,created_at,updated_at&is_soft_deleted=eq.false&order=updated_at.desc&limit={limit}"
    );
    let tasks_url = format!(
        "{base}/rest/v1/agent_remote_dev_tasks?select=id,thread_id,prompt,status,branch,pr_url,pr_state,exit_reason,error_message,started_at,finished_at,created_at,updated_at,last_event_seq&is_soft_deleted=eq.false&order=created_at.desc&limit={limit}"
    );

    let thread_values = supabase_get(&http, &threads_url, &supabase_key).await?;
    let mut thread_titles = HashMap::new();
    let threads: Vec<AgentThreadRow> = thread_values
        .iter()
        .map(|value| {
            let id = json_string(value, "id").unwrap_or_default();
            let title = json_string(value, "title").unwrap_or_else(|| "Remote thread".to_string());
            thread_titles.insert(id.clone(), title.clone());
            AgentThreadRow {
                id,
                title,
                repo: json_string(value, "repo").unwrap_or_default(),
                base_branch: json_string(value, "base_branch").unwrap_or_default(),
                archived_at: json_string(value, "archived_at"),
                created_at: json_string(value, "created_at"),
                updated_at: json_string(value, "updated_at"),
                task_count: 0,
                active_task_count: 0,
                latest_task_at: None,
            }
        })
        .collect();

    let task_values = supabase_get(&http, &tasks_url, &supabase_key).await?;
    let tasks: Vec<AgentTaskRow> = task_values
        .iter()
        .map(|value| {
            let thread_id = json_string(value, "thread_id").unwrap_or_default();
            AgentTaskRow {
                id: json_string(value, "id").unwrap_or_default(),
                thread_id: thread_id.clone(),
                thread_title: thread_titles.get(&thread_id).cloned(),
                prompt: json_string(value, "prompt").unwrap_or_default(),
                status: json_string(value, "status").unwrap_or_else(|| "unknown".to_string()),
                branch: json_string(value, "branch"),
                pr_url: json_string(value, "pr_url"),
                pr_state: json_string(value, "pr_state"),
                exit_reason: json_string(value, "exit_reason"),
                error_message: json_string(value, "error_message"),
                started_at: json_string(value, "started_at"),
                finished_at: json_string(value, "finished_at"),
                created_at: json_string(value, "created_at"),
                updated_at: json_string(value, "updated_at"),
                last_event_seq: json_i32(value, "last_event_seq"),
                event_count: json_i64(value, "event_count"),
                latest_event_kind: None,
                latest_payload: None,
            }
        })
        .collect();

    Ok((threads, tasks))
}

async fn supabase_get(http: &reqwest::Client, url: &str, key: &str) -> Result<Vec<Value>, String> {
    let response = http
        .get(url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"))
        .header("apikey", key)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        eprintln!(
            "agent tasks supabase http error: status={} body={}",
            status.as_u16(),
            body.chars().take(300).collect::<String>()
        );
        return Err(format!("supabase http {}", status.as_u16()));
    }
    serde_json::from_str::<Vec<Value>>(&body).map_err(|error| error.to_string())
}

async fn healthz() -> impl IntoResponse {
    record_request("GET", "/healthz", StatusCode::OK);
    Json(HealthResponse {
        ok: true,
        service: "dd-remote-rest-api".to_string(),
        mode: "database-boundary".to_string(),
    })
}

async fn agents_tasks(Query(query): Query<AgentsQuery>) -> impl IntoResponse {
    record_request("GET", "/api/agents/tasks", StatusCode::OK);
    Json(fetch_agents_snapshot(limit_from_query(&query)).await)
}

async fn known_git_repos(Query(query): Query<AgentsQuery>) -> impl IntoResponse {
    record_request("GET", "/api/agents/git-repos", StatusCode::OK);
    if postgres_database_url().is_none() {
        return Json(KnownGitReposResponse {
            ok: false,
            source: "postgres".to_string(),
            generated_at_ms: now_ms(),
            repos: Vec::new(),
            errors: vec!["postgres database URL is not configured".to_string()],
        });
    }

    match fetch_known_git_repos_from_postgres(limit_from_query(&query)).await {
        Ok(repos) => Json(KnownGitReposResponse {
            ok: true,
            source: "postgres".to_string(),
            generated_at_ms: now_ms(),
            repos,
            errors: Vec::new(),
        }),
        Err(error) => Json(KnownGitReposResponse {
            ok: false,
            source: "postgres".to_string(),
            generated_at_ms: now_ms(),
            repos: Vec::new(),
            errors: vec![public_data_source_error("postgres"), error],
        }),
    }
}

async fn save_known_git_repo(Json(request): Json<KnownGitRepoRequest>) -> Response {
    record_request("POST", "/api/agents/git-repos", StatusCode::OK);
    if postgres_database_url().is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(KnownGitReposResponse {
                ok: false,
                source: "postgres".to_string(),
                generated_at_ms: now_ms(),
                repos: Vec::new(),
                errors: vec!["postgres database URL is not configured".to_string()],
            }),
        )
            .into_response();
    }

    match upsert_known_git_repo_to_postgres(
        &request.repo_url,
        request.display_name.as_deref(),
        request.provider.as_deref(),
        request.default_branch.as_deref(),
    )
    .await
    {
        Ok(repo) => Json(KnownGitReposResponse {
            ok: true,
            source: "postgres".to_string(),
            generated_at_ms: now_ms(),
            repos: vec![repo],
            errors: Vec::new(),
        })
        .into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(KnownGitReposResponse {
                ok: false,
                source: "postgres".to_string(),
                generated_at_ms: now_ms(),
                repos: Vec::new(),
                errors: vec![error],
            }),
        )
            .into_response(),
    }
}

async fn lambda_functions(Query(query): Query<LambdasQuery>) -> impl IntoResponse {
    record_request("GET", "/api/lambdas/functions", StatusCode::OK);
    if postgres_database_url().is_none() {
        return Json(LambdaFunctionsResponse {
            ok: false,
            source: "postgres".to_string(),
            generated_at_ms: now_ms(),
            functions: Vec::new(),
            errors: vec!["postgres database URL is not configured".to_string()],
        });
    }

    match fetch_lambda_functions_from_postgres(
        lambda_limit_from_query(&query),
        &lambda_search_pattern(&query),
    )
    .await
    {
        Ok(functions) => Json(LambdaFunctionsResponse {
            ok: true,
            source: "postgres".to_string(),
            generated_at_ms: now_ms(),
            functions,
            errors: Vec::new(),
        }),
        Err(error) => {
            eprintln!("lambda functions postgres data source error: {error}");
            Json(LambdaFunctionsResponse {
                ok: false,
                source: "postgres".to_string(),
                generated_at_ms: now_ms(),
                functions: Vec::new(),
                errors: vec![public_data_source_error("postgres lambda functions")],
            })
        }
    }
}

async fn lambda_function(Path(identifier): Path<String>) -> Response {
    record_request("GET", "/api/lambdas/functions/:identifier", StatusCode::OK);
    if postgres_database_url().is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "postgres database URL is not configured" })),
        )
            .into_response();
    }

    match fetch_lambda_function_by_identifier(&identifier).await {
        Ok(function) => {
            Json(json!({ "ok": true, "source": "postgres", "function": function })).into_response()
        }
        Err(error) => {
            eprintln!("lambda function fetch failed: {error}");
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "lambda function not found" })),
            )
                .into_response()
        }
    }
}

async fn create_lambda_function(Json(request): Json<LambdaFunctionSaveRequest>) -> Response {
    record_request("POST", "/api/lambdas/functions", StatusCode::OK);
    if postgres_database_url().is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "postgres database URL is not configured" })),
        )
            .into_response();
    }

    match insert_lambda_function_to_postgres(&request).await {
        Ok(function) => {
            if let Err(error) = publish_lambda_function_update_to_nats("created", &function).await {
                eprintln!("lambda function nats publish failed: {error}");
            }
            Json(json!({ "ok": true, "source": "postgres", "function": function })).into_response()
        }
        Err(error) => {
            eprintln!("lambda function create failed: {error}");
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "failed to create lambda function" })),
            )
                .into_response()
        }
    }
}

async fn update_lambda_function(
    Path(id): Path<String>,
    Json(request): Json<LambdaFunctionSaveRequest>,
) -> Response {
    record_request("PATCH", "/api/lambdas/functions/:id", StatusCode::OK);
    if postgres_database_url().is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "postgres database URL is not configured" })),
        )
            .into_response();
    }

    match update_lambda_function_in_postgres(&id, &request).await {
        Ok(function) => {
            if let Err(error) = publish_lambda_function_update_to_nats("updated", &function).await {
                eprintln!("lambda function nats publish failed: {error}");
            }
            Json(json!({ "ok": true, "source": "postgres", "function": function })).into_response()
        }
        Err(error) => {
            eprintln!("lambda function update failed: {error}");
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "failed to update lambda function" })),
            )
                .into_response()
        }
    }
}

async fn agent_task_events(
    Path(task_id): Path<String>,
    Query(query): Query<ContextQuery>,
) -> impl IntoResponse {
    record_request("GET", "/api/agents/tasks/:taskId/events", StatusCode::OK);
    let limit = event_limit_from_query(&query);
    if postgres_database_url().is_some() {
        match fetch_agent_events_from_postgres(&task_id, limit).await {
            Ok(events) => {
                return Json(AgentTaskEventsResponse {
                    ok: true,
                    source: "postgres".to_string(),
                    task_id,
                    generated_at_ms: now_ms(),
                    events,
                    errors: Vec::new(),
                });
            }
            Err(error) => {
                return Json(AgentTaskEventsResponse {
                    ok: false,
                    source: "runtime-memory".to_string(),
                    task_id,
                    generated_at_ms: now_ms(),
                    events: Vec::new(),
                    errors: vec![public_data_source_error("postgres events"), error],
                });
            }
        }
    }

    Json(AgentTaskEventsResponse {
        ok: false,
        source: "runtime-memory".to_string(),
        task_id,
        generated_at_ms: now_ms(),
        events: Vec::new(),
        errors: vec![
            "postgres database URL is not configured; task events are unavailable".to_string(),
        ],
    })
}

async fn agent_task_feedback(
    Path(task_id): Path<String>,
    Json(request): Json<AgentFeedbackRequest>,
) -> Response {
    record_request("POST", "/api/agents/tasks/:taskId/feedback", StatusCode::OK);
    let vote = request.vote.trim().to_lowercase();
    if vote != "up" && vote != "down" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "feedback vote must be up or down" })),
        )
            .into_response();
    }
    if postgres_database_url().is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "postgres database URL is not configured; feedback is unavailable"
            })),
        )
            .into_response();
    }

    match persist_feedback_event_to_postgres(&task_id, &request).await {
        Ok(event) => Json(json!({
            "ok": true,
            "source": "postgres",
            "taskId": task_id,
            "event": event
        }))
        .into_response(),
        Err(error) => {
            eprintln!("agent feedback persist failed: {error}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": public_data_source_error("postgres feedback") })),
            )
                .into_response()
        }
    }
}

async fn thread_context(
    Path(thread_id): Path<String>,
    Query(query): Query<ContextQuery>,
) -> impl IntoResponse {
    record_request(
        "GET",
        "/api/agents/threads/:threadId/context",
        StatusCode::OK,
    );
    let limit = context_limit_from_query(&query);
    if postgres_database_url().is_some() {
        match fetch_thread_context_from_postgres(&thread_id, limit).await {
            Ok(tasks) => {
                return Json(ThreadContextResponse {
                    ok: true,
                    source: "postgres".to_string(),
                    thread_id,
                    generated_at_ms: now_ms(),
                    tasks,
                    errors: Vec::new(),
                });
            }
            Err(error) => {
                return Json(runtime_thread_context(
                    &thread_id,
                    limit,
                    vec![public_data_source_error("postgres"), error],
                ));
            }
        }
    }

    Json(runtime_thread_context(
        &thread_id,
        limit,
        vec!["postgres database URL is not configured; showing runtime memory only".to_string()],
    ))
}

async fn dispatch_thread_task(
    Path(thread_id): Path<String>,
    Json(request): Json<DispatchTaskRequest>,
) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/tasks",
        StatusCode::OK,
    );
    if request.thread_id != thread_id {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "threadId path/body mismatch" })),
        )
            .into_response();
    }
    let mut repo_config = match normalized_repo_config(&request) {
        Ok(repo_config) => repo_config,
        Err(error) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
        }
    };
    if postgres_database_url().is_some() {
        match fetch_thread_repo_config_from_postgres(&thread_id).await {
            Ok(Some(stored_config)) => {
                if stored_config.repo != repo_config.repo
                    || stored_config.base_branch != repo_config.base_branch
                {
                    return (
                        StatusCode::CONFLICT,
                        Json(json!({
                            "error": "thread already exists with a different repo or baseBranch"
                        })),
                    )
                        .into_response();
                }
                repo_config = stored_config;
            }
            Ok(None) => {}
            Err(error) => eprintln!("failed to fetch thread repo config before dispatch: {error}"),
        }
    }

    remember_runtime_task(&request, None);
    if let Err(error) = persist_runtime_task_to_postgres(&request, None).await {
        eprintln!("failed to persist remote task before worker wake: {error}");
    }

    let Ok((namespace, name, _results)) =
        ensure_thread_worker(&thread_id, &repo_config.repo, &repo_config.base_branch).await
    else {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "failed to create or wake thread worker" })),
        )
            .into_response();
    };
    let Some(secret) = worker_auth_secret() else {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": missing_worker_auth_secret_message() })),
        )
            .into_response();
    };
    if let Err(error) = wait_thread_worker_ready(&namespace, &name, &secret).await {
        return (StatusCode::BAD_GATEWAY, Json(json!({ "error": error }))).into_response();
    }

    let worker_body = json!({
        "taskId": &request.task_id,
        "threadId": &request.thread_id,
        "prompt": &request.prompt,
        "provider": &request.provider,
        "threadTitle": &request.thread_title,
        "repo": &repo_config.repo,
        "baseBranch": &repo_config.base_branch,
    });
    let client = reqwest::Client::new();
    let response = client
        .post(thread_worker_url(&namespace, &name, "/tasks"))
        .header("X-Server-Auth", secret)
        .json(&worker_body)
        .send()
        .await;
    match response {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if status.is_success() {
                let branch = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|value| json_string(&value, "branch"));
                remember_runtime_task(&request, branch.clone());
                if let Err(error) =
                    persist_runtime_task_to_postgres(&request, branch.as_deref()).await
                {
                    eprintln!("failed to persist remote task to postgres: {error}");
                }
                match tokio::time::timeout(
                    Duration::from_secs(2),
                    publish_task_shadow_to_nats(&request, branch.as_deref()),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => eprintln!("failed to publish remote task to nats: {error}"),
                    Err(_) => eprintln!("timed out publishing remote task to nats"),
                }
            }
            let public_status =
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            (
                public_status,
                [(header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response()
        }
        Err(error) => {
            eprintln!("thread worker dispatch proxy failed: {error}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": public_thread_worker_proxy_error("dispatch") })),
            )
                .into_response()
        }
    }
}

async fn ingest_agent_event(
    headers: HeaderMap,
    Json(request): Json<AgentEventIngestRequest>,
) -> Response {
    record_request("POST", "/api/agents/events", StatusCode::OK);
    if !authorized_internal_request(&headers) {
        return unauthorized_response();
    }
    let Some(event_kind) = json_string(&request.event, "kind") else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "event.kind is required" })),
        )
            .into_response();
    };
    match persist_agent_event_to_postgres(&request, &event_kind).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(error) => {
            eprintln!("agent event ingest failed: {error}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": public_data_source_error("postgres event ingest") })),
            )
                .into_response()
        }
    }
}

async fn prepare_thread(headers: HeaderMap, Path(thread_id): Path<String>) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/prepare",
        StatusCode::OK,
    );
    if !authorized_internal_request(&headers) {
        return unauthorized_response();
    }

    match prepare_thread_worker(&thread_id).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            eprintln!("thread worker prepare failed: {error}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": public_thread_worker_proxy_error("prepare") })),
            )
                .into_response()
        }
    }
}

async fn thread_runtime(Path(thread_id): Path<String>) -> Response {
    record_request(
        "GET",
        "/api/agents/threads/:threadId/runtime",
        StatusCode::OK,
    );
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(&thread_id);
    let mut errors = Vec::new();

    let deployment = match k8s_get_value(format!(
        "/apis/apps/v1/namespaces/{namespace}/deployments/{name}"
    ))
    .await
    {
        Ok(Some(value)) => Some(summarize_deployment(&value)),
        Ok(None) => None,
        Err(error) => {
            errors.push(error);
            None
        }
    };
    let service =
        match k8s_get_value(format!("/api/v1/namespaces/{namespace}/services/{name}")).await {
            Ok(Some(value)) => Some(summarize_service(&value)),
            Ok(None) => None,
            Err(error) => {
                errors.push(error);
                None
            }
        };
    let pods = match k8s_get_value(format!(
        "/api/v1/namespaces/{namespace}/pods?labelSelector=dd%2FthreadId%3D{thread_id}"
    ))
    .await
    {
        Ok(Some(value)) => json_at(&value, &["items"])
            .and_then(Value::as_array)
            .map(|items| items.iter().map(summarize_pod).collect::<Vec<_>>())
            .unwrap_or_default(),
        Ok(None) => Vec::new(),
        Err(error) => {
            errors.push(error);
            Vec::new()
        }
    };
    let summary = summarize_thread_runtime(deployment.as_ref(), &pods);
    Json(ThreadRuntimeResponse {
        ok: errors.is_empty(),
        source: "kubernetes".to_string(),
        thread_id,
        namespace,
        k8s_name: name,
        generated_at_ms: now_ms(),
        summary,
        deployment,
        service,
        pods,
        errors,
    })
    .into_response()
}

async fn stream_thread_task(Path((thread_id, task_id)): Path<(String, String)>) -> Response {
    record_request(
        "GET",
        "/api/agents/threads/:threadId/stream/:taskId",
        StatusCode::OK,
    );
    let namespace = thread_runtime_namespace();
    let name = thread_resource_name(&thread_id);
    let Some(secret) = worker_auth_secret() else {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": missing_worker_auth_secret_message() })),
        )
            .into_response();
    };
    let response = reqwest::Client::new()
        .get(thread_worker_url(
            &namespace,
            &name,
            &format!("/stream/{task_id}"),
        ))
        .header("X-Server-Auth", secret)
        .send()
        .await;
    match response {
        Ok(response) => {
            let status = StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::OK);
            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Body::from_stream(response.bytes_stream()))
                .unwrap_or_else(|error| {
                    (
                        StatusCode::BAD_GATEWAY,
                        format!("stream response build failed: {error}"),
                    )
                        .into_response()
                })
        }
        Err(error) => {
            eprintln!("thread worker stream proxy failed: {error}");
            (
                StatusCode::BAD_GATEWAY,
                public_thread_worker_proxy_error("stream"),
            )
                .into_response()
        }
    }
}

async fn sleep_thread(
    Path(thread_id): Path<String>,
    Json(request): Json<ThreadControlRequest>,
) -> Response {
    if let Err(error) = validate_thread_control_signal(&thread_id, "sleep", &request) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    scale_thread_runtime(thread_id, "sleep", 0, request.task_id.clone()).await
}

async fn archive_thread(
    Path(thread_id): Path<String>,
    Json(request): Json<ThreadControlRequest>,
) -> Response {
    if let Err(error) = validate_thread_control_signal(&thread_id, "archive", &request) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    scale_thread_runtime(thread_id, "archive", 0, request.task_id.clone()).await
}

async fn hard_delete_thread(
    Path(thread_id): Path<String>,
    Json(request): Json<ThreadControlRequest>,
) -> Response {
    if let Err(error) = validate_thread_control_signal(&thread_id, "hard-delete", &request) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    delete_thread_runtime(thread_id, request.task_id.clone()).await
}

async fn merge_upstream_thread(
    Path(thread_id): Path<String>,
    Json(request): Json<ThreadControlRequest>,
) -> Response {
    if let Err(error) = validate_thread_control_signal(&thread_id, "merge-upstream", &request) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    merge_thread_upstream(thread_id, request).await
}

async fn open_pr_thread(
    Path(thread_id): Path<String>,
    Json(request): Json<ThreadControlRequest>,
) -> Response {
    if let Err(error) = validate_thread_control_signal(&thread_id, "open-pr", &request) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    open_thread_pr(thread_id, request).await
}

async fn make_commit_thread(
    Path(thread_id): Path<String>,
    Json(request): Json<ThreadControlRequest>,
) -> Response {
    if let Err(error) = validate_thread_control_signal(&thread_id, "make-commit", &request) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    make_thread_commit(thread_id, request).await
}

async fn terminal_thread(
    Path(thread_id): Path<String>,
    Json(request): Json<ThreadControlRequest>,
) -> Response {
    if let Err(error) = validate_thread_control_signal(&thread_id, "terminal", &request) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    open_thread_terminal(thread_id, request).await
}

async fn metrics() -> impl IntoResponse {
    record_request("GET", "/metrics", StatusCode::OK);
    UPTIME_SECONDS.set(STARTED_AT.elapsed().as_secs() as i64);

    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder
        .encode(&metric_families, &mut buffer)
        .expect("failed to encode prometheus metrics");

    (
        [(header::CONTENT_TYPE, encoder.format_type().to_string())],
        buffer,
    )
}

#[tokio::main]
async fn main() {
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8082);

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/agents/tasks", get(agents_tasks))
        .route(
            "/api/agents/git-repos",
            get(known_git_repos).post(save_known_git_repo),
        )
        .route(
            "/api/lambdas/functions",
            get(lambda_functions).post(create_lambda_function),
        )
        .route(
            "/api/lambdas/functions/:id",
            get(lambda_function).patch(update_lambda_function),
        )
        .route("/api/agents/tasks/:task_id/events", get(agent_task_events))
        .route(
            "/api/agents/tasks/:task_id/feedback",
            post(agent_task_feedback),
        )
        .route("/api/agents/events", post(ingest_agent_event))
        .route(
            "/api/agents/threads/:thread_id/context",
            get(thread_context),
        )
        .route(
            "/api/agents/threads/:thread_id/runtime",
            get(thread_runtime),
        )
        .route(
            "/api/agents/threads/:thread_id/prepare",
            post(prepare_thread),
        )
        .route(
            "/api/agents/threads/:thread_id/tasks",
            post(dispatch_thread_task),
        )
        .route(
            "/api/agents/threads/:thread_id/stream/:task_id",
            get(stream_thread_task),
        )
        .route("/api/agents/threads/:thread_id/sleep", post(sleep_thread))
        .route(
            "/api/agents/threads/:thread_id/archive",
            post(archive_thread),
        )
        .route(
            "/api/agents/threads/:thread_id/hard-delete",
            post(hard_delete_thread),
        )
        .route(
            "/api/agents/threads/:thread_id/merge-upstream",
            post(merge_upstream_thread),
        )
        .route(
            "/api/agents/threads/:thread_id/open-pr",
            post(open_pr_thread),
        )
        .route(
            "/api/agents/threads/:thread_id/make-commit",
            post(make_commit_thread),
        )
        .route(
            "/api/agents/threads/:thread_id/terminal",
            post(terminal_thread),
        )
        .route("/metrics", get(metrics));

    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("failed to parse bind address");
    println!("dd-remote-rest-api listening on http://{address}");

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
