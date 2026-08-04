use std::collections::BTreeMap;

use serde_yaml::{Mapping, Value};

#[path = "lib.rs"]
mod legacy;

pub use legacy::{
    is_full_commit_sha, verify_github_signature, ArchitectureCapabilities, CapabilityLimits,
    CapabilityResponse, JobPlan, PlanRequest, PlannerLimits, WorkflowPlan, MAX_JOBS_DEFAULT,
    MAX_STEPS_PER_JOB_DEFAULT, MAX_WORKFLOW_BYTES_DEFAULT, PLAN_SCHEMA_VERSION, SERVICE_NAME,
};

const THREEFA_REPOSITORY: &str = "3FA-app/3fa-interfaces";
const THREEFA_WORKFLOW_PATH: &str = ".github/workflows/gha-clone-contracts.yml";
const GENERATED_RUST_PROFILE: &str = "rust-generated-verify";
const NODE_HARDENED_VERIFY_PROFILE: &str = "node-hardened-verify";
const NODE_HARDENED_TEST_PROFILE: &str = "node-hardened-test";

const CHECKOUT_ACTION: &str = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1";
const NODE_SETUP_ACTION: &str = "actions/setup-node@820762786026740c76f36085b0efc47a31fe5020";
const RUST_TOOLCHAIN_ACTION: &str =
    "dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30";
const NODE_ACTIONS: [&str; 2] = [CHECKOUT_ACTION, NODE_SETUP_ACTION];
const GENERATED_RUST_ACTIONS: [&str; 2] = [CHECKOUT_ACTION, RUST_TOOLCHAIN_ACTION];

const GENERATED_RUST_COMMANDS: [&str; 4] = [
    "cargo generate-lockfile --manifest-path generated/rust/Cargo.toml",
    "cargo fmt --manifest-path generated/rust/Cargo.toml -- --check",
    "cargo clippy --locked --manifest-path generated/rust/Cargo.toml --all-targets -- -D warnings",
    "cargo test --locked --manifest-path generated/rust/Cargo.toml --all-targets",
];

const NODE_HARDENED_VERIFY_COMMANDS: [&str; 4] = [
    "npm ci --ignore-scripts",
    "npm run check",
    "npm run test:operator-config",
    "npm audit --audit-level=high",
];

const NODE_HARDENED_TEST_COMMANDS: [&str; 2] = ["npm ci --ignore-scripts", "npm test"];

#[derive(Default)]
struct JobSteps {
    run_commands: Vec<String>,
    actions: Vec<String>,
}

pub fn capabilities(limits: &PlannerLimits) -> CapabilityResponse {
    let mut response = legacy::capabilities(limits);
    let mut profiles = Vec::with_capacity(response.independent_profiles.len() + 3);
    for profile in response.independent_profiles {
        let is_rust = profile == "rust-verify";
        let is_node = profile == "node-verify";
        profiles.push(profile);
        if is_rust {
            profiles.push(GENERATED_RUST_PROFILE.to_string());
        } else if is_node {
            profiles.push(NODE_HARDENED_VERIFY_PROFILE.to_string());
            profiles.push(NODE_HARDENED_TEST_PROFILE.to_string());
        }
    }
    response.independent_profiles = profiles;
    response
}

pub fn build_plan(
    request: &PlanRequest,
    limits: &PlannerLimits,
) -> Result<WorkflowPlan, Vec<String>> {
    let mut plan = legacy::build_plan(request, limits)?;
    if !is_threefa_bounded_workflow(request) {
        return Ok(plan);
    }

    let steps_by_job = workflow_job_steps(&request.workflow_yaml)?;
    for job in &mut plan.jobs {
        let Some(steps) = steps_by_job.get(&job.id) else {
            continue;
        };
        let lower = steps.run_commands.join("\n").to_ascii_lowercase();

        if generated_rust_intent(&lower) {
            enforce_exact_actions(job, &steps.actions, &GENERATED_RUST_ACTIONS);
            apply_exact_profile(
                job,
                generated_rust_profile(&steps.run_commands),
                "generated Rust jobs must use one exact reviewed command sequence in the documented order with no extra commands",
            );
        } else if hardened_node_intent(&lower) {
            enforce_exact_actions(job, &steps.actions, &NODE_ACTIONS);
            apply_exact_profile(
                job,
                hardened_node_profile(&steps.run_commands),
                "hardened Node jobs must use one exact reviewed command sequence in the documented order with no extra commands",
            );
        }
    }

    plan.independent_executable =
        plan.immutable_revision && plan.jobs.iter().all(|job| job.independent_supported);
    Ok(plan)
}

fn is_threefa_bounded_workflow(request: &PlanRequest) -> bool {
    request.repository == THREEFA_REPOSITORY && request.workflow_path == THREEFA_WORKFLOW_PATH
}

fn enforce_exact_actions(job: &mut JobPlan, actual: &[String], expected: &[&str]) {
    if !actual
        .iter()
        .map(String::as_str)
        .eq(expected.iter().copied())
    {
        job.independent_supported = false;
        job.independent_profile = None;
        job.independent_reasons.push(
            "3FA setup actions must match the exact reviewed pinned action sequence with no extra actions"
                .to_string(),
        );
    }
}

fn apply_exact_profile(job: &mut JobPlan, profile: Option<&'static str>, rejection: &str) {
    match profile {
        Some(profile) if job.independent_reasons.is_empty() => {
            job.independent_supported = true;
            job.independent_profile = Some(profile.to_string());
        }
        Some(_) => {
            job.independent_supported = false;
            job.independent_profile = None;
        }
        None => {
            job.independent_supported = false;
            job.independent_profile = None;
            job.independent_reasons.push(rejection.to_string());
        }
    }
}

