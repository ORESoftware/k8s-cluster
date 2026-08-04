from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CRATE = ROOT / "remote/deployments/gha-clone-server-rs"
IMMUTABLE_TEST_SHA = "0123456789abcdef0123456789abcdef01234567"


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    (ROOT / path).write_text(text, encoding="utf-8")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


def replace_known_test_action_refs() -> None:
    pattern = re.compile(
        r"(?P<action>actions/(?:checkout|setup-node|setup-python|setup-java)|"
        r"dtolnay/rust-toolchain|pnpm/action-setup|subosito/flutter-action)@abc\b"
    )
    changed = 0
    for path in sorted(CRATE.rglob("*")):
        if not path.is_file() or path.suffix not in {".rs", ".yml", ".yaml"}:
            continue
        source = path.read_text(encoding="utf-8")
        updated, count = pattern.subn(
            lambda match: f"{match.group('action')}@{IMMUTABLE_TEST_SHA}", source
        )
        if count:
            path.write_text(updated, encoding="utf-8")
            changed += count
    if changed < 4:
        raise RuntimeError(f"expected at least four synthetic mutable refs, found {changed}")


# --- Independent workflow compiler -------------------------------------------------
lib_path = "remote/deployments/gha-clone-server-rs/src/lib.rs"
text = read(lib_path)

text = replace_once(
    text,
    '''            "node-hardened-verify".to_string(),
            "python-verify".to_string(),''',
    '''            "node-hardened-verify".to_string(),
            "node-hardened-test".to_string(),
            "python-verify".to_string(),''',
    "capability profile list",
)
text = replace_once(
    text,
    '''    let mut notes = Vec::new();
    let mut combined = String::new();
    let has_services = mapping_get(job, "services").is_some();''',
    '''    let mut notes = Vec::new();
    let mut combined = String::new();
    let mut run_commands = Vec::new();
    let has_services = mapping_get(job, "services").is_some();''',
    "run command collection",
)
text = replace_once(
    text,
    '''    if contains_secret_expression(mapping_get(job, "env")) {
        reasons.push("job environment contains a secret expression".into());
    }''',
    '''    if let Some(environment) = mapping_get(job, "env") {
        if contains_secret_expression(Some(environment)) {
            reasons.push("job environment contains a secret expression".into());
        } else {
            reasons.push(
                "job environment is unsupported because fixed profiles do not forward caller-selected variables"
                    .into(),
            );
        }
    }''',
    "job environment rejection",
)
text = replace_once(
    text,
    '''        if contains_secret_expression(mapping_get(step, "env"))
            || contains_secret_expression(mapping_get(step, "with"))
        {
            reasons.push(format!(
                "{path}: secret-bearing env/with values are unsupported"
            ));
        }''',
    '''        if let Some(environment) = mapping_get(step, "env") {
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
            reasons.push(format!(
                "{path}: expressions in setup inputs are unsupported"
            ));
        }''',
    "step environment and setup expression rejection",
)
text = replace_once(
    text,
    '''        if let Some(run) = mapping_get(step, "run").and_then(Value::as_str) {
            combined.push_str(run);
            combined.push('\n');
            if run.contains("${{") {''',
    '''        if let Some(run) = mapping_get(step, "run").and_then(Value::as_str) {
            combined.push_str(run);
            combined.push('\n');
            run_commands.extend(
                run.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string),
            );
            if run.contains("${{") {''',
    "ordered run command collection",
)
text = replace_once(
    text,
    '''            if !allowed_setup_action(action) {
                reasons.push(format!(
                    "{path}: marketplace action {action:?} has no independent-lane equivalence"
                ));
            } else if mapping_get(step, "with").is_some() {''',
    '''            if !known_setup_action(action) {
                reasons.push(format!(
                    "{path}: marketplace action {action:?} has no independent-lane equivalence"
                ));
            } else if !immutable_action_ref(action) {
                reasons.push(format!(
                    "{path}: setup action {action:?} must use an exact 40-hex commit SHA"
                ));
            } else if mapping_get(step, "with").is_some() {''',
    "immutable setup action requirement",
)
text = replace_once(
    text,
    '''    let lower = combined.to_ascii_lowercase();
    let profile = classify_profile(&lower);
    if profile.is_none() {
        reasons.push("no fixed build-server profile matches this job".into());
    }''',
    '''    let lower = combined.to_ascii_lowercase();
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
    }''',
    "exact hardened profile classification",
)
text = replace_once(
    text,
    '''    if text.contains("npm ci --ignore-scripts")
        && text.contains("npm run check")
        && text.contains("npm run test:operator-config")
        && text.contains("npm audit --audit-level=high")
    {
        return Some("node-hardened-verify".into());
    }
''',
    "",
    "remove substring hardened classifier",
)
text = replace_once(
    text,
    '''fn contains_secret_expression(value: Option<&Value>) -> bool {
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
''',
    '''fn contains_expression(value: Option<&Value>) -> bool {
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

fn commands_match(commands: &[String], expected: &[&str]) -> bool {
    commands.len() == expected.len()
        && commands
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual == expected)
}

fn hardened_node_profile(commands: &[String]) -> Option<&'static str> {
    const OPERATOR: [&str; 4] = [
        "npm ci --ignore-scripts",
        "npm run check",
        "npm run test:operator-config",
        "npm audit --audit-level=high",
    ];
    const FULL_TEST: [&str; 2] = ["npm ci --ignore-scripts", "npm test"];

    if commands_match(commands, &OPERATOR) {
        Some("node-hardened-verify")
    } else if commands_match(commands, &FULL_TEST) {
        Some("node-hardened-test")
    } else {
        None
    }
}
''',
    "expression, secret, action, and exact profile helpers",
)

