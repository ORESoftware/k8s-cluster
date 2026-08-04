use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use gha_clone_server::{
    build_plan, capabilities, is_full_commit_sha, verify_github_signature, PlanRequest,
    PlannerLimits, WorkflowPlan, SERVICE_NAME,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::{
    net::TcpListener,
    sync::RwLock,
    time::{sleep, Duration, Instant},
};
use tracing::{error, info};

use uuid::Uuid;

mod run_reservation;

use run_reservation::{reserve_run_records, ReserveRunsError};

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    client: reqwest::Client,
    runs: Arc<RwLock<BTreeMap<Uuid, RunRecord>>>,
    webhook_deliveries: Arc<RwLock<BTreeMap<String, Instant>>>,
}

#[derive(Clone, Debug)]
struct Config {
    host: String,
    port: u16,
    auth_secret: Option<String>,
    webhook_secret: Option<String>,
    github_token: Option<String>,
    github_api_base_url: String,
    build_server_url: Option<String>,
    build_server_auth: Option<String>,
    allowed_repositories: BTreeSet<String>,
    workflow_rules: BTreeMap<String, Vec<String>>,
    execution_enabled: bool,
    webhook_execution_enabled: bool,
    limits: PlannerLimits,
    build_poll_seconds: u64,
    build_timeout_seconds: u64,
    max_runs: usize,
    webhook_failure_conclusions: BTreeSet<String>,
    webhook_ignored_workflows: BTreeSet<String>,
    webhook_delivery_ttl_seconds: u64,
    max_webhook_deliveries: usize,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let workflow_rules = env::var("GHA_CLONE_WORKFLOW_RULES_JSON")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                serde_json::from_str::<BTreeMap<String, Vec<String>>>(&value)
                    .map_err(|error| format!("GHA_CLONE_WORKFLOW_RULES_JSON is invalid: {error}"))
            })
            .transpose()?
            .unwrap_or_default();

        let allowed_repositories = csv_set("GHA_CLONE_ALLOWED_REPOSITORIES");
        validate_workflow_configuration(&allowed_repositories, &workflow_rules)?;

        Ok(Self {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env_u16("PORT", 8125)?,
            auth_secret: env_optional("GHA_CLONE_AUTH_SECRET"),
            webhook_secret: env_optional("GHA_CLONE_GITHUB_WEBHOOK_SECRET"),
            github_token: env_optional("GHA_CLONE_GITHUB_TOKEN"),
            github_api_base_url: github_api_base_url_from_env()?,
            build_server_url: env_optional("GHA_CLONE_BUILD_SERVER_URL")
                .map(|value| value.trim_end_matches('/').to_string()),
            build_server_auth: env_optional("GHA_CLONE_BUILD_SERVER_AUTH"),
            allowed_repositories,
            workflow_rules,
            execution_enabled: env_bool("GHA_CLONE_EXECUTION_ENABLED", false)?,
            webhook_execution_enabled: env_bool("GHA_CLONE_WEBHOOK_EXECUTION_ENABLED", false)?,
            limits: PlannerLimits {
                max_workflow_bytes: env_usize(
                    "GHA_CLONE_MAX_WORKFLOW_BYTES",
                    gha_clone_server::MAX_WORKFLOW_BYTES_DEFAULT,
                )?,
                max_jobs: env_usize("GHA_CLONE_MAX_JOBS", gha_clone_server::MAX_JOBS_DEFAULT)?,
                max_steps_per_job: env_usize(
                    "GHA_CLONE_MAX_STEPS_PER_JOB",
                    gha_clone_server::MAX_STEPS_PER_JOB_DEFAULT,
                )?,
            },
            build_poll_seconds: env_u64("GHA_CLONE_BUILD_POLL_SECONDS", 2)?,
            build_timeout_seconds: env_u64("GHA_CLONE_BUILD_TIMEOUT_SECONDS", 3600)?,
            max_runs: env_nonzero_usize("GHA_CLONE_MAX_RUNS", 256)?,
            webhook_failure_conclusions: csv_lower_set_or_default(
                "GHA_CLONE_WEBHOOK_FAILURE_CONCLUSIONS",
                &[
                    "failure",
                    "cancelled",
                    "timed_out",
                    "action_required",
                    "startup_failure",
                    "stale",
                ],
            ),
            webhook_ignored_workflows: csv_set_or_default(
                "GHA_CLONE_WEBHOOK_IGNORED_WORKFLOWS",
                &["GHA continuity server"],
            ),
            webhook_delivery_ttl_seconds: env_nonzero_u64(
                "GHA_CLONE_WEBHOOK_DELIVERY_TTL_SECONDS",
                86_400,
            )?,
            max_webhook_deliveries: env_nonzero_usize("GHA_CLONE_MAX_WEBHOOK_DELIVERIES", 4_096)?,
        })
    }

    fn execution_ready(&self) -> bool {
        !self.execution_enabled
            || (self.auth_secret.is_some()
                && self.build_server_url.is_some()
                && self.build_server_auth.is_some()
                && !self.allowed_repositories.is_empty())
    }

    fn webhook_execution_ready(&self) -> bool {
        !self.webhook_execution_enabled
            || (self.execution_enabled
                && self.execution_ready()
                && self.webhook_secret.is_some()
                && !self.workflow_rules.is_empty())
    }

    fn ready(&self) -> bool {
        self.execution_ready() && self.webhook_execution_ready()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunRequest {
    #[serde(flatten)]
    plan: PlanRequest,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunRecord {
    id: Uuid,
    plan_id: String,
    repository: String,
    revision: String,
    workflow_path: String,
    status: RunStatus,
    current_job: Option<String>,
    submissions: Vec<BuildSubmission>,
    error: Option<String>,
    created_at_ms: u128,
    updated_at_ms: u128,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum RunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildSubmission {
    job_id: String,
    profile: String,
    build_job_id: String,
    status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildJobResponse {
    id: String,
    status: String,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildServerRequest<'a> {
    schema_version: &'static str,
    job_kind: &'static str,
    repo_url: String,
    git_ref: &'a str,
    profile: &'a str,
    request_id: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gha_clone_server=info,tower_http=info".into()),
        )
        .init();

    let config = Config::from_env().unwrap_or_else(|error| {
        eprintln!("{SERVICE_NAME}: configuration error: {error}");
        std::process::exit(2);
    });
    let address = format!("{}:{}", config.host, config.port);
    let state = AppState {
        config: Arc::new(config),
        client: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .user_agent("gha-clone-server/0.1")
            .build()
            .expect("reqwest client"),
        runs: Arc::new(RwLock::new(BTreeMap::new())),
        webhook_deliveries: Arc::new(RwLock::new(BTreeMap::new())),
    };
    let app = router(state);
    let listener = TcpListener::bind(&address)
        .await
        .unwrap_or_else(|error| panic!("failed to bind {address}: {error}"));
    info!(%address, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server");
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(descriptor))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v1/capabilities", get(capability_handler))
        .route("/v1/plans", post(create_plan))
        .route("/v1/runs", post(create_run))
        .route("/v1/runs/:id", get(get_run))
        .route("/webhooks/github", post(github_webhook))
        .with_state(state)
}

async fn descriptor() -> Json<Value> {
    Json(json!({
        "service": SERVICE_NAME,
        "purpose": "GitHub Actions continuity through native ARC parity plus a fail-closed independent workflow compiler",
        "endpoints": {
            "capabilities": "GET /v1/capabilities",
            "plan": "POST /v1/plans",
            "run": "POST /v1/runs",
            "runStatus": "GET /v1/runs/<id>",
            "githubWebhook": "POST /webhooks/github",
            "health": "GET /healthz",
            "ready": "GET /readyz"
        }
    }))
}

async fn healthz(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "executionEnabled": state.config.execution_enabled,
        "webhookExecutionEnabled": state.config.webhook_execution_enabled,
        "authConfigured": state.config.auth_secret.is_some(),
        "webhookConfigured": state.config.webhook_secret.is_some(),
        "githubApiConfigured": state.config.github_token.is_some(),
        "buildServerConfigured": state.config.build_server_url.is_some()
            && state.config.build_server_auth.is_some(),
        "allowedRepositories": state.config.allowed_repositories.len(),
        "workflowRules": state.config.workflow_rules.len(),
        "runsRetained": state.runs.read().await.len(),
        "webhookDeliveriesRetained": state.webhook_deliveries.read().await.len(),
        "webhookFailureConclusions": state.config.webhook_failure_conclusions,
        "webhookIgnoredWorkflows": state.config.webhook_ignored_workflows,
        "webhookDeliveryTtlSeconds": state.config.webhook_delivery_ttl_seconds,
        "maxWebhookDeliveries": state.config.max_webhook_deliveries
    }))
}

