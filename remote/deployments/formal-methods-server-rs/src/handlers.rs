use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use axum::{
    body::Bytes,
    extract::{Path as AxumPath, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use tokio::fs;

use crate::{
    annotations::parse_annotations,
    github::{extract_pr_from_event, verify_github_signature},
    jobs::{job_id, prune_jobs, run_job},
    state::{now_ms, AppState},
    types::{
        AnalyzeRequest, HealthResponse, JobRecord, JobStatus, ValidateRequest, ValidateResponse,
    },
    validation::{
        clean_optional, ensure_allowed_prefix, require_auth, validate_analyze_request,
        validate_repo_url,
    },
    verify::{heuristic_checks, verify_block, VerifyContext},
    SCHEMA_VERSION, SERVICE_NAME,
};

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

pub(crate) async fn descriptor() -> impl IntoResponse {
    Json(json!({
        "service": SERVICE_NAME,
        "description": "Annotation-driven formal-methods analyser. Submit a repo, inline source, or a GitHub pull-request webhook event; get SMT-checked findings.",
        "schemaVersion": SCHEMA_VERSION,
        "endpoints": {
            "submit": "POST /analyses",
            "list": "GET /analyses",
            "status": "GET /analyses/<jobId>",
            "logs": "GET /analyses/<jobId>/logs",
            "validate": "POST /validate",
            "githubWebhook": "POST /webhooks/github (HMAC-verified, no x-server-auth)",
            "pullRequestStatus": "GET /pulls/<owner>/<repo>/<number>",
            "healthz": "GET /healthz",
            "metrics": "GET /metrics"
        },
        "annotationDsl": {
            "decl": "// @var name: Int|Real|Bool",
            "assume": "// @assume <expr>",
            "requires": "// @requires <expr>",
            "ensures": "// @ensures <expr>",
            "invariant": "// @invariant <expr>",
            "variant": "// @variant <int-expr>",
            "assert": "// @assert <expr>"
        },
        "supportedOperators": [
            "||", "&&", "!", "==", "!=", "<", "<=", ">", ">=",
            "+", "-", "*", "/", "%",
            "min(_,_)", "max(_,_)", "abs(_)"
        ],
        "reasoningModes": [
            "deduction (SMT refutation of negated goals)",
            "induction (loop @invariant base step + @variant non-negativity)",
            "path-condition propagation through nested `if (...)` branches"
        ],
        "githubWebhook": {
            "url": "POST /webhooks/github",
            "signatureHeader": "X-Hub-Signature-256",
            "secretEnv": "GITHUB_WEBHOOK_SECRET",
            "events": ["pull_request"],
            "actions": ["opened", "synchronize", "reopened", "ready_for_review"]
        }
    }))
}

pub(crate) async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let jobs = state.jobs.read().await;
    let queued = jobs
        .values()
        .filter(|job| matches!(job.status, JobStatus::Queued))
        .count();
    let mut allowed_repo_prefixes = state.config.allowed_repo_prefixes.clone();
    allowed_repo_prefixes.sort();
    let mut allowed_extensions: Vec<String> =
        state.config.allowed_extensions.iter().cloned().collect();
    allowed_extensions.sort();
    let z3_available = which_exists(&state.config.z3_bin).await;
    Json(HealthResponse {
        ok: true,
        service: SERVICE_NAME,
        schema_version: SCHEMA_VERSION,
        auth_configured: state.config.server_auth_secret.is_some(),
        z3_available,
        github_webhook_configured: state.config.github_webhook_secret.is_some(),
        github_comments_enabled: state.config.pr_comment_enabled
            && state.config.github_api_token.is_some(),
        pr_diff_only: state.config.pr_diff_only,
        allowed_repo_prefixes,
        allowed_extensions,
        queued,
        running: state.counters.running.load(Ordering::Relaxed),
    })
}

async fn which_exists(bin: &str) -> bool {
    if bin.contains('/') {
        return fs::metadata(bin).await.is_ok();
    }
    let paths = env::var("PATH").unwrap_or_default();
    for dir in paths.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(bin);
        if fs::metadata(&candidate).await.is_ok() {
            return true;
        }
    }
    false
}