text = replace_once(
    text,
    '''      - run: npm ci && npm test
"#,
        );''',
    '''      - run: |
          npm ci --ignore-scripts
          npm test
"#,
        );''',
    "Messaging Intel full-test unit fixture",
)
text = replace_once(
    text,
    '''        assert_eq!(
            plan.jobs[1].independent_profile.as_deref(),
            Some("node-verify")
        );''',
    '''        assert_eq!(
            plan.jobs[1].independent_profile.as_deref(),
            Some("node-hardened-test")
        );''',
    "Messaging Intel full-test unit profile",
)
text = replace_once(
    text,
    '''        assert_eq!(
            plan.jobs[0].independent_profile.as_deref(),
            Some("node-verify")
        );
    }

    #[test]
    fn review_order_is_deterministic_for_parallel_roots() {''',
    '''        assert!(!plan.independent_executable);
        assert!(!plan.jobs[0].independent_supported);
        assert!(plan.jobs[0].independent_profile.is_none());
        assert!(plan.jobs[0]
            .independent_reasons
            .iter()
            .any(|reason| reason.contains("exact reviewed command sequence")));
    }

    #[test]
    fn hardened_node_profiles_reject_extra_reordered_and_spoofed_commands() {
        for run in [
            "npm ci --ignore-scripts\nnpm run check\nnpm run test:operator-config\nnpm audit --audit-level=high\nnpm publish",
            "npm run check\nnpm ci --ignore-scripts\nnpm run test:operator-config\nnpm audit --audit-level=high",
            "echo 'npm ci --ignore-scripts npm run check npm run test:operator-config npm audit --audit-level=high'",
        ] {
            let indented = run
                .lines()
                .map(|line| format!("          {line}\n"))
                .collect::<String>();
            let yaml = format!(
                "jobs:\n  operator_config:\n    runs-on: ubuntu-latest\n    steps:\n      - run: |\n{indented}"
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
    fn review_order_is_deterministic_for_parallel_roots() {''',
    "semantic hardening unit tests",
)
write(lib_path, text)
replace_known_test_action_refs()

# Ensure the adversarial capability contract includes the second hardened profile.
adversarial_path = "remote/deployments/gha-clone-server-rs/tests/planner_adversarial.rs"
adversarial = read(adversarial_path)
adversarial = replace_once(
    adversarial,
    '''    assert!(profiles.contains("node-verify"));
    assert!(profiles.contains("python-verify"));''',
    '''    assert!(profiles.contains("node-verify"));
    assert!(profiles.contains("node-hardened-verify"));
    assert!(profiles.contains("node-hardened-test"));
    assert!(profiles.contains("python-verify"));''',
    "capability assertions",
)
write(adversarial_path, adversarial)

# --- Messaging Intel bounded fixture ----------------------------------------------
fixture_path = "remote/deployments/gha-clone-server-rs/tests/fixtures/msgint-operator-config.yml"
fixture = read(fixture_path)
fixture = replace_once(
    fixture,
    '''      - run: |
          npm ci
          npm test
''',
    '''      - run: |
          npm ci --ignore-scripts
          npm test
''',
    "lifecycle-script-free repository test fixture",
)
write(fixture_path, fixture)

