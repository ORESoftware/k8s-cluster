from pathlib import Path

TEST_REF = "0123456789abcdef0123456789abcdef01234567"
MSGINT_REVISION = "2ef2e4a1a5762b5289474f3da85ddf838b41bf3f"
NODE_IMAGE = (
    "docker.io/library/node:22.23.1-bookworm@"
    "sha256:5647be709086c696ff32edaaf1c70cd26d1da6ab2b39c32f3c7b4c4a31957e37"
)


def read(path: str) -> str:
    return Path(path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    Path(path).write_text(text, encoding="utf-8")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


def replace_count(text: str, old: str, new: str, expected: int, label: str) -> str:
    count = text.count(old)
    if count != expected:
        raise RuntimeError(f"{label}: expected {expected} matches, found {count}")
    return text.replace(old, new)


def replace_section(text: str, start: str, end: str, replacement: str, label: str) -> str:
    start_index = text.find(start)
    if start_index < 0:
        raise RuntimeError(f"{label}: start marker not found")
    end_index = text.find(end, start_index + len(start))
    if end_index < 0:
        raise RuntimeError(f"{label}: end marker not found")
    return text[:start_index] + replacement + text[end_index:]


# The first-stage transformer installs generic fail-closed planner primitives.
# This phase binds them to the exact Messaging Intel commands and workflow shape.
planner_path = "remote/deployments/gha-clone-server-rs/src/lib.rs"
text = read(planner_path)
text = replace_once(
    text,
    '''fn hardened_node_profile(commands: &[String]) -> Option<&'static str> {
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
    '''fn hardened_node_profile(commands: &[String]) -> Option<&'static str> {
    const OPERATOR: [&str; 5] = [
        "export npm_config_ignore_scripts=true",
        "npm ci --ignore-scripts --no-audit --no-fund",
        "npm run check",
        "npm run test:operator-config",
        "npm audit --audit-level=high",
    ];
    const FULL_TEST: [&str; 5] = [
        "export npm_config_ignore_scripts=true",
        "npm ci --ignore-scripts --no-audit --no-fund",
        "npm run check",
        "npm test",
        "npm audit --audit-level=high",
    ];
    if commands.iter().map(String::as_str).eq(OPERATOR) {
        Some("node-hardened-verify")
    } else if commands.iter().map(String::as_str).eq(FULL_TEST) {
        Some("node-hardened-test")
    } else {
        None
    }
}
''',
    "exact hardened Node commands",
)
text = replace_once(
    text,
    '''    let topological_order = validate_dependencies(&plans, &job_ids)?;
    let immutable_revision = is_full_commit_sha(&request.revision);''',
    '''    apply_messaging_intel_contract(request, &mut plans)?;
    let topological_order = validate_dependencies(&plans, &job_ids)?;
    let immutable_revision = is_full_commit_sha(&request.revision);''',
    "Messaging Intel contract hook",
)
contract = '''fn apply_messaging_intel_contract(
    request: &PlanRequest,
    jobs: &mut [JobPlan],
) -> Result<(), Vec<String>> {
    if request.repository != "messaging-intel/msgint-connectors"
        || request.workflow_path != ".github/workflows/gha-clone-operator-config.yml"
    {
        return Ok(());
    }

    if jobs.len() != 2 {
        return Err(vec![
            "Messaging Intel continuity workflow must contain exactly operator_config and repository_tests"
                .to_string(),
        ]);
    }
    let Some(operator_index) = jobs.iter().position(|job| job.id == "operator_config") else {
        return Err(vec![
            "Messaging Intel continuity workflow is missing operator_config".to_string(),
        ]);
    };
    let Some(repository_index) = jobs.iter().position(|job| job.id == "repository_tests") else {
        return Err(vec![
            "Messaging Intel continuity workflow is missing repository_tests".to_string(),
        ]);
    };
    let mut errors = Vec::new();
    if !jobs[operator_index].needs.is_empty() {
        errors.push("Messaging Intel operator_config must not depend on another job".to_string());
    }
    if jobs[repository_index].needs.len() != 1
        || jobs[repository_index].needs[0] != "operator_config"
    {
        errors.push(
            "Messaging Intel repository_tests must depend only on operator_config".to_string(),
        );
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    for (index, expected_profile) in [
        (operator_index, "node-hardened-verify"),
        (repository_index, "node-hardened-test"),
    ] {
        if jobs[index].independent_profile.as_deref() != Some(expected_profile) {
            jobs[index].independent_supported = false;
            jobs[index].independent_profile = None;
            jobs[index].independent_reasons.push(format!(
                "Messaging Intel job {} must map exactly to {expected_profile}",
                jobs[index].id
            ));
        }
    }
    Ok(())
}

'''
text = replace_once(
    text,
    "fn validate_dependencies(\n",
    contract + "fn validate_dependencies(\n",
    "Messaging Intel exact workflow contract",
)
map_start = '''    #[test]
    fn maps_messaging_intel_operator_workflow_to_hardened_and_full_profiles() {'''
map_end = '''    #[test]
    fn hardened_node_profile_requires_complete_reviewed_evidence() {'''
map_test = f'''    #[test]
    fn maps_messaging_intel_operator_workflow_to_hardened_and_full_profiles() {{
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
          node-version: '22.23.1'
          cache: npm
      - run: |
          export npm_config_ignore_scripts=true
          npm ci --ignore-scripts --no-audit --no-fund
          npm run check
          npm run test:operator-config
          npm audit --audit-level=high
  repository_tests:
    needs: operator_config
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
        with:
          persist-credentials: false
      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020
        with:
          node-version: '22.23.1'
          cache: npm
      - run: |
          export npm_config_ignore_scripts=true
          npm ci --ignore-scripts --no-audit --no-fund
          npm run check
          npm test
          npm audit --audit-level=high
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
    }}

'''
text = replace_section(text, map_start, map_end, map_test, "Messaging Intel mapping unit test")
incomplete_start = map_end
incomplete_end = '''    #[test]
    fn hardened_node_profiles_reject_spoofed_extra_and_reordered_commands() {'''
incomplete_test = '''    #[test]
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
          npm ci --ignore-scripts --no-audit --no-fund
          npm run check
          npm run test:operator-config
          npm audit --audit-level=high
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

'''
text = replace_section(
    text,
    incomplete_start,
    incomplete_end,
    incomplete_test,
    "incomplete hardened evidence unit test",
)
adversarial_end = '''    #[test]
    fn review_order_is_deterministic_for_parallel_roots() {'''
adversarial_tests = '''    #[test]
    fn hardened_node_profiles_reject_spoofed_extra_and_reordered_commands() {
        for run in [
            r#"echo 'export npm_config_ignore_scripts=true npm ci --ignore-scripts --no-audit --no-fund npm run check npm run test:operator-config npm audit --audit-level=high'"#,
            r#"export npm_config_ignore_scripts=true
npm ci --ignore-scripts --no-audit --no-fund
npm run check
npm run test:operator-config
npm audit --audit-level=high
npm publish"#,
            r#"export npm_config_ignore_scripts=true
npm run check
npm ci --ignore-scripts --no-audit --no-fund
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
    fn environments_and_setup_expressions_fail_closed() {
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
  expression:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/setup-node@0123456789abcdef0123456789abcdef01234567
        with:
          cache: ${{ github.ref }}
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
        assert!(plan.jobs[2]
            .independent_reasons
            .iter()
            .any(|reason| reason.contains("expressions in setup inputs")));
    }

    #[test]
    fn messaging_intel_contract_rejects_wrong_job_sets_and_profiles() {
        let mut missing = request(
            r#"
jobs:
  operator_config:
    runs-on: ubuntu-latest
    steps:
      - run: npm test
"#,
        );
        missing.repository = "messaging-intel/msgint-connectors".into();
        missing.workflow_path = ".github/workflows/gha-clone-operator-config.yml".into();
        let errors = build_plan(&missing, &PlannerLimits::default())
            .unwrap_err()
            .join("\n");
        assert!(errors.contains("exactly operator_config and repository_tests"));

        let mut generic = request(
            r#"
jobs:
  operator_config:
    runs-on: ubuntu-latest
    steps:
      - run: npm test
  repository_tests:
    needs: operator_config
    runs-on: ubuntu-latest
    steps:
      - run: npm test
"#,
        );
        generic.repository = "messaging-intel/msgint-connectors".into();
        generic.workflow_path = ".github/workflows/gha-clone-operator-config.yml".into();
        let plan = build_plan(&generic, &PlannerLimits::default()).expect("valid plan");
        assert!(!plan.independent_executable);
        let reasons = plan
            .jobs
            .iter()
            .flat_map(|job| job.independent_reasons.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(reasons.contains("must map exactly to node-hardened-verify"));
        assert!(reasons.contains("must map exactly to node-hardened-test"));
    }

'''
text = replace_section(
    text,
    incomplete_end,
    adversarial_end,
    adversarial_tests,
    "adversarial planner tests",
)
write(planner_path, text)


# Pin the fixed Node runtime and make lifecycle-script suppression apply to
# install, pre/post-test hooks, the focused suite, and the full suite.
profiles_path = "remote/deployments/build-server-rs/src/profiles.rs"
text = read(profiles_path)
text = replace_once(
    text,
    'const NODE_IMAGE: &str = "docker.io/library/node:22-bookworm";',
    f'const NODE_IMAGE: &str = "{NODE_IMAGE}";',
    "immutable Node runner image",
)
text = replace_once(
    text,
    '''    script: r#"set -euo pipefail
if [ ! -f package-lock.json ] && [ ! -f npm-shrinkwrap.json ]; then
  echo "node-hardened-verify requires package-lock.json or npm-shrinkwrap.json" >&2
  exit 2
fi
npm ci --ignore-scripts
npm run check
npm run test:operator-config
npm audit --audit-level=high"#,''',
    '''    script: r#"set -euo pipefail
if [ ! -f package-lock.json ] && [ ! -f npm-shrinkwrap.json ]; then
  echo "node-hardened-verify requires package-lock.json or npm-shrinkwrap.json" >&2
  exit 2
fi
export npm_config_ignore_scripts=true
npm ci --ignore-scripts --no-audit --no-fund
npm run check
npm run test:operator-config
npm audit --audit-level=high"#,''',
    "hardened operator profile",
)
text = replace_once(
    text,
    '''    script: r#"set -euo pipefail
if [ ! -f package-lock.json ] && [ ! -f npm-shrinkwrap.json ]; then
  echo "node-hardened-test requires package-lock.json or npm-shrinkwrap.json" >&2
  exit 2
fi
npm ci --ignore-scripts
npm test"#,''',
    '''    script: r#"set -euo pipefail
if [ ! -f package-lock.json ] && [ ! -f npm-shrinkwrap.json ]; then
  echo "node-hardened-test requires package-lock.json or npm-shrinkwrap.json" >&2
  exit 2
fi
export npm_config_ignore_scripts=true
npm ci --ignore-scripts --no-audit --no-fund
npm run check
npm test
npm audit --audit-level=high"#,''',
    "hardened full repository profile",
)
text = replace_once(
    text,
    '''        description: "Lifecycle-script-free lockfile install and complete Node repository tests",''',
    '''        description:
            "Lifecycle-script-free Node syntax checks, complete tests, and high-severity audit",''',
    "hardened full profile description",
)
every_start = '''    #[test]
    fn every_profile_is_linux_fixed_and_bounded() {'''
every_end = '''    #[test]
    fn continuity_profiles_are_installed() {'''
every_test = '''    #[test]
    fn every_profile_is_linux_fixed_and_bounded() {
        for profile in SPECS {
            assert_eq!(profile.platform, "linux");
            assert!(!profile.steps.is_empty());
            for step in profile.steps {
                assert!(!step.name.is_empty());
                assert!(!step.image.ends_with(":latest"));
                assert!(!step.script.trim().is_empty());
                assert!(!step.script.contains("curl | sh"));
                assert!(!step.script.contains("wget | sh"));
                if profile.name.starts_with("node-") {
                    assert_eq!(step.image, NODE_IMAGE);
                    assert!(step.image.contains("@sha256:"));
                }
            }
        }
    }

'''
text = replace_section(text, every_start, every_end, every_test, "profile image tests")
operator_start = '''    #[test]
    fn hardened_node_profile_is_ordered_and_supply_chain_bounded() {'''
operator_end = '''    #[test]
    fn hardened_node_test_profile_disables_lifecycle_scripts() {'''
operator_test = '''    #[test]
    fn hardened_node_profile_is_ordered_and_supply_chain_bounded() {
        let profile = find("node-hardened-verify").expect("hardened Node profile");
        let script = profile.steps[0].script;
        assert_eq!(profile.steps[0].subdirectory, ".");
        let exported = script
            .find("export npm_config_ignore_scripts=true")
            .expect("lifecycle suppression export");
        let install = script
            .find("npm ci --ignore-scripts --no-audit --no-fund")
            .expect("install step");
        let check = script.find("npm run check").expect("check step");
        let focused = script
            .find("npm run test:operator-config")
            .expect("focused test step");
        let audit = script
            .find("npm audit --audit-level=high")
            .expect("audit step");
        assert!(exported < install && install < check && check < focused && focused < audit);
        assert!(script.contains("package-lock.json"));
        assert!(!script.contains("npm install"));
        assert!(!script.contains("|| true"));
        assert!(!script.contains("--force"));
        assert!(!script.contains("curl"));
        assert!(!script.contains("wget"));
    }

'''
text = replace_section(text, operator_start, operator_end, operator_test, "operator profile tests")
full_start = operator_end
full_end = '''    #[test]
    fn rust_verify_has_only_the_reviewed_meta_server_monorepo_fallback() {'''
full_test = '''    #[test]
    fn hardened_node_test_profile_disables_all_lifecycle_hooks() {
        let profile = find("node-hardened-test").expect("hardened Node test profile");
        let script = profile.steps[0].script;
        let exported = script
            .find("export npm_config_ignore_scripts=true")
            .expect("lifecycle suppression export");
        let install = script
            .find("npm ci --ignore-scripts --no-audit --no-fund")
            .expect("install step");
        let check = script.find("npm run check").expect("check step");
        let test = script.find("npm test").expect("test step");
        let audit = script
            .find("npm audit --audit-level=high")
            .expect("audit step");
        assert!(exported < install && install < check && check < test && test < audit);
        assert!(script.contains("package-lock.json"));
        assert!(!script.contains("npm install"));
        assert!(!script.contains("|| true"));
        assert!(!script.contains("--force"));
        assert!(!script.contains("curl"));
        assert!(!script.contains("wget"));
    }

'''
text = replace_section(text, full_start, full_end, full_test, "full profile tests")
write(profiles_path, text)


# The dependency-free fixture deliberately fails if npm lifecycle hooks escape
# the fixed profiles' ignore-scripts boundary.
write(
    "remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile/package.json",
    '''{
  "name": "gha-node-profile-fixture",
  "version": "1.0.0",
  "private": true,
  "description": "Credential-free fixture for the GHA continuity Node profiles",
  "type": "module",
  "scripts": {
    "preinstall": "node -e \"throw new Error('preinstall must be suppressed')\"",
    "install": "node -e \"throw new Error('install must be suppressed')\"",
    "postinstall": "node -e \"throw new Error('postinstall must be suppressed')\"",
    "pretest": "node -e \"throw new Error('pretest must be suppressed')\"",
    "check": "node --check src/operator-config.mjs && node --check test/operator-config.test.mjs",
    "test:operator-config": "node --test test/operator-config.test.mjs",
    "test": "node --test test/operator-config.test.mjs",
    "posttest": "node -e \"throw new Error('posttest must be suppressed')\""
  }
}
''',
)


# Mirror the exact workflow committed at the immutable Messaging Intel revision.
write(
    "remote/deployments/gha-clone-server-rs/tests/fixtures/msgint-operator-config.yml",
    '''name: Messaging Intel GHA clone operator verification

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
          node-version: "22.23.1"
          cache: npm
      - run: |
          export npm_config_ignore_scripts=true
          npm ci --ignore-scripts --no-audit --no-fund
          npm run check
          npm run test:operator-config
          npm audit --audit-level=high

  repository_tests:
    needs: operator_config
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
        with:
          persist-credentials: false
      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020
        with:
          node-version: "22.23.1"
          cache: npm
      - run: |
          export npm_config_ignore_scripts=true
          npm ci --ignore-scripts --no-audit --no-fund
          npm run check
          npm test
          npm audit --audit-level=high
''',
)

integration_path = "remote/deployments/gha-clone-server-rs/tests/msgint_operator_config.rs"
text = read(integration_path)
text = replace_once(
    text,
    'const REVISION: &str = "7d905806b2000479bdacb9b206f33b26a707ba5e";',
    f'const REVISION: &str = "{MSGINT_REVISION}";',
    "immutable Messaging Intel revision",
)
if '"node-verify"' in text:
    raise RuntimeError("generic node-verify remained in Messaging Intel integration")
write(integration_path, text)


# Deploy both hardened profiles, admit only the exact repository URL, and give
# the build server the same short-lived App token used by the clone service.
write(
    "remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml",
    '''apiVersion: apps/v1
kind: Deployment
metadata:
  name: dd-build-server
  namespace: default
spec:
  template:
    spec:
      containers:
        - name: build-server
          env:
            - name: BUILD_SERVER_ALLOWED_PROFILES
              value: rust-verify,node-verify,node-hardened-verify,node-hardened-test,python-verify,flutter-verify,flutter-android-debug,flutter-web-release,flutter-linux-release,flutter-linux-desktop-entrypoint,flutter-web-e2e,playwright,puppeteer,browser-e2e
            - name: BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES
              value: https://github.com/ORESoftware/,https://github.com/sonus-auris/,git@github.com:ORESoftware/,git@github.com:sonus-auris/,=https://github.com/messaging-intel/msgint-connectors.git
            - name: BUILD_SERVER_GIT_TOKEN
              valueFrom:
                secretKeyRef:
                  name: dd-gha-clone-server-secrets
                  key: github_app_installation_token
''',
)


# Turn the profile smoke into an automatic credential-free proof plus a manual,
# repository-scoped live proof. The private token never enters profile containers.
workflow_path = ".github/workflows/gha-clone-server.yml"
text = read(workflow_path)
text = replace_count(
    text,
    "      - 'remote/deployments/build-server-rs/src/validation.rs'\n",
    "      - 'remote/deployments/build-server-rs/src/validation.rs'\n      - 'remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile/**'\n",
    2,
    "fixture workflow path filters",
)
text = replace_count(
    text,
    "run_msgint_profile_smoke",
    "run_msgint_private_smoke",
    2,
    "private smoke input rename",
)
text = replace_once(
    text,
    "description: Run the private Messaging Intel fixed-profile smoke with the repository GitHub App",
    "description: Run the exact private Messaging Intel smoke with a repository-scoped GitHub App token",
    "private smoke description",
)
text = replace_once(
    text,
    "cargo test --locked --lib -- --nocapture",
    "cargo test --locked --all-targets -- --nocapture",
    "binary-only build-server test command",
)
text = replace_once(text, "  msgint-profile-smoke:\n", "  msgint-private-profile-smoke:\n", "private job name")
text = replace_once(
    text,
    "  msgint-private-profile-smoke:\n    if: ${{ github.event_name == 'workflow_dispatch' && inputs.run_msgint_private_smoke }}\n    needs: [rust, build-server-profile]\n",
    "  msgint-private-profile-smoke:\n    if: ${{ github.event_name == 'workflow_dispatch' && inputs.run_msgint_private_smoke }}\n    needs: [rust, build-server-profile, node-profile-hermetic-smoke]\n",
    "private job dependencies",
)
text = replace_once(
    text,
    "MSGINT_REVISION: 7d905806b2000479bdacb9b206f33b26a707ba5e",
    f"MSGINT_REVISION: {MSGINT_REVISION}",
    "private smoke revision",
)
text = replace_once(
    text,
    "actions/create-github-app-token@fee1f7d63c2ff003460e3d139729b119787bc349 # v2.2.2",
    "actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1 # v3.2.0",
    "App token action pin",
)
text = replace_once(
    text,
    "          owner: messaging-intel\n          repositories: msgint-connectors\n",
    "          owner: messaging-intel\n          repositories: msgint-connectors\n          permission-contents: read\n",
    "App token contents scope",
)
text = replace_once(
    text,
    "      - name: Mint a short-lived token for the exact private repository\n",
    '''      - name: Require repository-scoped GitHub App credentials
        env:
          APP_ID: ${{ secrets.K8S_SUBMODULE_APP_ID }}
          APP_PRIVATE_KEY: ${{ secrets.K8S_SUBMODULE_APP_PRIVATE_KEY }}
        run: |
          set -euo pipefail
          test -n "$APP_ID" || { echo "K8S_SUBMODULE_APP_ID is not configured" >&2; exit 1; }
          test -n "$APP_PRIVATE_KEY" || { echo "K8S_SUBMODULE_APP_PRIVATE_KEY is not configured" >&2; exit 1; }

      - name: Mint a short-lived token for the exact private repository
''',
    "private App credential preflight",
)
private_mount = '''          for profile in node-hardened-verify node-hardened-test; do
            script="$RUNNER_TEMP/${profile}.sh"
            test -s "$script"
            docker run --rm \\
'''
private_mount_new = '''          for profile in node-hardened-verify node-hardened-test; do
            script="$RUNNER_TEMP/${profile}.sh"
            test -s "$script"
            worktree="$(mktemp -d "$RUNNER_TEMP/${profile}.XXXXXX")"
            cp -R "$MSGINT_REPO_DIR/." "$worktree/"
            docker run --rm \\
'''
text = replace_once(text, private_mount, private_mount_new, "isolated private profile worktrees")
text = replace_once(
    text,
    '              -v "$MSGINT_REPO_DIR:/workspace:rw" \\\n',
    '              -v "$worktree:/workspace:rw" \\\n',
    "private profile worktree mount",
)
hermetic_job = '''  node-profile-hermetic-smoke:
    needs: [rust, build-server-profile]
    runs-on: ubuntu-24.04
    timeout-minutes: 15
    steps:
      - name: Checkout continuity source
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - name: Execute both fixed profiles against the credential-free fixture
        run: |
          set -euo pipefail
          fixture_source="$GITHUB_WORKSPACE/remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile"
          python3 <<'PY'
          import os
          import re
          from pathlib import Path

          source = Path("remote/deployments/build-server-rs/src/profiles.rs").read_text(encoding="utf-8")
          image_match = re.search(r'const NODE_IMAGE: &str = "([^"]+)";', source)
          if image_match is None:
              raise SystemExit("NODE_IMAGE was not found")
          output_dir = Path(os.environ["RUNNER_TEMP"])
          (output_dir / "node-profile-image").write_text(image_match.group(1), encoding="utf-8")
          for profile, constant in (
              ("node-hardened-verify", "NODE_HARDENED_VERIFY_STEPS"),
              ("node-hardened-test", "NODE_HARDENED_TEST_STEPS"),
          ):
              pattern = rf'const {constant}:.*?script: r#"(.*?)"#,\n\}}\];'
              match = re.search(pattern, source, flags=re.DOTALL)
              if match is None:
                  raise SystemExit(f"{constant} script was not found")
              script_path = output_dir / f"{profile}.sh"
              script_path.write_text(match.group(1) + "\n", encoding="utf-8")
              script_path.chmod(0o500)
          PY
          profile_image="$(cat "$RUNNER_TEMP/node-profile-image")"
          case "$profile_image" in
            *@sha256:*) ;;
            *) echo "Node profile image is not digest-pinned" >&2; exit 1 ;;
          esac
          docker pull "$profile_image"
          for profile in node-hardened-verify node-hardened-test; do
            worktree="$(mktemp -d "$RUNNER_TEMP/${profile}-fixture.XXXXXX")"
            cp -R "$fixture_source/." "$worktree/"
            docker run --rm \\
              --pull=never \\
              --cap-drop=ALL \\
              --security-opt=no-new-privileges \\
              --pids-limit=256 \\
              --memory=2g \\
              --cpus=2 \\
              --read-only \\
              --tmpfs /tmp:rw,nosuid,nodev,noexec,size=512m \\
              --network=bridge \\
              -e CI=true \\
              -e HOME=/tmp/home \\
              -v "$worktree:/workspace:rw" \\
              -v "$RUNNER_TEMP/${profile}.sh:/profile.sh:ro" \\
              -w /workspace \\
              "$profile_image" \\
              bash /profile.sh
          done

'''
text = replace_once(
    text,
    "  msgint-private-profile-smoke:\n",
    hermetic_job + "  msgint-private-profile-smoke:\n",
    "automatic hermetic profile smoke",
)
write(workflow_path, text)


# Extend the static contracts so later edits cannot silently broaden repository,
# token, command, image, or private-smoke boundaries.
contracts_path = "remote/tests/general/gha-clone-server-config.test.ts"
text = read(contracts_path)
text = replace_once(
    text,
    "const metaWorkflowPath = '.github/workflows/gha-clone-server-meta.yml';\n",
    '''const metaWorkflowPath = '.github/workflows/gha-clone-server-meta.yml';
const nodeFixturePackagePath =
  'remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile/package.json';
const nodeFixtureLockPath =
  'remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile/package-lock.json';
const nodeFixtureSourcePath =
  'remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile/src/operator-config.mjs';
const nodeFixtureTestPath =
  'remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile/test/operator-config.test.mjs';
''',
    "fixture contract paths",
)
text = replace_once(
    text,
    "  assert.match(profiles, /npm ci --ignore-scripts/);\n",
    f'''  assert.match(profiles, /export npm_config_ignore_scripts=true/);
  assert.match(profiles, /npm ci --ignore-scripts --no-audit --no-fund/);
  assert.match(
    profiles,
    /const NODE_IMAGE: &str = "{NODE_IMAGE.replace('/', '\\/').replace('.', '\\.')}";/,
  );
''',
    "fixed profile supply-chain contract",
)
text = replace_once(
    text,
    "  assert.match(continuityPatch, /node-hardened-test/);\n",
    '''  assert.match(continuityPatch, /node-hardened-test/);
  assert.match(
    continuityPatch,
    /name:\s*BUILD_SERVER_GIT_TOKEN[\s\S]*name:\s*dd-gha-clone-server-secrets[\s\S]*key:\s*github_app_installation_token/,
  );
  const allowlist = continuityPatch
    .match(/name:\s*BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES\s+value:\s*([^\n]+)/)?.[1]
    ?.split(',')
    .map((entry) => entry.trim()) ?? [];
  assert.ok(allowlist.includes('=https://github.com/messaging-intel/msgint-connectors.git'));
  assert.ok(!allowlist.includes('https://github.com/messaging-intel/'));
''',
    "exact repository and token contracts",
)
text = text.replace("run_msgint_profile_smoke", "run_msgint_private_smoke")
fixture_contract = '''test('credential-free Node fixture exercises both hardened fixed profiles', () => {
  const packageJson = JSON.parse(read(nodeFixturePackagePath));
  const packageLock = JSON.parse(read(nodeFixtureLockPath));
  const source = read(nodeFixtureSourcePath);
  const tests = read(nodeFixtureTestPath);
  const workflow = read(workflowPath);

  assert.equal(packageJson.private, true);
  assert.deepEqual(packageJson.dependencies ?? {}, {});
  for (const hook of ['preinstall', 'install', 'postinstall', 'pretest', 'posttest']) {
    assert.match(packageJson.scripts[hook], /must be suppressed/);
  }
  assert.equal(packageLock.lockfileVersion, 3);
  assert.deepEqual(packageLock.packages[''].dependencies ?? {}, {});
  assert.match(packageJson.scripts.check, /node --check/);
  assert.match(packageJson.scripts['test:operator-config'], /node --test/);
  assert.match(source, /collectionMode must be official-api/);
  assert.match(source, /consentRequired must be true/);
  assert.match(tests, /browser-scrape/);
  assert.match(tests, /must-not-enter-config/);
  assert.match(workflow, /node-profile-hermetic-smoke:/);
  assert.match(workflow, /node-hardened-verify node-hardened-test/);
  assert.match(workflow, /--cap-drop=ALL/);
  assert.match(workflow, /--security-opt=no-new-privileges/);
  assert.match(workflow, /--read-only/);
});

test('private Messaging Intel smoke is manual, exact, and App-scoped', () => {
  const workflow = read(workflowPath);
  assert.match(workflow, /run_msgint_private_smoke:/);
  assert.match(workflow, /github\.event_name == 'workflow_dispatch'/);
  assert.match(workflow, /inputs\.run_msgint_private_smoke/);
  assert.match(
    workflow,
    /actions\/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1/,
  );
  assert.match(workflow, /owner: messaging-intel/);
  assert.match(workflow, /repositories: msgint-connectors/);
  assert.match(workflow, /permission-contents: read/);
  assert.match(workflow, /MSGINT_REVISION: 2ef2e4a1a5762b5289474f3da85ddf838b41bf3f/);
  assert.match(workflow, /persist-credentials: false/);
  assert.doesNotMatch(workflow, /ghp_[A-Za-z0-9]{20,}|github_pat_/);
});

'''
text = replace_once(
    text,
    "test('bounded meta workflow remains independently compilable', () => {\n",
    fixture_contract + "test('bounded meta workflow remains independently compilable', () => {\n",
    "fixture and private smoke contracts",
)
text = replace_once(
    text,
    "  assert.match(workflow, /dd-build-server-gha-continuity\\.patch\\.yaml/);\n",
    "  assert.match(workflow, /dd-build-server-gha-continuity\\.patch\\.yaml/);\n  assert.match(workflow, /node-profile-hermetic-smoke/);\n  assert.match(workflow, /run_msgint_private_smoke/);\n",
    "workflow smoke contracts",
)
write(contracts_path, text)


# Keep the operational access requirement explicit and prohibit a PAT fallback.
admission_path = "docs/gha-profile-repository-admission.md"
text = read(admission_path)
text += '''

## Private clone credential boundary

The continuity deployment uses the short-lived GitHub App installation token in
`dd-gha-clone-server-secrets.github_app_installation_token` for both workflow
fetches and the build server's exact private repository clone. The backing
`dd/remote-dev/gha-clone-server-secrets` record is refreshed by External Secrets;
its token must be issued by the reviewed broker and must never be a classic PAT.

The GitHub-hosted live smoke uses repository secrets
`K8S_SUBMODULE_APP_ID` and `K8S_SUBMODULE_APP_PRIVATE_KEY` only to mint a
one-job token restricted to `messaging-intel/msgint-connectors` with
`contents:read`. Ordinary pull-request verification uses the credential-free
fixture and therefore remains runnable when those trusted-branch credentials are
unavailable.
'''
write(admission_path, text)

clone_readme = "remote/deployments/gha-clone-server-rs/README.md"
text = read(clone_readme)
if "## Messaging Intel verification boundary" not in text:
    text = replace_once(
        text,
        "## Fail-closed exclusions\n",
        '''## Messaging Intel verification boundary

Pull-request CI exercises both fixed Node profiles against the dependency-free
fixture. The private repository smoke is manual and requires a short-lived,
repository-scoped GitHub App token. The build server admits only the exact
canonical HTTPS repository URL; no organization-wide Messaging Intel prefix and
no PAT fallback are accepted.

## Fail-closed exclusions
''',
        "Messaging Intel README boundary",
    )
write(clone_readme, text)

build_readme = "remote/deployments/build-server-rs/readme.md"
text = read(build_readme)
text = text.replace(
    "| `node-hardened-test` | npm lifecycle-script suppression and complete repository tests | none |",
    "| `node-hardened-test` | npm lifecycle-script suppression, syntax checks, complete tests, and high-severity audit | none |",
)
write(build_readme, text)
