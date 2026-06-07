mod runtime_config;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    error::Error,
    ffi::{c_int, c_void},
    net::SocketAddr,
    panic::{catch_unwind, AssertUnwindSafe},
    path::Path as FsPath,
    ptr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path as AxumPath, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use dd_nats_subject_defs::{
    DD_REMOTE_MIP_SOLVER_STREAM_NAME, DD_REMOTE_MIP_SOLVER_STREAM_SUBJECTS,
    MIP_SOLVER_CONTROL_SUBJECT, MIP_SOLVER_EVENTS_SUBJECT, MIP_SOLVER_JOBS_SUBJECT,
    MIP_SOLVER_RESULTS_SUBJECT, MIP_SOLVER_WORKERS_QUEUE_GROUP,
};
use dd_pg_defs::{
    MIP_SOLVER_EVENTS_TABLE, MIP_SOLVER_JOBS_TABLE, MIP_SOLVER_SESSIONS_TABLE,
    MIP_SOLVER_SOLVES_TABLE,
};
use dd_redis_interfaces::{
    container_pool_affinity_lock_key, CONTAINER_POOL_AFFINITY_LOCK_KEY_DEFAULT_PREFIX,
};
#[cfg(feature = "external-solver-verification")]
use des_engine::des::general as external_des_general;
use des_engine::des::general::{
    ip_mip_des::{
        solve_ipmip_with_des, BranchRule, ConcreteLpRelaxationAlgorithm, IPMIPProblem,
        IPMIPSolution, IPMIPSolveOptions, IPMIPStatus, LpRelaxationAlgorithm,
    },
    lp::{solve_lp_internal, InternalSimplexOptions, LPProblem, LPSolution, LPStatus, Sense},
};
use futures_util::StreamExt;
use libloading::Library;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

const SERVICE_NAME: &str = "dd-in-house-mip-solver-node";
const SERVICE_DESCRIPTION: &str =
    "In-house LP solver plus distributed MIP/IP branch-and-bound node with NATS JetStream master/slave execution.";
const API_DOCS_SCHEMA: &str = "dd.service-docs.v1";
const MAX_HTTP_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_VARS: usize = 10_000;
const MAX_CONSTRAINTS: usize = 50_000;
const MAX_STREAM_COMMANDS: usize = 2_000;
const MIP_SOLVER_REDIS_PREFIX: &str = "dd:mip-solver";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum NodeRole {
    Master,
    Slave,
}

impl NodeRole {
    fn from_env() -> Self {
        match env_value("MIP_SOLVER_NODE_ROLE", "master")
            .to_ascii_lowercase()
            .as_str()
        {
            "slave" | "worker" => NodeRole::Slave,
            _ => NodeRole::Master,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            NodeRole::Master => "master",
            NodeRole::Slave => "slave",
        }
    }
}

#[derive(Clone)]
struct AppState {
    role: NodeRole,
    node_id: String,
    nats: Option<async_nats::Client>,
    redis: Option<redis::Client>,
    pg: Option<PgPool>,
    coordination: CoordinationConfig,
    jobs_subject: String,
    results_subject: String,
    control_subject: String,
    events_subject: String,
    sessions: Arc<Mutex<HashMap<String, LiveSession>>>,
    problems: Arc<Mutex<HashMap<String, MipProblemSpec>>>,
    workers: Arc<Mutex<HashMap<String, WorkerNodeStatus>>>,
    solves: Arc<Mutex<HashMap<String, SolveRegistryEntry>>>,
    tasks: Arc<Mutex<HashMap<String, RuntimeTaskRecord>>>,
    cancelled_solves: Arc<Mutex<HashMap<String, CancelInfo>>>,
    metrics: Arc<Metrics>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CoordinationBackend {
    Redis,
    LiveMutex,
}

impl CoordinationBackend {
    fn as_str(self) -> &'static str {
        match self {
            CoordinationBackend::Redis => "redis",
            CoordinationBackend::LiveMutex => "live-mutex",
        }
    }
}

#[derive(Clone, Debug)]
struct LiveMutexConfig {
    base_url: String,
    auth_token: Option<String>,
    request_timeout_ms: u64,
    max_response_bytes: u64,
}

#[derive(Clone, Debug)]
struct CoordinationConfig {
    backends: Vec<CoordinationBackend>,
    redis_lock_prefix: String,
    ttl_ms: u64,
    wait_ms: u64,
    live_mutex: Option<LiveMutexConfig>,
}

#[derive(Clone, Debug)]
struct CoordinationGuard {
    key: String,
    holders: Vec<CoordinationHolder>,
}

#[derive(Clone, Debug)]
enum CoordinationHolder {
    Redis { token: String },
    LiveMutex { lock_uuid: String },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiveMutexLockResponse {
    acquired: bool,
    lock_uuid: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiveMutexUnlockResponse {
    unlocked: bool,
    error: Option<String>,
}

#[derive(Debug)]
struct HttpEndpoint {
    addr: String,
    host_header: String,
    path_prefix: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeTaskEntry {
    task_id: String,
    kind: String,
    node_id: String,
    role: NodeRole,
    status: String,
    problem_id: Option<String>,
    solve_id: Option<String>,
    request_id: Option<String>,
    job_id: Option<String>,
    job_uuid: Option<String>,
    abortable: bool,
    started_at_ms: u128,
    updated_at_ms: u128,
    finished_at_ms: Option<u128>,
}

struct RuntimeTaskRecord {
    entry: RuntimeTaskEntry,
    abort_handle: Option<tokio::task::AbortHandle>,
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    stream_events_total: AtomicU64,
    solve_requests_total: AtomicU64,
    subproblem_jobs_published_total: AtomicU64,
    subproblem_jobs_completed_total: AtomicU64,
    subproblem_jobs_redelegated_total: AtomicU64,
    subproblem_jobs_split_total: AtomicU64,
    worker_control_messages_total: AtomicU64,
    solve_cancel_requests_total: AtomicU64,
    slave_jobs_processed_total: AtomicU64,
    errors_total: AtomicU64,
}

#[derive(Clone)]
struct LiveSession {
    problem: Option<MipProblemSpec>,
    revision: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerJobStatus {
    job_id: String,
    job_uuid: Option<String>,
    solve_id: String,
    problem_id: Option<String>,
    status: String,
    started_at_ms: u128,
    last_seen_ms: u128,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerNodeStatus {
    node_id: String,
    last_command: String,
    consumer: Option<String>,
    jobs_subject: Option<String>,
    results_subject: Option<String>,
    last_job_id: Option<String>,
    last_solve_id: Option<String>,
    last_status: Option<String>,
    ready_at_ms: Option<u128>,
    last_seen_ms: u128,
    request_count: u64,
    completed_count: u64,
    active_jobs: HashMap<String, WorkerJobStatus>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobRegistryEntry {
    job_id: String,
    job_uuid: Option<String>,
    problem_id: Option<String>,
    root_job_id: String,
    retry_index: usize,
    depth: usize,
    status: String,
    worker_node: Option<String>,
    submitted_at_ms: u128,
    last_heartbeat_ms: Option<u128>,
    finished_at_ms: Option<u128>,
    error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveRegistryEntry {
    solve_id: String,
    request_id: String,
    problem_id: Option<String>,
    revision: u64,
    status: String,
    jobs_expected: usize,
    jobs_published: usize,
    jobs_completed: usize,
    jobs_redelegated: usize,
    jobs_split: usize,
    timed_out: bool,
    distributed: bool,
    started_at_ms: u128,
    updated_at_ms: u128,
    finished_at_ms: Option<u128>,
    cancel_requested: bool,
    cancel_requested_at_ms: Option<u128>,
    cancel_reason: Option<String>,
    warnings: Vec<String>,
    jobs: HashMap<String, JobRegistryEntry>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct CancelInfo {
    solve_id: String,
    request_id: Option<String>,
    problem_id: Option<String>,
    reason: String,
    requested_by: String,
    requested_at_ms: u128,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveHttpRequest {
    request_id: Option<String>,
    problem_id: Option<String>,
    problem: Option<MipProblemSpec>,
    commands: Option<Vec<Value>>,
    options: Option<SolveOptions>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredProblemSolveRequest {
    request_id: Option<String>,
    options: Option<SolveOptions>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelSolveRequest {
    reason: Option<String>,
    requested_by: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClusterSolvesQuery {
    problem: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MipProblemSpec {
    #[serde(default = "default_sense")]
    sense: String,
    c: Vec<f64>,
    #[serde(rename = "a", alias = "A")]
    a: Vec<Vec<f64>>,
    b: Vec<f64>,
    #[serde(default)]
    integer_vars: Vec<bool>,
    ub: Option<Vec<f64>>,
    var_names: Option<Vec<String>>,
    con_names: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BranchConstraint {
    coefs: Vec<f64>,
    rhs: f64,
    name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveOptions {
    max_nodes: Option<usize>,
    max_ticks: Option<usize>,
    lp_max_iters: Option<usize>,
    int_tol: Option<f64>,
    split_depth: Option<usize>,
    max_subproblems: Option<usize>,
    max_job_retries: Option<usize>,
    timeout_ms: Option<u64>,
    emit_trace: Option<bool>,
    verify_external: Option<bool>,
    external_verification_method: Option<String>,
    external_verification_tolerance: Option<f64>,
}

impl Default for SolveOptions {
    fn default() -> Self {
        SolveOptions {
            max_nodes: Some(20_000),
            max_ticks: Some(200_000),
            lp_max_iters: Some(5_000),
            int_tol: Some(1e-6),
            split_depth: Some(1),
            max_subproblems: Some(256),
            max_job_retries: Some(2),
            timeout_ms: Some(120_000),
            emit_trace: Some(false),
            verify_external: Some(false),
            external_verification_method: None,
            external_verification_tolerance: Some(1e-6),
        }
    }
}

impl SolveOptions {
    fn merged(input: Option<SolveOptions>) -> Self {
        Self::merged_with_defaults(input, Self::runtime_defaults())
    }

    fn merged_with_defaults(input: Option<SolveOptions>, defaults: SolveOptions) -> Self {
        let Some(input) = input else {
            return defaults;
        };
        SolveOptions {
            max_nodes: input.max_nodes.or(defaults.max_nodes),
            max_ticks: input.max_ticks.or(defaults.max_ticks),
            lp_max_iters: input.lp_max_iters.or(defaults.lp_max_iters),
            int_tol: input.int_tol.or(defaults.int_tol),
            split_depth: input.split_depth.or(defaults.split_depth),
            max_subproblems: input.max_subproblems.or(defaults.max_subproblems),
            max_job_retries: input.max_job_retries.or(defaults.max_job_retries),
            timeout_ms: input.timeout_ms.or(defaults.timeout_ms),
            emit_trace: input.emit_trace.or(defaults.emit_trace),
            verify_external: input.verify_external.or(defaults.verify_external),
            external_verification_method: input
                .external_verification_method
                .or(defaults.external_verification_method),
            external_verification_tolerance: input
                .external_verification_tolerance
                .or(defaults.external_verification_tolerance),
        }
    }

    fn runtime_defaults() -> Self {
        let defaults = Self::default();
        SolveOptions {
            max_nodes: Some(env_usize(
                "MIP_SOLVER_MAX_NODES",
                defaults.max_nodes.unwrap_or(20_000),
            )),
            max_ticks: Some(env_usize(
                "MIP_SOLVER_MAX_TICKS",
                defaults.max_ticks.unwrap_or(200_000),
            )),
            lp_max_iters: Some(env_usize(
                "MIP_SOLVER_LP_MAX_ITERS",
                defaults.lp_max_iters.unwrap_or(5_000),
            )),
            int_tol: Some(env_f64(
                "MIP_SOLVER_INT_TOL",
                defaults.int_tol.unwrap_or(1e-6),
            )),
            split_depth: Some(env_usize(
                "MIP_SOLVER_SPLIT_DEPTH",
                defaults.split_depth.unwrap_or(1),
            )),
            max_subproblems: Some(env_usize(
                "MIP_SOLVER_MAX_SUBPROBLEMS",
                defaults.max_subproblems.unwrap_or(256),
            )),
            max_job_retries: Some(env_usize_allow_zero(
                "MIP_SOLVER_MAX_JOB_RETRIES",
                defaults.max_job_retries.unwrap_or(2),
            )),
            timeout_ms: Some(env_u64(
                "MIP_SOLVER_TIMEOUT_MS",
                defaults.timeout_ms.unwrap_or(120_000),
            )),
            emit_trace: Some(env_bool(
                "MIP_SOLVER_EMIT_TRACE",
                defaults.emit_trace.unwrap_or(false),
            )),
            verify_external: Some(env_bool(
                "MIP_SOLVER_VERIFY_EXTERNAL",
                defaults.verify_external.unwrap_or(false),
            )),
            external_verification_method: optional_env_value(
                "MIP_SOLVER_EXTERNAL_VERIFICATION_METHOD",
            )
            .or(defaults.external_verification_method),
            external_verification_tolerance: Some(env_f64(
                "MIP_SOLVER_EXTERNAL_VERIFICATION_TOLERANCE",
                defaults.external_verification_tolerance.unwrap_or(1e-6),
            )),
        }
    }

    fn to_ipmip_options(&self) -> IPMIPSolveOptions {
        IPMIPSolveOptions {
            max_nodes: self.max_nodes,
            max_ticks: self.max_ticks,
            lp_max_iters: self.lp_max_iters,
            int_tol: self.int_tol,
            branch_rule: Some(BranchRule::MostFractional),
            lp_algorithm: Some(LpRelaxationAlgorithm::Concrete(
                ConcreteLpRelaxationAlgorithm::InternalSimplex,
            )),
            allow_external_solvers: Some(false),
            max_cut_rounds: Some(8),
            max_cuts_per_node: Some(16),
            heuristic_passes: Some(2),
            verbose: Some(false),
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubproblemJob {
    solve_id: String,
    request_id: String,
    job_id: String,
    #[serde(default = "new_uuid_string")]
    job_uuid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    problem_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    problem_stored: bool,
    revision: u64,
    depth: usize,
    master_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    problem: Option<MipProblemSpec>,
    extra_constraints: Vec<BranchConstraint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    avoid_worker_nodes: Vec<String>,
    options: SolveOptions,
    submitted_at_ms: u128,
}

impl SubproblemJob {
    fn problem(&self) -> Result<&MipProblemSpec, String> {
        self.problem.as_ref().ok_or_else(|| {
            format!(
                "subproblem {} has no embedded problem payload and could not be hydrated",
                self.job_id
            )
        })
    }

    fn without_problem_payload(mut self) -> Self {
        self.problem = None;
        self.problem_stored = true;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProblemStoreStatus {
    Created,
    Existing,
}

impl ProblemStoreStatus {
    fn as_str(self) -> &'static str {
        match self {
            ProblemStoreStatus::Created => "created",
            ProblemStoreStatus::Existing => "existing",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubproblemResult {
    solve_id: String,
    request_id: String,
    job_id: String,
    #[serde(default = "new_uuid_string")]
    job_uuid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    problem_id: Option<String>,
    revision: u64,
    worker_node: String,
    ok: bool,
    status: String,
    z: Option<f64>,
    x: Vec<f64>,
    best_bound: Option<f64>,
    gap: Option<f64>,
    lp: Option<LpSolveReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    child_jobs: Vec<SubproblemJob>,
    nodes_explored: usize,
    lp_solves: usize,
    elapsed_ms: f64,
    #[serde(default)]
    accelerator: AcceleratorReport,
    error: Option<String>,
    finished_at_ms: u128,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LpSolveReport {
    primal: LpPrimalReport,
    dual: LpDualReport,
    basis: LpBasisReport,
    iterations: Option<usize>,
    solver: String,
    elapsed_ms: f64,
    message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LpPrimalReport {
    objective: Option<f64>,
    x: Vec<f64>,
    var_names: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LpDualReport {
    inequality: Option<Vec<f64>>,
    equality: Option<Vec<f64>>,
    reduced_costs: Option<Vec<f64>>,
    row_names: Option<Vec<String>>,
    var_names: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LpBasisReport {
    variables: Option<Vec<String>>,
    rows: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcceleratorReport {
    mode: String,
    backend: String,
    gpu_available: bool,
    used_gpu: bool,
    used_cpu_parallel: bool,
    notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveResponse {
    ok: bool,
    solve_id: String,
    request_id: String,
    problem_id: Option<String>,
    status: String,
    revision: u64,
    z: Option<f64>,
    x: Vec<f64>,
    best_bound: Option<f64>,
    gap: Option<f64>,
    lp: Option<LpSolveReport>,
    jobs_expected: usize,
    jobs_published: usize,
    jobs_completed: usize,
    jobs_redelegated: usize,
    jobs_split: usize,
    timed_out: bool,
    distributed: bool,
    node_id: String,
    role: NodeRole,
    gpu: GpuStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    external_verification: Option<ExternalVerificationReport>,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExternalVerificationReport {
    requested: bool,
    enabled: bool,
    status: String,
    method: Option<String>,
    solver: Option<String>,
    solution_status: Option<String>,
    objective: Option<f64>,
    objective_delta: Option<f64>,
    tolerance: f64,
    elapsed_ms: f64,
    message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GpuStatus {
    available: bool,
    backend: String,
    used: bool,
    mode: String,
    note: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistenceContract {
    mode: String,
    postgres: PostgresPersistenceContract,
    redis: RedisPersistenceContract,
    coordination: CoordinationPersistenceContract,
    in_memory: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PostgresPersistenceContract {
    enabled: bool,
    url_env: Option<String>,
    session_table: &'static str,
    solve_table: &'static str,
    job_table: &'static str,
    event_table: &'static str,
    journal_kinds: Vec<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RedisPersistenceContract {
    enabled: bool,
    url_env: Option<String>,
    key_prefix: String,
    problem_model_key: String,
    solve_snapshot_key: String,
    solve_frontier_key: String,
    session_model_key: String,
    session_revision_lock_key: String,
    generated_mutex_key: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CoordinationPersistenceContract {
    enabled: bool,
    backends: Vec<&'static str>,
    redis_lock_prefix: String,
    live_mutex_url_env: Option<String>,
    lock_ttl_ms: u64,
    lock_wait_ms: u64,
    session_revision_lock_key: String,
    solve_request_lock_key: String,
}

#[derive(Debug)]
struct BoundPreprocess {
    infeasible_reason: Option<String>,
    accelerator: AcceleratorReport,
}

#[derive(Debug)]
struct FrontierNode {
    depth: usize,
    extra_constraints: Vec<BranchConstraint>,
}

#[derive(Debug)]
struct LpRelaxation {
    status: LPStatus,
    x: Vec<f64>,
}

#[derive(Clone, Debug)]
struct StaleWorkerJob {
    job_id: String,
    job_uuid: Option<String>,
    worker_node: String,
    last_heartbeat_ms: u128,
}

enum SubproblemSolveOutcome {
    IpMip(IPMIPSolution),
    Lp {
        problem: LPProblem,
        solution: LPSolution,
    },
    Split {
        children: Vec<SubproblemJob>,
        reason: String,
    },
    Pruned(String),
}

fn default_sense() -> String {
    "max".to_string()
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn env_value(key: &str, fallback: &str) -> String {
    runtime_config::value(key, fallback)
}

fn optional_env_value(key: &str) -> Option<String> {
    runtime_config::optional_value(key)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    runtime_config::u64_value(key, fallback)
}

fn env_usize(key: &str, fallback: usize) -> usize {
    runtime_config::usize_value(key, fallback)
}

fn env_usize_allow_zero(key: &str, fallback: usize) -> usize {
    runtime_config::usize_value_allow_zero(key, fallback)
}

fn env_f64(key: &str, fallback: f64) -> f64 {
    runtime_config::f64_value(key, fallback)
}

fn env_bool(key: &str, fallback: bool) -> bool {
    runtime_config::bool_value(key, fallback)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn request_id(input: Option<String>) -> String {
    input
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("mip-{}", Uuid::new_v4()))
}

fn new_uuid_string() -> String {
    Uuid::new_v4().to_string()
}

fn problem_id(input: Option<String>, request_id: &str) -> Result<String, String> {
    if let Some(value) = input {
        let value = value.trim();
        if value.is_empty() {
            return Err("problemId must be a UUID".to_string());
        }
        return Uuid::parse_str(value)
            .map(|uuid| uuid.to_string())
            .map_err(|_| "problemId must be a UUID".to_string());
    }
    Ok(Uuid::parse_str(request_id)
        .map(|uuid| uuid.to_string())
        .unwrap_or_else(|_| Uuid::new_v4().to_string()))
}

fn cancel_poll_interval() -> Duration {
    Duration::from_secs(env_u64("MIP_SOLVER_CANCEL_POLL_SECONDS", 10).max(1))
}

fn worker_stale_after() -> Duration {
    Duration::from_secs(env_u64("MIP_SOLVER_WORKER_STALE_SECONDS", 100).max(1))
}

fn gpu_mode() -> String {
    env_value("MIP_SOLVER_GPU_MODE", "auto").to_ascii_lowercase()
}

fn gpu_available() -> bool {
    let visible = optional_env_value("NVIDIA_VISIBLE_DEVICES")
        .filter(|value| !value.is_empty() && value != "void" && value != "none");
    let device = FsPath::new("/dev/nvidia0").exists();
    visible.is_some() || device
}

fn gpu_status() -> GpuStatus {
    gpu_status_from_report(&AcceleratorReport::runtime())
}

fn first_configured_env(keys: &[&str]) -> Option<String> {
    runtime_config::first_configured_key(keys)
}

fn first_configured_env_value(keys: &[&str]) -> Option<String> {
    runtime_config::first_configured_value(keys)
}

fn parse_coordination_backends(
    requested: &str,
    redis_available: bool,
    live_mutex_available: bool,
) -> Vec<CoordinationBackend> {
    let requested = requested.trim().to_ascii_lowercase();
    let mut backends = Vec::new();
    let mut push_backend = |backend: CoordinationBackend| {
        if !backends.contains(&backend) {
            backends.push(backend);
        }
    };

    if requested.is_empty() || requested == "auto" {
        if redis_available {
            push_backend(CoordinationBackend::Redis);
        }
        if live_mutex_available {
            push_backend(CoordinationBackend::LiveMutex);
        }
        return backends;
    }

    for part in requested.split(',').map(str::trim) {
        match part {
            "off" | "none" | "disabled" | "false" | "0" => return Vec::new(),
            "both" | "all" => {
                if redis_available {
                    push_backend(CoordinationBackend::Redis);
                }
                if live_mutex_available {
                    push_backend(CoordinationBackend::LiveMutex);
                }
            }
            "redis" => {
                if redis_available {
                    push_backend(CoordinationBackend::Redis);
                }
            }
            "live-mutex" | "live_mutex" | "livemutex" | "lmx" => {
                if live_mutex_available {
                    push_backend(CoordinationBackend::LiveMutex);
                }
            }
            _ => {}
        }
    }
    backends
}

impl CoordinationConfig {
    fn from_env(redis_available: bool) -> Self {
        let live_mutex_url = first_configured_env_value(&[
            "MIP_SOLVER_LIVE_MUTEX_URL",
            "LIVE_MUTEX_URL",
            "LMX_HTTP_URL",
        ]);
        let live_mutex = live_mutex_url.map(|base_url| LiveMutexConfig {
            base_url,
            auth_token: first_configured_env_value(&[
                "MIP_SOLVER_LIVE_MUTEX_AUTH_TOKEN",
                "LIVE_MUTEX_AUTH_TOKEN",
                "LMX_AUTH_TOKEN",
            ]),
            request_timeout_ms: env_u64("MIP_SOLVER_LIVE_MUTEX_REQUEST_TIMEOUT_MS", 10_000),
            max_response_bytes: env_u64("MIP_SOLVER_LIVE_MUTEX_MAX_RESPONSE_BYTES", 1_048_576),
        });
        let requested = env_value("MIP_SOLVER_COORDINATION_BACKENDS", "auto");
        let backends =
            parse_coordination_backends(&requested, redis_available, live_mutex.is_some());

        CoordinationConfig {
            backends,
            redis_lock_prefix: env_value("MIP_SOLVER_REDIS_LOCK_PREFIX", &redis_key_prefix()),
            ttl_ms: env_u64("MIP_SOLVER_COORDINATION_LOCK_TTL_MS", 30_000),
            wait_ms: env_u64("MIP_SOLVER_COORDINATION_WAIT_MS", 5_000),
            live_mutex,
        }
    }

    fn enabled(&self) -> bool {
        !self.backends.is_empty()
    }

    fn backend_names(&self) -> Vec<&'static str> {
        self.backends
            .iter()
            .copied()
            .map(CoordinationBackend::as_str)
            .collect()
    }
}

fn redis_key_prefix() -> String {
    env_value("MIP_SOLVER_REDIS_KEY_PREFIX", MIP_SOLVER_REDIS_PREFIX)
}

fn mip_solver_solve_snapshot_key(prefix: &str, solve_id: &str) -> String {
    format!("{prefix}:solve:{solve_id}:snapshot")
}

fn mip_solver_solve_frontier_key(prefix: &str, solve_id: &str) -> String {
    format!("{prefix}:solve:{solve_id}:frontier")
}

fn mip_solver_problem_model_key(prefix: &str, problem_id: &str, revision: u64) -> String {
    format!("{prefix}:problem:{problem_id}:revision:{revision}")
}

fn local_problem_model_key(problem_id: &str, revision: u64) -> String {
    format!("{problem_id}:{revision}")
}

fn mip_solver_session_model_key(prefix: &str, session_id: &str) -> String {
    format!("{prefix}:session:{session_id}:model")
}

fn mip_solver_session_revision_lock_key(prefix: &str, session_id: &str) -> String {
    format!("{prefix}:session:{session_id}:revision-lock")
}

fn mip_solver_solve_request_lock_key(prefix: &str, problem_id: &str) -> String {
    format!("{prefix}:solve-request:{problem_id}:lock")
}

fn persistence_contract() -> PersistenceContract {
    let pg_url_env = first_configured_env(&[
        "MIP_SOLVER_DATABASE_URL",
        "AGENT_TASKS_RDS_DATABASE_URL",
        "RDS_DATABASE_URL",
        "DATABASE_URL",
        "PG_DATABASE_URL",
    ]);
    let redis_url_env = first_configured_env(&["MIP_SOLVER_REDIS_URL", "REDIS_URL"]);
    let live_mutex_url_env = first_configured_env(&[
        "MIP_SOLVER_LIVE_MUTEX_URL",
        "LIVE_MUTEX_URL",
        "LMX_HTTP_URL",
    ]);
    let prefix = redis_key_prefix();
    let coordination = CoordinationConfig::from_env(redis_url_env.is_some());

    PersistenceContract {
        mode: if pg_url_env.is_some() || redis_url_env.is_some() {
            "durable-plus-hot-cache".to_string()
        } else {
            "in-memory-only".to_string()
        },
        postgres: PostgresPersistenceContract {
            enabled: pg_url_env.is_some(),
            url_env: pg_url_env,
            session_table: MIP_SOLVER_SESSIONS_TABLE,
            solve_table: MIP_SOLVER_SOLVES_TABLE,
            job_table: MIP_SOLVER_JOBS_TABLE,
            event_table: MIP_SOLVER_EVENTS_TABLE,
            journal_kinds: vec![
                "mip-solver.solve-started",
                "mip-solver.model-revision",
                "mip-solver.subproblem-submitted",
                "mip-solver.subproblem-finished",
                "mip-solver.subproblem-split",
                "mip-solver.solve-finished",
            ],
        },
        redis: RedisPersistenceContract {
            enabled: redis_url_env.is_some(),
            url_env: redis_url_env,
            key_prefix: prefix.clone(),
            problem_model_key: mip_solver_problem_model_key(&prefix, "{problemId}", 0),
            solve_snapshot_key: mip_solver_solve_snapshot_key(&prefix, "{solveId}"),
            solve_frontier_key: mip_solver_solve_frontier_key(&prefix, "{solveId}"),
            session_model_key: mip_solver_session_model_key(&prefix, "{sessionId}"),
            session_revision_lock_key: mip_solver_session_revision_lock_key(&prefix, "{sessionId}"),
            generated_mutex_key: container_pool_affinity_lock_key(
                CONTAINER_POOL_AFFINITY_LOCK_KEY_DEFAULT_PREFIX,
                "mip-solver",
                "{solveId}",
            ),
        },
        coordination: CoordinationPersistenceContract {
            enabled: coordination.enabled(),
            backends: coordination.backend_names(),
            redis_lock_prefix: coordination.redis_lock_prefix.clone(),
            live_mutex_url_env,
            lock_ttl_ms: coordination.ttl_ms,
            lock_wait_ms: coordination.wait_ms,
            session_revision_lock_key: mip_solver_session_revision_lock_key(
                &coordination.redis_lock_prefix,
                "{sessionId}",
            ),
            solve_request_lock_key: mip_solver_solve_request_lock_key(
                &coordination.redis_lock_prefix,
                "{problemId}",
            ),
        },
        in_memory: vec![
            "active solve registry".to_string(),
            "recent problem model cache".to_string(),
            "live session problem snapshot".to_string(),
            "current master-owned frontier".to_string(),
            "observed worker registry".to_string(),
        ],
    }
}

fn gpu_status_from_report(report: &AcceleratorReport) -> GpuStatus {
    let note = report.notes.first().cloned();
    GpuStatus {
        available: report.gpu_available,
        backend: report.backend.clone(),
        used: report.used_gpu,
        mode: report.mode.clone(),
        note,
    }
}

fn aggregate_gpu_status(results: &[SubproblemResult]) -> GpuStatus {
    let mut report = AcceleratorReport::runtime();
    for result in results {
        report.merge(&result.accelerator);
    }
    gpu_status_from_report(&report)
}

impl AcceleratorReport {
    fn runtime() -> Self {
        let mode = gpu_mode();
        let gpu_available = gpu_available();
        AcceleratorReport {
            mode,
            backend: if gpu_available {
                "cuda-visible".to_string()
            } else {
                "cpu".to_string()
            },
            gpu_available,
            used_gpu: false,
            used_cpu_parallel: false,
            notes: Vec::new(),
        }
    }

    fn for_mode(mode: &str) -> Self {
        let gpu_available = gpu_available();
        AcceleratorReport {
            mode: mode.to_ascii_lowercase(),
            backend: if gpu_available {
                "cuda-visible".to_string()
            } else {
                "cpu".to_string()
            },
            gpu_available,
            used_gpu: false,
            used_cpu_parallel: false,
            notes: Vec::new(),
        }
    }

    fn merge(&mut self, other: &AcceleratorReport) {
        self.gpu_available |= other.gpu_available;
        self.used_gpu |= other.used_gpu;
        self.used_cpu_parallel |= other.used_cpu_parallel;
        if other.used_gpu || other.backend != "cpu" {
            self.backend = other.backend.clone();
        }
        for note in &other.notes {
            if !self.notes.iter().any(|existing| existing == note) {
                self.notes.push(note.clone());
            }
        }
    }
}

fn gpu_disabled(mode: &str) -> bool {
    matches!(mode, "off" | "false" | "0" | "disabled" | "none")
}

fn gpu_required(mode: &str) -> bool {
    matches!(mode, "require" | "required" | "must")
}

fn dense_matvec_cpu(a: &[Vec<f64>], x: &[f64]) -> (Vec<f64>, bool) {
    let rows = a.len();
    if rows == 0 {
        return (Vec::new(), false);
    }
    let workers = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(rows);
    if workers <= 1 || rows < 256 {
        let values = a
            .iter()
            .map(|row| {
                row.iter()
                    .zip(x.iter())
                    .map(|(coef, value)| coef * value)
                    .sum()
            })
            .collect();
        return (values, false);
    }

    let chunk_len = rows.div_ceil(workers);
    let mut values = vec![0.0; rows];
    thread::scope(|scope| {
        for (chunk_index, out) in values.chunks_mut(chunk_len).enumerate() {
            let start = chunk_index * chunk_len;
            let input = &a[start..start + out.len()];
            scope.spawn(move || {
                for (slot, row) in out.iter_mut().zip(input.iter()) {
                    *slot = row
                        .iter()
                        .zip(x.iter())
                        .map(|(coef, value)| coef * value)
                        .sum();
                }
            });
        }
    });
    (values, true)
}

fn dense_matvec_accelerated_with_mode(
    a: &[Vec<f64>],
    x: &[f64],
    mode: &str,
) -> Result<(Vec<f64>, AcceleratorReport), String> {
    let mut report = AcceleratorReport::for_mode(mode);
    if a.is_empty() {
        return Ok((Vec::new(), report));
    }
    if a.iter().any(|row| row.len() != x.len()) {
        return Err("accelerated matvec received a non-rectangular matrix".to_string());
    }

    if !gpu_disabled(&report.mode) {
        if report.gpu_available {
            match dense_matvec_cuda_row_major(a, x) {
                Ok(values) => {
                    report.backend = "cuda-cublas-dgemv".to_string();
                    report.used_gpu = true;
                    return Ok((values, report));
                }
                Err(error) if gpu_required(&report.mode) => return Err(error),
                Err(error) => report.notes.push(format!(
                    "CUDA/cuBLAS unavailable; used CPU fallback: {error}"
                )),
            }
        } else {
            let note = "GPU requested but no NVIDIA device is visible".to_string();
            if gpu_required(&report.mode) {
                return Err(note);
            }
            report.notes.push(note);
        }
    }

    let (values, used_parallel) = dense_matvec_cpu(a, x);
    report.backend = if used_parallel {
        "in-house-cpu-threaded".to_string()
    } else {
        "in-house-cpu".to_string()
    };
    report.used_cpu_parallel = used_parallel;
    Ok((values, report))
}

type CudaResult = c_int;
type CublasResult = c_int;
type CublasHandle = *mut c_void;

const CUDA_SUCCESS: CudaResult = 0;
const CUBLAS_STATUS_SUCCESS: CublasResult = 0;
const CUDA_MEMCPY_HOST_TO_DEVICE: c_int = 1;
const CUDA_MEMCPY_DEVICE_TO_HOST: c_int = 2;
const CUBLAS_OP_T: c_int = 1;

struct CudaLibraries {
    _cudart: Library,
    _cublas: Library,
    cuda_malloc: unsafe extern "C" fn(*mut *mut c_void, usize) -> CudaResult,
    cuda_free: unsafe extern "C" fn(*mut c_void) -> CudaResult,
    cuda_memcpy: unsafe extern "C" fn(*mut c_void, *const c_void, usize, c_int) -> CudaResult,
    cuda_device_synchronize: unsafe extern "C" fn() -> CudaResult,
    cublas_create: unsafe extern "C" fn(*mut CublasHandle) -> CublasResult,
    cublas_destroy: unsafe extern "C" fn(CublasHandle) -> CublasResult,
    cublas_dgemv: unsafe extern "C" fn(
        CublasHandle,
        c_int,
        c_int,
        c_int,
        *const f64,
        *const f64,
        c_int,
        *const f64,
        c_int,
        *const f64,
        *mut f64,
        c_int,
    ) -> CublasResult,
}

impl CudaLibraries {
    fn load() -> Result<Self, String> {
        let cudart = open_first_library(&["libcudart.so", "libcudart.so.12", "libcudart.so.11.0"])?;
        let cublas = open_first_library(&["libcublas.so", "libcublas.so.12", "libcublas.so.11"])?;
        unsafe {
            Ok(CudaLibraries {
                cuda_malloc: load_symbol(&cudart, b"cudaMalloc\0")?,
                cuda_free: load_symbol(&cudart, b"cudaFree\0")?,
                cuda_memcpy: load_symbol(&cudart, b"cudaMemcpy\0")?,
                cuda_device_synchronize: load_symbol(&cudart, b"cudaDeviceSynchronize\0")?,
                cublas_create: load_symbol(&cublas, b"cublasCreate_v2\0")?,
                cublas_destroy: load_symbol(&cublas, b"cublasDestroy_v2\0")?,
                cublas_dgemv: load_symbol(&cublas, b"cublasDgemv_v2\0")?,
                _cudart: cudart,
                _cublas: cublas,
            })
        }
    }
}

fn open_first_library(candidates: &[&str]) -> Result<Library, String> {
    let mut errors = Vec::new();
    for candidate in candidates {
        match unsafe { Library::new(candidate) } {
            Ok(library) => return Ok(library),
            Err(error) => errors.push(format!("{candidate}: {error}")),
        }
    }
    Err(errors.join("; "))
}

unsafe fn load_symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T, String> {
    library
        .get::<T>(name)
        .map(|symbol| *symbol)
        .map_err(|error| {
            let label_bytes = name.strip_suffix(&[0]).unwrap_or(name);
            let label = String::from_utf8_lossy(label_bytes);
            format!("missing CUDA symbol {label}: {error}")
        })
}

struct CudaAllocation<'a> {
    libs: &'a CudaLibraries,
    ptr: *mut c_void,
}

impl<'a> CudaAllocation<'a> {
    fn new(libs: &'a CudaLibraries, bytes: usize) -> Result<Self, String> {
        let mut ptr = ptr::null_mut();
        check_cuda(unsafe { (libs.cuda_malloc)(&mut ptr, bytes) }, "cudaMalloc")?;
        Ok(CudaAllocation { libs, ptr })
    }

    fn copy_from_host<T>(&self, input: &[T]) -> Result<(), String> {
        check_cuda(
            unsafe {
                (self.libs.cuda_memcpy)(
                    self.ptr,
                    input.as_ptr() as *const c_void,
                    std::mem::size_of_val(input),
                    CUDA_MEMCPY_HOST_TO_DEVICE,
                )
            },
            "cudaMemcpy host-to-device",
        )
    }

    fn copy_to_host<T>(&self, output: &mut [T]) -> Result<(), String> {
        check_cuda(
            unsafe {
                (self.libs.cuda_memcpy)(
                    output.as_mut_ptr() as *mut c_void,
                    self.ptr,
                    std::mem::size_of_val(output),
                    CUDA_MEMCPY_DEVICE_TO_HOST,
                )
            },
            "cudaMemcpy device-to-host",
        )
    }
}

impl Drop for CudaAllocation<'_> {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            let _ = unsafe { (self.libs.cuda_free)(self.ptr) };
        }
    }
}

struct CublasContext<'a> {
    libs: &'a CudaLibraries,
    handle: CublasHandle,
}

impl<'a> CublasContext<'a> {
    fn new(libs: &'a CudaLibraries) -> Result<Self, String> {
        let mut handle = ptr::null_mut();
        check_cublas(
            unsafe { (libs.cublas_create)(&mut handle) },
            "cublasCreate_v2",
        )?;
        Ok(CublasContext { libs, handle })
    }
}

impl Drop for CublasContext<'_> {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            let _ = unsafe { (self.libs.cublas_destroy)(self.handle) };
        }
    }
}

fn check_cuda(code: CudaResult, op: &str) -> Result<(), String> {
    if code == CUDA_SUCCESS {
        Ok(())
    } else {
        Err(format!("{op} failed with CUDA status {code}"))
    }
}

fn check_cublas(code: CublasResult, op: &str) -> Result<(), String> {
    if code == CUBLAS_STATUS_SUCCESS {
        Ok(())
    } else {
        Err(format!("{op} failed with cuBLAS status {code}"))
    }
}

fn dense_matvec_cuda_row_major(a: &[Vec<f64>], x: &[f64]) -> Result<Vec<f64>, String> {
    let rows = a.len();
    let cols = x.len();
    if rows == 0 {
        return Ok(Vec::new());
    }
    if rows > c_int::MAX as usize || cols > c_int::MAX as usize {
        return Err("matrix is too large for cuBLAS int dimensions".to_string());
    }
    let libs = CudaLibraries::load()?;
    let cublas = CublasContext::new(&libs)?;
    let flat: Vec<f64> = a.iter().flat_map(|row| row.iter().copied()).collect();
    let mut output = vec![0.0; rows];

    let d_a = CudaAllocation::new(&libs, std::mem::size_of_val(flat.as_slice()))?;
    let d_x = CudaAllocation::new(&libs, std::mem::size_of_val(x))?;
    let d_y = CudaAllocation::new(&libs, std::mem::size_of_val(output.as_slice()))?;
    d_a.copy_from_host(&flat)?;
    d_x.copy_from_host(x)?;

    let alpha = 1.0;
    let beta = 0.0;
    check_cublas(
        unsafe {
            (libs.cublas_dgemv)(
                cublas.handle,
                CUBLAS_OP_T,
                cols as c_int,
                rows as c_int,
                &alpha,
                d_a.ptr as *const f64,
                cols as c_int,
                d_x.ptr as *const f64,
                1,
                &beta,
                d_y.ptr as *mut f64,
                1,
            )
        },
        "cublasDgemv_v2",
    )?;
    check_cuda(
        unsafe { (libs.cuda_device_synchronize)() },
        "cudaDeviceSynchronize",
    )?;
    d_y.copy_to_host(&mut output)?;
    Ok(output)
}

fn preprocess_bounds(
    problem: &MipProblemSpec,
    extra_constraints: &[BranchConstraint],
) -> Result<BoundPreprocess, String> {
    preprocess_bounds_with_mode(problem, extra_constraints, &gpu_mode())
}

fn preprocess_bounds_with_mode(
    problem: &MipProblemSpec,
    extra_constraints: &[BranchConstraint],
    mode: &str,
) -> Result<BoundPreprocess, String> {
    let problem = normalized_problem(problem.clone())?;
    let n = problem.c.len();
    let mut rows = problem.a.clone();
    let mut rhs = problem.b.clone();
    for constraint in extra_constraints {
        validate_branch_constraint(constraint, n)?;
        rows.push(constraint.coefs.clone());
        rhs.push(constraint.rhs);
    }

    let upper_bounds = problem.ub.clone().unwrap_or_else(|| vec![f64::INFINITY; n]);
    let finite_ub: Vec<f64> = upper_bounds
        .iter()
        .map(|ub| if ub.is_finite() { *ub } else { 0.0 })
        .collect();
    let mut positive = vec![vec![0.0; n]; rows.len()];
    let mut negative = vec![vec![0.0; n]; rows.len()];
    let mut positive_infinite = vec![false; rows.len()];
    let mut negative_infinite = vec![false; rows.len()];

    for (row_index, row) in rows.iter().enumerate() {
        for (col, coef) in row.iter().enumerate() {
            if *coef > 0.0 {
                if upper_bounds[col].is_finite() {
                    positive[row_index][col] = *coef;
                } else {
                    positive_infinite[row_index] = true;
                }
            } else if *coef < 0.0 {
                if upper_bounds[col].is_finite() {
                    negative[row_index][col] = *coef;
                } else {
                    negative_infinite[row_index] = true;
                }
            }
        }
    }

    let (max_finite, mut accelerator) =
        dense_matvec_accelerated_with_mode(&positive, &finite_ub, mode)?;
    let (min_finite, negative_report) =
        dense_matvec_accelerated_with_mode(&negative, &finite_ub, mode)?;
    accelerator.merge(&negative_report);

    let mut always_satisfied_rows = 0usize;
    for row_index in 0..rows.len() {
        let min_activity = if negative_infinite[row_index] {
            f64::NEG_INFINITY
        } else {
            min_finite[row_index]
        };
        let max_activity = if positive_infinite[row_index] {
            f64::INFINITY
        } else {
            max_finite[row_index]
        };
        if min_activity > rhs[row_index] + 1e-9 {
            return Ok(BoundPreprocess {
                infeasible_reason: Some(format!(
                    "bound preprocessing proved row {row_index} infeasible: min activity {min_activity:.6} > rhs {:.6}",
                    rhs[row_index]
                )),
                accelerator,
            });
        }
        if max_activity.is_finite() && max_activity <= rhs[row_index] + 1e-9 {
            always_satisfied_rows += 1;
        }
    }
    if always_satisfied_rows > 0 {
        accelerator.notes.push(format!(
            "bound preprocessing found {always_satisfied_rows} rows always satisfied by variable bounds"
        ));
    }

    Ok(BoundPreprocess {
        infeasible_reason: None,
        accelerator,
    })
}

fn sense_of(raw: &str) -> Sense {
    match raw.to_ascii_lowercase().as_str() {
        "min" | "minimize" | "minimise" => Sense::Min,
        _ => Sense::Max,
    }
}

fn validate_problem(problem: &MipProblemSpec) -> Result<(), String> {
    let n = problem.c.len();
    if n == 0 {
        return Err("objective vector `c` must not be empty".to_string());
    }
    if n > MAX_VARS {
        return Err(format!("variable count {n} exceeds limit {MAX_VARS}"));
    }
    if problem.a.len() != problem.b.len() {
        return Err(format!(
            "`a` has {} rows but `b` has {} entries",
            problem.a.len(),
            problem.b.len()
        ));
    }
    if problem.a.len() > MAX_CONSTRAINTS {
        return Err(format!(
            "constraint count {} exceeds limit {MAX_CONSTRAINTS}",
            problem.a.len()
        ));
    }
    if problem.c.iter().any(|v| !v.is_finite()) {
        return Err("objective coefficients must be finite".to_string());
    }
    if problem.b.iter().any(|v| !v.is_finite()) {
        return Err("right-hand sides must be finite".to_string());
    }
    for (i, row) in problem.a.iter().enumerate() {
        if row.len() != n {
            return Err(format!("row {i} has length {}, expected {n}", row.len()));
        }
        if row.iter().any(|v| !v.is_finite()) {
            return Err(format!("row {i} contains a non-finite coefficient"));
        }
    }
    if problem.integer_vars.len() > n {
        return Err("integerVars length must not exceed len(c)".to_string());
    }
    if let Some(ub) = &problem.ub {
        if ub.len() != n {
            return Err("ub length must equal len(c)".to_string());
        }
        if ub.iter().any(|v| v.is_nan() || *v < 0.0) {
            return Err("ub entries must be non-negative or infinite".to_string());
        }
    }
    if let Some(names) = &problem.var_names {
        if names.len() != n {
            return Err("varNames length must equal len(c)".to_string());
        }
    }
    if let Some(names) = &problem.con_names {
        if names.len() != problem.a.len() {
            return Err("conNames length must equal constraint count".to_string());
        }
    }
    Ok(())
}

fn normalized_problem(mut problem: MipProblemSpec) -> Result<MipProblemSpec, String> {
    validate_problem(&problem)?;
    problem.integer_vars.resize(problem.c.len(), false);
    Ok(problem)
}

fn validate_branch_constraint(constraint: &BranchConstraint, n: usize) -> Result<(), String> {
    if constraint.coefs.len() != n {
        return Err(format!(
            "branch constraint {} has length {}, expected {}",
            constraint.name,
            constraint.coefs.len(),
            n
        ));
    }
    if constraint.coefs.iter().any(|v| !v.is_finite()) {
        return Err(format!(
            "branch constraint {} contains a non-finite coefficient",
            constraint.name
        ));
    }
    if !constraint.rhs.is_finite() {
        return Err(format!(
            "branch constraint {} rhs must be finite",
            constraint.name
        ));
    }
    Ok(())
}

fn finite_f64(value: &Value, label: &str) -> Result<f64, String> {
    let Some(number) = value.as_f64() else {
        return Err(format!("{label} must be a finite number"));
    };
    if !number.is_finite() {
        return Err(format!("{label} must be a finite number"));
    }
    Ok(number)
}

fn vec_f64(command: &Value, key: &str) -> Result<Option<Vec<f64>>, String> {
    let Some(value) = command.get(key) else {
        return Ok(None);
    };
    let Some(items) = value.as_array() else {
        return Err(format!("{key} must be an array of finite numbers"));
    };
    items
        .iter()
        .enumerate()
        .map(|(index, value)| finite_f64(value, &format!("{key}[{index}]")))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn vec_vec_f64(command: &Value, key: &str) -> Result<Option<Vec<Vec<f64>>>, String> {
    let Some(value) = command.get(key) else {
        return Ok(None);
    };
    let Some(rows) = value.as_array() else {
        return Err(format!("{key} must be an array of numeric rows"));
    };
    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            let Some(cells) = row.as_array() else {
                return Err(format!(
                    "{key}[{row_index}] must be an array of finite numbers"
                ));
            };
            cells
                .iter()
                .enumerate()
                .map(|(col_index, value)| {
                    finite_f64(value, &format!("{key}[{row_index}][{col_index}]"))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn usize_at(command: &Value, key: &str) -> Result<Option<usize>, String> {
    let Some(value) = command.get(key) else {
        return Ok(None);
    };
    let Some(number) = value.as_u64() else {
        return Err(format!("{key} must be a non-negative integer"));
    };
    usize::try_from(number)
        .map(Some)
        .map_err(|_| format!("{key} is too large for this platform"))
}

fn f64_at(command: &Value, key: &str, fallback: f64) -> Result<f64, String> {
    command
        .get(key)
        .map(|value| finite_f64(value, key))
        .unwrap_or(Ok(fallback))
}

fn bool_at(command: &Value, key: &str, fallback: bool) -> Result<bool, String> {
    let Some(value) = command.get(key) else {
        return Ok(fallback);
    };
    value
        .as_bool()
        .ok_or_else(|| format!("{key} must be a boolean"))
}

fn vec_bool(command: &Value, key: &str) -> Result<Option<Vec<bool>>, String> {
    let Some(value) = command.get(key) else {
        return Ok(None);
    };
    let Some(items) = value.as_array() else {
        return Err(format!("{key} must be an array of booleans"));
    };
    items
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_bool()
                .ok_or_else(|| format!("{key}[{index}] must be a boolean"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn vec_string(command: &Value, key: &str) -> Result<Option<Vec<String>>, String> {
    let Some(value) = command.get(key) else {
        return Ok(None);
    };
    let Some(items) = value.as_array() else {
        return Err(format!("{key} must be an array of strings"));
    };
    items
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_str()
                .map(String::from)
                .ok_or_else(|| format!("{key}[{index}] must be a string"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn str_at(command: &Value, key: &str) -> Result<Option<String>, String> {
    let Some(value) = command.get(key) else {
        return Ok(None);
    };
    value
        .as_str()
        .map(|value| Some(value.to_string()))
        .ok_or_else(|| format!("{key} must be a string"))
}

fn parse_problem_from_commands(
    commands: &[Value],
) -> Result<(MipProblemSpec, u64, Vec<Value>), String> {
    if commands.len() > MAX_STREAM_COMMANDS {
        return Err(format!(
            "stream command count {} exceeds limit {MAX_STREAM_COMMANDS}",
            commands.len()
        ));
    }
    let mut problem: Option<MipProblemSpec> = None;
    let mut revision = 0;
    let mut frames = Vec::new();
    for command in commands {
        apply_stream_command(&mut problem, &mut revision, command, &mut frames)?;
    }
    let problem = problem.ok_or_else(|| {
        "no problem initialized; first command must be {\"op\":\"init\", ...}".to_string()
    })?;
    Ok((problem, revision, frames))
}

fn apply_stream_command(
    problem: &mut Option<MipProblemSpec>,
    revision: &mut u64,
    command: &Value,
    frames: &mut Vec<Value>,
) -> Result<(), String> {
    let op = command.get("op").and_then(Value::as_str).unwrap_or("");
    if op == "init" {
        let mut next = if let Some(raw) = command.get("problem") {
            serde_json::from_value::<MipProblemSpec>(raw.clone())
                .map_err(|err| format!("invalid problem: {err}"))?
        } else {
            MipProblemSpec {
                sense: str_at(command, "sense")?.unwrap_or_else(default_sense),
                c: vec_f64(command, "c")?.unwrap_or_default(),
                a: vec_vec_f64(command, "a")?.unwrap_or_default(),
                b: vec_f64(command, "b")?.unwrap_or_default(),
                integer_vars: vec_bool(command, "integerVars")?.unwrap_or_default(),
                ub: vec_f64(command, "ub")?,
                var_names: vec_string(command, "varNames")?,
                con_names: vec_string(command, "conNames")?,
            }
        };
        next = normalized_problem(next)?;
        *problem = Some(next);
        *revision += 1;
        frames.push(json!({"event":"initialized","revision":revision}));
        return Ok(());
    }

    let p = problem
        .as_mut()
        .ok_or_else(|| "no problem initialized; send init first".to_string())?;
    match op {
        "add_constraint" => {
            let coefs = vec_f64(command, "coefs")?.unwrap_or_default();
            if coefs.len() != p.c.len() {
                return Err("coefs length must equal variable count".to_string());
            }
            let rhs = f64_at(command, "rhs", 0.0)?;
            if !rhs.is_finite() {
                return Err("rhs must be finite".to_string());
            }
            p.a.push(coefs);
            p.b.push(rhs);
            if let Some(names) = p.con_names.as_mut() {
                names.push(
                    str_at(command, "name")?
                        .unwrap_or_else(|| format!("constraint{}", p.a.len() - 1)),
                );
            }
        }
        "set_constraint"
        | "modify_constraint"
        | "change_constraint_weights"
        | "set_constraint_weights" => {
            let index = usize_at(command, "index")?.ok_or("index is required")?;
            if index >= p.a.len() {
                return Err("constraint index out of range".to_string());
            }
            if let Some(coefs) = vec_f64(command, "coefs")? {
                if coefs.len() != p.c.len() {
                    return Err("coefs length must equal variable count".to_string());
                }
                p.a[index] = coefs;
            }
            if command.get("rhs").is_some() {
                let rhs = f64_at(command, "rhs", p.b[index])?;
                if !rhs.is_finite() {
                    return Err("rhs must be finite".to_string());
                }
                p.b[index] = rhs;
            }
            if let (Some(name), Some(names)) = (str_at(command, "name")?, p.con_names.as_mut()) {
                names[index] = name;
            }
        }
        "remove_constraint" | "rm_constraint" => {
            let index = usize_at(command, "index")?.ok_or("index is required")?;
            if index >= p.a.len() {
                return Err("constraint index out of range".to_string());
            }
            p.a.remove(index);
            p.b.remove(index);
            if let Some(names) = p.con_names.as_mut() {
                names.remove(index);
            }
        }
        "set_rhs" => {
            let index = usize_at(command, "index")?.ok_or("index is required")?;
            if index >= p.b.len() {
                return Err("constraint index out of range".to_string());
            }
            let rhs = f64_at(command, "rhs", p.b[index])?;
            if !rhs.is_finite() {
                return Err("rhs must be finite".to_string());
            }
            p.b[index] = rhs;
        }
        "set_coefficient" | "set_constraint_weight" | "change_constraint_weight" => {
            let row = usize_at(command, "row")?.ok_or("row is required")?;
            let col = usize_at(command, "col")?.ok_or("col is required")?;
            if row >= p.a.len() || col >= p.c.len() {
                return Err("coefficient index out of range".to_string());
            }
            p.a[row][col] = f64_at(command, "value", p.a[row][col])?;
        }
        "add_variable" => {
            let column = vec_f64(command, "column")?.unwrap_or_else(|| vec![0.0; p.a.len()]);
            if column.len() != p.a.len() {
                return Err("column length must equal constraint count".to_string());
            }
            p.c.push(f64_at(command, "c", 0.0)?);
            p.integer_vars.push(bool_at(command, "integer", false)?);
            for (row, value) in p.a.iter_mut().zip(column.iter()) {
                row.push(*value);
            }
            if p.ub.is_some() || command.get("ub").is_some() {
                let upper = f64_at(command, "ub", f64::INFINITY)?;
                p.ub.get_or_insert_with(|| vec![f64::INFINITY; p.c.len() - 1])
                    .push(upper);
            }
            if let Some(names) = p.var_names.as_mut() {
                names.push(
                    str_at(command, "name")?.unwrap_or_else(|| format!("x{}", p.c.len() - 1)),
                );
            }
        }
        "set_variable" | "modify_variable" => {
            let index = usize_at(command, "index")?.ok_or("index is required")?;
            if index >= p.c.len() {
                return Err("variable index out of range".to_string());
            }
            if command.get("c").is_some() {
                p.c[index] = f64_at(command, "c", p.c[index])?;
            }
            if command.get("integer").is_some() {
                p.integer_vars[index] = bool_at(command, "integer", p.integer_vars[index])?;
            }
            if command.get("ub").is_some() {
                p.ub.get_or_insert_with(|| vec![f64::INFINITY; p.c.len()])[index] =
                    f64_at(command, "ub", f64::INFINITY)?;
            }
            if let Some(column) = vec_f64(command, "column")? {
                if column.len() != p.a.len() {
                    return Err("column length must equal constraint count".to_string());
                }
                for (row, value) in p.a.iter_mut().zip(column.iter()) {
                    row[index] = *value;
                }
            }
            if let (Some(name), Some(names)) = (str_at(command, "name")?, p.var_names.as_mut()) {
                names[index] = name;
            }
        }
        "remove_variable" | "rm_variable" => {
            let index = usize_at(command, "index")?.ok_or("index is required")?;
            if index >= p.c.len() {
                return Err("variable index out of range".to_string());
            }
            if p.c.len() == 1 {
                return Err("cannot remove the last variable".to_string());
            }
            p.c.remove(index);
            p.integer_vars.remove(index);
            for row in &mut p.a {
                row.remove(index);
            }
            if let Some(ub) = p.ub.as_mut() {
                ub.remove(index);
            }
            if let Some(names) = p.var_names.as_mut() {
                names.remove(index);
            }
        }
        "set_objective" => {
            let c = vec_f64(command, "c")?.unwrap_or_default();
            if c.len() != p.c.len() {
                return Err("c length must equal variable count".to_string());
            }
            p.c = c;
        }
        "set_integer" => {
            let index = usize_at(command, "index")?.ok_or("index is required")?;
            if index >= p.integer_vars.len() {
                return Err("variable index out of range".to_string());
            }
            p.integer_vars[index] = bool_at(command, "integer", true)?;
        }
        "set_upper_bound" | "set_ub" => {
            let index = usize_at(command, "index")?.ok_or("index is required")?;
            if index >= p.c.len() {
                return Err("variable index out of range".to_string());
            }
            p.ub.get_or_insert_with(|| vec![f64::INFINITY; p.c.len()])[index] =
                f64_at(command, "ub", f64::INFINITY)?;
        }
        "set_sense" => {
            p.sense = str_at(command, "sense")?.unwrap_or_else(default_sense);
        }
        "snapshot" => {
            frames.push(json!({
                "event":"model",
                "revision": revision,
                "numVars": p.c.len(),
                "numConstraints": p.a.len(),
                "integerVars": p.integer_vars,
            }));
            return Ok(());
        }
        other => return Err(format!("unknown stream op `{other}`")),
    }
    *revision += 1;
    validate_problem(p)?;
    frames.push(json!({"event":"applied","op":op,"revision":revision}));
    Ok(())
}

fn to_ipmip_problem(
    problem: &MipProblemSpec,
    extra_constraints: &[BranchConstraint],
) -> Result<IPMIPProblem, String> {
    let problem = normalized_problem(problem.clone())?;
    let mut a = problem.a.clone();
    let mut b = problem.b.clone();
    let mut con_names = problem.con_names.clone();
    for constraint in extra_constraints {
        validate_branch_constraint(constraint, problem.c.len())?;
        a.push(constraint.coefs.clone());
        b.push(constraint.rhs);
        if let Some(names) = con_names.as_mut() {
            names.push(constraint.name.clone());
        }
    }
    Ok(IPMIPProblem {
        sense: sense_of(&problem.sense),
        c: problem.c,
        a,
        b,
        integer_vars: problem.integer_vars,
        ub: problem.ub,
        var_names: problem.var_names,
        con_names,
        lazy_constraints: None,
        variable_nodes: None,
        constraint_nodes: None,
    })
}

fn to_lp_problem(
    problem: &MipProblemSpec,
    extra_constraints: &[BranchConstraint],
) -> Result<LPProblem, String> {
    let problem = normalized_problem(problem.clone())?;
    let mut a = problem.a.clone();
    let mut b = problem.b.clone();
    let mut con_names = problem.con_names.clone();
    for constraint in extra_constraints {
        validate_branch_constraint(constraint, problem.c.len())?;
        a.push(constraint.coefs.clone());
        b.push(constraint.rhs);
        if let Some(names) = con_names.as_mut() {
            names.push(constraint.name.clone());
        }
    }
    Ok(LPProblem {
        sense: sense_of(&problem.sense),
        c: problem.c.clone(),
        a_ub: Some(a),
        b_ub: Some(b),
        a_eq: None,
        b_eq: None,
        lb: Some(vec![Some(0.0); problem.c.len()]),
        ub: problem
            .ub
            .map(|ub| ub.into_iter().map(|v| v.is_finite().then_some(v)).collect()),
        var_names: problem.var_names.clone(),
        con_names,
    })
}

fn solve_lp_relaxation(
    problem: &MipProblemSpec,
    extra_constraints: &[BranchConstraint],
    lp_max_iters: usize,
) -> Result<LpRelaxation, String> {
    let lp = to_lp_problem(problem, extra_constraints)?;
    let sol = solve_lp_internal(
        &lp,
        &InternalSimplexOptions {
            max_iter: Some(lp_max_iters),
            tol: Some(1e-9),
            basis_start: None,
        },
    );
    Ok(LpRelaxation {
        status: sol.status,
        x: sol.x,
    })
}

fn is_pure_lp(problem: &MipProblemSpec) -> Result<bool, String> {
    let problem = normalized_problem(problem.clone())?;
    Ok(!problem.integer_vars.iter().any(|integer| *integer))
}

fn lp_report_from_solution(lp: &LPProblem, solution: &LPSolution) -> LpSolveReport {
    LpSolveReport {
        primal: LpPrimalReport {
            objective: solution.objective.is_finite().then_some(solution.objective),
            x: solution.x.clone(),
            var_names: lp.var_names.clone(),
        },
        dual: LpDualReport {
            inequality: solution.dual_ub.clone(),
            equality: solution.dual_eq.clone(),
            reduced_costs: solution.reduced_costs.clone(),
            row_names: lp.con_names.clone(),
            var_names: lp.var_names.clone(),
        },
        basis: LpBasisReport {
            variables: solution.var_basis.clone(),
            rows: solution.row_basis.clone(),
        },
        iterations: solution.iters,
        solver: solution.solver.clone(),
        elapsed_ms: solution.elapsed_ms,
        message: solution.message.clone(),
    }
}

fn first_fractional(problem: &MipProblemSpec, x: &[f64], int_tol: f64) -> Option<(usize, f64)> {
    problem
        .integer_vars
        .iter()
        .enumerate()
        .filter(|(index, integer)| **integer && *index < x.len())
        .map(|(index, _)| (index, x[index]))
        .find(|(_, value)| (value - value.round()).abs() > int_tol)
}

fn branch_constraints(var: usize, value: f64, n: usize, depth: usize) -> [BranchConstraint; 2] {
    let floor = value.floor();
    let ceil = value.ceil();
    let mut left = vec![0.0; n];
    left[var] = 1.0;
    let mut right = vec![0.0; n];
    right[var] = -1.0;
    [
        BranchConstraint {
            coefs: left,
            rhs: floor,
            name: format!("branch_d{depth}_x{var}_le_{floor:.0}"),
        },
        BranchConstraint {
            coefs: right,
            rhs: -ceil,
            name: format!("branch_d{depth}_x{var}_ge_{ceil:.0}"),
        },
    ]
}

fn split_subproblem_children(
    job: &SubproblemJob,
) -> Result<Option<(Vec<SubproblemJob>, String)>, String> {
    let split_depth = job.options.split_depth.unwrap_or(1).min(8);
    if job.depth >= split_depth {
        return Ok(None);
    }
    let problem = job.problem()?;
    let lp_max_iters = job.options.lp_max_iters.unwrap_or(5_000);
    let int_tol = job.options.int_tol.unwrap_or(1e-6);
    let relaxation = solve_lp_relaxation(problem, &job.extra_constraints, lp_max_iters)?;
    if relaxation.status != LPStatus::Optimal {
        return Ok(None);
    }
    let Some((var, value)) = first_fractional(problem, &relaxation.x, int_tol) else {
        return Ok(None);
    };

    let [left, right] = branch_constraints(var, value, problem.c.len(), job.depth);
    let next_depth = job.depth + 1;
    let mut left_constraints = job.extra_constraints.clone();
    left_constraints.push(left);
    let mut right_constraints = job.extra_constraints.clone();
    right_constraints.push(right);
    let now = now_ms();
    let root = job_retry_root(&job.job_id);
    let child_problem = if job.problem_stored {
        None
    } else {
        job.problem.clone()
    };
    let children = vec![
        SubproblemJob {
            solve_id: job.solve_id.clone(),
            request_id: job.request_id.clone(),
            job_id: format!("{root}-split-d{next_depth}-left"),
            job_uuid: new_uuid_string(),
            problem_id: job.problem_id.clone(),
            problem_stored: job.problem_stored,
            revision: job.revision,
            depth: next_depth,
            master_node: job.master_node.clone(),
            problem: child_problem.clone(),
            extra_constraints: left_constraints,
            avoid_worker_nodes: job.avoid_worker_nodes.clone(),
            options: job.options.clone(),
            submitted_at_ms: now,
        },
        SubproblemJob {
            solve_id: job.solve_id.clone(),
            request_id: job.request_id.clone(),
            job_id: format!("{root}-split-d{next_depth}-right"),
            job_uuid: new_uuid_string(),
            problem_id: job.problem_id.clone(),
            problem_stored: job.problem_stored,
            revision: job.revision,
            depth: next_depth,
            master_node: job.master_node.clone(),
            problem: child_problem,
            extra_constraints: right_constraints,
            avoid_worker_nodes: job.avoid_worker_nodes.clone(),
            options: job.options.clone(),
            submitted_at_ms: now,
        },
    ];
    Ok(Some((
        children,
        format!(
            "subproblem {} split at depth {} on x{}={value:.6}",
            job.job_id, job.depth, var
        ),
    )))
}

fn build_frontier_jobs(
    problem: &MipProblemSpec,
    solve_id: &str,
    request_id: &str,
    problem_id: &str,
    revision: u64,
    master_node: &str,
    options: &SolveOptions,
    problem_stored: bool,
) -> Result<(Vec<SubproblemJob>, Vec<String>), String> {
    let split_depth = options.split_depth.unwrap_or(1).min(8);
    let lp_max_iters = options.lp_max_iters.unwrap_or(5_000);
    let int_tol = options.int_tol.unwrap_or(1e-6);
    let max_subproblems = options.max_subproblems.unwrap_or(256).clamp(1, 100_000);
    let mut warnings = Vec::new();
    let mut warned_frontier_cap = false;
    let mut queue = VecDeque::from([FrontierNode {
        depth: 0,
        extra_constraints: Vec::new(),
    }]);
    let mut jobs = Vec::new();

    while let Some(node) = queue.pop_front() {
        let relaxation = solve_lp_relaxation(problem, &node.extra_constraints, lp_max_iters)?;
        match relaxation.status {
            LPStatus::Infeasible => continue,
            LPStatus::NumericalError | LPStatus::IterLimit => {
                warnings.push(format!(
                    "LP relaxation at depth {} returned {}; keeping it as a subtree job",
                    node.depth,
                    relaxation.status.as_str()
                ));
            }
            LPStatus::Unbounded => {
                warnings.push(format!(
                    "LP relaxation at depth {} is unbounded; keeping it as a subtree job",
                    node.depth
                ));
            }
            LPStatus::Optimal => {}
        }

        if relaxation.status == LPStatus::Optimal && node.depth < split_depth {
            if let Some((var, value)) = first_fractional(problem, &relaxation.x, int_tol) {
                if jobs.len() + queue.len() + 2 <= max_subproblems {
                    let [left, right] = branch_constraints(var, value, problem.c.len(), node.depth);
                    let mut left_constraints = node.extra_constraints.clone();
                    left_constraints.push(left);
                    queue.push_back(FrontierNode {
                        depth: node.depth + 1,
                        extra_constraints: left_constraints,
                    });
                    let mut right_constraints = node.extra_constraints;
                    right_constraints.push(right);
                    queue.push_back(FrontierNode {
                        depth: node.depth + 1,
                        extra_constraints: right_constraints,
                    });
                    continue;
                }
                if !warned_frontier_cap {
                    warnings.push(format!(
                        "frontier split capped at {max_subproblems} subproblems; remaining fractional nodes will be delegated as subtree jobs"
                    ));
                    warned_frontier_cap = true;
                }
            }
        }

        let job_id = format!("{solve_id}-{}", jobs.len());
        jobs.push(SubproblemJob {
            solve_id: solve_id.to_string(),
            request_id: request_id.to_string(),
            job_id,
            job_uuid: new_uuid_string(),
            problem_id: Some(problem_id.to_string()),
            problem_stored,
            revision,
            depth: node.depth,
            master_node: master_node.to_string(),
            problem: if problem_stored {
                None
            } else {
                Some(problem.clone())
            },
            extra_constraints: node.extra_constraints,
            avoid_worker_nodes: Vec::new(),
            options: options.clone(),
            submitted_at_ms: now_ms(),
        });
    }

    Ok((jobs, warnings))
}

fn solve_subproblem(job: SubproblemJob, worker_node: String) -> SubproblemResult {
    let started = Instant::now();
    let mut accelerator = AcceleratorReport::runtime();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let problem = job.problem()?;
        let preprocess = preprocess_bounds(problem, &job.extra_constraints)?;
        accelerator = preprocess.accelerator;
        if let Some(reason) = preprocess.infeasible_reason {
            return Ok(SubproblemSolveOutcome::Pruned(reason));
        }
        if !is_pure_lp(problem)? {
            if let Some((children, reason)) = split_subproblem_children(&job)? {
                return Ok(SubproblemSolveOutcome::Split { children, reason });
            }
        }
        if is_pure_lp(problem)? {
            let lp = to_lp_problem(problem, &job.extra_constraints)?;
            let solution = solve_lp_internal(
                &lp,
                &InternalSimplexOptions {
                    max_iter: job.options.lp_max_iters,
                    tol: Some(1e-9),
                    basis_start: None,
                },
            );
            return Ok(SubproblemSolveOutcome::Lp {
                problem: lp,
                solution,
            });
        }
        let problem = to_ipmip_problem(problem, &job.extra_constraints)?;
        let solution = solve_ipmip_with_des(problem, job.options.to_ipmip_options());
        Ok::<_, String>(SubproblemSolveOutcome::IpMip(solution))
    }));

    match result {
        Ok(Ok(SubproblemSolveOutcome::IpMip(solution))) => SubproblemResult {
            solve_id: job.solve_id,
            request_id: job.request_id,
            job_id: job.job_id,
            job_uuid: job.job_uuid,
            problem_id: job.problem_id,
            revision: job.revision,
            worker_node,
            ok: solution.status == IPMIPStatus::Optimal || !solution.x.is_empty(),
            status: solution.status.as_str().to_string(),
            z: solution.z.is_finite().then_some(solution.z),
            x: solution.x,
            best_bound: solution
                .best_bound
                .is_finite()
                .then_some(solution.best_bound),
            gap: solution.gap.is_finite().then_some(solution.gap),
            lp: None,
            child_jobs: Vec::new(),
            nodes_explored: solution.nodes_explored,
            lp_solves: solution.lp_solves,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            accelerator,
            error: None,
            finished_at_ms: now_ms(),
        },
        Ok(Ok(SubproblemSolveOutcome::Split { children, reason })) => {
            split_subproblem(job, worker_node, accelerator, children, reason, started)
        }
        Ok(Ok(SubproblemSolveOutcome::Lp { problem, solution })) => {
            let optimal = solution.status == LPStatus::Optimal;
            let objective = solution.objective.is_finite().then_some(solution.objective);
            let error = if optimal {
                None
            } else {
                solution
                    .message
                    .clone()
                    .or_else(|| Some(format!("LP solve returned {}", solution.status.as_str())))
            };
            let lp = lp_report_from_solution(&problem, &solution);
            SubproblemResult {
                solve_id: job.solve_id,
                request_id: job.request_id,
                job_id: job.job_id,
                job_uuid: job.job_uuid,
                problem_id: job.problem_id,
                revision: job.revision,
                worker_node,
                ok: optimal,
                status: solution.status.as_str().to_string(),
                z: objective,
                x: solution.x,
                best_bound: if optimal { objective } else { None },
                gap: if optimal { Some(0.0) } else { None },
                lp: Some(lp),
                child_jobs: Vec::new(),
                nodes_explored: 1,
                lp_solves: 1,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                accelerator,
                error,
                finished_at_ms: now_ms(),
            }
        }
        Ok(Ok(SubproblemSolveOutcome::Pruned(reason))) => {
            infeasible_subproblem(job, worker_node, accelerator, reason, started)
        }
        Ok(Err(error)) => failed_subproblem(job, worker_node, accelerator, error, started),
        Err(_) => failed_subproblem(
            job,
            worker_node,
            accelerator,
            "solver panicked".to_string(),
            started,
        ),
    }
}

fn failed_subproblem(
    job: SubproblemJob,
    worker_node: String,
    accelerator: AcceleratorReport,
    error: String,
    started: Instant,
) -> SubproblemResult {
    SubproblemResult {
        solve_id: job.solve_id,
        request_id: job.request_id,
        job_id: job.job_id,
        job_uuid: job.job_uuid,
        problem_id: job.problem_id,
        revision: job.revision,
        worker_node,
        ok: false,
        status: "error".to_string(),
        z: None,
        x: Vec::new(),
        best_bound: None,
        gap: None,
        lp: None,
        child_jobs: Vec::new(),
        nodes_explored: 0,
        lp_solves: 0,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        accelerator,
        error: Some(error),
        finished_at_ms: now_ms(),
    }
}

fn split_subproblem(
    job: SubproblemJob,
    worker_node: String,
    accelerator: AcceleratorReport,
    children: Vec<SubproblemJob>,
    reason: String,
    started: Instant,
) -> SubproblemResult {
    SubproblemResult {
        solve_id: job.solve_id,
        request_id: job.request_id,
        job_id: job.job_id,
        job_uuid: job.job_uuid,
        problem_id: job.problem_id,
        revision: job.revision,
        worker_node,
        ok: false,
        status: "split".to_string(),
        z: None,
        x: Vec::new(),
        best_bound: None,
        gap: None,
        lp: None,
        child_jobs: children,
        nodes_explored: 0,
        lp_solves: 1,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        accelerator,
        error: Some(reason),
        finished_at_ms: now_ms(),
    }
}

fn infeasible_subproblem(
    job: SubproblemJob,
    worker_node: String,
    accelerator: AcceleratorReport,
    reason: String,
    started: Instant,
) -> SubproblemResult {
    SubproblemResult {
        solve_id: job.solve_id,
        request_id: job.request_id,
        job_id: job.job_id,
        job_uuid: job.job_uuid,
        problem_id: job.problem_id,
        revision: job.revision,
        worker_node,
        ok: false,
        status: "infeasible".to_string(),
        z: None,
        x: Vec::new(),
        best_bound: None,
        gap: None,
        lp: None,
        child_jobs: Vec::new(),
        nodes_explored: 0,
        lp_solves: 0,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        accelerator,
        error: Some(reason),
        finished_at_ms: now_ms(),
    }
}

fn job_retry_root(job_id: &str) -> &str {
    job_id
        .split_once("-retry-")
        .map_or(job_id, |(root, _)| root)
}

fn job_retry_index(job_id: &str) -> usize {
    job_id
        .rsplit_once("-retry-")
        .and_then(|(_, index)| index.parse::<usize>().ok())
        .unwrap_or(0)
}

fn redelegated_job(original: &SubproblemJob, retry_index: usize) -> SubproblemJob {
    let mut job = original.clone();
    job.job_id = format!("{}-retry-{retry_index}", job_retry_root(&original.job_id));
    job.job_uuid = new_uuid_string();
    job.submitted_at_ms = now_ms();
    job
}

fn should_redelegate_result(
    result: &SubproblemResult,
    retry_index: usize,
    max_retries: usize,
) -> bool {
    result.status == "error" && retry_index < max_retries
}

fn terminal_solve_status(status: &str) -> bool {
    matches!(
        status,
        "optimal" | "infeasible" | "unbounded" | "timeout" | "error" | "cancelled"
    )
}

fn resolve_solve_id(solves: &HashMap<String, SolveRegistryEntry>, key: &str) -> Option<String> {
    if solves.contains_key(key) {
        return Some(key.to_string());
    }
    newest_matching_solve_id(solves, |solve| solve.request_id == key).or_else(|| {
        let key_uuid = Uuid::parse_str(key).ok()?.to_string();
        newest_matching_solve_id(solves, |solve| {
            solve.problem_id.as_deref() == Some(key_uuid.as_str())
                || Uuid::parse_str(&solve.request_id)
                    .map(|request_uuid| request_uuid.to_string() == key_uuid)
                    .unwrap_or(false)
        })
    })
}

fn newest_matching_solve_id<F>(
    solves: &HashMap<String, SolveRegistryEntry>,
    mut matches: F,
) -> Option<String>
where
    F: FnMut(&SolveRegistryEntry) -> bool,
{
    solves
        .values()
        .filter(|solve| matches(solve))
        .max_by(|left, right| {
            let left_active = left.finished_at_ms.is_none();
            let right_active = right.finished_at_ms.is_none();
            left_active
                .cmp(&right_active)
                .then_with(|| left.started_at_ms.cmp(&right.started_at_ms))
                .then_with(|| left.solve_id.cmp(&right.solve_id))
        })
        .map(|solve| solve.solve_id.clone())
}

fn solve_cancel_info(state: &AppState, solve_id: &str) -> Option<CancelInfo> {
    state
        .cancelled_solves
        .lock()
        .expect("cancelled solves mutex poisoned")
        .get(solve_id)
        .cloned()
}

fn solve_cancel_requested(state: &AppState, solve_id: &str) -> bool {
    solve_cancel_info(state, solve_id).is_some()
}

fn solve_cancel_info_for(
    state: &AppState,
    solve_id: &str,
    problem_id: Option<&str>,
) -> Option<CancelInfo> {
    solve_cancel_info(state, solve_id)
        .or_else(|| problem_id.and_then(|problem_id| solve_cancel_info(state, problem_id)))
}

fn solve_cancel_requested_for(state: &AppState, solve_id: &str, problem_id: Option<&str>) -> bool {
    solve_cancel_info_for(state, solve_id, problem_id).is_some()
}

fn request_solve_cancel(
    state: &AppState,
    key: &str,
    reason: String,
    requested_by: String,
) -> Result<CancelInfo, String> {
    let now = now_ms();
    let mut solves = state.solves.lock().expect("solves mutex poisoned");
    let solve_id = resolve_solve_id(&solves, key)
        .ok_or_else(|| format!("solve or request id not found: {key}"))?;
    let Some(solve) = solves.get_mut(&solve_id) else {
        return Err(format!("solve not found after lookup: {solve_id}"));
    };
    if terminal_solve_status(&solve.status) && solve.status != "cancelled" {
        return Err(format!(
            "solve {} is already terminal with status {}",
            solve.solve_id, solve.status
        ));
    }

    solve.cancel_requested = true;
    solve.cancel_requested_at_ms.get_or_insert(now);
    solve.cancel_reason.get_or_insert_with(|| reason.clone());
    if solve.finished_at_ms.is_none() {
        solve.status = "cancelling".to_string();
    }
    solve.updated_at_ms = now;
    let info = CancelInfo {
        solve_id: solve.solve_id.clone(),
        request_id: Some(solve.request_id.clone()),
        problem_id: solve.problem_id.clone(),
        reason,
        requested_by,
        requested_at_ms: now,
    };
    drop(solves);

    state
        .cancelled_solves
        .lock()
        .expect("cancelled solves mutex poisoned")
        .insert(solve_id, info.clone());
    state
        .metrics
        .solve_cancel_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Ok(info)
}

fn cleanup_finished_solve(
    state: &AppState,
    key: &str,
) -> Result<Option<SolveRegistryEntry>, String> {
    let mut solves = state.solves.lock().expect("solves mutex poisoned");
    let Some(solve_id) = resolve_solve_id(&solves, key) else {
        return Err(format!("solve or request id not found: {key}"));
    };
    let finished = solves
        .get(&solve_id)
        .and_then(|solve| solve.finished_at_ms)
        .is_some();
    if !finished {
        return Ok(None);
    }
    let removed = solves.remove(&solve_id);
    drop(solves);
    state
        .cancelled_solves
        .lock()
        .expect("cancelled solves mutex poisoned")
        .remove(&solve_id);
    Ok(removed)
}

fn track_solve_cancelled(
    state: &AppState,
    solve_id: &str,
    reason: &str,
) -> Option<SolveRegistryEntry> {
    let now = now_ms();
    let mut solves = state.solves.lock().expect("solves mutex poisoned");
    let solve = solves.get_mut(solve_id)?;
    solve.status = "cancelled".to_string();
    solve.cancel_requested = true;
    solve.cancel_requested_at_ms.get_or_insert(now);
    solve
        .cancel_reason
        .get_or_insert_with(|| reason.to_string());
    solve.updated_at_ms = now;
    solve.finished_at_ms = Some(now);
    for job in solve.jobs.values_mut() {
        if job.finished_at_ms.is_none() {
            job.status = "cancelled".to_string();
            job.finished_at_ms = Some(now);
            job.error.get_or_insert_with(|| reason.to_string());
        }
    }
    Some(solve.clone())
}

fn cancelled_solve_response(
    state: &AppState,
    solve_id: String,
    request_id: String,
    problem_id: Option<String>,
    revision: u64,
    distributed: bool,
    mut warnings: Vec<String>,
    reason: String,
) -> SolveResponse {
    warnings.push(format!("solve cancelled: {reason}"));
    let entry = track_solve_cancelled(state, &solve_id, &reason);
    let (jobs_expected, jobs_published, jobs_completed, jobs_redelegated, jobs_split) = entry
        .as_ref()
        .map(|entry| {
            (
                entry.jobs_expected,
                entry.jobs_published,
                entry.jobs_completed,
                entry.jobs_redelegated,
                entry.jobs_split,
            )
        })
        .unwrap_or_default();
    SolveResponse {
        ok: false,
        solve_id,
        request_id,
        problem_id,
        status: "cancelled".to_string(),
        revision,
        z: None,
        x: Vec::new(),
        best_bound: None,
        gap: None,
        lp: None,
        jobs_expected,
        jobs_published,
        jobs_completed,
        jobs_redelegated,
        jobs_split,
        timed_out: false,
        distributed,
        node_id: state.node_id.clone(),
        role: state.role,
        gpu: aggregate_gpu_status(&[]),
        external_verification: None,
        warnings,
        generated_at_ms: now_ms(),
    }
}

fn track_solve_started(
    state: &AppState,
    solve_id: &str,
    request_id: &str,
    problem_id: &str,
    revision: u64,
    jobs_expected: usize,
    distributed: bool,
) -> Result<(), String> {
    let now = now_ms();
    let mut solves = state.solves.lock().expect("solves mutex poisoned");
    if let Some(active) = solves.values().find(|solve| {
        solve.finished_at_ms.is_none() && solve.problem_id.as_deref() == Some(problem_id)
    }) {
        return Err(format!(
            "problemId {problem_id} already has running solve {}",
            active.solve_id
        ));
    }
    solves.insert(
        solve_id.to_string(),
        SolveRegistryEntry {
            solve_id: solve_id.to_string(),
            request_id: request_id.to_string(),
            problem_id: Some(problem_id.to_string()),
            revision,
            status: "running".to_string(),
            jobs_expected,
            distributed,
            started_at_ms: now,
            updated_at_ms: now,
            ..SolveRegistryEntry::default()
        },
    );
    Ok(())
}

fn track_job_submitted(state: &AppState, job: &SubproblemJob) {
    let retry_index = job_retry_index(&job.job_id);
    let mut solves = state.solves.lock().expect("solves mutex poisoned");
    let Some(solve) = solves.get_mut(&job.solve_id) else {
        return;
    };
    solve.jobs_published = solve.jobs_published.saturating_add(1);
    solve.updated_at_ms = now_ms();
    solve.jobs.insert(
        job.job_id.clone(),
        JobRegistryEntry {
            job_id: job.job_id.clone(),
            job_uuid: Some(job.job_uuid.clone()),
            problem_id: job.problem_id.clone(),
            root_job_id: job_retry_root(&job.job_id).to_string(),
            retry_index,
            depth: job.depth,
            status: "submitted".to_string(),
            submitted_at_ms: job.submitted_at_ms,
            ..JobRegistryEntry::default()
        },
    );
}

fn track_job_result(state: &AppState, result: &SubproblemResult, terminal: bool) {
    let mut solves = state.solves.lock().expect("solves mutex poisoned");
    let Some(solve) = solves.get_mut(&result.solve_id) else {
        return;
    };
    let job = solve
        .jobs
        .entry(result.job_id.clone())
        .or_insert_with(|| JobRegistryEntry {
            job_id: result.job_id.clone(),
            job_uuid: Some(result.job_uuid.clone()),
            problem_id: result.problem_id.clone(),
            root_job_id: job_retry_root(&result.job_id).to_string(),
            retry_index: job_retry_index(&result.job_id),
            ..JobRegistryEntry::default()
        });
    job.job_uuid = Some(result.job_uuid.clone());
    job.problem_id = result.problem_id.clone();
    job.status = if terminal || result.status == "split" {
        result.status.clone()
    } else {
        "retrying".to_string()
    };
    job.worker_node = Some(result.worker_node.clone());
    job.finished_at_ms = Some(result.finished_at_ms);
    job.error = result.error.clone();
    solve.updated_at_ms = now_ms();
    if terminal {
        solve.jobs_completed = solve.jobs_completed.saturating_add(1);
    }
}

fn track_job_stale_requeued(
    state: &AppState,
    solve_id: &str,
    job_id: &str,
    worker_node: &str,
    retry_job_id: &str,
    last_heartbeat_ms: u128,
) {
    let mut solves = state.solves.lock().expect("solves mutex poisoned");
    let Some(solve) = solves.get_mut(solve_id) else {
        return;
    };
    let Some(job) = solve.jobs.get_mut(job_id) else {
        return;
    };
    job.status = "stale-requeued".to_string();
    job.worker_node = Some(worker_node.to_string());
    job.error = Some(format!(
        "worker {worker_node} missed heartbeat since {last_heartbeat_ms}; requeued as {retry_job_id}"
    ));
    job.finished_at_ms = None;
    solve.updated_at_ms = now_ms();
}

fn stale_worker_jobs(
    state: &AppState,
    solve_id: &str,
    active_job_ids: &HashSet<String>,
    completed_job_ids: &HashSet<String>,
    stale_after: Duration,
    now: u128,
) -> Vec<StaleWorkerJob> {
    let stale_after_ms = stale_after.as_millis();
    let solves = state.solves.lock().expect("solves mutex poisoned");
    let Some(solve) = solves.get(solve_id) else {
        return Vec::new();
    };
    solve
        .jobs
        .values()
        .filter(|job| {
            active_job_ids.contains(&job.job_id)
                && !completed_job_ids.contains(&job.job_id)
                && job.status == "running"
        })
        .filter_map(|job| {
            let worker_node = job.worker_node.clone()?;
            let last_heartbeat_ms = job.last_heartbeat_ms?;
            (now.saturating_sub(last_heartbeat_ms) > stale_after_ms).then(|| StaleWorkerJob {
                job_id: job.job_id.clone(),
                job_uuid: job.job_uuid.clone(),
                worker_node,
                last_heartbeat_ms,
            })
        })
        .collect()
}

fn track_job_redelegated(state: &AppState, solve_id: &str) {
    let mut solves = state.solves.lock().expect("solves mutex poisoned");
    let Some(solve) = solves.get_mut(solve_id) else {
        return;
    };
    solve.jobs_redelegated = solve.jobs_redelegated.saturating_add(1);
    solve.updated_at_ms = now_ms();
}

fn track_job_split(state: &AppState, solve_id: &str, child_count: usize) {
    let mut solves = state.solves.lock().expect("solves mutex poisoned");
    let Some(solve) = solves.get_mut(solve_id) else {
        return;
    };
    solve.jobs_split = solve.jobs_split.saturating_add(1);
    solve.jobs_expected = solve
        .jobs_expected
        .saturating_add(child_count.saturating_sub(1));
    solve.updated_at_ms = now_ms();
}

fn track_solve_finished(state: &AppState, response: &SolveResponse) {
    let mut solves = state.solves.lock().expect("solves mutex poisoned");
    let Some(solve) = solves.get_mut(&response.solve_id) else {
        return;
    };
    solve.status = response.status.clone();
    if response.problem_id.is_some() {
        solve.problem_id = response.problem_id.clone();
    }
    solve.jobs_expected = response.jobs_expected;
    solve.jobs_published = response.jobs_published;
    solve.jobs_completed = response.jobs_completed;
    solve.jobs_redelegated = response.jobs_redelegated;
    solve.jobs_split = response.jobs_split;
    solve.timed_out = response.timed_out;
    solve.warnings = response.warnings.clone();
    solve.updated_at_ms = response.generated_at_ms;
    solve.finished_at_ms = Some(response.generated_at_ms);
}

fn runtime_task_retention_ms() -> u128 {
    u128::from(env_u64("MIP_SOLVER_TASK_RETENTION_SECONDS", 3_600)) * 1_000
}

fn prune_runtime_tasks_locked(tasks: &mut HashMap<String, RuntimeTaskRecord>) {
    let now = now_ms();
    let retention_ms = runtime_task_retention_ms();
    tasks.retain(|_, record| {
        record
            .entry
            .finished_at_ms
            .map(|finished| now.saturating_sub(finished) <= retention_ms)
            .unwrap_or(true)
    });
}

#[allow(clippy::too_many_arguments)]
fn track_runtime_task_started(
    state: &AppState,
    task_id: String,
    kind: &str,
    problem_id: Option<String>,
    solve_id: Option<String>,
    request_id: Option<String>,
    job_id: Option<String>,
    job_uuid: Option<String>,
    abort_handle: Option<tokio::task::AbortHandle>,
) {
    let now = now_ms();
    let mut tasks = state.tasks.lock().expect("runtime tasks mutex poisoned");
    prune_runtime_tasks_locked(&mut tasks);
    let abortable = abort_handle.is_some();
    tasks.insert(
        task_id.clone(),
        RuntimeTaskRecord {
            entry: RuntimeTaskEntry {
                task_id,
                kind: kind.to_string(),
                node_id: state.node_id.clone(),
                role: state.role,
                status: "running".to_string(),
                problem_id,
                solve_id,
                request_id,
                job_id,
                job_uuid,
                abortable,
                started_at_ms: now,
                updated_at_ms: now,
                finished_at_ms: None,
            },
            abort_handle,
        },
    );
}

fn track_runtime_task_solve(state: &AppState, task_id: &str, solve_id: &str) {
    let mut tasks = state.tasks.lock().expect("runtime tasks mutex poisoned");
    if let Some(record) = tasks.get_mut(task_id) {
        record.entry.solve_id = Some(solve_id.to_string());
        record.entry.updated_at_ms = now_ms();
    }
}

fn track_runtime_task_abort_handle(
    state: &AppState,
    task_id: &str,
    abort_handle: tokio::task::AbortHandle,
) {
    let mut tasks = state.tasks.lock().expect("runtime tasks mutex poisoned");
    if let Some(record) = tasks.get_mut(task_id) {
        record.abort_handle = Some(abort_handle);
        record.entry.abortable = true;
        record.entry.updated_at_ms = now_ms();
    }
}

fn track_runtime_task_finished(state: &AppState, task_id: &str, status: &str) {
    let mut tasks = state.tasks.lock().expect("runtime tasks mutex poisoned");
    if let Some(record) = tasks.get_mut(task_id) {
        let now = now_ms();
        record.entry.status = status.to_string();
        record.entry.updated_at_ms = now;
        record.entry.finished_at_ms.get_or_insert(now);
        record.abort_handle = None;
        record.entry.abortable = false;
    }
}

fn track_runtime_task_finished_if_open(state: &AppState, task_id: &str, status: &str) {
    let mut tasks = state.tasks.lock().expect("runtime tasks mutex poisoned");
    if let Some(record) = tasks.get_mut(task_id) {
        if record.entry.finished_at_ms.is_some() {
            return;
        }
        let now = now_ms();
        record.entry.status = status.to_string();
        record.entry.updated_at_ms = now;
        record.entry.finished_at_ms = Some(now);
        record.abort_handle = None;
        record.entry.abortable = false;
    }
}

fn runtime_task_matches(entry: &RuntimeTaskEntry, key: &str) -> bool {
    entry.task_id == key
        || entry.problem_id.as_deref() == Some(key)
        || entry.solve_id.as_deref() == Some(key)
        || entry.request_id.as_deref() == Some(key)
        || entry.job_id.as_deref() == Some(key)
        || entry.job_uuid.as_deref() == Some(key)
}

fn runtime_task_entries(state: &AppState) -> Vec<RuntimeTaskEntry> {
    let mut tasks = state.tasks.lock().expect("runtime tasks mutex poisoned");
    prune_runtime_tasks_locked(&mut tasks);
    let mut entries: Vec<_> = tasks.values().map(|record| record.entry.clone()).collect();
    entries.sort_by(|left, right| {
        right
            .updated_at_ms
            .cmp(&left.updated_at_ms)
            .then_with(|| left.task_id.cmp(&right.task_id))
    });
    entries
}

fn runtime_task_lookup(state: &AppState, key: &str) -> Option<RuntimeTaskEntry> {
    let mut tasks = state.tasks.lock().expect("runtime tasks mutex poisoned");
    prune_runtime_tasks_locked(&mut tasks);
    if let Some(record) = tasks.get(key) {
        return Some(record.entry.clone());
    }
    tasks
        .values()
        .find(|record| runtime_task_matches(&record.entry, key))
        .map(|record| record.entry.clone())
}

struct RuntimeTaskFinishGuard {
    state: AppState,
    task_id: String,
    default_status: &'static str,
}

impl RuntimeTaskFinishGuard {
    fn new(state: AppState, task_id: String, default_status: &'static str) -> Self {
        RuntimeTaskFinishGuard {
            state,
            task_id,
            default_status,
        }
    }
}

impl Drop for RuntimeTaskFinishGuard {
    fn drop(&mut self) {
        track_runtime_task_finished_if_open(&self.state, &self.task_id, self.default_status);
    }
}

fn aggregate_results(
    solve_id: String,
    request_id: String,
    problem_id: Option<String>,
    revision: u64,
    problem: &MipProblemSpec,
    options: &SolveOptions,
    jobs_expected: usize,
    jobs_published: usize,
    jobs_redelegated: usize,
    jobs_split: usize,
    results: Vec<SubproblemResult>,
    timed_out: bool,
    distributed: bool,
    state: &AppState,
    mut warnings: Vec<String>,
) -> SolveResponse {
    let maximize = sense_of(&problem.sense) == Sense::Max;
    let mut feasible: Vec<&SubproblemResult> = results
        .iter()
        .filter(|result| result.ok && result.z.is_some() && !result.x.is_empty())
        .collect();
    feasible.sort_by(|left, right| {
        let lz = left.z.unwrap_or(f64::NAN);
        let rz = right.z.unwrap_or(f64::NAN);
        if maximize {
            rz.total_cmp(&lz)
        } else {
            lz.total_cmp(&rz)
        }
    });
    let best = feasible.first().copied();
    let best_bound = if maximize {
        results.iter().filter_map(|r| r.best_bound).reduce(f64::max)
    } else {
        results.iter().filter_map(|r| r.best_bound).reduce(f64::min)
    };
    let z = best.and_then(|r| r.z);
    let gap = match (z, best_bound) {
        (Some(z), Some(bound)) => Some((bound - z).abs() / 1.0_f64.max(z.abs())),
        _ => None,
    };
    if timed_out {
        warnings.push("solve timed out before every subproblem result returned".to_string());
    }
    let all_finished = results.len() == jobs_expected && !timed_out;
    let all_terminal = all_finished
        && results
            .iter()
            .all(|result| matches!(result.status.as_str(), "optimal" | "infeasible"));
    let has_error = results.iter().any(|result| {
        matches!(
            result.status.as_str(),
            "error" | "iter-limit" | "numerical-error"
        )
    });
    let status = if best.is_some() && all_terminal {
        "optimal"
    } else if best.is_some() {
        "feasible-partial"
    } else if results.iter().any(|result| result.status == "unbounded") {
        "unbounded"
    } else if timed_out {
        "timeout"
    } else if has_error {
        "error"
    } else {
        "infeasible"
    };
    let verification = external_verification_report(
        problem,
        options,
        status,
        z,
        best.map(|r| r.x.as_slice()).unwrap_or(&[]),
    );
    let external_verification = verification.as_ref().map(|(report, _)| report.clone());
    if let Some((_, Some(warning))) = verification {
        warnings.push(warning);
    }

    SolveResponse {
        ok: best.is_some() || status == "infeasible",
        solve_id,
        request_id,
        problem_id,
        status: status.to_string(),
        revision,
        z,
        x: best.map(|r| r.x.clone()).unwrap_or_default(),
        best_bound,
        gap,
        lp: best.and_then(|r| r.lp.clone()),
        jobs_expected,
        jobs_published,
        jobs_completed: results.len(),
        jobs_redelegated,
        jobs_split,
        timed_out,
        distributed,
        node_id: state.node_id.clone(),
        role: state.role,
        gpu: aggregate_gpu_status(&results),
        external_verification,
        warnings,
        generated_at_ms: now_ms(),
    }
}

fn external_verification_requested(options: &SolveOptions) -> bool {
    options.verify_external.unwrap_or(false)
}

fn external_verification_tolerance(options: &SolveOptions) -> f64 {
    options
        .external_verification_tolerance
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(1e-6)
}

fn external_verification_method(options: &SolveOptions) -> String {
    options
        .external_verification_method
        .clone()
        .unwrap_or_else(|| "highs".to_string())
}

#[cfg(not(feature = "external-solver-verification"))]
fn external_verification_report(
    _problem: &MipProblemSpec,
    options: &SolveOptions,
    _status: &str,
    _z: Option<f64>,
    _x: &[f64],
) -> Option<(ExternalVerificationReport, Option<String>)> {
    if !external_verification_requested(options) {
        return None;
    }
    let tolerance = external_verification_tolerance(options);
    let report = ExternalVerificationReport {
        requested: true,
        enabled: false,
        status: "unavailable".to_string(),
        method: Some(external_verification_method(options)),
        solver: None,
        solution_status: None,
        objective: None,
        objective_delta: None,
        tolerance,
        elapsed_ms: 0.0,
        message: Some("compiled without the external-solver-verification feature".to_string()),
    };
    Some((
        report,
        Some("external solver verification requested but feature is not enabled".to_string()),
    ))
}

#[cfg(feature = "external-solver-verification")]
fn external_verification_report(
    problem: &MipProblemSpec,
    options: &SolveOptions,
    status: &str,
    z: Option<f64>,
    x: &[f64],
) -> Option<(ExternalVerificationReport, Option<String>)> {
    if !external_verification_requested(options) {
        return None;
    }
    let started = Instant::now();
    let tolerance = external_verification_tolerance(options);
    let method = external_verification_method(options);
    if status != "optimal" || z.is_none() || x.is_empty() {
        let report = ExternalVerificationReport {
            requested: true,
            enabled: true,
            status: "skipped".to_string(),
            method: Some(method),
            solver: None,
            solution_status: Some(status.to_string()),
            objective: None,
            objective_delta: None,
            tolerance,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            message: Some("external verification requires an optimal incumbent".to_string()),
        };
        return Some((report, None));
    }

    let solved = catch_unwind(AssertUnwindSafe(|| {
        run_external_verification(problem, options, z.unwrap(), &method, tolerance)
    }));
    match solved {
        Ok(Ok(report)) => {
            let warning = match report.status.as_str() {
                "verified" => None,
                "mismatch" => Some(format!(
                    "external solver verification mismatch: solver objective {:?}, in-house objective {:?}, delta {:?}, tolerance {}",
                    report.objective, z, report.objective_delta, report.tolerance
                )),
                _ => Some(format!(
                    "external solver verification did not verify result: {}{}",
                    report.status,
                    report
                        .message
                        .as_ref()
                        .map(|message| format!(" ({message})"))
                        .unwrap_or_default()
                )),
            };
            Some((report, warning))
        }
        Ok(Err(message)) => {
            let report = ExternalVerificationReport {
                requested: true,
                enabled: true,
                status: "error".to_string(),
                method: Some(method),
                solver: None,
                solution_status: None,
                objective: None,
                objective_delta: None,
                tolerance,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                message: Some(message.clone()),
            };
            Some((
                report,
                Some(format!("external solver verification failed: {message}")),
            ))
        }
        Err(_) => {
            let report = ExternalVerificationReport {
                requested: true,
                enabled: true,
                status: "error".to_string(),
                method: Some(method),
                solver: None,
                solution_status: None,
                objective: None,
                objective_delta: None,
                tolerance,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                message: Some("external verifier panicked".to_string()),
            };
            Some((
                report,
                Some("external solver verification panicked".to_string()),
            ))
        }
    }
}

#[cfg(feature = "external-solver-verification")]
fn run_external_verification(
    problem: &MipProblemSpec,
    options: &SolveOptions,
    expected_z: f64,
    method: &str,
    tolerance: f64,
) -> Result<ExternalVerificationReport, String> {
    let started = Instant::now();
    let problem = normalized_problem(problem.clone())?;
    if problem.integer_vars.iter().any(|integer| *integer) {
        verify_mip_with_external_des(&problem, options, expected_z, method, tolerance, started)
    } else {
        verify_lp_with_external_des(&problem, expected_z, method, tolerance, started)
    }
}

#[cfg(feature = "external-solver-verification")]
fn verify_lp_with_external_des(
    problem: &MipProblemSpec,
    expected_z: f64,
    method: &str,
    tolerance: f64,
    started: Instant,
) -> Result<ExternalVerificationReport, String> {
    let lp = external_lp_problem(problem)?;
    let lp_method = method.trim().strip_prefix("external-").unwrap_or(method);
    let solution = external_des_general::lp::solve_lp_external(
        &lp,
        &external_des_general::lp::ExternalSolverOptions {
            method: Some(lp_method.to_string()),
            ..Default::default()
        },
    );
    let objective = solution.objective.is_finite().then_some(solution.objective);
    let objective_delta = objective.map(|value| (value - expected_z).abs());
    let status = if solution.status == external_des_general::lp::LPStatus::Optimal {
        if objective_delta.is_some_and(|delta| delta <= tolerance) {
            "verified"
        } else {
            "mismatch"
        }
    } else {
        "unverified"
    };
    Ok(ExternalVerificationReport {
        requested: true,
        enabled: true,
        status: status.to_string(),
        method: Some(lp_method.to_string()),
        solver: Some(solution.solver),
        solution_status: Some(solution.status.as_str().to_string()),
        objective,
        objective_delta,
        tolerance,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        message: solution.message,
    })
}

#[cfg(feature = "external-solver-verification")]
fn verify_mip_with_external_des(
    problem: &MipProblemSpec,
    options: &SolveOptions,
    expected_z: f64,
    method: &str,
    tolerance: f64,
    started: Instant,
) -> Result<ExternalVerificationReport, String> {
    let external_problem = external_ipmip_problem(problem)?;
    let solution = external_des_general::ip_mip_des::solve_ipmip_with_des(
        external_problem,
        external_des_ipmip_options(options, method),
    );
    let objective = solution.z.is_finite().then_some(solution.z);
    let objective_delta = objective.map(|value| (value - expected_z).abs());
    let status = if solution.status == external_des_general::ip_mip_des::IPMIPStatus::Optimal {
        if objective_delta.is_some_and(|delta| delta <= tolerance) {
            "verified"
        } else {
            "mismatch"
        }
    } else {
        "unverified"
    };
    Ok(ExternalVerificationReport {
        requested: true,
        enabled: true,
        status: status.to_string(),
        method: Some(method.to_string()),
        solver: Some("des-ipmip-external-lp".to_string()),
        solution_status: Some(solution.status.as_str().to_string()),
        objective,
        objective_delta,
        tolerance,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        message: Some(format!(
            "lpAlgorithm={}, usesExternalSolvers={}",
            solution.lp_algorithm.as_str(),
            solution.uses_external_solvers
        )),
    })
}

#[cfg(feature = "external-solver-verification")]
fn external_sense(sense: &str) -> external_des_general::lp::Sense {
    if sense.eq_ignore_ascii_case("min") {
        external_des_general::lp::Sense::Min
    } else {
        external_des_general::lp::Sense::Max
    }
}

#[cfg(feature = "external-solver-verification")]
fn external_lp_problem(
    problem: &MipProblemSpec,
) -> Result<external_des_general::lp::LPProblem, String> {
    Ok(external_des_general::lp::LPProblem {
        sense: external_sense(&problem.sense),
        c: problem.c.clone(),
        a_ub: Some(problem.a.clone()),
        b_ub: Some(problem.b.clone()),
        a_eq: None,
        b_eq: None,
        lb: Some(vec![Some(0.0); problem.c.len()]),
        ub: problem
            .ub
            .clone()
            .map(|ub| ub.into_iter().map(|v| v.is_finite().then_some(v)).collect()),
        var_names: problem.var_names.clone(),
        con_names: problem.con_names.clone(),
    })
}

#[cfg(feature = "external-solver-verification")]
fn external_ipmip_problem(
    problem: &MipProblemSpec,
) -> Result<external_des_general::ip_mip_des::IPMIPProblem, String> {
    Ok(external_des_general::ip_mip_des::IPMIPProblem {
        sense: external_sense(&problem.sense),
        c: problem.c.clone(),
        a: problem.a.clone(),
        b: problem.b.clone(),
        integer_vars: problem.integer_vars.clone(),
        ub: problem.ub.clone(),
        var_names: problem.var_names.clone(),
        con_names: problem.con_names.clone(),
        lazy_constraints: None,
        variable_nodes: None,
        constraint_nodes: None,
    })
}

#[cfg(feature = "external-solver-verification")]
fn external_des_ipmip_options(
    options: &SolveOptions,
    method: &str,
) -> external_des_general::ip_mip_des::IPMIPSolveOptions {
    external_des_general::ip_mip_des::IPMIPSolveOptions {
        max_nodes: options.max_nodes,
        max_ticks: options.max_ticks,
        lp_max_iters: options.lp_max_iters,
        int_tol: options.int_tol,
        branch_rule: Some(external_des_general::ip_mip_des::BranchRule::MostFractional),
        lp_algorithm: Some(
            external_des_general::ip_mip_des::LpRelaxationAlgorithm::Concrete(
                external_des_ipmip_algorithm(method),
            ),
        ),
        allow_external_solvers: Some(true),
        max_cut_rounds: Some(8),
        max_cuts_per_node: Some(16),
        heuristic_passes: Some(2),
        verbose: Some(false),
        ..Default::default()
    }
}

#[cfg(feature = "external-solver-verification")]
fn external_des_ipmip_algorithm(
    method: &str,
) -> external_des_general::ip_mip_des::ConcreteLpRelaxationAlgorithm {
    let normalized = method.trim().to_ascii_lowercase().replace('_', "-");
    if normalized.contains("ipm") {
        external_des_general::ip_mip_des::ConcreteLpRelaxationAlgorithm::ExternalHighsIpm
    } else if normalized.contains("ds") || normalized.contains("dual") {
        external_des_general::ip_mip_des::ConcreteLpRelaxationAlgorithm::ExternalHighsDs
    } else {
        external_des_general::ip_mip_des::ConcreteLpRelaxationAlgorithm::ExternalHighs
    }
}

fn accept_subproblem_result(
    result: SubproblemResult,
    solve_id: &str,
    expected_job_ids: &HashSet<String>,
    completed_job_ids: &mut HashSet<String>,
) -> Result<Option<SubproblemResult>, String> {
    if result.solve_id != solve_id {
        return Ok(None);
    }
    if !expected_job_ids.contains(&result.job_id) {
        return Err(format!("ignored result for unknown job {}", result.job_id));
    }
    if !completed_job_ids.insert(result.job_id.clone()) {
        return Err(format!(
            "ignored duplicate result for job {}",
            result.job_id
        ));
    }
    Ok(Some(result))
}

fn i32_count(value: usize) -> i32 {
    value.min(i32::MAX as usize) as i32
}

fn json_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or_else(|_| json!({}))
}

async fn record_pg_result(
    state: &AppState,
    label: &str,
    result: Result<sqlx::postgres::PgQueryResult, sqlx::Error>,
) -> bool {
    match result {
        Ok(_) => true,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!("mip solver postgres {label} failed: {error}");
            false
        }
    }
}

async fn persist_event(
    state: &AppState,
    event_kind: &str,
    solve_id: Option<&str>,
    session_id: Option<&str>,
    job_id: Option<&str>,
    payload: Value,
) {
    let Some(pool) = &state.pg else {
        return;
    };
    let sql = format!(
        "insert into {} (solve_id, session_id, job_id, event_kind, payload) values ($1, $2, $3, $4, $5)",
        MIP_SOLVER_EVENTS_TABLE
    );
    record_pg_result(
        state,
        "insert event",
        sqlx::query(&sql)
            .bind(solve_id.map(str::to_string))
            .bind(session_id.map(str::to_string))
            .bind(job_id.map(str::to_string))
            .bind(event_kind)
            .bind(payload)
            .execute(pool)
            .await,
    )
    .await;
}

async fn persist_solve_started(
    state: &AppState,
    solve_id: &str,
    request_id: &str,
    revision: u64,
    problem: &MipProblemSpec,
    options: &SolveOptions,
    jobs_expected: usize,
    distributed: bool,
) {
    let Some(pool) = &state.pg else {
        return;
    };
    let problem_json = json_value(problem);
    let options_json = json_value(options);
    let sql = format!(
        concat!(
            "insert into {} (solve_id, request_id, revision, status, node_id, node_role, problem, options, ",
            "jobs_expected, distributed, updated_at) values ($1, $2, $3, 'running', $4, $5, $6, $7, $8, $9, now()) ",
            "on conflict (solve_id) do update set request_id = excluded.request_id, revision = excluded.revision, ",
            "status = excluded.status, node_id = excluded.node_id, node_role = excluded.node_role, problem = excluded.problem, ",
            "options = excluded.options, jobs_expected = excluded.jobs_expected, distributed = excluded.distributed, updated_at = now()"
        ),
        MIP_SOLVER_SOLVES_TABLE
    );
    let wrote = record_pg_result(
        state,
        "upsert solve start",
        sqlx::query(&sql)
            .bind(solve_id)
            .bind(request_id)
            .bind(revision as i64)
            .bind(&state.node_id)
            .bind(state.role.as_str())
            .bind(problem_json)
            .bind(options_json)
            .bind(i32_count(jobs_expected))
            .bind(distributed)
            .execute(pool)
            .await,
    )
    .await;
    if wrote {
        persist_event(
            state,
            "mip-solver.solve-started",
            Some(solve_id),
            None,
            None,
            json!({
                "requestId": request_id,
                "revision": revision,
                "jobsExpected": jobs_expected,
                "distributed": distributed,
            }),
        )
        .await;
    }
}

async fn persist_solve_registry_entry(state: &AppState, solve_id: &str) {
    let Some(pool) = &state.pg else {
        return;
    };
    let entry = state
        .solves
        .lock()
        .expect("solves mutex poisoned")
        .get(solve_id)
        .cloned();
    let Some(entry) = entry else {
        return;
    };
    let warnings = json_value(&entry.warnings);
    let sql = format!(
        concat!(
            "update {} set status = $2, jobs_expected = $3, jobs_published = $4, jobs_completed = $5, ",
            "jobs_redelegated = $6, jobs_split = $7, timed_out = $8, distributed = $9, warnings = $10, ",
            "updated_at = now(), finished_at = case when $11 then coalesce(finished_at, now()) else finished_at end ",
            "where solve_id = $1"
        ),
        MIP_SOLVER_SOLVES_TABLE
    );
    record_pg_result(
        state,
        "update solve registry",
        sqlx::query(&sql)
            .bind(&entry.solve_id)
            .bind(&entry.status)
            .bind(i32_count(entry.jobs_expected))
            .bind(i32_count(entry.jobs_published))
            .bind(i32_count(entry.jobs_completed))
            .bind(i32_count(entry.jobs_redelegated))
            .bind(i32_count(entry.jobs_split))
            .bind(entry.timed_out)
            .bind(entry.distributed)
            .bind(warnings)
            .bind(entry.finished_at_ms.is_some())
            .execute(pool)
            .await,
    )
    .await;
}

async fn persist_solve_response(state: &AppState, response: &SolveResponse) {
    let Some(pool) = &state.pg else {
        return;
    };
    let response_json = json_value(response);
    let warnings = json_value(&response.warnings);
    let sql = format!(
        concat!(
            "update {} set status = $2, response = $3, jobs_expected = $4, jobs_published = $5, ",
            "jobs_completed = $6, jobs_redelegated = $7, jobs_split = $8, timed_out = $9, distributed = $10, ",
            "warnings = $11, updated_at = now(), finished_at = now() where solve_id = $1"
        ),
        MIP_SOLVER_SOLVES_TABLE
    );
    let wrote = record_pg_result(
        state,
        "update solve response",
        sqlx::query(&sql)
            .bind(&response.solve_id)
            .bind(&response.status)
            .bind(response_json)
            .bind(i32_count(response.jobs_expected))
            .bind(i32_count(response.jobs_published))
            .bind(i32_count(response.jobs_completed))
            .bind(i32_count(response.jobs_redelegated))
            .bind(i32_count(response.jobs_split))
            .bind(response.timed_out)
            .bind(response.distributed)
            .bind(warnings)
            .execute(pool)
            .await,
    )
    .await;
    if wrote {
        persist_event(
            state,
            "mip-solver.solve-finished",
            Some(&response.solve_id),
            None,
            None,
            json!({
                "requestId": response.request_id,
                "status": response.status,
                "jobsExpected": response.jobs_expected,
                "jobsCompleted": response.jobs_completed,
                "jobsRedelegated": response.jobs_redelegated,
                "jobsSplit": response.jobs_split,
                "timedOut": response.timed_out,
            }),
        )
        .await;
    }
}

async fn persist_job_submitted(state: &AppState, job: &SubproblemJob) {
    let Some(pool) = &state.pg else {
        return;
    };
    let job_payload = json_value(job);
    let sql = format!(
        concat!(
            "insert into {} (job_id, solve_id, root_job_id, retry_index, depth, status, job_payload, updated_at) ",
            "values ($1, $2, $3, $4, $5, 'submitted', $6, now()) ",
            "on conflict (job_id) do update set status = excluded.status, job_payload = excluded.job_payload, ",
            "updated_at = now()"
        ),
        MIP_SOLVER_JOBS_TABLE
    );
    let wrote = record_pg_result(
        state,
        "upsert job submitted",
        sqlx::query(&sql)
            .bind(&job.job_id)
            .bind(&job.solve_id)
            .bind(job_retry_root(&job.job_id))
            .bind(i32_count(job_retry_index(&job.job_id)))
            .bind(i32_count(job.depth))
            .bind(job_payload)
            .execute(pool)
            .await,
    )
    .await;
    if wrote {
        persist_event(
            state,
            "mip-solver.subproblem-submitted",
            Some(&job.solve_id),
            None,
            Some(&job.job_id),
            json!({
                "requestId": job.request_id,
                "depth": job.depth,
                "retryIndex": job_retry_index(&job.job_id),
                "rootJobId": job_retry_root(&job.job_id),
            }),
        )
        .await;
    }
}

async fn persist_job_result(state: &AppState, result: &SubproblemResult, terminal: bool) {
    let Some(pool) = &state.pg else {
        return;
    };
    let status = if terminal || result.status == "split" {
        result.status.clone()
    } else {
        "retrying".to_string()
    };
    let result_payload = json_value(result);
    let sql = format!(
        concat!(
            "insert into {} (job_id, solve_id, root_job_id, retry_index, depth, status, worker_node, result_payload, finished_at, updated_at) ",
            "values ($1, $2, $3, $4, 0, $5, $6, $7, now(), now()) ",
            "on conflict (job_id) do update set status = excluded.status, worker_node = excluded.worker_node, ",
            "result_payload = excluded.result_payload, finished_at = excluded.finished_at, updated_at = now()"
        ),
        MIP_SOLVER_JOBS_TABLE
    );
    let wrote = record_pg_result(
        state,
        "upsert job result",
        sqlx::query(&sql)
            .bind(&result.job_id)
            .bind(&result.solve_id)
            .bind(job_retry_root(&result.job_id))
            .bind(i32_count(job_retry_index(&result.job_id)))
            .bind(&status)
            .bind(&result.worker_node)
            .bind(result_payload)
            .execute(pool)
            .await,
    )
    .await;
    if wrote {
        let event_kind = if result.status == "split" {
            "mip-solver.subproblem-split"
        } else if terminal {
            "mip-solver.subproblem-finished"
        } else {
            "mip-solver.subproblem-retrying"
        };
        persist_event(
            state,
            event_kind,
            Some(&result.solve_id),
            None,
            Some(&result.job_id),
            json!({
                "requestId": result.request_id,
                "status": result.status,
                "terminal": terminal,
                "workerNode": result.worker_node,
                "error": result.error,
                "childJobCount": result.child_jobs.len(),
            }),
        )
        .await;
    }
}

fn session_problem_from_json(problem: Value) -> Option<MipProblemSpec> {
    serde_json::from_value::<MipProblemSpec>(problem).ok()
}

fn remember_problem_model(
    state: &AppState,
    problem_id: &str,
    revision: u64,
    problem: MipProblemSpec,
) -> MipProblemSpec {
    state
        .problems
        .lock()
        .expect("problems mutex poisoned")
        .insert(
            local_problem_model_key(problem_id, revision),
            problem.clone(),
        );
    problem
}

fn ensure_local_problem_model(
    state: &AppState,
    problem_id: &str,
    revision: u64,
    problem: &MipProblemSpec,
) -> Result<ProblemStoreStatus, String> {
    let key = local_problem_model_key(problem_id, revision);
    let mut problems = state.problems.lock().expect("problems mutex poisoned");
    if let Some(existing) = problems.get(&key) {
        if existing == problem {
            return Ok(ProblemStoreStatus::Existing);
        }
        return Err(format!(
            "stored problem {problem_id} revision {revision} already exists with a different model"
        ));
    }
    problems.insert(key, problem.clone());
    Ok(ProblemStoreStatus::Created)
}

fn cached_problem_model(
    state: &AppState,
    problem_id: &str,
    revision: u64,
) -> Option<MipProblemSpec> {
    state
        .problems
        .lock()
        .expect("problems mutex poisoned")
        .get(&local_problem_model_key(problem_id, revision))
        .cloned()
}

fn problem_model_from_value(value: Value) -> Option<MipProblemSpec> {
    value
        .get("problem")
        .cloned()
        .and_then(session_problem_from_json)
        .and_then(|problem| normalized_problem(problem).ok())
}

async fn load_redis_problem_model(
    state: &AppState,
    problem_id: &str,
    revision: u64,
) -> Option<MipProblemSpec> {
    let prefix = redis_key_prefix();
    redis_get_json(
        state,
        mip_solver_problem_model_key(&prefix, problem_id, revision),
    )
    .await
    .and_then(problem_model_from_value)
}

async fn store_problem_model(
    state: &AppState,
    problem_id: &str,
    revision: u64,
    problem: &MipProblemSpec,
) -> Result<ProblemStoreStatus, String> {
    if state.redis.is_none() {
        return ensure_local_problem_model(state, problem_id, revision, problem);
    }
    if let Some(existing) = load_redis_problem_model(state, problem_id, revision).await {
        if &existing == problem {
            remember_problem_model(state, problem_id, revision, existing);
            return Ok(ProblemStoreStatus::Existing);
        }
        return Err(format!(
            "stored problem {problem_id} revision {revision} already exists with a different model"
        ));
    }
    let prefix = redis_key_prefix();
    let stored = redis_set_json_nx_checked(
        state,
        mip_solver_problem_model_key(&prefix, problem_id, revision),
        json!({
            "schema": "dd.mip-solver.problem-model.v1",
            "service": SERVICE_NAME,
            "problemId": problem_id,
            "revision": revision,
            "problem": problem,
            "generatedAtMs": now_ms(),
        }),
    )
    .await?;
    if stored {
        remember_problem_model(state, problem_id, revision, problem.clone());
        return Ok(ProblemStoreStatus::Created);
    }
    let Some(existing) = load_redis_problem_model(state, problem_id, revision).await else {
        return Err(format!(
            "stored problem {problem_id} revision {revision} already exists but could not be reloaded"
        ));
    };
    if &existing != problem {
        return Err(format!(
            "stored problem {problem_id} revision {revision} already exists with a different model"
        ));
    }
    remember_problem_model(state, problem_id, revision, existing);
    Ok(ProblemStoreStatus::Existing)
}

async fn load_problem_model(
    state: &AppState,
    problem_id: &str,
    revision: u64,
) -> Option<MipProblemSpec> {
    if let Some(problem) = cached_problem_model(state, problem_id, revision) {
        return Some(problem);
    }
    let prefix = redis_key_prefix();
    let value = redis_get_json(
        state,
        mip_solver_problem_model_key(&prefix, problem_id, revision),
    )
    .await?;
    let problem = problem_model_from_value(value)?;
    Some(remember_problem_model(state, problem_id, revision, problem))
}

async fn hydrate_subproblem_job(state: &AppState, job: &mut SubproblemJob) -> Result<(), String> {
    if job.problem.is_some() && !job.problem_stored {
        return Ok(());
    }
    let problem_id = job.problem_id.clone().ok_or_else(|| {
        format!(
            "subproblem {} omitted problem payload but has no problemId",
            job.job_id
        )
    })?;
    let problem = load_problem_model(state, &problem_id, job.revision)
        .await
        .ok_or_else(|| {
            format!(
                "subproblem {} could not load problem {} revision {}",
                job.job_id, problem_id, job.revision
            )
        })?;
    job.problem = Some(problem);
    Ok(())
}

async fn load_pg_session_model(
    state: &AppState,
    session_id: &str,
) -> Result<Option<LiveSession>, String> {
    let Some(pool) = &state.pg else {
        return Ok(None);
    };
    let sql = format!(
        "select revision, problem from {} where session_id = $1",
        MIP_SOLVER_SESSIONS_TABLE
    );
    let row = sqlx::query(&sql)
        .bind(session_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| format!("load session model from Postgres: {error}"))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let revision: i64 = row
        .try_get("revision")
        .map_err(|error| format!("read session revision: {error}"))?;
    let problem: Value = row
        .try_get("problem")
        .map_err(|error| format!("read session problem: {error}"))?;
    Ok(Some(LiveSession {
        revision: revision.max(0) as u64,
        problem: session_problem_from_json(problem),
    }))
}

fn remember_session_model(state: &AppState, session_id: &str, session: LiveSession) -> LiveSession {
    state
        .sessions
        .lock()
        .expect("sessions mutex poisoned")
        .insert(session_id.to_string(), session.clone());
    session
}

async fn load_redis_session_model(state: &AppState, session_id: &str) -> Option<LiveSession> {
    let prefix = redis_key_prefix();
    let value = redis_get_json(state, mip_solver_session_model_key(&prefix, session_id)).await?;
    let revision = value.get("revision").and_then(Value::as_u64).unwrap_or(0);
    let problem = value
        .get("problem")
        .cloned()
        .and_then(session_problem_from_json);
    Some(LiveSession { problem, revision })
}

async fn load_session_model(state: &AppState, session_id: &str) -> Option<LiveSession> {
    match load_pg_session_model(state, session_id).await {
        Ok(Some(session)) => Some(remember_session_model(state, session_id, session)),
        Ok(None) => {
            if let Some(session) = load_redis_session_model(state, session_id).await {
                return Some(remember_session_model(state, session_id, session));
            }
            state
                .sessions
                .lock()
                .expect("sessions mutex poisoned")
                .get(session_id)
                .cloned()
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!("mip solver session load failed for {session_id}: {error}");
            if let Some(session) = load_redis_session_model(state, session_id).await {
                return Some(remember_session_model(state, session_id, session));
            }
            state
                .sessions
                .lock()
                .expect("sessions mutex poisoned")
                .get(session_id)
                .cloned()
        }
    }
}

async fn persist_session_model_checked(
    state: &AppState,
    session_id: &str,
    session: &LiveSession,
    expected_revision: u64,
) -> Result<bool, String> {
    let Some(pool) = &state.pg else {
        return Ok(true);
    };
    let problem = session
        .problem
        .as_ref()
        .map(json_value)
        .unwrap_or_else(|| json!({}));
    let sql = format!(
        concat!(
            "insert into {} (session_id, revision, problem, updated_at) values ($1, $2, $3, now()) ",
            "on conflict (session_id) do update set revision = excluded.revision, problem = excluded.problem, updated_at = now() ",
            "where {}.revision = $4"
        ),
        MIP_SOLVER_SESSIONS_TABLE,
        MIP_SOLVER_SESSIONS_TABLE
    );
    let result = sqlx::query(&sql)
        .bind(session_id)
        .bind(session.revision as i64)
        .bind(problem)
        .bind(expected_revision as i64)
        .execute(pool)
        .await
        .map_err(|error| format!("persist session model: {error}"))?;
    if result.rows_affected() == 0 {
        return Ok(false);
    }
    persist_event(
        state,
        "mip-solver.model-revision",
        None,
        Some(session_id),
        None,
        json!({"revision": session.revision}),
    )
    .await;
    Ok(true)
}

async fn persist_session_model(state: &AppState, session_id: &str, session: &LiveSession) {
    let Some(pool) = &state.pg else {
        return;
    };
    let problem = session
        .problem
        .as_ref()
        .map(json_value)
        .unwrap_or_else(|| json!({}));
    let sql = format!(
        concat!(
            "insert into {} (session_id, revision, problem, updated_at) values ($1, $2, $3, now()) ",
            "on conflict (session_id) do update set revision = excluded.revision, problem = excluded.problem, updated_at = now()"
        ),
        MIP_SOLVER_SESSIONS_TABLE
    );
    let wrote = record_pg_result(
        state,
        "upsert session model",
        sqlx::query(&sql)
            .bind(session_id)
            .bind(session.revision as i64)
            .bind(problem)
            .execute(pool)
            .await,
    )
    .await;
    if wrote {
        persist_event(
            state,
            "mip-solver.model-revision",
            None,
            Some(session_id),
            None,
            json!({"revision": session.revision}),
        )
        .await;
    }
}

async fn redis_set_json_checked(state: &AppState, key: String, value: Value) -> Result<(), String> {
    let Some(client) = &state.redis else {
        return Err("Redis is not configured".to_string());
    };
    let ttl_seconds = env_u64("MIP_SOLVER_REDIS_TTL_SECONDS", 86_400);
    let payload = serde_json::to_string(&value)
        .map_err(|error| format!("Redis payload serialization failed for {key}: {error}"))?;
    let mut connection = client
        .get_multiplexed_async_connection()
        .await
        .map_err(|error| format!("Redis connection failed for {key}: {error}"))?;
    let result: redis::RedisResult<()> = redis::cmd("SET")
        .arg(&key)
        .arg(payload)
        .arg("EX")
        .arg(ttl_seconds)
        .query_async(&mut connection)
        .await;
    result.map_err(|error| format!("Redis SET failed for {key}: {error}"))
}

async fn redis_set_json_nx_checked(
    state: &AppState,
    key: String,
    value: Value,
) -> Result<bool, String> {
    let Some(client) = &state.redis else {
        return Err("Redis is not configured".to_string());
    };
    let ttl_seconds = env_u64("MIP_SOLVER_REDIS_TTL_SECONDS", 86_400);
    let payload = serde_json::to_string(&value)
        .map_err(|error| format!("Redis payload serialization failed for {key}: {error}"))?;
    let mut connection = client
        .get_multiplexed_async_connection()
        .await
        .map_err(|error| format!("Redis connection failed for {key}: {error}"))?;
    let result: redis::RedisResult<Option<String>> = redis::cmd("SET")
        .arg(&key)
        .arg(payload)
        .arg("EX")
        .arg(ttl_seconds)
        .arg("NX")
        .query_async(&mut connection)
        .await;
    result
        .map(|value| value.is_some())
        .map_err(|error| format!("Redis SET NX failed for {key}: {error}"))
}

async fn redis_set_json(state: &AppState, key: String, value: Value) {
    if let Err(error) = redis_set_json_checked(state, key, value).await {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        eprintln!("mip solver {error}");
    }
}

async fn redis_get_json(state: &AppState, key: String) -> Option<Value> {
    let Some(client) = &state.redis else {
        return None;
    };
    let mut connection = match client.get_multiplexed_async_connection().await {
        Ok(connection) => connection,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!("mip solver redis connection failed for {key}: {error}");
            return None;
        }
    };
    let result: redis::RedisResult<Option<String>> = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut connection)
        .await;
    let payload = match result {
        Ok(Some(payload)) => payload,
        Ok(None) => return None,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!("mip solver redis GET failed for {key}: {error}");
            return None;
        }
    };
    match serde_json::from_str(&payload) {
        Ok(value) => Some(value),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!("mip solver redis payload parse failed for {key}: {error}");
            None
        }
    }
}

fn parse_http_endpoint(base_url: &str) -> Result<HttpEndpoint, String> {
    let Some(rest) = base_url
        .trim()
        .trim_end_matches('/')
        .strip_prefix("http://")
    else {
        return Err("live-mutex URL must start with http://".to_string());
    };
    let (authority, path_prefix) = match rest.find('/') {
        Some(index) => (
            &rest[..index],
            rest[index..].trim_end_matches('/').to_string(),
        ),
        None => (rest, String::new()),
    };
    if authority.is_empty() {
        return Err("live-mutex URL is missing a host".to_string());
    }
    if authority.contains(['\r', '\n']) || path_prefix.contains(['\r', '\n']) {
        return Err("live-mutex URL must not contain CRLF characters".to_string());
    }
    let addr = if authority.starts_with('[') || authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    Ok(HttpEndpoint {
        addr,
        host_header: authority.to_string(),
        path_prefix,
    })
}

fn http_path(prefix: &str, path: &str) -> String {
    if prefix.is_empty() {
        path.to_string()
    } else {
        format!("{prefix}{path}")
    }
}

fn decode_chunked_body(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoded = Vec::new();
    let mut index = 0usize;
    loop {
        let Some(line_end) = body[index..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .map(|position| index + position)
        else {
            return Err("chunked response missing chunk header terminator".to_string());
        };
        let size_text = std::str::from_utf8(&body[index..line_end])
            .map_err(|error| format!("invalid chunk header utf8: {error}"))?;
        let size_hex = size_text
            .split_once(';')
            .map(|(size, _)| size)
            .unwrap_or(size_text)
            .trim();
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|error| format!("invalid chunk size {size_text:?}: {error}"))?;
        index = line_end + 2;
        if size == 0 {
            return Ok(decoded);
        }
        let chunk_end = index.saturating_add(size);
        if chunk_end + 2 > body.len() {
            return Err("chunked response ended before declared chunk size".to_string());
        }
        if &body[chunk_end..chunk_end + 2] != b"\r\n" {
            return Err("chunked response missing chunk data terminator".to_string());
        }
        decoded.extend_from_slice(&body[index..chunk_end]);
        index = chunk_end + 2;
    }
}

fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

fn response_content_length(headers: &str) -> Result<Option<usize>, String> {
    header_value(headers, "content-length")
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|error| format!("invalid live-mutex content-length {value:?}: {error}"))
        })
        .transpose()
}

fn response_is_chunked(headers: &str) -> bool {
    header_value(headers, "transfer-encoding").is_some_and(|value| {
        value
            .split(',')
            .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
    })
}

fn chunked_body_complete(body: &[u8]) -> bool {
    let mut index = 0usize;
    loop {
        let Some(line_end) = body[index..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .map(|position| index + position)
        else {
            return false;
        };
        let Ok(size_text) = std::str::from_utf8(&body[index..line_end]) else {
            return false;
        };
        let size_hex = size_text
            .split_once(';')
            .map(|(size, _)| size)
            .unwrap_or(size_text)
            .trim();
        let Ok(size) = usize::from_str_radix(size_hex, 16) else {
            return false;
        };
        index = line_end + 2;
        if size == 0 {
            return body[index..].windows(2).any(|window| window == b"\r\n");
        }
        let chunk_end = index.saturating_add(size);
        if chunk_end + 2 > body.len() {
            return false;
        }
        if &body[chunk_end..chunk_end + 2] != b"\r\n" {
            return false;
        }
        index = chunk_end + 2;
    }
}

async fn live_mutex_post_json(
    config: &LiveMutexConfig,
    path: &str,
    body: Value,
) -> Result<Value, String> {
    let endpoint = parse_http_endpoint(&config.base_url)?;
    let body = serde_json::to_vec(&body).map_err(|error| format!("serialize request: {error}"))?;
    if config
        .auth_token
        .as_ref()
        .is_some_and(|token| token.contains(['\r', '\n']))
    {
        return Err("live-mutex auth token must not contain CRLF characters".to_string());
    }
    let auth_header = config
        .auth_token
        .as_ref()
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request = format!(
        concat!(
            "POST {} HTTP/1.1\r\n",
            "Host: {}\r\n",
            "Content-Type: application/json\r\n",
            "Accept: application/json\r\n",
            "{}",
            "Connection: close\r\n",
            "Content-Length: {}\r\n",
            "\r\n"
        ),
        http_path(&endpoint.path_prefix, path),
        endpoint.host_header,
        auth_header,
        body.len()
    );
    let timeout = Duration::from_millis(config.request_timeout_ms.max(1));
    let response = tokio::time::timeout(timeout, async {
        let mut stream = tokio::net::TcpStream::connect(&endpoint.addr)
            .await
            .map_err(|error| format!("connect {}: {error}", endpoint.addr))?;
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|error| format!("write request: {error}"))?;
        stream
            .write_all(&body)
            .await
            .map_err(|error| format!("write request body: {error}"))?;
        stream
            .flush()
            .await
            .map_err(|error| format!("flush request body: {error}"))?;
        let max_response_bytes = config.max_response_bytes.clamp(1, 16 * 1024 * 1024);
        let mut response = Vec::new();
        let mut header_end: Option<usize> = None;
        let mut expected_len: Option<usize> = None;
        let mut chunked = false;
        let mut buf = [0u8; 4096];
        loop {
            if response.len() as u64 > max_response_bytes {
                return Err(format!(
                    "live-mutex response exceeded {} bytes",
                    max_response_bytes
                ));
            }
            if let Some(end) = header_end {
                let body_start = end + 4;
                let body_len = response.len().saturating_sub(body_start);
                if let Some(length) = expected_len {
                    if body_len >= length {
                        response.truncate(body_start + length);
                        break;
                    }
                } else if chunked && chunked_body_complete(&response[body_start..]) {
                    break;
                }
            }
            let read = stream
                .read(&mut buf)
                .await
                .map_err(|error| format!("read response: {error}"))?;
            if read == 0 {
                break;
            }
            response.extend_from_slice(&buf[..read]);
            if header_end.is_none() {
                if let Some(end) = response.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&response[..end]).map_err(|error| {
                        format!("live-mutex response headers are not utf8: {error}")
                    })?;
                    expected_len = response_content_length(headers)?;
                    chunked = response_is_chunked(headers);
                    header_end = Some(end);
                }
            }
        }
        Ok::<Vec<u8>, String>(response)
    })
    .await
    .map_err(|_| format!("live-mutex request to {path} timed out"))??;

    let Some(header_end) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Err("live-mutex response missing HTTP headers".to_string());
    };
    let headers = std::str::from_utf8(&response[..header_end])
        .map_err(|error| format!("live-mutex response headers are not utf8: {error}"))?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| "live-mutex response missing status code".to_string())?;
    let mut response_body = response[header_end + 4..].to_vec();
    if headers.lines().any(|line| {
        line.to_ascii_lowercase()
            .starts_with("transfer-encoding: chunked")
    }) {
        response_body = decode_chunked_body(&response_body)?;
    }
    if !(200..300).contains(&status) {
        let message = String::from_utf8_lossy(&response_body);
        return Err(format!(
            "live-mutex {path} returned HTTP {status}: {message}"
        ));
    }
    serde_json::from_slice(&response_body)
        .map_err(|error| format!("parse live-mutex {path} response: {error}"))
}

async fn acquire_redis_coordination_lock(
    state: &AppState,
    key: &str,
    ttl_ms: u64,
    wait_ms: u64,
) -> Result<String, String> {
    let Some(client) = &state.redis else {
        return Err("Redis coordination requested but Redis is not configured".to_string());
    };
    let token = format!("{}:{}", state.node_id, Uuid::new_v4());
    let connection_timeout = Duration::from_millis(wait_ms.max(1));
    let mut connection = tokio::time::timeout(
        connection_timeout,
        client.get_multiplexed_async_connection(),
    )
    .await
    .map_err(|_| format!("Redis coordination connection timed out after {wait_ms} ms"))?
    .map_err(|error| format!("Redis coordination connection failed: {error}"))?;
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    loop {
        let now = Instant::now();
        let remaining = deadline.saturating_duration_since(now);
        if remaining.is_zero() {
            return Err(format!("coordination lock busy: {key}"));
        }
        let result: redis::RedisResult<Option<String>> = tokio::time::timeout(
            remaining,
            redis::cmd("SET")
                .arg(key)
                .arg(&token)
                .arg("NX")
                .arg("PX")
                .arg(ttl_ms.max(1))
                .query_async(&mut connection),
        )
        .await
        .map_err(|_| format!("Redis coordination SET NX timed out for {key}"))?;
        match result {
            Ok(Some(response)) if response == "OK" => return Ok(token),
            Ok(_) => {}
            Err(error) => return Err(format!("Redis coordination SET NX failed: {error}")),
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(format!("coordination lock busy: {key}"));
        }
        let remaining = deadline.saturating_duration_since(now);
        tokio::time::sleep(remaining.min(Duration::from_millis(50))).await;
    }
}

async fn release_redis_coordination_lock(state: &AppState, key: &str, token: &str) -> bool {
    let Some(client) = &state.redis else {
        return false;
    };
    let mut connection = match client.get_multiplexed_async_connection().await {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("mip solver redis coordination release connection failed for {key}: {error}");
            return false;
        }
    };
    let script = concat!(
        "if redis.call('GET', KEYS[1]) == ARGV[1] then ",
        "return redis.call('DEL', KEYS[1]) else return 0 end"
    );
    let result: redis::RedisResult<i64> = redis::cmd("EVAL")
        .arg(script)
        .arg(1)
        .arg(key)
        .arg(token)
        .query_async(&mut connection)
        .await;
    match result {
        Ok(deleted) => deleted == 1,
        Err(error) => {
            eprintln!("mip solver redis coordination release failed for {key}: {error}");
            false
        }
    }
}

async fn acquire_live_mutex_coordination_lock(
    state: &AppState,
    key: &str,
    ttl_ms: u64,
    wait_ms: u64,
) -> Result<String, String> {
    let Some(config) = &state.coordination.live_mutex else {
        return Err(
            "live-mutex coordination requested but no live-mutex URL is configured".to_string(),
        );
    };
    let value = live_mutex_post_json(
        config,
        "/v1/lock",
        json!({
            "key": key,
            "ttlMs": ttl_ms.max(1),
            "waitMs": wait_ms,
        }),
    )
    .await?;
    let response: LiveMutexLockResponse = serde_json::from_value(value)
        .map_err(|error| format!("parse live-mutex lock response: {error}"))?;
    if !response.acquired {
        let detail = response.error.unwrap_or_else(|| "not acquired".to_string());
        return Err(format!("coordination lock busy: {key}: {detail}"));
    }
    response
        .lock_uuid
        .ok_or_else(|| "live-mutex lock response missing lockUuid".to_string())
}

async fn release_live_mutex_coordination_lock(
    state: &AppState,
    key: &str,
    lock_uuid: &str,
) -> bool {
    let Some(config) = &state.coordination.live_mutex else {
        return false;
    };
    let value = match live_mutex_post_json(
        config,
        "/v1/unlock",
        json!({
            "key": key,
            "lockUuid": lock_uuid,
        }),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            eprintln!("mip solver live-mutex release failed for {key}: {error}");
            return false;
        }
    };
    let response: LiveMutexUnlockResponse = match serde_json::from_value(value) {
        Ok(response) => response,
        Err(error) => {
            eprintln!("mip solver live-mutex release parse failed for {key}: {error}");
            return false;
        }
    };
    if response.unlocked {
        true
    } else {
        let detail = response.error.unwrap_or_else(|| "not unlocked".to_string());
        eprintln!("mip solver live-mutex release failed for {key}: {detail}");
        false
    }
}

async fn acquire_coordination_lock(
    state: &AppState,
    key: String,
    ttl_ms: u64,
) -> Result<Option<CoordinationGuard>, String> {
    if !state.coordination.enabled() {
        return Ok(None);
    }
    let mut holders = Vec::new();
    for backend in state.coordination.backends.iter().copied() {
        let holder_result = match backend {
            CoordinationBackend::Redis => {
                acquire_redis_coordination_lock(state, &key, ttl_ms, state.coordination.wait_ms)
                    .await
                    .map(|token| CoordinationHolder::Redis { token })
            }
            CoordinationBackend::LiveMutex => acquire_live_mutex_coordination_lock(
                state,
                &key,
                ttl_ms,
                state.coordination.wait_ms,
            )
            .await
            .map(|lock_uuid| CoordinationHolder::LiveMutex { lock_uuid }),
        };
        match holder_result {
            Ok(holder) => holders.push(holder),
            Err(error) => {
                release_coordination_guard(
                    state,
                    Some(CoordinationGuard {
                        key: key.clone(),
                        holders,
                    }),
                )
                .await;
                return Err(error);
            }
        }
    }
    Ok(Some(CoordinationGuard { key, holders }))
}

async fn release_coordination_guard(state: &AppState, guard: Option<CoordinationGuard>) {
    let Some(guard) = guard else {
        return;
    };
    for holder in guard.holders.into_iter().rev() {
        let released = match holder {
            CoordinationHolder::Redis { token } => {
                release_redis_coordination_lock(state, &guard.key, &token).await
            }
            CoordinationHolder::LiveMutex { lock_uuid } => {
                release_live_mutex_coordination_lock(state, &guard.key, &lock_uuid).await
            }
        };
        if !released {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        }
    }
}

async fn cache_solve_registry_entry(state: &AppState, solve_id: &str) {
    let entry = state
        .solves
        .lock()
        .expect("solves mutex poisoned")
        .get(solve_id)
        .cloned();
    let Some(entry) = entry else {
        return;
    };
    let prefix = redis_key_prefix();
    redis_set_json(
        state,
        mip_solver_solve_snapshot_key(&prefix, solve_id),
        json!({
            "schema": "dd.mip-solver.solve-snapshot.v1",
            "service": SERVICE_NAME,
            "solve": entry,
            "generatedAtMs": now_ms(),
        }),
    )
    .await;
}

async fn cache_solve_frontier(state: &AppState, solve_id: &str, jobs: &[SubproblemJob]) {
    let prefix = redis_key_prefix();
    redis_set_json(
        state,
        mip_solver_solve_frontier_key(&prefix, solve_id),
        json!({
            "schema": "dd.mip-solver.frontier.v1",
            "service": SERVICE_NAME,
            "solveId": solve_id,
            "jobs": jobs,
            "generatedAtMs": now_ms(),
        }),
    )
    .await;
}

async fn snapshot_solve_state(state: &AppState, solve_id: &str) {
    cache_solve_registry_entry(state, solve_id).await;
    persist_solve_registry_entry(state, solve_id).await;
}

async fn snapshot_solve_frontier(state: &AppState, solve_id: &str, jobs: &[SubproblemJob]) {
    cache_solve_frontier(state, solve_id, jobs).await;
}

async fn finalize_solve_state(state: &AppState, response: &SolveResponse) {
    cache_solve_registry_entry(state, &response.solve_id).await;
    persist_solve_response(state, response).await;
}

async fn cache_session_model(state: &AppState, session_id: &str, session: LiveSession) {
    let prefix = redis_key_prefix();
    redis_set_json(
        state,
        mip_solver_session_model_key(&prefix, session_id),
        json!({
            "schema": "dd.mip-solver.session-model.v1",
            "service": SERVICE_NAME,
            "sessionId": session_id,
            "revision": session.revision,
            "problem": session.problem,
            "generatedAtMs": now_ms(),
        }),
    )
    .await;
}

async fn snapshot_session_model(state: &AppState, session_id: &str, session: LiveSession) {
    cache_session_model(state, session_id, session.clone()).await;
    persist_session_model(state, session_id, &session).await;
}

async fn record_job_submitted(state: &AppState, job: &SubproblemJob) {
    track_job_submitted(state, job);
    persist_job_submitted(state, job).await;
    persist_solve_registry_entry(state, &job.solve_id).await;
}

async fn record_job_result(state: &AppState, result: &SubproblemResult, terminal: bool) {
    track_job_result(state, result, terminal);
    persist_job_result(state, result, terminal).await;
    persist_solve_registry_entry(state, &result.solve_id).await;
}

async fn publish_event(state: &AppState, event_name: &str, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let event = json!({
        "schema":"dd.mip-solver.event.v1",
        "service": SERVICE_NAME,
        "nodeId": state.node_id,
        "role": state.role.as_str(),
        "eventName": event_name,
        "payload": payload,
        "timeMs": now_ms(),
    });
    if let Ok(bytes) = serde_json::to_vec(&event) {
        let _ = nats
            .publish(state.events_subject.clone(), bytes.into())
            .await;
    }
}

async fn publish_control(state: &AppState, command_name: &str, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let command = json!({
        "schema":"dd.mip-solver.control.v1",
        "service": SERVICE_NAME,
        "nodeId": state.node_id,
        "role": state.role.as_str(),
        "commandName": command_name,
        "payload": payload,
        "timeMs": now_ms(),
    });
    if let Ok(bytes) = serde_json::to_vec(&command) {
        let _ = nats
            .publish(state.control_subject.clone(), bytes.into())
            .await;
    }
}

fn value_str(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(|s| s.to_string())
}

fn value_u128(value: &Value, key: &str) -> Option<u128> {
    value.get(key).and_then(Value::as_u64).map(u128::from)
}

fn payload_job_key(payload: &Value) -> Option<String> {
    value_str(payload, "jobUuid").or_else(|| value_str(payload, "jobId"))
}

fn worker_job_status_from_payload(
    payload: &Value,
    status: &str,
    seen_at: u128,
) -> Option<WorkerJobStatus> {
    let job_id = value_str(payload, "jobId")?;
    let solve_id = value_str(payload, "solveId")?;
    Some(WorkerJobStatus {
        job_id,
        job_uuid: value_str(payload, "jobUuid"),
        solve_id,
        problem_id: value_str(payload, "problemId"),
        status: status.to_string(),
        started_at_ms: value_u128(payload, "startedAtMs").unwrap_or(seen_at),
        last_seen_ms: seen_at,
    })
}

fn track_job_worker_progress(state: &AppState, worker_node: &str, payload: &Value, seen_at: u128) {
    let Some(solve_id) = value_str(payload, "solveId") else {
        return;
    };
    let Some(job_id) = value_str(payload, "jobId") else {
        return;
    };
    let mut solves = state.solves.lock().expect("solves mutex poisoned");
    let Some(solve) = solves.get_mut(&solve_id) else {
        return;
    };
    let job = solve
        .jobs
        .entry(job_id.clone())
        .or_insert_with(|| JobRegistryEntry {
            job_id: job_id.clone(),
            job_uuid: value_str(payload, "jobUuid"),
            problem_id: value_str(payload, "problemId"),
            root_job_id: job_retry_root(&job_id).to_string(),
            retry_index: job_retry_index(&job_id),
            submitted_at_ms: value_u128(payload, "startedAtMs").unwrap_or(seen_at),
            ..JobRegistryEntry::default()
        });
    if let Some(job_uuid) = value_str(payload, "jobUuid") {
        job.job_uuid = Some(job_uuid);
    }
    if let Some(problem_id) = value_str(payload, "problemId") {
        job.problem_id = Some(problem_id);
    }
    job.status = "running".to_string();
    job.worker_node = Some(worker_node.to_string());
    job.last_heartbeat_ms = Some(seen_at);
    job.finished_at_ms = None;
    job.error = None;
    solve.updated_at_ms = seen_at;
}

fn record_cancel_control_frame(state: &AppState, frame: &Value) -> Result<bool, String> {
    if frame.get("service").and_then(Value::as_str) != Some(SERVICE_NAME) {
        return Ok(false);
    }
    if frame.get("commandName").and_then(Value::as_str) != Some("cancel-solve") {
        return Ok(false);
    }
    let payload = frame.get("payload").unwrap_or(&Value::Null);
    let mut request_id = value_str(payload, "requestId");
    let mut problem_id = match value_str(payload, "problemId") {
        Some(value) if value.trim().is_empty() => {
            return Err("cancel frame problemId must be a UUID".to_string());
        }
        Some(value) => Some(
            Uuid::parse_str(value.trim())
                .map(|uuid| uuid.to_string())
                .map_err(|_| "cancel frame problemId must be a UUID".to_string())?,
        ),
        None => None,
    };
    let solve_id = match value_str(payload, "solveId") {
        Some(solve_id) => solve_id,
        None => {
            let lookup_key = problem_id
                .as_deref()
                .or(request_id.as_deref())
                .ok_or_else(|| {
                    "cancel frame missing solveId, problemId, or requestId".to_string()
                })?;
            let solves = state.solves.lock().expect("solves mutex poisoned");
            match resolve_solve_id(&solves, lookup_key) {
                Some(solve_id) => solve_id,
                None => problem_id
                    .clone()
                    .ok_or_else(|| format!("cancel frame target not found: {lookup_key}"))?,
            }
        }
    };
    let reason = value_str(payload, "reason").unwrap_or_else(|| "cancel requested".to_string());
    let requested_by = value_str(payload, "requestedBy")
        .or_else(|| value_str(frame, "nodeId"))
        .unwrap_or_else(|| "unknown".to_string());
    let requested_at_ms = payload
        .get("requestedAtMs")
        .and_then(Value::as_u64)
        .map(u128::from)
        .or_else(|| frame.get("timeMs").and_then(Value::as_u64).map(u128::from))
        .unwrap_or_else(now_ms);
    let mut solves = state.solves.lock().expect("solves mutex poisoned");
    if let Some(solve) = solves.get_mut(&solve_id) {
        request_id.get_or_insert_with(|| solve.request_id.clone());
        if problem_id.is_none() {
            problem_id = solve.problem_id.clone();
        }
        solve.cancel_requested = true;
        solve.cancel_requested_at_ms.get_or_insert(requested_at_ms);
        solve.cancel_reason.get_or_insert_with(|| reason.clone());
        if solve.finished_at_ms.is_none() {
            solve.status = "cancelling".to_string();
        }
        solve.updated_at_ms = now_ms();
    }
    drop(solves);

    let info = CancelInfo {
        solve_id: solve_id.clone(),
        request_id,
        problem_id,
        reason: reason.clone(),
        requested_by,
        requested_at_ms,
    };
    state
        .cancelled_solves
        .lock()
        .expect("cancelled solves mutex poisoned")
        .insert(solve_id.clone(), info);
    Ok(true)
}

fn record_worker_control_frame(state: &AppState, frame: &Value) -> Result<(), String> {
    if frame.get("service").and_then(Value::as_str) != Some(SERVICE_NAME) {
        return Ok(());
    }
    let node_id =
        value_str(frame, "nodeId").ok_or_else(|| "control frame missing nodeId".to_string())?;
    let command = value_str(frame, "commandName")
        .ok_or_else(|| "control frame missing commandName".to_string())?;
    let payload = frame.get("payload").unwrap_or(&Value::Null);
    let seen_at = frame
        .get("timeMs")
        .and_then(Value::as_u64)
        .map(u128::from)
        .unwrap_or_else(now_ms);

    let mut workers = state.workers.lock().expect("workers mutex poisoned");
    let worker = workers
        .entry(node_id.clone())
        .or_insert_with(|| WorkerNodeStatus {
            node_id: node_id.clone(),
            ..WorkerNodeStatus::default()
        });
    worker.last_command = command.clone();
    worker.last_seen_ms = seen_at;

    if let Some(consumer) = value_str(payload, "consumer") {
        worker.consumer = Some(consumer);
    }
    if let Some(jobs_subject) = value_str(payload, "jobsSubject") {
        worker.jobs_subject = Some(jobs_subject);
    }
    if let Some(results_subject) = value_str(payload, "resultsSubject") {
        worker.results_subject = Some(results_subject);
    }

    match command.as_str() {
        "worker-ready" => {
            worker.ready_at_ms.get_or_insert(seen_at);
        }
        "request-work" => {
            worker.request_count = worker.request_count.saturating_add(1);
            if let Some(active_job) = worker_job_status_from_payload(payload, "running", seen_at) {
                let key = active_job
                    .job_uuid
                    .clone()
                    .unwrap_or_else(|| active_job.job_id.clone());
                worker.last_job_id = Some(active_job.job_id.clone());
                worker.last_solve_id = Some(active_job.solve_id.clone());
                worker.last_status = Some(active_job.status.clone());
                worker.active_jobs.insert(key, active_job);
            }
        }
        "worker-progress" => {
            if let Some(active_job) = worker_job_status_from_payload(payload, "running", seen_at) {
                let key = active_job
                    .job_uuid
                    .clone()
                    .unwrap_or_else(|| active_job.job_id.clone());
                worker.last_job_id = Some(active_job.job_id.clone());
                worker.last_solve_id = Some(active_job.solve_id.clone());
                worker.last_status = Some(active_job.status.clone());
                worker.active_jobs.insert(key, active_job);
            }
        }
        "worker-completed" => {
            worker.completed_count = worker.completed_count.saturating_add(1);
            worker.last_job_id = value_str(payload, "jobId");
            worker.last_solve_id = value_str(payload, "solveId");
            worker.last_status = value_str(payload, "status");
            if let Some(key) = payload_job_key(payload) {
                worker.active_jobs.remove(&key);
            }
        }
        "worker-skipped-cancelled"
        | "worker-stopped-cancelled"
        | "worker-discarded-cancelled-result" => {
            worker.last_job_id = value_str(payload, "jobId");
            worker.last_solve_id = value_str(payload, "solveId");
            worker.last_status = Some("cancelled".to_string());
            if let Some(key) = payload_job_key(payload) {
                worker.active_jobs.remove(&key);
            }
        }
        _ => {}
    }
    drop(workers);
    if matches!(command.as_str(), "request-work" | "worker-progress") {
        track_job_worker_progress(state, &node_id, payload, seen_at);
    }
    Ok(())
}

fn mip_stream_config() -> async_nats::jetstream::stream::Config {
    async_nats::jetstream::stream::Config {
        name: DD_REMOTE_MIP_SOLVER_STREAM_NAME.to_string(),
        subjects: DD_REMOTE_MIP_SOLVER_STREAM_SUBJECTS
            .iter()
            .map(|subject| subject.to_string())
            .collect(),
        retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
        max_age: Duration::from_secs(60 * 60 * 24 * 7),
        max_message_size: 8 * 1024 * 1024,
        ..Default::default()
    }
}

async fn ensure_mip_stream(
    client: async_nats::Client,
) -> Result<async_nats::jetstream::stream::Stream, Box<dyn Error + Send + Sync>> {
    let jetstream = async_nats::jetstream::new(client);
    Ok(jetstream.get_or_create_stream(mip_stream_config()).await?)
}

async fn jetstream_publish_ack(
    client: &async_nats::Client,
    subject: &str,
    payload: Vec<u8>,
) -> Result<u64, String> {
    let jetstream = async_nats::jetstream::new(client.clone());
    jetstream
        .get_or_create_stream(mip_stream_config())
        .await
        .map_err(|err| format!("ensure JetStream stream: {err}"))?;
    let ack = jetstream
        .publish(subject.to_string(), payload.into())
        .await
        .map_err(|err| format!("JetStream publish {subject}: {err}"))?
        .await
        .map_err(|err| format!("JetStream publish ack {subject}: {err}"))?;
    if ack.stream != DD_REMOTE_MIP_SOLVER_STREAM_NAME {
        return Err(format!(
            "JetStream ack for {subject} landed in stream {}, expected {}",
            ack.stream, DD_REMOTE_MIP_SOLVER_STREAM_NAME
        ));
    }
    Ok(ack.sequence)
}

async fn publish_subproblem_job(
    client: &async_nats::Client,
    jobs_subject: &str,
    job: &SubproblemJob,
) -> Result<u64, String> {
    validate_subproblem_job_payload(job)?;
    let payload = serde_json::to_vec(job).map_err(|err| format!("serialize job: {err}"))?;
    jetstream_publish_ack(client, jobs_subject, payload).await
}

fn validate_subproblem_job_payload(job: &SubproblemJob) -> Result<(), String> {
    if job.problem_stored {
        if job.problem_id.is_none() {
            return Err(format!(
                "job {} marked problemStored but has no problemId",
                job.job_id
            ));
        }
        if job.problem.is_some() {
            return Err(format!(
                "job {} marked problemStored but still carries an embedded problem",
                job.job_id
            ));
        }
        return Ok(());
    }
    if job.problem.is_none() {
        return Err(format!(
            "job {} has no stored problem reference and no embedded problem",
            job.job_id
        ));
    }
    Ok(())
}

fn result_consumer_name(solve_id: &str) -> String {
    format!("{solve_id}-results")
}

fn result_consumer_config(
    consumer_name: &str,
    result_subject: &str,
    start_sequence: u64,
) -> async_nats::jetstream::consumer::pull::Config {
    async_nats::jetstream::consumer::pull::Config {
        name: Some(consumer_name.to_string()),
        deliver_policy: async_nats::jetstream::consumer::DeliverPolicy::ByStartSequence {
            start_sequence,
        },
        filter_subject: result_subject.to_string(),
        ack_wait: Duration::from_secs(60),
        max_deliver: 1,
        max_ack_pending: 1024,
        inactive_threshold: Duration::from_secs(120),
        ..Default::default()
    }
}

fn worker_consumer_config(
    consumer_name: &str,
    jobs_subject: &str,
    ack_wait: Duration,
    max_ack_pending: i64,
    max_deliver: i64,
) -> async_nats::jetstream::consumer::pull::Config {
    async_nats::jetstream::consumer::pull::Config {
        durable_name: Some(consumer_name.to_string()),
        filter_subject: jobs_subject.to_string(),
        ack_wait,
        max_ack_pending,
        max_deliver,
        ..Default::default()
    }
}

async fn build_result_consumer(
    client: async_nats::Client,
    consumer_name: &str,
    result_subject: &str,
    start_sequence: u64,
) -> Result<async_nats::jetstream::consumer::PullConsumer, Box<dyn Error + Send + Sync>> {
    let stream = ensure_mip_stream(client).await?;
    let config = result_consumer_config(consumer_name, result_subject, start_sequence);
    Ok(stream
        .get_or_create_consumer::<async_nats::jetstream::consumer::pull::Config>(
            consumer_name,
            config,
        )
        .await?)
}

async fn finish_cancelled_solve(
    state: &AppState,
    solve_id: String,
    request_id: String,
    problem_id: Option<String>,
    revision: u64,
    distributed: bool,
    warnings: Vec<String>,
    reason: String,
) -> SolveResponse {
    let response = cancelled_solve_response(
        state,
        solve_id,
        request_id,
        problem_id,
        revision,
        distributed,
        warnings,
        reason,
    );
    finalize_solve_state(state, &response).await;
    publish_event(
        state,
        "solve-cancelled",
        json!({
            "solveId": &response.solve_id,
            "requestId": &response.request_id,
            "jobsPublished": response.jobs_published,
            "jobsCompleted": response.jobs_completed,
        }),
    )
    .await;
    response
}

async fn solve_pure_lp_local(
    state: AppState,
    request_id: String,
    problem_id: String,
    revision: u64,
    problem: MipProblemSpec,
    options: SolveOptions,
) -> Result<SolveResponse, String> {
    let solve_id = format!("solve-{}", Uuid::new_v4());
    let job = SubproblemJob {
        solve_id: solve_id.clone(),
        request_id: request_id.clone(),
        job_id: format!("{solve_id}-lp"),
        job_uuid: new_uuid_string(),
        problem_id: Some(problem_id.clone()),
        problem_stored: false,
        revision,
        depth: 0,
        master_node: state.node_id.clone(),
        problem: Some(problem.clone()),
        extra_constraints: Vec::new(),
        avoid_worker_nodes: Vec::new(),
        options: options.clone(),
        submitted_at_ms: now_ms(),
    };
    let warnings = Vec::new();
    track_solve_started(
        &state,
        &solve_id,
        &request_id,
        &problem_id,
        revision,
        1,
        false,
    )?;
    track_runtime_task_solve(&state, &problem_id, &solve_id);
    persist_solve_started(
        &state,
        &solve_id,
        &request_id,
        revision,
        &problem,
        &options,
        1,
        false,
    )
    .await;
    snapshot_solve_state(&state, &solve_id).await;
    snapshot_solve_frontier(&state, &solve_id, std::slice::from_ref(&job)).await;

    let cancel_poll = cancel_poll_interval();
    if let Some(cancel) = solve_cancel_info(&state, &solve_id) {
        let response = finish_cancelled_solve(
            &state,
            solve_id,
            request_id,
            Some(problem_id.clone()),
            revision,
            false,
            warnings,
            cancel.reason,
        )
        .await;
        return Ok(response);
    }

    record_job_submitted(&state, &job).await;
    let node = state.node_id.clone();
    let attempt_job = job.clone();
    let task_id = attempt_job.job_uuid.clone();
    track_runtime_task_started(
        &state,
        task_id.clone(),
        "local-lp",
        Some(problem_id.clone()),
        Some(solve_id.clone()),
        Some(request_id.clone()),
        Some(attempt_job.job_id.clone()),
        Some(task_id.clone()),
        None,
    );
    let mut solve_task = tokio::task::spawn_blocking(move || solve_subproblem(attempt_job, node));
    track_runtime_task_abort_handle(&state, &task_id, solve_task.abort_handle());
    let _task_guard = RuntimeTaskFinishGuard::new(state.clone(), task_id.clone(), "finished");
    let result = loop {
        tokio::select! {
            joined = &mut solve_task => {
                break match joined {
                    Ok(result) => result,
                    Err(err) => {
                        track_runtime_task_finished(&state, &task_id, "error");
                        return Err(format!("local LP solve task failed: {err}"));
                    }
                };
            }
            _ = tokio::time::sleep(cancel_poll) => {
                if let Some(cancel) = solve_cancel_info(&state, &solve_id) {
                    let response = finish_cancelled_solve(
                        &state,
                        solve_id,
                        request_id,
                        Some(problem_id.clone()),
                        revision,
                        false,
                        warnings,
                        cancel.reason,
                    )
                    .await;
                    track_runtime_task_finished(&state, &task_id, "cancelled");
                    return Ok(response);
                }
            }
        }
    };
    track_runtime_task_finished(&state, &task_id, &result.status);
    if let Some(cancel) = solve_cancel_info(&state, &solve_id) {
        let response = finish_cancelled_solve(
            &state,
            solve_id,
            request_id,
            Some(problem_id.clone()),
            revision,
            false,
            warnings,
            cancel.reason,
        )
        .await;
        track_runtime_task_finished(&state, &task_id, "cancelled");
        return Ok(response);
    }

    record_job_result(&state, &result, true).await;
    let response = aggregate_results(
        solve_id,
        request_id,
        Some(problem_id.clone()),
        revision,
        &problem,
        &options,
        1,
        1,
        0,
        0,
        vec![result],
        false,
        false,
        &state,
        warnings,
    );
    track_solve_finished(&state, &response);
    finalize_solve_state(&state, &response).await;
    publish_event(
        &state,
        "solve-finished",
        json!({
            "solveId": &response.solve_id,
            "requestId": &response.request_id,
            "status": &response.status,
            "jobsPublished": response.jobs_published,
            "jobsCompleted": response.jobs_completed,
            "jobsRedelegated": response.jobs_redelegated,
            "jobsSplit": response.jobs_split,
            "timedOut": response.timed_out,
        }),
    )
    .await;
    Ok(response)
}

#[allow(clippy::too_many_arguments)]
async fn requeue_stale_worker_jobs(
    state: &AppState,
    nats: &async_nats::Client,
    solve_id: &str,
    request_id: &str,
    stale_after: Duration,
    expected_job_ids: &mut HashSet<String>,
    completed_job_ids: &HashSet<String>,
    jobs_by_id: &mut HashMap<String, SubproblemJob>,
    retry_index_by_job_id: &mut HashMap<String, usize>,
    superseded_job_ids: &mut HashSet<String>,
    jobs_published: &mut usize,
    jobs_redelegated: &mut usize,
    warnings: &mut Vec<String>,
) {
    let stale_jobs = stale_worker_jobs(
        state,
        solve_id,
        expected_job_ids,
        completed_job_ids,
        stale_after,
        now_ms(),
    );
    for stale in stale_jobs {
        if superseded_job_ids.contains(&stale.job_id) {
            continue;
        }
        let Some(original_job) = jobs_by_id.get(&stale.job_id).cloned() else {
            warnings.push(format!(
                "cannot requeue stale job {}; original job payload not found",
                stale.job_id
            ));
            continue;
        };
        let next_retry_index = retry_index_by_job_id
            .get(&stale.job_id)
            .copied()
            .unwrap_or_else(|| job_retry_index(&stale.job_id))
            + 1;
        let mut retry_job = redelegated_job(&original_job, next_retry_index);
        if !retry_job
            .avoid_worker_nodes
            .iter()
            .any(|node| node == &stale.worker_node)
        {
            retry_job.avoid_worker_nodes.push(stale.worker_node.clone());
        }
        match publish_subproblem_job(nats, &state.jobs_subject, &retry_job).await {
            Ok(_) => {
                track_job_stale_requeued(
                    state,
                    solve_id,
                    &stale.job_id,
                    &stale.worker_node,
                    &retry_job.job_id,
                    stale.last_heartbeat_ms,
                );
                record_job_submitted(state, &retry_job).await;
                track_job_redelegated(state, solve_id);
                snapshot_solve_state(state, solve_id).await;
                expected_job_ids.remove(&stale.job_id);
                expected_job_ids.insert(retry_job.job_id.clone());
                superseded_job_ids.insert(stale.job_id.clone());
                retry_index_by_job_id.insert(retry_job.job_id.clone(), next_retry_index);
                jobs_by_id.insert(retry_job.job_id.clone(), retry_job.clone());
                *jobs_published += 1;
                *jobs_redelegated += 1;
                state
                    .metrics
                    .subproblem_jobs_published_total
                    .fetch_add(1, Ordering::Relaxed);
                state
                    .metrics
                    .subproblem_jobs_redelegated_total
                    .fetch_add(1, Ordering::Relaxed);
                let job_uuid = stale.job_uuid.unwrap_or_else(|| "unknown".to_string());
                warnings.push(format!(
                    "worker {} missed heartbeat for job {} ({job_uuid}); requeued as {}",
                    stale.worker_node, stale.job_id, retry_job.job_id
                ));
                publish_event(
                    state,
                    "worker-stale-requeued",
                    json!({
                        "solveId": solve_id,
                        "requestId": request_id,
                        "staleWorkerNode": stale.worker_node,
                        "staleJobId": stale.job_id,
                        "staleJobUuid": job_uuid,
                        "retryJobId": retry_job.job_id,
                        "retryJobUuid": retry_job.job_uuid,
                        "lastHeartbeatMs": stale.last_heartbeat_ms,
                        "staleAfterSeconds": stale_after.as_secs(),
                    }),
                )
                .await;
            }
            Err(error) => {
                warnings.push(format!(
                    "failed to requeue stale job {} from worker {}: {error}",
                    stale.job_id, stale.worker_node
                ));
            }
        }
    }
}

async fn solve_problem_distributed(
    state: AppState,
    request_id: String,
    problem_id: String,
    revision: u64,
    problem: MipProblemSpec,
    options: SolveOptions,
) -> Result<SolveResponse, String> {
    let problem = normalized_problem(problem)?;
    if is_pure_lp(&problem)? {
        return solve_pure_lp_local(state, request_id, problem_id, revision, problem, options)
            .await;
    }
    let solve_id = format!("solve-{}", Uuid::new_v4());
    let mut warnings = Vec::new();
    let problem_stored = if state.nats.is_some() && state.redis.is_some() {
        match store_problem_model(&state, &problem_id, revision, &problem).await {
            Ok(_) => true,
            Err(error) if is_problem_model_conflict(&error) => return Err(error),
            Err(error) => {
                warnings.push(format!(
                    "problem model storage unavailable; embedding problem in jobs: {error}"
                ));
                false
            }
        }
    } else {
        remember_problem_model(&state, &problem_id, revision, problem.clone());
        false
    };
    let (jobs, frontier_warnings) = build_frontier_jobs(
        &problem,
        &solve_id,
        &request_id,
        &problem_id,
        revision,
        &state.node_id,
        &options,
        problem_stored,
    )?;
    warnings.extend(frontier_warnings);
    track_solve_started(
        &state,
        &solve_id,
        &request_id,
        &problem_id,
        revision,
        jobs.len(),
        state.nats.is_some(),
    )?;
    track_runtime_task_solve(&state, &problem_id, &solve_id);
    persist_solve_started(
        &state,
        &solve_id,
        &request_id,
        revision,
        &problem,
        &options,
        jobs.len(),
        state.nats.is_some(),
    )
    .await;
    snapshot_solve_state(&state, &solve_id).await;
    snapshot_solve_frontier(&state, &solve_id, &jobs).await;
    let cancel_poll = cancel_poll_interval();
    if let Some(cancel) = solve_cancel_info(&state, &solve_id) {
        let response = finish_cancelled_solve(
            &state,
            solve_id,
            request_id,
            Some(problem_id.clone()),
            revision,
            state.nats.is_some(),
            warnings,
            cancel.reason,
        )
        .await;
        return Ok(response);
    }
    if jobs.is_empty() {
        let response = aggregate_results(
            solve_id,
            request_id,
            Some(problem_id.clone()),
            revision,
            &problem,
            &options,
            0,
            0,
            0,
            0,
            Vec::new(),
            false,
            false,
            &state,
            warnings,
        );
        track_solve_finished(&state, &response);
        finalize_solve_state(&state, &response).await;
        return Ok(response);
    }

    let Some(nats) = state.nats.clone() else {
        let mut results = Vec::new();
        let mut jobs_published = 0usize;
        let mut jobs_redelegated = 0usize;
        let mut jobs_split = 0usize;
        let mut jobs_expected = jobs.len();
        let max_retries = options.max_job_retries.unwrap_or(2);
        let mut pending: VecDeque<SubproblemJob> = jobs.iter().cloned().collect();
        while let Some(initial_job) = pending.pop_front() {
            if let Some(cancel) = solve_cancel_info(&state, &solve_id) {
                let response = finish_cancelled_solve(
                    &state,
                    solve_id,
                    request_id,
                    Some(problem_id.clone()),
                    revision,
                    false,
                    warnings,
                    cancel.reason,
                )
                .await;
                return Ok(response);
            }
            let mut job = initial_job;
            let mut retry_index = job_retry_index(&job.job_id);
            loop {
                if let Some(cancel) = solve_cancel_info(&state, &solve_id) {
                    let response = finish_cancelled_solve(
                        &state,
                        solve_id,
                        request_id,
                        Some(problem_id.clone()),
                        revision,
                        false,
                        warnings,
                        cancel.reason,
                    )
                    .await;
                    return Ok(response);
                }
                jobs_published += 1;
                record_job_submitted(&state, &job).await;
                let node = state.node_id.clone();
                let attempt_job = job.clone();
                let task_id = attempt_job.job_uuid.clone();
                let job_id = attempt_job.job_id.clone();
                let job_problem_id = attempt_job.problem_id.clone();
                track_runtime_task_started(
                    &state,
                    task_id.clone(),
                    "local-subproblem",
                    job_problem_id,
                    Some(solve_id.clone()),
                    Some(request_id.clone()),
                    Some(job_id),
                    Some(task_id.clone()),
                    None,
                );
                let mut solve_task =
                    tokio::task::spawn_blocking(move || solve_subproblem(attempt_job, node));
                track_runtime_task_abort_handle(&state, &task_id, solve_task.abort_handle());
                let _task_guard =
                    RuntimeTaskFinishGuard::new(state.clone(), task_id.clone(), "finished");
                let result = loop {
                    tokio::select! {
                        joined = &mut solve_task => {
                            break match joined {
                                Ok(result) => result,
                                Err(err) => {
                                    track_runtime_task_finished(&state, &task_id, "error");
                                    return Err(format!("local solve task failed: {err}"));
                                }
                            };
                        }
                        _ = tokio::time::sleep(cancel_poll) => {
                            if let Some(cancel) = solve_cancel_info(&state, &solve_id) {
                                let response = finish_cancelled_solve(
                                    &state,
                                    solve_id,
                                    request_id,
                                    Some(problem_id.clone()),
                                    revision,
                                    false,
                                    warnings,
                                    cancel.reason,
                                )
                                .await;
                                track_runtime_task_finished(&state, &task_id, "cancelled");
                                return Ok(response);
                            }
                        }
                    }
                };
                track_runtime_task_finished(&state, &task_id, &result.status);
                if let Some(cancel) = solve_cancel_info(&state, &solve_id) {
                    let response = finish_cancelled_solve(
                        &state,
                        solve_id,
                        request_id,
                        Some(problem_id.clone()),
                        revision,
                        false,
                        warnings,
                        cancel.reason,
                    )
                    .await;
                    track_runtime_task_finished(&state, &task_id, "cancelled");
                    return Ok(response);
                }
                if result.status == "split" && !result.child_jobs.is_empty() {
                    let child_count = result.child_jobs.len();
                    let child_jobs = result.child_jobs.clone();
                    record_job_result(&state, &result, false).await;
                    track_job_split(&state, &solve_id, child_count);
                    state
                        .metrics
                        .subproblem_jobs_split_total
                        .fetch_add(1, Ordering::Relaxed);
                    jobs_split += 1;
                    jobs_expected = jobs_expected.saturating_add(child_count.saturating_sub(1));
                    snapshot_solve_state(&state, &solve_id).await;
                    snapshot_solve_frontier(&state, &solve_id, &child_jobs).await;
                    warnings.push(format!(
                        "local job {} split into {} child subproblems",
                        result.job_id, child_count
                    ));
                    for child in child_jobs {
                        pending.push_back(child);
                    }
                    break;
                }
                if should_redelegate_result(&result, retry_index, max_retries) {
                    record_job_result(&state, &result, false).await;
                    warnings.push(format!(
                        "local job {} failed; re-delegating retry {} of {}",
                        result.job_id,
                        retry_index + 1,
                        max_retries
                    ));
                    retry_index += 1;
                    job = redelegated_job(&job, retry_index);
                    jobs_redelegated += 1;
                    track_job_redelegated(&state, &solve_id);
                    snapshot_solve_state(&state, &solve_id).await;
                    continue;
                }
                record_job_result(&state, &result, true).await;
                results.push(result);
                break;
            }
        }
        let response = aggregate_results(
            solve_id,
            request_id,
            Some(problem_id.clone()),
            revision,
            &problem,
            &options,
            jobs_expected,
            jobs_published,
            jobs_redelegated,
            jobs_split,
            results,
            false,
            false,
            &state,
            warnings,
        );
        track_solve_finished(&state, &response);
        finalize_solve_state(&state, &response).await;
        return Ok(response);
    };

    publish_event(
        &state,
        "solve-frontier-built",
        json!({"solveId": &solve_id, "requestId": &request_id, "jobs": jobs.len()}),
    )
    .await;

    let mut first_job_sequence = None;
    for job in &jobs {
        if let Some(cancel) = solve_cancel_info(&state, &solve_id) {
            let response = finish_cancelled_solve(
                &state,
                solve_id,
                request_id,
                Some(problem_id.clone()),
                revision,
                true,
                warnings,
                cancel.reason,
            )
            .await;
            return Ok(response);
        }
        let sequence = publish_subproblem_job(&nats, &state.jobs_subject, job).await?;
        first_job_sequence.get_or_insert(sequence);
        record_job_submitted(&state, job).await;
        state
            .metrics
            .subproblem_jobs_published_total
            .fetch_add(1, Ordering::Relaxed);
    }

    let result_consumer = build_result_consumer(
        nats.clone(),
        &result_consumer_name(&solve_id),
        &state.results_subject,
        first_job_sequence.unwrap_or(1),
    )
    .await
    .map_err(|err| format!("create result consumer: {err}"))?;
    let mut result_sub = result_consumer
        .messages()
        .await
        .map_err(|err| format!("open result consumer: {err}"))?;

    let timeout = Duration::from_millis(options.timeout_ms.unwrap_or(120_000));
    let deadline = Instant::now() + timeout;
    let mut results = Vec::new();
    let mut jobs_expected = jobs.len();
    let mut jobs_published = jobs.len();
    let mut jobs_redelegated = 0usize;
    let mut jobs_split = 0usize;
    let max_retries = options.max_job_retries.unwrap_or(2);
    let mut jobs_by_id: HashMap<String, SubproblemJob> = jobs
        .iter()
        .cloned()
        .map(|job| (job.job_id.clone(), job))
        .collect();
    let mut retry_index_by_job_id: HashMap<String, usize> =
        jobs.iter().map(|job| (job.job_id.clone(), 0)).collect();
    let mut expected_job_ids: HashSet<String> = jobs.iter().map(|job| job.job_id.clone()).collect();
    let mut completed_job_ids = HashSet::new();
    let mut superseded_job_ids = HashSet::new();
    let stale_after = worker_stale_after();
    let mut timed_out = false;
    while results.len() < jobs_expected {
        if let Some(cancel) = solve_cancel_info(&state, &solve_id) {
            let response = finish_cancelled_solve(
                &state,
                solve_id,
                request_id,
                Some(problem_id.clone()),
                revision,
                true,
                warnings,
                cancel.reason,
            )
            .await;
            return Ok(response);
        }
        requeue_stale_worker_jobs(
            &state,
            &nats,
            &solve_id,
            &request_id,
            stale_after,
            &mut expected_job_ids,
            &completed_job_ids,
            &mut jobs_by_id,
            &mut retry_index_by_job_id,
            &mut superseded_job_ids,
            &mut jobs_published,
            &mut jobs_redelegated,
            &mut warnings,
        )
        .await;
        let now = Instant::now();
        if now >= deadline {
            timed_out = true;
            break;
        }
        let remaining = deadline - now;
        let wait_for = remaining.min(cancel_poll);
        match tokio::time::timeout(wait_for, result_sub.next()).await {
            Ok(Some(Ok(message))) => {
                let parsed = serde_json::from_slice::<SubproblemResult>(&message.payload).ok();
                if let Some(result) = parsed {
                    if superseded_job_ids.contains(&result.job_id) {
                        let _ = message.ack().await;
                        continue;
                    }
                    match accept_subproblem_result(
                        result,
                        &solve_id,
                        &expected_job_ids,
                        &mut completed_job_ids,
                    ) {
                        Ok(Some(result)) => {
                            state
                                .metrics
                                .subproblem_jobs_completed_total
                                .fetch_add(1, Ordering::Relaxed);
                            if let Some(cancel) = solve_cancel_info(&state, &solve_id) {
                                let _ = message.ack().await;
                                let response = finish_cancelled_solve(
                                    &state,
                                    solve_id,
                                    request_id,
                                    Some(problem_id.clone()),
                                    revision,
                                    true,
                                    warnings,
                                    cancel.reason,
                                )
                                .await;
                                return Ok(response);
                            }
                            let retry_index = retry_index_by_job_id
                                .get(&result.job_id)
                                .copied()
                                .unwrap_or(0);
                            if result.status == "split" && !result.child_jobs.is_empty() {
                                let child_count = result.child_jobs.len();
                                let child_jobs = result.child_jobs.clone();
                                let mut published_children = Vec::with_capacity(child_count);
                                let mut publish_error = None;
                                for child in child_jobs {
                                    if let Some(cancel) = solve_cancel_info(&state, &solve_id) {
                                        let _ = message.ack().await;
                                        let response = finish_cancelled_solve(
                                            &state,
                                            solve_id,
                                            request_id,
                                            Some(problem_id.clone()),
                                            revision,
                                            true,
                                            warnings,
                                            cancel.reason,
                                        )
                                        .await;
                                        return Ok(response);
                                    }
                                    match publish_subproblem_job(&nats, &state.jobs_subject, &child)
                                        .await
                                    {
                                        Ok(_) => {
                                            record_job_submitted(&state, &child).await;
                                            expected_job_ids.insert(child.job_id.clone());
                                            retry_index_by_job_id.insert(child.job_id.clone(), 0);
                                            jobs_by_id.insert(child.job_id.clone(), child.clone());
                                            published_children.push(child.job_id);
                                            jobs_published += 1;
                                            state
                                                .metrics
                                                .subproblem_jobs_published_total
                                                .fetch_add(1, Ordering::Relaxed);
                                        }
                                        Err(error) => {
                                            publish_error = Some(error);
                                            break;
                                        }
                                    }
                                }

                                if let Some(error) = publish_error {
                                    warnings.push(format!(
                                        "failed to publish split children for job {}: {error}",
                                        result.job_id
                                    ));
                                    let mut terminal_error = result.clone();
                                    terminal_error.status = "error".to_string();
                                    terminal_error.error =
                                        Some(format!("failed to publish split children: {error}"));
                                    record_job_result(&state, &terminal_error, true).await;
                                    results.push(terminal_error);
                                    continue;
                                }

                                record_job_result(&state, &result, false).await;
                                track_job_split(&state, &solve_id, child_count);
                                jobs_expected =
                                    jobs_expected.saturating_add(child_count.saturating_sub(1));
                                jobs_split += 1;
                                snapshot_solve_state(&state, &solve_id).await;
                                snapshot_solve_frontier(&state, &solve_id, &result.child_jobs)
                                    .await;
                                state
                                    .metrics
                                    .subproblem_jobs_split_total
                                    .fetch_add(1, Ordering::Relaxed);
                                publish_event(
                                    &state,
                                    "subproblem-split",
                                    json!({
                                        "solveId": &solve_id,
                                        "requestId": &request_id,
                                        "parentJobId": &result.job_id,
                                        "childJobIds": published_children,
                                        "childCount": child_count,
                                        "workerNode": &result.worker_node,
                                        "reason": &result.error,
                                    }),
                                )
                                .await;
                                continue;
                            }
                            if should_redelegate_result(&result, retry_index, max_retries) {
                                record_job_result(&state, &result, false).await;
                                let Some(original_job) = jobs_by_id.get(&result.job_id).cloned()
                                else {
                                    warnings.push(format!(
                                        "cannot re-delegate {}; original job payload not found",
                                        result.job_id
                                    ));
                                    results.push(result);
                                    continue;
                                };
                                let next_retry_index = retry_index + 1;
                                let retry_job = redelegated_job(&original_job, next_retry_index);
                                if let Some(cancel) = solve_cancel_info(&state, &solve_id) {
                                    let _ = message.ack().await;
                                    let response = finish_cancelled_solve(
                                        &state,
                                        solve_id,
                                        request_id,
                                        Some(problem_id.clone()),
                                        revision,
                                        true,
                                        warnings,
                                        cancel.reason,
                                    )
                                    .await;
                                    return Ok(response);
                                }
                                match publish_subproblem_job(&nats, &state.jobs_subject, &retry_job)
                                    .await
                                {
                                    Ok(_) => {
                                        record_job_submitted(&state, &retry_job).await;
                                        track_job_redelegated(&state, &solve_id);
                                        snapshot_solve_state(&state, &solve_id).await;
                                        publish_event(
                                            &state,
                                            "subproblem-redelegated",
                                            json!({
                                                "solveId": &solve_id,
                                                "requestId": &request_id,
                                                "failedJobId": &result.job_id,
                                                "retryJobId": &retry_job.job_id,
                                                "retryIndex": next_retry_index,
                                                "maxRetries": max_retries,
                                                "workerNode": &result.worker_node,
                                                "error": &result.error,
                                            }),
                                        )
                                        .await;
                                        expected_job_ids.insert(retry_job.job_id.clone());
                                        retry_index_by_job_id
                                            .insert(retry_job.job_id.clone(), next_retry_index);
                                        jobs_by_id.insert(retry_job.job_id.clone(), retry_job);
                                        jobs_published += 1;
                                        jobs_redelegated += 1;
                                        state
                                            .metrics
                                            .subproblem_jobs_published_total
                                            .fetch_add(1, Ordering::Relaxed);
                                        state
                                            .metrics
                                            .subproblem_jobs_redelegated_total
                                            .fetch_add(1, Ordering::Relaxed);
                                    }
                                    Err(error) => {
                                        warnings.push(format!(
                                            "failed to re-delegate job {}: {error}",
                                            result.job_id
                                        ));
                                        record_job_result(&state, &result, true).await;
                                        results.push(result);
                                    }
                                }
                            } else {
                                record_job_result(&state, &result, true).await;
                                results.push(result);
                            }
                        }
                        Ok(None) => {}
                        Err(warning) => warnings.push(warning),
                    }
                }
                let _ = message.ack().await;
            }
            Ok(Some(Err(error))) => {
                warnings.push(format!("JetStream result consumer error: {error}"));
            }
            Ok(None) => {
                warnings.push("JetStream result consumer closed".to_string());
                timed_out = true;
                break;
            }
            Err(_) if wait_for < remaining => {
                continue;
            }
            Err(_) => {
                timed_out = true;
                break;
            }
        }
    }

    let response = aggregate_results(
        solve_id,
        request_id,
        Some(problem_id.clone()),
        revision,
        &problem,
        &options,
        jobs_expected,
        jobs_published,
        jobs_redelegated,
        jobs_split,
        results,
        timed_out,
        true,
        &state,
        warnings,
    );
    track_solve_finished(&state, &response);
    finalize_solve_state(&state, &response).await;
    publish_event(
        &state,
        "solve-finished",
        json!({
            "solveId": &response.solve_id,
            "requestId": &response.request_id,
            "status": &response.status,
            "jobsPublished": response.jobs_published,
            "jobsCompleted": response.jobs_completed,
            "jobsRedelegated": response.jobs_redelegated,
            "jobsSplit": response.jobs_split,
            "timedOut": response.timed_out,
        }),
    )
    .await;
    Ok(response)
}

async fn run_problem_task(
    state: AppState,
    request_id: String,
    problem_id: String,
    revision: u64,
    problem: MipProblemSpec,
    options: SolveOptions,
) -> Result<SolveResponse, String> {
    let task_id = problem_id.clone();
    track_runtime_task_started(
        &state,
        task_id.clone(),
        "problem",
        Some(problem_id.clone()),
        None,
        Some(request_id.clone()),
        None,
        None,
        None,
    );
    let solve_task = tokio::spawn(solve_problem_distributed(
        state.clone(),
        request_id.clone(),
        problem_id.clone(),
        revision,
        problem,
        options,
    ));
    track_runtime_task_abort_handle(&state, &task_id, solve_task.abort_handle());
    let result = solve_task
        .await
        .map_err(|err| format!("problem task failed: {err}"))?;
    match &result {
        Ok(response) => track_runtime_task_finished(&state, &task_id, &response.status),
        Err(_) => track_runtime_task_finished(&state, &task_id, "error"),
    }
    result
}

async fn run_problem_task_with_coordination(
    state: AppState,
    request_id: String,
    problem_id: String,
    revision: u64,
    problem: MipProblemSpec,
    options: SolveOptions,
) -> Result<SolveResponse, String> {
    let ttl_ms = options
        .timeout_ms
        .unwrap_or(120_000)
        .saturating_add(env_u64(
            "MIP_SOLVER_COORDINATION_SOLVE_LOCK_MARGIN_MS",
            60_000,
        ));
    let lock_key =
        mip_solver_solve_request_lock_key(&state.coordination.redis_lock_prefix, &problem_id);
    let guard = acquire_coordination_lock(&state, lock_key, ttl_ms).await?;
    let result = run_problem_task(
        state.clone(),
        request_id,
        problem_id,
        revision,
        problem,
        options,
    )
    .await;
    release_coordination_guard(&state, guard).await;
    result
}

fn response_json<T: Serialize>(status: StatusCode, value: T) -> Response {
    (status, Json(value)).into_response()
}

fn is_problem_model_conflict(error: &str) -> bool {
    error.contains("already exists with a different model")
}

fn solve_error_status(error: &str) -> StatusCode {
    if error.contains("already has running solve")
        || error.contains("coordination lock busy")
        || is_problem_model_conflict(error)
    {
        StatusCode::CONFLICT
    } else if error.contains("coordination")
        || error.contains("live-mutex")
        || error.contains("Redis")
    {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::BAD_REQUEST
    }
}

fn build_info_document() -> Value {
    json!({
        "packageVersion": env!("CARGO_PKG_VERSION"),
        "gitCommit": option_env!("DD_GIT_COMMIT").unwrap_or("unknown"),
        "gitCommitShort": option_env!("DD_GIT_COMMIT_SHORT").unwrap_or("unknown"),
        "gitRef": option_env!("DD_GIT_REF").unwrap_or("unknown"),
        "gitDirty": option_env!("DD_GIT_DIRTY").unwrap_or("unknown"),
        "builtAt": option_env!("DD_BUILD_TIME_UTC").unwrap_or("unknown"),
    })
}

fn runtime_source_document() -> Value {
    json!({
        "clusterGitUrl": optional_env_value("MIP_SOLVER_CLUSTER_GIT_URL"),
        "clusterGitRef": optional_env_value("MIP_SOLVER_CLUSTER_GIT_REF"),
        "natsUrlConfigured": optional_env_value("NATS_URL").is_some(),
        "postgresEnv": first_configured_env(&[
            "MIP_SOLVER_DATABASE_URL",
            "AGENT_TASKS_RDS_DATABASE_URL",
            "RDS_DATABASE_URL",
            "DATABASE_URL",
            "PG_DATABASE_URL",
        ]),
        "redisEnv": first_configured_env(&["MIP_SOLVER_REDIS_URL", "REDIS_URL"]),
    })
}

fn subjects_document(state: &AppState) -> Value {
    json!({
        "jobs": &state.jobs_subject,
        "results": &state.results_subject,
        "control": &state.control_subject,
        "events": &state.events_subject,
    })
}

fn links_document() -> Value {
    json!({
        "home": "/home",
        "healthz": "/healthz",
        "readyz": "/readyz",
        "version": "/version",
        "versionJson": "/version.json",
        "apiDocs": "/docs/api",
        "apiDocsJson": "/api/docs.json",
        "natsStatus": "/mip-solver-cluster/nats",
        "workers": "/mip-solver-cluster/workers",
        "solves": "/mip-solver-cluster/solves",
        "metrics": "/metrics",
        "exampleModel": "/model/example",
        "uploadProblem": "/problems/{problemId}",
        "solveProblem": "/problems/{problemId}/solve",
        "solve": "/solve",
    })
}

fn version_document(state: &AppState) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "description": SERVICE_DESCRIPTION,
        "version": env!("CARGO_PKG_VERSION"),
        "role": state.role.as_str(),
        "nodeId": &state.node_id,
        "build": build_info_document(),
        "runtime": runtime_source_document(),
    })
}

fn nats_status_document(state: &AppState) -> Value {
    let connected = state.nats.is_some();
    let workers: Vec<WorkerNodeStatus> = state
        .workers
        .lock()
        .expect("workers mutex poisoned")
        .values()
        .cloned()
        .collect();
    json!({
        "ok": connected,
        "service": SERVICE_NAME,
        "role": state.role.as_str(),
        "nodeId": &state.node_id,
        "connected": connected,
        "readyForDistributedWork": connected,
        "stream": {
            "name": DD_REMOTE_MIP_SOLVER_STREAM_NAME,
            "subjects": DD_REMOTE_MIP_SOLVER_STREAM_SUBJECTS,
        },
        "subjects": subjects_document(state),
        "workerConsumer": env_value("MIP_SOLVER_NATS_CONSUMER", MIP_SOLVER_WORKERS_QUEUE_GROUP),
        "workerQueueGroup": MIP_SOLVER_WORKERS_QUEUE_GROUP,
        "workersKnown": workers.len(),
        "workers": workers,
        "notes": [
            "masters publish subproblem jobs to the jobs subject using JetStream publish acks",
            "slaves pull from the shared durable worker consumer and publish results to the results subject",
            "slaves also publish worker-ready/request-work/worker-completed control frames that masters observe"
        ],
    })
}

fn api_docs_document(state: &AppState) -> Value {
    json!({
        "schema": API_DOCS_SCHEMA,
        "service": {
            "name": SERVICE_NAME,
            "version": env!("CARGO_PKG_VERSION"),
            "description": SERVICE_DESCRIPTION,
        },
        "build": build_info_document(),
        "nats": nats_status_document(state),
        "links": links_document(),
        "endpoints": [
            {"method": "GET", "path": "/", "kind": "json", "description": "Service descriptor with runtime subjects, persistence, GPU status, and links."},
            {"method": "GET", "path": "/home", "kind": "html", "description": "Human-readable service home page."},
            {"method": "GET", "path": "/healthz", "kind": "json", "description": "Process liveness. Does not require NATS."},
            {"method": "GET", "path": "/readyz", "kind": "json", "description": "Readiness. Requires an active NATS connection for distributed solver work."},
            {"method": "GET", "path": "/version", "kind": "html", "description": "Build and runtime version page with git commit metadata."},
            {"method": "GET", "path": "/version.json", "kind": "json", "description": "Build and runtime version metadata."},
            {"method": "GET", "path": "/docs/api", "kind": "html", "description": "Human-readable API documentation."},
            {"method": "GET", "path": "/api/docs", "kind": "html", "description": "Compatibility alias for the API documentation page."},
            {"method": "GET", "path": "/api/docs.json", "kind": "json", "description": "Machine-readable API documentation."},
            {"method": "GET", "path": "/mip-solver-cluster/nats", "kind": "json", "description": "NATS stream, subject, durable worker consumer, and observed worker status."},
            {"method": "GET", "path": "/mip-solver-cluster/workers", "kind": "json", "description": "Slave workers the master has observed through NATS control frames."},
            {"method": "GET", "path": "/mip-solver-cluster/solves", "kind": "json", "description": "Master solve registry with job counts and per-attempt status."},
            {"method": "DELETE", "path": "/mip-solver-cluster/solves/:solve_id", "kind": "json", "description": "Cancel a running solve by solve id."},
            {"method": "POST", "path": "/mip-solver-cluster/solves/:solve_id/cancel", "kind": "json", "description": "Cancel a running solve by solve id."},
            {"method": "POST", "path": "/mip-solver-cluster/requests/:request_id/cancel", "kind": "json", "description": "Cancel a running solve by request id."},
            {"method": "GET", "path": "/workers", "kind": "json", "description": "Compatibility alias for /mip-solver-cluster/workers."},
            {"method": "GET", "path": "/model/example", "kind": "json", "description": "Example knapsack MIP request payload."},
            {"method": "POST", "path": "/problems/:problem_id", "kind": "json", "description": "Stream and store a problem model by UUID for later solve-by-reference requests."},
            {"method": "POST", "path": "/problems/:problem_id/solve", "kind": "json", "description": "Solve a previously stored problem model by UUID."},
            {"method": "POST", "path": "/solve", "kind": "json", "description": "Submit a MIP/IP/LP solve request to a master node."},
            {"method": "GET", "path": "/sessions/:session_id", "kind": "json", "description": "Read a live session model snapshot."},
            {"method": "POST", "path": "/sessions/:session_id/events", "kind": "json", "description": "Apply live model editing events to a session."},
            {"method": "POST", "path": "/sessions/:session_id/solve", "kind": "json", "description": "Solve the current live session model snapshot."},
            {"method": "GET", "path": "/metrics", "kind": "text", "description": "Prometheus metrics for HTTP, NATS jobs/results, worker control, solve registry, and errors."}
        ],
    })
}

fn html_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn service_page(title: &str, subtitle: &str, body: String) -> Html<String> {
    Html(format!(
        concat!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">",
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">",
            "<title>{title}</title>",
            "<style>",
            "body{{margin:0;background:#f7f8fb;color:#18202f;font-family:ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif;}}",
            "main{{max-width:980px;margin:0 auto;padding:32px 20px 48px;}}",
            "h1{{font-size:2rem;line-height:1.15;margin:0 0 8px;letter-spacing:0;}}",
            "h2{{font-size:1rem;margin:28px 0 10px;color:#30405c;}}",
            "p{{line-height:1.55;color:#4d5b73;}}",
            "a{{color:#0b5cab;text-decoration:none;}}a:hover{{text-decoration:underline;}}",
            ".nav{{display:flex;flex-wrap:wrap;gap:10px;margin:20px 0 28px;}}",
            ".nav a,.pill{{border:1px solid #cbd5e1;border-radius:6px;padding:6px 10px;background:#fff;color:#23324a;}}",
            ".grid{{display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:12px;}}",
            ".card{{border:1px solid #d8dee8;border-radius:8px;background:#fff;padding:14px;}}",
            "code,pre{{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;}}",
            "pre{{overflow:auto;background:#101827;color:#e5edf7;border-radius:8px;padding:14px;}}",
            "table{{width:100%;border-collapse:collapse;background:#fff;border:1px solid #d8dee8;border-radius:8px;overflow:hidden;}}",
            "th,td{{text-align:left;border-bottom:1px solid #e7ebf2;padding:9px 10px;vertical-align:top;}}",
            "th{{font-size:.8rem;text-transform:uppercase;color:#5a6780;background:#f0f3f8;}}",
            ".ok{{color:#067647;}}.warn{{color:#b54708;}}",
            "</style></head><body><main>",
            "<h1>{title}</h1><p>{subtitle}</p>",
            "<nav class=\"nav\"><a href=\"/home\">Home</a><a href=\"/docs/api\">API Docs</a>",
            "<a href=\"/version\">Version</a><a href=\"/mip-solver-cluster/nats\">NATS</a>",
            "<a href=\"/healthz\">Health</a><a href=\"/readyz\">Readiness</a><a href=\"/metrics\">Metrics</a></nav>",
            "{body}</main></body></html>"
        ),
        title = html_escape(title),
        subtitle = html_escape(subtitle),
        body = body
    ))
}

async fn home(State(state): State<AppState>) -> Html<String> {
    let connected = state.nats.is_some();
    let nats_class = if connected { "ok" } else { "warn" };
    let nats_text = if connected {
        "connected"
    } else {
        "not connected"
    };
    service_page(
        SERVICE_NAME,
        SERVICE_DESCRIPTION,
        format!(
            concat!(
                "<section class=\"grid\">",
                "<div class=\"card\"><h2>Node</h2><p><strong>Role:</strong> {role}<br><strong>Node:</strong> <code>{node}</code><br><strong>Version:</strong> {version}</p></div>",
                "<div class=\"card\"><h2>NATS</h2><p><strong>Status:</strong> <span class=\"{nats_class}\">{nats_text}</span><br><strong>Stream:</strong> <code>{stream}</code><br><strong>Worker consumer:</strong> <code>{consumer}</code></p></div>",
                "<div class=\"card\"><h2>Cluster</h2><p><strong>Workers observed:</strong> {workers}<br><strong>Solves tracked:</strong> {solves}</p></div>",
                "</section>",
                "<h2>Common Routes</h2>",
                "<p><a href=\"/model/example\"><code>/model/example</code></a> gives a sample solve request. ",
                "<a href=\"/mip-solver-cluster/workers\"><code>/mip-solver-cluster/workers</code></a> shows slave heartbeats observed by the master. ",
                "<a href=\"/api/docs.json\"><code>/api/docs.json</code></a> is the machine-readable API contract.</p>"
            ),
            role = html_escape(state.role.as_str()),
            node = html_escape(&state.node_id),
            version = html_escape(env!("CARGO_PKG_VERSION")),
            nats_class = nats_class,
            nats_text = nats_text,
            stream = html_escape(DD_REMOTE_MIP_SOLVER_STREAM_NAME),
            consumer = html_escape(&env_value(
                "MIP_SOLVER_NATS_CONSUMER",
                MIP_SOLVER_WORKERS_QUEUE_GROUP,
            )),
            workers = state.workers.lock().expect("workers mutex poisoned").len(),
            solves = state.solves.lock().expect("solves mutex poisoned").len(),
        ),
    )
}

async fn version_json(State(state): State<AppState>) -> impl IntoResponse {
    Json(version_document(&state))
}

async fn version_page(State(state): State<AppState>) -> Html<String> {
    let doc = serde_json::to_string_pretty(&version_document(&state)).unwrap_or_default();
    service_page(
        "Version",
        "Build and runtime metadata for the solver node.",
        format!("<pre>{}</pre>", html_escape(&doc)),
    )
}

async fn api_docs_json(State(state): State<AppState>) -> impl IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        Json(api_docs_document(&state)),
    )
}

async fn api_docs_html(State(state): State<AppState>) -> Html<String> {
    let docs = api_docs_document(&state);
    let rows = docs
        .get("endpoints")
        .and_then(Value::as_array)
        .map(|endpoints| {
            endpoints
                .iter()
                .map(|endpoint| {
                    let method = endpoint
                        .get("method")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let path = endpoint.get("path").and_then(Value::as_str).unwrap_or("");
                    let kind = endpoint.get("kind").and_then(Value::as_str).unwrap_or("");
                    let description = endpoint
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    format!(
                        "<tr><td><code>{}</code></td><td><a href=\"{}\"><code>{}</code></a></td><td>{}</td><td>{}</td></tr>",
                        html_escape(method),
                        html_escape(path),
                        html_escape(path),
                        html_escape(kind),
                        html_escape(description),
                    )
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    service_page(
        "API Docs",
        "Canonical HTTP surface for the distributed MIP solver node.",
        format!(
            concat!(
                "<p><span class=\"pill\">schema {schema}</span><span class=\"pill\">version {version}</span>",
                "<span class=\"pill\">stream {stream}</span></p>",
                "<table><thead><tr><th>Method</th><th>Path</th><th>Kind</th><th>Description</th></tr></thead><tbody>{rows}</tbody></table>"
            ),
            schema = html_escape(API_DOCS_SCHEMA),
            version = html_escape(env!("CARGO_PKG_VERSION")),
            stream = html_escape(DD_REMOTE_MIP_SOLVER_STREAM_NAME),
            rows = rows,
        ),
    )
}

async fn nats_status(State(state): State<AppState>) -> impl IntoResponse {
    Json(nats_status_document(&state))
}

async fn root(State(state): State<AppState>) -> impl IntoResponse {
    let tasks = runtime_task_entries(&state);
    let active_tasks = tasks
        .iter()
        .filter(|task| task.finished_at_ms.is_none())
        .count();
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "description": SERVICE_DESCRIPTION,
        "version": env!("CARGO_PKG_VERSION"),
        "role": state.role.as_str(),
        "nodeId": &state.node_id,
        "subjects": subjects_document(&state),
        "stream": DD_REMOTE_MIP_SOLVER_STREAM_NAME,
        "queueGroup": MIP_SOLVER_WORKERS_QUEUE_GROUP,
        "workersKnown": state.workers.lock().expect("workers mutex poisoned").len(),
        "tasksTracked": tasks.len(),
        "activeTasks": active_tasks,
        "solvesTracked": state.solves.lock().expect("solves mutex poisoned").len(),
        "nats": nats_status_document(&state),
        "build": build_info_document(),
        "links": links_document(),
        "persistence": persistence_contract(),
        "gpu": gpu_status(),
    }))
}

async fn healthz() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "gitCommit": option_env!("DD_GIT_COMMIT_SHORT").unwrap_or("unknown"),
    }))
}

async fn readyz(State(state): State<AppState>) -> Response {
    let nats_ready = state.nats.is_some();
    let status = if nats_ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    response_json(
        status,
        json!({
            "ok": nats_ready,
            "service": SERVICE_NAME,
            "version": env!("CARGO_PKG_VERSION"),
            "role": state.role.as_str(),
            "nodeId": &state.node_id,
            "nats": nats_ready,
            "subjects": subjects_document(&state),
            "stream": DD_REMOTE_MIP_SOLVER_STREAM_NAME,
            "reason": if nats_ready { Value::Null } else { json!("NATS connection is required for distributed solver readiness") },
        }),
    )
}

fn prometheus_label_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let m = &state.metrics;
    let published = m.subproblem_jobs_published_total.load(Ordering::Relaxed);
    let completed = m.subproblem_jobs_completed_total.load(Ordering::Relaxed);
    let in_flight = published.saturating_sub(completed);
    let solves = state.solves.lock().expect("solves mutex poisoned");
    let solves_tracked = solves.len();
    let active_solves = solves
        .values()
        .filter(|solve| solve.finished_at_ms.is_none())
        .count();
    drop(solves);
    let node_id = prometheus_label_value(&state.node_id);
    let role = prometheus_label_value(state.role.as_str());
    let body = format!(
        concat!(
            "# HELP dd_mip_solver_node_info Static solver node metadata.\n",
            "# TYPE dd_mip_solver_node_info gauge\n",
            "dd_mip_solver_node_info{{role=\"{}\",node_id=\"{}\"}} 1\n",
            "# HELP dd_mip_solver_http_requests_total Total HTTP requests handled by this node.\n",
            "# TYPE dd_mip_solver_http_requests_total counter\n",
            "dd_mip_solver_http_requests_total {}\n",
            "# HELP dd_mip_solver_stream_events_total Total live model stream events applied by this node.\n",
            "# TYPE dd_mip_solver_stream_events_total counter\n",
            "dd_mip_solver_stream_events_total {}\n",
            "# HELP dd_mip_solver_solve_requests_total Total solve requests handled by this node.\n",
            "# TYPE dd_mip_solver_solve_requests_total counter\n",
            "dd_mip_solver_solve_requests_total {}\n",
            "# HELP dd_mip_solver_solve_cancel_requests_total Total top-level solve cancellation requests accepted by this node.\n",
            "# TYPE dd_mip_solver_solve_cancel_requests_total counter\n",
            "dd_mip_solver_solve_cancel_requests_total {}\n",
            "# HELP dd_mip_solver_subproblem_jobs_published_total Total NATS subproblem jobs published by masters.\n",
            "# TYPE dd_mip_solver_subproblem_jobs_published_total counter\n",
            "dd_mip_solver_subproblem_jobs_published_total {}\n",
            "# HELP dd_mip_solver_subproblem_jobs_completed_total Total expected NATS subproblem results accepted by masters.\n",
            "# TYPE dd_mip_solver_subproblem_jobs_completed_total counter\n",
            "dd_mip_solver_subproblem_jobs_completed_total {}\n",
            "# HELP dd_mip_solver_subproblem_jobs_in_flight Current master-observed subproblem jobs awaiting accepted results.\n",
            "# TYPE dd_mip_solver_subproblem_jobs_in_flight gauge\n",
            "dd_mip_solver_subproblem_jobs_in_flight {}\n",
            "# HELP dd_mip_solver_subproblem_jobs_redelegated_total Total errored subproblem jobs re-published by masters.\n",
            "# TYPE dd_mip_solver_subproblem_jobs_redelegated_total counter\n",
            "dd_mip_solver_subproblem_jobs_redelegated_total {}\n",
            "# HELP dd_mip_solver_subproblem_jobs_split_total Total accepted subproblem attempts split into child jobs by masters.\n",
            "# TYPE dd_mip_solver_subproblem_jobs_split_total counter\n",
            "dd_mip_solver_subproblem_jobs_split_total {}\n",
            "# HELP dd_mip_solver_workers_known Current worker nodes observed through NATS control messages.\n",
            "# TYPE dd_mip_solver_workers_known gauge\n",
            "dd_mip_solver_workers_known {}\n",
            "# HELP dd_mip_solver_worker_control_messages_total Total worker control messages received by master nodes.\n",
            "# TYPE dd_mip_solver_worker_control_messages_total counter\n",
            "dd_mip_solver_worker_control_messages_total {}\n",
            "# HELP dd_mip_solver_solves_tracked Current solve records retained in master memory.\n",
            "# TYPE dd_mip_solver_solves_tracked gauge\n",
            "dd_mip_solver_solves_tracked {}\n",
            "# HELP dd_mip_solver_active_solves Current solves without a terminal aggregate response.\n",
            "# TYPE dd_mip_solver_active_solves gauge\n",
            "dd_mip_solver_active_solves {}\n",
            "# HELP dd_mip_solver_slave_jobs_processed_total Total subproblem jobs processed by slave nodes.\n",
            "# TYPE dd_mip_solver_slave_jobs_processed_total counter\n",
            "dd_mip_solver_slave_jobs_processed_total {}\n",
            "# HELP dd_mip_solver_errors_total Total errors observed by this node.\n",
            "# TYPE dd_mip_solver_errors_total counter\n",
            "dd_mip_solver_errors_total {}\n"
        ),
        role,
        node_id,
        m.http_requests_total.load(Ordering::Relaxed),
        m.stream_events_total.load(Ordering::Relaxed),
        m.solve_requests_total.load(Ordering::Relaxed),
        m.solve_cancel_requests_total.load(Ordering::Relaxed),
        published,
        completed,
        in_flight,
        m.subproblem_jobs_redelegated_total.load(Ordering::Relaxed),
        m.subproblem_jobs_split_total.load(Ordering::Relaxed),
        state.workers.lock().expect("workers mutex poisoned").len(),
        m.worker_control_messages_total.load(Ordering::Relaxed),
        solves_tracked,
        active_solves,
        m.slave_jobs_processed_total.load(Ordering::Relaxed),
        m.errors_total.load(Ordering::Relaxed),
    );
    ([("Content-Type", "text/plain; version=0.0.4")], body)
}

async fn example() -> impl IntoResponse {
    Json(json!({
        "requestId": "knapsack-demo",
        "problem": {
            "sense": "max",
            "c": [10.0, 40.0, 30.0, 50.0],
            "a": [[5.0, 4.0, 6.0, 3.0]],
            "b": [10.0],
            "integerVars": [true, true, true, true],
            "ub": [1.0, 1.0, 1.0, 1.0],
            "varNames": ["item0", "item1", "item2", "item3"]
        },
        "options": {
            "splitDepth": 2,
            "maxNodes": 10000,
            "timeoutMs": 120000
        }
    }))
}

async fn read_limited_body(body: Body) -> Result<Vec<u8>, String> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("read request body: {error}"))?;
        let next_len = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| "request body is too large".to_string())?;
        if next_len > MAX_HTTP_BODY_BYTES {
            return Err(format!(
                "request body exceeds {} bytes",
                MAX_HTTP_BODY_BYTES
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn problem_from_upload_bytes(bytes: &[u8]) -> Result<MipProblemSpec, String> {
    if let Ok(problem) = serde_json::from_slice::<MipProblemSpec>(bytes) {
        return normalized_problem(problem);
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| format!("parse problem JSON: {error}"))?;
    let problem = value
        .get("problem")
        .cloned()
        .ok_or_else(|| {
            "problem upload must be a problem object or contain a problem field".to_string()
        })
        .and_then(|problem| {
            serde_json::from_value::<MipProblemSpec>(problem)
                .map_err(|error| format!("parse problem field: {error}"))
        })?;
    normalized_problem(problem)
}

async fn upload_problem(
    State(state): State<AppState>,
    AxumPath(problem_id): AxumPath<String>,
    body: Body,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if state.role != NodeRole::Master {
        return response_json(
            StatusCode::CONFLICT,
            json!({"ok":false,"error":"this pod booted as slave and will not store problems"}),
        );
    }
    let problem_id = match Uuid::parse_str(problem_id.trim()) {
        Ok(uuid) => uuid.to_string(),
        Err(_) => {
            return response_json(
                StatusCode::BAD_REQUEST,
                json!({"ok":false,"error":"problem id must be a UUID"}),
            )
        }
    };
    let body = match read_limited_body(body).await {
        Ok(body) => body,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return response_json(
                StatusCode::PAYLOAD_TOO_LARGE,
                json!({"ok":false,"error":error}),
            );
        }
    };
    let problem = match problem_from_upload_bytes(&body) {
        Ok(problem) => problem,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return response_json(StatusCode::BAD_REQUEST, json!({"ok":false,"error":error}));
        }
    };
    let revision = 0;
    let store_status = match store_problem_model(&state, &problem_id, revision, &problem).await {
        Ok(status) => status,
        Err(error) if state.redis.is_none() => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return response_json(
                solve_error_status(&error),
                json!({"ok":false,"error":error}),
            );
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return response_json(
                solve_error_status(&error),
                json!({"ok":false,"error":error}),
            );
        }
    };
    let redis_stored = state.redis.is_some();
    response_json(
        StatusCode::OK,
        json!({
            "ok": true,
            "problemId": problem_id,
            "revision": revision,
            "stored": true,
            "storeStatus": store_status.as_str(),
            "redisStored": redis_stored,
            "distributedReady": redis_stored,
            "bytes": body.len(),
            "variables": problem.c.len(),
            "constraints": problem.a.len(),
        }),
    )
}

async fn solve_http(
    State(state): State<AppState>,
    Json(input): Json<SolveHttpRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .solve_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if state.role != NodeRole::Master {
        return response_json(
            StatusCode::CONFLICT,
            json!({"ok":false,"error":"this pod booted as slave and will not act as master"}),
        );
    }
    let request_id = request_id(input.request_id);
    let problem_id = match problem_id(input.problem_id, &request_id) {
        Ok(problem_id) => problem_id,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return response_json(StatusCode::BAD_REQUEST, json!({"ok":false,"error":error}));
        }
    };
    let options = SolveOptions::merged(input.options);
    let (problem, revision) = if let Some(problem) = input.problem {
        (problem, 0)
    } else if let Some(commands) = input.commands {
        match parse_problem_from_commands(&commands) {
            Ok((problem, revision, _frames)) => (problem, revision),
            Err(error) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                return response_json(StatusCode::BAD_REQUEST, json!({"ok":false,"error":error}));
            }
        }
    } else {
        return response_json(
            StatusCode::BAD_REQUEST,
            json!({"ok":false,"error":"request needs either problem or commands"}),
        );
    };

    match run_problem_task_with_coordination(
        state.clone(),
        request_id,
        problem_id,
        revision,
        problem,
        options,
    )
    .await
    {
        Ok(response) => response_json(StatusCode::OK, response),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            response_json(
                solve_error_status(&error),
                json!({"ok":false,"error":error}),
            )
        }
    }
}

async fn solve_stored_problem(
    State(state): State<AppState>,
    AxumPath(problem_id): AxumPath<String>,
    Json(input): Json<StoredProblemSolveRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .solve_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if state.role != NodeRole::Master {
        return response_json(
            StatusCode::CONFLICT,
            json!({"ok":false,"error":"this pod booted as slave and will not act as master"}),
        );
    }
    let problem_id = match Uuid::parse_str(problem_id.trim()) {
        Ok(uuid) => uuid.to_string(),
        Err(_) => {
            return response_json(
                StatusCode::BAD_REQUEST,
                json!({"ok":false,"error":"problem id must be a UUID"}),
            )
        }
    };
    let revision = 0;
    let Some(problem) = load_problem_model(&state, &problem_id, revision).await else {
        return response_json(
            StatusCode::NOT_FOUND,
            json!({"ok":false,"error":"stored problem not found"}),
        );
    };
    let request_id = request_id(input.request_id);
    let options = SolveOptions::merged(input.options);
    match run_problem_task_with_coordination(
        state.clone(),
        request_id,
        problem_id,
        revision,
        problem,
        options,
    )
    .await
    {
        Ok(response) => response_json(StatusCode::OK, response),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            response_json(
                solve_error_status(&error),
                json!({"ok":false,"error":error}),
            )
        }
    }
}

async fn stream_session(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(input): Json<Value>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let commands = match input {
        Value::Array(items) => items,
        value => vec![value],
    };
    if commands.len() > MAX_STREAM_COMMANDS {
        return response_json(
            StatusCode::BAD_REQUEST,
            json!({"ok":false,"error":"too many stream commands"}),
        );
    }

    if !state.coordination.enabled() && state.pg.is_none() {
        let mut frames = Vec::new();
        let (revision, session_snapshot) = {
            let mut sessions = state.sessions.lock().expect("sessions mutex poisoned");
            let session = sessions.entry(session_id.clone()).or_insert(LiveSession {
                problem: None,
                revision: 0,
            });
            for command in &commands {
                if let Err(error) = apply_stream_command(
                    &mut session.problem,
                    &mut session.revision,
                    command,
                    &mut frames,
                ) {
                    state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                    frames
                        .push(json!({"event":"error","message":error,"revision":session.revision}));
                } else {
                    state
                        .metrics
                        .stream_events_total
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            (session.revision, session.clone())
        };
        snapshot_session_model(&state, &session_id, session_snapshot).await;
        return response_json(
            StatusCode::OK,
            json!({
                "ok": true,
                "sessionId": session_id,
                "revision": revision,
                "frames": frames,
            }),
        );
    }

    let lock_key =
        mip_solver_session_revision_lock_key(&state.coordination.redis_lock_prefix, &session_id);
    let guard = match acquire_coordination_lock(&state, lock_key, state.coordination.ttl_ms).await {
        Ok(guard) => guard,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return response_json(
                solve_error_status(&error),
                json!({"ok":false,"error":error}),
            );
        }
    };

    let mut frames = Vec::new();
    let mut session = load_session_model(&state, &session_id)
        .await
        .unwrap_or(LiveSession {
            problem: None,
            revision: 0,
        });
    let expected_revision = session.revision;
    for command in &commands {
        if let Err(error) = apply_stream_command(
            &mut session.problem,
            &mut session.revision,
            command,
            &mut frames,
        ) {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            frames.push(json!({"event":"error","message":error,"revision":session.revision}));
        } else {
            state
                .metrics
                .stream_events_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    let response = match persist_session_model_checked(
        &state,
        &session_id,
        &session,
        expected_revision,
    )
    .await
    {
        Ok(true) => {
            state
                .sessions
                .lock()
                .expect("sessions mutex poisoned")
                .insert(session_id.clone(), session.clone());
            cache_session_model(&state, &session_id, session.clone()).await;
            response_json(
                StatusCode::OK,
                json!({
                    "ok": true,
                    "sessionId": session_id,
                    "revision": session.revision,
                    "frames": frames,
                }),
            )
        }
        Ok(false) => response_json(
            StatusCode::CONFLICT,
            json!({"ok":false,"error":"session revision changed before this update could be persisted"}),
        ),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            response_json(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"ok":false,"error":error}),
            )
        }
    };
    release_coordination_guard(&state, guard).await;
    response
}

async fn get_session(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
) -> Response {
    match load_session_model(&state, &session_id).await {
        Some(session) => response_json(
            StatusCode::OK,
            json!({
                "ok": true,
                "sessionId": session_id,
                "revision": session.revision,
                "problem": session.problem,
            }),
        ),
        None => response_json(
            StatusCode::NOT_FOUND,
            json!({"ok":false,"error":"session not found"}),
        ),
    }
}

async fn workers(State(state): State<AppState>) -> Response {
    let mut workers: Vec<WorkerNodeStatus> = state
        .workers
        .lock()
        .expect("workers mutex poisoned")
        .values()
        .cloned()
        .collect();
    workers.sort_by(|left, right| left.node_id.cmp(&right.node_id));
    response_json(
        StatusCode::OK,
        json!({
            "ok": true,
            "role": state.role.as_str(),
            "count": workers.len(),
            "workers": workers,
        }),
    )
}

async fn runtime_tasks(State(state): State<AppState>) -> Response {
    let tasks = runtime_task_entries(&state);
    let active = tasks
        .iter()
        .filter(|task| task.finished_at_ms.is_none())
        .count();
    response_json(
        StatusCode::OK,
        json!({
            "ok": true,
            "role": state.role.as_str(),
            "nodeId": state.node_id,
            "count": tasks.len(),
            "active": active,
            "tasks": tasks,
        }),
    )
}

async fn runtime_task(
    State(state): State<AppState>,
    AxumPath(task_id): AxumPath<String>,
) -> Response {
    match runtime_task_lookup(&state, task_id.trim()) {
        Some(task) => response_json(
            StatusCode::OK,
            json!({
                "ok": true,
                "role": state.role.as_str(),
                "nodeId": state.node_id,
                "task": task,
            }),
        ),
        None => response_json(
            StatusCode::NOT_FOUND,
            json!({"ok":false,"error":format!("runtime task not found: {task_id}")}),
        ),
    }
}

async fn cluster_solves(
    State(state): State<AppState>,
    Query(query): Query<ClusterSolvesQuery>,
) -> Response {
    let problem_filter = match query.problem.as_deref() {
        Some(problem) if problem.trim().is_empty() => {
            return response_json(
                StatusCode::BAD_REQUEST,
                json!({"ok":false,"error":"problem query parameter must be a UUID"}),
            );
        }
        Some(problem) => match Uuid::parse_str(problem.trim()) {
            Ok(uuid) => Some(uuid),
            Err(_) => {
                return response_json(
                    StatusCode::BAD_REQUEST,
                    json!({"ok":false,"error":"problem query parameter must be a UUID"}),
                );
            }
        },
        None => None,
    };
    let mut solves: Vec<SolveRegistryEntry> = state
        .solves
        .lock()
        .expect("solves mutex poisoned")
        .values()
        .cloned()
        .collect();
    if let Some(problem_uuid) = problem_filter.as_ref() {
        solves.retain(|solve| {
            solve
                .problem_id
                .as_deref()
                .or(Some(solve.request_id.as_str()))
                .and_then(|id| Uuid::parse_str(id).ok())
                .map(|request_uuid| request_uuid == *problem_uuid)
                .unwrap_or(false)
        });
    }
    solves.sort_by(|left, right| {
        right
            .started_at_ms
            .cmp(&left.started_at_ms)
            .then_with(|| left.solve_id.cmp(&right.solve_id))
    });
    let active = solves
        .iter()
        .filter(|solve| solve.finished_at_ms.is_none())
        .count();
    response_json(
        StatusCode::OK,
        json!({
            "ok": true,
            "role": state.role.as_str(),
            "problem": problem_filter.map(|uuid| uuid.to_string()),
            "count": solves.len(),
            "active": active,
            "solves": solves,
        }),
    )
}

async fn publish_solve_cancel(state: &AppState, info: &CancelInfo) {
    publish_control(
        state,
        "cancel-solve",
        json!({
            "solveId": &info.solve_id,
            "requestId": &info.request_id,
            "problemId": &info.problem_id,
            "reason": &info.reason,
            "requestedBy": &info.requested_by,
            "requestedAtMs": info.requested_at_ms,
        }),
    )
    .await;
    publish_event(
        state,
        "solve-cancel-requested",
        json!({
            "solveId": &info.solve_id,
            "requestId": &info.request_id,
            "problemId": &info.problem_id,
            "reason": &info.reason,
            "requestedBy": &info.requested_by,
        }),
    )
    .await;
}

async fn cancel_solve_key(state: AppState, key: String, input: CancelSolveRequest) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if state.role != NodeRole::Master {
        return response_json(
            StatusCode::CONFLICT,
            json!({"ok":false,"error":"this pod booted as slave and will not cancel master solves"}),
        );
    }
    let reason = input
        .reason
        .unwrap_or_else(|| "cancel requested by client".to_string());
    let requested_by = input.requested_by.unwrap_or_else(|| state.node_id.clone());
    match request_solve_cancel(&state, &key, reason, requested_by) {
        Ok(info) => {
            snapshot_solve_state(&state, &info.solve_id).await;
            publish_solve_cancel(&state, &info).await;
            response_json(
                StatusCode::ACCEPTED,
                json!({
                    "ok": true,
                    "status": "cancelling",
                    "solveId": info.solve_id,
                    "requestId": info.request_id,
                    "problemId": info.problem_id,
                    "reason": info.reason,
                    "requestedBy": info.requested_by,
                    "requestedAtMs": info.requested_at_ms,
                }),
            )
        }
        Err(error) => response_json(StatusCode::NOT_FOUND, json!({"ok":false,"error":error})),
    }
}

async fn cancel_solve(
    State(state): State<AppState>,
    AxumPath(solve_id): AxumPath<String>,
    Json(input): Json<CancelSolveRequest>,
) -> Response {
    cancel_solve_key(state, solve_id, input).await
}

async fn cancel_solve_default(
    State(state): State<AppState>,
    AxumPath(solve_id): AxumPath<String>,
) -> Response {
    match cleanup_finished_solve(&state, &solve_id) {
        Ok(Some(entry)) => {
            state
                .metrics
                .http_requests_total
                .fetch_add(1, Ordering::Relaxed);
            return response_json(
                StatusCode::OK,
                json!({
                    "ok": true,
                    "status": "cleaned",
                    "solveId": entry.solve_id,
                    "requestId": entry.request_id,
                    "previousStatus": entry.status,
                    "jobsRemoved": entry.jobs.len(),
                }),
            );
        }
        Ok(None) => {}
        Err(_) => {}
    }
    cancel_solve_key(state, solve_id, CancelSolveRequest::default()).await
}

async fn cancel_request(
    State(state): State<AppState>,
    AxumPath(request_id): AxumPath<String>,
    Json(input): Json<CancelSolveRequest>,
) -> Response {
    cancel_solve_key(state, request_id, input).await
}

async fn cancel_problem(
    State(state): State<AppState>,
    AxumPath(problem_id): AxumPath<String>,
    Json(input): Json<CancelSolveRequest>,
) -> Response {
    match Uuid::parse_str(problem_id.trim()) {
        Ok(uuid) => cancel_solve_key(state, uuid.to_string(), input).await,
        Err(_) => response_json(
            StatusCode::BAD_REQUEST,
            json!({"ok":false,"error":"problem id must be a UUID"}),
        ),
    }
}

async fn solve_session(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(input): Json<SolveHttpRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .solve_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if state.role != NodeRole::Master {
        return response_json(
            StatusCode::CONFLICT,
            json!({"ok":false,"error":"this pod booted as slave and will not act as master"}),
        );
    }

    let lock_key =
        mip_solver_session_revision_lock_key(&state.coordination.redis_lock_prefix, &session_id);
    let guard = match acquire_coordination_lock(&state, lock_key, state.coordination.ttl_ms).await {
        Ok(guard) => guard,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return response_json(
                solve_error_status(&error),
                json!({"ok":false,"error":error}),
            );
        }
    };
    let snapshot = match load_session_model(&state, &session_id).await {
        Some(session) => match session.problem.clone() {
            Some(problem) => Ok((problem, session.revision)),
            None => Err(response_json(
                StatusCode::BAD_REQUEST,
                json!({"ok":false,"error":"session has no initialized problem"}),
            )),
        },
        None => Err(response_json(
            StatusCode::NOT_FOUND,
            json!({"ok":false,"error":"session not found"}),
        )),
    };
    release_coordination_guard(&state, guard).await;
    let (problem, revision) = match snapshot {
        Ok(snapshot) => snapshot,
        Err(response) => return response,
    };

    let request_id = request_id(input.request_id.or(Some(session_id)));
    let problem_id = match problem_id(input.problem_id, &request_id) {
        Ok(problem_id) => problem_id,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return response_json(StatusCode::BAD_REQUEST, json!({"ok":false,"error":error}));
        }
    };
    let options = SolveOptions::merged(input.options);
    match run_problem_task_with_coordination(
        state.clone(),
        request_id,
        problem_id,
        revision,
        problem,
        options,
    )
    .await
    {
        Ok(response) => response_json(StatusCode::OK, response),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            response_json(
                solve_error_status(&error),
                json!({"ok":false,"error":error}),
            )
        }
    }
}

async fn build_jetstream_consumer(
    client: async_nats::Client,
    consumer_name: &str,
    jobs_subject: &str,
    ack_wait: Duration,
    max_ack_pending: i64,
    max_deliver: i64,
) -> Result<async_nats::jetstream::consumer::PullConsumer, Box<dyn Error + Send + Sync>> {
    let stream = ensure_mip_stream(client).await?;
    let config = worker_consumer_config(
        consumer_name,
        jobs_subject,
        ack_wait,
        max_ack_pending,
        max_deliver,
    );
    let consumer = stream
        .get_or_create_consumer::<async_nats::jetstream::consumer::pull::Config>(
            consumer_name,
            config,
        )
        .await?;
    Ok(consumer)
}

fn worker_ready_payload(state: &AppState, consumer_name: &str) -> Value {
    json!({
        "consumer": consumer_name,
        "jobsSubject": &state.jobs_subject,
        "resultsSubject": &state.results_subject,
    })
}

fn worker_job_payload(
    state: &AppState,
    consumer_name: &str,
    job: &SubproblemJob,
    started_at_ms: u128,
    max_in_flight: usize,
) -> Value {
    json!({
        "consumer": consumer_name,
        "jobId": &job.job_id,
        "jobUuid": &job.job_uuid,
        "solveId": &job.solve_id,
        "problemId": &job.problem_id,
        "startedAtMs": started_at_ms,
        "jobsSubject": &state.jobs_subject,
        "resultsSubject": &state.results_subject,
        "maxInFlight": max_in_flight,
    })
}

async fn publish_worker_ready(state: &AppState, consumer_name: &str) {
    publish_control(
        state,
        "worker-ready",
        worker_ready_payload(state, consumer_name),
    )
    .await;
}

async fn run_worker_heartbeat(
    state: AppState,
    consumer_name: String,
    heartbeat_interval: Duration,
) {
    loop {
        tokio::time::sleep(heartbeat_interval).await;
        publish_worker_ready(&state, &consumer_name).await;
    }
}

async fn run_slave(state: AppState) -> Result<(), Box<dyn Error + Send + Sync>> {
    let Some(nats) = state.nats.clone() else {
        eprintln!("slave role requires NATS_URL");
        return Ok(());
    };
    let consumer_name = env_value("MIP_SOLVER_NATS_CONSUMER", MIP_SOLVER_WORKERS_QUEUE_GROUP);
    let worker_heartbeat_interval =
        Duration::from_secs(env_u64("MIP_SOLVER_WORKER_HEARTBEAT_SECONDS", 25).max(1));
    let max_in_flight = env_usize("MIP_SOLVER_WORKER_MAX_IN_FLIGHT", 5).clamp(1, 128);
    let saturation_retry =
        Duration::from_secs(env_u64("MIP_SOLVER_WORKER_SATURATION_RETRY_SECONDS", 180).max(1));
    let ack_wait = Duration::from_secs(env_u64("MIP_SOLVER_ACK_WAIT_SECONDS", 600));
    let max_ack_pending = env_u64("MIP_SOLVER_MAX_ACK_PENDING", 32) as i64;
    let max_deliver = env_u64("MIP_SOLVER_MAX_DELIVER", 5) as i64;
    let consumer = build_jetstream_consumer(
        nats.clone(),
        &consumer_name,
        &state.jobs_subject,
        ack_wait,
        max_ack_pending,
        max_deliver,
    )
    .await?;
    let mut messages = consumer.messages().await?;
    publish_event(
        &state,
        "slave-started",
        json!({"consumer": &consumer_name, "jobsSubject": &state.jobs_subject}),
    )
    .await;
    publish_worker_ready(&state, &consumer_name).await;
    tokio::spawn(run_worker_heartbeat(
        state.clone(),
        consumer_name.clone(),
        worker_heartbeat_interval,
    ));
    let worker_slots = Arc::new(tokio::sync::Semaphore::new(max_in_flight));
    let cancel_poll = cancel_poll_interval();

    while let Some(message) = messages.next().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                eprintln!("mip solver worker message fetch failed: {error}");
                continue;
            }
        };
        let job = match serde_json::from_slice::<SubproblemJob>(&message.payload) {
            Ok(job) => job,
            Err(error) => {
                eprintln!("invalid mip solver job payload: {error}");
                let _ = message.ack().await;
                continue;
            }
        };
        if job
            .avoid_worker_nodes
            .iter()
            .any(|node| node == &state.node_id)
        {
            publish_control(
                &state,
                "worker-avoided-stale-retry",
                json!({
                    "consumer": &consumer_name,
                    "jobId": &job.job_id,
                    "jobUuid": &job.job_uuid,
                    "solveId": &job.solve_id,
                    "problemId": &job.problem_id,
                    "retryAfterSeconds": saturation_retry.as_secs(),
                }),
            )
            .await;
            let _ = message
                .ack_with(async_nats::jetstream::AckKind::Nak(Some(saturation_retry)))
                .await;
            continue;
        }
        if solve_cancel_requested_for(&state, &job.solve_id, job.problem_id.as_deref()) {
            publish_control(
                &state,
                "worker-skipped-cancelled",
                json!({
                    "consumer": &consumer_name,
                    "jobId": &job.job_id,
                    "jobUuid": &job.job_uuid,
                    "solveId": &job.solve_id,
                    "problemId": &job.problem_id,
                }),
            )
            .await;
            let _ = message.ack().await;
            continue;
        }
        let job_problem_id_for_cancel = job.problem_id.clone();
        let permit = match worker_slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                publish_control(
                    &state,
                    "worker-saturated",
                    json!({
                        "consumer": &consumer_name,
                        "jobId": &job.job_id,
                        "jobUuid": &job.job_uuid,
                        "solveId": &job.solve_id,
                        "problemId": &job.problem_id,
                        "maxInFlight": max_in_flight,
                        "retryAfterSeconds": saturation_retry.as_secs(),
                    }),
                )
                .await;
                let _ = message
                    .ack_with(async_nats::jetstream::AckKind::Nak(Some(saturation_retry)))
                    .await;
                continue;
            }
        };
        let worker_node = state.node_id.clone();
        let state_for_task = state.clone();
        let state_for_registry = state.clone();
        let nats_for_task = nats.clone();
        let consumer_name_for_task = consumer_name.clone();
        let task_id = job.job_uuid.clone();
        let task_problem_id = job.problem_id.clone();
        let task_solve_id = job.solve_id.clone();
        let task_request_id = job.request_id.clone();
        let task_job_id = job.job_id.clone();
        let job_started_at_ms = now_ms();
        track_runtime_task_started(
            &state_for_registry,
            task_id.clone(),
            "worker-subproblem",
            task_problem_id,
            Some(task_solve_id),
            Some(task_request_id),
            Some(task_job_id),
            Some(task_id.clone()),
            None,
        );
        let task_id_for_task = task_id.clone();
        let worker_task = tokio::spawn(async move {
            let _permit = permit;
            let _task_guard = RuntimeTaskFinishGuard::new(
                state_for_task.clone(),
                task_id_for_task.clone(),
                "finished",
            );
            publish_control(
                &state_for_task,
                "request-work",
                worker_job_payload(
                    &state_for_task,
                    &consumer_name_for_task,
                    &job,
                    job_started_at_ms,
                    max_in_flight,
                ),
            )
            .await;
            if solve_cancel_requested_for(&state_for_task, &job.solve_id, job.problem_id.as_deref())
            {
                publish_control(
                    &state_for_task,
                    "worker-skipped-cancelled",
                    json!({
                        "consumer": &consumer_name_for_task,
                        "jobId": &job.job_id,
                        "jobUuid": &job.job_uuid,
                        "solveId": &job.solve_id,
                        "problemId": &job.problem_id,
                    }),
                )
                .await;
                track_runtime_task_finished(&state_for_task, &task_id_for_task, "cancelled");
                let _ = message.ack().await;
                return;
            }
            let job_id_for_cancel = job.job_id.clone();
            let job_uuid_for_cancel = job.job_uuid.clone();
            let solve_id_for_cancel = job.solve_id.clone();
            let problem_id_for_cancel = job_problem_id_for_cancel.clone();
            let progress_payload = worker_job_payload(
                &state_for_task,
                &consumer_name_for_task,
                &job,
                job_started_at_ms,
                max_in_flight,
            );
            let mut progress_tick = tokio::time::interval_at(
                tokio::time::Instant::now() + worker_heartbeat_interval,
                worker_heartbeat_interval,
            );
            let mut job = job;
            let result = match hydrate_subproblem_job(&state_for_task, &mut job).await {
                Ok(()) => {
                    let mut solve_task =
                        tokio::task::spawn_blocking(move || solve_subproblem(job, worker_node));
                    loop {
                        tokio::select! {
                            joined = &mut solve_task => {
                                break match joined {
                                    Ok(result) => result,
                                    Err(error) => {
                                        eprintln!("mip solver worker task failed: {error}");
                                            let _ = message
                                                .ack_with(async_nats::jetstream::AckKind::Nak(Some(
                                                    Duration::from_secs(5),
                                                )))
                                                .await;
                                            track_runtime_task_finished(
                                                &state_for_task,
                                                &task_id_for_task,
                                                "error",
                                            );
                                            return;
                                        }
                                };
                            }
                            _ = tokio::time::sleep(cancel_poll) => {
                                if solve_cancel_requested_for(
                                    &state_for_task,
                                    &solve_id_for_cancel,
                                    problem_id_for_cancel.as_deref(),
                                ) {
                                    publish_control(
                                        &state_for_task,
                                        "worker-stopped-cancelled",
                                        json!({
                                            "consumer": &consumer_name_for_task,
                                            "jobId": &job_id_for_cancel,
                                            "jobUuid": &job_uuid_for_cancel,
                                            "solveId": &solve_id_for_cancel,
                                            "problemId": &problem_id_for_cancel,
                                        }),
                                        )
                                        .await;
                                        track_runtime_task_finished(
                                            &state_for_task,
                                            &task_id_for_task,
                                            "cancelled",
                                        );
                                        let _ = message.ack().await;
                                        return;
                                    }
                            }
                            _ = progress_tick.tick() => {
                                publish_control(
                                    &state_for_task,
                                    "worker-progress",
                                    progress_payload.clone(),
                                )
                                .await;
                            }
                        }
                    }
                }
                Err(error) => failed_subproblem(
                    job,
                    worker_node,
                    AcceleratorReport::runtime(),
                    error,
                    Instant::now(),
                ),
            };
            if solve_cancel_requested_for(
                &state_for_task,
                &result.solve_id,
                result.problem_id.as_deref(),
            ) {
                publish_control(
                    &state_for_task,
                    "worker-discarded-cancelled-result",
                    json!({
                        "consumer": &consumer_name_for_task,
                        "jobId": &result.job_id,
                        "jobUuid": &result.job_uuid,
                        "solveId": &result.solve_id,
                        "problemId": &result.problem_id,
                        "status": &result.status,
                    }),
                )
                .await;
                track_runtime_task_finished(&state_for_task, &task_id_for_task, "cancelled");
                let _ = message.ack().await;
                return;
            }
            let payload = match serde_json::to_vec(&result) {
                Ok(payload) => payload,
                Err(error) => {
                    state_for_task
                        .metrics
                        .errors_total
                        .fetch_add(1, Ordering::Relaxed);
                    eprintln!("mip solver result serialization failed: {error}");
                    let _ = message
                        .ack_with(async_nats::jetstream::AckKind::Nak(Some(
                            Duration::from_secs(5),
                        )))
                        .await;
                    track_runtime_task_finished(&state_for_task, &task_id_for_task, "error");
                    return;
                }
            };
            if let Err(error) =
                jetstream_publish_ack(&nats_for_task, &state_for_task.results_subject, payload)
                    .await
            {
                state_for_task
                    .metrics
                    .errors_total
                    .fetch_add(1, Ordering::Relaxed);
                eprintln!("publish subproblem result failed: {error}");
                let _ = message
                    .ack_with(async_nats::jetstream::AckKind::Nak(Some(
                        Duration::from_secs(5),
                    )))
                    .await;
                track_runtime_task_finished(&state_for_task, &task_id_for_task, "error");
                return;
            }
            publish_control(
                &state_for_task,
                "worker-completed",
                json!({
                    "consumer": &consumer_name_for_task,
                    "jobId": &result.job_id,
                    "jobUuid": &result.job_uuid,
                    "solveId": &result.solve_id,
                    "problemId": &result.problem_id,
                    "status": &result.status,
                    "resultsSubject": &state_for_task.results_subject,
                }),
            )
            .await;
            state_for_task
                .metrics
                .slave_jobs_processed_total
                .fetch_add(1, Ordering::Relaxed);
            track_runtime_task_finished(&state_for_task, &task_id_for_task, &result.status);
            if let Err(error) = message.ack().await {
                eprintln!("mip solver job ack failed: {error}");
            }
        });
        track_runtime_task_abort_handle(&state, &task_id, worker_task.abort_handle());
    }
    Ok(())
}

async fn run_master_control_listener(state: AppState) -> Result<(), Box<dyn Error + Send + Sync>> {
    let Some(nats) = state.nats.clone() else {
        return Ok(());
    };
    let mut messages = nats.subscribe(state.control_subject.clone()).await?;
    publish_event(
        &state,
        "master-control-listener-started",
        json!({"controlSubject": &state.control_subject}),
    )
    .await;
    while let Some(message) = messages.next().await {
        match serde_json::from_slice::<Value>(&message.payload) {
            Ok(frame) => {
                match record_cancel_control_frame(&state, &frame) {
                    Ok(true) => {
                        state
                            .metrics
                            .worker_control_messages_total
                            .fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                        eprintln!("mip solver cancel control frame ignored: {error}");
                        continue;
                    }
                }
                if let Err(error) = record_worker_control_frame(&state, &frame) {
                    state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                    eprintln!("mip solver control frame ignored: {error}");
                } else {
                    state
                        .metrics
                        .worker_control_messages_total
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(error) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                eprintln!("invalid mip solver control payload: {error}");
            }
        }
    }
    Ok(())
}

async fn connect_nats() -> Option<async_nats::Client> {
    let url = optional_env_value("NATS_URL")?;
    let attempts = env_u64("MIP_SOLVER_NATS_CONNECT_ATTEMPTS", 60).clamp(1, 360);
    let retry_delay =
        Duration::from_secs(env_u64("MIP_SOLVER_NATS_CONNECT_RETRY_SECONDS", 2).clamp(1, 60));

    for attempt in 1..=attempts {
        match async_nats::connect(url.clone()).await {
            Ok(client) => {
                if attempt > 1 {
                    eprintln!("connected to NATS at {url} after {attempt} attempts");
                }
                return Some(client);
            }
            Err(error) => {
                eprintln!(
                    "failed to connect to NATS at {url} on attempt {attempt}/{attempts}: {error}"
                );
                if attempt < attempts {
                    tokio::time::sleep(retry_delay).await;
                }
            }
        }
    }

    None
}

fn connect_redis() -> Option<redis::Client> {
    let env_key = first_configured_env(&["MIP_SOLVER_REDIS_URL", "REDIS_URL"])?;
    let url = optional_env_value(&env_key)?;
    match redis::Client::open(url.clone()) {
        Ok(client) => Some(client),
        Err(error) => {
            eprintln!("failed to configure Redis client from {env_key}: {error}");
            None
        }
    }
}

async fn connect_postgres() -> Option<PgPool> {
    let env_key = first_configured_env(&[
        "MIP_SOLVER_DATABASE_URL",
        "AGENT_TASKS_RDS_DATABASE_URL",
        "RDS_DATABASE_URL",
        "DATABASE_URL",
        "PG_DATABASE_URL",
    ])?;
    let url = optional_env_value(&env_key)?;
    let max_connections = env_usize("MIP_SOLVER_PG_POOL_SIZE", 4).clamp(1, 32) as u32;
    match PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(&url)
        .await
    {
        Ok(pool) => Some(pool),
        Err(error) => {
            eprintln!("failed to connect to Postgres from {env_key}: {error}");
            None
        }
    }
}

fn app_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(root))
        .route("/home", get(home))
        .route("/home/", get(home))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/version", get(version_page))
        .route("/version.json", get(version_json))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/api-docs", get(api_docs_html))
        .route("/api-docs/", get(api_docs_html))
        .route("/api-docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/mip-solver-cluster/nats", get(nats_status))
        .route("/nats", get(nats_status))
        .route("/workers", get(workers))
        .route("/tasks", get(runtime_tasks))
        .route("/tasks/:task_id", get(runtime_task))
        .route("/solves", get(cluster_solves))
        .route("/mip-solver-cluster/workers", get(workers))
        .route("/mip-solver-cluster/tasks", get(runtime_tasks))
        .route("/mip-solver-cluster/tasks/:task_id", get(runtime_task))
        .route("/mip-solver-cluster/solves", get(cluster_solves))
        .route(
            "/mip-solver-cluster/solves/:solve_id",
            delete(cancel_solve_default),
        )
        .route(
            "/mip-solver-cluster/solves/:solve_id/cancel",
            post(cancel_solve),
        )
        .route(
            "/mip-solver-cluster/requests/:request_id/cancel",
            post(cancel_request),
        )
        .route("/problems/:problem_id/cancel", post(cancel_problem))
        .route("/problems/:problem_id", post(upload_problem))
        .route("/problems/:problem_id/solve", post(solve_stored_problem))
        .route("/model/example", get(example))
        .route("/solve", post(solve_http))
        .route("/sessions/:session_id", get(get_session))
        .route("/sessions/:session_id/events", post(stream_session))
        .route("/sessions/:session_id/solve", post(solve_session))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("failed to install Ctrl-C signal handler: {error}");
        }
    };

    #[cfg(unix)]
    {
        let terminate = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut signal) => {
                    signal.recv().await;
                }
                Err(error) => {
                    eprintln!("failed to install SIGTERM signal handler: {error}");
                    std::future::pending::<()>().await;
                }
            }
        };
        tokio::select! {
            _ = ctrl_c => {},
            _ = terminate => {},
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let runtime_config = runtime_config::initialize();
    for warning in runtime_config.warnings() {
        eprintln!("runtime config warning: {warning}");
    }
    if !runtime_config.cli_values().is_empty() {
        let config_path = runtime_config
            .config_path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| ".cli-flags.toml".to_string());
        eprintln!(
            "runtime config applied {} CLI override(s) via {:?} from {config_path}",
            runtime_config.cli_values().len(),
            runtime_config.cli_flag_source()
        );
    }

    let role = NodeRole::from_env();
    let node_id = optional_env_value("MIP_SOLVER_NODE_ID")
        .or_else(|| optional_env_value("POD_NAME"))
        .or_else(|| optional_env_value("HOSTNAME"))
        .unwrap_or_else(|| format!("{}-{}", SERVICE_NAME, Uuid::new_v4()));
    let nats = connect_nats().await;
    let redis = connect_redis();
    let pg = connect_postgres().await;
    let coordination = CoordinationConfig::from_env(redis.is_some());
    let state = AppState {
        role,
        node_id,
        nats,
        redis,
        pg,
        coordination,
        jobs_subject: env_value("MIP_SOLVER_JOBS_SUBJECT", MIP_SOLVER_JOBS_SUBJECT),
        results_subject: env_value("MIP_SOLVER_RESULTS_SUBJECT", MIP_SOLVER_RESULTS_SUBJECT),
        control_subject: env_value("MIP_SOLVER_CONTROL_SUBJECT", MIP_SOLVER_CONTROL_SUBJECT),
        events_subject: env_value("MIP_SOLVER_EVENTS_SUBJECT", MIP_SOLVER_EVENTS_SUBJECT),
        sessions: Arc::new(Mutex::new(HashMap::new())),
        problems: Arc::new(Mutex::new(HashMap::new())),
        workers: Arc::new(Mutex::new(HashMap::new())),
        solves: Arc::new(Mutex::new(HashMap::new())),
        tasks: Arc::new(Mutex::new(HashMap::new())),
        cancelled_solves: Arc::new(Mutex::new(HashMap::new())),
        metrics: Arc::new(Metrics::default()),
    };

    let control_state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = run_master_control_listener(control_state).await {
            eprintln!("mip solver control listener stopped: {error}");
        }
    });

    if state.role == NodeRole::Slave {
        let worker_state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = run_slave(worker_state).await {
                eprintln!("mip solver slave loop stopped: {error}");
            }
        });
    }

    let app = app_router(state);

    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8097");
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("{SERVICE_NAME} listening on {addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Method, Request},
    };
    use serde_json::json;
    use tower::ServiceExt;

    fn test_state(role: NodeRole) -> AppState {
        AppState {
            role,
            node_id: "test-node".to_string(),
            nats: None,
            redis: None,
            pg: None,
            coordination: CoordinationConfig {
                backends: Vec::new(),
                redis_lock_prefix: redis_key_prefix(),
                ttl_ms: 30_000,
                wait_ms: 0,
                live_mutex: None,
            },
            jobs_subject: MIP_SOLVER_JOBS_SUBJECT.to_string(),
            results_subject: MIP_SOLVER_RESULTS_SUBJECT.to_string(),
            control_subject: MIP_SOLVER_CONTROL_SUBJECT.to_string(),
            events_subject: MIP_SOLVER_EVENTS_SUBJECT.to_string(),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            problems: Arc::new(Mutex::new(HashMap::new())),
            workers: Arc::new(Mutex::new(HashMap::new())),
            solves: Arc::new(Mutex::new(HashMap::new())),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            cancelled_solves: Arc::new(Mutex::new(HashMap::new())),
            metrics: Arc::new(Metrics::default()),
        }
    }

    fn binary_knapsack_problem() -> MipProblemSpec {
        MipProblemSpec {
            sense: "max".to_string(),
            c: vec![10.0, 40.0, 30.0, 50.0],
            a: vec![vec![5.0, 4.0, 6.0, 3.0]],
            b: vec![10.0],
            integer_vars: vec![true, true, true, true],
            ub: Some(vec![1.0, 1.0, 1.0, 1.0]),
            var_names: None,
            con_names: None,
        }
    }

    fn pure_lp_problem() -> MipProblemSpec {
        MipProblemSpec {
            sense: "max".to_string(),
            c: vec![3.0, 2.0],
            a: vec![vec![1.0, 1.0], vec![1.0, 0.0], vec![0.0, 1.0]],
            b: vec![4.0, 2.0, 3.0],
            integer_vars: vec![false, false],
            ub: None,
            var_names: Some(vec!["x0".to_string(), "x1".to_string()]),
            con_names: Some(vec![
                "shared".to_string(),
                "x0_cap".to_string(),
                "x1_cap".to_string(),
            ]),
        }
    }

    fn general_integer_problem() -> MipProblemSpec {
        MipProblemSpec {
            sense: "max".to_string(),
            c: vec![1.0, 1.0],
            a: vec![vec![1.0, 1.0]],
            b: vec![3.5],
            integer_vars: vec![true, true],
            ub: Some(vec![10.0, 10.0]),
            var_names: None,
            con_names: None,
        }
    }

    fn hundred_variable_two_hundred_constraint_mip() -> MipProblemSpec {
        let mut c = vec![0.0; 100];
        c[0] = 1.0;
        c[1] = 1.0;
        c[2] = 1.0;

        let mut a = Vec::with_capacity(200);
        let mut b = Vec::with_capacity(200);
        let mut con_names = Vec::with_capacity(200);

        let mut knapsack = vec![0.0; 100];
        knapsack[0] = 2.0;
        knapsack[1] = 2.0;
        knapsack[2] = 2.0;
        a.push(knapsack);
        b.push(5.0);
        con_names.push("three_item_capacity".to_string());

        for var in 0..99 {
            let mut row = vec![0.0; 100];
            row[var] = 1.0;
            a.push(row);
            b.push(1.0);
            con_names.push(format!("x{var}_upper"));
        }

        for var in 0..100 {
            let mut row = vec![0.0; 100];
            row[var] = -1.0;
            a.push(row);
            b.push(0.0);
            con_names.push(format!("x{var}_lower"));
        }

        assert_eq!(a.len(), 200);
        assert_eq!(b.len(), 200);
        assert_eq!(con_names.len(), 200);

        MipProblemSpec {
            sense: "max".to_string(),
            c,
            a,
            b,
            integer_vars: vec![true; 100],
            ub: Some(vec![1.0; 100]),
            var_names: Some((0..100).map(|index| format!("x{index}")).collect()),
            con_names: Some(con_names),
        }
    }

    fn hundred_variable_one_hundred_fifty_constraint_dispatch_mip() -> MipProblemSpec {
        let n = 100;
        let mut c = vec![0.0; n];
        for pair in 0..50 {
            c[pair * 2] = 1000.0 - pair as f64;
            c[pair * 2 + 1] = 25.0 + pair as f64;
        }

        let mut a = Vec::with_capacity(150);
        let mut b = Vec::with_capacity(150);
        let mut con_names = Vec::with_capacity(150);

        let fleet_budget = vec![2.0; n];
        a.push(fleet_budget);
        b.push(99.0);
        con_names.push("fleet_budget_allows_49_full_dispatches".to_string());

        for pair in 0..50 {
            let mut row = vec![0.0; n];
            row[pair * 2] = 1.0;
            row[pair * 2 + 1] = 1.0;
            a.push(row);
            b.push(1.0);
            con_names.push(format!("route_pair_{pair}_choose_at_most_one"));
        }

        for var in 0..99 {
            let mut row = vec![0.0; n];
            row[var] = 1.0;
            a.push(row);
            b.push(1.0);
            con_names.push(format!("dispatch_{var}_capacity"));
        }

        assert_eq!(a.len(), 150);
        assert_eq!(b.len(), 150);
        assert_eq!(con_names.len(), 150);

        MipProblemSpec {
            sense: "max".to_string(),
            c,
            a,
            b,
            integer_vars: vec![true; n],
            ub: Some(vec![1.0; n]),
            var_names: Some((0..n).map(|index| format!("dispatch_{index}")).collect()),
            con_names: Some(con_names),
        }
    }

    fn test_job(problem: MipProblemSpec) -> SubproblemJob {
        SubproblemJob {
            solve_id: "solve-test".to_string(),
            request_id: "request-test".to_string(),
            job_id: "job-test".to_string(),
            job_uuid: new_uuid_string(),
            problem_id: Some("33333333-3333-4333-8333-333333333333".to_string()),
            problem_stored: false,
            revision: 0,
            depth: 0,
            master_node: "master-test".to_string(),
            problem: Some(problem),
            extra_constraints: Vec::new(),
            avoid_worker_nodes: Vec::new(),
            options: SolveOptions {
                split_depth: Some(0),
                ..SolveOptions::default()
            },
            submitted_at_ms: now_ms(),
        }
    }

    async fn post_json(app: Router, path: &str, payload: Value) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), MAX_HTTP_BODY_BYTES)
            .await
            .unwrap();
        let value = serde_json::from_slice(&body).unwrap();
        (status, value)
    }

    async fn delete_json(app: Router, path: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(Method::DELETE)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), MAX_HTTP_BODY_BYTES)
            .await
            .unwrap();
        let value = serde_json::from_slice(&body).unwrap();
        (status, value)
    }

    async fn get_text(app: Router, path: &str) -> (StatusCode, String) {
        let request = Request::builder()
            .method(Method::GET)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), MAX_HTTP_BODY_BYTES)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        (status, text)
    }

    async fn get_json(app: Router, path: &str) -> (StatusCode, Value) {
        let (status, text) = get_text(app, path).await;
        let value = serde_json::from_str(&text).unwrap();
        (status, value)
    }

    #[tokio::test]
    async fn metadata_pages_and_api_docs_are_served() {
        let app = app_router(test_state(NodeRole::Master));

        let (status, home) = get_text(app.clone(), "/home").await;
        assert_eq!(status, StatusCode::OK);
        assert!(home.contains(SERVICE_NAME));
        assert!(home.contains(DD_REMOTE_MIP_SOLVER_STREAM_NAME));

        let (status, version_page) = get_text(app.clone(), "/version").await;
        assert_eq!(status, StatusCode::OK);
        assert!(version_page.contains("gitCommit"));
        assert!(version_page.contains(env!("CARGO_PKG_VERSION")));

        let (status, version_json) = get_json(app.clone(), "/version.json").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(version_json.get("service"), Some(&json!(SERVICE_NAME)));
        assert_eq!(
            version_json.pointer("/build/packageVersion"),
            Some(&json!(env!("CARGO_PKG_VERSION")))
        );

        let (status, docs_html) = get_text(app.clone(), "/docs/api").await;
        assert_eq!(status, StatusCode::OK);
        assert!(docs_html.contains("/mip-solver-cluster/nats"));
        assert!(docs_html.contains("/api/docs.json"));

        let (status, docs_json) = get_json(app, "/api/docs.json").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(docs_json.get("schema"), Some(&json!(API_DOCS_SCHEMA)));
        assert!(docs_json
            .get("endpoints")
            .and_then(Value::as_array)
            .is_some_and(|endpoints| endpoints.iter().any(|endpoint| {
                endpoint.get("path").and_then(Value::as_str) == Some("/mip-solver-cluster/nats")
            })));
    }

    #[test]
    fn streaming_edits_update_live_problem_revision() {
        let commands = vec![
            json!({"op":"init","sense":"max","c":[3.0,2.0],"a":[[1.0,1.0]],"b":[4.0],"integerVars":[true,false]}),
            json!({"op":"set_rhs","index":0,"rhs":5.0}),
            json!({"op":"add_constraint","coefs":[2.0,1.0],"rhs":8.0}),
            json!({"op":"add_variable","c":4.0,"column":[0.0,1.0],"integer":true,"ub":3.0}),
            json!({"op":"set_variable","index":2,"c":5.0,"integer":true}),
            json!({"op":"remove_constraint","index":0}),
            json!({"op":"snapshot"}),
        ];
        let (problem, revision, frames) = parse_problem_from_commands(&commands).unwrap();
        assert_eq!(revision, 6);
        assert_eq!(problem.c, vec![3.0, 2.0, 5.0]);
        assert_eq!(problem.a, vec![vec![2.0, 1.0, 1.0]]);
        assert_eq!(problem.b, vec![8.0]);
        assert_eq!(problem.integer_vars, vec![true, false, true]);
        assert_eq!(problem.ub.as_ref().unwrap()[2], 3.0);
        assert!(frames
            .iter()
            .any(|frame| frame.get("event") == Some(&json!("model"))));
    }

    #[test]
    fn streaming_edits_reject_non_numeric_scalars_instead_of_defaulting() {
        let commands = vec![
            json!({"op":"init","sense":"max","c":[1.0],"a":[[1.0]],"b":[1.0],"integerVars":[true]}),
            json!({"op":"set_rhs","index":0,"rhs":"not-a-number"}),
        ];

        let error = parse_problem_from_commands(&commands).unwrap_err();

        assert!(error.contains("rhs must be a finite number"));
    }

    #[test]
    fn streaming_init_rejects_non_numeric_matrix_cells() {
        let commands = vec![json!({
            "op": "init",
            "sense": "max",
            "c": [1.0],
            "a": [["bad-cell"]],
            "b": [1.0],
            "integerVars": [true]
        })];

        let error = parse_problem_from_commands(&commands).unwrap_err();

        assert!(error.contains("a[0][0] must be a finite number"));
    }

    #[test]
    fn frontier_builder_splits_fractional_lp_relaxation() {
        let problem = normalized_problem(MipProblemSpec {
            sense: "max".to_string(),
            c: vec![1.0],
            a: vec![vec![1.0]],
            b: vec![1.5],
            integer_vars: vec![true],
            ub: None,
            var_names: None,
            con_names: None,
        })
        .unwrap();
        let options = SolveOptions {
            split_depth: Some(1),
            ..SolveOptions::default()
        };
        let (jobs, warnings) = build_frontier_jobs(
            &problem,
            "solve-test",
            "request-test",
            "33333333-3333-4333-8333-333333333333",
            7,
            "master-a",
            &options,
            false,
        )
        .unwrap();
        assert!(warnings.is_empty());
        assert_eq!(jobs.len(), 1);
        assert!(jobs.iter().all(|job| job.revision == 7));
        assert!(jobs
            .iter()
            .all(|job| job.problem_id.as_deref() == Some("33333333-3333-4333-8333-333333333333")));
        assert!(jobs
            .iter()
            .all(|job| Uuid::parse_str(&job.job_uuid).is_ok()));
        assert!(jobs.iter().all(|job| job.extra_constraints.len() == 1));
        assert_eq!(jobs[0].extra_constraints[0].coefs, vec![1.0]);
        assert_eq!(jobs[0].extra_constraints[0].rhs, 1.0);
        assert_eq!(jobs[0].depth, 1);
    }

    #[test]
    fn frontier_builder_can_emit_ref_only_jobs() {
        let problem = normalized_problem(binary_knapsack_problem()).unwrap();
        let options = SolveOptions {
            split_depth: Some(1),
            ..SolveOptions::default()
        };

        let (jobs, warnings) = build_frontier_jobs(
            &problem,
            "solve-ref-test",
            "request-ref-test",
            "77777777-7777-4777-8777-777777777777",
            3,
            "master-a",
            &options,
            true,
        )
        .unwrap();

        assert!(warnings.is_empty());
        assert!(!jobs.is_empty());
        assert!(jobs.iter().all(|job| job.problem_stored));
        assert!(jobs.iter().all(|job| job.problem.is_none()));
        let payload = json_value(&jobs[0]);
        assert!(payload.get("problem").is_none());
        assert_eq!(payload.get("problemStored"), Some(&json!(true)));
    }

    #[tokio::test]
    async fn hydrate_subproblem_job_loads_problem_from_cache() {
        let state = test_state(NodeRole::Slave);
        let problem = normalized_problem(binary_knapsack_problem()).unwrap();
        let problem_id = "88888888-8888-4888-8888-888888888888";
        remember_problem_model(&state, problem_id, 4, problem.clone());
        let mut job = test_job(problem.clone()).without_problem_payload();
        job.problem_id = Some(problem_id.to_string());
        job.revision = 4;

        hydrate_subproblem_job(&state, &mut job).await.unwrap();

        assert_eq!(
            job.problem.as_ref().map(|problem| &problem.c),
            Some(&problem.c)
        );
    }

    #[tokio::test]
    async fn hydrate_subproblem_job_uses_stored_model_over_embedded_payload() {
        let state = test_state(NodeRole::Slave);
        let stored = normalized_problem(binary_knapsack_problem()).unwrap();
        let problem_id = "8aaaaaaa-8888-4888-8888-888888888888";
        remember_problem_model(&state, problem_id, 0, stored.clone());
        let mut embedded = stored.clone();
        embedded.c[0] = 999.0;
        let mut job = test_job(embedded);
        job.problem_id = Some(problem_id.to_string());
        job.problem_stored = true;

        hydrate_subproblem_job(&state, &mut job).await.unwrap();

        assert_eq!(
            job.problem.as_ref().map(|problem| &problem.c),
            Some(&stored.c)
        );
    }

    #[tokio::test]
    async fn store_problem_model_rejects_same_id_with_different_model() {
        let state = test_state(NodeRole::Master);
        let problem_id = "8bbbbbbb-8888-4888-8888-888888888888";
        let first = normalized_problem(binary_knapsack_problem()).unwrap();
        let mut second = first.clone();
        second.c[0] = 11.0;

        let created = store_problem_model(&state, problem_id, 0, &first)
            .await
            .unwrap();
        let existing = store_problem_model(&state, problem_id, 0, &first)
            .await
            .unwrap();
        let conflict = store_problem_model(&state, problem_id, 0, &second)
            .await
            .unwrap_err();

        assert_eq!(created, ProblemStoreStatus::Created);
        assert_eq!(existing, ProblemStoreStatus::Existing);
        assert!(is_problem_model_conflict(&conflict));
    }

    #[test]
    fn validate_subproblem_job_payload_enforces_reference_contract() {
        let problem = normalized_problem(binary_knapsack_problem()).unwrap();
        let mut ref_job = test_job(problem.clone()).without_problem_payload();
        ref_job.problem_id = Some("8ccccccc-8888-4888-8888-888888888888".to_string());
        assert!(validate_subproblem_job_payload(&ref_job).is_ok());

        let mut leaked_ref_job = ref_job.clone();
        leaked_ref_job.problem = Some(problem);
        assert!(validate_subproblem_job_payload(&leaked_ref_job)
            .unwrap_err()
            .contains("embedded problem"));

        let mut missing_embedded = test_job(binary_knapsack_problem());
        missing_embedded.problem = None;
        assert!(validate_subproblem_job_payload(&missing_embedded)
            .unwrap_err()
            .contains("no stored problem reference"));
    }

    #[test]
    fn solve_options_merge_request_values_over_runtime_defaults() {
        let defaults = SolveOptions {
            max_nodes: Some(111),
            max_ticks: Some(222),
            lp_max_iters: Some(333),
            int_tol: Some(1e-4),
            split_depth: Some(2),
            max_subproblems: Some(12),
            max_job_retries: Some(4),
            timeout_ms: Some(444),
            emit_trace: Some(false),
            verify_external: Some(false),
            external_verification_method: Some("highs".to_string()),
            external_verification_tolerance: Some(1e-5),
        };
        let input = SolveOptions {
            max_nodes: Some(999),
            max_ticks: None,
            lp_max_iters: Some(777),
            int_tol: None,
            split_depth: Some(5),
            max_subproblems: Some(3),
            max_job_retries: Some(9),
            timeout_ms: None,
            emit_trace: Some(true),
            verify_external: Some(true),
            external_verification_method: Some("highs-ds".to_string()),
            external_verification_tolerance: None,
        };

        let merged = SolveOptions::merged_with_defaults(Some(input), defaults);

        assert_eq!(merged.max_nodes, Some(999));
        assert_eq!(merged.max_ticks, Some(222));
        assert_eq!(merged.lp_max_iters, Some(777));
        assert_eq!(merged.int_tol, Some(1e-4));
        assert_eq!(merged.split_depth, Some(5));
        assert_eq!(merged.max_subproblems, Some(3));
        assert_eq!(merged.max_job_retries, Some(9));
        assert_eq!(merged.timeout_ms, Some(444));
        assert_eq!(merged.emit_trace, Some(true));
        assert_eq!(merged.verify_external, Some(true));
        assert_eq!(
            merged.external_verification_method.as_deref(),
            Some("highs-ds")
        );
        assert_eq!(merged.external_verification_tolerance, Some(1e-5));
    }

    #[cfg(not(feature = "external-solver-verification"))]
    #[test]
    fn external_verification_request_reports_unavailable_without_feature() {
        let problem = binary_knapsack_problem();
        let state = test_state(NodeRole::Master);
        let optimal = SubproblemResult {
            solve_id: "solve-verify-test".to_string(),
            request_id: "request-verify-test".to_string(),
            job_id: "job-verify".to_string(),
            job_uuid: new_uuid_string(),
            problem_id: Some("77777777-7777-4777-8777-777777777777".to_string()),
            revision: 0,
            worker_node: "worker-verify".to_string(),
            ok: true,
            status: "optimal".to_string(),
            z: Some(90.0),
            x: vec![0.0, 1.0, 0.0, 1.0],
            best_bound: Some(90.0),
            gap: Some(0.0),
            lp: None,
            child_jobs: Vec::new(),
            nodes_explored: 1,
            lp_solves: 1,
            elapsed_ms: 1.0,
            accelerator: AcceleratorReport::default(),
            error: None,
            finished_at_ms: now_ms(),
        };
        let options = SolveOptions {
            verify_external: Some(true),
            external_verification_method: Some("highs".to_string()),
            ..SolveOptions::default()
        };

        let response = aggregate_results(
            "solve-verify-test".to_string(),
            "request-verify-test".to_string(),
            None,
            0,
            &problem,
            &options,
            1,
            1,
            0,
            0,
            vec![optimal],
            false,
            true,
            &state,
            Vec::new(),
        );

        let verification = response.external_verification.expect("verification report");
        assert_eq!(verification.status, "unavailable");
        assert!(!verification.enabled);
        assert!(response
            .warnings
            .iter()
            .any(|warning| warning.contains("feature is not enabled")));
    }

    #[test]
    fn frontier_builder_caps_presplit_subproblem_count() {
        let problem = normalized_problem(MipProblemSpec {
            sense: "max".to_string(),
            c: vec![1.0],
            a: vec![vec![1.0]],
            b: vec![1.5],
            integer_vars: vec![true],
            ub: None,
            var_names: None,
            con_names: None,
        })
        .unwrap();
        let options = SolveOptions {
            split_depth: Some(4),
            max_subproblems: Some(1),
            ..SolveOptions::default()
        };

        let (jobs, warnings) = build_frontier_jobs(
            &problem,
            "solve-test",
            "request-test",
            "44444444-4444-4444-8444-444444444444",
            7,
            "master-a",
            &options,
            false,
        )
        .unwrap();

        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].depth, 0);
        assert!(jobs[0].extra_constraints.is_empty());
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("frontier split capped at 1 subproblems")));
    }

    #[test]
    fn branch_constraints_extend_named_constraint_metadata() {
        let problem = normalized_problem(MipProblemSpec {
            sense: "max".to_string(),
            c: vec![1.0],
            a: vec![vec![1.0]],
            b: vec![1.5],
            integer_vars: vec![true],
            ub: Some(vec![3.0]),
            var_names: Some(vec!["x0".to_string()]),
            con_names: Some(vec!["capacity".to_string()]),
        })
        .unwrap();
        let extra = vec![BranchConstraint {
            coefs: vec![1.0],
            rhs: 1.0,
            name: "branch_d0_x0_le_1".to_string(),
        }];

        let ipmip = to_ipmip_problem(&problem, &extra).unwrap();
        let lp = to_lp_problem(&problem, &extra).unwrap();

        assert_eq!(
            ipmip.con_names,
            Some(vec![
                "capacity".to_string(),
                "branch_d0_x0_le_1".to_string()
            ])
        );
        assert_eq!(
            lp.con_names,
            Some(vec![
                "capacity".to_string(),
                "branch_d0_x0_le_1".to_string()
            ])
        );
    }

    #[test]
    fn branch_constraints_are_validated_before_lp_and_mip_conversion() {
        let lp_problem = normalized_problem(pure_lp_problem()).unwrap();
        let bad_width = BranchConstraint {
            coefs: vec![1.0],
            rhs: 1.0,
            name: "bad_width".to_string(),
        };
        let err = to_lp_problem(&lp_problem, &[bad_width]).unwrap_err();
        assert!(err.contains("bad_width has length 1, expected 2"));

        let mip_problem = normalized_problem(binary_knapsack_problem()).unwrap();
        let bad_coef = BranchConstraint {
            coefs: vec![1.0, f64::NAN, 0.0, 0.0],
            rhs: 1.0,
            name: "bad_coef".to_string(),
        };
        let err = to_ipmip_problem(&mip_problem, &[bad_coef]).unwrap_err();
        assert!(err.contains("bad_coef contains a non-finite coefficient"));

        let bad_rhs = BranchConstraint {
            coefs: vec![1.0, 0.0, 0.0, 0.0],
            rhs: f64::INFINITY,
            name: "bad_rhs".to_string(),
        };
        let err = preprocess_bounds_with_mode(&mip_problem, &[bad_rhs], "off").unwrap_err();
        assert!(err.contains("bad_rhs rhs must be finite"));
    }

    #[test]
    fn bound_preprocess_prunes_rows_impossible_under_lower_bounds() {
        let problem = MipProblemSpec {
            sense: "max".to_string(),
            c: vec![1.0],
            a: vec![vec![1.0]],
            b: vec![-1.0],
            integer_vars: vec![false],
            ub: Some(vec![10.0]),
            var_names: None,
            con_names: None,
        };

        let report = preprocess_bounds_with_mode(&problem, &[], "off").unwrap();

        assert!(report
            .infeasible_reason
            .as_deref()
            .unwrap_or_default()
            .contains("bound preprocessing proved row 0 infeasible"));
        assert_eq!(report.accelerator.backend, "in-house-cpu");
        assert!(!report.accelerator.used_gpu);
    }

    #[test]
    fn bound_preprocess_reports_rows_always_satisfied_by_bounds() {
        let problem = MipProblemSpec {
            sense: "max".to_string(),
            c: vec![1.0],
            a: vec![vec![1.0]],
            b: vec![5.0],
            integer_vars: vec![false],
            ub: Some(vec![3.0]),
            var_names: None,
            con_names: None,
        };

        let report = preprocess_bounds_with_mode(&problem, &[], "off").unwrap();

        assert!(report.infeasible_reason.is_none());
        assert!(report
            .accelerator
            .notes
            .iter()
            .any(|note| note.contains("rows always satisfied by variable bounds")));
    }

    #[test]
    fn solve_options_force_in_house_lp_and_mip_engines() {
        let options = SolveOptions::default().to_ipmip_options();

        assert_eq!(options.allow_external_solvers, Some(false));
        assert!(matches!(
            options.lp_algorithm,
            Some(LpRelaxationAlgorithm::Concrete(
                ConcreteLpRelaxationAlgorithm::InternalSimplex
            ))
        ));
        assert!(matches!(
            options.branch_rule,
            Some(BranchRule::MostFractional)
        ));
    }

    #[test]
    fn nats_subjects_are_generated_mip_solver_namespace() {
        let subjects = [
            MIP_SOLVER_JOBS_SUBJECT,
            MIP_SOLVER_RESULTS_SUBJECT,
            MIP_SOLVER_CONTROL_SUBJECT,
            MIP_SOLVER_EVENTS_SUBJECT,
        ];
        for subject in subjects {
            assert!(subject.starts_with("dd.remote.mip_solver."));
            assert!(DD_REMOTE_MIP_SOLVER_STREAM_SUBJECTS.contains(&subject));
        }

        let mut unique = subjects.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), subjects.len());
        assert_eq!(DD_REMOTE_MIP_SOLVER_STREAM_NAME, "DD_REMOTE_MIP_SOLVER");
        assert_eq!(
            MIP_SOLVER_WORKERS_QUEUE_GROUP,
            "dd-in-house-mip-solver-node-workers"
        );
    }

    #[test]
    fn persistence_contract_uses_generated_mip_pg_defs_and_redis_namespace() {
        let contract = persistence_contract();

        assert_eq!(contract.postgres.session_table, MIP_SOLVER_SESSIONS_TABLE);
        assert_eq!(contract.postgres.solve_table, MIP_SOLVER_SOLVES_TABLE);
        assert_eq!(contract.postgres.job_table, MIP_SOLVER_JOBS_TABLE);
        assert_eq!(contract.postgres.event_table, MIP_SOLVER_EVENTS_TABLE);
        assert!(contract
            .postgres
            .journal_kinds
            .contains(&"mip-solver.subproblem-split"));
        assert_eq!(
            mip_solver_solve_snapshot_key("dd:mip-solver", "solve-a"),
            "dd:mip-solver:solve:solve-a:snapshot"
        );
        assert_eq!(
            mip_solver_session_model_key("dd:mip-solver", "session-a"),
            "dd:mip-solver:session:session-a:model"
        );
        assert!(contract
            .redis
            .generated_mutex_key
            .contains("dd:container-pool:affinity:mip-solver"));
    }

    #[test]
    fn coordination_backend_parser_supports_auto_and_explicit_both() {
        assert_eq!(
            parse_coordination_backends("auto", true, true),
            vec![CoordinationBackend::Redis, CoordinationBackend::LiveMutex]
        );
        assert_eq!(
            parse_coordination_backends("redis,live-mutex", true, true),
            vec![CoordinationBackend::Redis, CoordinationBackend::LiveMutex]
        );
        assert_eq!(
            parse_coordination_backends("both", true, false),
            vec![CoordinationBackend::Redis]
        );
        assert!(parse_coordination_backends("none", true, true).is_empty());
    }

    #[test]
    fn live_mutex_http_endpoint_parser_preserves_base_path() {
        let endpoint =
            parse_http_endpoint("http://dd-rust-network-mutex.default.svc.cluster.local:6971/api")
                .unwrap();

        assert_eq!(
            endpoint.addr,
            "dd-rust-network-mutex.default.svc.cluster.local:6971"
        );
        assert_eq!(
            endpoint.host_header,
            "dd-rust-network-mutex.default.svc.cluster.local:6971"
        );
        assert_eq!(http_path(&endpoint.path_prefix, "/v1/lock"), "/api/v1/lock");
        assert!(parse_http_endpoint("https://example.com").is_err());
        assert!(parse_http_endpoint("http://example.com\r\nX-Bad: yes").is_err());
    }

    #[test]
    fn chunked_decoder_accepts_chunk_extensions() {
        let decoded = decode_chunked_body(b"4;foo=bar\r\nrust\r\n0\r\n\r\n").unwrap();

        assert_eq!(decoded, b"rust");
    }

    #[tokio::test]
    async fn live_mutex_rejects_crlf_auth_token_before_network() {
        let config = LiveMutexConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            auth_token: Some("bad\r\nAuthorization: nope".to_string()),
            request_timeout_ms: 1,
            max_response_bytes: 1024,
        };

        let error = live_mutex_post_json(&config, "/v1/lock", json!({}))
            .await
            .unwrap_err();

        assert!(error.contains("CRLF"));
    }

    #[tokio::test]
    async fn live_mutex_http_client_stops_after_content_length_without_eof() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0u8; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            let body = br#"{"acquired":true,"lockUuid":"lock-a"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.write_all(body).await.unwrap();
            tokio::time::sleep(Duration::from_millis(750)).await;
        });
        let config = LiveMutexConfig {
            base_url: format!("http://{addr}"),
            auth_token: None,
            request_timeout_ms: 1_000,
            max_response_bytes: 1024,
        };

        let started = Instant::now();
        let value = live_mutex_post_json(&config, "/v1/lock", json!({"key":"content-length"}))
            .await
            .unwrap();

        assert_eq!(value["lockUuid"], "lock-a");
        assert!(started.elapsed() < Duration::from_millis(500));
        server.await.unwrap();
    }

    #[test]
    fn jetstream_stream_config_contains_generated_subjects() {
        let config = mip_stream_config();

        assert_eq!(config.name, DD_REMOTE_MIP_SOLVER_STREAM_NAME);
        for subject in DD_REMOTE_MIP_SOLVER_STREAM_SUBJECTS {
            assert!(config
                .subjects
                .iter()
                .any(|configured| configured == subject));
        }
        assert_eq!(
            config.subjects.len(),
            DD_REMOTE_MIP_SOLVER_STREAM_SUBJECTS.len()
        );
    }

    #[test]
    fn result_consumer_config_reads_persisted_results_from_job_sequence() {
        let name = result_consumer_name("solve-test");
        let config = result_consumer_config(&name, MIP_SOLVER_RESULTS_SUBJECT, 42);

        assert_eq!(config.name.as_deref(), Some(name.as_str()));
        assert_eq!(config.filter_subject, MIP_SOLVER_RESULTS_SUBJECT);
        assert_eq!(config.durable_name, None);
        assert_eq!(config.max_deliver, 1);
        assert_eq!(config.inactive_threshold, Duration::from_secs(120));
        assert!(matches!(
            config.deliver_policy,
            async_nats::jetstream::consumer::DeliverPolicy::ByStartSequence { start_sequence: 42 }
        ));
    }

    #[test]
    fn worker_consumer_config_uses_runtime_jobs_subject_and_delivery_limits() {
        let config = worker_consumer_config(
            MIP_SOLVER_WORKERS_QUEUE_GROUP,
            "dd.remote.mip_solver.jobs.custom",
            Duration::from_secs(900),
            64,
            7,
        );

        assert_eq!(
            config.durable_name.as_deref(),
            Some(MIP_SOLVER_WORKERS_QUEUE_GROUP)
        );
        assert_eq!(config.filter_subject, "dd.remote.mip_solver.jobs.custom");
        assert_eq!(config.ack_wait, Duration::from_secs(900));
        assert_eq!(config.max_ack_pending, 64);
        assert_eq!(config.max_deliver, 7);
    }

    #[tokio::test]
    async fn nats_status_route_reports_master_slave_wiring() {
        let state = test_state(NodeRole::Master);
        record_worker_control_frame(
            &state,
            &json!({
                "schema":"dd.mip-solver.control.v1",
                "service": SERVICE_NAME,
                "nodeId":"worker-route-test",
                "role":"slave",
                "commandName":"worker-ready",
                "payload":{
                    "consumer": MIP_SOLVER_WORKERS_QUEUE_GROUP,
                    "jobsSubject": MIP_SOLVER_JOBS_SUBJECT,
                    "resultsSubject": MIP_SOLVER_RESULTS_SUBJECT
                },
                "timeMs": 1000
            }),
        )
        .unwrap();
        let app = app_router(state);

        let (status, body) = get_json(app, "/mip-solver-cluster/nats").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.get("ok"), Some(&json!(false)));
        assert_eq!(body.get("connected"), Some(&json!(false)));
        assert_eq!(
            body.pointer("/stream/name"),
            Some(&json!(DD_REMOTE_MIP_SOLVER_STREAM_NAME))
        );
        assert_eq!(
            body.pointer("/subjects/jobs"),
            Some(&json!(MIP_SOLVER_JOBS_SUBJECT))
        );
        assert_eq!(
            body.get("workerConsumer"),
            Some(&json!(MIP_SOLVER_WORKERS_QUEUE_GROUP))
        );
        assert_eq!(body.get("workersKnown"), Some(&json!(1)));
        assert_eq!(
            body.pointer("/workers/0/nodeId"),
            Some(&json!("worker-route-test"))
        );
    }

    #[test]
    fn worker_control_frames_update_master_registry() {
        let state = test_state(NodeRole::Master);

        record_worker_control_frame(
            &state,
            &json!({
                "schema":"dd.mip-solver.control.v1",
                "service": SERVICE_NAME,
                "nodeId":"worker-a",
                "role":"slave",
                "commandName":"worker-ready",
                "payload":{
                    "consumer":"dd-in-house-mip-solver-node-workers",
                    "jobsSubject": MIP_SOLVER_JOBS_SUBJECT,
                    "resultsSubject": MIP_SOLVER_RESULTS_SUBJECT
                },
                "timeMs": 1000
            }),
        )
        .unwrap();
        record_worker_control_frame(
            &state,
            &json!({
                "schema":"dd.mip-solver.control.v1",
                "service": SERVICE_NAME,
                "nodeId":"worker-a",
                "role":"slave",
                "commandName":"request-work",
                "payload":{"consumer":"dd-in-house-mip-solver-node-workers"},
                "timeMs": 1100
            }),
        )
        .unwrap();
        record_worker_control_frame(
            &state,
            &json!({
                "schema":"dd.mip-solver.control.v1",
                "service": SERVICE_NAME,
                "nodeId":"worker-a",
                "role":"slave",
                "commandName":"worker-completed",
                "payload":{
                    "consumer":"dd-in-house-mip-solver-node-workers",
                    "jobId":"solve-test-0",
                    "solveId":"solve-test",
                    "status":"optimal",
                    "resultsSubject": MIP_SOLVER_RESULTS_SUBJECT
                },
                "timeMs": 1200
            }),
        )
        .unwrap();

        let workers = state.workers.lock().expect("workers mutex poisoned");
        let worker = workers.get("worker-a").expect("worker-a");
        assert_eq!(worker.ready_at_ms, Some(1000));
        assert_eq!(worker.last_seen_ms, 1200);
        assert_eq!(worker.request_count, 1);
        assert_eq!(worker.completed_count, 1);
        assert_eq!(worker.last_command, "worker-completed");
        assert_eq!(worker.last_job_id.as_deref(), Some("solve-test-0"));
        assert_eq!(worker.last_solve_id.as_deref(), Some("solve-test"));
        assert_eq!(worker.last_status.as_deref(), Some("optimal"));
    }

    #[test]
    fn worker_progress_updates_active_job_and_stale_detection() {
        let state = test_state(NodeRole::Master);
        let job = test_job(binary_knapsack_problem());
        let problem_id = job.problem_id.clone().unwrap();
        track_solve_started(
            &state,
            &job.solve_id,
            &job.request_id,
            &problem_id,
            job.revision,
            1,
            true,
        )
        .unwrap();
        track_job_submitted(&state, &job);

        record_worker_control_frame(
            &state,
            &json!({
                "schema":"dd.mip-solver.control.v1",
                "service": SERVICE_NAME,
                "nodeId":"worker-progress-a",
                "role":"slave",
                "commandName":"request-work",
                "payload":{
                    "consumer":"dd-in-house-mip-solver-node-workers",
                    "jobId": &job.job_id,
                    "jobUuid": &job.job_uuid,
                    "solveId": &job.solve_id,
                    "problemId": &problem_id,
                    "startedAtMs": 1000
                },
                "timeMs": 1000
            }),
        )
        .unwrap();
        record_worker_control_frame(
            &state,
            &json!({
                "schema":"dd.mip-solver.control.v1",
                "service": SERVICE_NAME,
                "nodeId":"worker-progress-a",
                "role":"slave",
                "commandName":"worker-progress",
                "payload":{
                    "consumer":"dd-in-house-mip-solver-node-workers",
                    "jobId": &job.job_id,
                    "jobUuid": &job.job_uuid,
                    "solveId": &job.solve_id,
                    "problemId": &problem_id,
                    "startedAtMs": 1000
                },
                "timeMs": 26_000
            }),
        )
        .unwrap();

        let workers = state.workers.lock().expect("workers mutex poisoned");
        let worker = workers.get("worker-progress-a").expect("worker");
        assert_eq!(worker.request_count, 1);
        assert_eq!(worker.last_job_id.as_deref(), Some(job.job_id.as_str()));
        assert_eq!(worker.last_solve_id.as_deref(), Some(job.solve_id.as_str()));
        assert_eq!(worker.last_status.as_deref(), Some("running"));
        assert!(worker.active_jobs.contains_key(&job.job_uuid));
        drop(workers);

        let solves = state.solves.lock().expect("solves mutex poisoned");
        let tracked = solves
            .get(&job.solve_id)
            .and_then(|solve| solve.jobs.get(&job.job_id))
            .expect("tracked job");
        assert_eq!(tracked.status, "running");
        assert_eq!(tracked.worker_node.as_deref(), Some("worker-progress-a"));
        assert_eq!(tracked.last_heartbeat_ms, Some(26_000));
        drop(solves);

        let active = HashSet::from([job.job_id.clone()]);
        let completed = HashSet::new();
        assert!(stale_worker_jobs(
            &state,
            &job.solve_id,
            &active,
            &completed,
            Duration::from_secs(100),
            126_000,
        )
        .is_empty());
        let stale = stale_worker_jobs(
            &state,
            &job.solve_id,
            &active,
            &completed,
            Duration::from_secs(100),
            126_002,
        );
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].job_id, job.job_id);
        assert_eq!(stale[0].job_uuid.as_deref(), Some(job.job_uuid.as_str()));
        assert_eq!(stale[0].worker_node, "worker-progress-a");
    }

    #[test]
    fn solve_subproblem_solves_binary_mip_with_in_house_solver() {
        let result = solve_subproblem(
            test_job(binary_knapsack_problem()),
            "worker-test".to_string(),
        );

        assert!(result.ok, "subproblem error: {:?}", result.error);
        assert_eq!(result.status, "optimal");
        assert_eq!(result.z, Some(90.0));
        assert_eq!(result.x.len(), 4);
        assert!((result.x[1] - 1.0).abs() < 1e-6);
        assert!((result.x[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn solve_subproblem_can_split_fractional_delegated_subtree() {
        let problem = normalized_problem(MipProblemSpec {
            sense: "max".to_string(),
            c: vec![1.0],
            a: vec![vec![1.0]],
            b: vec![1.5],
            integer_vars: vec![true],
            ub: None,
            var_names: None,
            con_names: None,
        })
        .unwrap();
        let mut job = test_job(problem);
        job.options.split_depth = Some(1);

        let result = solve_subproblem(job, "worker-test".to_string());

        assert!(!result.ok);
        assert_eq!(result.status, "split");
        assert_eq!(result.child_jobs.len(), 2);
        assert_eq!(result.lp_solves, 1);
        assert!(result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("split"));
        assert!(result
            .child_jobs
            .iter()
            .all(|child| child.depth == 1 && child.extra_constraints.len() == 1));
    }

    #[test]
    fn solve_subproblem_solves_lp_with_in_house_solver() {
        let result = solve_subproblem(test_job(pure_lp_problem()), "worker-test".to_string());

        assert!(result.ok, "subproblem error: {:?}", result.error);
        assert_eq!(result.status, "optimal");
        assert_eq!(result.x.len(), 2);
        assert!(result.z.is_some_and(|z| (z - 10.0).abs() < 1e-6));
        assert!((result.x[0] - 2.0).abs() < 1e-6);
        assert!((result.x[1] - 2.0).abs() < 1e-6);
        let lp = result.lp.as_ref().expect("LP solve report");
        assert_eq!(
            lp.dual.row_names.as_ref().unwrap(),
            &vec![
                "shared".to_string(),
                "x0_cap".to_string(),
                "x1_cap".to_string()
            ]
        );
        let dual = lp.dual.inequality.as_ref().expect("row duals");
        assert_eq!(dual.len(), 3);
        assert!((dual[0] - 2.0).abs() < 1e-6, "dual = {dual:?}");
        assert!((dual[1] - 1.0).abs() < 1e-6, "dual = {dual:?}");
        assert!(dual[2].abs() < 1e-6, "dual = {dual:?}");
        assert_eq!(
            lp.basis.variables.as_ref().unwrap(),
            &vec!["basic".to_string(), "basic".to_string()]
        );
    }

    #[test]
    fn solve_subproblem_solves_general_integer_program_with_in_house_solver() {
        let result = solve_subproblem(
            test_job(general_integer_problem()),
            "worker-test".to_string(),
        );

        assert!(result.ok, "subproblem error: {:?}", result.error);
        assert_eq!(result.status, "optimal");
        assert_eq!(result.x.len(), 2);
        assert!(result.z.is_some_and(|z| (z - 3.0).abs() < 1e-6));
        assert!(result
            .x
            .iter()
            .all(|value| { *value >= -1e-6 && (*value - value.round()).abs() < 1e-6 }));
        assert!(result.x.iter().sum::<f64>() <= 3.0 + 1e-6);
    }

    #[test]
    fn solve_subproblem_accepts_named_constraints_with_branch_rows() {
        let mut problem = binary_knapsack_problem();
        problem.var_names = Some(vec![
            "item0".to_string(),
            "item1".to_string(),
            "item2".to_string(),
            "item3".to_string(),
        ]);
        problem.con_names = Some(vec!["capacity".to_string()]);
        let mut job = test_job(problem);
        job.extra_constraints.push(BranchConstraint {
            coefs: vec![1.0, 0.0, 0.0, 0.0],
            rhs: 0.0,
            name: "branch_d0_x0_le_0".to_string(),
        });

        let result = solve_subproblem(job, "worker-test".to_string());

        assert!(result.ok, "subproblem error: {:?}", result.error);
        assert_eq!(result.status, "optimal");
    }

    #[tokio::test]
    async fn master_local_fallback_solves_binary_mip() {
        let state = test_state(NodeRole::Master);
        let options = SolveOptions {
            split_depth: Some(2),
            max_nodes: Some(10_000),
            ..SolveOptions::default()
        };

        let response = solve_problem_distributed(
            state,
            "request-test".to_string(),
            "33333333-3333-4333-8333-333333333333".to_string(),
            3,
            binary_knapsack_problem(),
            options,
        )
        .await
        .unwrap();

        assert!(response.ok, "warnings: {:?}", response.warnings);
        assert_eq!(response.status, "optimal");
        assert_eq!(response.revision, 3);
        assert_eq!(response.z, Some(90.0));
        assert!(!response.distributed);
        assert_eq!(response.jobs_expected, response.jobs_completed);
        assert_eq!(response.jobs_redelegated, 0);
        assert!(response.jobs_published >= response.jobs_completed);
        assert!(response.jobs_published > 0);
    }

    #[cfg(feature = "external-solver-verification")]
    #[tokio::test]
    async fn master_local_fallback_verifies_binary_mip_with_external_highs() {
        if std::process::Command::new("highs")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("SKIP external HiGHS verification test: highs command not installed");
            return;
        }
        let state = test_state(NodeRole::Master);
        let options = SolveOptions {
            split_depth: Some(2),
            max_nodes: Some(10_000),
            verify_external: Some(true),
            external_verification_method: Some("highs".to_string()),
            external_verification_tolerance: Some(1e-6),
            ..SolveOptions::default()
        };

        let response = solve_problem_distributed(
            state,
            "request-external-verify-test".to_string(),
            "88888888-8888-4888-8888-888888888888".to_string(),
            5,
            binary_knapsack_problem(),
            options,
        )
        .await
        .unwrap();

        assert!(response.ok, "warnings: {:?}", response.warnings);
        assert_eq!(response.status, "optimal");
        assert_eq!(response.z, Some(90.0));
        let verification = response
            .external_verification
            .expect("external verification report");
        assert_eq!(verification.status, "verified", "{verification:?}");
        assert_eq!(verification.solution_status.as_deref(), Some("optimal"));
        assert!(verification
            .objective_delta
            .is_some_and(|delta| delta <= verification.tolerance));
        assert!(verification
            .message
            .as_deref()
            .is_some_and(|message| message.contains("usesExternalSolvers=true")));
    }

    #[tokio::test]
    async fn pure_lp_request_uses_single_local_lp_job() {
        let state = test_state(NodeRole::Master);
        let response = solve_problem_distributed(
            state.clone(),
            "request-lp-local".to_string(),
            "44444444-4444-4444-8444-444444444444".to_string(),
            4,
            pure_lp_problem(),
            SolveOptions {
                split_depth: Some(8),
                max_subproblems: Some(1_000),
                ..SolveOptions::default()
            },
        )
        .await
        .unwrap();

        assert!(response.ok, "warnings: {:?}", response.warnings);
        assert_eq!(response.status, "optimal");
        assert_eq!(response.z, Some(10.0));
        assert_eq!(response.jobs_expected, 1);
        assert_eq!(response.jobs_published, 1);
        assert_eq!(response.jobs_completed, 1);
        assert_eq!(response.jobs_redelegated, 0);
        assert_eq!(response.jobs_split, 0);
        assert!(!response.distributed);
        assert!(response.lp.is_some());

        let solves = state.solves.lock().expect("solves mutex poisoned");
        let solve = solves.get(&response.solve_id).expect("tracked LP solve");
        assert!(!solve.distributed);
        assert_eq!(solve.jobs.len(), 1);
        let (job_id, job) = solve.jobs.iter().next().expect("single LP job");
        assert!(job_id.ends_with("-lp"));
        assert_eq!(job.status, "optimal");
        assert_eq!(job.depth, 0);
        drop(solves);

        let tasks = runtime_task_entries(&state);
        assert!(tasks.iter().any(|task| {
            task.kind == "local-lp" && task.solve_id.as_deref() == Some(&response.solve_id)
        }));
    }

    #[test]
    fn simulated_three_slave_delegation_solves_100_by_200_mip() {
        let state = test_state(NodeRole::Master);
        let problem = normalized_problem(hundred_variable_two_hundred_constraint_mip()).unwrap();
        let options = SolveOptions {
            split_depth: Some(2),
            max_subproblems: Some(8),
            max_nodes: Some(10_000),
            max_ticks: Some(10_000),
            ..SolveOptions::default()
        };
        assert_eq!(problem.c.len(), 100);
        assert_eq!(problem.a.len(), 200);

        let (jobs, warnings) = build_frontier_jobs(
            &problem,
            "solve-large-test",
            "request-large-test",
            "55555555-5555-4555-8555-555555555555",
            11,
            "master-test",
            &options,
            false,
        )
        .unwrap();
        assert!(
            jobs.len() >= 3,
            "expected at least three delegated subproblems, got {} with warnings {:?}",
            jobs.len(),
            warnings
        );

        let workers = ["slave-a", "slave-b", "slave-c"];
        let mut pending: VecDeque<SubproblemJob> = jobs.into_iter().collect();
        let mut results = Vec::new();
        let mut used_workers = HashSet::new();
        let mut jobs_expected = pending.len();
        let mut jobs_published = pending.len();
        let mut jobs_split = 0usize;
        let mut next_worker = 0usize;

        while let Some(job) = pending.pop_front() {
            let worker = workers[next_worker % workers.len()].to_string();
            next_worker += 1;
            used_workers.insert(worker.clone());
            let result = solve_subproblem(job, worker);
            if result.status == "split" && !result.child_jobs.is_empty() {
                jobs_split += 1;
                jobs_expected =
                    jobs_expected.saturating_add(result.child_jobs.len().saturating_sub(1));
                jobs_published = jobs_published.saturating_add(result.child_jobs.len());
                for child in result.child_jobs {
                    pending.push_back(child);
                }
            } else {
                results.push(result);
            }
        }

        let response = aggregate_results(
            "solve-large-test".to_string(),
            "request-large-test".to_string(),
            None,
            11,
            &problem,
            &options,
            jobs_expected,
            jobs_published,
            0,
            jobs_split,
            results,
            false,
            true,
            &state,
            warnings,
        );

        assert_eq!(used_workers.len(), 3, "used workers: {used_workers:?}");
        assert!(response.ok, "response warnings: {:?}", response.warnings);
        assert_eq!(response.status, "optimal");
        assert_eq!(response.revision, 11);
        assert_eq!(response.jobs_expected, response.jobs_completed);
        assert!(response.jobs_published >= 3);
        assert!(response.distributed);
        assert_eq!(response.z, Some(2.0));
        assert_eq!(response.x.len(), 100);
        assert_eq!(
            response
                .x
                .iter()
                .take(3)
                .filter(|value| **value > 0.5)
                .count(),
            2
        );
    }

    #[test]
    fn simulated_three_slave_delegation_solves_real_100_by_150_dispatch_mip() {
        let state = test_state(NodeRole::Master);
        let problem =
            normalized_problem(hundred_variable_one_hundred_fifty_constraint_dispatch_mip())
                .unwrap();
        let options = SolveOptions {
            split_depth: Some(2),
            max_subproblems: Some(8),
            max_nodes: Some(20_000),
            max_ticks: Some(20_000),
            lp_max_iters: Some(10_000),
            ..SolveOptions::default()
        };
        #[cfg(feature = "external-solver-verification")]
        let (options, expect_external_verification) = {
            let mut options = options;
            let expect_external_verification = if std::process::Command::new("highs")
                .arg("--version")
                .output()
                .is_ok()
            {
                options.verify_external = Some(true);
                options.external_verification_method = Some("highs".to_string());
                options.external_verification_tolerance = Some(1e-6);
                true
            } else {
                eprintln!("SKIP 100x150 external HiGHS verification: highs command not installed");
                false
            };
            (options, expect_external_verification)
        };
        #[cfg(not(feature = "external-solver-verification"))]
        let expect_external_verification = false;
        assert_eq!(problem.c.len(), 100);
        assert_eq!(problem.a.len(), 150);

        let (jobs, warnings) = build_frontier_jobs(
            &problem,
            "solve-dispatch-test",
            "request-dispatch-test",
            "66666666-6666-4666-8666-666666666666",
            12,
            "master-test",
            &options,
            false,
        )
        .unwrap();
        assert!(
            jobs.len() >= 3,
            "expected at least three delegated subproblems, got {} with warnings {:?}",
            jobs.len(),
            warnings
        );

        let workers = ["slave-a", "slave-b", "slave-c"];
        let mut pending: VecDeque<SubproblemJob> = jobs.into_iter().collect();
        let mut results = Vec::new();
        let mut used_workers = HashSet::new();
        let mut jobs_expected = pending.len();
        let mut jobs_published = pending.len();
        let mut jobs_split = 0usize;
        let mut next_worker = 0usize;

        while let Some(job) = pending.pop_front() {
            let worker = workers[next_worker % workers.len()].to_string();
            next_worker += 1;
            used_workers.insert(worker.clone());
            let result = solve_subproblem(job, worker);
            if result.status == "split" && !result.child_jobs.is_empty() {
                jobs_split += 1;
                jobs_expected =
                    jobs_expected.saturating_add(result.child_jobs.len().saturating_sub(1));
                jobs_published = jobs_published.saturating_add(result.child_jobs.len());
                for child in result.child_jobs {
                    pending.push_back(child);
                }
            } else {
                results.push(result);
            }
        }

        let response = aggregate_results(
            "solve-dispatch-test".to_string(),
            "request-dispatch-test".to_string(),
            None,
            12,
            &problem,
            &options,
            jobs_expected,
            jobs_published,
            0,
            jobs_split,
            results,
            false,
            true,
            &state,
            warnings,
        );

        let expected_z = (0..49).map(|pair| 1000.0 - pair as f64).sum::<f64>();
        assert_eq!(used_workers.len(), 3, "used workers: {used_workers:?}");
        assert!(response.ok, "response warnings: {:?}", response.warnings);
        assert_eq!(response.status, "optimal");
        assert_eq!(response.revision, 12);
        assert_eq!(response.jobs_expected, response.jobs_completed);
        assert!(response.jobs_published >= 3);
        assert!(response.distributed);
        assert_eq!(response.z, Some(expected_z));
        assert_eq!(response.x.len(), 100);
        assert_eq!(response.x.iter().filter(|value| **value > 0.5).count(), 49);
        for pair in 0..49 {
            assert!(
                response.x[pair * 2] > 0.5,
                "expected primary dispatch option for pair {pair}"
            );
            assert!(
                response.x[pair * 2 + 1] < 1e-6,
                "did not expect alternate dispatch option for pair {pair}"
            );
        }
        assert!(response.x[98] < 1e-6);
        assert!(response.x[99] < 1e-6);
        if expect_external_verification {
            let verification = response
                .external_verification
                .as_ref()
                .expect("external verification report");
            assert_eq!(verification.status, "verified", "{verification:?}");
            assert_eq!(verification.method.as_deref(), Some("highs"));
            assert_eq!(
                verification.solver.as_deref(),
                Some("des-ipmip-external-lp")
            );
            assert_eq!(verification.solution_status.as_deref(), Some("optimal"));
            assert!(
                verification
                    .objective_delta
                    .is_some_and(|delta| delta <= verification.tolerance),
                "{verification:?}"
            );
            assert!(
                verification.objective.is_some_and(
                    |objective| (objective - expected_z).abs() <= verification.tolerance
                ),
                "{verification:?}"
            );
            assert!(
                verification
                    .message
                    .as_deref()
                    .is_some_and(|message| message.contains("usesExternalSolvers=true")),
                "{verification:?}"
            );
        } else {
            assert!(response.external_verification.is_none());
        }
    }

    #[cfg(feature = "external-solver-verification")]
    #[test]
    fn external_highs_methods_verify_real_100_by_150_dispatch_mip_optimum() {
        if std::process::Command::new("highs")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("SKIP external HiGHS method verification: highs command not installed");
            return;
        }
        let problem =
            normalized_problem(hundred_variable_one_hundred_fifty_constraint_dispatch_mip())
                .unwrap();
        let options = SolveOptions {
            max_nodes: Some(20_000),
            max_ticks: Some(20_000),
            lp_max_iters: Some(10_000),
            ..SolveOptions::default()
        };
        let expected_z = (0..49).map(|pair| 1000.0 - pair as f64).sum::<f64>();

        for method in ["highs", "highs-ds", "highs-ipm"] {
            let verification =
                run_external_verification(&problem, &options, expected_z, method, 1e-6)
                    .unwrap_or_else(|error| panic!("{method} verification failed: {error}"));
            assert_eq!(
                verification.status, "verified",
                "{method}: {verification:?}"
            );
            assert_eq!(verification.method.as_deref(), Some(method));
            assert_eq!(
                verification.solver.as_deref(),
                Some("des-ipmip-external-lp")
            );
            assert_eq!(verification.solution_status.as_deref(), Some("optimal"));
            assert!(
                verification
                    .objective
                    .is_some_and(|objective| (objective - expected_z).abs() <= 1e-6),
                "{method}: {verification:?}"
            );
            assert!(
                verification
                    .objective_delta
                    .is_some_and(|delta| delta <= verification.tolerance),
                "{method}: {verification:?}"
            );
            assert!(
                verification
                    .message
                    .as_deref()
                    .is_some_and(|message| message.contains("usesExternalSolvers=true")),
                "{method}: {verification:?}"
            );
        }
    }

    #[tokio::test]
    async fn http_solve_endpoint_solves_binary_mip() {
        let app = app_router(test_state(NodeRole::Master));
        let problem_id = "33333333-3333-4333-8333-333333333333";
        let payload = json!({
            "requestId": "http-test",
            "problemId": problem_id,
            "problem": binary_knapsack_problem(),
            "options": {
                "splitDepth": 2,
                "maxNodes": 10000
            }
        });

        let (status, body) = post_json(app, "/solve", payload).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.get("ok"), Some(&json!(true)));
        assert_eq!(body.get("status"), Some(&json!("optimal")));
        assert_eq!(body.get("z"), Some(&json!(90.0)));
        assert_eq!(body.get("distributed"), Some(&json!(false)));
        assert_eq!(body.get("problemId"), Some(&json!(problem_id)));
        assert_eq!(body.pointer("/role"), Some(&json!("master")));
    }

    #[tokio::test]
    async fn http_problem_upload_then_solve_uses_stored_model() {
        let app = app_router(test_state(NodeRole::Master));
        let problem_id = "99999999-9999-4999-8999-999999999999";

        let (upload_status, upload_body) = post_json(
            app.clone(),
            &format!("/problems/{problem_id}"),
            json!({
                "problem": binary_knapsack_problem()
            }),
        )
        .await;
        assert_eq!(upload_status, StatusCode::OK);
        assert_eq!(upload_body.get("ok"), Some(&json!(true)));
        assert_eq!(upload_body.get("problemId"), Some(&json!(problem_id)));
        assert_eq!(upload_body.get("storeStatus"), Some(&json!("created")));
        assert_eq!(upload_body.get("redisStored"), Some(&json!(false)));

        let (solve_status, solve_body) = post_json(
            app,
            &format!("/problems/{problem_id}/solve"),
            json!({
                "requestId": "stored-problem-http-test",
                "options": {
                    "splitDepth": 2,
                    "maxNodes": 10000
                }
            }),
        )
        .await;

        assert_eq!(solve_status, StatusCode::OK);
        assert_eq!(solve_body.get("ok"), Some(&json!(true)));
        assert_eq!(solve_body.get("status"), Some(&json!("optimal")));
        assert_eq!(solve_body.get("z"), Some(&json!(90.0)));
        assert_eq!(solve_body.get("problemId"), Some(&json!(problem_id)));
    }

    #[tokio::test]
    async fn http_problem_upload_rejects_different_model_for_existing_problem_id() {
        let app = app_router(test_state(NodeRole::Master));
        let problem_id = "9aaaaaaa-9999-4999-8999-999999999999";
        let mut changed = binary_knapsack_problem();
        changed.c[0] = 11.0;

        let (first_status, first_body) = post_json(
            app.clone(),
            &format!("/problems/{problem_id}"),
            json!({"problem": binary_knapsack_problem()}),
        )
        .await;
        let (second_status, second_body) = post_json(
            app,
            &format!("/problems/{problem_id}"),
            json!({"problem": changed}),
        )
        .await;

        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(first_body.get("storeStatus"), Some(&json!("created")));
        assert_eq!(second_status, StatusCode::CONFLICT);
        assert_eq!(second_body.get("ok"), Some(&json!(false)));
        assert!(second_body
            .get("error")
            .and_then(Value::as_str)
            .is_some_and(is_problem_model_conflict));
    }

    #[tokio::test]
    async fn http_solve_rejects_duplicate_running_problem_uuid() {
        let state = test_state(NodeRole::Master);
        let problem_id = "55555555-5555-4555-8555-555555555555";
        state.solves.lock().expect("solves mutex poisoned").insert(
            "solve-active-problem".to_string(),
            SolveRegistryEntry {
                solve_id: "solve-active-problem".to_string(),
                request_id: "request-active-problem".to_string(),
                problem_id: Some(problem_id.to_string()),
                status: "running".to_string(),
                jobs_expected: 1,
                started_at_ms: 1000,
                updated_at_ms: 1000,
                ..SolveRegistryEntry::default()
            },
        );
        let app = app_router(state);
        let payload = json!({
            "requestId": "duplicate-problem",
            "problemId": problem_id,
            "problem": binary_knapsack_problem()
        });

        let (status, body) = post_json(app, "/solve", payload).await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body.get("ok"), Some(&json!(false)));
        assert!(body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("already has running solve"));
    }

    #[tokio::test]
    async fn metrics_endpoint_exposes_prometheus_node_and_inflight_metrics() {
        let state = test_state(NodeRole::Master);
        state
            .metrics
            .subproblem_jobs_published_total
            .store(7, Ordering::Relaxed);
        state
            .metrics
            .subproblem_jobs_completed_total
            .store(3, Ordering::Relaxed);
        state
            .metrics
            .subproblem_jobs_redelegated_total
            .store(2, Ordering::Relaxed);
        state
            .metrics
            .subproblem_jobs_split_total
            .store(4, Ordering::Relaxed);
        state
            .metrics
            .worker_control_messages_total
            .store(5, Ordering::Relaxed);
        state
            .workers
            .lock()
            .expect("workers mutex poisoned")
            .insert(
                "worker-a".to_string(),
                WorkerNodeStatus {
                    node_id: "worker-a".to_string(),
                    last_command: "worker-ready".to_string(),
                    last_seen_ms: 1000,
                    ..WorkerNodeStatus::default()
                },
            );
        state.solves.lock().expect("solves mutex poisoned").insert(
            "solve-a".to_string(),
            SolveRegistryEntry {
                solve_id: "solve-a".to_string(),
                request_id: "request-a".to_string(),
                status: "running".to_string(),
                jobs_expected: 2,
                started_at_ms: 1000,
                updated_at_ms: 1000,
                ..SolveRegistryEntry::default()
            },
        );
        let app = app_router(state);

        let (status, body) = get_text(app, "/metrics").await;

        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("# TYPE dd_mip_solver_node_info gauge"));
        assert!(body.contains("dd_mip_solver_node_info{role=\"master\",node_id=\"test-node\"} 1"));
        assert!(body.contains("# TYPE dd_mip_solver_subproblem_jobs_in_flight gauge"));
        assert!(body.contains("dd_mip_solver_subproblem_jobs_in_flight 4"));
        assert!(body.contains("# TYPE dd_mip_solver_subproblem_jobs_redelegated_total counter"));
        assert!(body.contains("dd_mip_solver_subproblem_jobs_redelegated_total 2"));
        assert!(body.contains("# TYPE dd_mip_solver_subproblem_jobs_split_total counter"));
        assert!(body.contains("dd_mip_solver_subproblem_jobs_split_total 4"));
        assert!(body.contains("# TYPE dd_mip_solver_workers_known gauge"));
        assert!(body.contains("dd_mip_solver_workers_known 1"));
        assert!(body.contains("# TYPE dd_mip_solver_worker_control_messages_total counter"));
        assert!(body.contains("dd_mip_solver_worker_control_messages_total 5"));
        assert!(body.contains("# TYPE dd_mip_solver_solves_tracked gauge"));
        assert!(body.contains("dd_mip_solver_solves_tracked 1"));
        assert!(body.contains("# TYPE dd_mip_solver_active_solves gauge"));
        assert!(body.contains("dd_mip_solver_active_solves 1"));
    }

    #[tokio::test]
    async fn mip_solver_cluster_workers_endpoint_reports_master_observed_slaves() {
        let state = test_state(NodeRole::Master);
        record_worker_control_frame(
            &state,
            &json!({
                "schema":"dd.mip-solver.control.v1",
                "service": SERVICE_NAME,
                "nodeId":"worker-b",
                "role":"slave",
                "commandName":"worker-ready",
                "payload":{
                    "consumer":"dd-in-house-mip-solver-node-workers",
                    "jobsSubject": MIP_SOLVER_JOBS_SUBJECT,
                    "resultsSubject": MIP_SOLVER_RESULTS_SUBJECT
                },
                "timeMs": 2000
            }),
        )
        .unwrap();
        let app = app_router(state);

        let (status, body) = get_json(app.clone(), "/workers").await;
        let (compat_status, compat_body) = get_json(app, "/mip-solver-cluster/workers").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(compat_status, StatusCode::OK);
        assert_eq!(body.get("ok"), Some(&json!(true)));
        assert_eq!(body.get("count"), Some(&json!(1)));
        assert_eq!(compat_body.get("count"), Some(&json!(1)));
        assert_eq!(body.pointer("/workers/0/nodeId"), Some(&json!("worker-b")));
        assert_eq!(
            body.pointer("/workers/0/consumer"),
            Some(&json!("dd-in-house-mip-solver-node-workers"))
        );
    }

    #[tokio::test]
    async fn runtime_tasks_endpoint_reports_live_problem_threads() {
        let state = test_state(NodeRole::Master);
        let problem_id = "12121212-1212-4212-8212-121212121212";
        track_runtime_task_started(
            &state,
            "problem-task-a".to_string(),
            "problem",
            Some(problem_id.to_string()),
            Some("solve-task-a".to_string()),
            Some("request-task-a".to_string()),
            None,
            None,
            None,
        );
        let app = app_router(state);

        let (status, body) = get_json(app.clone(), "/tasks").await;
        let (compat_status, compat_body) = get_json(app.clone(), "/mip-solver-cluster/tasks").await;
        let (lookup_status, lookup_body) = get_json(app, &format!("/tasks/{problem_id}")).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(compat_status, StatusCode::OK);
        assert_eq!(lookup_status, StatusCode::OK);
        assert_eq!(body.get("count"), Some(&json!(1)));
        assert_eq!(body.get("active"), Some(&json!(1)));
        assert_eq!(compat_body.get("count"), Some(&json!(1)));
        assert_eq!(lookup_body.pointer("/task/kind"), Some(&json!("problem")));
        assert_eq!(
            lookup_body.pointer("/task/problemId"),
            Some(&json!(problem_id))
        );
        assert_eq!(
            lookup_body.pointer("/task/solveId"),
            Some(&json!("solve-task-a"))
        );
    }

    #[tokio::test]
    async fn mip_solver_cluster_solves_endpoint_reports_tracked_jobs() {
        let state = test_state(NodeRole::Master);
        let app = app_router(state.clone());
        let response = solve_problem_distributed(
            state.clone(),
            "tracked-solve".to_string(),
            "44444444-4444-4444-8444-444444444444".to_string(),
            9,
            pure_lp_problem(),
            SolveOptions::default(),
        )
        .await
        .unwrap();

        assert_eq!(response.status, "optimal");
        let (status, body) = get_json(app.clone(), "/solves").await;
        let (compat_status, compat_body) = get_json(app, "/mip-solver-cluster/solves").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(compat_status, StatusCode::OK);
        assert_eq!(body.get("ok"), Some(&json!(true)));
        assert_eq!(body.get("count"), Some(&json!(1)));
        assert_eq!(body.get("active"), Some(&json!(0)));
        assert_eq!(compat_body.get("count"), Some(&json!(1)));
        assert_eq!(
            body.pointer("/solves/0/requestId"),
            Some(&json!("tracked-solve"))
        );
        assert_eq!(body.pointer("/solves/0/status"), Some(&json!("optimal")));
        assert_eq!(body.pointer("/solves/0/jobsExpected"), Some(&json!(1)));
        assert_eq!(body.pointer("/solves/0/jobsCompleted"), Some(&json!(1)));
        let jobs = body
            .pointer("/solves/0/jobs")
            .and_then(Value::as_object)
            .expect("jobs object");
        assert_eq!(jobs.len(), 1);
        let job = jobs.values().next().expect("one job");
        assert_eq!(job.get("status"), Some(&json!("optimal")));
    }

    #[tokio::test]
    async fn solves_endpoint_filters_by_problem_uuid_query() {
        let state = test_state(NodeRole::Master);
        let problem_id = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        let other_problem_id = Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap();
        state.solves.lock().expect("solves mutex poisoned").extend([
            (
                "solve-problem-a".to_string(),
                SolveRegistryEntry {
                    solve_id: "solve-problem-a".to_string(),
                    request_id: "request-problem-a".to_string(),
                    problem_id: Some(problem_id.to_string()),
                    status: "running".to_string(),
                    jobs_expected: 2,
                    jobs_published: 2,
                    jobs_completed: 1,
                    started_at_ms: 2000,
                    updated_at_ms: 2100,
                    ..SolveRegistryEntry::default()
                },
            ),
            (
                "solve-problem-b".to_string(),
                SolveRegistryEntry {
                    solve_id: "solve-problem-b".to_string(),
                    request_id: "request-problem-b".to_string(),
                    problem_id: Some(other_problem_id.to_string()),
                    status: "running".to_string(),
                    jobs_expected: 3,
                    jobs_published: 3,
                    jobs_completed: 0,
                    started_at_ms: 3000,
                    updated_at_ms: 3100,
                    ..SolveRegistryEntry::default()
                },
            ),
        ]);
        let app = app_router(state);

        let (status, body) = get_json(app.clone(), &format!("/solves?problem={problem_id}")).await;
        let (compat_status, compat_body) = get_json(
            app.clone(),
            &format!("/mip-solver-cluster/solves?problem={problem_id}"),
        )
        .await;
        let (invalid_status, invalid_body) = get_json(app, "/solves?problem=not-a-uuid").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(compat_status, StatusCode::OK);
        assert_eq!(body.get("problem"), Some(&json!(problem_id.to_string())));
        assert_eq!(body.get("count"), Some(&json!(1)));
        assert_eq!(body.get("active"), Some(&json!(1)));
        assert_eq!(
            body.pointer("/solves/0/solveId"),
            Some(&json!("solve-problem-a"))
        );
        assert_eq!(compat_body.get("count"), Some(&json!(1)));
        assert_eq!(invalid_status, StatusCode::BAD_REQUEST);
        assert_eq!(invalid_body.get("ok"), Some(&json!(false)));
    }

    #[tokio::test]
    async fn tasks_endpoint_reports_problem_and_job_uuid_lookups() {
        let state = test_state(NodeRole::Master);
        let problem_id = "99999999-9999-4999-8999-999999999999".to_string();
        let job_uuid = new_uuid_string();
        track_runtime_task_started(
            &state,
            problem_id.clone(),
            "problem",
            Some(problem_id.clone()),
            Some("solve-task-test".to_string()),
            Some("request-task-test".to_string()),
            None,
            None,
            None,
        );
        track_runtime_task_started(
            &state,
            job_uuid.clone(),
            "local-subproblem",
            Some(problem_id.clone()),
            Some("solve-task-test".to_string()),
            Some("request-task-test".to_string()),
            Some("job-task-test".to_string()),
            Some(job_uuid.clone()),
            None,
        );
        let app = app_router(state);

        let (list_status, list_body) = get_json(app.clone(), "/tasks").await;
        let (problem_status, problem_body) =
            get_json(app.clone(), &format!("/tasks/{problem_id}")).await;
        let (job_status, job_body) = get_json(app, &format!("/tasks/{job_uuid}")).await;

        assert_eq!(list_status, StatusCode::OK);
        assert_eq!(list_body.get("count"), Some(&json!(2)));
        assert_eq!(list_body.get("active"), Some(&json!(2)));
        assert_eq!(problem_status, StatusCode::OK);
        assert_eq!(problem_body.pointer("/task/kind"), Some(&json!("problem")));
        assert_eq!(job_status, StatusCode::OK);
        assert_eq!(
            job_body.pointer("/task/jobId"),
            Some(&json!("job-task-test"))
        );
        assert_eq!(job_body.pointer("/task/jobUuid"), Some(&json!(job_uuid)));
    }

    #[tokio::test]
    async fn cancel_endpoint_marks_running_solve_by_request_id() {
        let state = test_state(NodeRole::Master);
        let problem_id = "77777777-7777-4777-8777-777777777777";
        state.solves.lock().expect("solves mutex poisoned").insert(
            "solve-cancel-test".to_string(),
            SolveRegistryEntry {
                solve_id: "solve-cancel-test".to_string(),
                request_id: "request-cancel-test".to_string(),
                problem_id: Some(problem_id.to_string()),
                status: "running".to_string(),
                jobs_expected: 4,
                jobs_published: 2,
                started_at_ms: 1000,
                updated_at_ms: 1000,
                ..SolveRegistryEntry::default()
            },
        );
        let app = app_router(state.clone());

        let (status, body) = post_json(
            app,
            "/mip-solver-cluster/requests/request-cancel-test/cancel",
            json!({"reason":"client changed the model","requestedBy":"unit-test"}),
        )
        .await;

        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(body.get("ok"), Some(&json!(true)));
        assert_eq!(body.get("status"), Some(&json!("cancelling")));
        assert_eq!(body.get("solveId"), Some(&json!("solve-cancel-test")));
        assert_eq!(body.get("problemId"), Some(&json!(problem_id)));
        assert!(solve_cancel_requested(&state, "solve-cancel-test"));
        let solves = state.solves.lock().expect("solves mutex poisoned");
        let solve = solves.get("solve-cancel-test").unwrap();
        assert_eq!(solve.status, "cancelling");
        assert!(solve.cancel_requested);
        assert_eq!(
            solve.cancel_reason.as_deref(),
            Some("client changed the model")
        );
    }

    #[tokio::test]
    async fn cancel_endpoint_marks_running_solve_by_problem_id() {
        let state = test_state(NodeRole::Master);
        let problem_id = "88888888-8888-4888-8888-888888888888";
        state.solves.lock().expect("solves mutex poisoned").insert(
            "solve-cancel-problem-test".to_string(),
            SolveRegistryEntry {
                solve_id: "solve-cancel-problem-test".to_string(),
                request_id: "request-cancel-problem-test".to_string(),
                problem_id: Some(problem_id.to_string()),
                status: "running".to_string(),
                jobs_expected: 2,
                started_at_ms: 1000,
                updated_at_ms: 1000,
                ..SolveRegistryEntry::default()
            },
        );
        let app = app_router(state.clone());

        let (status, body) = post_json(
            app,
            &format!("/problems/{problem_id}/cancel"),
            json!({"reason":"problem cancelled","requestedBy":"unit-test"}),
        )
        .await;

        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(
            body.get("solveId"),
            Some(&json!("solve-cancel-problem-test"))
        );
        assert_eq!(body.get("problemId"), Some(&json!(problem_id)));
        assert!(solve_cancel_requested(&state, "solve-cancel-problem-test"));
    }

    #[tokio::test]
    async fn delete_endpoint_cleans_finished_solve_from_memory() {
        let state = test_state(NodeRole::Master);
        state.solves.lock().expect("solves mutex poisoned").insert(
            "solve-clean-test".to_string(),
            SolveRegistryEntry {
                solve_id: "solve-clean-test".to_string(),
                request_id: "request-clean-test".to_string(),
                status: "cancelled".to_string(),
                jobs_expected: 1,
                jobs_published: 1,
                started_at_ms: 1000,
                updated_at_ms: 1200,
                finished_at_ms: Some(1200),
                cancel_requested: true,
                cancel_requested_at_ms: Some(1100),
                cancel_reason: Some("test cleanup".to_string()),
                jobs: HashMap::from([(
                    "job-clean-test".to_string(),
                    JobRegistryEntry {
                        job_id: "job-clean-test".to_string(),
                        status: "cancelled".to_string(),
                        submitted_at_ms: 1000,
                        finished_at_ms: Some(1200),
                        ..JobRegistryEntry::default()
                    },
                )]),
                ..SolveRegistryEntry::default()
            },
        );
        state
            .cancelled_solves
            .lock()
            .expect("cancelled solves mutex poisoned")
            .insert(
                "solve-clean-test".to_string(),
                CancelInfo {
                    solve_id: "solve-clean-test".to_string(),
                    request_id: Some("request-clean-test".to_string()),
                    problem_id: None,
                    reason: "test cleanup".to_string(),
                    requested_by: "unit-test".to_string(),
                    requested_at_ms: 1100,
                },
            );
        let app = app_router(state.clone());

        let (status, body) = delete_json(app, "/mip-solver-cluster/solves/solve-clean-test").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.get("status"), Some(&json!("cleaned")));
        assert_eq!(body.get("jobsRemoved"), Some(&json!(1)));
        assert!(!state
            .solves
            .lock()
            .expect("solves mutex poisoned")
            .contains_key("solve-clean-test"));
        assert!(!state
            .cancelled_solves
            .lock()
            .expect("cancelled solves mutex poisoned")
            .contains_key("solve-clean-test"));
    }

    #[test]
    fn cancel_control_frames_update_local_cancel_map_without_solve_registry() {
        let state = test_state(NodeRole::Slave);

        let handled = record_cancel_control_frame(
            &state,
            &json!({
                "schema":"dd.mip-solver.control.v1",
                "service": SERVICE_NAME,
                "nodeId":"master-a",
                "role":"master",
                "commandName":"cancel-solve",
                "payload":{
                    "solveId":"solve-cancel-broadcast",
                    "requestId":"request-cancel-broadcast",
                    "reason":"client cancelled",
                    "requestedBy":"master-a",
                    "requestedAtMs": 3000
                },
                "timeMs": 3001
            }),
        )
        .unwrap();

        assert!(handled);
        let info = solve_cancel_info(&state, "solve-cancel-broadcast").unwrap();
        assert_eq!(info.request_id.as_deref(), Some("request-cancel-broadcast"));
        assert_eq!(info.reason, "client cancelled");
        assert_eq!(info.requested_at_ms, 3000);
    }

    #[test]
    fn cancel_control_frame_resolves_problem_id_to_running_solve() {
        let state = test_state(NodeRole::Master);
        let problem_id = "99999999-9999-4999-8999-999999999999";
        state.solves.lock().expect("solves mutex poisoned").insert(
            "solve-problem-cancel".to_string(),
            SolveRegistryEntry {
                solve_id: "solve-problem-cancel".to_string(),
                request_id: "request-problem-cancel".to_string(),
                problem_id: Some(problem_id.to_string()),
                status: "running".to_string(),
                jobs_expected: 1,
                started_at_ms: 1000,
                updated_at_ms: 1000,
                ..SolveRegistryEntry::default()
            },
        );

        let handled = record_cancel_control_frame(
            &state,
            &json!({
                "schema":"dd.mip-solver.control.v1",
                "service": SERVICE_NAME,
                "nodeId":"master-a",
                "role":"master",
                "commandName":"cancel-solve",
                "payload":{
                    "problemId": problem_id,
                    "reason":"nats problem cancel",
                    "requestedBy":"master-a",
                    "requestedAtMs": 4000
                },
                "timeMs": 4001
            }),
        )
        .unwrap();

        assert!(handled);
        let info = solve_cancel_info(&state, "solve-problem-cancel").unwrap();
        assert_eq!(info.request_id.as_deref(), Some("request-problem-cancel"));
        assert_eq!(info.problem_id.as_deref(), Some(problem_id));
        assert_eq!(info.reason, "nats problem cancel");
        let solves = state.solves.lock().expect("solves mutex poisoned");
        let solve = solves.get("solve-problem-cancel").unwrap();
        assert_eq!(solve.status, "cancelling");
        assert!(solve.cancel_requested);
    }

    #[test]
    fn cancel_control_frame_records_problem_id_for_workers_without_registry() {
        let state = test_state(NodeRole::Slave);
        let problem_id = "abababab-abab-4bab-8bab-abababababab";

        let handled = record_cancel_control_frame(
            &state,
            &json!({
                "schema":"dd.mip-solver.control.v1",
                "service": SERVICE_NAME,
                "nodeId":"master-a",
                "role":"master",
                "commandName":"cancel-solve",
                "payload":{
                    "problemId": problem_id,
                    "reason":"nats problem cancel",
                    "requestedBy":"master-a",
                    "requestedAtMs": 5000
                },
                "timeMs": 5001
            }),
        )
        .unwrap();

        assert!(handled);
        let info = solve_cancel_info(&state, problem_id).unwrap();
        assert_eq!(info.problem_id.as_deref(), Some(problem_id));
        assert_eq!(info.reason, "nats problem cancel");
        assert!(solve_cancel_requested_for(
            &state,
            "solve-worker-local",
            Some(problem_id)
        ));
    }

    #[tokio::test]
    async fn readyz_requires_nats_connection_for_cluster_readiness() {
        let app = app_router(test_state(NodeRole::Slave));

        let (status, body) = get_json(app, "/readyz").await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body.get("ok"), Some(&json!(false)));
        assert_eq!(body.get("nats"), Some(&json!(false)));
        assert!(body
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("NATS connection is required"));
    }

    #[tokio::test]
    async fn http_slave_rejects_master_solve_endpoint() {
        let app = app_router(test_state(NodeRole::Slave));
        let payload = json!({
            "requestId": "slave-test",
            "problem": binary_knapsack_problem()
        });

        let (status, body) = post_json(app, "/solve", payload).await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body.get("ok"), Some(&json!(false)));
        assert!(body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains("booted as slave"));
    }

    #[tokio::test]
    async fn http_session_streams_dynamic_edits_then_solves() {
        let app = app_router(test_state(NodeRole::Master));
        let commands = json!([
            {
                "op": "init",
                "sense": "max",
                "c": [10.0, 40.0, 30.0, 50.0],
                "a": [[5.0, 4.0, 6.0, 3.0]],
                "b": [7.0],
                "integerVars": [true, true, true, true],
                "ub": [1.0, 1.0, 1.0, 1.0]
            },
            {
                "op": "set_rhs",
                "index": 0,
                "rhs": 10.0
            },
            {
                "op": "snapshot"
            }
        ]);

        let (events_status, events_body) =
            post_json(app.clone(), "/sessions/live-mip/events", commands).await;
        assert_eq!(events_status, StatusCode::OK);
        assert_eq!(events_body.get("ok"), Some(&json!(true)));
        assert_eq!(events_body.get("revision"), Some(&json!(2)));
        assert!(events_body
            .get("frames")
            .and_then(Value::as_array)
            .is_some_and(|frames| frames
                .iter()
                .any(|frame| frame.get("event") == Some(&json!("model")))));

        let solve_payload = json!({
            "requestId": "live-mip",
            "options": {
                "splitDepth": 2,
                "maxNodes": 10000
            }
        });
        let (solve_status, solve_body) =
            post_json(app, "/sessions/live-mip/solve", solve_payload).await;

        assert_eq!(solve_status, StatusCode::OK);
        assert_eq!(solve_body.get("ok"), Some(&json!(true)));
        assert_eq!(solve_body.get("status"), Some(&json!("optimal")));
        assert_eq!(solve_body.get("revision"), Some(&json!(2)));
        assert_eq!(solve_body.get("z"), Some(&json!(90.0)));
    }

    #[tokio::test]
    async fn http_session_streams_lp_edits_and_returns_primal_dual_certificate() {
        let app = app_router(test_state(NodeRole::Master));
        let commands = json!([
            {
                "op": "init",
                "sense": "max",
                "c": [3.0, 2.0],
                "a": [[1.0, 1.0], [1.0, 0.0]],
                "b": [4.0, 2.0],
                "integerVars": [false, false],
                "varNames": ["x0", "x1"],
                "conNames": ["shared", "x0_cap"]
            },
            {
                "op": "add_constraint",
                "coefs": [0.0, 1.0],
                "rhs": 3.0,
                "name": "x1_cap"
            },
            {
                "op": "change_constraint_weight",
                "row": 2,
                "col": 1,
                "value": 1.0
            },
            {
                "op": "snapshot"
            }
        ]);

        let (events_status, events_body) =
            post_json(app.clone(), "/sessions/live-lp/events", commands).await;
        assert_eq!(events_status, StatusCode::OK);
        assert_eq!(events_body.get("ok"), Some(&json!(true)));
        assert_eq!(events_body.get("revision"), Some(&json!(3)));

        let (solve_status, solve_body) = post_json(
            app,
            "/sessions/live-lp/solve",
            json!({"requestId":"live-lp"}),
        )
        .await;

        assert_eq!(solve_status, StatusCode::OK);
        assert_eq!(solve_body.get("ok"), Some(&json!(true)));
        assert_eq!(solve_body.get("status"), Some(&json!("optimal")));
        assert_eq!(solve_body.get("distributed"), Some(&json!(false)));
        assert_eq!(
            solve_body.pointer("/lp/primal/objective"),
            Some(&json!(10.0))
        );
        assert_eq!(solve_body.pointer("/lp/primal/x"), Some(&json!([2.0, 2.0])));
        let dual = solve_body
            .pointer("/lp/dual/inequality")
            .and_then(Value::as_array)
            .expect("LP inequality duals");
        assert_eq!(dual.len(), 3);
        assert!((dual[0].as_f64().unwrap() - 2.0).abs() < 1e-6);
        assert!((dual[1].as_f64().unwrap() - 1.0).abs() < 1e-6);
        assert!(dual[2].as_f64().unwrap().abs() < 1e-6);
        assert_eq!(
            solve_body.pointer("/lp/dual/rowNames"),
            Some(&json!(["shared", "x0_cap", "x1_cap"]))
        );
    }

    #[test]
    fn aggregate_results_counts_infeasible_subtrees_as_complete() {
        let problem = binary_knapsack_problem();
        let state = test_state(NodeRole::Master);
        let optimal = SubproblemResult {
            solve_id: "solve-test".to_string(),
            request_id: "request-test".to_string(),
            job_id: "job-0".to_string(),
            job_uuid: new_uuid_string(),
            problem_id: Some("11111111-1111-4111-8111-111111111111".to_string()),
            revision: 0,
            worker_node: "worker-a".to_string(),
            ok: true,
            status: "optimal".to_string(),
            z: Some(90.0),
            x: vec![0.0, 1.0, 0.0, 1.0],
            best_bound: Some(90.0),
            gap: Some(0.0),
            lp: None,
            child_jobs: Vec::new(),
            nodes_explored: 1,
            lp_solves: 1,
            elapsed_ms: 1.0,
            accelerator: AcceleratorReport::default(),
            error: None,
            finished_at_ms: now_ms(),
        };
        let infeasible = SubproblemResult {
            solve_id: "solve-test".to_string(),
            request_id: "request-test".to_string(),
            job_id: "job-1".to_string(),
            job_uuid: new_uuid_string(),
            problem_id: Some("11111111-1111-4111-8111-111111111111".to_string()),
            revision: 0,
            worker_node: "worker-b".to_string(),
            ok: false,
            status: "infeasible".to_string(),
            z: None,
            x: Vec::new(),
            best_bound: None,
            gap: None,
            lp: None,
            child_jobs: Vec::new(),
            nodes_explored: 0,
            lp_solves: 0,
            elapsed_ms: 1.0,
            accelerator: AcceleratorReport::default(),
            error: Some("pruned".to_string()),
            finished_at_ms: now_ms(),
        };

        let response = aggregate_results(
            "solve-test".to_string(),
            "request-test".to_string(),
            None,
            0,
            &problem,
            &SolveOptions::default(),
            2,
            2,
            0,
            0,
            vec![optimal, infeasible],
            false,
            true,
            &state,
            Vec::new(),
        );

        assert!(response.ok);
        assert_eq!(response.status, "optimal");
        assert_eq!(response.jobs_completed, 2);
        assert_eq!(response.jobs_expected, 2);
        assert_eq!(response.jobs_redelegated, 0);
        assert_eq!(response.jobs_split, 0);
        assert_eq!(response.z, Some(90.0));
    }

    #[test]
    fn redelegated_job_preserves_payload_and_advances_retry_id() {
        let mut job = test_job(binary_knapsack_problem());
        job.job_id = "solve-test-0".to_string();
        job.extra_constraints.push(BranchConstraint {
            coefs: vec![1.0, 0.0, 0.0, 0.0],
            rhs: 0.0,
            name: "branch_x0_le_0".to_string(),
        });

        let retry = redelegated_job(&job, 2);

        assert_eq!(retry.job_id, "solve-test-0-retry-2");
        assert_eq!(retry.solve_id, job.solve_id);
        assert_eq!(retry.request_id, job.request_id);
        assert_eq!(retry.revision, job.revision);
        assert_eq!(
            retry.problem.as_ref().map(|problem| &problem.c),
            job.problem.as_ref().map(|problem| &problem.c)
        );
        assert_eq!(retry.extra_constraints, job.extra_constraints);
        assert_ne!(retry.job_uuid, job.job_uuid);
        assert_eq!(retry.problem_id, job.problem_id);
        assert!(retry.submitted_at_ms >= job.submitted_at_ms);
    }

    #[test]
    fn aggregate_results_treats_redelegated_attempt_as_complete() {
        let problem = binary_knapsack_problem();
        let state = test_state(NodeRole::Master);
        let optimal_retry = SubproblemResult {
            solve_id: "solve-test".to_string(),
            request_id: "request-test".to_string(),
            job_id: "job-0-retry-1".to_string(),
            job_uuid: new_uuid_string(),
            problem_id: Some("11111111-1111-4111-8111-111111111111".to_string()),
            revision: 0,
            worker_node: "worker-b".to_string(),
            ok: true,
            status: "optimal".to_string(),
            z: Some(90.0),
            x: vec![0.0, 1.0, 0.0, 1.0],
            best_bound: Some(90.0),
            gap: Some(0.0),
            lp: None,
            child_jobs: Vec::new(),
            nodes_explored: 1,
            lp_solves: 1,
            elapsed_ms: 1.0,
            accelerator: AcceleratorReport::default(),
            error: None,
            finished_at_ms: now_ms(),
        };

        let response = aggregate_results(
            "solve-test".to_string(),
            "request-test".to_string(),
            None,
            0,
            &problem,
            &SolveOptions::default(),
            1,
            2,
            1,
            0,
            vec![optimal_retry],
            false,
            true,
            &state,
            Vec::new(),
        );

        assert!(response.ok);
        assert_eq!(response.status, "optimal");
        assert_eq!(response.jobs_expected, 1);
        assert_eq!(response.jobs_published, 2);
        assert_eq!(response.jobs_completed, 1);
        assert_eq!(response.jobs_redelegated, 1);
        assert_eq!(response.jobs_split, 0);
        assert_eq!(response.z, Some(90.0));
    }

    #[test]
    fn result_acceptance_ignores_duplicate_and_unknown_jobs() {
        let mut expected = HashSet::new();
        expected.insert("job-0".to_string());
        expected.insert("job-1".to_string());
        let mut completed = HashSet::new();
        let result = SubproblemResult {
            solve_id: "solve-test".to_string(),
            request_id: "request-test".to_string(),
            job_id: "job-0".to_string(),
            job_uuid: new_uuid_string(),
            problem_id: Some("11111111-1111-4111-8111-111111111111".to_string()),
            revision: 0,
            worker_node: "worker-a".to_string(),
            ok: true,
            status: "optimal".to_string(),
            z: Some(90.0),
            x: vec![0.0, 1.0],
            best_bound: Some(90.0),
            gap: Some(0.0),
            lp: None,
            child_jobs: Vec::new(),
            nodes_explored: 1,
            lp_solves: 1,
            elapsed_ms: 1.0,
            accelerator: AcceleratorReport::default(),
            error: None,
            finished_at_ms: now_ms(),
        };

        assert!(
            accept_subproblem_result(result.clone(), "solve-test", &expected, &mut completed)
                .unwrap()
                .is_some()
        );
        assert_eq!(completed.len(), 1);

        let duplicate =
            accept_subproblem_result(result.clone(), "solve-test", &expected, &mut completed)
                .unwrap_err();
        assert!(duplicate.contains("duplicate"));
        assert_eq!(completed.len(), 1);

        let mut unknown = result.clone();
        unknown.job_id = "job-missing".to_string();
        let warning =
            accept_subproblem_result(unknown, "solve-test", &expected, &mut completed).unwrap_err();
        assert!(warning.contains("unknown job"));
        assert_eq!(completed.len(), 1);

        let mut other_solve = result;
        other_solve.solve_id = "solve-other".to_string();
        assert!(
            accept_subproblem_result(other_solve, "solve-test", &expected, &mut completed)
                .unwrap()
                .is_none()
        );
        assert_eq!(completed.len(), 1);
    }
}
