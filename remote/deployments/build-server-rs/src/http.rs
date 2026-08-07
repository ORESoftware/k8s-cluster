use std::{collections::HashSet, path::PathBuf, sync::atomic::Ordering};

use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::fs;
use tokio_util::io::ReaderStream;

use crate::exec::build_dependencies_ready;
use crate::jobs::enqueue_build;
use crate::state::{AppState, SERVICE_NAME};
use crate::types::{BuildRequest, BuildStatus, HealthResponse};
use crate::{db, gh_secrets, profiles, webhooks};

pub(crate) fn request_is_authorized(headers: &HeaderMap, secret: &str) -> bool {
    headers
        .get("x-server-auth")
        .or_else(|| headers.get("x-build-server-auth"))
        .or_else(|| headers.get("x-agent-auth"))
        .and_then(|value| value.to_str().ok())
        // Constant-time comparison of digests: no timing side channel and no
        // length leak from the shared secret.
        .is_some_and(|value| {
            let presented = Sha256::digest(value.as_bytes());
            let expected = Sha256::digest(secret.as_bytes());
            presented.as_slice().ct_eq(expected.as_slice()).into()
        })
}

pub(crate) fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "SERVER_AUTH_SECRET is not configured" })),
        )
            .into_response());
    };
    if !request_is_authorized(headers, secret) {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthorized",
                "errMessage": "missing required build server auth header",
            })),
        )
            .into_response());
    }
    Ok(())
}

pub(crate) async fn descriptor(State(state): State<AppState>) -> impl IntoResponse {
    let config = &state.config;
    Json(json!({
        "service": SERVICE_NAME,
        "description": "Authenticated Rust build server for repo image builds and controlled Kubernetes deploys, with fiducia.cloud build locks, Postgres persistence, NATS events, webhooks, and GitHub secret sync.",
        "endpoints": {
            "submit": "POST /builds",
            "list": "GET /builds",
            "status": "GET /builds/<jobId>",
            "logs": "GET /builds/<jobId>/logs",
            "artifacts": "GET /builds/<jobId>/artifacts",
            "githubWebhook": "POST /webhooks/github",
            "registryWebhook": "POST /webhooks/registry",
            "syncSecrets": "POST /secrets/sync",
            "syncSecretsStatus": "GET /secrets/sync/status",
            "healthz": "GET /healthz",
            "metrics": "GET /metrics"
        },
        "jobSchema": {
            "schemaVersion": "build-server.v1",
            "jobKind": ["build-image", "build-and-deploy", "run-profile"],
            "required": ["repoUrl"],
            "conditional": {
                "build-image/build-and-deploy": ["image"],
                "run-profile": ["profile"]
            },
            "optional": ["gitRef", "contextDir", "dockerfile", "buildArgs", "push", "deploy", "executor", "requestId"]
        },
        "profiles": profiles::SPECS,
        "delegatedCapabilities": [
            { "platform": "macos", "profiles": ["flutter-ios-release", "flutter-macos-release"], "runner": "GitHub-hosted macOS or a dedicated macOS worker" },
            { "platform": "windows", "profiles": ["flutter-windows-release"], "runner": "GitHub-hosted Windows or a dedicated Windows worker" }
        ],
        "executors": ["local", "lambda"],
        "pushRegistries": ["amazon-ecr"],
        "deployKinds": ["kustomize", "manifest", "none"],
        "coordination": {
            "provider": "fiducia.cloud",
            "enabled": config.coordination_enabled,
            "required": config.coordination_required
        },
        "persistence": { "postgres": config.database_url.is_some(), "database": "dd_build_server" },
        "messaging": {
            "nats": config.nats_enabled,
            "intake": config.nats_intake_enabled,
            "eventSubject": config.nats_event_subject,
            "requestSubject": config.nats_request_subject
        },
        "webhooks": {
            "github": config.github_webhook_secret.is_some(),
            "registry": config.registry_webhook_secret.is_some(),
            "rules": config.webhook_rules.len()
        },
        "secretSync": { "enabled": config.gh_sync_enabled, "rules": config.gh_sync_rules.len() }
    }))
}