pub(crate) async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let jobs = state.jobs.read().await;
    let queued = jobs
        .values()
        .filter(|job| matches!(job.status, JobStatus::Queued))
        .count();
    let body = format!(
        "# HELP dd_formal_methods_jobs_submitted_total Analysis jobs accepted.\n\
         # TYPE dd_formal_methods_jobs_submitted_total counter\n\
         dd_formal_methods_jobs_submitted_total {}\n\
         # HELP dd_formal_methods_jobs_running Current running jobs.\n\
         # TYPE dd_formal_methods_jobs_running gauge\n\
         dd_formal_methods_jobs_running {}\n\
         # HELP dd_formal_methods_jobs_queued Current queued jobs.\n\
         # TYPE dd_formal_methods_jobs_queued gauge\n\
         dd_formal_methods_jobs_queued {}\n\
         # HELP dd_formal_methods_jobs_succeeded_total Analyses that completed successfully.\n\
         # TYPE dd_formal_methods_jobs_succeeded_total counter\n\
         dd_formal_methods_jobs_succeeded_total {}\n\
         # HELP dd_formal_methods_jobs_failed_total Analyses that failed.\n\
         # TYPE dd_formal_methods_jobs_failed_total counter\n\
         dd_formal_methods_jobs_failed_total {}\n\
         # HELP dd_formal_methods_requests_rejected_total Requests rejected before queueing.\n\
         # TYPE dd_formal_methods_requests_rejected_total counter\n\
         dd_formal_methods_requests_rejected_total {}\n\
         # HELP dd_formal_methods_findings_total Findings emitted across all analyses.\n\
         # TYPE dd_formal_methods_findings_total counter\n\
         dd_formal_methods_findings_total {}\n\
         # HELP dd_formal_methods_z3_calls_total Z3 invocations.\n\
         # TYPE dd_formal_methods_z3_calls_total counter\n\
         dd_formal_methods_z3_calls_total {}\n\
         # HELP dd_formal_methods_z3_failures_total Z3 invocations that errored.\n\
         # TYPE dd_formal_methods_z3_failures_total counter\n\
         dd_formal_methods_z3_failures_total {}\n\
         # HELP dd_formal_methods_webhooks_received_total GitHub webhooks accepted.\n\
         # TYPE dd_formal_methods_webhooks_received_total counter\n\
         dd_formal_methods_webhooks_received_total {}\n\
         # HELP dd_formal_methods_webhooks_rejected_total GitHub webhooks rejected (bad HMAC or shape).\n\
         # TYPE dd_formal_methods_webhooks_rejected_total counter\n\
         dd_formal_methods_webhooks_rejected_total {}\n\
         # HELP dd_formal_methods_pr_jobs_queued_total PR-driven analysis jobs queued.\n\
         # TYPE dd_formal_methods_pr_jobs_queued_total counter\n\
         dd_formal_methods_pr_jobs_queued_total {}\n\
         # HELP dd_formal_methods_pr_comments_posted_total PR comments successfully posted to GitHub.\n\
         # TYPE dd_formal_methods_pr_comments_posted_total counter\n\
         dd_formal_methods_pr_comments_posted_total {}\n\
         # HELP dd_formal_methods_pr_comments_failed_total PR comment POSTs that failed.\n\
         # TYPE dd_formal_methods_pr_comments_failed_total counter\n\
         dd_formal_methods_pr_comments_failed_total {}\n",
        state.counters.submitted.load(Ordering::Relaxed),
        state.counters.running.load(Ordering::Relaxed),
        queued,
        state.counters.succeeded.load(Ordering::Relaxed),
        state.counters.failed.load(Ordering::Relaxed),
        state.counters.rejected.load(Ordering::Relaxed),
        state.counters.findings_total.load(Ordering::Relaxed),
        state.counters.z3_calls.load(Ordering::Relaxed),
        state.counters.z3_failures.load(Ordering::Relaxed),
        state.counters.webhooks_received.load(Ordering::Relaxed),
        state.counters.webhooks_rejected.load(Ordering::Relaxed),
        state.counters.pr_jobs_queued.load(Ordering::Relaxed),
        state.counters.pr_comments_posted.load(Ordering::Relaxed),
        state.counters.pr_comments_failed.load(Ordering::Relaxed),
    );
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