async fn readyz(State(state): State<AppState>) -> Response {
    let execution_ready = state.config.execution_ready();
    let webhook_execution_ready = state.config.webhook_execution_ready();
    let ready = state.config.ready();
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "ok": ready,
            "service": SERVICE_NAME,
            "executionReady": execution_ready,
            "webhookExecutionReady": webhook_execution_ready
        })),
    )
        .into_response()
}

async fn capability_handler(State(state): State<AppState>) -> Json<Value> {
    Json(serde_json::to_value(capabilities(&state.config.limits)).expect("serializable"))
}

async fn create_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PlanRequest>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if let Err(response) = require_allowed_repository(&request.repository, &state) {
        return response;
    }
    match build_plan(&request, &state.config.limits) {
        Ok(plan) => (StatusCode::OK, Json(plan)).into_response(),
        Err(errors) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "workflow plan rejected", "reasons": errors })),
        )
            .into_response(),
    }
}

async fn create_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RunRequest>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if let Err(response) = require_allowed_repository(&request.plan.repository, &state) {
        return response;
    }
    if !state.config.execution_enabled {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "independent execution is disabled",
                "hint": "set GHA_CLONE_EXECUTION_ENABLED=true only after build-server auth and trusted repository allowlists are reconciled"
            })),
        )
            .into_response();
    }
    let plan = match build_plan(&request.plan, &state.config.limits) {
        Ok(plan) => plan,
        Err(errors) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": "workflow plan rejected", "reasons": errors })),
            )
                .into_response()
        }
    };
    if !plan.independent_executable {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error": "workflow is not independently executable",
                "plan": plan
            })),
        )
            .into_response();
    }

    let record = queued_run_record(&plan);
    if let Err(error) = reserve_runs(&state, vec![record.clone()]).await {
        return reservation_error_response(error);
    }
    let run_id = record.id;
    tokio::spawn(async move {
        if let Err(error) = execute_plan(&state, run_id, plan).await {
            error!(%run_id, %error, "independent workflow run failed");
            update_run(&state, run_id, |run| {
                run.status = RunStatus::Failed;
                run.error = Some(error);
                run.current_job = None;
            })
            .await;
        }
    });
    (StatusCode::ACCEPTED, Json(record)).into_response()
}