pub(crate) async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let jobs = state.jobs.read().await;
    let queued = jobs
        .values()
        .filter(|job| matches!(job.status, BuildStatus::Queued))
        .count();
    let mut allowed_namespaces = state
        .config
        .allowed_namespaces
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    allowed_namespaces.sort();
    let mut allowed_repo_prefixes = state.config.allowed_repo_prefixes.clone();
    allowed_repo_prefixes.sort();
    let mut allowed_image_prefixes = state.config.allowed_image_prefixes.clone();
    allowed_image_prefixes.sort();
    let mut allowed_profiles = state
        .config
        .allowed_profiles
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    allowed_profiles.sort();
    let mut allowed_profile_repo_prefixes = state.config.allowed_profile_repo_prefixes.clone();
    allowed_profile_repo_prefixes.sort();

    Json(HealthResponse {
        ok: true,
        service: SERVICE_NAME,
        auth_configured: state.config.server_auth_secret.is_some(),
        deploy_enabled: state.config.deploy_enabled,
        push_enabled: state.config.push_enabled,
        ecr_login_enabled: state.config.ecr_login_enabled,
        allowed_repo_prefixes,
        allowed_image_prefixes,
        allowed_namespaces,
        allowed_profiles,
        allowed_profile_repo_prefixes,
        queued,
        running: state.counters.running.load(Ordering::Relaxed),
    })
}

pub(crate) async fn readyz(State(state): State<AppState>) -> Response {
    let ready = build_dependencies_ready(&state.config);
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "ok": ready,
            "service": SERVICE_NAME,
            "dependenciesReady": ready,
        })),
    )
        .into_response()
}