async fn enqueue_job(state: &AppState, request: AnalyzeRequest) -> JobRecord {
    let counter = state.counters.submitted.fetch_add(1, Ordering::Relaxed) + 1;
    let id = job_id(counter);
    let job_dir = state.config.work_root.join(&id);
    let log_path = job_dir.join("analysis.log");
    let pull_request = request.pull_request.clone();
    let record = JobRecord {
        id: id.clone(),
        status: JobStatus::Queued,
        request,
        created_at_ms: now_ms(),
        started_at_ms: None,
        finished_at_ms: None,
        log_path: log_path.to_string_lossy().to_string(),
        error: None,
        findings_count: 0,
        findings: Vec::new(),
        files_scanned: 0,
        z3_queries: 0,
        pull_request,
        changed_paths: None,
        pr_comment_status: None,
    };
    {
        let mut jobs = state.jobs.write().await;
        jobs.insert(id.clone(), record.clone());
    }
    prune_jobs(state).await;
    let task_state = state.clone();
    tokio::spawn(async move {
        run_job(task_state, id).await;
    });
    record
}

pub(crate) async fn submit_analysis(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AnalyzeRequest>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if let Err(error) = validate_analyze_request(&state.config, &request) {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    let record = enqueue_job(&state, request).await;
    (StatusCode::ACCEPTED, Json(record)).into_response()
}

pub(crate) async fn github_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    state
        .counters
        .webhooks_received
        .fetch_add(1, Ordering::Relaxed);
    let Some(secret) = state.config.github_webhook_secret.as_deref() else {
        state
            .counters
            .webhooks_rejected
            .fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "GITHUB_WEBHOOK_SECRET is not configured" })),
        )
            .into_response();
    };
    let signature = match headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
    {
        Some(value) => value.to_string(),
        None => {
            state
                .counters
                .webhooks_rejected
                .fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "missing X-Hub-Signature-256 header" })),
            )
                .into_response();
        }
    };
    if !verify_github_signature(secret, &body, &signature) {
        state
            .counters
            .webhooks_rejected
            .fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid X-Hub-Signature-256" })),
        )
            .into_response();
    }
    let event = headers
        .get("x-github-event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    match event {
        "ping" => Json(json!({
            "ok": true,
            "service": SERVICE_NAME,
            "event": "ping",
        }))
        .into_response(),
        "pull_request" => handle_pull_request_event(state, &body).await,
        other => Json(json!({
            "ignored": true,
            "event": other,
        }))
        .into_response(),
    }
}

async fn handle_pull_request_event(state: AppState, body: &[u8]) -> Response {
    let payload: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(error) => {
            state
                .counters
                .webhooks_rejected
                .fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid JSON: {error}") })),
            )
                .into_response();
        }
    };
    let action = payload.get("action").and_then(Value::as_str).unwrap_or("");
    if !matches!(
        action,
        "opened" | "synchronize" | "reopened" | "ready_for_review"
    ) {
        return Json(json!({
            "ignored": true,
            "action": action,
        }))
        .into_response();
    }
    let pr = match extract_pr_from_event(&payload) {
        Ok(pr) => pr,
        Err(error) => {
            state
                .counters
                .webhooks_rejected
                .fetch_add(1, Ordering::Relaxed);
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
        }
    };
    if let Err(error) = validate_repo_url(&pr.head_clone_url) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }
    if let Err(error) = ensure_allowed_prefix(
        "pull_request.head_clone_url",
        &pr.head_clone_url,
        &state.config.allowed_repo_prefixes,
        "FORMAL_METHODS_ALLOWED_REPO_PREFIXES",
    ) {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": error }))).into_response();
    }
    let request = AnalyzeRequest {
        schema_version: Some(SCHEMA_VERSION.to_string()),
        repo_url: Some(pr.head_clone_url.clone()),
        git_ref: Some(pr.head_sha.clone()),
        paths: None,
        languages: None,
        inline_source: None,
        inline_filename: None,
        heuristics: Some(true),
        pull_request: Some(pr.clone()),
    };
    let record = enqueue_job(&state, request).await;
    state
        .counters
        .pr_jobs_queued
        .fetch_add(1, Ordering::Relaxed);
    (StatusCode::ACCEPTED, Json(record)).into_response()
}

