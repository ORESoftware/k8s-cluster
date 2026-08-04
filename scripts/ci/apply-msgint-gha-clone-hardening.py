from pathlib import Path

IMMUTABLE_TEST_REF = "0123456789abcdef0123456789abcdef01234567"


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


def replace_at_least(text: str, old: str, new: str, minimum: int, label: str) -> str:
    count = text.count(old)
    if count < minimum:
        raise RuntimeError(f"{label}: expected at least {minimum} matches, found {count}")
    return text.replace(old, new)


# Harden the independent-lane planner. A fixed profile must never be selected
# from substring evidence when a job is claiming the hardened Node contract.
path = Path("remote/deployments/gha-clone-server-rs/src/lib.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "    let mut combined = String::new();\n    let has_services = mapping_get(job, \"services\").is_some();",
    "    let mut combined = String::new();\n    let mut run_commands = Vec::new();\n    let has_services = mapping_get(job, \"services\").is_some();",
    "collect exact run commands",
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
    "reject all caller-selected job environments",
)
text = replace_once(
    text,
    '''        if contains_secret_expression(mapping_get(step, "env"))
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
        }''',
    "step environment, expression, and action hardening",
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
    "exact hardened profile selection",
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
    "remove substring-only hardened classifier",
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
            || compact.contains("github[\\\"token\\\"]")
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
''',
    "secret, immutable action, and exact command helpers",
)
text = replace_at_least(
    text,
    "@abc",
    f"@{IMMUTABLE_TEST_REF}",
    4,
    "pin planner unit-test setup actions",
)
text = replace_once(
    text,
    '''      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020
      - run: npm ci && npm test
"#,
        );
        input.repository = "messaging-intel/msgint-connectors".into();''',
    '''      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020
      - run: |
          npm ci --ignore-scripts
          npm test
"#,
        );
        input.repository = "messaging-intel/msgint-connectors".into();''',
    "harden full Messaging Intel unit-test job",
)
text = replace_once(
    text,
    '''        assert_eq!(
            plan.jobs[1].independent_profile.as_deref(),
            Some("node-verify")
        );
    }

    #[test]
    fn hardened_node_profile_requires_complete_reviewed_evidence() {''',
    '''        assert_eq!(
            plan.jobs[1].independent_profile.as_deref(),
            Some("node-hardened-test")
        );
    }

    #[test]
    fn hardened_node_profile_requires_complete_reviewed_evidence() {''',
    "full Messaging Intel profile assertion",
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
    fn review_order_is_deterministic_for_parallel_roots() {''',
    "planner adversarial tests",
)
path.write_text(text, encoding="utf-8")


# The bounded fixture must match the fixed profile exactly and suppress npm
# lifecycle scripts for both jobs.
path = Path("remote/deployments/gha-clone-server-rs/tests/fixtures/msgint-operator-config.yml")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''      - run: |
          npm ci
          npm test
''',
    '''      - run: |
          npm ci --ignore-scripts
          npm test
''',
    "harden full Messaging Intel fixture job",
)
path.write_text(text, encoding="utf-8")


# Prove the real server dispatches only the two exact fixed profiles and that
# nearby adversarial workflows create zero additional build submissions.
path = Path("remote/deployments/gha-clone-server-rs/tests/msgint_operator_config.rs")
text = path.read_text(encoding="utf-8")
if text.count('"node-verify"') != 2:
    raise RuntimeError(
        f"Messaging Intel integration: expected two node-verify assertions, found {text.count(chr(34) + 'node-verify' + chr(34))}"
    )
text = text.replace('"node-verify"', '"node-hardened-test"')
text = replace_once(
    text,
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
        "          npm audit --audit-level=high\\n",
        "          npm audit --audit-level=high\\n          npm publish\\n",
        1,
    );
    let mutable_action = workflow_yaml.replacen(
        "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
        "actions/checkout@main",
        1,
    );
    let bracket_secret = workflow_yaml.replacen(
        "          persist-credentials: false\\n",
        "          persist-credentials: false\\n        env:\\n          MSGINT_META_ACCESS_TOKEN: ${{ secrets['PROD_TOKEN'] }}\\n",
        1,
    );

    for (label, rejected_yaml, expected_reason) in [
        (
            "extra hardened command",
            extra_command,
            "exact reviewed command sequence",
        ),
        (
            "mutable setup action",
            mutable_action,
            "exact 40-hex commit SHA",
        ),
        ("bracket secret expression", bracket_secret, "secret-bearing"),
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
        let rejected: Value = response
            .json()
            .await
            .unwrap_or_else(|error| panic!("read rejected {label} response: {error}"));
        assert_eq!(rejected["error"], "workflow is not independently executable");
        assert!(
            rejected.to_string().contains(expected_reason),
            "{label} response did not explain {expected_reason}: {rejected}"
        );
        assert_eq!(
            mock_state.submissions.lock().await.len(),
            2,
            "{label} dispatched a build despite rejection"
        );
    }

    mock_task.abort();''',
    "real-server adversarial rejection proof",
)
path.write_text(text, encoding="utf-8")


