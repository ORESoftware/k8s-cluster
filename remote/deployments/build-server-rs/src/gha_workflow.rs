use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_yaml::{Mapping, Value};
use sha2::{Digest, Sha256};
use tokio::{
    sync::RwLock,
    time::{sleep, Instant},
};

use crate::{
    config::{env_bool, env_u64, env_usize},
    http::require_auth,
    jobs::enqueue_build,
    profiles,
    state::AppState,
    types::{BuildRequest, BuildStatus},
    util::now_ms,
    validation::validate_build_request,
};

const WORKFLOW_SCHEMA_VERSION: &str = "gha-indie-workflow.v1";
const PLAN_SCHEMA_VERSION: &str = "gha-indie-plan.v1";
const MAX_YAML_BYTES_DEFAULT: usize = 256 * 1024;
const MAX_JOBS_DEFAULT: usize = 64;
const MAX_STEPS_PER_JOB_DEFAULT: usize = 128;
const MAX_YAML_NODES_DEFAULT: usize = 16_384;
const MAX_YAML_DEPTH_DEFAULT: usize = 64;

#[derive(Clone, Debug)]
struct PlannerLimits {
    max_yaml_bytes: usize,
    max_jobs: usize,
    max_steps_per_job: usize,
    max_yaml_nodes: usize,
    max_yaml_depth: usize,
}

impl PlannerLimits {
    fn from_env() -> Self {
        Self {
            max_yaml_bytes: bounded_env_usize(
                "BUILD_SERVER_GHA_MAX_YAML_BYTES",
                MAX_YAML_BYTES_DEFAULT,
                1_024,
                1_048_576,
            ),
            max_jobs: bounded_env_usize("BUILD_SERVER_GHA_MAX_JOBS", MAX_JOBS_DEFAULT, 1, 256),
            max_steps_per_job: bounded_env_usize(
                "BUILD_SERVER_GHA_MAX_STEPS_PER_JOB",
                MAX_STEPS_PER_JOB_DEFAULT,
                1,
                512,
            ),
            max_yaml_nodes: bounded_env_usize(
                "BUILD_SERVER_GHA_MAX_YAML_NODES",
                MAX_YAML_NODES_DEFAULT,
                128,
                65_536,
            ),
            max_yaml_depth: bounded_env_usize(
                "BUILD_SERVER_GHA_MAX_YAML_DEPTH",
                MAX_YAML_DEPTH_DEFAULT,
                8,
                128,
            ),
        }
    }
}

impl Default for PlannerLimits {
    fn default() -> Self {
        Self {
            max_yaml_bytes: MAX_YAML_BYTES_DEFAULT,
            max_jobs: MAX_JOBS_DEFAULT,
            max_steps_per_job: MAX_STEPS_PER_JOB_DEFAULT,
            max_yaml_nodes: MAX_YAML_NODES_DEFAULT,
            max_yaml_depth: MAX_YAML_DEPTH_DEFAULT,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowRequest {
    #[serde(default)]
    schema_version: Option<String>,
    repository: String,
    revision: String,
    #[serde(default = "default_workflow_path")]
    workflow_path: String,
    workflow_yaml: String,
    #[serde(default)]
    request_id: Option<String>,
}

fn default_workflow_path() -> String {
    ".github/workflows/ci.yml".to_string()
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowPlan {
    schema_version: String,
    plan_id: String,
    repository: String,
    revision: String,
    workflow_path: String,
    immutable_revision: bool,
    executable: bool,
    topological_order: Vec<String>,
    jobs: Vec<JobPlan>,
    warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobPlan {
    id: String,
    needs: Vec<String>,
    runs_on: Vec<String>,
    supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
    reasons: Vec<String>,
    notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum WorkflowRunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum WorkflowJobStatus {
    Pending,
    Queued,
    Running,
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowJobRecord {
    id: String,
    profile: String,
    needs: Vec<String>,
    status: WorkflowJobStatus,
    build_id: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowRunRecord {
    id: String,
    request_id: String,
    status: WorkflowRunStatus,
    plan: WorkflowPlan,
    jobs: Vec<WorkflowJobRecord>,
    created_at_ms: u128,
    started_at_ms: Option<u128>,
    finished_at_ms: Option<u128>,
    error: Option<String>,
}

#[derive(Clone)]
struct WorkflowState {
    build: AppState,
    runs: Arc<RwLock<HashMap<String, WorkflowRunRecord>>>,
    counter: Arc<AtomicU64>,
    limits: PlannerLimits,
    execution_enabled: bool,
    poll_interval: Duration,
    run_timeout: Duration,
    max_runs: usize,
}

impl WorkflowState {
    fn from_env(build: AppState) -> Self {
        Self {
            build,
            runs: Arc::new(RwLock::new(HashMap::new())),
            counter: Arc::new(AtomicU64::new(0)),
            limits: PlannerLimits::from_env(),
            execution_enabled: env_bool("BUILD_SERVER_GHA_WORKFLOW_EXECUTION_ENABLED", false),
            poll_interval: Duration::from_millis(
                env_u64("BUILD_SERVER_GHA_POLL_MILLISECONDS", 250).clamp(10, 60_000),
            ),
            run_timeout: Duration::from_secs(
                env_u64("BUILD_SERVER_GHA_RUN_TIMEOUT_SECONDS", 3600).clamp(1, 86_400),
            ),
            max_runs: bounded_env_usize("BUILD_SERVER_GHA_MAX_RUNS", 512, 1, 10_000),
        }
    }
}

fn bounded_env_usize(name: &str, default: usize, minimum: usize, maximum: usize) -> usize {
    env_usize(name, default).clamp(minimum, maximum)
}

pub(crate) fn router(build: AppState) -> Router {
    workflow_router(WorkflowState::from_env(build))
}

fn workflow_router(state: WorkflowState) -> Router {
    Router::new()
        .route("/gha/workflows/capabilities", get(capabilities_handler))
        .route("/gha/workflows/plan", post(plan_handler))
        .route("/gha/workflows/runs", get(list_runs).post(submit_handler))
        .route("/gha/workflows/runs/:run_id", get(get_run))
        .with_state(state)
}

async fn capabilities_handler(State(state): State<WorkflowState>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&headers, &state.build) {
        return response;
    }
    let mut profile_names = profiles::names().collect::<Vec<_>>();
    profile_names.sort();
    Json(json!({
        "service": "gha-indie-worker",
        "schemaVersion": WORKFLOW_SCHEMA_VERSION,
        "planSchemaVersion": PLAN_SCHEMA_VERSION,
        "executionEnabled": state.execution_enabled,
        "limits": {
            "maxYamlBytes": state.limits.max_yaml_bytes,
            "maxJobs": state.limits.max_jobs,
            "maxStepsPerJob": state.limits.max_steps_per_job,
            "maxYamlNodes": state.limits.max_yaml_nodes,
            "maxYamlDepth": state.limits.max_yaml_depth,
            "runTimeoutSeconds": state.run_timeout.as_secs(),
        },
        "profiles": profile_names,
        "supported": [
            "static jobs and needs DAGs",
            "Linux runner labels",
            "immutable checkout/setup actions",
            "run-step classification to operator-reviewed fixed profiles",
            "deterministic sequential execution of dependency order",
            "immutable repository revisions",
        ],
        "rejected": [
            "caller-selected shell forwarding",
            "secret or expression contexts",
            "matrices, reusable workflows, services, and job containers",
            "job or step environments",
            "working-directory, custom shell, conditions, and continue-on-error",
            "mutable action refs and mutable repository refs",
        ],
    }))
    .into_response()
}

async fn plan_handler(
    State(state): State<WorkflowState>,
    headers: HeaderMap,
    Json(request): Json<WorkflowRequest>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state.build) {
        return response;
    }
    match build_plan(&request, &state.limits) {
        Ok(plan) => (StatusCode::OK, Json(plan)).into_response(),
        Err(errors) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid workflow", "errors": errors })),
        )
            .into_response(),
    }
}

async fn submit_handler(
    State(state): State<WorkflowState>,
    headers: HeaderMap,
    Json(request): Json<WorkflowRequest>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state.build) {
        return response;
    }
    if !state.execution_enabled {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "GHA workflow execution is disabled by BUILD_SERVER_GHA_WORKFLOW_EXECUTION_ENABLED=false"
            })),
        )
            .into_response();
    }

