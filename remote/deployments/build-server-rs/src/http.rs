use std::{collections::HashSet, path::PathBuf, sync::atomic::Ordering};

use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
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
use crate::{db, gh_secrets, profiles};

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

pub(crate) async fn sync_secrets_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
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
