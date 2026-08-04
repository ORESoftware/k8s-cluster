from pathlib import Path

BRANCH = "agent/msgint-gha-clone-operator-verify-20260804"
IMMUTABLE_TEST_REF = "0123456789abcdef0123456789abcdef01234567"


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


def replace_at_least(text: str, old: str, new: str, minimum: int, label: str) -> str:
    count = text.count(old)
    if count < minimum:
        raise RuntimeError(f"{label}: expected at least {minimum} matches, found {count}")
    return text.replace(old, new)


# Harden the independent-lane compiler.
path = Path("remote/deployments/gha-clone-server-rs/src/lib.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "    let mut combined = String::new();\n    let has_services = mapping_get(job, \"services\").is_some();",
    "    let mut combined = String::new();\n    let mut run_commands = Vec::new();\n    let has_services = mapping_get(job, \"services\").is_some();",
    "collect run commands",
)
text = replace_once(
    text,
    "    if contains_secret_expression(mapping_get(job, \"env\")) {\n        reasons.push(\"job environment contains a secret expression\".into());\n    }",
    "    if let Some(environment) = mapping_get(job, \"env\") {\n        if contains_secret_expression(Some(environment)) {\n            reasons.push(\"job environment contains a secret expression\".into());\n        } else {\n            reasons.push(\n                \"job environment is unsupported because fixed profiles do not forward caller-selected variables\"\n                    .into(),\n            );\n        }\n    }",
    "reject all job environments",
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
        let text = compact_yaml(value).to_ascii_lowercase();
        let compact = text
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
    const FULL: [&str; 4] = [
        "npm ci --ignore-scripts",
        "npm run check",
        "npm test",
        "npm audit --audit-level=high",
    ];
    if commands.iter().map(String::as_str).eq(OPERATOR) {
        Some("node-hardened-verify")
    } else if commands.iter().map(String::as_str).eq(FULL) {
        Some("node-hardened-full-verify")
    } else {
        None
    }
}
''',
    "secret and immutable action helpers",
)
text = replace_at_least(text, "@abc", f"@{IMMUTABLE_TEST_REF}", 4, "immutable test actions")
text = replace_once(
    text,
    '''      - run: npm ci && npm test
"#,
        );''',
    '''      - run: |
          npm ci --ignore-scripts
          npm run check
          npm test
          npm audit --audit-level=high
"#,
        );''',
    "full Messaging Intel profile fixture in unit test",
)
text = replace_once(
    text,
    '''        assert_eq!(
            plan.jobs[1].independent_profile.as_deref(),
            Some("node-verify")
        );''',
    '''        assert_eq!(
            plan.jobs[1].independent_profile.as_deref(),
            Some("node-hardened-full-verify")
        );''',
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
    fn hardened_node_profiles_reject_extra_reordered_and_spoofed_commands() {
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
    "hardened planner tests",
)
path.write_text(text, encoding="utf-8")


# Add a lifecycle-script-free full repository profile.
path = Path("remote/deployments/build-server-rs/src/profiles.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''const PYTHON_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {''',
    '''const NODE_HARDENED_FULL_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {
    name: "Hardened Node full repository verification",
    image: NODE_IMAGE,
    subdirectory: ".",
    script: r#"set -euo pipefail
if [ ! -f package-lock.json ] && [ ! -f npm-shrinkwrap.json ]; then
  echo "node-hardened-full-verify requires package-lock.json or npm-shrinkwrap.json" >&2
  exit 2
fi
npm ci --ignore-scripts
npm run check
npm test
npm audit --audit-level=high"#,
}];

const PYTHON_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {''',
    "full hardened profile steps",
)
text = replace_once(
    text,
    '''    ProfileSpec {
        name: "python-verify",''',
    '''    ProfileSpec {
        name: "node-hardened-full-verify",
        platform: "linux",
        description:
            "Lifecycle-script-free Node syntax checks, full tests, and high-severity audit",
        steps: NODE_HARDENED_FULL_VERIFY_STEPS,
        artifact_paths: &[],
    },
    ProfileSpec {
        name: "python-verify",''',
    "full hardened profile registration",
)
text = replace_once(
    text,
    '''            "node-hardened-verify",
            "python-verify",''',
    '''            "node-hardened-verify",
            "node-hardened-full-verify",
            "python-verify",''',
    "profile installation test",
)
text = replace_once(
    text,
    '''    #[test]
    fn rust_verify_has_only_the_reviewed_meta_server_monorepo_fallback() {''',
    '''    #[test]
    fn hardened_full_node_profile_is_ordered_and_supply_chain_bounded() {
        let profile = find("node-hardened-full-verify").expect("hardened full Node profile");
        let script = profile.steps[0].script;
        assert_eq!(profile.steps[0].subdirectory, ".");
        let install = script
            .find("npm ci --ignore-scripts")
            .expect("install step");
        let check = script.find("npm run check").expect("check step");
        let full = script.find("npm test").expect("full test step");
        let audit = script
            .find("npm audit --audit-level=high")
            .expect("audit step");
        assert!(install < check && check < full && full < audit);
        assert!(script.contains("package-lock.json"));
        assert!(!script.contains("npm install"));
        assert!(!script.contains("|| true"));
        assert!(!script.contains("--force"));
        assert!(!script.contains("curl"));
        assert!(!script.contains("wget"));
    }

    #[test]
    fn rust_verify_has_only_the_reviewed_meta_server_monorepo_fallback() {''',
    "full hardened profile tests",
)
path.write_text(text, encoding="utf-8")