    let plan = match build_plan(&request, &state.limits) {
        Ok(plan) => plan,
        Err(errors) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid workflow", "errors": errors })),
            )
                .into_response();
        }
    };
    if !plan.executable {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error": "workflow is not executable by the independent worker",
                "plan": plan,
            })),
        )
            .into_response();
    }

    for job in &plan.jobs {
        let request = build_request_for_job(&plan, job);
        if let Err(error) = validate_build_request(&state.build.config, &request) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "workflow job is outside the build-server execution policy",
                    "jobId": job.id,
                    "details": error,
                })),
            )
                .into_response();
        }
    }

    let request_id = request
        .request_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&plan.plan_id)
        .to_string();

    let counter = state.counter.fetch_add(1, Ordering::Relaxed) + 1;
    let run_id = format!("gha-{}-{counter}", now_ms());
    let record = WorkflowRunRecord {
        id: run_id.clone(),
        request_id: request_id.clone(),
        status: WorkflowRunStatus::Queued,
        jobs: plan
            .jobs
            .iter()
            .map(|job| WorkflowJobRecord {
                id: job.id.clone(),
                profile: job
                    .profile
                    .clone()
                    .expect("executable plan job must have a profile"),
                needs: job.needs.clone(),
                status: WorkflowJobStatus::Pending,
                build_id: None,
                error: None,
            })
            .collect(),
        plan,
        created_at_ms: now_ms(),
        started_at_ms: None,
        finished_at_ms: None,
        error: None,
    };
    {
        let mut runs = state.runs.write().await;
        if let Some(existing) = runs
            .values()
            .find(|run| run.request_id == request_id)
            .cloned()
        {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "workflow request was already accepted",
                    "run": existing,
                })),
            )
                .into_response();
        }
        prune_terminal_runs(&mut runs, state.max_runs.saturating_sub(1));
        if runs.len() >= state.max_runs {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": "workflow run retention is full; wait for an active run to finish"
                })),
            )
                .into_response();
        }
        runs.insert(run_id.clone(), record.clone());
    }

    let task_state = state.clone();
    let task_id = run_id.clone();
    tokio::spawn(async move {
        execute_workflow(task_state, task_id).await;
    });

    (StatusCode::ACCEPTED, Json(record)).into_response()
}

async fn list_runs(State(state): State<WorkflowState>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&headers, &state.build) {
        return response;
    }
    let mut runs = state
        .runs
        .read()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    runs.sort_by_key(|run| std::cmp::Reverse(run.created_at_ms));
    Json(runs).into_response()
}

async fn get_run(
    State(state): State<WorkflowState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state.build) {
        return response;
    }
    match state.runs.read().await.get(&run_id).cloned() {
        Some(run) => Json(run).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "workflow run not found" })),
        )
            .into_response(),
    }
}

