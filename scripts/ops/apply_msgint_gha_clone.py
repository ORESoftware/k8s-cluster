from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


def update(path: str, transforms) -> None:
    file = Path(path)
    text = file.read_text(encoding="utf-8")
    for old, new, label in transforms:
        text = replace_once(text, old, new, label)
    file.write_text(text, encoding="utf-8")


update(
    "remote/deployments/gha-clone-server-rs/src/lib.rs",
    [
        (
            '            "rust-verify".to_string(),\n            "node-verify".to_string(),\n            "python-verify".to_string(),',
            '            "rust-verify".to_string(),\n            "node-verify".to_string(),\n            "node-hardened-verify".to_string(),\n            "python-verify".to_string(),',
            "capability profile list",
        ),
        (
            '''    if text.contains("npm ")
        || text.contains("pnpm ")
        || text.contains("yarn ")
        || text.contains("setup-node")
        || text.contains("node --test")
    {
        return Some("node-verify".into());
    }
''',
            '''    if text.contains("npm ci --ignore-scripts")
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
''',
            "hardened Node classifier",
        ),
        (
            '''    #[test]
    fn review_order_is_deterministic_for_parallel_roots() {
''',
            '''    #[test]
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
''',
            "Messaging Intel planner tests",
        ),
    ],
)

update(
    "remote/deployments/build-server-rs/src/profiles.rs",
    [
        (
            'const PYTHON_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {',
            '''const NODE_HARDENED_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {
    name: "Hardened Node operator configuration verification",
    image: NODE_IMAGE,
    subdirectory: ".",
    script: r#"set -euo pipefail
if [ ! -f package-lock.json ] && [ ! -f npm-shrinkwrap.json ]; then
  echo "node-hardened-verify requires package-lock.json or npm-shrinkwrap.json" >&2
  exit 2
fi
npm ci --ignore-scripts
npm run check
npm run test:operator-config
npm audit --audit-level=high"#,
}];

const PYTHON_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {''',
            "hardened Node profile steps",
        ),
        (
            '''    ProfileSpec {
        name: "python-verify",
        platform: "linux",
        description: "Python bytecode compilation, declared dependency install, and pytest",
        steps: PYTHON_VERIFY_STEPS,
        artifact_paths: &[],
    },
''',
            '''    ProfileSpec {
        name: "node-hardened-verify",
        platform: "linux",
        description: "Lifecycle-script-free Node operator checks, focused tests, and high-severity audit",
        steps: NODE_HARDENED_VERIFY_STEPS,
        artifact_paths: &[],
    },
    ProfileSpec {
        name: "python-verify",
        platform: "linux",
        description: "Python bytecode compilation, declared dependency install, and pytest",
        steps: PYTHON_VERIFY_STEPS,
        artifact_paths: &[],
    },
''',
            "hardened Node profile registry",
        ),
        (
            '        for name in ["rust-verify", "node-verify", "python-verify"] {',
            '        for name in [\n            "rust-verify",\n            "node-verify",\n            "node-hardened-verify",\n            "python-verify",\n        ] {',
            "continuity profile test list",
        ),
        (
            '''    #[test]
    fn rust_verify_has_only_the_reviewed_meta_server_monorepo_fallback() {
''',
            '''    #[test]
    fn hardened_node_profile_is_ordered_and_supply_chain_bounded() {
        let profile = find("node-hardened-verify").expect("hardened Node profile");
        let script = profile.steps[0].script;
        assert_eq!(profile.steps[0].subdirectory, ".");
        let install = script.find("npm ci --ignore-scripts").expect("install step");
        let check = script.find("npm run check").expect("check step");
        let focused = script
            .find("npm run test:operator-config")
            .expect("focused test step");
        let audit = script
            .find("npm audit --audit-level=high")
            .expect("audit step");
        assert!(install < check && check < focused && focused < audit);
        assert!(script.contains("package-lock.json"));
        assert!(!script.contains("npm install"));
        assert!(!script.contains("|| true"));
        assert!(!script.contains("--force"));
        assert!(!script.contains("curl"));
        assert!(!script.contains("wget"));
    }

    #[test]
    fn rust_verify_has_only_the_reviewed_meta_server_monorepo_fallback() {
''',
            "hardened Node profile tests",
        ),
    ],
)

