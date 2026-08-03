use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

pub type JsonObject = BTreeMap<String, Value>;

fn default_queue() -> String {
    "default".to_string()
}

fn default_max_attempts() -> u32 {
    3
}

fn default_initial_backoff_ms() -> u64 {
    1_000
}

fn default_max_backoff_ms() -> u64 {
    60_000
}

fn default_backoff_multiplier() -> f64 {
    2.0
}

fn default_timeout_ms() -> u64 {
    15 * 60 * 1_000
}

fn default_lease_ms() -> u64 {
    60_000
}

fn default_worker_slots() -> u32 {
    1
}

fn default_worker_ttl_ms() -> u64 {
    45_000
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    Paused,
    Succeeded,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Blocked,
    WaitingTimer,
    WaitingSignal,
    Queued,
    Leased,
    Running,
    WaitingRetry,
    Succeeded,
    Failed,
    Cancelled,
}

impl StepStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Leased | Self::Running)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Online,
    Draining,
    Offline,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetryPolicy {
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_initial_backoff_ms")]
    pub initial_backoff_ms: u64,
    #[serde(default = "default_max_backoff_ms")]
    pub max_backoff_ms: u64,
    #[serde(default = "default_backoff_multiplier")]
    pub multiplier: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            initial_backoff_ms: default_initial_backoff_ms(),
            max_backoff_ms: default_max_backoff_ms(),
            multiplier: default_backoff_multiplier(),
        }
    }
}