pub(crate) async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let jobs = state.jobs.read().await;
    let queued = jobs
        .values()
        .filter(|job| matches!(job.status, BuildStatus::Queued))
        .count();
    let mut body = format!(
        "# HELP dd_build_server_jobs_submitted_total Build jobs accepted by the build server.\n\
         # TYPE dd_build_server_jobs_submitted_total counter\n\
         dd_build_server_jobs_submitted_total {}\n\
         # HELP dd_build_server_jobs_running Current running build jobs.\n\
         # TYPE dd_build_server_jobs_running gauge\n\
         dd_build_server_jobs_running {}\n\
         # HELP dd_build_server_jobs_queued Current queued build jobs.\n\
         # TYPE dd_build_server_jobs_queued gauge\n\
         dd_build_server_jobs_queued {}\n\
         # HELP dd_build_server_jobs_succeeded_total Build jobs that completed successfully.\n\
         # TYPE dd_build_server_jobs_succeeded_total counter\n\
         dd_build_server_jobs_succeeded_total {}\n\
         # HELP dd_build_server_jobs_failed_total Build jobs that failed.\n\
         # TYPE dd_build_server_jobs_failed_total counter\n\
         dd_build_server_jobs_failed_total {}\n\
         # HELP dd_build_server_requests_rejected_total Requests rejected before queueing.\n\
         # TYPE dd_build_server_requests_rejected_total counter\n\
         dd_build_server_requests_rejected_total {}\n\
         # HELP dd_build_server_command_failures_total Build pipeline command failures.\n\
         # TYPE dd_build_server_command_failures_total counter\n\
         dd_build_server_command_failures_total {}\n\
         # HELP dd_build_server_ecr_logins_total Successful ECR registry logins.\n\
         # TYPE dd_build_server_ecr_logins_total counter\n\
         dd_build_server_ecr_logins_total {}\n\
         # HELP dd_build_server_ecr_login_failures_total Failed ECR registry logins.\n\
         # TYPE dd_build_server_ecr_login_failures_total counter\n\
         dd_build_server_ecr_login_failures_total {}\n",
        state.counters.submitted.load(Ordering::Relaxed),
        state.counters.running.load(Ordering::Relaxed),
        queued,
        state.counters.succeeded.load(Ordering::Relaxed),
        state.counters.failed.load(Ordering::Relaxed),
        state.counters.rejected.load(Ordering::Relaxed),
        state.counters.command_failures.load(Ordering::Relaxed),
        state.counters.ecr_logins.load(Ordering::Relaxed),
        state.counters.ecr_login_failures.load(Ordering::Relaxed),
    );
    body.push_str(&format!(
        "# HELP dd_build_server_locks_acquired_total fiducia.cloud build locks acquired.\n\
         # TYPE dd_build_server_locks_acquired_total counter\n\
         dd_build_server_locks_acquired_total {}\n\
         # HELP dd_build_server_lock_failures_total fiducia lock contention or unavailability.\n\
         # TYPE dd_build_server_lock_failures_total counter\n\
         dd_build_server_lock_failures_total {}\n\
         # HELP dd_build_server_webhooks_received_total Inbound webhooks accepted (after auth).\n\
         # TYPE dd_build_server_webhooks_received_total counter\n\
         dd_build_server_webhooks_received_total {}\n\
         # HELP dd_build_server_webhooks_rejected_total Inbound webhooks rejected (bad signature/secret).\n\
         # TYPE dd_build_server_webhooks_rejected_total counter\n\
         dd_build_server_webhooks_rejected_total {}\n\
         # HELP dd_build_server_nats_published_total NATS events published.\n\
         # TYPE dd_build_server_nats_published_total counter\n\
         dd_build_server_nats_published_total {}\n\
         # HELP dd_build_server_nats_publish_failures_total NATS publish failures.\n\
         # TYPE dd_build_server_nats_publish_failures_total counter\n\
         dd_build_server_nats_publish_failures_total {}\n\
         # HELP dd_build_server_gh_secrets_synced_total GitHub Actions secrets synced.\n\
         # TYPE dd_build_server_gh_secrets_synced_total counter\n\
         dd_build_server_gh_secrets_synced_total {}\n\
         # HELP dd_build_server_gh_secret_sync_failures_total GitHub Actions secret sync failures.\n\
         # TYPE dd_build_server_gh_secret_sync_failures_total counter\n\
         dd_build_server_gh_secret_sync_failures_total {}\n",
        state.counters.locks_acquired.load(Ordering::Relaxed),
        state.counters.lock_failures.load(Ordering::Relaxed),
        state.counters.webhooks_received.load(Ordering::Relaxed),
        state.counters.webhooks_rejected.load(Ordering::Relaxed),
        state.counters.nats_published.load(Ordering::Relaxed),
        state.counters.nats_publish_failures.load(Ordering::Relaxed),
        state.counters.gh_secrets_synced.load(Ordering::Relaxed),
        state.counters.gh_secret_sync_failures.load(Ordering::Relaxed),
    ));
    body.push_str(&format!(
        "# HELP dd_build_server_dependencies_ready Whether auth, work storage, and required build tools are available.\n\
         # TYPE dd_build_server_dependencies_ready gauge\n\
         dd_build_server_dependencies_ready {}\n",
        u8::from(build_dependencies_ready(&state.config))
    ));
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

pub(crate) async fn submit_build(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BuildRequest>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    match enqueue_build(&state, request, "http").await {
        Ok(record) => (StatusCode::ACCEPTED, Json(record)).into_response(),
        Err((status, message)) => (status, Json(json!({ "error": message }))).into_response(),
    }
}

pub(crate) async fn sync_secrets(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if !state.config.gh_sync_enabled {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "gh secret sync is disabled by BUILD_SERVER_GH_SYNC_ENABLED=false" })),
        )
            .into_response();
    }
    let outcomes = gh_secrets::sync_all(&state).await;
    (StatusCode::OK, Json(json!({ "outcomes": outcomes }))).into_response()
}

pub(crate) async fn sync_secrets_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let runs = match state.db.as_ref() {
        Some(db) => db::recent_secret_sync_runs(db, 100).await,
        None => Vec::new(),
    };
    (StatusCode::OK, Json(json!({ "runs": runs }))).into_response()
}