update(
    "remote/deployments/gha-clone-server-rs/README.md",
    [
        (
            '| npm/pnpm/yarn/Node tests | `node-verify` |',
            '| npm/pnpm/yarn/Node tests | `node-verify` |\n| npm install-script suppression + operator checks + high-severity audit | `node-hardened-verify` |',
            "GHA profile table",
        ),
        (
            'The independent lane never forwards caller-selected shell, action code, runner\nimages, or Kubernetes manifests. It submits only a trusted repository, immutable\ncommit SHA, and operator-reviewed profile name to `dd-build-server`.',
            'The independent lane never forwards caller-selected shell, action code, runner\nimages, or Kubernetes manifests. It submits only a trusted repository, immutable\ncommit SHA, and operator-reviewed profile name to `dd-build-server`. Messaging\nIntel uses a dedicated two-job mirror: `node-hardened-verify` for the non-secret\noperator contract and `node-verify` for the complete repository test suite.',
            "Messaging Intel architecture note",
        ),
    ],
)

update(
    "remote/deployments/build-server-rs/readme.md",
    [
        (
            '| `flutter-verify` | Flutter analyze and unit tests | none |',
            '| `rust-verify` | Rust formatting, Clippy, and all-target tests | none |\n| `node-verify` | Lockfile-strict Node repository tests | none |\n| `node-hardened-verify` | npm lifecycle-script suppression, operator checks, and high-severity audit | none |\n| `python-verify` | Python compilation and pytest | none |\n| `flutter-verify` | Flutter analyze and unit tests | none |',
            "build-server profile table",
        ),
        (
            'The cluster permits the `ORESoftware` and `sonus-auris` organizations;\nadding another organization requires an explicit manifest review.',
            'The cluster permits the `ORESoftware` and `sonus-auris` organizations and\none exact HTTPS repository URL for `messaging-intel/msgint-connectors`; adding\nanother organization or repository requires an explicit manifest review.',
            "profile repository allowlist docs",
        ),
    ],
)

update(
    "remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml",
    [
        (
            '    ORESoftware/k8s-cluster,\n    sonus-auris/sonus-auris-interfaces,',
            '    ORESoftware/k8s-cluster,\n    messaging-intel/msgint-connectors,\n    sonus-auris/sonus-auris-interfaces,',
            "Messaging Intel repository allowlist",
        ),
        (
            '      "ORESoftware/k8s-cluster": [".github/workflows/gha-clone-server-meta.yml"],\n      "sonus-auris/sonus-auris-interfaces": [".github/workflows/ci.yml"],',
            '      "ORESoftware/k8s-cluster": [".github/workflows/gha-clone-server-meta.yml"],\n      "messaging-intel/msgint-connectors": [".github/workflows/gha-clone-operator-config.yml"],\n      "sonus-auris/sonus-auris-interfaces": [".github/workflows/ci.yml"],',
            "Messaging Intel workflow rule",
        ),
    ],
)

update(
    "remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml",
    [
        (
            '              value: rust-verify,node-verify,python-verify,flutter-verify,flutter-android-debug,flutter-web-release,flutter-linux-release,flutter-linux-desktop-entrypoint,flutter-web-e2e,playwright,puppeteer,browser-e2e',
            '              value: rust-verify,node-verify,node-hardened-verify,python-verify,flutter-verify,flutter-android-debug,flutter-web-release,flutter-linux-release,flutter-linux-desktop-entrypoint,flutter-web-e2e,playwright,puppeteer,browser-e2e\n            - name: BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES\n              value: https://github.com/ORESoftware/,https://github.com/sonus-auris/,git@github.com:ORESoftware/,git@github.com:sonus-auris/,https://github.com/messaging-intel/msgint-connectors.git',
            "continuity deployment profile and repository allowlists",
        ),
    ],
)

