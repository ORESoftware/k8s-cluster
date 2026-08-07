pub mod credentials;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

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
            "rust-generated-verify".to_string(),
            "node-verify".to_string(),
            "node-hardened-verify".to_string(),
            "node-hardened-test".to_string(),
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
            "caller-selected environments, secret-bearing expressions, mutable setup actions, or mutable branch execution"
                .to_string(),
        ],
    }
}

fn is_block_scalar_header(value: &str) -> bool {
    let token = value.split_whitespace().next().unwrap_or_default();
    let Some(modifiers) = token.strip_prefix('|').or_else(|| token.strip_prefix('>')) else {
        return false;
    };
    modifiers
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'+' | b'-'))
}

fn validate_workflow_document(source: &str) -> Result<(), String> {
    if source.as_bytes().contains(&b'\t') {
        return Err("workflowYaml contains a tab; indentation must use spaces".into());
    }

    let mut parents = Vec::<(usize, String)>::new();
    let mut seen = BTreeMap::<String, BTreeSet<String>>::new();
    let mut sequence_ordinals = BTreeMap::<String, usize>::new();
    let mut block_scalar_indent = None::<usize>;

    for (index, raw_line) in source.lines().enumerate() {
        let line_number = index + 1;
        let indent = raw_line.bytes().take_while(|byte| *byte == b' ').count();
        let trimmed = raw_line.trim();

        if let Some(block_indent) = block_scalar_indent {
            if trimmed.is_empty() || indent > block_indent {
                continue;
            }
            block_scalar_indent = None;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "---"
            || trimmed.starts_with("--- ")
            || trimmed == "..."
            || trimmed.starts_with("... ")
        {
            return Err(format!(
                "workflowYaml line {line_number} uses a YAML document marker"
            ));
        }

        while parents
            .last()
            .is_some_and(|(parent_indent, _)| *parent_indent >= indent)
        {
            parents.pop();
        }

        let raw_content = raw_line[indent..].trim_end();
        let (content, key_indent) = if let Some(after_dash) = raw_content.strip_prefix('-') {
            if after_dash.is_empty() || after_dash.starts_with(' ') {
                let spaces_after_dash = after_dash.bytes().take_while(|byte| *byte == b' ').count();
                let parent_scope = parents
                    .iter()
                    .map(|(_, parent)| parent.as_str())
                    .collect::<Vec<_>>()
                    .join("/");
                let counter = sequence_ordinals
                    .entry(format!("{parent_scope}@{indent}"))
                    .or_default();
                parents.push((indent, format!("[{counter}]")));
                *counter += 1;
                (
                    after_dash[spaces_after_dash..].trim_end(),
                    indent + 1 + spaces_after_dash,
                )
            } else {
                (raw_content, indent)
            }
        } else {
            (raw_content, indent)
        };
        if content.is_empty() {
            continue;
        }
        if content.starts_with(['!', '&', '*']) {
            return Err(format!(
                "workflowYaml line {line_number} uses a YAML tag, anchor, or alias"
            ));
        }
        if content.starts_with('{') && content.trim() != "{}" {
            return Err(format!(
                "workflowYaml line {line_number} uses a non-empty flow mapping"
            ));
        }

        let Some((raw_key, raw_value)) = content.split_once(':') else {
            continue;
        };
        let key = raw_key.trim().trim_matches(['\'', '"']);
        let value = raw_value.trim_start();
        if key.is_empty() {
            continue;
        }
        if !key.is_ascii() {
            return Err(format!(
                "workflowYaml line {line_number} has a non-ASCII mapping key"
            ));
        }
        if key == "<<" {
            return Err(format!(
                "workflowYaml line {line_number} uses a YAML merge key"
            ));
        }
        if value.starts_with(['!', '&', '*']) {
            return Err(format!(
                "workflowYaml line {line_number} uses a YAML tag, anchor, or alias"
            ));
        }
        let uncommented_value = value.split('#').next().unwrap_or(value).trim_end();
        if uncommented_value.starts_with('{') && uncommented_value != "{}" {
            return Err(format!(
                "workflowYaml line {line_number} uses a non-empty flow mapping"
            ));
        }

        let scope = parents
            .iter()
            .map(|(_, parent)| parent.as_str())
            .collect::<Vec<_>>()
            .join("/");
        if !seen
            .entry(scope)
            .or_default()
            .insert(key.to_ascii_lowercase())
        {
            return Err(format!(
                "workflowYaml line {line_number} repeats mapping key {key:?}"
            ));
        }

        if is_block_scalar_header(value) {
            block_scalar_indent = Some(key_indent);
        } else if value.is_empty() {
            parents.push((key_indent, key.to_ascii_lowercase()));
        }
    }
    Ok(())
}

pub fn build_plan(
    request: &PlanRequest,
    limits: &PlannerLimits,
) -> Result<WorkflowPlan, Vec<String>> {
    let mut errors = Vec::new();
    if limits.max_workflow_bytes == 0 {
        errors.push("maxWorkflowBytes must be greater than zero".into());
    }
    if limits.max_jobs == 0 {
        errors.push("maxJobs must be greater than zero".into());
    }
    if limits.max_steps_per_job == 0 {
        errors.push("maxStepsPerJob must be greater than zero".into());
    }
    if !valid_repository(&request.repository) {
        errors.push(
            "repository must be an owner/name identifier using GitHub-safe characters".into(),
        );
    }
    if !valid_workflow_path(&request.workflow_path) {
        errors
            .push("workflowPath must stay under .github/workflows as one direct ASCII <file>.yml or .yaml file".into());
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
    if let Err(error) = validate_workflow_document(&request.workflow_yaml) {
        errors.push(error);
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let workflow: Value = serde_yaml::from_str(&request.workflow_yaml)
        .map_err(|error| vec![format!("workflowYaml is not valid YAML: {error}")])?;
    let root = workflow
        .as_mapping()
        .ok_or_else(|| vec!["workflow document must be a YAML mapping".to_string()])?;

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
        if let Some(environment) = mapping_get(step, "env") {
            if contains_secret_expression(Some(environment)) {
                reasons.push(format!(
                    "{path}: secret-bearing step environments are unsupported"
                ));
            } else {
                reasons.push(format!(
                    "{path}: step environments are unsupported because fixed profiles do not forward caller-selected variables"
                ));
            }
        }
        if contains_secret_expression(mapping_get(step, "with")) {
            reasons.push(format!(
                "{path}: secret-bearing setup inputs are unsupported"
            ));
        } else if contains_expression(mapping_get(step, "with")) {
            reasons.push(format!("{path}: expressions in setup inputs are unsupported"));
        }
        if let Some(run) = mapping_get(step, "run").and_then(Value::as_str) {
            combined.push_str(run);
            combined.push('\n');
            run_commands.extend(
                run.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string),
            );
            if run.contains("${{") {
                reasons.push(format!(
                    "{path}: expressions inside run commands are unsupported"
                ));
            }
        }
        if let Some(action) = mapping_get(step, "uses").and_then(Value::as_str) {
            combined.push_str(action);
            combined.push('\n');
            if !known_setup_action(action) {
                reasons.push(format!(
                    "{path}: marketplace action {action:?} has no independent-lane equivalence"
                ));
            } else if !immutable_action_ref(action) {
                reasons.push(format!(
                    "{path}: setup action {action:?} must use an exact 40-hex commit SHA"
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
    let profile = if hardened_node_intent(&lower) {
        match hardened_node_profile(&run_commands) {
            Some(profile) => Some(profile.to_string()),
            None => {
                reasons.push(
                    "hardened Node jobs must use one exact reviewed command sequence in the documented order with no extra commands"
                        .into(),
                );
                None
            }
        }
    } else {
        classify_profile(&lower)
    };
    if profile.is_none() && reasons.is_empty() {
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

    if plan.jobs.len() != profiles.len() {
        return Err(vec![
            "reviewed Messaging Intel contract and generic planner produced different job sets"
                .to_string(),
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

fn contains_expression(value: Option<&Value>) -> bool {
    value.is_some_and(|value| compact_yaml(value).contains("${{"))
}

fn contains_secret_expression(value: Option<&Value>) -> bool {
    value.is_some_and(|value| {
        let compact = compact_yaml(value)
            .to_ascii_lowercase()
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        compact.contains("${{secrets")
            || compact.contains("tojson(secrets)")
            || compact.contains("fromjson(secrets)")
            || compact.contains("github.token")
            || compact.contains("github['token']")
            || compact.contains("github[\"token\"]")
            || compact.contains("actions_id_token_request")
    })
}

fn known_setup_action(action: &str) -> bool {
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

fn immutable_action_ref(action: &str) -> bool {
    action
        .rsplit_once('@')
        .is_some_and(|(_, reference)| is_full_commit_sha(reference))
}

fn hardened_node_intent(text: &str) -> bool {
    text.contains("npm ci --ignore-scripts")
        || text.contains("npm run test:operator-config")
        || text.contains("npm audit --audit-level=high")
}

fn hardened_node_profile(commands: &[String]) -> Option<&'static str> {
    const OPERATOR: [&str; 4] = [
        "npm ci --ignore-scripts",
        "npm run check",
        "npm run test:operator-config",
        "npm audit --audit-level=high",
    ];
    const FULL_TEST: [&str; 2] = ["npm ci --ignore-scripts", "npm test"];
    if commands.iter().map(String::as_str).eq(OPERATOR) {
        Some("node-hardened-verify")
    } else if commands.iter().map(String::as_str).eq(FULL_TEST) {
        Some("node-hardened-test")
    } else {
        None
    }
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
    const PREFIX: &str = ".github/workflows/";
    let Some(file) = value.strip_prefix(PREFIX) else {
        return false;
    };
    !file.is_empty()
        && !file.contains('/')
        && !file.contains('\\')
        && !file.contains("..")
        && (file.ends_with(".yml") || file.ends_with(".yaml"))
        && file
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
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
      - uses: actions/checkout@0123456789abcdef0123456789abcdef01234567
      - uses: dtolnay/rust-toolchain@0123456789abcdef0123456789abcdef01234567
      - run: cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
  node:
    needs: rust
    runs-on: [self-hosted, linux]
    steps:
      - uses: actions/checkout@0123456789abcdef0123456789abcdef01234567
      - uses: actions/setup-node@0123456789abcdef0123456789abcdef01234567
      - run: npm ci && npm test
  python:
    needs: [rust, node]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/setup-python@0123456789abcdef0123456789abcdef01234567
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
      - run: |
          npm ci --ignore-scripts
          npm test
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
            Some("node-hardened-test")
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
      - uses: actions/setup-node@0123456789abcdef0123456789abcdef01234567
      - run: |
          npm ci --ignore-scripts
          npm run check
          npm run test:operator-config
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("valid plan");
        assert!(!plan.independent_executable);
        assert!(!plan.jobs[0].independent_supported);
        assert!(plan.jobs[0].independent_profile.is_none());
        assert!(plan.jobs[0]
            .independent_reasons
            .iter()
            .any(|reason| reason.contains("exact reviewed command sequence")));
    }

    #[test]
    fn hardened_node_profiles_reject_spoofed_extra_and_reordered_commands() {
        for run in [
            r#"echo 'npm ci --ignore-scripts npm run check npm run test:operator-config npm audit --audit-level=high'"#,
            r#"npm ci --ignore-scripts
npm run check
npm run test:operator-config
npm audit --audit-level=high
npm publish"#,
            r#"npm run check
npm ci --ignore-scripts
npm run test:operator-config
npm audit --audit-level=high"#,
        ] {
            let yaml = format!(
                "jobs:\n  operator_config:\n    runs-on: ubuntu-latest\n    steps:\n      - run: |\n{}",
                run.lines()
                    .map(|line| format!("          {line}\n"))
                    .collect::<String>()
            );
            let plan = build_plan(&request(&yaml), &PlannerLimits::default())
                .expect("structurally valid plan");
            assert!(!plan.independent_executable, "unexpected executable plan: {run}");
            assert!(plan.jobs[0].independent_profile.is_none());
            assert!(plan.jobs[0]
                .independent_reasons
                .iter()
                .any(|reason| reason.contains("exact reviewed command sequence")));
        }
    }

    #[test]
    fn setup_actions_require_immutable_commit_refs() {
        let plan = build_plan(
            &request(
                r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/setup-node@main
      - run: npm test
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("valid but unsupported plan");
        assert!(!plan.independent_executable);
        assert!(plan.jobs[0]
            .independent_reasons
            .iter()
            .any(|reason| reason.contains("exact 40-hex commit SHA")));
    }

    #[test]
    fn plain_environments_and_bracket_secret_expressions_fail_closed() {
        let plan = build_plan(
            &request(
                r#"
jobs:
  plain:
    runs-on: ubuntu-latest
    env:
      NODE_ENV: test
    steps:
      - run: npm test
  secret:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/setup-node@0123456789abcdef0123456789abcdef01234567
        env:
          TOKEN: ${{ secrets['PROD_TOKEN'] }}
      - run: npm test
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("valid but unsupported plan");
        assert!(!plan.independent_executable);
        assert!(plan.jobs[0]
            .independent_reasons
            .iter()
            .any(|reason| reason.contains("fixed profiles do not forward")));
        assert!(plan.jobs[1]
            .independent_reasons
            .iter()
            .any(|reason| reason.contains("secret-bearing")));
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

    for job in &mut plan.jobs {
        let Some(profile) = profiles.get(&job.id) else {
            return Err(vec![format!(
                "reviewed Messaging Intel contract did not authorize job {:?}",
                job.id
            )]);
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
      - uses: actions/setup-node@0123456789abcdef0123456789abcdef01234567
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

    #[test]
    fn generated_rust_commands_are_exact_and_order_sensitive() {
        let exact = [
            "cargo generate-lockfile --manifest-path generated/rust/Cargo.toml",
            "cargo fmt --manifest-path generated/rust/Cargo.toml -- --check",
            "cargo clippy --locked --manifest-path generated/rust/Cargo.toml --all-targets -- -D warnings",
            "cargo test --locked --manifest-path generated/rust/Cargo.toml --all-targets",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        assert_eq!(
            generated_rust_profile(&exact),
            Some("rust-generated-verify")
        );

        let mut reordered = exact.clone();
        reordered.swap(2, 3);
        assert_eq!(generated_rust_profile(&reordered), None);

        let mut extra = exact.clone();
        extra.push("cargo publish --manifest-path generated/rust/Cargo.toml".into());
        assert_eq!(generated_rust_profile(&extra), None);
        assert!(generated_rust_intent(
            &exact
                .join(
                    "
"
                )
                .to_ascii_lowercase()
        ));
    }
}

#[cfg(test)]
mod den_1606_planner_input_tests {
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
    fn direct_workflow_paths_only() {
        for invalid in [
            ".github/workflows/nested/ci.yml",
            ".github/workflows\\ci.yml",
            ".github/workflows/ci.txt",
            ".github/workflows/",
        ] {
            let mut input = request("jobs: {}");
            input.workflow_path = invalid.into();
            let errors = build_plan(&input, &PlannerLimits::default())
                .unwrap_err()
                .join("\n");
            assert!(errors.contains("workflowPath"), "{invalid}: {errors}");
        }
    }

    #[test]
    fn zero_planner_limits_fail_closed() {
        let errors = build_plan(
            &request("jobs: {}"),
            &PlannerLimits {
                max_workflow_bytes: 0,
                max_jobs: 0,
                max_steps_per_job: 0,
            },
        )
        .unwrap_err()
        .join("\n");
        for field in ["maxWorkflowBytes", "maxJobs", "maxStepsPerJob"] {
            assert!(errors.contains(field), "{errors}");
        }
    }

    #[test]
    fn ambiguous_block_yaml_is_rejected_before_deserialization() {
        let cases = [
            ("---\njobs: {}\n", "document marker"),
            ("jobs: &jobs {}\n", "anchor"),
            ("jobs: *jobs\n", "alias"),
            ("jobs:\n  <<: *jobs\n", "merge key"),
            ("jobs: {}\njobs: {}\n", "repeats mapping key"),
            ("jóbs: {}\n", "non-ASCII"),
            ("jobs:\n\ttest: {}\n", "tab"),
        ];
        for (yaml, expected) in cases {
            let errors = build_plan(&request(yaml), &PlannerLimits::default())
                .unwrap_err()
                .join("\n");
            assert!(errors.contains(expected), "{yaml:?}: {errors}");
        }
    }

    #[test]
    fn block_scalar_text_is_not_mistaken_for_yaml_structure() {
        let plan = build_plan(
            &request(
                r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - name: script
        run: |
          echo ---
          echo '&anchor *alias <<:'
          cargo test
      - name: second
        run: cargo fmt --check
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("block scalar contents are command text");
        assert!(plan.independent_executable);
    }

    #[test]
    fn sequence_items_have_independent_mapping_scopes() {
        let plan = build_plan(
            &request(
                r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - name: format
        run: cargo fmt --check
      - name: test
        run: cargo test
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("different steps may repeat name and run keys");
        assert!(plan.independent_executable);

        let errors = build_plan(
            &request(
                r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - name: test
        run: cargo test
        run: cargo fmt --check
"#,
            ),
            &PlannerLimits::default(),
        )
        .unwrap_err()
        .join("\n");
        assert!(errors.contains("repeats mapping key \"run\""), "{errors}");
    }
}

#[cfg(test)]
mod den_1606_planner_input_followup_tests {
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
    fn block_scalar_siblings_return_to_the_sequence_item_scope() {
        let valid = build_plan(
            &request(
                r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - name: first
        run: |
          echo ok
          cargo test
        shell: bash
      - name: second
        run: cargo fmt --check
        shell: bash
"#,
            ),
            &PlannerLimits::default(),
        )
        .expect("separate sequence items may repeat mapping keys");
        assert!(!valid.independent_executable);

        let errors = build_plan(
            &request(
                r#"
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - name: duplicate
        run: |
          echo ok
          cargo test
        shell: bash
        shell: sh
"#,
            ),
            &PlannerLimits::default(),
        )
        .unwrap_err()
        .join("\n");
        assert!(errors.contains("repeats mapping key \"shell\""), "{errors}");
    }

    #[test]
    fn flow_mappings_and_unsafe_direct_paths_fail_closed() {
        let flow_errors = build_plan(
            &request("jobs: { test: { runs-on: ubuntu-latest } }\n"),
            &PlannerLimits::default(),
        )
        .unwrap_err()
        .join("\n");
        assert!(flow_errors.contains("flow mapping"), "{flow_errors}");

        for invalid in [
            ".github/workflows/ci file.yml",
            ".github/workflows/cí.yml",
            ".github/workflows/ci..yml",
        ] {
            let mut input = request("jobs: {}");
            input.workflow_path = invalid.into();
            let errors = build_plan(&input, &PlannerLimits::default())
                .unwrap_err()
                .join("\n");
            assert!(errors.contains("workflowPath"), "{invalid}: {errors}");
        }
    }

    #[test]
    fn commented_document_markers_fail_closed() {
        let errors = build_plan(
            &request("--- # document one\njobs: {}\n"),
            &PlannerLimits::default(),
        )
        .unwrap_err()
        .join("\n");
        assert!(errors.contains("document marker"), "{errors}");
    }
}