# --- Real HTTP server and recording build-server proof ----------------------------
integration_path = "remote/deployments/gha-clone-server-rs/tests/msgint_operator_config.rs"
integration = read(integration_path)
integration = integration.replace('"node-verify"', '"node-hardened-test"')
integration = replace_once(
    integration,
    '            "workflowYaml": workflow_yaml\n',
    '            "workflowYaml": &workflow_yaml\n',
    "retain workflow source for adversarial submissions",
)
integration = replace_once(
    integration,
    '''    assert!(submissions[1]["requestId"]
        .as_str()
        .is_some_and(
            |value| value.starts_with("gha-clone:") && value.ends_with(":repository_tests")
        ));

    mock_task.abort();''',
    '''    assert!(submissions[1]["requestId"]
        .as_str()
        .is_some_and(
            |value| value.starts_with("gha-clone:") && value.ends_with(":repository_tests")
        ));
    drop(submissions);

    let extra_command = workflow_yaml.replacen(
        "          npm audit --audit-level=high\n",
        "          npm audit --audit-level=high\n          npm publish\n",
        1,
    );
    let reordered_commands = workflow_yaml.replacen(
        "          npm ci --ignore-scripts\n          npm run check\n",
        "          npm run check\n          npm ci --ignore-scripts\n",
        1,
    );
    let mutable_action = workflow_yaml.replacen(
        "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
        "actions/checkout@main",
        1,
    );
    let bracket_secret = workflow_yaml.replacen(
        "          persist-credentials: false\n",
        "          persist-credentials: false\n        env:\n          MSGINT_META_ACCESS_TOKEN: ${{ secrets['PROD_TOKEN'] }}\n",
        1,
    );
    let plain_environment = workflow_yaml.replacen(
        "  operator_config:\n    runs-on: ubuntu-latest\n",
        "  operator_config:\n    runs-on: ubuntu-latest\n    env:\n      NODE_ENV: test\n",
        1,
    );

    for (label, rejected_yaml, expected_reason) in [
        (
            "extra hardened command",
            extra_command,
            "exact reviewed command sequence",
        ),
        (
            "reordered hardened commands",
            reordered_commands,
            "exact reviewed command sequence",
        ),
        (
            "mutable setup action",
            mutable_action,
            "exact 40-hex commit SHA",
        ),
        ("bracket secret expression", bracket_secret, "secret-bearing"),
        (
            "plain job environment",
            plain_environment,
            "fixed profiles do not forward",
        ),
    ] {
        let response = client
            .post(format!("{server_url}/v1/runs"))
            .header("x-gha-clone-auth", SERVER_AUTH)
            .json(&json!({
                "repository": REPOSITORY,
                "revision": REVISION,
                "workflowPath": WORKFLOW_PATH,
                "workflowYaml": rejected_yaml
            }))
            .send()
            .await
            .unwrap_or_else(|error| panic!("submit rejected {label} workflow: {error}"));
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{label} unexpectedly reached execution"
        );
        let body = response
            .text()
            .await
            .unwrap_or_else(|error| panic!("read rejected {label} response: {error}"));
        assert!(
            body.contains(expected_reason),
            "{label} response did not explain {expected_reason}: {body}"
        );
        assert_eq!(
            mock_state.submissions.lock().await.len(),
            2,
            "{label} dispatched a build despite rejection"
        );
    }

    mock_task.abort();''',
    "real-server zero-dispatch adversarial cases",
)
write(integration_path, integration)

# --- Fixed-profile deployment and hosted private canary ---------------------------
patch_path = "remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml"
patch = read(patch_path)
patch = replace_once(
    patch,
    "rust-verify,node-verify,node-hardened-verify,python-verify",
    "rust-verify,node-verify,node-hardened-verify,node-hardened-test,python-verify",
    "deployed hardened test profile allowlist",
)
write(patch_path, patch)

workflow_path = ".github/workflows/gha-clone-server.yml"
workflow = read(workflow_path)
workflow = replace_once(
    workflow,
    '("node-verify", "NODE_VERIFY_STEPS"),',
    '("node-hardened-test", "NODE_HARDENED_TEST_STEPS"),',
    "private canary full-test profile extraction",
)
workflow = replace_once(
    workflow,
    "for profile in node-hardened-verify node-verify; do",
    "for profile in node-hardened-verify node-hardened-test; do",
    "private canary profile loop",
)
write(workflow_path, workflow)

