"""Explicit semantic unions for the three reviewed DEN-977 overlaps."""

from __future__ import annotations

from collections.abc import Callable

WORKFLOW_PATH = ".github/workflows/gha-clone-server.yml"
PROFILES_PATH = "remote/deployments/build-server-rs/src/profiles.rs"
SERVER_CONTRACT_PATH = "remote/tests/general/gha-clone-server-config.test.ts"


def text(data: bytes | None, label: str) -> str:
    if data is None:
        raise SystemExit(f"{label} unexpectedly does not exist")
    return data.decode("utf-8")


def replace_exact(
    source: str,
    old: str,
    new: str,
    label: str,
    *,
    count: int = 1,
) -> str:
    actual = source.count(old)
    if actual != count:
        raise SystemExit(f"{label}: expected {count} exact anchor(s), found {actual}")
    return source.replace(old, new)


def extract_section(source: str, start: str, end: str, label: str) -> str:
    start_index = source.find(start)
    if start_index < 0:
        raise SystemExit(f"{label}: start boundary was not found")
    end_index = source.find(end, start_index + len(start))
    if end_index < 0:
        raise SystemExit(f"{label}: end boundary was not found")
    return source[start_index:end_index]


def reject_conflict_markers(path: str, source: str) -> None:
    for marker in ("<<<<<<<", "|||||||", "=======", ">>>>>>>"):
        if marker in source:
            raise SystemExit(f"semantic resolver left {marker!r} in {path}")