update(
    ".github/workflows/gha-clone-server.yml",
    [
        (
            "      - 'remote/argocd/dd-next-runtime/dd-build-server.deployment.yaml'\n      - 'remote/argocd/dd-next-runtime/kustomization.yaml'",
            "      - 'remote/argocd/dd-next-runtime/dd-build-server.deployment.yaml'\n      - 'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml'\n      - 'remote/argocd/dd-next-runtime/kustomization.yaml'",
            "pull request continuity patch trigger",
        ),
        (
            "      - 'remote/argocd/dd-next-runtime/dd-build-server.deployment.yaml'\n      - 'remote/argocd/dd-next-runtime/kustomization.yaml'",
            "      - 'remote/argocd/dd-next-runtime/dd-build-server.deployment.yaml'\n      - 'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml'\n      - 'remote/argocd/dd-next-runtime/kustomization.yaml'",
            "push continuity patch trigger",
        ),
        (
            '''      - name: Validate bounded meta workflow syntax
        uses: docker://rhysd/actionlint@sha256:b1934ee5f1c509618f2508e6eb47ee0d3520686341fec936f3b79331f9315667
        with:
          args: .github/workflows/gha-clone-server-meta.yml
''',
            '''      - name: Validate bounded meta workflow syntax
        uses: docker://rhysd/actionlint@sha256:b1934ee5f1c509618f2508e6eb47ee0d3520686341fec936f3b79331f9315667
        with:
          args: .github/workflows/gha-clone-server-meta.yml
      - name: Validate Messaging Intel bounded fixture syntax
        uses: docker://rhysd/actionlint@sha256:b1934ee5f1c509618f2508e6eb47ee0d3520686341fec936f3b79331f9315667
        with:
          args: remote/deployments/gha-clone-server-rs/tests/fixtures/msgint-operator-config.yml
''',
            "Messaging Intel actionlint step",
        ),
    ],
)

