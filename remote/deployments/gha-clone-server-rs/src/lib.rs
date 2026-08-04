pub mod credentials;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use sha2::{Digest, Sha256};

pub const SERVICE_NAME: &str = "gha-clone-server";
pub const PLAN_SCHEMA_VERSION: &str = "gha-clone-plan.v1";
pub const MAX_WORKFLOW_BYTES_DEFAULT: usize = 256 * 1024;
pub const MAX_JOBS_DEFAULT: usize = 64;
pub const MAX_STEPS_PER_JOB_DEFAULT: usize = 128;

#[derive(Clone, Debug)]
pub struct PlannerLimits {
    pub max_workflow_bytes: usize,
    pub max_jobs: usize,
    pub max_steps_per_job: usize,
}

impl Default for PlannerLimits {
    fn default() -> Self {
        Self {
            max_workflow_bytes: MAX_WORKFLOW_BYTES_DEFAULT,
            max_jobs: MAX_JOBS_DEFAULT,
            max_steps_per_job: MAX_STEPS_PER_JOB_DEFAULT,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRequest {
    pub repository: String,
    pub revision: String,
    #[serde(default = "default_workflow_path")]
    pub workflow_path: String,
    pub workflow_yaml: String,
}

fn default_workflow_path() -> String {
    ".github/workflows/ci.yml".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPlan {
    pub schema_version: String,
    pub plan_id: String,
    pub repository: String,
    pub revision: String,
    pub workflow_path: String,
    pub immutable_revision: bool,
    pub arc_fully_covered: bool,
    pub independent_executable: bool,
    pub topological_order: Vec<String>,
    pub jobs: Vec<JobPlan>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobPlan {
    pub id: String,
    pub needs: Vec<String>,
    pub runs_on: Vec<String>,
    pub arc_compatible: bool,
    pub arc_lane: String,
    pub independent_supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub independent_profile: Option<String>,
    pub independent_reasons: Vec<String>,
    pub independent_notes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityResponse {
    pub service: String,
    pub plan_schema_version: String,
    pub architecture: ArchitectureCapabilities,
    pub independent_profiles: Vec<String>,
    pub limits: CapabilityLimits,
    pub explicitly_unsupported: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchitectureCapabilities {
    pub native_parity_lane: String,
    pub independent_lane: String,
    pub native_arc_labels: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityLimits {
    pub max_workflow_bytes: usize,
    pub max_jobs: usize,
    pub max_steps_per_job: usize,
}

pub fn capabilities(limits: &PlannerLimits) -> CapabilityResponse {
    CapabilityResponse {
        service: SERVICE_NAME.to_string(),
        plan_schema_version: PLAN_SCHEMA_VERSION.to_string(),
        architecture: ArchitectureCapabilities {
            native_parity_lane:
                "Actions Runner Controller executes original workflow YAML through GitHub's runner protocol"
                    .to_string(),
            independent_lane:
                "fail-closed compiler maps a bounded static workflow subset to fixed dd-build-server profiles"
                    .to_string(),
            native_arc_labels: vec![
                "sonus-ci".to_string(),
                "sonus-browser".to_string(),
                "sonus-ci-dind".to_string(),
                "sonus-android-kvm".to_string(),
            ],
        },
        independent_profiles: vec![
            "rust-verify".to_string(),
            "node-verify".to_string(),
            "node-hardened-verify".to_string(),
            "python-verify".to_string(),
            "flutter-verify".to_string(),
            "flutter-android-debug".to_string(),
            "flutter-web-release".to_string(),
            "flutter-linux-release".to_string(),
            "flutter-linux-desktop-entrypoint".to_string(),
            "playwright".to_string(),
            "puppeteer".to_string(),
            "browser-e2e".to_string(),
        ],
        limits: CapabilityLimits {
            max_workflow_bytes: limits.max_workflow_bytes,
            max_jobs: limits.max_jobs,
            max_steps_per_job: limits.max_steps_per_job,
        },
        explicitly_unsupported: vec![
            "macOS/iOS and Windows native execution in the independent lane".to_string(),
            "dynamic matrices, reusable workflows, OIDC, environments, deployments, and approvals"
                .to_string(),
            "arbitrary marketplace actions, job containers, service containers, KVM, and caller-selected commands"
                .to_string(),
            "secret-bearing expressions or mutable branch execution".to_string(),
        ],
    }
}

pub fn build_plan(
    request: &PlanRequest,
    limits: &PlannerLimits,
) -> Result<WorkflowPlan, Vec<String>> {
    let mut errors = Vec::new();
    if !valid_repository(&request.repository) {
        errors.push(
            "repository must be an owner/name identifier using GitHub-safe characters".into(),
        );
    }
    if !valid_workflow_path(&request.workflow_path) {
        errors
            .push("workflowPath must stay under .github/workflows and end in .yml or .yaml".into());
    }
    if request.workflow_yaml.len() > limits.max_workflow_bytes {
        errors.push(format!(
            "workflowYaml exceeds the {} byte limit",
            limits.max_workflow_bytes
        ));
    }
    if request.workflow_yaml.as_bytes().contains(&0) {
        errors.push("workflowYaml must not contain NUL bytes".into());
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let workflow: Value = serde_yaml::from_str(&request.workflow_yaml)
        .map_err(|error| vec![format!("workflowYaml is not valid YAML: {error}")])?;
    let root = workflow
        .as_mapping()
        .ok_or_else(|| vec!["workflow document must be a YAML mapping".to_string()])?;
    let jobs = mapping_get(root, "jobs")
        .and_then(Value::as_mapping)
        .ok_or_else(|| vec!["workflow.jobs must be a mapping".to_string()])?;

    if jobs.is_empty() {
        errors.push("workflow.jobs must contain at least one job".into());
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
    for key in root.keys().filter_map(Value::as_str) {
        if !matches!(key, "name" | "run-name" | "on" | "jobs") {
            workflow_reasons.push(format!(
                "workflow-level {key} is unsupported by the independent lane"
            ));
        }
    }

    let job_ids = jobs
        .keys()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    if job_ids.len() != jobs.len() {
        return Err(vec!["every workflow job ID must be a string".into()]);
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
                plan.independent_reasons.extend(workflow_reasons.clone());
                if !plan.independent_reasons.is_empty() {
                    plan.independent_supported = false;
                    plan.independent_profile = None;
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
            "revision is not an exact 40-hex commit SHA; planning is allowed but independent execution is refused"
                .to_string(),
        );
    }
    let arc_fully_covered = plans.iter().all(|job| job.arc_compatible);
    let independent_executable =
        immutable_revision && plans.iter().all(|job| job.independent_supported);

    let plan_id = plan_id(request);
    Ok(WorkflowPlan {
        schema_version: PLAN_SCHEMA_VERSION.to_string(),
        plan_id,
        repository: request.repository.clone(),
        revision: request.revision.clone(),
        workflow_path: request.workflow_path.clone(),
        immutable_revision,
        arc_fully_covered,
        independent_executable,
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
    let has_services = mapping_get(job, "services").is_some();
    let has_container = mapping_get(job, "container").is_some();
    let has_strategy = mapping_get(job, "strategy").is_some();

    for key in [
        "uses",
        "permissions",
        "environment",
        "secrets",
        "defaults",
        "outputs",
        "continue-on-error",
        "timeout-minutes",
    ] {
        if mapping_get(job, key).is_some() {
            reasons.push(format!(
                "job-level {key} is unsupported by the independent lane"
            ));
        }
    }
    if has_services {
        reasons.push("service containers require the isolated ARC DinD lane".into());
    }
    if has_container {
        reasons.push("job containers are not reproduced by the independent lane".into());
    }
    if has_strategy {
        reasons.push("dynamic strategy/matrix expansion is unsupported".into());
    }
    let runner_text = runs_on.join(" ").to_ascii_lowercase();
    if runner_text.contains("macos") || runner_text.contains("windows") {
        reasons.push("non-Linux native execution is unavailable in the independent lane".into());
    }
    if let Some(value) = mapping_get(job, "if") {
        reasons.push(format!(
            "job-level if condition is unsupported: {}",
            compact_yaml(value)
        ));
    }
    if contains_secret_expression(mapping_get(job, "env")) {
        reasons.push("job environment contains a secret expression".into());
    }

    let Some(steps) = mapping_get(job, "steps").and_then(Value::as_sequence) else {
        errors.push(format!("jobs.{id}.steps must be a sequence"));
        return Err(errors);
    };
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
        if mapping_get(step, "if").is_some() {
            reasons.push(format!("{path}: conditional steps are unsupported"));
        }
        for key in [
            "working-directory",
            "continue-on-error",
            "timeout-minutes",
            "shell",
        ] {
            if mapping_get(step, key).is_some() {
                reasons.push(format!(
                    "{path}: {key} is unsupported by the fixed-profile executor"
                ));
            }
        }
        if contains_secret_expression(mapping_get(step, "env"))
            || contains_secret_expression(mapping_get(step, "with"))
        {
            reasons.push(format!(
                "{path}: secret-bearing env/with values are unsupported"
            ));
        }
        if let Some(run) = mapping_get(step, "run").and_then(Value::as_str) {
            combined.push_str(run);
            combined.push('\n');
            if run.contains("${{") {
                reasons.push(format!(
                    "{path}: expressions inside run commands are unsupported"
                ));
            }
        }
        if let Some(action) = mapping_get(step, "uses").and_then(Value::as_str) {
            combined.push_str(action);
            combined.push('\n');
            if !allowed_setup_action(action) {
                reasons.push(format!(
                    "{path}: marketplace action {action:?} has no independent-lane equivalence"
                ));
            } else if mapping_get(step, "with").is_some() {
                notes.push(format!(
                    "{path}: setup action inputs are advisory; the fixed profile pins the actual toolchain"
                ));
            }
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let lower = combined.to_ascii_lowercase();
    let profile = classify_profile(&lower);
    if profile.is_none() {
        reasons.push("no fixed build-server profile matches this job".into());
    }
    let independent_supported = reasons.is_empty() && profile.is_some();
    let (arc_compatible, arc_lane) =
        classify_arc_lane(&runs_on, &lower, has_services, has_container);

    Ok(JobPlan {
        id: id.to_string(),
        needs,
        runs_on,
        arc_compatible,
        arc_lane,
        independent_supported,
        independent_profile: if independent_supported { profile } else { None },
        independent_reasons: reasons,
        independent_notes: notes,
    })
}

fn classify_profile(text: &str) -> Option<String> {
    if text.contains("flutter") {
        if text.contains("build apk") || text.contains("build appbundle") {
            return Some("flutter-android-debug".into());
        }
        if text.contains("build web") {
            return Some("flutter-web-release".into());
        }
        if text.contains("build linux") && text.contains("main_desktop.dart") {
            return Some("flutter-linux-desktop-entrypoint".into());
        }
        if text.contains("build linux") {
            return Some("flutter-linux-release".into());
        }
        return Some("flutter-verify".into());
    }
    if text.contains("playwright") {
        return Some("playwright".into());
    }
    if text.contains("puppeteer") {
        return Some("puppeteer".into());
    }
    if text.contains("cargo ") || text.contains("rust-toolchain") || text.contains("rustfmt") {
        return Some("rust-verify".into());
    }
    if text.contains("pytest")
        || text.contains("python -m")
        || text.contains("setup-python")
        || text.contains("pip install")
    {
        return Some("python-verify".into());
    }
    if text.contains("npm ci --ignore-scripts")
        && text.contains("npm run check")
        && text.contains("npm run test:operator-config")
        && text.contains("npm audit --audit-level=high")
    {
        return Some("node-hardened-verify".into());
    }
    if text.contains("npm ")
        || text.contains("pnpm ")
        || text.contains("yarn ")
        || text.contains("setup-node")
        || text.contains("node --test")
    {
        return Some("node-verify".into());
    }
    None
}

fn classify_arc_lane(
    runs_on: &[String],
    text: &str,
    has_services: bool,
    has_container: bool,
) -> (bool, String) {
    let joined = runs_on.join(" ").to_ascii_lowercase();
    if joined.contains("macos") || joined.contains("windows") {
        return (false, "github-hosted-native".into());
    }
    if joined.contains("android")
        || joined.contains("kvm")
        || text.contains("android-emulator-runner")
        || text.contains("avdmanager")
        || text.contains("emulator -")
    {
        return (true, "sonus-android-kvm".into());
    }
    if has_services
        || has_container
        || text.contains("docker build")
        || text.contains("docker compose")
        || text.contains("buildx")
    {
        return (true, "sonus-ci-dind".into());
    }
    if text.contains("playwright")
        || text.contains("puppeteer")
        || text.contains("selenium")
        || text.contains("chromium")
    {
        return (true, "sonus-browser".into());
    }
    (true, "sonus-ci".into())
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

fn contains_secret_expression(value: Option<&Value>) -> bool {
    value.is_some_and(|value| {
        let text = compact_yaml(value).to_ascii_lowercase();
        text.contains("${{ secrets.")
            || text.contains("${{secrets.")
            || text.contains("github.token")
            || text.contains("actions_id_token_request")
    })
}

fn allowed_setup_action(action: &str) -> bool {
    let lower = action.to_ascii_lowercase();
    [
        "actions/checkout@",
        "actions/setup-node@",
        "actions/setup-python@",
        "actions/setup-java@",
        "dtolnay/rust-toolchain@",
        "pnpm/action-setup@",
        "subosito/flutter-action@",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

fn mapping_get<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    mapping.get(Value::String(key.to_string()))
}

fn compact_yaml(value: &Value) -> String {
    serde_yaml::to_string(value)
        .unwrap_or_else(|_| "<unprintable>".into())
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

pub fn is_full_commit_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn plan_id(request: &PlanRequest) -> String {
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

pub fn verify_github_signature(secret: &str, body: &[u8], presented: &str) -> bool {
    let Some(hex_signature) = presented.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(signature) = hex::decode(hex_signature) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(yaml: &str) -> PlanRequest {
        PlanRequest {
            repository: "sonus-auris/sonus-auris-interfaces".into(),
            revision: "0123456789abcdef0123456789abcdef01234567".into(),
            workflow_path: ".github/workflows/ci.yml".into(),
            workflow_yaml: yaml.into(),
        }
    }

    #[test]
    fn maps_static_rust_node_python_dag_to_fixed_profiles() {
        let plan = build_plan(
            &request(
                r#"
name: CI
on: [push]
jobs:
  rust:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@abc
      - uses: dtolnay/rust-toolchain@abc
      - run: cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
  node:
    needs: rust
    runs-on: [self-hosted, linux]
    steps:
      - uses: actions/checkout@abc
      - uses: actions/setup-node@abc
      - run: npm ci && npm test
  python:
    needs: [rust, node]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/setup-python@abc
      - run: python -m compileall . && python -m pytest
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("valid plan");

        assert_eq!(plan.topological_order, vec!["rust", "node", "python"]);
        assert!(plan.independent_executable);
        assert!(plan.arc_fully_covered);
        assert_eq!(
            plan.jobs[0].independent_profile.as_deref(),
            Some("rust-verify")
        );
        assert_eq!(
            plan.jobs[1].independent_profile.as_deref(),
            Some("node-verify")
        );
        assert_eq!(
            plan.jobs[2].independent_profile.as_deref(),
            Some("python-verify")
        );
    }

    #[test]
    fn maps_messaging_intel_operator_workflow_to_hardened_and_full_profiles() {
        let mut input = request(
            r#"
name: Messaging Intel GHA clone operator verification
on:
  workflow_dispatch:
jobs:
  operator_config:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
        with:
          persist-credentials: false
      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020
        with:
          node-version: '22.17.0'
          cache: npm
      - run: |
          npm ci --ignore-scripts
          npm run check
          npm run test:operator-config
          npm audit --audit-level=high
  repository_tests:
    needs: operator_config
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020
      - run: npm ci && npm test
"#,
        );
        input.repository = "messaging-intel/msgint-connectors".into();
        input.workflow_path = ".github/workflows/gha-clone-operator-config.yml".into();
        let plan = build_plan(&input, &PlannerLimits::default()).expect("valid plan");

        assert!(plan.independent_executable);
        assert_eq!(
            plan.topological_order,
            vec!["operator_config", "repository_tests"]
        );
        assert_eq!(
            plan.jobs[0].independent_profile.as_deref(),
            Some("node-hardened-verify")
        );
        assert_eq!(
            plan.jobs[1].independent_profile.as_deref(),
            Some("node-verify")
        );
    }

    #[test]
    fn hardened_node_profile_requires_complete_reviewed_evidence() {
        let plan = build_plan(
            &request(
                r#"
jobs:
  operator_config:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/setup-node@abc
      - run: |
          npm ci --ignore-scripts
          npm run check
          npm run test:operator-config
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("valid plan");
        assert_eq!(
            plan.jobs[0].independent_profile.as_deref(),
            Some("node-verify")
        );
    }

    #[test]
    fn review_order_is_deterministic_for_parallel_roots() {
        let plan = build_plan(
            &request(
                r#"
jobs:
  z:
    runs-on: ubuntu-latest
    steps: [{ run: "cargo test" }]
  a:
    runs-on: ubuntu-latest
    steps: [{ run: "npm test" }]
  child:
    needs: [z, a]
    runs-on: ubuntu-latest
    steps: [{ run: "pytest" }]
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("valid plan");
        assert_eq!(plan.topological_order, vec!["a", "z", "child"]);
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
    fn independent_lane_fails_closed_on_secrets_services_and_marketplace_actions() {
        let plan = build_plan(
            &request(
                r#"
jobs:
  unsafe:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:17
    steps:
      - uses: vendor/arbitrary-action@main
        with:
          token: ${{ secrets.PROD_TOKEN }}
      - run: npm test
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("plan is valid but unsupported");

        let job = &plan.jobs[0];
        assert!(!job.independent_supported);
        assert_eq!(job.arc_lane, "sonus-ci-dind");
        let reasons = job.independent_reasons.join("\n");
        assert!(reasons.contains("service containers"));
        assert!(reasons.contains("marketplace action"));
        assert!(reasons.contains("secret-bearing"));
    }

    #[test]
    fn classifies_browser_android_and_native_operating_system_lanes() {
        let plan = build_plan(
            &request(
                r#"
jobs:
  browser:
    runs-on: ubuntu-latest
    steps: [{ run: "npx playwright test" }]
  android:
    runs-on: [self-hosted, linux, kvm]
    steps: [{ run: "flutter build apk --debug" }]
  ios:
    runs-on: macos-15
    steps: [{ run: "flutter build ipa" }]
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("valid plan");

        assert_eq!(plan.jobs[0].arc_lane, "sonus-browser");
        assert_eq!(plan.jobs[1].arc_lane, "sonus-android-kvm");
        assert_eq!(plan.jobs[2].arc_lane, "github-hosted-native");
        assert!(!plan.jobs[2].arc_compatible);
        assert!(!plan.jobs[2].independent_supported);
        assert!(plan.jobs[2]
            .independent_reasons
            .iter()
            .any(|reason| reason.contains("non-Linux")));
        assert!(!plan.arc_fully_covered);
    }

    #[test]
    fn branch_refs_can_be_planned_but_not_executed() {
        let mut request = request(
            r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps: [{ run: "cargo test" }]
"#,
        );
        request.revision = "main".into();
        let plan = build_plan(&request, &PlannerLimits::default()).expect("valid plan");
        assert!(!plan.immutable_revision);
        assert!(!plan.independent_executable);
        assert!(!plan.warnings.is_empty());
    }

    #[test]
    fn workflow_limits_and_paths_fail_closed() {
        let mut invalid_path_request = request("jobs: {}");
        invalid_path_request.workflow_path = "../ci.yml".into();
        let errors = build_plan(&invalid_path_request, &PlannerLimits::default())
            .unwrap_err()
            .join("\n");
        assert!(errors.contains("workflowPath"));

        let limits = PlannerLimits {
            max_workflow_bytes: 4,
            ..PlannerLimits::default()
        };
        let errors = build_plan(&request("jobs: {}"), &limits)
            .unwrap_err()
            .join("\n");
        assert!(errors.contains("byte limit"));
    }

    #[test]
    fn workflow_and_working_directory_semantics_are_not_silently_ignored() {
        let plan = build_plan(
            &request(
                r#"
permissions:
  contents: read
concurrency: ci-main
jobs:
  test:
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: subdir
    steps:
      - run: npm test
        working-directory: subdir
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("valid but unsupported plan");

        assert!(!plan.independent_executable);
        let reasons = plan.jobs[0].independent_reasons.join("\n");
        assert!(reasons.contains("workflow-level permissions"));
        assert!(reasons.contains("workflow-level concurrency"));
        assert!(reasons.contains("job-level defaults"));
        assert!(reasons.contains("working-directory"));
    }

    #[test]
    fn setup_action_inputs_are_reported_as_fixed_profile_advisory_notes() {
        let plan = build_plan(
            &request(
                r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/setup-node@abc
        with:
          node-version: '22'
      - run: npm test
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("valid plan");
        assert!(plan.jobs[0].independent_supported);
        assert!(plan.jobs[0]
            .independent_notes
            .iter()
            .any(|note| note.contains("fixed profile pins")));
    }

    #[test]
    fn verifies_github_hmac_sha256() {
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(b"secret").unwrap();
        mac.update(b"body");
        let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        assert!(verify_github_signature("secret", b"body", &signature));
        assert!(!verify_github_signature("secret", b"tampered", &signature));
        assert!(!verify_github_signature("secret", b"body", "sha1=00"));
    }
}