async fn get_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    match state.runs.read().await.get(&id).cloned() {
        Some(run) => (StatusCode::OK, Json(run)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "run not found" })),
        )
            .into_response(),
    }
}

async fn github_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(secret) = state.config.webhook_secret.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "GitHub webhook secret is not configured" })),
        )
            .into_response();
    };
    let Some(signature) = headers
        .get("x-hub-signature-256")
        .and_then(|value| value.to_str().ok())
    else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "missing X-Hub-Signature-256" })),
        )
            .into_response();
    };
    if !verify_github_signature(secret, &body, signature) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid GitHub webhook signature" })),
        )
            .into_response();
    }
    let Some(delivery) = github_delivery(&headers) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing or invalid X-GitHub-Delivery UUID" })),
        )
            .into_response();
    };
    let event = headers
        .get("x-github-event")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid webhook JSON: {error}") })),
            )
                .into_response()
        }
    };
    let Some(repository) = payload
        .pointer("/repository/full_name")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "webhook payload is missing repository.full_name" })),
        )
            .into_response();
    };
    if let Err(response) = require_allowed_repository(&repository, &state) {
        return response;
    }
    if event != "workflow_run" {
        return (
            StatusCode::ACCEPTED,
            Json(json!({
                "accepted": false,
                "event": event,
                "delivery": delivery,
                "repository": repository,
                "reason": "only workflow_run events may trigger the failure fallback"
            })),
        )
            .into_response();
    }
    let Some(revision) = webhook_revision(event, &payload) else {
        return (
            StatusCode::ACCEPTED,
            Json(json!({
                "accepted": false,
                "event": event,
                "delivery": delivery,
                "reason": "event does not identify an immutable push, pull-request, or workflow-run revision"
            })),
        )
            .into_response();
    };
    if !is_full_commit_sha(&revision) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "webhook revision is not a full commit SHA" })),
        )
            .into_response();
    }
    let Some(configured_paths) = state.config.workflow_rules.get(&repository) else {
        return (
            StatusCode::ACCEPTED,
            Json(json!({
                "accepted": false,
                "delivery": delivery,
                "repository": repository,
                "revision": revision,
                "reason": "no workflow mirror rules are configured for this repository"
            })),
        )
            .into_response();
    };
    let paths = match webhook_paths(
        event,
        &payload,
        configured_paths,
        &state.config.webhook_failure_conclusions,
        &state.config.webhook_ignored_workflows,
    ) {
        Ok(paths) => paths,
        Err(reason) => {
            return (
                StatusCode::ACCEPTED,
                Json(json!({
                    "accepted": false,
                    "event": event,
                    "delivery": delivery,
                    "repository": repository,
                    "revision": revision,
                    "reason": reason
                })),
            )
                .into_response()
        }
    };
    let mut plans = Vec::new();
    for path in &paths {
        let workflow_yaml = match fetch_workflow(&state, &repository, &revision, path).await {
            Ok(workflow) => workflow,
            Err(error) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": error, "workflowPath": path })),
                )
                    .into_response()
            }
        };
        let request = PlanRequest {
            repository: repository.clone(),
            revision: revision.clone(),
            workflow_path: path.clone(),
            workflow_yaml,
        };
        let plan = match build_plan(&request, &state.config.limits) {
            Ok(plan) => plan,
            Err(reasons) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(json!({
                        "error": "mirrored workflow plan rejected",
                        "workflowPath": path,
                        "reasons": reasons
                    })),
                )
                    .into_response()
            }
        };
        plans.push(plan);
    }

    if state.config.webhook_execution_enabled {
        if !state.config.execution_enabled {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "webhook execution requires GHA_CLONE_EXECUTION_ENABLED=true"
                })),
            )
                .into_response();
        }
        if let Some(plan) = plans.iter().find(|plan| !plan.independent_executable) {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "error": "webhook workflow is not independently executable",
                    "plan": plan
                })),
            )
                .into_response();
        }
        if !remember_webhook_delivery(&state, &delivery).await {
            return (
                StatusCode::ACCEPTED,
                Json(json!({
                    "accepted": false,
                    "event": event,
                    "delivery": delivery,
                    "repository": repository,
                    "revision": revision,
                    "reason": "duplicate GitHub delivery was already dispatched"
                })),
            )
                .into_response();
        }

        let scheduled = plans
            .clone()
            .into_iter()
            .map(|plan| {
                let record = queued_run_record(&plan);
                (record, plan)
            })
            .collect::<Vec<_>>();
        let records = scheduled
            .iter()
            .map(|(record, _)| record.clone())
            .collect::<Vec<_>>();
        if let Err(error) = reserve_runs(&state, records).await {
            forget_webhook_delivery(&state, &delivery).await;
            return reservation_error_response(error);
        }

        let run_ids = scheduled
            .iter()
            .map(|(record, _)| record.id)
            .collect::<Vec<_>>();
        for (record, plan) in scheduled {
            let run_id = record.id;
            let task_state = state.clone();
            tokio::spawn(async move {
                if let Err(error) = execute_plan(&task_state, run_id, plan).await {
                    update_run(&task_state, run_id, |run| {
                        run.status = RunStatus::Failed;
                        run.error = Some(error);
                        run.current_job = None;
                    })
                    .await;
                }
            });
        }
        return (
            StatusCode::ACCEPTED,
            Json(json!({
                "accepted": true,
                "event": event,
                "delivery": delivery,
                "repository": repository,
                "revision": revision,
                "runIds": run_ids
            })),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(json!({
            "accepted": true,
            "execution": false,
            "event": event,
            "delivery": delivery,
            "repository": repository,
            "revision": revision,
            "plans": plans
        })),
    )
        .into_response()
}