update(
    "remote/tests/general/gha-clone-server-config.test.ts",
    [
        (
            "const profilesPath = 'remote/deployments/build-server-rs/src/profiles.rs';\nconst plannerPath",
            "const profilesPath = 'remote/deployments/build-server-rs/src/profiles.rs';\nconst continuityPatchPath =\n  'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml';\nconst plannerPath",
            "continuity patch path constant",
        ),
        (
            "const metaIntegrationTestPath =\n  'remote/deployments/gha-clone-server-rs/tests/meta_self_test.rs';\nconst workflowPath",
            "const metaIntegrationTestPath =\n  'remote/deployments/gha-clone-server-rs/tests/meta_self_test.rs';\nconst msgintIntegrationTestPath =\n  'remote/deployments/gha-clone-server-rs/tests/msgint_operator_config.rs';\nconst msgintFixturePath =\n  'remote/deployments/gha-clone-server-rs/tests/fixtures/msgint-operator-config.yml';\nconst workflowPath",
            "Messaging Intel test path constants",
        ),
        (
            "  assert.match(config, /ORESoftware\\/k8s-cluster/);\n  assert.match(config, /sonus-auris\\/sonus-auris-interfaces/);",
            "  assert.match(config, /ORESoftware\\/k8s-cluster/);\n  assert.match(config, /messaging-intel\\/msgint-connectors/);\n  assert.match(config, /sonus-auris\\/sonus-auris-interfaces/);",
            "Messaging Intel allowlist assertion",
        ),
        (
            '''  assert.doesNotMatch(
    config,
    /"ORESoftware\\/k8s-cluster": \\["\\.github\\/workflows\\/gha-clone-server\\.yml"\\]/,
  );
''',
            '''  assert.doesNotMatch(
    config,
    /"ORESoftware\\/k8s-cluster": \\["\\.github\\/workflows\\/gha-clone-server\\.yml"\\]/,
  );
  assert.match(
    config,
    /"messaging-intel\\/msgint-connectors": \\["\\.github\\/workflows\\/gha-clone-operator-config\\.yml"\\]/,
  );
''',
            "Messaging Intel workflow rule assertion",
        ),
        (
            "  for (const profile of ['rust-verify', 'node-verify', 'python-verify']) {",
            "  for (const profile of [\n    'rust-verify',\n    'node-verify',\n    'node-hardened-verify',\n    'python-verify',\n  ]) {",
            "profile registry assertion list",
        ),
        (
            "  assert.match(profiles, /pnpm install --frozen-lockfile/);\n  assert.match(profiles, /python -m pytest/);",
            "  assert.match(profiles, /pnpm install --frozen-lockfile/);\n  assert.match(profiles, /npm ci --ignore-scripts/);\n  assert.match(profiles, /npm run test:operator-config/);\n  assert.match(profiles, /npm audit --audit-level=high/);\n  assert.match(profiles, /python -m pytest/);\n\n  const continuityPatch = read(continuityPatchPath);\n  assert.match(continuityPatch, /node-hardened-verify/);\n  assert.match(\n    continuityPatch,\n    /https:\\/\\/github\\.com\\/messaging-intel\\/msgint-connectors\\.git/,\n  );",
            "hardened profile and deployment assertions",
        ),
        (
            "  assert.match(planner, /non-Linux native execution is unavailable/);",
            "  assert.match(planner, /non-Linux native execution is unavailable/);\n  assert.match(planner, /node-hardened-verify/);",
            "hardened classifier assertion",
        ),
        (
            '''test('bounded meta workflow remains independently compilable', () => {
''',
            '''test('Messaging Intel integration starts the real server and dispatches both fixed profiles', () => {
  const integration = read(msgintIntegrationTestPath);
  assert.match(integration, /CARGO_BIN_EXE_gha-clone-server/);
  assert.match(integration, /messaging-intel\\/msgint-connectors/);
  assert.match(integration, /gha-clone-operator-config\\.yml/);
  assert.match(integration, /node-hardened-verify/);
  assert.match(integration, /node-verify/);
  assert.match(integration, /submissions\\.len\\(\\), 2/);
  assert.match(integration, /env_remove\\("GHA_CLONE_GITHUB_TOKEN"\\)/);
});

test('Messaging Intel mirror remains independently compilable and non-secret', () => {
  const workflow = read(msgintFixturePath);
  assert.match(workflow, /workflow_dispatch:/);
  assert.match(workflow, /operator_config:/);
  assert.match(workflow, /repository_tests:/);
  assert.match(workflow, /needs: operator_config/);
  assert.match(workflow, /npm ci --ignore-scripts/);
  assert.match(workflow, /npm run check/);
  assert.match(workflow, /npm run test:operator-config/);
  assert.match(workflow, /npm audit --audit-level=high/);
  assert.match(workflow, /npm test/);
  assert.equal((workflow.match(/persist-credentials:\\s*false/g) ?? []).length, 2);
  assert.doesNotMatch(
    workflow,
    /\\$\\{\\{|secrets\\.|working-directory:|timeout-minutes:|permissions:|concurrency:/,
  );
  assert.doesNotMatch(workflow, /services:|container:|strategy:|\\bcurl\\b|\\bwget\\b/);
});

test('bounded meta workflow remains independently compilable', () => {
''',
            "Messaging Intel static contracts",
        ),
        (
            "  assert.match(workflow, /gha-clone-server-meta\\.yml/);\n  assert.match(workflow, /actionlint@sha256:/);",
            "  assert.match(workflow, /gha-clone-server-meta\\.yml/);\n  assert.match(workflow, /msgint-operator-config\\.yml/);\n  assert.match(workflow, /dd-build-server-gha-continuity\\.patch\\.yaml/);\n  assert.match(workflow, /actionlint@sha256:/);",
            "workflow coverage assertions",
        ),
    ],
)