async fn execute_workflow(state: WorkflowState, run_id: String) {
    mutate_run(&state, &run_id, |run| {
        run.status = WorkflowRunStatus::Running;
        run.started_at_ms = Some(now_ms());
    })
    .await;

    let Some(plan) = state
        .runs
        .read()
        .await
        .get(&run_id)
        .map(|run| run.plan.clone())
    else {
        return;
    };

    let deadline = Instant::now() + state.run_timeout;
    let mut outcomes = HashMap::<String, WorkflowJobStatus>::new();

    for job_id in &plan.topological_order {
        let Some(job) = plan.jobs.iter().find(|job| &job.id == job_id).cloned() else {
            fail_workflow(&state, &run_id, "planned job disappeared before execution").await;
            return;
        };

        if Instant::now() >= deadline {
            let error = format!(
                "workflow exceeded its {} second execution deadline before job submission",
                state.run_timeout.as_secs()
            );
            mutate_job(&state, &run_id, &job.id, |record| {
                record.status = WorkflowJobStatus::Failed;
                record.error = Some(error.clone());
            })
            .await;
            outcomes.insert(job.id.clone(), WorkflowJobStatus::Failed);
            continue;
        }

        let dependency_failed = job.needs.iter().any(|dependency| {
            !matches!(outcomes.get(dependency), Some(WorkflowJobStatus::Succeeded))
        });
        if dependency_failed {
            mutate_job(&state, &run_id, &job.id, |record| {
                record.status = WorkflowJobStatus::Skipped;
                record.error = Some("one or more required jobs did not succeed".to_string());
            })
            .await;
            outcomes.insert(job.id.clone(), WorkflowJobStatus::Skipped);
            continue;
        }

        let build_request = build_request_for_job(&plan, &job);
        let build = match enqueue_build(&state.build, build_request, "gha-yaml").await {
            Ok(build) => build,
            Err((_, error)) => {
                mutate_job(&state, &run_id, &job.id, |record| {
                    record.status = WorkflowJobStatus::Failed;
                    record.error = Some(error.clone());
                })
                .await;
                outcomes.insert(job.id.clone(), WorkflowJobStatus::Failed);
                continue;
            }
        };
        mutate_job(&state, &run_id, &job.id, |record| {
            record.status = WorkflowJobStatus::Queued;
            record.build_id = Some(build.id.clone());
        })
        .await;

        let outcome = loop {
            if Instant::now() >= deadline {
                let error = format!(
                    "workflow exceeded its {} second execution deadline",
                    state.run_timeout.as_secs()
                );
                mutate_job(&state, &run_id, &job.id, |record| {
                    record.status = WorkflowJobStatus::Failed;
                    record.error = Some(error.clone());
                })
                .await;
                break WorkflowJobStatus::Failed;
            }

            let build_snapshot = state.build.jobs.read().await.get(&build.id).cloned();
            match build_snapshot {
                Some(snapshot) => match snapshot.status {
                    BuildStatus::Queued => {
                        mutate_job(&state, &run_id, &job.id, |record| {
                            record.status = WorkflowJobStatus::Queued;
                        })
                        .await;
                    }
                    BuildStatus::Running => {
                        mutate_job(&state, &run_id, &job.id, |record| {
                            record.status = WorkflowJobStatus::Running;
                        })
                        .await;
                    }
                    BuildStatus::Succeeded => {
                        mutate_job(&state, &run_id, &job.id, |record| {
                            record.status = WorkflowJobStatus::Succeeded;
                            record.error = None;
                        })
                        .await;
                        break WorkflowJobStatus::Succeeded;
                    }
                    BuildStatus::Failed => {
                        let error = snapshot
                            .error
                            .unwrap_or_else(|| "build-server profile failed".to_string());
                        mutate_job(&state, &run_id, &job.id, |record| {
                            record.status = WorkflowJobStatus::Failed;
                            record.error = Some(error.clone());
                        })
                        .await;
                        break WorkflowJobStatus::Failed;
                    }
                },
                None => {
                    mutate_job(&state, &run_id, &job.id, |record| {
                        record.status = WorkflowJobStatus::Failed;
                        record.error =
                            Some("build record disappeared before completion".to_string());
                    })
                    .await;
                    break WorkflowJobStatus::Failed;
                }
            }
            sleep(state.poll_interval).await;
        };
        outcomes.insert(job.id.clone(), outcome);
    }

    let failed = outcomes
        .values()
        .any(|status| !matches!(status, WorkflowJobStatus::Succeeded));
    mutate_run(&state, &run_id, |run| {
        run.status = if failed {
            WorkflowRunStatus::Failed
        } else {
            WorkflowRunStatus::Succeeded
        };
        run.finished_at_ms = Some(now_ms());
        if failed {
            run.error = Some("one or more workflow jobs failed or were skipped".to_string());
        }
    })
    .await;
}

async fn fail_workflow(state: &WorkflowState, run_id: &str, error: &str) {
    mutate_run(state, run_id, |run| {
        run.status = WorkflowRunStatus::Failed;
        run.finished_at_ms = Some(now_ms());
        run.error = Some(error.to_string());
    })
    .await;
}

async fn mutate_run<F>(state: &WorkflowState, run_id: &str, mutate: F)
where
    F: FnOnce(&mut WorkflowRunRecord),
{
    if let Some(run) = state.runs.write().await.get_mut(run_id) {
        mutate(run);
    }
}

async fn mutate_job<F>(state: &WorkflowState, run_id: &str, job_id: &str, mutate: F)
where
    F: FnOnce(&mut WorkflowJobRecord),
{
    if let Some(run) = state.runs.write().await.get_mut(run_id) {
        if let Some(job) = run.jobs.iter_mut().find(|job| job.id == job_id) {
            mutate(job);
        }
    }
}

fn prune_terminal_runs(runs: &mut HashMap<String, WorkflowRunRecord>, target_len: usize) {
    if runs.len() <= target_len {
        return;
    }
    let mut candidates = runs
        .values()
        .filter(|run| {
            matches!(
                run.status,
                WorkflowRunStatus::Succeeded | WorkflowRunStatus::Failed
            )
        })
        .map(|run| (run.created_at_ms, run.id.clone()))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(created_at_ms, _)| *created_at_ms);
    let remove_count = runs.len().saturating_sub(target_len);
    for (_, id) in candidates.into_iter().take(remove_count) {
        runs.remove(&id);
    }
}

fn build_request_for_job(plan: &WorkflowPlan, job: &JobPlan) -> BuildRequest {
    BuildRequest {
        schema_version: Some("build-server.v1".to_string()),
        job_kind: Some("run-profile".to_string()),
        repo_url: format!("https://github.com/{}.git", plan.repository),
        git_ref: Some(plan.revision.clone()),
        image: String::new(),
        profile: job.profile.clone(),
        context_dir: Some(".".to_string()),
        dockerfile: None,
        build_args: None,
        push: Some(false),
        deploy: None,
        executor: Some("local".to_string()),
        request_id: Some(format!("gha:{}:{}", plan.plan_id, job.id)),
    }
}