fn workflow_job_steps(source: &str) -> Result<BTreeMap<String, JobSteps>, Vec<String>> {
    let workflow: Value = serde_yaml::from_str(source)
        .map_err(|error| vec![format!("workflowYaml is not valid YAML: {error}")])?;
    let root = workflow
        .as_mapping()
        .ok_or_else(|| vec!["workflow document must be a YAML mapping".to_string()])?;
    let jobs = mapping_get(root, "jobs")
        .and_then(Value::as_mapping)
        .ok_or_else(|| vec!["workflow.jobs must be a mapping".to_string()])?;

    let mut jobs_by_id = BTreeMap::new();
    for (job_id, job_value) in jobs {
        let Some(job_id) = job_id.as_str() else {
            continue;
        };
        let Some(job) = job_value.as_mapping() else {
            continue;
        };
        let Some(steps) = mapping_get(job, "steps").and_then(Value::as_sequence) else {
            continue;
        };

        let mut job_steps = JobSteps::default();
        for step in steps {
            let Some(step) = step.as_mapping() else {
                continue;
            };
            if let Some(action) = mapping_get(step, "uses").and_then(Value::as_str) {
                job_steps.actions.push(action.trim().to_string());
            }
            if let Some(run) = mapping_get(step, "run").and_then(Value::as_str) {
                job_steps.run_commands.extend(
                    run.lines()
                        .map(str::trim)
                        .filter(|command| !command.is_empty())
                        .map(str::to_string),
                );
            }
        }
        jobs_by_id.insert(job_id.to_string(), job_steps);
    }
    Ok(jobs_by_id)
}

fn mapping_get<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    mapping.get(Value::String(key.to_string()))
}

fn generated_rust_intent(text: &str) -> bool {
    text.contains("generated/rust/cargo.toml")
}

fn generated_rust_profile(commands: &[String]) -> Option<&'static str> {
    commands
        .iter()
        .map(String::as_str)
        .eq(GENERATED_RUST_COMMANDS)
        .then_some(GENERATED_RUST_PROFILE)
}

fn hardened_node_intent(text: &str) -> bool {
    text.contains("npm ci --ignore-scripts") || text.contains("npm run test:operator-config")
}

fn hardened_node_profile(commands: &[String]) -> Option<&'static str> {
    if commands
        .iter()
        .map(String::as_str)
        .eq(NODE_HARDENED_VERIFY_COMMANDS)
    {
        Some(NODE_HARDENED_VERIFY_PROFILE)
    } else if commands
        .iter()
        .map(String::as_str)
        .eq(NODE_HARDENED_TEST_COMMANDS)
    {
        Some(NODE_HARDENED_TEST_PROFILE)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_include_hardened_profiles_once() {
        let profiles = capabilities(&PlannerLimits::default()).independent_profiles;
        for expected in [
            GENERATED_RUST_PROFILE,
            NODE_HARDENED_VERIFY_PROFILE,
            NODE_HARDENED_TEST_PROFILE,
        ] {
            assert_eq!(
                profiles
                    .iter()
                    .filter(|profile| profile.as_str() == expected)
                    .count(),
                1,
                "{expected} must appear exactly once"
            );
        }
    }

    #[test]
    fn exact_command_sequences_do_not_accept_extensions_or_reordering() {
        let node = NODE_HARDENED_TEST_COMMANDS
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert_eq!(
            hardened_node_profile(&node),
            Some(NODE_HARDENED_TEST_PROFILE)
        );
        let mut extra_node = node.clone();
        extra_node.push("npm audit --audit-level=high".to_string());
        assert_eq!(hardened_node_profile(&extra_node), None);

        let generated = GENERATED_RUST_COMMANDS
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert_eq!(
            generated_rust_profile(&generated),
            Some(GENERATED_RUST_PROFILE)
        );
        let mut reordered = generated.clone();
        reordered.swap(2, 3);
        assert_eq!(generated_rust_profile(&reordered), None);
    }

    #[test]
    fn exact_action_sequences_reject_mutable_and_extra_actions() {
        let node = NODE_ACTIONS
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert!(node.iter().map(String::as_str).eq(NODE_ACTIONS));

        let mutable = [CHECKOUT_ACTION, "actions/setup-node@main"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert!(!mutable.iter().map(String::as_str).eq(NODE_ACTIONS));

        let extra = [
            CHECKOUT_ACTION,
            NODE_SETUP_ACTION,
            "owner/extra@0123456789abcdef0123456789abcdef01234567",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        assert!(!extra.iter().map(String::as_str).eq(NODE_ACTIONS));
    }

    #[test]
    fn exact_command_enforcement_is_scoped_to_the_registered_threefa_workflow() {
        let exact = PlanRequest {
            repository: THREEFA_REPOSITORY.to_string(),
            revision: "0123456789abcdef0123456789abcdef01234567".to_string(),
            workflow_path: THREEFA_WORKFLOW_PATH.to_string(),
            workflow_yaml: "jobs: {}".to_string(),
        };
        assert!(is_threefa_bounded_workflow(&exact));

        let mut sibling = exact.clone();
        sibling.repository = "StreemPilot/streempilot-interfaces".to_string();
        assert!(!is_threefa_bounded_workflow(&sibling));

        let mut other_path = exact;
        other_path.workflow_path = ".github/workflows/ci.yml".to_string();
        assert!(!is_threefa_bounded_workflow(&other_path));
    }
}