fn queued_run_record(plan: &WorkflowPlan) -> RunRecord {
    let now = now_ms();
    RunRecord {
        id: Uuid::new_v4(),
        plan_id: plan.plan_id.clone(),
        repository: plan.repository.clone(),
        revision: plan.revision.clone(),
        workflow_path: plan.workflow_path.clone(),
        status: RunStatus::Queued,
        current_job: None,
        submissions: Vec::new(),
        error: None,
        created_at_ms: now,
        updated_at_ms: now,
    }
}

async fn reserve_runs(state: &AppState, records: Vec<RunRecord>) -> Result<(), ReserveRunsError> {
    let mut runs = state.runs.write().await;
    reserve_run_records(&mut runs, records, state.config.max_runs)
}

fn reservation_error_response(error: ReserveRunsError) -> Response {
    match error {
        ReserveRunsError::Capacity {
            max_runs,
            active_runs,
            requested,
        } => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "error": "run capacity is full",
                "maxRuns": max_runs,
                "activeRuns": active_runs,
                "requestedRuns": requested
            })),
        )
            .into_response(),
        ReserveRunsError::DuplicateId(id) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "generated run identifier collided with retained state",
                "runId": id
            })),
        )
            .into_response(),
    }
}

async fn execute_plan(state: &AppState, run_id: Uuid, plan: WorkflowPlan) -> Result<(), String> {
    let build_server_url = state
        .config
        .build_server_url
        .as_deref()
        .ok_or_else(|| "GHA_CLONE_BUILD_SERVER_URL is not configured".to_string())?;
    let build_server_auth = state
        .config
        .build_server_auth
        .as_deref()
        .ok_or_else(|| "GHA_CLONE_BUILD_SERVER_AUTH is not configured".to_string())?;

    update_run(state, run_id, |run| run.status = RunStatus::Running).await;
    for job_id in &plan.topological_order {
        let job = plan
            .jobs
            .iter()
            .find(|job| &job.id == job_id)
            .ok_or_else(|| format!("planned job {job_id:?} disappeared"))?;
        let profile = job
            .independent_profile
            .as_deref()
            .ok_or_else(|| format!("job {job_id:?} has no fixed profile"))?;
        update_run(state, run_id, |run| {
            run.current_job = Some(job_id.clone());
        })
        .await;

        let request = BuildServerRequest {
            schema_version: "build-server.v1",
            job_kind: "run-profile",
            repo_url: format!("https://github.com/{}.git", plan.repository),
            git_ref: &plan.revision,
            profile,
            request_id: format!("gha-clone:{}:{job_id}", plan.plan_id),
        };
        let response = state
            .client
            .post(format!("{build_server_url}/builds"))
            .header("x-build-server-auth", build_server_auth)
            .json(&request)
            .send()
            .await
            .map_err(|error| format!("build server submission failed for {job_id}: {error}"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| format!("build server response read failed: {error}"))?;
        if status != StatusCode::ACCEPTED {
            return Err(format!(
                "build server rejected {job_id} with HTTP {status}: {}",
                bounded_text(&body, 1024)
            ));
        }
        let build: BuildJobResponse = serde_json::from_str(&body)
            .map_err(|error| format!("build server returned invalid job JSON: {error}"))?;
        update_run(state, run_id, |run| {
            run.submissions.push(BuildSubmission {
                job_id: job_id.clone(),
                profile: profile.to_string(),
                build_job_id: build.id.clone(),
                status: build.status.clone(),
            });
        })
        .await;

        let terminal = wait_for_build(
            state,
            build_server_url,
            build_server_auth,
            job_id,
            &build.id,
        )
        .await?;
        update_run(state, run_id, |run| {
            if let Some(submission) = run
                .submissions
                .iter_mut()
                .find(|submission| submission.build_job_id == build.id)
            {
                submission.status = terminal.status.clone();
            }
        })
        .await;
        if terminal.status != "succeeded" {
            return Err(format!(
                "build-server job {} for workflow job {job_id} ended as {}: {}",
                terminal.id,
                terminal.status,
                terminal.error.unwrap_or_else(|| "no error detail".into())
            ));
        }
    }

    update_run(state, run_id, |run| {
        run.status = RunStatus::Succeeded;
        run.current_job = None;
    })
    .await;
    Ok(())
}

async fn wait_for_build(
    state: &AppState,
    build_server_url: &str,
    build_server_auth: &str,
    workflow_job_id: &str,
    build_job_id: &str,
) -> Result<BuildJobResponse, String> {
    let deadline = Instant::now() + Duration::from_secs(state.config.build_timeout_seconds);
    loop {
        if Instant::now() >= deadline {
            return Err(format!(
                "build-server job {build_job_id} for {workflow_job_id} exceeded {} seconds",
                state.config.build_timeout_seconds
            ));
        }
        let response = state
            .client
            .get(format!("{build_server_url}/builds/{build_job_id}"))
            .header("x-build-server-auth", build_server_auth)
            .send()
            .await
            .map_err(|error| format!("build status request failed: {error}"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| format!("build status response read failed: {error}"))?;
        if status != StatusCode::OK {
            return Err(format!(
                "build status returned HTTP {status}: {}",
                bounded_text(&body, 1024)
            ));
        }
        let build: BuildJobResponse = serde_json::from_str(&body)
            .map_err(|error| format!("build status JSON is invalid: {error}"))?;
        match build.status.as_str() {
            "succeeded" | "failed" => return Ok(build),
            "queued" | "running" => {
                sleep(Duration::from_secs(state.config.build_poll_seconds)).await
            }
            other => return Err(format!("build server returned unknown status {other:?}")),
        }
    }
}

async fn fetch_workflow(
    state: &AppState,
    repository: &str,
    revision: &str,
    path: &str,
) -> Result<String, String> {
    let mut request = state
        .client
        .get(format!(
            "{}/repos/{repository}/contents/{path}?ref={revision}",
            state.config.github_api_base_url
        ))
        .header("Accept", "application/vnd.github.raw+json");
    if let Some(token) = state.config.github_token.as_deref() {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("GitHub workflow fetch failed: {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("GitHub workflow response read failed: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "GitHub workflow fetch returned HTTP {status}: {}",
            bounded_text(&body, 512)
        ));
    }
    if body.len() > state.config.limits.max_workflow_bytes {
        return Err("GitHub workflow exceeds configured byte limit".into());
    }
    Ok(body)
}

fn webhook_paths(
    event: &str,
    payload: &Value,
    configured_paths: &[String],
    failure_conclusions: &BTreeSet<String>,
    ignored_workflows: &BTreeSet<String>,
) -> Result<Vec<String>, String> {
    if event != "workflow_run" {
        return Ok(configured_paths.to_vec());
    }

    let action = payload.get("action").and_then(Value::as_str).unwrap_or("");
    if action != "completed" {
        return Err(format!(
            "workflow_run action {action:?} is not the completed terminal phase"
        ));
    }

    let workflow_name = payload
        .pointer("/workflow_run/name")
        .and_then(Value::as_str)
        .unwrap_or("");
    if ignored_workflows.contains(workflow_name) {
        return Err(format!(
            "workflow {workflow_name:?} is excluded to prevent fallback recursion"
        ));
    }

    let conclusion = payload
        .pointer("/workflow_run/conclusion")
        .and_then(Value::as_str)
        .ok_or_else(|| "completed workflow_run is missing a conclusion".to_string())?
        .to_ascii_lowercase();
    if !failure_conclusions.contains(&conclusion) {
        return Err(format!(
            "workflow_run conclusion {conclusion:?} is not configured for failure fallback"
        ));
    }

    let workflow_path = payload
        .pointer("/workflow_run/path")
        .and_then(Value::as_str)
        .ok_or_else(|| "workflow_run is missing workflow_run.path".to_string())?;
    if !configured_paths.iter().any(|path| path == workflow_path) {
        return Err(format!(
            "failed workflow path {workflow_path:?} is not configured for this repository"
        ));
    }
    Ok(vec![workflow_path.to_string()])
}

fn github_delivery(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-github-delivery")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .map(|value| value.to_string())
}

async fn remember_webhook_delivery(state: &AppState, delivery: &str) -> bool {
    let now = Instant::now();
    let ttl = Duration::from_secs(state.config.webhook_delivery_ttl_seconds);
    let mut deliveries = state.webhook_deliveries.write().await;
    prune_webhook_deliveries(
        &mut deliveries,
        now,
        ttl,
        state.config.max_webhook_deliveries,
    );
    if deliveries.contains_key(delivery) {
        return false;
    }
    deliveries.insert(delivery.to_string(), now);
    prune_webhook_deliveries(
        &mut deliveries,
        now,
        ttl,
        state.config.max_webhook_deliveries,
    );
    true
}

async fn forget_webhook_delivery(state: &AppState, delivery: &str) {
    state.webhook_deliveries.write().await.remove(delivery);
}

fn prune_webhook_deliveries(
    deliveries: &mut BTreeMap<String, Instant>,
    now: Instant,
    ttl: Duration,
    max_deliveries: usize,
) {
    deliveries.retain(|_, seen_at| now.saturating_duration_since(*seen_at) < ttl);
    let remove = deliveries.len().saturating_sub(max_deliveries);
    if remove == 0 {
        return;
    }
    let mut oldest = deliveries
        .iter()
        .map(|(delivery, seen_at)| (*seen_at, delivery.clone()))
        .collect::<Vec<_>>();
    oldest.sort_by_key(|(seen_at, _)| *seen_at);
    for (_, delivery) in oldest.into_iter().take(remove) {
        deliveries.remove(&delivery);
    }
}

fn webhook_revision(event: &str, payload: &Value) -> Option<String> {
    match event {
        "push" => payload.get("after").and_then(Value::as_str),
        "pull_request" => payload
            .pointer("/pull_request/head/sha")
            .and_then(Value::as_str),
        "workflow_run" => payload
            .pointer("/workflow_run/head_sha")
            .and_then(Value::as_str),
        _ => None,
    }
    .map(str::to_string)
}

#[allow(clippy::result_large_err)] // Axum guard returns a response only when a request is rejected.
fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    let Some(expected) = state.config.auth_secret.as_deref() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "GHA_CLONE_AUTH_SECRET is not configured" })),
        )
            .into_response());
    };
    let presented = headers
        .get("x-server-auth")
        .or_else(|| headers.get("x-gha-clone-auth"))
        .and_then(|value| value.to_str().ok());
    if presented.is_some_and(|value| digest_eq(value, expected)) {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response())
    }
}