pub(crate) async fn list_builds(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let mut jobs = state
        .jobs
        .read()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at_ms));
    // With persistence on, also surface recent jobs from prior processes
    // (the in-memory map only holds this process's jobs).
    if let Some(db) = state.db.as_ref() {
        let known: HashSet<String> = jobs.iter().map(|job| job.id.clone()).collect();
        let persisted = db::recent_jobs(db, 200).await;
        let mut merged = persisted
            .into_iter()
            .filter(|row| !known.contains(&row.id))
            .map(|row| {
                json!({
                    "id": row.id,
                    "status": row.status,
                    "jobKind": row.job_kind,
                    "source": row.source,
                    "executor": row.executor,
                    "repoUrl": row.repo_url,
                    "gitRef": row.git_ref,
                    "image": row.image,
                    "error": row.error,
                    "persisted": true,
                })
            })
            .collect::<Vec<_>>();
        let mut live = jobs
            .iter()
            .map(|job| serde_json::to_value(job).unwrap_or(serde_json::Value::Null))
            .collect::<Vec<_>>();
        live.append(&mut merged);
        return Json(live).into_response();
    }
    Json(jobs).into_response()
}

pub(crate) async fn get_build(
    State(state): State<AppState>,
    AxumPath(job_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let jobs = state.jobs.read().await;
    match jobs.get(&job_id) {
        Some(job) => Json(job).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "build job not found" })),
        )
            .into_response(),
    }
}

pub(crate) async fn get_build_logs(
    State(state): State<AppState>,
    AxumPath(job_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let log_path = {
        let jobs = state.jobs.read().await;
        match jobs.get(&job_id) {
            Some(job) => PathBuf::from(&job.log_path),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": "build job not found" })),
                )
                    .into_response();
            }
        }
    };

    match fs::read_to_string(&log_path).await {
        Ok(body) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "build log not found" })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("failed to read build log: {error}") })),
        )
            .into_response(),
    }
}

pub(crate) async fn get_build_artifacts(
    State(state): State<AppState>,
    AxumPath(job_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    {
        let jobs = state.jobs.read().await;
        if !jobs.contains_key(&job_id) {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "build job not found" })),
            )
                .into_response();
        }
    }

    let artifact_path = state
        .config
        .work_root
        .join(&job_id)
        .join("artifacts.tar.gz");
    match fs::File::open(&artifact_path).await {
        Ok(file) => {
            let stream = ReaderStream::new(file);
            let disposition = format!("attachment; filename=\"{job_id}-artifacts.tar.gz\"");
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "application/gzip".to_string()),
                    (header::CONTENT_DISPOSITION, disposition),
                ],
                Body::from_stream(stream),
            )
                .into_response()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "build artifacts not found" })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("failed to open build artifacts: {error}") })),
        )
            .into_response(),
    }
}

pub(crate) async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

pub(crate) async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

/// Build the HTTP router with all routes and the request state baked in.
/// Extracted so integration tests can drive the exact production route table
/// in-process via `tower::ServiceExt::oneshot`.
pub(crate) fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(descriptor))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/builds", get(list_builds).post(submit_build))
        .route("/builds/:job_id", get(get_build))
        .route("/builds/:job_id/logs", get(get_build_logs))
        .route("/builds/:job_id/artifacts", get(get_build_artifacts))
        .route("/webhooks/github", post(webhooks::github_webhook))
        .route("/webhooks/registry", post(webhooks::registry_webhook))
        .route("/secrets/sync", post(sync_secrets))
        .route("/secrets/sync/status", get(sync_secrets_status))
        .with_state(state)
        .merge(dd_runtime_config_client::router())
}

/// End-to-end HTTP tests that drive the production `build_router` in-process via
/// `tower::ServiceExt::oneshot`. No network, DB, NATS, or fiducia is required:
/// db/nats are `None` and coordination is disabled, so these exercise the real
/// auth middleware, request validation, webhook verification, and read handlers
/// exactly as deployed. They deliberately use payloads that are rejected BEFORE
/// a job is enqueued, so no real `git`/`nerdctl` subprocess is ever spawned.
#[cfg(test)]
mod e2e {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use hmac::{Hmac, Mac};
    use tokio::sync::{RwLock, Semaphore};
    use tower::ServiceExt;