# Mirror both reviewed command sequences exactly.
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
          npm run check
          npm test
          npm audit --audit-level=high
''',
    "full fixture command sequence",
)
path.write_text(text, encoding="utf-8")


# Extend the real-server proof with zero-dispatch adversarial requests.
path = Path("remote/deployments/gha-clone-server-rs/tests/msgint_operator_config.rs")
text = path.read_text(encoding="utf-8")
text = text.replace('"node-verify"', '"node-hardened-full-verify"')
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
    "real-server adversarial rejection cases",
)
path.write_text(text, encoding="utf-8")


# Replace the opaque unauthenticated clone with an explicit least-privilege App gate.
path = Path(".github/workflows/gha-clone-server.yml")
text = path.read_text(encoding="utf-8")
start = text.index("  msgint-profile-smoke:\n")
end = text.index("  contracts:\n", start)
job = '''  msgint-profile-smoke:
    needs: [rust, build-server-profile]
    runs-on: ubuntu-latest
    timeout-minutes: 30
    env:
      MSGINT_REPOSITORY: https://github.com/messaging-intel/msgint-connectors.git
      MSGINT_REVISION: 7d905806b2000479bdacb9b206f33b26a707ba5e
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - name: Mint exact-repository read token
        id: mint-msgint-token
        env:
          MSGINT_REPOSITORY_READ_APP_ID: ${{ secrets.MSGINT_REPOSITORY_READ_APP_ID }}
          MSGINT_REPOSITORY_READ_APP_PRIVATE_KEY: ${{ secrets.MSGINT_REPOSITORY_READ_APP_PRIVATE_KEY }}
        run: |
          set -euo pipefail
          if [[ ! "$MSGINT_REPOSITORY_READ_APP_ID" =~ ^[0-9]+$ ]] || [[ -z "$MSGINT_REPOSITORY_READ_APP_PRIVATE_KEY" ]]; then
            echo "::error title=Messaging Intel repository access missing::Install a contents-read GitHub App on messaging-intel/msgint-connectors and configure MSGINT_REPOSITORY_READ_APP_ID plus MSGINT_REPOSITORY_READ_APP_PRIVATE_KEY in ORESoftware/k8s-cluster" >&2
            exit 2
          fi
          token_file="$RUNNER_TEMP/msgint-repository-read-token"
          K8S_SUBMODULE_APP_ID="$MSGINT_REPOSITORY_READ_APP_ID" \
          K8S_SUBMODULE_APP_PRIVATE_KEY="$MSGINT_REPOSITORY_READ_APP_PRIVATE_KEY" \
            scripts/ci/mint-github-app-installation-token.sh \
              messaging-intel "$token_file" msgint-connectors
          printf 'token_file=%s\n' "$token_file" >>"$GITHUB_OUTPUT"
      - name: Prepare exact fixed profiles and immutable repository checkout
        id: prepare-msgint
        env:
          MSGINT_TOKEN_FILE: ${{ steps.mint-msgint-token.outputs.token_file }}
        run: |
          set -euo pipefail
          printf '%s' "$MSGINT_REVISION" | grep -Eq '^[a-f0-9]{40}$'
          test -s "$MSGINT_TOKEN_FILE"
          auth_dir="$(mktemp -d "$RUNNER_TEMP/msgint-git-auth.XXXXXX")"
          repo_dir="$(mktemp -d "$RUNNER_TEMP/msgint-connectors.XXXXXX")"
          askpass="$auth_dir/askpass.sh"
          cleanup() {
            find "$auth_dir" -depth -delete
            if [[ -e "$MSGINT_TOKEN_FILE" ]]; then
              find "$MSGINT_TOKEN_FILE" -delete
            fi
          }
          trap cleanup EXIT
          cat >"$askpass" <<'ASKPASS'
          #!/usr/bin/env bash
          case "${1:-}" in
            *Username*) printf '%s\n' x-access-token ;;
            *Password*) cat "${MSGINT_TOKEN_FILE:?}" ;;
            *) exit 1 ;;
          esac
          ASKPASS
          chmod 500 "$askpass"
          export GIT_ASKPASS="$askpass"
          export GIT_TERMINAL_PROMPT=0
          git clone --filter=blob:none --no-checkout "$MSGINT_REPOSITORY" "$repo_dir"
          git -C "$repo_dir" fetch --depth=1 origin "$MSGINT_REVISION"
          git -C "$repo_dir" checkout --detach "$MSGINT_REVISION"
          test "$(git -C "$repo_dir" rev-parse HEAD)" = "$MSGINT_REVISION"
          python3 <<'PY'
          import os
          import re
          from pathlib import Path

          source = Path("remote/deployments/build-server-rs/src/profiles.rs").read_text(encoding="utf-8")
          image_match = re.search(r'const NODE_IMAGE: &str = "([^"]+)";', source)
          if image_match is None:
              raise SystemExit("NODE_IMAGE was not found")

          output_dir = Path(os.environ["RUNNER_TEMP"])
          for profile, constant in (
              ("node-hardened-verify", "NODE_HARDENED_VERIFY_STEPS"),
              ("node-hardened-full-verify", "NODE_HARDENED_FULL_VERIFY_STEPS"),
          ):
              pattern = rf'const {constant}:.*?script: r#"(.*?)"#,\n\}}\];'
              match = re.search(pattern, source, flags=re.DOTALL)
              if match is None:
                  raise SystemExit(f"{constant} script was not found")
              script_path = output_dir / f"{profile}.sh"
              script_path.write_text(match.group(1) + "\n", encoding="utf-8")
              script_path.chmod(0o500)

          with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as output:
              output.write(f"image={image_match.group(1)}\n")
              output.write(f"repo_dir={os.environ['RUNNER_TEMP']}/" + Path(os.environ["MSGINT_REPO_BASENAME"]).name + "\n" if os.environ.get("MSGINT_REPO_BASENAME") else "")
          PY
          printf 'repo_dir=%s\n' "$repo_dir" >>"$GITHUB_OUTPUT"
      - name: Execute both fixed profiles in the reviewed Node image
        env:
          PROFILE_IMAGE: ${{ steps.prepare-msgint.outputs.image }}
          MSGINT_REPO_DIR: ${{ steps.prepare-msgint.outputs.repo_dir }}
        run: |
          set -euo pipefail
          docker pull "$PROFILE_IMAGE"
          resolved_image="$(docker image inspect --format='{{index .RepoDigests 0}}' "$PROFILE_IMAGE")"
          case "$resolved_image" in
            *@sha256:*) ;;
            *) echo "Node profile image did not resolve to an immutable digest" >&2; exit 1 ;;
          esac
          printf 'resolved_node_profile_image=%s\n' "$resolved_image"
          for profile in node-hardened-verify node-hardened-full-verify; do
            script="$RUNNER_TEMP/${profile}.sh"
            test -s "$script"
            docker run --rm \
              --pull=never \
              --cap-drop=ALL \
              --security-opt=no-new-privileges \
              --pids-limit=512 \
              --memory=4g \
              --cpus=2 \
              --read-only \
              --tmpfs /tmp:rw,nosuid,nodev,noexec,size=1g \
              --network=bridge \
              -e CI=true \
              -e HOME=/tmp/home \
              -v "$MSGINT_REPO_DIR:/workspace:rw" \
              -v "$script:/profile.sh:ro" \
              -w /workspace \
              "$resolved_image" \
              bash /profile.sh
          done