def resolve_workflow(current_data: bytes | None, reviewed_data: bytes | None) -> bytes:
    current = text(current_data, "current continuity workflow")
    reviewed = text(reviewed_data, "reviewed Messaging Intel workflow")

    current = replace_exact(
        current,
        "      - 'remote/argocd/dd-next-runtime/dd-build-server.deployment.yaml'\n"
        "      - 'remote/argocd/dd-next-runtime/kustomization.yaml'",
        "      - 'remote/argocd/dd-next-runtime/dd-build-server.deployment.yaml'\n"
        "      - 'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml'\n"
        "      - 'remote/argocd/dd-next-runtime/kustomization.yaml'",
        "continuity workflow build-server patch path",
        count=2,
    )
    current = replace_exact(
        current,
        "      - 'remote/tests/general/gha-clone-webhook-config.test.ts'\n"
        "      - 'remote/tests/general/gha-clone-streempilot-config.test.ts'",
        "      - 'remote/tests/general/gha-clone-webhook-config.test.ts'\n"
        "      - 'remote/tests/general/gha-clone-msgint-config.test.ts'\n"
        "      - 'remote/tests/general/gha-clone-streempilot-config.test.ts'",
        "continuity workflow Messaging Intel contract path",
        count=2,
    )
    current = replace_exact(
        current,
        "      - 'docs/gha-executor-router-activation.md'\n"
        "      - 'docs/streempilot-ci-continuity.md'",
        "      - 'docs/gha-executor-router-activation.md'\n"
        "      - 'docs/gha-profile-repository-admission.md'\n"
        "      - 'docs/streempilot-ci-continuity.md'",
        "continuity workflow profile-admission documentation path",
        count=2,
    )
    current = replace_exact(
        current,
        "        default: false\n\npermissions:",
        "        default: false\n"
        "      run_msgint_profile_smoke:\n"
        "        description: Run the private Messaging Intel fixed-profile smoke with the repository GitHub App\n"
        "        type: boolean\n"
        "        required: true\n"
        "        default: false\n\npermissions:",
        "continuity workflow Messaging Intel dispatch input",
    )
    current = replace_exact(
        current,
        "      - name: Run unit, process, webhook, meta, and StreemPilot mirror integration tests",
        "      - name: Run unit, process, webhook, meta, Messaging Intel, and StreemPilot mirror integration tests",
        "continuity workflow Rust test label",
    )
    current = replace_exact(
        current,
        "      - name: Check the modified idempotency leaf modules formatting\n"
        "        working-directory: remote/deployments/build-server-rs\n"
        "        run: >-\n"
        "          rustfmt --edition 2021 --check\n"
        "          src/jobs.rs\n"
        "          src/nats_submit.rs",
        "      - name: Check modified build-server leaf modules formatting\n"
        "        working-directory: remote/deployments/build-server-rs\n"
        "        run: >-\n"
        "          rustfmt --edition 2021 --check\n"
        "          src/jobs.rs\n"
        "          src/nats_submit.rs\n"
        "          src/profiles.rs\n"
        "          src/validation.rs",
        "continuity workflow build-server formatting set",
    )
    current = replace_exact(
        current,
        "      - name: Test the fixed-profile registry and reviewed fallbacks\n"
        "        working-directory: remote/deployments/build-server-rs\n"
        "        run: cargo test --locked profiles::tests -- --nocapture\n"
        "      - name: Test build request idempotency and retry semantics",
        "      - name: Test the fixed-profile registry and reviewed fallbacks\n"
        "        working-directory: remote/deployments/build-server-rs\n"
        "        run: cargo test --locked profiles::tests -- --nocapture\n"
        "      - name: Test exact repository admission\n"
        "        working-directory: remote/deployments/build-server-rs\n"
        "        run: cargo test --locked validation::tests -- --nocapture\n"
        "      - name: Test build request idempotency and retry semantics",
        "continuity workflow repository-admission test",
    )
    current = replace_exact(
        current,
        "      - name: Test strict NATS conflict and redelivery classification\n"
        "        working-directory: remote/deployments/build-server-rs\n"
        "        run: cargo test --locked nats_submit::tests -- --nocapture\n\n"
        "  contracts:",
        "      - name: Test strict NATS conflict and redelivery classification\n"
        "        working-directory: remote/deployments/build-server-rs\n"
        "        run: cargo test --locked nats_submit::tests -- --nocapture\n"
        "      - name: Execute the credential-free hardened Node fixture\n"
        "        working-directory: remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile\n"
        "        run: |\n"
        "          set -euo pipefail\n"
        "          npm ci --ignore-scripts\n"
        "          npm run check\n"
        "          npm run test:operator-config\n"
        "          npm audit --audit-level=high\n"
        "          npm test\n\n"
        "  contracts:",
        "continuity workflow hardened Node fixture",
    )

    msgint_job = extract_section(
        reviewed,
        "\n  msgint-profile-smoke:\n",
        "\n  contracts:\n",
        "reviewed Messaging Intel smoke job",
    )
    current = replace_exact(
        current,
        "\n  contracts:\n",
        msgint_job + "\n  contracts:\n",
        "continuity workflow Messaging Intel smoke insertion",
    )
    current = replace_exact(
        current,
        "      - name: Validate bounded meta workflow syntax\n"
        "        uses: docker://rhysd/actionlint@sha256:b1934ee5f1c509618f2508e6eb47ee0d3520686341fec936f3b79331f9315667\n"
        "        with:\n"
        "          args: .github/workflows/gha-clone-server-meta.yml\n"
        "      - uses: pnpm/action-setup@0ebf47130e4866e96fce0953f49152a61190b271 # v6",
        "      - name: Validate bounded meta workflow syntax\n"
        "        uses: docker://rhysd/actionlint@sha256:b1934ee5f1c509618f2508e6eb47ee0d3520686341fec936f3b79331f9315667\n"
        "        with:\n"
        "          args: .github/workflows/gha-clone-server-meta.yml\n"
        "      - name: Validate Messaging Intel bounded fixture syntax\n"
        "        uses: docker://rhysd/actionlint@sha256:b1934ee5f1c509618f2508e6eb47ee0d3520686341fec936f3b79331f9315667\n"
        "        with:\n"
        "          args: remote/deployments/gha-clone-server-rs/tests/fixtures/msgint-operator-config.yml\n"
        "      - uses: pnpm/action-setup@0ebf47130e4866e96fce0953f49152a61190b271 # v6",
        "continuity workflow Messaging Intel fixture actionlint",
    )
    current = replace_exact(
        current,
        """      - name: Validate deployment, routing, execution, webhook, activation, and StreemPilot boundaries
        working-directory: remote/tests
        run: |
          pnpm exec tsx --test \
            general/gha-clone-server-config.test.ts \
            general/gha-clone-webhook-config.test.ts \
            general/gha-clone-streempilot-config.test.ts
          node --test general/gha-executor-router-activation.test.mjs""",
        """      - name: Validate deployment, routing, execution, webhook, activation, Messaging Intel, and StreemPilot boundaries
        working-directory: remote/tests
        run: |
          pnpm exec tsx --test \
            general/gha-clone-server-config.test.ts \
            general/gha-clone-webhook-config.test.ts \
            general/gha-clone-msgint-config.test.ts \
            general/gha-clone-streempilot-config.test.ts
          node --test general/gha-executor-router-activation.test.mjs""",
        "continuity workflow contract union",
    )
    current = replace_exact(
        current,
        """            docs/gha-executor-router-activation.md \
            docs/streempilot-ci-continuity.md; then""",
        """            docs/gha-executor-router-activation.md \
            docs/gha-profile-repository-admission.md \
            docs/streempilot-ci-continuity.md; then""",
        "continuity workflow credential-scan documentation union",
    )

    reject_conflict_markers(WORKFLOW_PATH, current)
    for required in (
        "run_arc_smoke:",
        "run_msgint_profile_smoke:",
        "msgint-profile-smoke:",
        "gha-clone-streempilot-config.test.ts",
        "gha-clone-msgint-config.test.ts",
        "gha-executor-router-activation.test.mjs",
        "cargo test --locked --test executor_router_http",
    ):
        if required not in current:
            raise SystemExit(f"continuity workflow union lost required marker: {required}")
    return current.encode()