#[allow(clippy::result_large_err)] // Preserve direct Axum rejection responses without heap boxing.
fn require_allowed_repository(repository: &str, state: &AppState) -> Result<(), Response> {
    if state.config.allowed_repositories.contains(repository) {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "repository is not allowlisted",
                "repository": repository
            })),
        )
            .into_response())
    }
}

fn digest_eq(left: &str, right: &str) -> bool {
    let left = Sha256::digest(left.as_bytes());
    let right = Sha256::digest(right.as_bytes());
    left.as_slice().ct_eq(right.as_slice()).into()
}

async fn update_run<F>(state: &AppState, id: Uuid, mutate: F)
where
    F: FnOnce(&mut RunRecord),
{
    if let Some(run) = state.runs.write().await.get_mut(&id) {
        mutate(run);
        run.updated_at_ms = now_ms();
    }
}

fn validate_workflow_configuration(
    allowed_repositories: &BTreeSet<String>,
    workflow_rules: &BTreeMap<String, Vec<String>>,
) -> Result<(), String> {
    for repository in allowed_repositories {
        if !valid_repository_name(repository) {
            return Err(format!(
                "GHA_CLONE_ALLOWED_REPOSITORIES contains invalid repository {repository:?}"
            ));
        }
    }

    for (repository, paths) in workflow_rules {
        if !allowed_repositories.contains(repository) {
            return Err(format!(
                "workflow rule repository {repository:?} is absent from GHA_CLONE_ALLOWED_REPOSITORIES"
            ));
        }
        if paths.is_empty() {
            return Err(format!(
                "workflow rule repository {repository:?} must contain at least one workflow path"
            ));
        }
        let mut seen = BTreeSet::new();
        for path in paths {
            if !valid_configured_workflow_path(path) {
                return Err(format!(
                    "workflow rule repository {repository:?} contains invalid path {path:?}"
                ));
            }
            if !seen.insert(path) {
                return Err(format!(
                    "workflow rule repository {repository:?} contains duplicate path {path:?}"
                ));
            }
        }
    }
    Ok(())
}

