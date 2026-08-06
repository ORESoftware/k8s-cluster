use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DispatchRequest {
    pub(super) schema_version: String,
    pub(super) request_id: String,
    pub(super) request_digest: String,
    pub(super) plan_digest: String,
    pub(super) profile_catalog_digest: String,
    pub(super) repository_url: String,
    pub(super) commit_sha: String,
    pub(super) job_instance_id: String,
    pub(super) base_job_id: String,
    pub(super) job_order_index: usize,
    pub(super) profile: String,
    pub(super) profile_digest: String,
    pub(super) context_dir: String,
    pub(super) needs_instances: Vec<String>,
    pub(super) matrix: BTreeMap<String, Value>,
    pub(super) fail_fast: bool,
    pub(super) max_parallel: Option<usize>,
}