pub(crate) async fn get_pull_request_status(
    State(state): State<AppState>,
    AxumPath((owner, repo, number)): AxumPath<(String, String, u64)>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let jobs = state.jobs.read().await;
    let mut matched: Vec<JobRecord> = jobs
        .values()
        .filter(|job| {
            job.pull_request.as_ref().is_some_and(|pr| {
                pr.owner.eq_ignore_ascii_case(&owner)
                    && pr.repo.eq_ignore_ascii_case(&repo)
                    && pr.number == number
            })
        })
        .cloned()
        .collect();
    if matched.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "no analysis jobs found for that pull request" })),
        )
            .into_response();
    }
    matched.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
    let latest = matched.first().cloned();
    Json(json!({
        "owner": owner,
        "repo": repo,
        "number": number,
        "latest": latest,
        "jobs": matched,
    }))
    .into_response()
}

pub(crate) async fn list_analyses(State(state): State<AppState>, headers: HeaderMap) -> Response {
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
    jobs.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
    Json(jobs).into_response()
}

pub(crate) async fn get_analysis(
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
            Json(json!({ "error": "analysis job not found" })),
        )
            .into_response(),
    }
}

pub(crate) async fn get_analysis_logs(
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
                    Json(json!({ "error": "analysis job not found" })),
                )
                    .into_response();
            }
        }
    };
    match fs::read_to_string(&log_path).await {
        Ok(body) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "analysis log not found" })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("failed to read analysis log: {error}") })),
        )
            .into_response(),
    }
}

pub(crate) async fn validate_inline(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ValidateRequest>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if let Some(version) = clean_optional(request.schema_version.as_deref()) {
        if version != SCHEMA_VERSION {
            state.counters.rejected.fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("schemaVersion must be {SCHEMA_VERSION}") })),
            )
                .into_response();
        }
    }
    if request.source.len() > state.config.max_inline_source_bytes {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!(
                    "source must be {} bytes or fewer",
                    state.config.max_inline_source_bytes
                )
            })),
        )
            .into_response();
    }

    let z3_calls = AtomicU64::new(0);
    let z3_failures = AtomicU64::new(0);
    let ctx = VerifyContext {
        config: &state.config,
    };
    let file_label = request
        .filename
        .clone()
        .unwrap_or_else(|| "inline.txt".to_string());
    let parsed = parse_annotations(&file_label, &request.source);
    let mut decls_lookup = HashMap::new();
    for block in &parsed.blocks {
        for decl in &block.decls {
            decls_lookup.insert(decl.name.clone(), decl.sort.clone());
        }
    }
    let mut findings = Vec::new();
    for block in &parsed.blocks {
        let mut block_findings = verify_block(&ctx, block, &z3_calls, &z3_failures).await;
        findings.append(&mut block_findings);
    }
    if request.heuristics.unwrap_or(true) && !decls_lookup.is_empty() {
        let mut h = heuristic_checks(&ctx, &parsed, &decls_lookup, &z3_calls, &z3_failures).await;
        findings.append(&mut h);
    }
    let z3_calls_final = z3_calls.load(Ordering::Relaxed);
    state
        .counters
        .z3_calls
        .fetch_add(z3_calls_final, Ordering::Relaxed);
    state
        .counters
        .z3_failures
        .fetch_add(z3_failures.load(Ordering::Relaxed), Ordering::Relaxed);
    state
        .counters
        .findings_total
        .fetch_add(findings.len() as u64, Ordering::Relaxed);
    Json(ValidateResponse {
        schema_version: SCHEMA_VERSION,
        findings_count: findings.len(),
        findings,
        z3_queries: z3_calls_final,
    })
    .into_response()
}

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

// Pretty-rendered JSON for ad-hoc debugging endpoints (unused but kept for
// completeness so consumers can opt into pretty output if desired).
#[allow(dead_code)]
fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}