# Keep the private repository smoke manual-only and switch its second execution
# to the lifecycle-script-free fixed profile.
path = Path(".github/workflows/gha-clone-server.yml")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '("node-verify", "NODE_VERIFY_STEPS"),',
    '("node-hardened-test", "NODE_HARDENED_TEST_STEPS"),',
    "manual smoke profile extraction",
)
text = replace_once(
    text,
    "for profile in node-hardened-verify node-verify; do",
    "for profile in node-hardened-verify node-hardened-test; do",
    "manual smoke execution profiles",
)
if "if: ${{ github.event_name == 'workflow_dispatch' && inputs.run_msgint_profile_smoke }}" not in text:
    raise RuntimeError("manual Messaging Intel smoke guard is missing")
path.write_text(text, encoding="utf-8")


# Register the second fixed profile and preserve the exact canonical repository
# admission rule.
path = Path("remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "rust-verify,node-verify,node-hardened-verify,python-verify",
    "rust-verify,node-verify,node-hardened-verify,node-hardened-test,python-verify",
    "deployed hardened test profile",
)
if "=https://github.com/messaging-intel/msgint-connectors.git" not in text:
    raise RuntimeError("exact Messaging Intel profile repository rule is missing")
path.write_text(text, encoding="utf-8")


path = Path("remote/deployments/build-server-rs/readme.md")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "| `node-hardened-verify` | npm lifecycle-script suppression, operator checks, and high-severity audit | none |",
    "| `node-hardened-verify` | npm lifecycle-script suppression, operator checks, and high-severity audit | none |\n| `node-hardened-test` | npm lifecycle-script suppression and complete repository tests | none |",
    "build-server profile table",
)
path.write_text(text, encoding="utf-8")

path = Path("remote/deployments/gha-clone-server-rs/README.md")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "| npm install-script suppression + operator checks + high-severity audit | `node-hardened-verify` |",
    "| npm install-script suppression + operator checks + high-severity audit | `node-hardened-verify` |\n| npm install-script suppression + complete repository tests | `node-hardened-test` |",
    "GHA clone profile table",
)
text = replace_once(
    text,
    "- secret/OIDC expressions in `env`, `with`, or commands;",
    "- every caller-selected job/step environment, plus secret/OIDC expressions in `with` or commands;\n- mutable setup-action references and expressions inside setup inputs;",
    "fail-closed exclusions",
)
path.write_text(text, encoding="utf-8")


# Static contracts make the exact command, immutable action, manual private
# smoke, and exact repository boundaries load-bearing.
path = Path("remote/tests/general/gha-clone-server-config.test.ts")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "    'node-hardened-verify',\n    'python-verify',",
    "    'node-hardened-verify',\n    'node-hardened-test',\n    'python-verify',",
    "profile contract list",
)
text = replace_once(
    text,
    "  assert.match(continuityPatch, /node-hardened-verify/);",
    "  assert.match(continuityPatch, /node-hardened-verify/);\n  assert.match(continuityPatch, /node-hardened-test/);",
    "deployed hardened profile contract",
)
text = replace_once(
    text,
    '''  assert.match(
    continuityPatch,
    /https:\\/\\/github\\.com\\/messaging-intel\\/msgint-connectors\\.git/,
  );''',
    '''  assert.match(
    continuityPatch,
    /=https:\\/\\/github\\.com\\/messaging-intel\\/msgint-connectors\\.git/,
  );''',
    "exact repository rule contract",
)
text = replace_once(
    text,
    "  assert.match(planner, /secret-bearing env\\/with values are unsupported/);",
    "  assert.match(planner, /fixed profiles do not forward caller-selected variables/);\n  assert.match(planner, /secret-bearing setup inputs are unsupported/);",
    "planner environment contract",
)
text = replace_once(
    text,
    "  assert.match(planner, /node-hardened-verify/);",
    "  assert.match(planner, /node-hardened-verify/);\n  assert.match(planner, /node-hardened-test/);\n  assert.match(planner, /exact reviewed command sequence/);\n  assert.match(planner, /exact 40-hex commit SHA/);",
    "planner hardening contracts",
)
text = replace_once(
    text,
    "  assert.match(integration, /node-verify/);",
    "  assert.match(integration, /node-hardened-test/);\n  assert.match(integration, /UNPROCESSABLE_ENTITY/);\n  assert.match(integration, /npm publish/);\n  assert.match(integration, /secrets\\['PROD_TOKEN'\\]/);",
    "Messaging Intel integration contract",
)
text = replace_once(
    text,
    "  assert.match(workflow, /npm test/);\n  assert.equal((workflow.match(/persist-credentials:\\s*false/g) ?? []).length, 2);",
    "  assert.match(workflow, /npm test/);\n  assert.equal((workflow.match(/npm ci --ignore-scripts/g) ?? []).length, 2);\n  assert.equal((workflow.match(/persist-credentials:\\s*false/g) ?? []).length, 2);",
    "fixture exact install contract",
)
text = replace_once(
    text,
    "  assert.match(workflow, /persist-credentials:\\s*false/);\n});",
    "  assert.match(workflow, /persist-credentials:\\s*false/);\n  assert.match(workflow, /run_msgint_profile_smoke/);\n  assert.match(workflow, /create-github-app-token@/);\n  assert.match(workflow, /node-hardened-test/);\n  assert.doesNotMatch(workflow, /rm -rf|ghp_|github_pat_/);\n});",
    "manual private smoke contract",
)
path.write_text(text, encoding="utf-8")