impl RetryPolicy {
    pub fn backoff_ms(&self, completed_attempt: u32) -> u64 {
        let exponent = completed_attempt.saturating_sub(1) as i32;
        let scaled = (self.initial_backoff_ms as f64) * self.multiplier.powi(exponent);
        scaled.round().clamp(0.0, self.max_backoff_ms as f64) as u64
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConcurrencyPolicy {
    pub key: String,
    pub limit: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StepDefinition {
    pub key: String,
    pub task_type: String,
    #[serde(default = "default_queue")]
    pub queue: String,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub input: JsonObject,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub required_capabilities: BTreeSet<String>,
    #[serde(default)]
    pub retry: RetryPolicy,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_lease_ms")]
    pub lease_ms: u64,
    pub not_before_ms: Option<u64>,
    pub wait_for_signal: Option<String>,
    pub concurrency: Option<ConcurrencyPolicy>,
    pub affinity_key: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubmitRunRequest {
    pub idempotency_key: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub metadata: JsonObject,
    pub steps: Vec<StepDefinition>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubmitTaskRequest {
    pub idempotency_key: Option<String>,
    pub name: Option<String>,
    pub task_type: String,
    #[serde(default = "default_queue")]
    pub queue: String,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub input: JsonObject,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub metadata: JsonObject,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub required_capabilities: BTreeSet<String>,
    #[serde(default)]
    pub retry: RetryPolicy,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_lease_ms")]
    pub lease_ms: u64,
    pub not_before_ms: Option<u64>,
    pub wait_for_signal: Option<String>,
    pub concurrency: Option<ConcurrencyPolicy>,
    pub affinity_key: Option<String>,
}

impl SubmitTaskRequest {
    pub fn into_run(self) -> SubmitRunRequest {
        SubmitRunRequest {
            idempotency_key: self.idempotency_key,
            name: self.name,
            metadata: self.metadata,
            steps: vec![StepDefinition {
                key: "task".to_string(),
                task_type: self.task_type,
                queue: self.queue,
                input: self.input,
                depends_on: Vec::new(),
                priority: self.priority,
                required_capabilities: self.required_capabilities,
                retry: self.retry,
                timeout_ms: self.timeout_ms,
                lease_ms: self.lease_ms,
                not_before_ms: self.not_before_ms,
                wait_for_signal: self.wait_for_signal,
                concurrency: self.concurrency,
                affinity_key: self.affinity_key,
            }],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubmitRunResponse {
    pub run_id: String,
    pub status: RunStatus,
    pub idempotent_replay: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunCounts {
    pub total: u32,
    pub blocked: u32,
    pub queued: u32,
    pub active: u32,
    pub succeeded: u32,
    pub failed: u32,
    pub cancelled: u32,
}

impl RunCounts {
    pub fn empty(total: usize) -> Self {
        Self {
            total: total as u32,
            blocked: total as u32,
            queued: 0,
            active: 0,
            succeeded: 0,
            failed: 0,
            cancelled: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub id: String,
    pub name: String,
    pub status: RunStatus,
    #[schema(value_type = Object)]
    pub metadata: JsonObject,
    pub step_ids: BTreeMap<String, String>,
    pub counts: RunCounts,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub completed_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LeaseRecord {
    pub token: String,
    pub generation: u64,
    pub worker_id: String,
    pub acquired_at_ms: u64,
    pub expires_at_ms: u64,
    pub fencing_token: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FailureRecord {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub failed_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutputReceipt {
    pub chunk_id: String,
    pub payload_hash: String,
    pub sequence: u64,
    pub event_id: String,
    pub occurred_at_ms: u64,
    pub stream: String,
    pub final_chunk: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StepRecord {
    pub id: String,
    pub run_id: String,
    pub key: String,
    pub task_type: String,
    pub queue: String,
    #[schema(value_type = Object)]
    pub input: JsonObject,
    pub depends_on: Vec<String>,
    pub priority: i32,
    pub required_capabilities: BTreeSet<String>,
    pub retry: RetryPolicy,
    pub timeout_ms: u64,
    pub lease_ms: u64,
    pub not_before_ms: Option<u64>,
    pub wait_for_signal: Option<String>,
    pub concurrency: Option<ConcurrencyPolicy>,
    pub affinity_key: Option<String>,
    pub status: StepStatus,
    pub attempt: u32,
    pub lease_generation: u64,
    pub lease: Option<LeaseRecord>,
    #[serde(default)]
    pub last_lease: Option<LeaseRecord>,
    #[schema(value_type = Object)]
    pub result: JsonObject,
    pub failure: Option<FailureRecord>,
    pub output_sequence: u64,
    #[serde(default)]
    pub last_output: Option<OutputReceipt>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub started_at_ms: Option<u64>,
    pub completed_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunSnapshot {
    pub run: RunRecord,
    pub steps: Vec<StepRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRegistration {
    pub worker_id: String,
    #[serde(default)]
    pub queues: BTreeSet<String>,
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub labels: JsonObject,
    #[serde(default = "default_worker_slots")]
    pub slots: u32,
    #[serde(default = "default_worker_ttl_ms")]
    pub ttl_ms: u64,
    pub drain: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRecord {
    pub worker_id: String,
    pub queues: BTreeSet<String>,
    pub capabilities: BTreeSet<String>,
    #[schema(value_type = Object)]
    pub labels: JsonObject,
    pub slots: u32,
    pub ttl_ms: u64,
    pub status: WorkerStatus,
    pub registered_at_ms: u64,
    pub last_heartbeat_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkerHeartbeatRequest {
    pub drain: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PollResponse {
    pub assignment: Option<StepAssignment>,
    pub retry_after_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StepAssignment {
    pub run_id: String,
    pub step_id: String,
    pub step_key: String,
    pub task_type: String,
    pub queue: String,
    #[schema(value_type = Object)]
    pub input: JsonObject,
    pub attempt: u32,
    pub lease_token: String,
    pub lease_generation: u64,
    pub fencing_token: u64,
    pub lease_expires_at_ms: u64,
    pub timeout_ms: u64,
    pub affinity_key: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LeaseCommand {
    pub worker_id: String,
    pub lease_token: String,
    pub lease_generation: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompleteStepRequest {
    pub worker_id: String,
    pub lease_token: String,
    pub lease_generation: u64,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub result: JsonObject,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FailStepRequest {
    pub worker_id: String,
    pub lease_token: String,
    pub lease_generation: u64,
    pub code: String,
    pub message: String,
    #[serde(default = "default_true")]
    pub retryable: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StepOutputRequest {
    pub chunk_id: String,
    pub worker_id: String,
    pub lease_token: String,
    pub lease_generation: u64,
    pub stream: Option<String>,
    pub chunk: String,
    pub final_chunk: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignalRequest {
    #[serde(default)]
    #[schema(value_type = Object)]
    pub payload: JsonObject,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignalResponse {
    pub run_id: String,
    pub signal_name: String,
    pub released_steps: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MutationResponse {
    pub ok: bool,
    pub run_id: Option<String>,
    pub step_id: Option<String>,
    pub status: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DurableEvent {
    pub schema_version: String,
    pub event_id: String,
    pub event_type: String,
    pub run_id: String,
    pub step_id: Option<String>,
    pub worker_id: Option<String>,
    pub occurred_at_ms: u64,
    #[schema(value_type = Object)]
    pub data: JsonObject,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct PollQuery {
    pub wait_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdempotencyRecord {
    pub run_id: String,
    pub request_hash: String,
    pub created_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneHolder {
    pub lease_token: String,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneRecord {
    pub limit: u32,
    pub holders: BTreeMap<String, LaneHolder>,
    pub updated_at_ms: u64,
}
