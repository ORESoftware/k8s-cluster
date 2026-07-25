use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BuildRequest {
    pub(crate) schema_version: Option<String>,
    pub(crate) job_kind: Option<String>,
    pub(crate) repo_url: String,
    pub(crate) git_ref: Option<String>,
    #[serde(default)]
    pub(crate) image: String,
    /// Fixed operator-reviewed command pipeline for jobKind=run-profile.
    pub(crate) profile: Option<String>,
    pub(crate) context_dir: Option<String>,
    pub(crate) dockerfile: Option<String>,
    pub(crate) build_args: Option<BTreeMap<String, String>>,
    pub(crate) push: Option<bool>,
    pub(crate) deploy: Option<DeployRequest>,
    /// "local" (default: git + nerdctl + kubectl on this node) or "lambda"
    /// (forward to the gleam-lambda-runner build function).
    pub(crate) executor: Option<String>,
    /// Caller-supplied idempotency id for at-least-once transports
    /// (NATS/webhooks); duplicate ids are accepted-and-ignored.
    pub(crate) request_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeployRequest {
    pub(crate) kind: String,
    pub(crate) path: String,
    pub(crate) namespace: Option<String>,
    pub(crate) rollout: Option<String>,
    pub(crate) rollout_timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BuildJobRecord {
    pub(crate) id: String,
    pub(crate) status: BuildStatus,
    pub(crate) request: BuildRequest,
    /// Where the job came from: http | webhook | nats.
    pub(crate) source: String,
    /// Which executor runs it: local | lambda.
    pub(crate) executor: String,
    pub(crate) created_at_ms: u128,
    pub(crate) started_at_ms: Option<u128>,
    pub(crate) finished_at_ms: Option<u128>,
    pub(crate) log_path: String,
    pub(crate) error: Option<String>,
    /// fiducia.cloud lock key + fencing token, when coordination is enabled.
    pub(crate) lock_key: Option<String>,
    pub(crate) fencing_token: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum BuildStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HealthResponse {
    pub(crate) ok: bool,
    pub(crate) service: &'static str,
    pub(crate) auth_configured: bool,
    pub(crate) deploy_enabled: bool,
    pub(crate) push_enabled: bool,
    pub(crate) ecr_login_enabled: bool,
    pub(crate) allowed_repo_prefixes: Vec<String>,
    pub(crate) allowed_image_prefixes: Vec<String>,
    pub(crate) allowed_namespaces: Vec<String>,
    pub(crate) allowed_profiles: Vec<String>,
    pub(crate) allowed_profile_repo_prefixes: Vec<String>,
    pub(crate) queued: usize,
    pub(crate) running: u64,
}

/// Failure classes for the NATS intake path (drives ack vs term vs nak).
pub enum NatsSubmitError {
    /// Permanently invalid — the message can never succeed (bad JSON/schema).
    Invalid(String),
    /// Transient — queue full or a dependency is down; redeliver later.
    Transient(String),
}