def resolve_profiles(current_data: bytes | None, reviewed_data: bytes | None) -> bytes:
    current = text(current_data, "current fixed-profile registry")
    reviewed = text(reviewed_data, "reviewed Messaging Intel fixed-profile registry")

    hardened_steps = '''const NODE_HARDENED_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {
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

const NODE_HARDENED_TEST_STEPS: &[ProfileStep] = &[ProfileStep {
    name: "Lifecycle-script-free Node repository tests",
    image: NODE_IMAGE,
    subdirectory: ".",
    script: r#"set -euo pipefail
if [ ! -f package-lock.json ] && [ ! -f npm-shrinkwrap.json ]; then
  echo "node-hardened-test requires package-lock.json or npm-shrinkwrap.json" >&2
  exit 2
fi
npm ci --ignore-scripts
npm test"#,
}];

'''
    hardened_specs = '''    ProfileSpec {
        name: "node-hardened-verify",
        platform: "linux",
        description:
            "Lifecycle-script-free Node operator checks, focused tests, and high-severity audit",
        steps: NODE_HARDENED_VERIFY_STEPS,
        artifact_paths: &[],
    },
    ProfileSpec {
        name: "node-hardened-test",
        platform: "linux",
        description: "Lifecycle-script-free lockfile install and complete Node repository tests",
        steps: NODE_HARDENED_TEST_STEPS,
        artifact_paths: &[],
    },
'''
    hardened_tests = '''    #[test]
    fn hardened_node_profile_is_ordered_and_supply_chain_bounded() {
        let profile = find("node-hardened-verify").expect("hardened Node profile");
        let script = profile.steps[0].script;
        assert_eq!(profile.steps[0].subdirectory, ".");
        let install = script
            .find("npm ci --ignore-scripts")
            .expect("install step");
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
    fn hardened_node_test_profile_disables_lifecycle_scripts() {
        let profile = find("node-hardened-test").expect("hardened Node test profile");
        let script = profile.steps[0].script;
        let install = script
            .find("npm ci --ignore-scripts")
            .expect("install step");
        let test = script.find("npm test").expect("test step");
        assert!(install < test);
        assert!(script.contains("package-lock.json"));
        assert!(!script.contains("npm install"));
        assert!(!script.contains("|| true"));
        assert!(!script.contains("--force"));
        assert!(!script.contains("curl"));
        assert!(!script.contains("wget"));
    }

'''

    for marker in (
        "const NODE_HARDENED_VERIFY_STEPS",
        'name: "node-hardened-verify"',
        "fn hardened_node_profile_is_ordered_and_supply_chain_bounded",
    ):
        if marker not in reviewed:
            raise SystemExit(f"reviewed profile source lost expected marker: {marker}")
        if marker in current:
            raise SystemExit(f"current profile source unexpectedly already contains: {marker}")

    current = replace_exact(
        current,
        "const PYTHON_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {",
        hardened_steps + "const PYTHON_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {",
        "hardened Node step insertion",
    )
    current = replace_exact(
        current,
        '    ProfileSpec {\n        name: "python-verify",',
        hardened_specs + '    ProfileSpec {\n        name: "python-verify",',
        "hardened Node profile-spec insertion",
    )
    current = replace_exact(
        current,
        '        for name in ["rust-verify", "node-verify", "python-verify"] {',
        '''        for name in [
            "rust-verify",
            "node-verify",
            "node-hardened-verify",
            "node-hardened-test",
            "python-verify",
        ] {''',
        "installed continuity-profile list",
    )
    current = replace_exact(
        current,
        "    #[test]\n    fn rust_verify_has_only_reviewed_monorepo_fallbacks() {",
        hardened_tests
        + "    #[test]\n    fn rust_verify_has_only_reviewed_monorepo_fallbacks() {",
        "hardened Node regression-test insertion",
    )

    reject_conflict_markers(PROFILES_PATH, current)
    for required in (
        "fn rust_verify_has_only_reviewed_monorepo_fallbacks",
        "remote/deployments/gha-clone-server-rs/Cargo.toml",
        "generated/rust/Cargo.toml",
        "fn hardened_node_profile_is_ordered_and_supply_chain_bounded",
        "fn hardened_node_test_profile_disables_lifecycle_scripts",
    ):
        if required not in current:
            raise SystemExit(f"fixed-profile union lost required marker: {required}")
    if "rust_verify_has_only_the_reviewed_meta_server_monorepo_fallback" in current:
        raise SystemExit("fixed-profile union retained the stale reviewed Rust fallback test")
    return current.encode()


