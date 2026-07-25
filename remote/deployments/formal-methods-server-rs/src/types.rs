use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// HTTP request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnalyzeRequest {
    pub(crate) schema_version: Option<String>,
    pub(crate) repo_url: Option<String>,
    pub(crate) git_ref: Option<String>,
    pub(crate) paths: Option<Vec<String>>,
    pub(crate) languages: Option<Vec<String>>,
    pub(crate) inline_source: Option<String>,
    pub(crate) inline_filename: Option<String>,
    pub(crate) heuristics: Option<bool>,
    pub(crate) pull_request: Option<PullRequestRef>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PullRequestRef {
    pub(crate) owner: String,
    pub(crate) repo: String,
    pub(crate) number: u64,
    pub(crate) head_sha: String,
    pub(crate) base_sha: String,
    pub(crate) head_clone_url: String,
    #[serde(default)]
    pub(crate) head_ref: Option<String>,
    #[serde(default)]
    pub(crate) base_ref: Option<String>,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) html_url: Option<String>,
    #[serde(default)]
    pub(crate) sender: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JobRecord {
    pub(crate) id: String,
    pub(crate) status: JobStatus,
    pub(crate) request: AnalyzeRequest,
    pub(crate) created_at_ms: u128,
    pub(crate) started_at_ms: Option<u128>,
    pub(crate) finished_at_ms: Option<u128>,
    pub(crate) log_path: String,
    pub(crate) error: Option<String>,
    pub(crate) findings_count: usize,
    pub(crate) findings: Vec<Finding>,
    pub(crate) files_scanned: usize,
    pub(crate) z3_queries: u64,
    pub(crate) pull_request: Option<PullRequestRef>,
    pub(crate) changed_paths: Option<Vec<String>>,
    pub(crate) pr_comment_status: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub(crate) enum FindingKind {
    PostconditionViolation,
    AssertionViolation,
    UnsatisfiablePrecondition,
    LoopInvariantNotEstablished,
    LoopInvariantNotPreserved,
    LoopVariantNotDecreasing,
    TautologyAlwaysTrue,
    TautologyAlwaysFalse,
    DeadNestedBranch,
    UnsupportedExpression,
    SolverUnknown,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Finding {
    pub(crate) kind: FindingKind,
    pub(crate) severity: Severity,
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) end_line: usize,
    pub(crate) message: String,
    pub(crate) detail: Option<String>,
    pub(crate) goal: Option<String>,
    pub(crate) counterexample: Option<BTreeMap<String, String>>,
    pub(crate) smt_query: Option<String>,
    pub(crate) solver_status: Option<String>,
    pub(crate) reasoning: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HealthResponse {
    pub(crate) ok: bool,
    pub(crate) service: &'static str,
    pub(crate) schema_version: &'static str,
    pub(crate) auth_configured: bool,
    pub(crate) z3_available: bool,
    pub(crate) github_webhook_configured: bool,
    pub(crate) github_comments_enabled: bool,
    pub(crate) pr_diff_only: bool,
    pub(crate) allowed_repo_prefixes: Vec<String>,
    pub(crate) allowed_extensions: Vec<String>,
    pub(crate) queued: usize,
    pub(crate) running: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ValidateRequest {
    pub(crate) schema_version: Option<String>,
    pub(crate) source: String,
    pub(crate) filename: Option<String>,
    pub(crate) heuristics: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ValidateResponse {
    pub(crate) schema_version: &'static str,
    pub(crate) findings_count: usize,
    pub(crate) findings: Vec<Finding>,
    pub(crate) z3_queries: u64,
}
