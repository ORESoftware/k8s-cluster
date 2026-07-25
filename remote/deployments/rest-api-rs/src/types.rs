use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
pub(crate) struct HealthResponse {
    pub(crate) ok: bool,
    pub(crate) service: String,
    pub(crate) mode: String,
}

#[derive(Deserialize)]
pub(crate) struct AgentsQuery {
    pub(crate) limit: Option<i64>,
}

#[derive(Deserialize)]
pub(crate) struct ContextQuery {
    pub(crate) limit: Option<i64>,
}

#[derive(Deserialize)]
pub(crate) struct LambdasQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) search: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentsSnapshot {
    pub(crate) ok: bool,
    pub(crate) source: String,
    pub(crate) generated_at_ms: u128,
    pub(crate) config: AgentsDataConfig,
    pub(crate) summary: AgentsSummary,
    pub(crate) threads: Vec<AgentThreadRow>,
    pub(crate) tasks: Vec<AgentTaskRow>,
    pub(crate) errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadContextResponse {
    pub(crate) ok: bool,
    pub(crate) source: String,
    pub(crate) thread_id: String,
    pub(crate) generated_at_ms: u128,
    pub(crate) tasks: Vec<AgentTaskRow>,
    pub(crate) errors: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentContextCandidate {
    pub(crate) context_id: String,
    pub(crate) project_id: String,
    pub(crate) repo_id: Option<String>,
    pub(crate) context_title: String,
    pub(crate) context_blob: String,
    pub(crate) score: f64,
    pub(crate) match_source: String,
    pub(crate) embedding_model: Option<String>,
    pub(crate) updated_at: Option<String>,
    /// Discriminator for the picker so breadcrumbs and context blobs can ride
    /// the same `contextIds` / `contextBlobs` rails without the worker having
    /// to guess. `"context-blob"` is the legacy default; `"breadcrumb"` rows
    /// carry a serialized AgentBreadcrumbRow JSON in `context_blob`.
    pub(crate) kind: String,
}

pub(crate) const CONTEXT_KIND_BLOB: &str = "context-blob";
pub(crate) const CONTEXT_KIND_BREADCRUMB: &str = "breadcrumb";
pub(crate) const CONTEXT_KIND_TASK: &str = "thread-task";
pub(crate) const BREADCRUMB_CANDIDATE_PREFIX: &str = "breadcrumb:";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentContextCandidatesResponse {
    pub(crate) ok: bool,
    pub(crate) source: String,
    pub(crate) thread_id: String,
    pub(crate) generated_at_ms: u128,
    pub(crate) project_id: String,
    pub(crate) repo_id: Option<String>,
    pub(crate) candidates: Vec<AgentContextCandidate>,
    pub(crate) errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KnownGitReposResponse {
    pub(crate) ok: bool,
    pub(crate) source: String,
    pub(crate) generated_at_ms: u128,
    pub(crate) repos: Vec<KnownGitRepoRow>,
    pub(crate) errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentsDataConfig {
    pub(crate) rds_configured: bool,
    pub(crate) postgres_configured: bool,
    pub(crate) supabase_configured: bool,
    pub(crate) nats_configured: bool,
    pub(crate) nats_url: String,
    pub(crate) postgres_plan: String,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentsSummary {
    pub(crate) thread_count: usize,
    pub(crate) task_count: usize,
    pub(crate) running_count: usize,
    pub(crate) failed_count: usize,
    pub(crate) done_count: usize,
    pub(crate) pr_count: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentThreadRow {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) repo: String,
    pub(crate) base_branch: String,
    pub(crate) archived_at: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) task_count: i64,
    pub(crate) active_task_count: i64,
    pub(crate) latest_task_at: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KnownGitRepoRow {
    pub(crate) id: String,
    pub(crate) repo_url: String,
    pub(crate) display_name: String,
    pub(crate) provider: String,
    pub(crate) default_branch: String,
    pub(crate) status: String,
    pub(crate) last_verified_at: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentTaskRow {
    pub(crate) id: String,
    pub(crate) thread_id: String,
    pub(crate) thread_title: Option<String>,
    pub(crate) prompt: String,
    pub(crate) status: String,
    pub(crate) branch: Option<String>,
    pub(crate) pr_url: Option<String>,
    pub(crate) pr_state: Option<String>,
    pub(crate) exit_reason: Option<String>,
    pub(crate) error_message: Option<String>,
    pub(crate) started_at: Option<String>,
    pub(crate) finished_at: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) last_event_seq: i32,
    pub(crate) event_count: i64,
    pub(crate) latest_event_kind: Option<String>,
    pub(crate) latest_payload: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentEventRow {
    pub(crate) task_id: String,
    pub(crate) seq: i32,
    pub(crate) event_kind: String,
    pub(crate) payload: Value,
    pub(crate) created_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentTaskEventsResponse {
    pub(crate) ok: bool,
    pub(crate) source: String,
    pub(crate) task_id: String,
    pub(crate) generated_at_ms: u128,
    pub(crate) events: Vec<AgentEventRow>,
    pub(crate) errors: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LambdaFunctionRow {
    pub(crate) id: String,
    pub(crate) slug: String,
    pub(crate) display_name: String,
    pub(crate) description: String,
    pub(crate) runtime: String,
    pub(crate) entry_command: String,
    pub(crate) function_body: String,
    pub(crate) reuse_key: Option<String>,
    pub(crate) idle_timeout_seconds: i32,
    pub(crate) max_run_ms: i32,
    pub(crate) containerized: bool,
    pub(crate) container_image: Option<String>,
    pub(crate) container_build_status: String,
    pub(crate) container_build_error: Option<String>,
    pub(crate) container_built_at: Option<String>,
    pub(crate) status: String,
    pub(crate) labels: Value,
    pub(crate) meta_data: Value,
    pub(crate) last_invoked_at: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LambdaFunctionsResponse {
    pub(crate) ok: bool,
    pub(crate) source: String,
    pub(crate) generated_at_ms: u128,
    pub(crate) functions: Vec<LambdaFunctionRow>,
    pub(crate) errors: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LambdaFunctionSaveRequest {
    pub(crate) slug: String,
    pub(crate) display_name: String,
    pub(crate) description: Option<String>,
    pub(crate) runtime: Option<String>,
    pub(crate) entry_command: Option<String>,
    pub(crate) function_body: String,
    pub(crate) reuse_key: Option<String>,
    pub(crate) idle_timeout_seconds: Option<i32>,
    pub(crate) max_run_ms: Option<i32>,
    pub(crate) containerized: Option<bool>,
    pub(crate) status: Option<String>,
    pub(crate) labels: Option<Value>,
    pub(crate) meta_data: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadActionResult {
    pub(crate) resource: String,
    pub(crate) status: u16,
    pub(crate) ok: bool,
    pub(crate) body: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadActionResponse {
    pub(crate) ok: bool,
    pub(crate) action: String,
    pub(crate) thread_id: String,
    pub(crate) k8s_name: String,
    pub(crate) namespace: String,
    pub(crate) results: Vec<ThreadActionResult>,
    pub(crate) errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadRuntimeResponse {
    pub(crate) ok: bool,
    pub(crate) source: String,
    pub(crate) thread_id: String,
    pub(crate) namespace: String,
    pub(crate) k8s_name: String,
    pub(crate) generated_at_ms: u128,
    pub(crate) summary: Value,
    pub(crate) deployment: Option<Value>,
    pub(crate) service: Option<Value>,
    pub(crate) pods: Vec<Value>,
    pub(crate) errors: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DispatchTaskRequest {
    pub(crate) task_id: String,
    pub(crate) thread_id: String,
    pub(crate) repo: String,
    pub(crate) base_branch: Option<String>,
    pub(crate) prompt: String,
    pub(crate) provider: Option<String>,
    pub(crate) thread_title: Option<String>,
    pub(crate) dispatch_mode: Option<String>,
    pub(crate) context_mode: Option<String>,
    pub(crate) context_ids: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentContextCandidatesRequest {
    pub(crate) prompt: String,
    pub(crate) repo: String,
    pub(crate) base_branch: Option<String>,
    pub(crate) project_id: Option<String>,
    pub(crate) limit: Option<i64>,
    /// When set, the candidates endpoint returns only the matching items
    /// resolved against blob/task/breadcrumb tables. Used by dev-server's
    /// `fetchSelectedContextBlobs` to refetch full payloads it received only
    /// as IDs.
    #[serde(default)]
    pub(crate) context_ids: Option<Vec<String>>,
}

pub(crate) struct ExistingTaskDispatch {
    pub(crate) thread_id: String,
    pub(crate) prompt: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KnownGitRepoRequest {
    pub(crate) repo_url: String,
    pub(crate) display_name: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) default_branch: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentEventIngestRequest {
    pub(crate) task_id: String,
    pub(crate) thread_id: Option<String>,
    pub(crate) seq: i32,
    pub(crate) event: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentBreadcrumbIngestRequest {
    pub(crate) thread_id: String,
    pub(crate) task_id: Option<String>,
    pub(crate) kind: String,
    pub(crate) payload: Option<Value>,
    pub(crate) pod_name: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) provider: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentBreadcrumbRow {
    pub(crate) id: i64,
    pub(crate) thread_id: String,
    pub(crate) task_id: Option<String>,
    pub(crate) kind: String,
    pub(crate) payload: Value,
    pub(crate) emitted_at: String,
    pub(crate) pod_name: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) provider: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentBreadcrumbTailResponse {
    pub(crate) thread_id: String,
    pub(crate) items: Vec<AgentBreadcrumbRow>,
    pub(crate) source: &'static str,
    pub(crate) excluded_task_id: Option<String>,
    pub(crate) limit: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentFeedbackRequest {
    pub(crate) target_seq: Option<i32>,
    pub(crate) vote: String,
    pub(crate) note: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadControlRequest {
    pub(crate) kind: String,
    pub(crate) action: String,
    pub(crate) thread_id: String,
    pub(crate) task_id: Option<String>,
    pub(crate) requested_by: Option<String>,
    pub(crate) reason: Option<String>,
}

#[derive(Clone)]
pub(crate) struct ThreadRepoConfig {
    pub(crate) repo: String,
    pub(crate) base_branch: String,
    pub(crate) thread_title: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NatsLambdaFunctionMessage {
    pub(crate) version: u8,
    pub(crate) message_kind: &'static str,
    pub(crate) action: String,
    pub(crate) function_id: String,
    pub(crate) slug: String,
    pub(crate) status: String,
    pub(crate) updated_at_ms: u128,
}