def resolve_server_contract(
    current_data: bytes | None,
    reviewed_data: bytes | None,
) -> bytes:
    current = text(current_data, "current continuity TypeScript contract")
    reviewed = text(reviewed_data, "reviewed Messaging Intel TypeScript contract")

    for marker in (
        "const genericPlannerPath =",
        "secret-bearing setup inputs are unsupported",
        "fixed profiles do not forward caller-selected variables",
    ):
        if marker not in reviewed:
            raise SystemExit(f"reviewed TypeScript contract lost expected marker: {marker}")

    current = replace_exact(
        current,
        "const plannerPath = 'remote/deployments/gha-clone-server-rs/src/lib.rs';\n"
        "const serverPath = 'remote/deployments/gha-clone-server-rs/src/main.rs';",
        "const plannerPath = 'remote/deployments/gha-clone-server-rs/src/lib.rs';\n"
        "const genericPlannerPath =\n"
        "  'remote/deployments/gha-clone-server-rs/src/planner.rs';\n"
        "const serverPath = 'remote/deployments/gha-clone-server-rs/src/main.rs';",
        "generic planner contract path",
    )
    current = replace_exact(
        current,
        "  const planner = read(plannerPath);",
        "  const planner = read(genericPlannerPath);",
        "generic planner contract selection",
    )
    current = replace_exact(
        current,
        r"  assert.match(planner, /secret-bearing env\/with values are unsupported/);",
        "  assert.match(planner, /secret-bearing setup inputs are unsupported/);\n"
        "  assert.match(planner, /secret-bearing step environments are unsupported/);\n"
        "  assert.match(planner, /fixed profiles do not forward caller-selected variables/);",
        "generic planner secret-boundary assertions",
    )

    reject_conflict_markers(SERVER_CONTRACT_PATH, current)
    for required in (
        "genericPlannerPath",
        "secret-bearing setup inputs are unsupported",
        "secret-bearing step environments are unsupported",
        "fixed profiles do not forward caller-selected variables",
        "executor router code and live tests preserve no-duplicate provider pinning",
    ):
        if required not in current:
            raise SystemExit(f"TypeScript contract union lost required marker: {required}")
    return current.encode()


Resolver = Callable[[bytes | None, bytes | None], bytes]
RESOLVERS: dict[str, Resolver] = {
    WORKFLOW_PATH: resolve_workflow,
    PROFILES_PATH: resolve_profiles,
    SERVER_CONTRACT_PATH: resolve_server_contract,
}