    use crate::config::Config;
    use crate::state::Counters;

    const AUTH: &str = "test-server-auth-secret";
    const GH_HOOK: &str = "test-github-webhook-secret";
    const REG_HOOK: &str = "test-registry-webhook-secret";

    fn test_config() -> Config {
        let unique = uuid::Uuid::new_v4();
        Config {
            work_root: std::env::temp_dir().join(format!("dd-bs-e2e-{unique}")),
            git_bin: "git".to_string(),
            git_credential_source: None,
            nerdctl_bin: "nerdctl".to_string(),
            kubectl_bin: "kubectl".to_string(),
            tar_bin: "tar".to_string(),
            containerd_namespace: "dd-build-test".to_string(),
            allowed_repo_prefixes: vec!["https://github.com/ORESoftware/".to_string()],
            allowed_image_prefixes: vec![
                "710156900967.dkr.ecr.us-east-1.amazonaws.com/".to_string()
            ],
            allowed_namespaces: HashSet::from(["default".to_string()]),
            allowed_profiles: HashSet::from(["playwright".to_string()]),
            allowed_profile_repo_prefixes: vec!["https://github.com/ORESoftware/".to_string()],
            profile_cpus: "2".to_string(),
            profile_memory: "2g".to_string(),
            profile_pids_limit: "512".to_string(),
            deploy_enabled: true,
            push_enabled: false,
            ecr_login_enabled: false,
            aws_region: "us-east-1".to_string(),
            job_timeout: Duration::from_secs(60),
            job_deadline: Duration::from_secs(120),
            max_log_bytes: 1_000_000,
            max_jobs: 100,
            max_queued: 16,
            keep_workdirs: false,
            server_auth_secret: Some(AUTH.to_string()),
            database_url: None,
            fiducia_url: "http://127.0.0.1:1/unused".to_string(),
            fiducia_api_key: None,
            coordination_enabled: false,
            coordination_required: false,
            lock_ttl: Duration::from_secs(60),
            lock_wait_budget: Duration::from_secs(1),
            lock_retry_interval: Duration::from_millis(10),
            idempotency_lease: Duration::from_secs(60),
            idempotency_retention: Duration::from_secs(60),
            nats_url: "nats://127.0.0.1:4222".to_string(),
            nats_enabled: false,
            nats_intake_enabled: false,
            nats_event_subject: "dd.remote.build_server.events".to_string(),
            nats_result_subject: "dd.remote.build_server.results".to_string(),
            nats_image_subject: "dd.remote.build_server.images".to_string(),
            nats_request_subject: "dd.remote.build_server.requests".to_string(),
            nats_critical_subject: "dd.remote.events.critical".to_string(),
            github_webhook_secret: Some(GH_HOOK.to_string()),
            registry_webhook_secret: Some(REG_HOOK.to_string()),
            webhook_rules: Vec::new(),
            gh_sync_enabled: false,
            gh_sync_token: None,
            gh_sync_rules: Vec::new(),
            gh_sync_interval: Duration::ZERO,
            lambda_executor_enabled: false,
            lambda_url: "http://127.0.0.1:1/unused".to_string(),
            lambda_function_id: None,
            lambda_auth_secret: None,
        }
    }

