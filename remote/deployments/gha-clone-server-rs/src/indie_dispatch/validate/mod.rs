mod identifiers;
mod metadata;
mod repository;

use super::model::DispatchRequest;
use super::DISPATCH_SCHEMA;
use identifiers::{
    validate_base_job_id, validate_commit_sha, validate_digest, validate_instance_id,
    validate_profile_name, validate_request_id,
};
use metadata::{validate_dependencies, validate_matrix};
use repository::{validate_context_dir, validate_repository_url};

const MAX_JOB_ORDER_INDEX: usize = 255;
const MAX_PARALLEL: usize = 1_024;

pub(super) fn validate_request(request: &DispatchRequest) -> Result<(), String> {
    if request.schema_version != DISPATCH_SCHEMA {
        return Err(format!("schemaVersion must be {DISPATCH_SCHEMA}"));
    }
    validate_request_id(&request.request_id)?;
    validate_digest("requestDigest", &request.request_digest)?;
    validate_digest("planDigest", &request.plan_digest)?;
    validate_digest("profileCatalogDigest", &request.profile_catalog_digest)?;
    validate_repository_url(&request.repository_url)?;
    validate_commit_sha(&request.commit_sha)?;
    validate_instance_id("jobInstanceId", &request.job_instance_id)?;
    validate_base_job_id("baseJobId", &request.base_job_id)?;
    if request.job_order_index > MAX_JOB_ORDER_INDEX {
        return Err(format!(
            "jobOrderIndex must be at most {MAX_JOB_ORDER_INDEX}"
        ));
    }
    validate_profile_name(&request.profile)?;
    validate_digest("profileDigest", &request.profile_digest)?;
    validate_context_dir(&request.context_dir)?;
    validate_dependencies(&request.job_instance_id, &request.needs_instances)?;
    validate_matrix(&request.matrix)?;
    if matches!(request.max_parallel, Some(0))
        || request
            .max_parallel
            .is_some_and(|value| value > MAX_PARALLEL)
    {
        return Err(format!("maxParallel must be between 1 and {MAX_PARALLEL}"));
    }
    Ok(())
}