'''
# Remove a dead optional expression from the generated job before writing it.
job = job.replace(
    '              output.write(f"repo_dir={os.environ[\'RUNNER_TEMP\']}/" + Path(os.environ["MSGINT_REPO_BASENAME"]).name + "\\n" if os.environ.get("MSGINT_REPO_BASENAME") else "")\n',
    "",
)
text = text[:start] + job + text[end:]
path.write_text(text, encoding="utf-8")


# Register the second fixed profile in the deployed build-server allowlist.
path = Path("remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "rust-verify,node-verify,node-hardened-verify,python-verify",
    "rust-verify,node-verify,node-hardened-verify,node-hardened-full-verify,python-verify",
    "deployed full hardened profile",
)
path.write_text(text, encoding="utf-8")


# Keep operator documentation aligned.
path = Path("remote/deployments/build-server-rs/readme.md")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "| `node-hardened-verify` | npm lifecycle-script suppression, operator checks, and high-severity audit | none |",
    "| `node-hardened-verify` | npm lifecycle-script suppression, operator checks, and high-severity audit | none |\n| `node-hardened-full-verify` | npm lifecycle-script suppression, syntax checks, full tests, and high-severity audit | none |",
    "build-server profile table",
)
path.write_text(text, encoding="utf-8")

path = Path("remote/deployments/gha-clone-server-rs/README.md")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "Intel uses a dedicated two-job mirror: `node-hardened-verify` for the non-secret\noperator contract and `node-verify` for the complete repository test suite.",
    "Intel uses a dedicated two-job mirror: `node-hardened-verify` for the non-secret\noperator contract and `node-hardened-full-verify` for lifecycle-script-free full\nrepository checks, tests, and dependency audit.",
    "GHA clone Messaging Intel overview",
)
text = replace_once(
    text,
    "| npm install-script suppression + operator checks + high-severity audit | `node-hardened-verify` |",
    "| npm install-script suppression + operator checks + high-severity audit | `node-hardened-verify` |\n| npm install-script suppression + syntax checks + full tests + high-severity audit | `node-hardened-full-verify` |",
    "GHA clone profile table",
)
path.write_text(text, encoding="utf-8")


# Extend static deployment/workflow contracts.
path = Path("remote/tests/general/gha-clone-server-config.test.ts")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "    'node-hardened-verify',\n    'python-verify',",
    "    'node-hardened-verify',\n    'node-hardened-full-verify',\n    'python-verify',",
    "profile contract list",
)
text = replace_once(
    text,
    "  assert.match(planner, /node-hardened-verify/);",
    "  assert.match(planner, /node-hardened-verify/);\n  assert.match(planner, /node-hardened-full-verify/);\n  assert.match(planner, /exact reviewed command sequence/);\n  assert.match(planner, /exact 40-hex commit SHA/);",
    "planner hardening contracts",
)
text = replace_once(
    text,
    "  assert.match(integration, /node-verify/);",
    "  assert.match(integration, /node-hardened-full-verify/);\n  assert.match(integration, /UNPROCESSABLE_ENTITY/);\n  assert.match(integration, /npm publish/);\n  assert.match(integration, /secrets\['PROD_TOKEN'\]/);",
    "Messaging Intel integration contract",
)
text = replace_once(
    text,
    "  assert.match(workflow, /npm test/);\n  assert.equal((workflow.match(/persist-credentials:\\s*false/g) ?? []).length, 2);",
    "  assert.match(workflow, /npm test/);\n  assert.equal((workflow.match(/npm ci --ignore-scripts/g) ?? []).length, 2);\n  assert.equal((workflow.match(/npm audit --audit-level=high/g) ?? []).length, 2);\n  assert.equal((workflow.match(/persist-credentials:\\s*false/g) ?? []).length, 2);",
    "fixture exact sequences",
)
text = replace_once(
    text,
    "  assert.match(workflow, /persist-credentials:\\s*false/);\n});",
    "  assert.match(workflow, /persist-credentials:\\s*false/);\n  assert.match(workflow, /MSGINT_REPOSITORY_READ_APP_ID/);\n  assert.match(workflow, /MSGINT_REPOSITORY_READ_APP_PRIVATE_KEY/);\n  assert.match(workflow, /mint-github-app-installation-token\\.sh/);\n  assert.match(workflow, /node-hardened-full-verify/);\n  assert.doesNotMatch(workflow, /rm -rf|ghp_|github_pat_/);\n});",
    "hosted private repository access contract",
)
path.write_text(text, encoding="utf-8")