    fn state_from(config: Config) -> AppState {
        let max_concurrent = 1;
        AppState {
            config: Arc::new(config),
            http: reqwest::Client::new(),
            jobs: Arc::new(RwLock::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            counters: Arc::new(Counters::default()),
            db: None,
            nats: None,
            holder: "dd-build-server/e2e-test".to_string(),
            recent_request_ids: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    fn app(config: Config) -> Router {
        build_router(state_from(config))
    }

    async fn send(router: Router, request: Request<Body>) -> (StatusCode, String) {
        let response = router
            .oneshot(request)
            .await
            .expect("router handled request");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .expect("body");
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    fn post_json(uri: &str, auth: Option<&str>, body: &serde_json::Value) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(secret) = auth {
            builder = builder.header("x-server-auth", secret);
        }
        builder.body(Body::from(body.to_string())).unwrap()
    }

    fn github_sig(secret: &str, body: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body.as_bytes());
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    // ---- health / observability: unauthenticated, no secret leakage ----

    #[tokio::test]
    async fn health_ready_metrics_are_public_and_ok() {
        for path in ["/healthz", "/readyz", "/metrics"] {
            let (status, _) = send(app(test_config()), get(path)).await;
            assert!(
                status == StatusCode::OK || status == StatusCode::SERVICE_UNAVAILABLE,
                "{path} returned {status}"
            );
        }
    }

    #[tokio::test]
    async fn healthz_does_not_leak_the_auth_secret() {
        let (_, body) = send(app(test_config()), get("/healthz")).await;
        assert!(!body.contains(AUTH), "healthz body leaked the auth secret");
    }

    // ---- auth enforcement on mutating routes ----

    #[tokio::test]
    async fn submit_without_auth_is_rejected_before_any_work() {
        let body = json!({ "repoUrl": "https://github.com/ORESoftware/x.git", "image": "710156900967.dkr.ecr.us-east-1.amazonaws.com/x:tag" });
        let (status, _) = send(app(test_config()), post_json("/builds", None, &body)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn submit_with_wrong_secret_is_unauthorized() {
        let body = json!({ "repoUrl": "https://github.com/ORESoftware/x.git" });
        let (status, _) = send(
            app(test_config()),
            post_json("/builds", Some("wrong"), &body),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wrong_secret_of_different_length_still_unauthorized_no_length_leak() {
        // The digest compare must not behave differently for a short vs long
        // wrong secret; both are simply unauthorized.
        for wrong in ["x", "a-much-longer-wrong-secret-value-than-the-real-one"] {
            let body = json!({ "repoUrl": "https://github.com/ORESoftware/x.git" });
            let (status, _) =
                send(app(test_config()), post_json("/builds", Some(wrong), &body)).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "wrong secret {wrong:?}");
        }
    }

    #[tokio::test]
    async fn alternate_auth_headers_are_accepted() {
        for header in ["x-server-auth", "x-build-server-auth", "x-agent-auth"] {
            // Authenticated but repo not allowed → 400 (proves auth passed, then
            // validation ran). A 401 would mean the header was not honored.
            let body = json!({ "repoUrl": "https://github.com/attacker/x.git" });
            let request = Request::builder()
                .method("POST")
                .uri("/builds")
                .header("content-type", "application/json")
                .header(header, AUTH)
                .body(Body::from(body.to_string()))
                .unwrap();
            let (status, _) = send(app(test_config()), request).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "header {header}");
        }
    }

    #[tokio::test]
    async fn secrets_sync_requires_auth_then_reports_disabled() {
        let (status, _) = send(
            app(test_config()),
            post_json("/secrets/sync", None, &json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, body) = send(
            app(test_config()),
            post_json("/secrets/sync", Some(AUTH), &json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.contains("disabled"));
    }

    // ---- submission validation: the injection guards, end to end over HTTP ----

    async fn assert_submit_rejected(body: serde_json::Value) {
        let (status, _) = send(app(test_config()), post_json("/builds", Some(AUTH), &body)).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "expected 400 for payload {body}"
        );
    }

    #[tokio::test]
    async fn submit_rejects_repo_outside_allowlist() {
        assert_submit_rejected(json!({
            "repoUrl": "https://github.com/attacker/evil.git",
            "image": "710156900967.dkr.ecr.us-east-1.amazonaws.com/x:tag"
        }))
        .await;
    }

    #[tokio::test]
    async fn submit_rejects_mount_injection_in_context_dir() {
        assert_submit_rejected(json!({
            "repoUrl": "https://github.com/ORESoftware/x.git",
            "image": "710156900967.dkr.ecr.us-east-1.amazonaws.com/x:tag",
            "contextDir": "x,src=/home/ec2-user"
        }))
        .await;
    }

    #[tokio::test]
    async fn submit_rejects_dockerfile_traversal() {
        assert_submit_rejected(json!({
            "repoUrl": "https://github.com/ORESoftware/x.git",
            "image": "710156900967.dkr.ecr.us-east-1.amazonaws.com/x:tag",
            "dockerfile": "../../etc/passwd"
        }))
        .await;
    }

    #[tokio::test]
    async fn submit_rejects_leading_dash_image() {
        assert_submit_rejected(json!({
            "repoUrl": "https://github.com/ORESoftware/x.git",
            "image": "-t:latest"
        }))
        .await;
    }

    #[tokio::test]
    async fn submit_rejects_rollout_flag_injection() {
        assert_submit_rejected(json!({
            "repoUrl": "https://github.com/ORESoftware/x.git",
            "image": "710156900967.dkr.ecr.us-east-1.amazonaws.com/x:tag",
            "deploy": { "kind": "manifest", "path": "k8s/app.yaml", "rollout": "--server=http://evil/" }
        }))
        .await;
    }

    #[tokio::test]
    async fn submit_rejects_file_transport_repo_url() {
        assert_submit_rejected(json!({
            "repoUrl": "file:///etc/passwd",
            "image": "710156900967.dkr.ecr.us-east-1.amazonaws.com/x:tag"
        }))
        .await;
    }

    #[tokio::test]
    async fn submit_rejects_disallowed_profile() {
        assert_submit_rejected(json!({
            "jobKind": "run-profile",
            "repoUrl": "https://github.com/ORESoftware/x.git",
            "profile": "sh -c evil"
        }))
        .await;
    }

    #[tokio::test]
    async fn submit_with_empty_allowlist_fails_closed() {
        let mut config = test_config();
        config.allowed_repo_prefixes = Vec::new();
        let body = json!({
            "repoUrl": "https://github.com/ORESoftware/x.git",
            "image": "710156900967.dkr.ecr.us-east-1.amazonaws.com/x:tag"
        });
        let (status, _) = send(app(config), post_json("/builds", Some(AUTH), &body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // ---- GitHub webhook: HMAC gate, fails closed, no panic on bad sha ----

    #[tokio::test]
    async fn github_webhook_missing_signature_is_unauthorized() {
        let body = json!({ "ref": "refs/heads/dev" }).to_string();
        let request = Request::builder()
            .method("POST")
            .uri("/webhooks/github")
            .header("content-type", "application/json")
            .header("x-github-event", "push")
            .header("x-github-delivery", "d1")
            .body(Body::from(body))
            .unwrap();
        let (status, _) = send(app(test_config()), request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn github_webhook_bad_signature_is_unauthorized() {
        let body = json!({ "ref": "refs/heads/dev" }).to_string();
        let request = Request::builder()
            .method("POST")
            .uri("/webhooks/github")
            .header("content-type", "application/json")
            .header("x-github-event", "push")
            .header("x-github-delivery", "d1")
            .header("x-hub-signature-256", "sha256=deadbeef")
            .body(Body::from(body))
            .unwrap();
        let (status, _) = send(app(test_config()), request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn github_webhook_valid_signature_non_actionable_is_ignored() {
        let body = json!({ "ref": "refs/heads/dev", "deleted": true,
            "repository": { "full_name": "ORESoftware/x" } })
        .to_string();
        let sig = github_sig(GH_HOOK, &body);
        let request = Request::builder()
            .method("POST")
            .uri("/webhooks/github")
            .header("content-type", "application/json")
            .header("x-github-event", "push")
            .header("x-github-delivery", "d-actionable")
            .header("x-hub-signature-256", sig)
            .body(Body::from(body))
            .unwrap();
        let (status, body) = send(app(test_config()), request).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("ignored"));
    }

    #[tokio::test]
    async fn github_webhook_malformed_sha_does_not_panic_and_is_ignored() {
        // A non-ASCII/short `after` used to panic via a byte-slice; now it is
        // gated by valid_commit_sha and ignored. A real matching rule exists so
        // the only thing stopping a build is the sha check.
        let mut config = test_config();
        config.webhook_rules = vec![webhooks::WebhookRule {
            repo: "ORESoftware/x".to_string(),
            branch: Some("dev".to_string()),
            tags: false,
            events: Some(vec!["push".to_string()]),
            image: Some(
                "710156900967.dkr.ecr.us-east-1.amazonaws.com/x:dev-{shortSha}".to_string(),
            ),
            profile: None,
            context_dir: None,
            dockerfile: None,
            push: false,
            executor: None,
            deploy: None,
        }];
        let body = json!({ "ref": "refs/heads/dev", "after": "zzz\u{e9}",
            "repository": { "full_name": "ORESoftware/x" } })
        .to_string();
        let sig = github_sig(GH_HOOK, &body);
        let request = Request::builder()
            .method("POST")
            .uri("/webhooks/github")
            .header("content-type", "application/json")
            .header("x-github-event", "push")
            .header("x-github-delivery", "d-badsha")
            .header("x-hub-signature-256", sig)
            .body(Body::from(body))
            .unwrap();
        let (status, body) = send(app(config), request).await;
        assert_eq!(status, StatusCode::OK, "must not 500/panic");
        assert!(body.contains("ignored"));
    }

    // ---- registry webhook: secret gate + delivery-id dedupe guard ----

    #[tokio::test]
    async fn registry_webhook_wrong_secret_is_unauthorized() {
        let request = Request::builder()
            .method("POST")
            .uri("/webhooks/registry")
            .header("content-type", "application/json")
            .header("x-registry-webhook-secret", "wrong")
            .header("x-delivery-id", "r1")
            .body(Body::from(json!({ "events": [] }).to_string()))
            .unwrap();
        let (status, _) = send(app(test_config()), request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn registry_webhook_missing_delivery_id_is_rejected() {
        let request = Request::builder()
            .method("POST")
            .uri("/webhooks/registry")
            .header("content-type", "application/json")
            .header("x-registry-webhook-secret", REG_HOOK)
            .body(Body::from(json!({ "events": [] }).to_string()))
            .unwrap();
        let (status, body) = send(app(test_config()), request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.to_lowercase().contains("delivery"));
    }

    #[tokio::test]
    async fn registry_webhook_valid_is_accepted() {
        let request = Request::builder()
            .method("POST")
            .uri("/webhooks/registry")
            .header("content-type", "application/json")
            .header("x-registry-webhook-secret", REG_HOOK)
            .header("x-delivery-id", "r-ok")
            .body(Body::from(
                json!({ "events": [{ "action": "push",
                    "target": { "repository": "dd/x", "tag": "t", "digest": "sha256:abc" } }] })
                .to_string(),
            ))
            .unwrap();
        let (status, _) = send(app(test_config()), request).await;
        assert_eq!(status, StatusCode::OK);
    }

    // ---- path-traversal safety on the job read endpoints ----

    fn authed_get(uri: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header("x-server-auth", AUTH)
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn job_read_endpoints_require_auth() {
        // Unauthenticated reads are rejected before any filesystem/map access.
        for uri in ["/builds/anything/logs", "/builds/anything/artifacts"] {
            let (status, _) = send(app(test_config()), get(uri)).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        }
    }

    #[tokio::test]
    async fn job_logs_traversal_and_unknown_id_are_not_found() {
        // Even WITH valid auth, a traversal or unknown job id resolves through
        // the in-memory job map (miss → 404), never a raw filesystem path, so
        // `../../etc/passwd` cannot read an arbitrary file.
        for uri in [
            "/builds/..%2f..%2f..%2fetc%2fpasswd/logs",
            "/builds/does-not-exist/logs",
            "/builds/does-not-exist/artifacts",
        ] {
            let (status, body) = send(app(test_config()), authed_get(uri)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
            assert!(
                !body.contains("root:"),
                "{uri} appears to have read /etc/passwd"
            );
        }
    }

    #[tokio::test]
    async fn list_builds_requires_auth() {
        let (status, _) = send(app(test_config()), get("/builds")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
