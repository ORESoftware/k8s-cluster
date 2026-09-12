#!/usr/bin/env python3
"""Resolve the three reviewed DEN-1550 merge conflicts by semantic union."""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

CONFLICT = re.compile(
    r"<<<<<<< HEAD\n(.*?)=======\n(.*?)>>>>>>> [^\n]+\n",
    re.DOTALL,
)


def replace_once(source: str, old: str, new: str, label: str) -> str:
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return source.replace(old, new, 1)


def union_lines(left: str, right: str) -> str:
    result: list[str] = []
    seen: set[str] = set()
    for line in (left + right).splitlines(keepends=True):
        if line not in seen:
            result.append(line)
            seen.add(line)
    return "".join(result)


def resolve_workflow() -> None:
    path = Path(".github/workflows/gha-clone-server.yml")
    source = path.read_text(encoding="utf-8")
    blocks = list(CONFLICT.finditer(source))
    if len(blocks) != 7:
        raise SystemExit(f"expected seven workflow conflicts, found {len(blocks)}")

    rust_test_name = (
        "      - name: Run unit, process, webhook, meta, StreemPilot mirror, "
        "and Messaging Intel integration tests\n"
    )
    profile_test_name = (
        "      - name: Test fixed-profile registry, reviewed fallbacks, and "
        "Messaging Intel hardened profiles\n"
    )
    contract_name = (
        "      - name: Validate deployment, routing, execution, webhook, "
        "activation, StreemPilot, and Messaging Intel boundaries\n"
    )
    contract_body = "\n".join(
        [
            "            general/gha-clone-streempilot-config.test.ts \\",
            "            general/gha-clone-msgint-config.test.ts",
            "          node --test general/gha-executor-router-activation.test.mjs",
            "      - name: Install pinned kubectl renderer",
            "        uses: azure/setup-kubectl@829323503d1be3d00ca8346e5391ca0b07a9ab0d # v5",
            "        with:",
            "          version: v1.32.2",
            "      - name: Render the complete continuity overlay",
            "        run: |",
            "          set -euo pipefail",
            '          rendered="${RUNNER_TEMP}/dd-next-runtime.yaml"',
            '          kubectl kustomize remote/argocd/dd-next-runtime >"$rendered"',
            '          test -s "$rendered"',
            "          grep -F 'name: dd-gha-clone-server' \"$rendered\"",
            "          grep -F 'name: dd-gha-executor-router' \"$rendered\"",
            "          test \"$(grep -c 'replicas: 0' \"$rendered\")\" -ge 2",
        ]
    ) + "\n"
    credential_scan = "\n".join(
        [
            "            docs/gha-executor-router-activation.md \\",
            "            docs/streempilot-ci-continuity.md \\",
            "            docs/gha-profile-repository-admission.md; then",
        ]
    ) + "\n"

    def replacement(match: re.Match[str]) -> str:
        index = replacement.index
        replacement.index += 1
        left, right = match.group(1), match.group(2)
        if index in (0, 1):
            return union_lines(left, right)
        if index == 2:
            return rust_test_name
        if index == 3:
            return profile_test_name
        if index == 4:
            return contract_name
        if index == 5:
            return contract_body
        if index == 6:
            return credential_scan
        raise AssertionError(index)

    replacement.index = 0  # type: ignore[attr-defined]
    resolved = CONFLICT.sub(replacement, source)
    if any(marker in resolved for marker in ("<<<<<<<", "=======", ">>>>>>>")):
        raise SystemExit("workflow conflict markers remain")
    for required in (
        "general/gha-executor-router-activation.test.mjs",
        "general/gha-clone-streempilot-config.test.ts",
        "general/gha-clone-msgint-config.test.ts",
        "docs/gha-executor-router-activation.md",
        "docs/streempilot-ci-continuity.md",
        "docs/gha-profile-repository-admission.md",
    ):
        if required not in resolved:
            raise SystemExit(f"resolved workflow omitted {required}")
    path.write_text(resolved, encoding="utf-8")


def resolve_profiles() -> None:
    path = Path("remote/deployments/build-server-rs/src/profiles.rs")
    subprocess.run(
        ["git", "checkout", "--ours", "--", str(path)],
        check=True,
    )
    source = path.read_text(encoding="utf-8")

    hardened_steps = r'''const NODE_HARDENED_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {
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
    source = replace_once(
        source,
        "const PYTHON_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {",
        hardened_steps + "const PYTHON_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {",
        "hardened profile step insertion",
    )

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
    source = replace_once(
        source,
        '''    ProfileSpec {
        name: "python-verify",''',
        hardened_specs
        + '''    ProfileSpec {
        name: "python-verify",''',
        "hardened profile registration",
    )

    source = replace_once(
        source,
        '        for name in ["rust-verify", "node-verify", "python-verify"] {',
        '''        for name in [
            "rust-verify",
            "node-verify",
            "node-hardened-verify",
            "node-hardened-test",
            "python-verify",
        ] {''',
        "hardened profile inventory",
    )

    hardened_tests = r'''    #[test]
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
    source = replace_once(
        source,
        '''    #[test]
    fn rust_verify_has_only_reviewed_monorepo_fallbacks() {''',
        hardened_tests
        + '''    #[test]
    fn rust_verify_has_only_reviewed_monorepo_fallbacks() {''',
        "hardened profile regression tests",
    )

    for preserved in (
        "generated/rust/Cargo.toml",
        "check:typescript",
        "pnpm install --frozen-lockfile",
    ):
        if preserved not in source:
            raise SystemExit(f"current-dev profile behavior was lost: {preserved}")
    path.write_text(source, encoding="utf-8")


def resolve_typescript_contract() -> None:
    path = Path("remote/tests/general/gha-clone-server-config.test.ts")
    source = path.read_text(encoding="utf-8")
    blocks = list(CONFLICT.finditer(source))
    if len(blocks) != 1:
        raise SystemExit(f"expected one TypeScript conflict, found {len(blocks)}")

    resolved = CONFLICT.sub(lambda match: match.group(2), source, count=1)
    if any(marker in resolved for marker in ("<<<<<<<", "=======", ">>>>>>>")):
        raise SystemExit("TypeScript conflict markers remain")
    required = (
        "const genericPlannerPath =",
        "const planner = read(genericPlannerPath);",
        "const observabilityPaths = [",
        "const routerSourcePaths = [",
        "const routerTestPaths = [",
    )
    for marker in required:
        if marker not in resolved:
            raise SystemExit(f"resolved TypeScript contract omitted {marker}")
    path.write_text(resolved, encoding="utf-8")


def main() -> None:
    resolve_workflow()
    resolve_profiles()
    resolve_typescript_contract()
    print("resolved DEN-1550 current-dev conflict set")


if __name__ == "__main__":
    main()
