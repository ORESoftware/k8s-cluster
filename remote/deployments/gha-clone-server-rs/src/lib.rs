//! Public continuity-planner API.
//!
//! The generic planner remains preserved byte-for-byte in `lib_original.rs`.
//! This wrapper reserves one privileged private-repository workflow identity
//! before returning an independently executable plan. A reserved identity
//! mismatch is terminal and never falls back to generic Node classification.

#[cfg(not(test))]
mod msgint_contract;

#[cfg(test)]
mod msgint_contract {
    use std::collections::BTreeMap;

    use serde_yaml::Mapping;

    #[allow(dead_code)]
    pub enum ContractMatch {
        NotApplicable,
        Match(BTreeMap<String, String>),
        Reject(Vec<String>),
    }

    pub fn classify_msgint_workflow(
        _repository: &str,
        _revision: &str,
        _workflow_path: &str,
        _root: &Mapping,
    ) -> ContractMatch {
        // The real module is compiled by every normal library/binary build and
        // therefore by the process-level integration tests. Keeping the
        // original planner's unit-test module isolated preserves its existing
        // generic-classifier coverage without running the same private
        // contract fixtures twice.
        ContractMatch::NotApplicable
    }
}

mod original {
    include!("lib_original.rs");
}

pub use original::{
    capabilities, is_full_commit_sha, verify_github_signature, ArchitectureCapabilities,
    CapabilityLimits, CapabilityResponse, JobPlan, PlanRequest, PlannerLimits, WorkflowPlan,
    MAX_JOBS_DEFAULT, MAX_STEPS_PER_JOB_DEFAULT, MAX_WORKFLOW_BYTES_DEFAULT,
    PLAN_SCHEMA_VERSION, SERVICE_NAME,
};

/// Build a generic plan, then apply the exact reserved Messaging Intel
/// repository/revision/workflow contract before privileged fixed profiles can
/// be returned.
pub fn build_plan(
    request: &PlanRequest,
    limits: &PlannerLimits,
) -> Result<WorkflowPlan, Vec<String>> {
    let mut plan = original::build_plan(request, limits)?;
    let workflow: serde_yaml::Value = serde_yaml::from_str(&request.workflow_yaml)
        .map_err(|error| vec![format!("workflowYaml is not valid YAML: {error}")])?;
    let root = workflow
        .as_mapping()
        .ok_or_else(|| vec!["workflow document must be a YAML mapping".to_string()])?;

    let profiles = match msgint_contract::classify_msgint_workflow(
        &request.repository,
        &request.revision,
        &request.workflow_path,
        root,
    ) {
        msgint_contract::ContractMatch::NotApplicable => return Ok(plan),
        msgint_contract::ContractMatch::Reject(reasons) => return Err(reasons),
        msgint_contract::ContractMatch::Match(profiles) => profiles,
    };

    if plan.jobs.len() != profiles.len() {
        return Err(vec![
            "reviewed Messaging Intel contract and generic planner produced different job sets"
                .to_string(),
        ]);
    }

    for job in &mut plan.jobs {
        let Some(profile) = profiles.get(&job.id) else {
            return Err(vec![format!(
                "reviewed Messaging Intel contract did not authorize job {:?}",
                job.id
            )]);
        };
        if !job.independent_reasons.is_empty() {
            return Err(vec![format!(
                "jobs.{}: reviewed Messaging Intel contract matched but generic safety checks rejected the job: {}",
                job.id,
                job.independent_reasons.join("; ")
            )]);
        }
        job.independent_supported = true;
        job.independent_profile = Some(profile.clone());
    }
    plan.independent_executable =
        plan.immutable_revision && plan.jobs.iter().all(|job| job.independent_supported);
    Ok(plan)
}