fn build_plan(
    request: &WorkflowRequest,
    limits: &PlannerLimits,
) -> Result<WorkflowPlan, Vec<String>> {
    let mut errors = Vec::new();
    if let Some(schema_version) = request.schema_version.as_deref() {
        if schema_version != WORKFLOW_SCHEMA_VERSION {
            errors.push(format!(
                "schemaVersion must be {WORKFLOW_SCHEMA_VERSION} when supplied"
            ));
        }
    }
    if !valid_repository(&request.repository) {
        errors.push(
            "repository must be an owner/name identifier using GitHub-safe characters".to_string(),
        );
    }
    if !valid_workflow_path(&request.workflow_path) {
        errors.push(
            "workflowPath must stay under .github/workflows and end in .yml or .yaml".to_string(),
        );
    }
    if request
        .request_id
        .as_deref()
        .is_some_and(|request_id| !valid_request_id(request_id))
    {
        errors.push(
            "requestId must be 1-128 printable GitHub-safe characters when supplied".to_string(),
        );
    }
    if request.workflow_yaml.len() > limits.max_yaml_bytes {
        errors.push(format!(
            "workflowYaml exceeds the {} byte limit",
            limits.max_yaml_bytes
        ));
    }
    if request.workflow_yaml.as_bytes().contains(&0) {
        errors.push("workflowYaml must not contain NUL bytes".to_string());
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let workflow: Value = serde_yaml::from_str(&request.workflow_yaml)
        .map_err(|error| vec![format!("workflowYaml is not valid YAML: {error}")])?;
    validate_yaml_shape(&workflow, limits)?;
    let root = workflow
        .as_mapping()
        .ok_or_else(|| vec!["workflow document must be a YAML mapping".to_string()])?;
    let jobs = mapping_get(root, "jobs")
        .and_then(Value::as_mapping)
        .ok_or_else(|| vec!["workflow.jobs must be a mapping".to_string()])?;

    if jobs.is_empty() {
        errors.push("workflow.jobs must contain at least one job".to_string());
    }
    if jobs.len() > limits.max_jobs {
        errors.push(format!(
            "workflow has {} jobs; maximum is {}",
            jobs.len(),
            limits.max_jobs
        ));
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut workflow_reasons = Vec::new();
    for key in root.keys() {
        match key.as_str() {
            Some("name" | "run-name" | "on" | "jobs") => {}
            Some(key) => workflow_reasons.push(format!(
                "workflow-level {key} is unsupported by the independent worker"
            )),
            None => errors.push("every workflow-level key must be a string".to_string()),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let job_ids = jobs
        .keys()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    if job_ids.len() != jobs.len() {
        return Err(vec!["every workflow job ID must be a string".to_string()]);
    }

    let mut plans = Vec::with_capacity(jobs.len());
    for (job_key, job_value) in jobs {
        let id = job_key
            .as_str()
            .expect("validated string job ID")
            .to_string();
        if !valid_job_id(&id) {
            errors.push(format!(
                "jobs.{id}: job ID must use letters, numbers, '_', or '-' and be at most 100 characters"
            ));
            continue;
        }
        let Some(job) = job_value.as_mapping() else {
            errors.push(format!("jobs.{id}: job must be a mapping"));
            continue;
        };
        match compile_job(&id, job, limits) {
            Ok(mut plan) => {
                plan.reasons.extend(workflow_reasons.clone());
                if !plan.reasons.is_empty() {
                    plan.supported = false;
                    plan.profile = None;
                }
                plans.push(plan);
            }
            Err(mut job_errors) => errors.append(&mut job_errors),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let topological_order = validate_dependencies(&plans, &job_ids)?;
    let immutable_revision = is_full_commit_sha(&request.revision);
    let mut warnings = Vec::new();
    if !immutable_revision {
        warnings.push(
            "revision is not an exact 40-hex commit SHA; planning is allowed but execution is refused"
                .to_string(),
        );
    }
    let executable = immutable_revision && plans.iter().all(|job| job.supported);

    Ok(WorkflowPlan {
        schema_version: PLAN_SCHEMA_VERSION.to_string(),
        plan_id: plan_id(request),
        repository: request.repository.clone(),
        revision: request.revision.clone(),
        workflow_path: request.workflow_path.clone(),
        immutable_revision,
        executable,
        topological_order,
        jobs: plans,
        warnings,
    })
}

fn compile_job(id: &str, job: &Mapping, limits: &PlannerLimits) -> Result<JobPlan, Vec<String>> {
    let mut errors = Vec::new();
    let needs = parse_string_or_sequence(
        mapping_get(job, "needs"),
        &format!("jobs.{id}.needs"),
        &mut errors,
    );
    let runs_on = parse_string_or_sequence(
        mapping_get(job, "runs-on"),
        &format!("jobs.{id}.runs-on"),
        &mut errors,
    );
    if runs_on.is_empty() {
        errors.push(format!(
            "jobs.{id}.runs-on: at least one runner label is required"
        ));
    }

    let mut reasons = Vec::new();
    let mut notes = Vec::new();
    let mut combined = String::new();
    let mut saw_run = false;

    for key in job.keys() {
        match key.as_str() {
            Some("name" | "needs" | "runs-on" | "steps") => {}
            Some(key) => reasons.push(format!(
                "job-level {key} is unsupported by the independent worker"
            )),
            None => errors.push(format!("jobs.{id}: every job key must be a string")),
        }
    }
    let runner_text = runs_on.join(" ").to_ascii_lowercase();
    if runner_text.contains("macos") || runner_text.contains("windows") {
        reasons.push("non-Linux native execution is unavailable".to_string());
    }
    if runs_on.iter().any(|label| label.contains("${{")) {
        reasons.push("expressions in runs-on are unsupported".to_string());
    }
    for label in &runs_on {
        if !valid_runner_label(label) {
            reasons.push(format!(
                "runner label {label:?} is not a bounded static label"
            ));
        }
    }

    let Some(steps) = mapping_get(job, "steps").and_then(Value::as_sequence) else {
        errors.push(format!("jobs.{id}.steps must be a sequence"));
        return Err(errors);
    };
    if steps.is_empty() {
        errors.push(format!("jobs.{id}.steps must contain at least one step"));
    }
    if steps.len() > limits.max_steps_per_job {
        errors.push(format!(
            "jobs.{id} has {} steps; maximum is {}",
            steps.len(),
            limits.max_steps_per_job
        ));
    }

    for (index, step_value) in steps.iter().enumerate() {
        let path = format!("jobs.{id}.steps[{index}]");
        let Some(step) = step_value.as_mapping() else {
            errors.push(format!("{path}: step must be a mapping"));
            continue;
        };
        for key in step.keys() {
            match key.as_str() {
                Some("name" | "id" | "run" | "uses" | "with") => {}
                Some(key) => reasons.push(format!(
                    "{path}: {key} is unsupported by the fixed-profile worker"
                )),
                None => errors.push(format!("{path}: every step key must be a string")),
            }
        }

        let run = mapping_get(step, "run").and_then(Value::as_str);
        let action = mapping_get(step, "uses").and_then(Value::as_str);
        match (run, action) {
            (Some(_), Some(_)) => reasons.push(format!(
                "{path}: step must contain exactly one of run or uses"
            )),
            (None, None) => errors.push(format!(
                "{path}: step must contain exactly one of run or uses"
            )),
            _ => {}
        }

        if let Some(run) = run {
            saw_run = true;
            combined.push_str(run);
            combined.push('\n');
            if run.contains("${{") {
                reasons.push(format!(
                    "{path}: expressions inside run commands are unsupported"
                ));
            }
            if mapping_get(step, "with").is_some() {
                reasons.push(format!("{path}: with is invalid for run steps"));
            }
        }

        if let Some(action) = action {
            combined.push_str(action);
            combined.push('\n');
            if !known_setup_action(action) {
                reasons.push(format!(
                    "{path}: marketplace action {action:?} has no independent-worker equivalence"
                ));
            } else if !immutable_action_ref(action) {
                reasons.push(format!(
                    "{path}: setup action {action:?} must use an exact 40-hex commit SHA"
                ));
            }
            validate_setup_inputs(
                action,
                mapping_get(step, "with"),
                &path,
                &mut reasons,
                &mut notes,
            );
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    if !saw_run {
        reasons.push("job has no run step to classify into a fixed profile".to_string());
    }

    let profile = classify_profile(&combined, &mut reasons);
    if let Some(profile_name) = profile.as_deref() {
        if profiles::find(profile_name).is_none() {
            reasons.push(format!(
                "fixed profile {profile_name:?} is not installed in this worker"
            ));
        }
    }
    if profile.is_none() && reasons.is_empty() {
        reasons.push("no fixed build-server profile matches this job".to_string());
    }
    let supported = reasons.is_empty() && profile.is_some();

    Ok(JobPlan {
        id: id.to_string(),
        needs,
        runs_on,
        supported,
        profile: if supported { profile } else { None },
        reasons,
        notes,
    })
}

fn classify_profile(text: &str, reasons: &mut Vec<String>) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    if lower.contains("flutter") {
        if lower.contains("playwright") || lower.contains("puppeteer") {
            if lower.contains("build web") {
                return Some("flutter-web-e2e".to_string());
            }
            reasons.push(
                "Flutter browser jobs must include a Flutter web build for the fixed e2e profile"
                    .to_string(),
            );
            return None;
        }
        if lower.contains("build apk") || lower.contains("build appbundle") {
            return Some("flutter-android-debug".to_string());
        }
        if lower.contains("build web") {
            return Some("flutter-web-release".to_string());
        }
        if lower.contains("build linux") && lower.contains("main_desktop.dart") {
            return Some("flutter-linux-desktop-entrypoint".to_string());
        }
        if lower.contains("build linux") {
            return Some("flutter-linux-release".to_string());
        }
        return Some("flutter-verify".to_string());
    }
    if lower.contains("playwright") {
        return Some("playwright".to_string());
    }
    if lower.contains("puppeteer") {
        return Some("puppeteer".to_string());
    }

    let rust =
        lower.contains("cargo ") || lower.contains("rust-toolchain") || lower.contains("rustfmt");
    let python = lower.contains("pytest")
        || lower.contains("python -m")
        || lower.contains("setup-python")
        || lower.contains("pip install");
    let node = lower.contains("npm ")
        || lower.contains("pnpm ")
        || lower.contains("yarn ")
        || lower.contains("setup-node")
        || lower.contains("node --test");
    let candidate_count = usize::from(rust) + usize::from(python) + usize::from(node);
    if candidate_count > 1 {
        reasons.push(
            "job mixes multiple language toolchains and cannot map to one fixed profile"
                .to_string(),
        );
        return None;
    }
    if rust {
        Some("rust-verify".to_string())
    } else if python {
        Some("python-verify".to_string())
    } else if node {
        Some("node-verify".to_string())
    } else {
        None
    }
}

fn validate_dependencies(
    jobs: &[JobPlan],
    job_ids: &BTreeSet<String>,
) -> Result<Vec<String>, Vec<String>> {
    let mut errors = Vec::new();
    let mut indegree = BTreeMap::<String, usize>::new();
    let mut children = BTreeMap::<String, Vec<String>>::new();
    for job in jobs {
        indegree.insert(job.id.clone(), job.needs.len());
        for dependency in &job.needs {
            if dependency == &job.id {
                errors.push(format!(
                    "jobs.{}.needs: job cannot depend on itself",
                    job.id
                ));
            } else if !job_ids.contains(dependency) {
                errors.push(format!(
                    "jobs.{}.needs: unknown dependency {dependency:?}",
                    job.id
                ));
            } else {
                children
                    .entry(dependency.clone())
                    .or_default()
                    .push(job.id.clone());
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut ready = indegree
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(id.clone()))
        .collect::<VecDeque<_>>();
    let mut ordered = Vec::with_capacity(jobs.len());
    while let Some(id) = ready.pop_front() {
        ordered.push(id.clone());
        if let Some(next_jobs) = children.get(&id) {
            let mut next_jobs = next_jobs.clone();
            next_jobs.sort();
            for child in next_jobs {
                let count = indegree
                    .get_mut(&child)
                    .expect("validated dependency target exists");
                *count -= 1;
                if *count == 0 {
                    ready.push_back(child);
                }
            }
        }
    }
    if ordered.len() != jobs.len() {
        return Err(vec![
            "workflow job dependency graph contains a cycle".to_string()
        ]);
    }
    Ok(ordered)
}

fn validate_yaml_shape(value: &Value, limits: &PlannerLimits) -> Result<(), Vec<String>> {
    fn walk(
        value: &Value,
        depth: usize,
        nodes: &mut usize,
        limits: &PlannerLimits,
    ) -> Result<(), String> {
        *nodes += 1;
        if *nodes > limits.max_yaml_nodes {
            return Err(format!(
                "workflowYaml exceeds the {} node limit",
                limits.max_yaml_nodes
            ));
        }
        if depth > limits.max_yaml_depth {
            return Err(format!(
                "workflowYaml exceeds the {} level nesting limit",
                limits.max_yaml_depth
            ));
        }
        match value {
            Value::Sequence(values) => {
                for value in values {
                    walk(value, depth + 1, nodes, limits)?;
                }
            }
            Value::Mapping(mapping) => {
                for (key, value) in mapping {
                    walk(key, depth + 1, nodes, limits)?;
                    walk(value, depth + 1, nodes, limits)?;
                }
            }
            Value::Tagged(_) => {
                return Err("YAML tags are unsupported".to_string());
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
        Ok(())
    }

    let mut nodes = 0;
    walk(value, 0, &mut nodes, limits).map_err(|error| vec![error])
}

fn parse_string_or_sequence(
    value: Option<&Value>,
    path: &str,
    errors: &mut Vec<String>,
) -> Vec<String> {
    match value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(value)) => vec![value.clone()],
        Some(Value::Sequence(values)) => {
            let mut result = Vec::with_capacity(values.len());
            for item in values {
                if let Some(value) = item.as_str() {
                    result.push(value.to_string());
                } else {
                    errors.push(format!("{path}: every item must be a string"));
                }
            }
            result.sort();
            result.dedup();
            result
        }
        Some(_) => {
            errors.push(format!("{path}: expected a string or string sequence"));
            Vec::new()
        }
    }
}

fn mapping_get<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    mapping.get(Value::String(key.to_string()))
}

fn contains_expression(value: Option<&Value>) -> bool {
    value.is_some_and(|value| compact_yaml(value).contains("${{"))
}

fn contains_secret_expression(value: Option<&Value>) -> bool {
    value.is_some_and(|value| {
        let compact = compact_yaml(value)
            .to_ascii_lowercase()
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        compact.contains("${{secrets")
            || compact.contains("tojson(secrets)")
            || compact.contains("fromjson(secrets)")
            || compact.contains("github.token")
            || compact.contains("github['token']")
            || compact.contains("github[\"token\"]")
            || compact.contains("actions_id_token_request")
    })
}

fn split_action_ref(action: &str) -> Option<(&str, &str)> {
    let (name, reference) = action.split_once('@')?;
    if name.is_empty() || reference.is_empty() || reference.contains('@') {
        return None;
    }
    Some((name, reference))
}

fn known_setup_action(action: &str) -> bool {
    let Some((name, _)) = split_action_ref(action) else {
        return false;
    };
    matches!(
        name.to_ascii_lowercase().as_str(),
        "actions/checkout"
            | "actions/setup-node"
            | "actions/setup-python"
            | "actions/setup-java"
            | "dtolnay/rust-toolchain"
            | "pnpm/action-setup"
            | "subosito/flutter-action"
    )
}

fn immutable_action_ref(action: &str) -> bool {
    split_action_ref(action).is_some_and(|(_, reference)| is_full_commit_sha(reference))
}

fn validate_setup_inputs(
    action: &str,
    value: Option<&Value>,
    path: &str,
    reasons: &mut Vec<String>,
    notes: &mut Vec<String>,
) {
    let Some(value) = value else {
        return;
    };
    if contains_secret_expression(Some(value)) {
        reasons.push(format!(
            "{path}: secret-bearing setup-action inputs are unsupported"
        ));
        return;
    }
    if contains_expression(Some(value)) {
        reasons.push(format!(
            "{path}: expressions in setup-action inputs are unsupported"
        ));
        return;
    }
    let Some(inputs) = value.as_mapping() else {
        reasons.push(format!("{path}: setup-action inputs must be a mapping"));
        return;
    };
    let action_name = split_action_ref(action)
        .map(|(name, _)| name.to_ascii_lowercase())
        .unwrap_or_default();
    for (key, value) in inputs {
        let Some(key) = key.as_str() else {
            reasons.push(format!(
                "{path}: every setup-action input key must be a string"
            ));
            continue;
        };
        if !is_scalar_literal(value) || compact_yaml(value).len() > 1_024 {
            reasons.push(format!(
                "{path}: setup-action input {key:?} must be a bounded scalar literal"
            ));
            continue;
        }
        if action_name == "actions/checkout" {
            match key {
                "persist-credentials" if scalar_is_false(value) => {}
                "fetch-depth" | "show-progress" => {}
                "fetch-tags" if scalar_is_false(value) => {}
                _ => reasons.push(format!(
                    "{path}: checkout input {key:?} changes repository or credential semantics and is unsupported"
                )),
            }
        }
    }
    if reasons.is_empty() {
        notes.push(format!(
            "{path}: setup-action inputs are advisory; the fixed profile pins the actual toolchain"
        ));
    }
}

fn is_scalar_literal(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

fn scalar_is_false(value: &Value) -> bool {
    matches!(value, Value::Bool(false))
        || value
            .as_str()
            .is_some_and(|value| value.eq_ignore_ascii_case("false"))
}

fn compact_yaml(value: &Value) -> String {
    serde_yaml::to_string(value)
        .unwrap_or_else(|_| "<unprintable>".to_string())
        .replace('\n', " ")
}

fn valid_repository(value: &str) -> bool {
    let mut parts = value.split('/');
    let Some(owner) = parts.next() else {
        return false;
    };
    let Some(repo) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && valid_github_component(owner)
        && valid_github_component(repo)
        && owner.len() <= 100
        && repo.len() <= 100
}

fn valid_github_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_workflow_path(value: &str) -> bool {
    value.starts_with(".github/workflows/")
        && (value.ends_with(".yml") || value.ends_with(".yaml"))
        && !value.contains("..")
        && !value.contains('\\')
        && value.len() <= 256
}

fn valid_job_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_runner_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/'))
}

fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

fn is_full_commit_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn plan_id(request: &WorkflowRequest) -> String {
    let mut hasher = Sha256::new();
    for part in [
        request.repository.as_bytes(),
        request.revision.as_bytes(),
        request.workflow_path.as_bytes(),
        request.workflow_yaml.as_bytes(),
    ] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tokio::sync::Semaphore;
    use tower::ServiceExt;

    use crate::{config::Config, state::Counters};

    fn request(yaml: &str) -> WorkflowRequest {
        WorkflowRequest {
            schema_version: Some(WORKFLOW_SCHEMA_VERSION.to_string()),
            repository: "ORESoftware/k8s-cluster".to_string(),
            revision: "0123456789abcdef0123456789abcdef01234567".to_string(),
            workflow_path: ".github/workflows/ci.yml".to_string(),
            workflow_yaml: yaml.to_string(),
            request_id: None,
        }
    }

    fn test_config() -> Config {
        Config {
            work_root: std::env::temp_dir()
                .join(format!("gha-indie-worker-test-{}", uuid::Uuid::new_v4())),
            git_bin: "git".to_string(),
            git_http_auth_header: None,
            nerdctl_bin: "nerdctl".to_string(),
            kubectl_bin: "kubectl".to_string(),
            tar_bin: "tar".to_string(),
            containerd_namespace: "gha-indie-test".to_string(),
            allowed_repo_prefixes: vec!["https://github.com/ORESoftware/".to_string()],
            allowed_image_prefixes: vec![
                "710156900967.dkr.ecr.us-east-1.amazonaws.com/".to_string()
            ],
            allowed_namespaces: HashSet::from(["default".to_string()]),
            allowed_profiles: HashSet::from([
                "rust-verify".to_string(),
                "node-verify".to_string(),
                "python-verify".to_string(),
            ]),
            allowed_profile_repo_prefixes: vec!["https://github.com/ORESoftware/".to_string()],
            profile_cpus: "2".to_string(),
            profile_memory: "2g".to_string(),
            profile_pids_limit: "512".to_string(),
            deploy_enabled: false,
            push_enabled: false,
            ecr_login_enabled: false,
            aws_region: "us-east-1".to_string(),
            job_timeout: Duration::from_secs(60),
            job_deadline: Duration::from_secs(120),
            max_log_bytes: 1_000_000,
            max_jobs: 100,
            max_queued: 16,
            keep_workdirs: false,
            server_auth_secret: Some("gha-test-auth".to_string()),
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
            github_webhook_secret: None,
            registry_webhook_secret: None,
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

    fn test_workflow_state(execution_enabled: bool) -> WorkflowState {
        let build = AppState {
            config: Arc::new(test_config()),
            http: reqwest::Client::new(),
            jobs: Arc::new(RwLock::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(1)),
            counters: Arc::new(Counters::default()),
            db: None,
            nats: None,
            holder: "gha-indie-worker/http-test".to_string(),
            recent_request_ids: Arc::new(RwLock::new(HashSet::new())),
        };
        WorkflowState {
            build,
            runs: Arc::new(RwLock::new(HashMap::new())),
            counter: Arc::new(AtomicU64::new(0)),
            limits: PlannerLimits::default(),
            execution_enabled,
            poll_interval: Duration::from_millis(10),
            run_timeout: Duration::from_secs(1),
            max_runs: 8,
        }
    }

    async fn send(router: Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = router.oneshot(request).await.expect("router response");
        let status = response.status();
        let body = to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .expect("response body");
        let value = serde_json::from_slice(&body).unwrap_or_else(|_| {
            json!({
                "raw": String::from_utf8_lossy(&body),
            })
        });
        (status, value)
    }

    fn post_json(path: &str, auth: Option<&str>, value: serde_json::Value) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json");
        if let Some(auth) = auth {
            builder = builder.header("x-server-auth", auth);
        }
        builder
            .body(Body::from(value.to_string()))
            .expect("request")
    }

    fn valid_http_request() -> serde_json::Value {
        json!({
            "schemaVersion": WORKFLOW_SCHEMA_VERSION,
            "repository": "ORESoftware/k8s-cluster",
            "revision": "0123456789abcdef0123456789abcdef01234567",
            "workflowPath": ".github/workflows/ci.yml",
            "workflowYaml": "jobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test\n"
        })
    }

    #[tokio::test]
    async fn http_plan_requires_auth_and_returns_executable_plan() {
        let router = workflow_router(test_workflow_state(false));
        let (status, _) = send(
            router.clone(),
            post_json("/gha/workflows/plan", None, valid_http_request()),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, body) = send(
            router,
            post_json(
                "/gha/workflows/plan",
                Some("gha-test-auth"),
                valid_http_request(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["executable"], true);
        assert_eq!(body["jobs"][0]["profile"], "rust-verify");
    }

    #[tokio::test]
    async fn http_invalid_yaml_is_bad_request_and_unsupported_yaml_is_a_plan() {
        let router = workflow_router(test_workflow_state(false));
        let mut invalid = valid_http_request();
        invalid["workflowYaml"] = json!("jobs: [");
        let (status, body) = send(
            router.clone(),
            post_json("/gha/workflows/plan", Some("gha-test-auth"), invalid),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid workflow");

        let mut unsupported = valid_http_request();
        unsupported["workflowYaml"] = json!(
            "jobs:\n  test:\n    runs-on: ubuntu-latest\n    strategy:\n      matrix: { node: [20, 22] }\n    steps:\n      - run: npm test\n"
        );
        let (status, body) = send(
            router,
            post_json("/gha/workflows/plan", Some("gha-test-auth"), unsupported),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["executable"], false);
        assert!(body["jobs"][0]["reasons"]
            .as_array()
            .is_some_and(|reasons| !reasons.is_empty()));
    }

    #[tokio::test]
    async fn http_execution_is_disabled_before_any_build_is_enqueued() {
        let state = test_workflow_state(false);
        let jobs = state.build.jobs.clone();
        let (status, body) = send(
            workflow_router(state),
            post_json(
                "/gha/workflows/runs",
                Some("gha-test-auth"),
                valid_http_request(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body["error"]
            .as_str()
            .is_some_and(|error| error.contains("execution is disabled")));
        assert!(jobs.read().await.is_empty());
    }

    #[test]
    fn plans_static_multi_job_workflow_into_fixed_profiles() {
        let plan = build_plan(
            &request(
                r#"
name: CI
on: [push]
jobs:
  rust:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@0123456789abcdef0123456789abcdef01234567
      - uses: dtolnay/rust-toolchain@0123456789abcdef0123456789abcdef01234567
      - run: cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
  node:
    needs: rust
    runs-on: [self-hosted, linux]
    steps:
      - uses: actions/checkout@0123456789abcdef0123456789abcdef01234567
      - uses: actions/setup-node@0123456789abcdef0123456789abcdef01234567
      - run: npm ci && npm test
  python:
    needs: [rust, node]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/setup-python@0123456789abcdef0123456789abcdef01234567
      - run: python -m compileall . && python -m pytest
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("valid plan");
        assert!(plan.executable);
        assert_eq!(plan.topological_order, vec!["rust", "node", "python"]);
        assert_eq!(plan.jobs[0].profile.as_deref(), Some("rust-verify"));
        assert_eq!(plan.jobs[1].profile.as_deref(), Some("node-verify"));
        assert_eq!(plan.jobs[2].profile.as_deref(), Some("python-verify"));
    }

    #[test]
    fn branch_revision_can_be_planned_but_not_executed() {
        let mut input = request(
            r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps: [{ run: "cargo test" }]
"#,
        );
        input.revision = "main".to_string();
        let plan = build_plan(&input, &PlannerLimits::default()).expect("valid plan");
        assert!(!plan.immutable_revision);
        assert!(!plan.executable);
        assert!(!plan.warnings.is_empty());
    }

    #[test]
    fn rejects_mutable_actions_secrets_and_caller_selected_environments() {
        let plan = build_plan(
            &request(
                r#"
jobs:
  unsafe:
    runs-on: ubuntu-latest
    env:
      NODE_ENV: test
    steps:
      - uses: actions/setup-node@main
        with:
          token: ${{ secrets.PROD_TOKEN }}
      - run: npm test
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("structurally valid plan");
        assert!(!plan.executable);
        let reasons = plan.jobs[0].reasons.join("\n");
        assert!(reasons.contains("job-level env"));
        assert!(reasons.contains("exact 40-hex commit SHA"));
        assert!(reasons.contains("secret-bearing"));
    }

    #[test]
    fn rejects_services_matrices_conditions_and_working_directories() {
        let plan = build_plan(
            &request(
                r#"
jobs:
  unsupported:
    runs-on: ubuntu-latest
    strategy:
      matrix: { node: [20, 22] }
    services:
      postgres: { image: postgres:17 }
    if: success()
    steps:
      - run: npm test
        working-directory: app
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("structurally valid plan");
        assert!(!plan.executable);
        let reasons = plan.jobs[0].reasons.join("\n");
        assert!(reasons.contains("job-level strategy"));
        assert!(reasons.contains("job-level services"));
        assert!(reasons.contains("job-level if"));
        assert!(reasons.contains("working-directory"));
    }

    #[test]
    fn rejects_cycles_and_unknown_dependencies() {
        let cycle = build_plan(
            &request(
                r#"
jobs:
  a:
    needs: b
    runs-on: ubuntu-latest
    steps: [{ run: "cargo test" }]
  b:
    needs: a
    runs-on: ubuntu-latest
    steps: [{ run: "cargo test" }]
"#,
            ),
            &PlannerLimits::default(),
        )
        .unwrap_err()
        .join("\n");
        assert!(cycle.contains("contains a cycle"));

        let unknown = build_plan(
            &request(
                r#"
jobs:
  a:
    needs: missing
    runs-on: ubuntu-latest
    steps: [{ run: "cargo test" }]
"#,
            ),
            &PlannerLimits::default(),
        )
        .unwrap_err()
        .join("\n");
        assert!(unknown.contains("unknown dependency"));
    }

    #[test]
    fn rejects_mixed_language_jobs_instead_of_guessing() {
        let plan = build_plan(
            &request(
                r#"
jobs:
  mixed:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test
      - run: npm test
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("structurally valid plan");
        assert!(!plan.executable);
        assert!(plan.jobs[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("multiple language toolchains")));
    }

    #[test]
    fn plan_id_is_stable_and_changes_with_yaml() {
        let first = request(
            r#"jobs: { test: { runs-on: ubuntu-latest, steps: [{ run: "cargo test" }] } }"#,
        );
        let same = first.clone();
        let mut changed = first.clone();
        changed.workflow_yaml.push('\n');
        assert_eq!(plan_id(&first), plan_id(&same));
        assert_ne!(plan_id(&first), plan_id(&changed));
    }

    #[test]
    fn build_request_is_profile_only_and_immutable() {
        let plan = build_plan(
            &request(
                r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps: [{ run: "cargo test" }]
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("valid plan");
        let request = build_request_for_job(&plan, &plan.jobs[0]);
        assert_eq!(request.job_kind.as_deref(), Some("run-profile"));
        assert_eq!(request.git_ref.as_deref(), Some(plan.revision.as_str()));
        assert_eq!(request.profile.as_deref(), Some("rust-verify"));
        assert!(request.image.is_empty());
        assert!(request.build_args.is_none());
        assert!(request.deploy.is_none());
        assert!(request
            .request_id
            .as_deref()
            .is_some_and(|value| value.starts_with("gha:")));
    }

    #[test]
    fn workflow_path_and_yaml_limits_fail_closed() {
        let mut invalid_path = request("jobs: {}");
        invalid_path.workflow_path = "../ci.yml".to_string();
        assert!(build_plan(&invalid_path, &PlannerLimits::default()).is_err());

        let limits = PlannerLimits {
            max_yaml_bytes: 4,
            ..PlannerLimits::default()
        };
        assert!(build_plan(&request("jobs: {}"), &limits).is_err());
    }

    #[test]
    fn rejects_unknown_keys_and_steps_with_both_run_and_uses() {
        let plan = build_plan(
            &request(
                r#"
unknown-root: true
jobs:
  test:
    runs-on: ubuntu-latest
    mystery-job-key: true
    steps:
      - run: cargo test
        uses: actions/checkout@0123456789abcdef0123456789abcdef01234567
        mystery-step-key: true
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("structurally valid plan");
        assert!(!plan.executable);
        let reasons = plan.jobs[0].reasons.join("\n");
        assert!(reasons.contains("workflow-level unknown-root"));
        assert!(reasons.contains("job-level mystery-job-key"));
        assert!(reasons.contains("mystery-step-key"));
        assert!(reasons.contains("exactly one of run or uses"));
    }

    #[test]
    fn checkout_inputs_cannot_change_repository_or_credentials() {
        for input in [
            "repository",
            "ref",
            "path",
            "token",
            "ssh-key",
            "submodules",
        ] {
            let yaml = format!(
                "jobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@0123456789abcdef0123456789abcdef01234567\n        with:\n          {input}: attacker/value\n      - run: cargo test\n"
            );
            let plan = build_plan(&request(&yaml), &PlannerLimits::default())
                .expect("structurally valid plan");
            assert!(!plan.executable, "checkout input {input:?} was accepted");
            assert!(plan.jobs[0]
                .reasons
                .iter()
                .any(|reason| reason.contains("checkout input")));
        }
    }

    #[test]
    fn malformed_action_refs_and_invalid_request_ids_fail_closed() {
        let plan = build_plan(
            &request(
                r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@evil@0123456789abcdef0123456789abcdef01234567
      - run: cargo test
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("structurally valid plan");
        assert!(!plan.executable);
        assert!(plan.jobs[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("marketplace action")));

        let mut invalid = request(
            r#"jobs: { test: { runs-on: ubuntu-latest, steps: [{ run: "cargo test" }] } }"#,
        );
        invalid.request_id = Some("bad request id".to_string());
        assert!(build_plan(&invalid, &PlannerLimits::default()).is_err());
    }

    #[test]
    fn retention_pruning_is_strict_and_preserves_active_runs() {
        let plan = build_plan(
            &request(
                r#"jobs: { test: { runs-on: ubuntu-latest, steps: [{ run: "cargo test" }] } }"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("valid plan");
        let make_run =
            |id: &str, status: WorkflowRunStatus, created_at_ms: u128| WorkflowRunRecord {
                id: id.to_string(),
                request_id: id.to_string(),
                status,
                jobs: Vec::new(),
                plan: plan.clone(),
                created_at_ms,
                started_at_ms: None,
                finished_at_ms: None,
                error: None,
            };
        let mut runs = HashMap::from([
            (
                "old".to_string(),
                make_run("old", WorkflowRunStatus::Succeeded, 1),
            ),
            (
                "new".to_string(),
                make_run("new", WorkflowRunStatus::Failed, 2),
            ),
            (
                "active".to_string(),
                make_run("active", WorkflowRunStatus::Running, 0),
            ),
        ]);
        prune_terminal_runs(&mut runs, 2);
        assert!(!runs.contains_key("old"));
        assert!(runs.contains_key("new"));
        assert!(runs.contains_key("active"));
    }
}