fn valid_repository_name(value: &str) -> bool {
    let Some((owner, repository)) = value.split_once('/') else {
        return false;
    };
    !owner.is_empty()
        && !repository.is_empty()
        && !repository.contains('/')
        && [owner, repository].into_iter().all(|component| {
            component
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}

fn valid_configured_workflow_path(value: &str) -> bool {
    value.starts_with(".github/workflows/")
        && (value.ends_with(".yml") || value.ends_with(".yaml"))
        && !value.contains("..")
        && !value.contains('\\')
        && value.len() <= 256
}

fn github_api_base_url_from_env() -> Result<String, String> {
    normalize_github_api_base_url(
        &env::var("GHA_CLONE_GITHUB_API_BASE_URL")
            .unwrap_or_else(|_| "https://api.github.com".to_string()),
    )
}

fn normalize_github_api_base_url(value: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        return Err("GHA_CLONE_GITHUB_API_BASE_URL must not be empty".to_string());
    }
    let parsed = reqwest::Url::parse(value)
        .map_err(|error| format!("GHA_CLONE_GITHUB_API_BASE_URL is invalid: {error}"))?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(
            "GHA_CLONE_GITHUB_API_BASE_URL must not contain credentials, query, or fragment"
                .to_string(),
        );
    }
    let loopback_http = parsed.scheme() == "http"
        && matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
    if parsed.scheme() != "https" && !loopback_http {
        return Err(
            "GHA_CLONE_GITHUB_API_BASE_URL must use HTTPS; HTTP is allowed only for loopback tests"
                .to_string(),
        );
    }
    Ok(value.to_string())
}