# --- Documentation ----------------------------------------------------------------
build_readme_path = "remote/deployments/build-server-rs/readme.md"
build_readme = read(build_readme_path)
build_readme = replace_once(
    build_readme,
    "| `node-hardened-verify` | npm lifecycle-script suppression, operator checks, and high-severity audit | none |",
    "| `node-hardened-verify` | npm lifecycle-script suppression, operator checks, and high-severity audit | none |\n| `node-hardened-test` | npm lifecycle-script suppression and complete repository tests | none |",
    "build-server profile documentation",
)
write(build_readme_path, build_readme)

gha_readme_path = "remote/deployments/gha-clone-server-rs/README.md"
gha_readme = read(gha_readme_path)
gha_readme = replace_once(
    gha_readme,
    "operator contract and `node-verify` for the complete repository test suite.",
    "operator contract and `node-hardened-test` for lifecycle-script-free complete repository tests.",
    "Messaging Intel profile overview",
)
gha_readme = replace_once(
    gha_readme,
    "| npm install-script suppression + operator checks + high-severity audit | `node-hardened-verify` |",
    "| npm install-script suppression + operator checks + high-severity audit | `node-hardened-verify` |\n| npm install-script suppression + complete repository tests | `node-hardened-test` |",
    "independent profile mapping documentation",
)
gha_readme = replace_once(
    gha_readme,
    "- arbitrary marketplace actions;",
    "- arbitrary marketplace actions or setup actions referenced by mutable tags/branches;",
    "mutable setup action exclusion documentation",
)
gha_readme = replace_once(
    gha_readme,
    "- environments, deployments, reusable workflows, and caller-selected commands.",
    "- environments, deployments, reusable workflows, caller-selected environment variables, and commands outside an exact reviewed hardened sequence.",
    "caller-selected input exclusion documentation",
)
write(gha_readme_path, gha_readme)

# --- Static deployment/workflow contracts -----------------------------------------
contracts_path = "remote/tests/general/gha-clone-server-config.test.ts"
contracts = read(contracts_path)
contracts = replace_once(
    contracts,
    "    'node-hardened-verify',\n    'python-verify',",
    "    'node-hardened-verify',\n    'node-hardened-test',\n    'python-verify',",
    "profile contract list",
)
contracts = replace_once(
    contracts,
    "  assert.match(planner, /node-hardened-verify/);",
    "  assert.match(planner, /node-hardened-verify/);\n  assert.match(planner, /node-hardened-test/);\n  assert.match(planner, /exact reviewed command sequence/);\n  assert.match(planner, /exact 40-hex commit SHA/);",
    "planner semantic contracts",
)
contracts = replace_once(
    contracts,
    "  assert.match(integration, /node-verify/);",
    "  assert.match(integration, /node-hardened-test/);\n  assert.match(integration, /UNPROCESSABLE_ENTITY/);\n  assert.match(integration, /npm publish/);\n  assert.match(integration, /PROD_TOKEN/);",
    "real-server adversarial contracts",
)
contracts = replace_once(
    contracts,
    "  assert.match(workflow, /npm test/);\n  assert.equal((workflow.match(/persist-credentials:\\s*false/g) ?? []).length, 2);",
    "  assert.match(workflow, /npm test/);\n  assert.equal((workflow.match(/npm ci --ignore-scripts/g) ?? []).length, 2);\n  assert.equal((workflow.match(/persist-credentials:\\s*false/g) ?? []).length, 2);",
    "fixture lifecycle-script suppression contract",
)
contracts = replace_once(
    contracts,
    "  assert.match(workflow, /persist-credentials:\\s*false/);\n});",
    "  assert.match(workflow, /persist-credentials:\\s*false/);\n  assert.match(workflow, /node-hardened-test/);\n  assert.match(workflow, /MSGINT_REPOSITORY_READ_APP_ID|K8S_SUBMODULE_APP_ID/);\n  assert.doesNotMatch(workflow, /ghp_|github_pat_/);\n});",
    "hosted private canary contract",
)
write(contracts_path, contracts)

print("Messaging Intel semantic hardening patch applied")