fn csv_set(name: &str) -> BTreeSet<String> {
    env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn csv_set_or_default(name: &str, defaults: &[&str]) -> BTreeSet<String> {
    let configured = csv_set(name);
    if configured.is_empty() {
        defaults.iter().map(|value| (*value).to_string()).collect()
    } else {
        configured
    }
}

fn csv_lower_set_or_default(name: &str, defaults: &[&str]) -> BTreeSet<String> {
    csv_set_or_default(name, defaults)
        .into_iter()
        .map(|value| value.to_ascii_lowercase())
        .collect()
}

fn env_optional(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_bool(name: &str, default: bool) -> Result<bool, String> {
    match env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(format!("{name} must be true or false")),
        },
        Err(_) => Ok(default),
    }
}

fn env_u16(name: &str, default: u16) -> Result<u16, String> {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<u16>()
                .map_err(|error| format!("{name} is invalid: {error}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_u64(name: &str, default: u64) -> Result<u64, String> {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| format!("{name} is invalid: {error}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_usize(name: &str, default: usize) -> Result<usize, String> {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|error| format!("{name} is invalid: {error}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_nonzero_u64(name: &str, default: u64) -> Result<u64, String> {
    let value = env_u64(name, default)?;
    if value == 0 {
        Err(format!("{name} must be greater than zero"))
    } else {
        Ok(value)
    }
}

fn env_nonzero_usize(name: &str, default: usize) -> Result<usize, String> {
    let value = env_usize(name, default)?;
    if value == 0 {
        Err(format!("{name} must be greater than zero"))
    } else {
        Ok(value)
    }
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
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
    fn digest_comparison_is_content_exact_without_length_shortcuts() {
        assert!(digest_eq("same", "same"));
        assert!(!digest_eq("same", "different"));
        assert!(!digest_eq("same", "same\0"));
    }

    #[test]
    fn extracts_only_supported_webhook_revisions() {
        let payload = json!({
            "after": "a".repeat(40),
            "pull_request": { "head": { "sha": "b".repeat(40) } },
            "workflow_run": { "head_sha": "c".repeat(40) }
        });
        assert_eq!(webhook_revision("push", &payload), Some("a".repeat(40)));
        assert_eq!(
            webhook_revision("pull_request", &payload),
            Some("b".repeat(40))
        );
        assert_eq!(
            webhook_revision("workflow_run", &payload),
            Some("c".repeat(40))
        );
        assert_eq!(webhook_revision("issues", &payload), None);
    }

    #[test]
    fn workflow_run_fallback_is_completed_failure_only_and_exact_path() {
        let configured = vec![".github/workflows/ci.yml".to_string()];
        let failures = BTreeSet::from(["failure".to_string(), "timed_out".to_string()]);
        let ignored = BTreeSet::from(["GHA continuity server".to_string()]);
        let failed = json!({
            "action": "completed",
            "workflow_run": {
                "name": "CI",
                "path": ".github/workflows/ci.yml",
                "conclusion": "failure"
            }
        });
        assert_eq!(
            webhook_paths("workflow_run", &failed, &configured, &failures, &ignored).unwrap(),
            configured
        );

        let mut success = failed.clone();
        success["workflow_run"]["conclusion"] = json!("success");
        assert!(
            webhook_paths("workflow_run", &success, &configured, &failures, &ignored)
                .unwrap_err()
                .contains("not configured for failure fallback")
        );

        let mut in_progress = failed.clone();
        in_progress["action"] = json!("in_progress");
        assert!(webhook_paths(
            "workflow_run",
            &in_progress,
            &configured,
            &failures,
            &ignored
        )
        .unwrap_err()
        .contains("not the completed terminal phase"));

        let mut recursive = failed.clone();
        recursive["workflow_run"]["name"] = json!("GHA continuity server");
        assert!(
            webhook_paths("workflow_run", &recursive, &configured, &failures, &ignored)
                .unwrap_err()
                .contains("excluded to prevent fallback recursion")
        );

        let mut unrelated = failed;
        unrelated["workflow_run"]["path"] = json!(".github/workflows/release.yml");
        assert!(
            webhook_paths("workflow_run", &unrelated, &configured, &failures, &ignored)
                .unwrap_err()
                .contains("is not configured for this repository")
        );
    }

    #[test]
    fn github_delivery_requires_a_uuid() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-github-delivery",
            "4f5f1f6e-68a6-4d95-90b4-c0a892938f0f".parse().unwrap(),
        );
        assert_eq!(
            github_delivery(&headers).as_deref(),
            Some("4f5f1f6e-68a6-4d95-90b4-c0a892938f0f")
        );
        headers.insert("x-github-delivery", "not-a-uuid".parse().unwrap());
        assert_eq!(github_delivery(&headers), None);
    }

    #[test]
    fn delivery_retention_expires_old_entries_and_bounds_memory() {
        let now = Instant::now();
        let mut deliveries = BTreeMap::from([
            (
                "expired".to_string(),
                now.checked_sub(Duration::from_secs(120)).unwrap(),
            ),
            (
                "older".to_string(),
                now.checked_sub(Duration::from_secs(20)).unwrap(),
            ),
            (
                "newer".to_string(),
                now.checked_sub(Duration::from_secs(5)).unwrap(),
            ),
        ]);
        prune_webhook_deliveries(&mut deliveries, now, Duration::from_secs(60), 1);
        assert_eq!(deliveries.len(), 1);
        assert!(deliveries.contains_key("newer"));
    }

    #[test]
    fn workflow_configuration_is_exact_and_reviewable() {
        let allowed = BTreeSet::from(["owner/repo".to_string()]);
        let valid = BTreeMap::from([(
            "owner/repo".to_string(),
            vec![".github/workflows/ci.yml".to_string()],
        )]);
        assert!(validate_workflow_configuration(&allowed, &valid).is_ok());

        let invalid_path =
            BTreeMap::from([("owner/repo".to_string(), vec!["../ci.yml".to_string()])]);
        assert!(validate_workflow_configuration(&allowed, &invalid_path)
            .unwrap_err()
            .contains("invalid path"));

        let duplicate = BTreeMap::from([(
            "owner/repo".to_string(),
            vec![
                ".github/workflows/ci.yml".to_string(),
                ".github/workflows/ci.yml".to_string(),
            ],
        )]);
        assert!(validate_workflow_configuration(&allowed, &duplicate)
            .unwrap_err()
            .contains("duplicate path"));
    }

    #[test]
    fn github_api_origin_requires_https_except_loopback_tests() {
        assert_eq!(
            normalize_github_api_base_url("https://api.github.com/").unwrap(),
            "https://api.github.com"
        );
        assert!(normalize_github_api_base_url("http://127.0.0.1:8080").is_ok());
        assert!(normalize_github_api_base_url("http://localhost:8080/api/v3").is_ok());
        assert!(normalize_github_api_base_url("http://example.com").is_err());
        assert!(normalize_github_api_base_url("https://user:pass@example.com").is_err());
        assert!(normalize_github_api_base_url("https://example.com?token=x").is_err());
    }
}
