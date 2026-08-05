#!/usr/bin/env python3
"""Apply and validate the permanent StreemPilot media-router continuity slice.

This script is intentionally branch-scoped and deterministic. It adds one exact
workflow marker, one fixed build-server profile, one exact repository/profile
binding, and static contracts. It removes itself and its one-shot workflow
before the final product commit.
"""

from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PROFILES = ROOT / "remote/deployments/build-server-rs/src/profiles.rs"
PLANNER = ROOT / "remote/deployments/gha-clone-server-rs/src/lib.rs"
PATCH = ROOT / "remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml"
README = ROOT / "remote/deployments/gha-clone-server-rs/README.md"
CONTRACT = ROOT / "remote/tests/general/streempilot-media-router-continuity.test.mjs"
SELF = ROOT / "scripts/finalize-den-1757-streempilot-media-router-continuity.py"
WORKFLOW = ROOT / ".github/workflows/finalize-den-1757-streempilot-media-router-continuity.yml"

PROFILE_NAME = "streempilot-media-router-starter-verify"
REPOSITORY = "https://github.com/StreemPilot/streempilot-monorepo.git"
WORKFLOW_PATH = ".github/workflows/gha-clone-media-router.yml"


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


def update_profiles() -> None:
    text = PROFILES.read_text(encoding="utf-8")
    if PROFILE_NAME in text:
        raise SystemExit("profile already exists")

    steps = r'''const STREEMPILOT_MEDIA_ROUTER_STEPS: &[ProfileStep] = &[ProfileStep {
    name: "Deterministic StreemPilot media-router starter verification",
    image: RUST_IMAGE,
    subdirectory: ".",
    script: r#"set -euo pipefail
for path in \
  services/wave2-rust-services.v1.json \
  scripts/render-media-router-starter.py \
  scripts/render-formatted-media-router-starter.py \
  tests/test_media_router_starter.py \
  .github/workflows/gha-clone-media-router.yml; do
  test -f "$path" || {
    echo "streempilot-media-router-starter-verify requires $path" >&2
    exit 2
  }
done
command -v python3 >/dev/null
rustup component add rustfmt clippy
python3 -m py_compile \
  scripts/render-media-router-starter.py \
  scripts/render-formatted-media-router-starter.py
python3 -m unittest tests.test_media_router_starter -v
rm -rf .gha-continuity
mkdir -p .gha-continuity
python3 scripts/render-formatted-media-router-starter.py \
  --output .gha-continuity/media-router \
  --archive .gha-continuity/streempilot-media-router.tar.gz
test -f .gha-continuity/streempilot-media-router.tar.gz
test -f .gha-continuity/streempilot-media-router.tar.gz.sha256
cd .gha-continuity/media-router
cargo metadata --locked --offline --no-deps --format-version 1 >/dev/null
cargo fmt --all -- --check
cargo clippy --locked --offline --all-targets -- -D warnings
cargo test --locked --offline --all-targets
STREEMPILOT_MEDIA_ROUTER_MAX_DESTINATIONS=2 \
  cargo run --locked --offline --bin streempilot-media-router"#,
}];

'''
    text = replace_once(
        text,
        "const FLUTTER_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {",
        steps + "const FLUTTER_VERIFY_STEPS: &[ProfileStep] = &[ProfileStep {",
        "profile steps",
    )

    spec = '''    ProfileSpec {
        name: "streempilot-media-router-starter-verify",
        platform: "linux",
        description: "Deterministic StreemPilot media-router starter rendering and offline Rust verification",
        steps: STREEMPILOT_MEDIA_ROUTER_STEPS,
        artifact_paths: &[
            ".gha-continuity/streempilot-media-router.tar.gz",
            ".gha-continuity/streempilot-media-router.tar.gz.sha256",
        ],
    },
'''
    text = replace_once(
        text,
        '    ProfileSpec {\n        name: "node-verify",',
        spec + '    ProfileSpec {\n        name: "node-verify",',
        "profile spec",
    )
    text = replace_once(
        text,
        '            "rust-generated-verify",\n            "node-verify",',
        '            "rust-generated-verify",\n            "streempilot-media-router-starter-verify",\n            "node-verify",',
        "installed profile test",
    )

    test = r'''    #[test]
    fn streempilot_media_router_profile_is_exact_offline_and_non_publishing() {
        let profile = find("streempilot-media-router-starter-verify")
            .expect("StreemPilot media-router profile");
        assert_eq!(profile.steps.len(), 1);
        assert_eq!(profile.steps[0].image, RUST_IMAGE);
        assert_eq!(profile.steps[0].subdirectory, ".");
        assert_eq!(
            profile.artifact_paths,
            &[
                ".gha-continuity/streempilot-media-router.tar.gz",
                ".gha-continuity/streempilot-media-router.tar.gz.sha256",
            ]
        );
        let script = profile.steps[0].script;
        for required in [
            "services/wave2-rust-services.v1.json",
            "scripts/render-formatted-media-router-starter.py",
            "python3 -m unittest tests.test_media_router_starter -v",
            "cargo metadata --locked --offline",
            "cargo clippy --locked --offline --all-targets -- -D warnings",
            "cargo test --locked --offline --all-targets",
            "STREEMPILOT_MEDIA_ROUTER_MAX_DESTINATIONS=2",
        ] {
            assert!(script.contains(required), "profile missing {required}");
        }
        for forbidden in [
            "cargo publish",
            "kubectl",
            "curl",
            "wget",
            "git clone",
            "docker",
            "nerdctl",
            "|| true",
        ] {
            assert!(!script.contains(forbidden), "profile contains {forbidden}");
        }
    }

'''
    text = replace_once(
        text,
        "    #[test]\n    fn generated_rust_profile_is_locked_ordered_and_non_publishing() {",
        test + "    #[test]\n    fn generated_rust_profile_is_locked_ordered_and_non_publishing() {",
        "profile unit test",
    )
    PROFILES.write_text(text, encoding="utf-8")


def update_planner() -> None:
    text = PLANNER.read_text(encoding="utf-8")
    if PROFILE_NAME in text:
        raise SystemExit("planner profile already exists")
    text = replace_once(
        text,
        '            "rust-verify".to_string(),\n            "node-verify".to_string(),',
        '            "rust-verify".to_string(),\n            "streempilot-media-router-starter-verify".to_string(),\n            "node-verify".to_string(),',
        "capability profile",
    )
    text = replace_once(
        text,
        "fn classify_profile(text: &str) -> Option<String> {\n    if text.contains(\"flutter\") {",
        "fn classify_profile(text: &str) -> Option<String> {\n    if text.contains(\"streempilot-media-router-starter-verify\") {\n        return Some(\"streempilot-media-router-starter-verify\".into());\n    }\n    if text.contains(\"flutter\") {",
        "profile classifier",
    )

    test = r'''    #[test]
    fn exact_streempilot_media_router_marker_maps_to_one_fixed_profile() {
        let mut request = request(
            r#"
name: Media router independent continuity mirror
on:
  workflow_dispatch:
jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - run: echo "cargo test --locked # streempilot-media-router-starter-verify"
"#,
        );
        request.repository = "StreemPilot/streempilot-monorepo".into();
        request.workflow_path = ".github/workflows/gha-clone-media-router.yml".into();
        let plan = build_plan(&request, &PlannerLimits::default()).expect("valid plan");
        assert!(plan.independent_executable);
        assert_eq!(plan.jobs.len(), 1);
        assert_eq!(
            plan.jobs[0].independent_profile.as_deref(),
            Some("streempilot-media-router-starter-verify")
        );

        request.workflow_yaml = request
            .workflow_yaml
            .replace("streempilot-media-router-starter-verify", "streempilot-media-router-starter-verify-adjacent");
        let adjacent = build_plan(&request, &PlannerLimits::default()).expect("valid plan");
        assert!(!adjacent.independent_executable);
        assert_ne!(
            adjacent.jobs[0].independent_profile.as_deref(),
            Some("streempilot-media-router-starter-verify")
        );
    }

'''
    text = replace_once(
        text,
        "    #[test]\n    fn maps_static_rust_node_python_dag_to_fixed_profiles() {",
        test + "    #[test]\n    fn maps_static_rust_node_python_dag_to_fixed_profiles() {",
        "planner unit test",
    )
    PLANNER.write_text(text, encoding="utf-8")


def update_patch() -> None:
    text = PATCH.read_text(encoding="utf-8")
    text = replace_once(
        text,
        "value: rust-verify,rust-generated-verify,node-verify",
        "value: rust-verify,rust-generated-verify,streempilot-media-router-starter-verify,node-verify",
        "allowed profile list",
    )
    marker = '{"repository":"https://github.com/3FA-app/3fa-interfaces.git","profiles":["node-hardened-test","rust-generated-verify"]}'
    replacement = marker + ',{"repository":"https://github.com/StreemPilot/streempilot-monorepo.git","profiles":["streempilot-media-router-starter-verify"]}'
    text = replace_once(text, marker, replacement, "exact repository profile binding")
    PATCH.write_text(text, encoding="utf-8")


def update_readme() -> None:
    text = README.read_text(encoding="utf-8")
    row = "| StreemPilot media-router starter marker | `streempilot-media-router-starter-verify` |\n"
    text = replace_once(
        text,
        "| Cargo/rustfmt/Clippy/tests | `rust-verify` |\n",
        "| Cargo/rustfmt/Clippy/tests | `rust-verify` |\n" + row,
        "profile documentation",
    )
    appendix = f'''\n## StreemPilot media-router continuity\n\nThe exact repository `{REPOSITORY}` and workflow `{WORKFLOW_PATH}` may map the\nstatic marker `streempilot-media-router-starter-verify` to one fixed profile.\nThe profile renders the reviewed starter, runs its Python contracts, then runs\nlocked offline Rust metadata, formatting, strict Clippy, tests, and a bounded\nstartup probe. It emits only the canonical starter archive and checksum.\n\nThe mapping does not accept caller-selected commands, directories, images,\nproviders, or credentials. The clone-server deployment remains zero-replica\nand both API and webhook execution remain disabled until scoped GitHub App and\nbuild-server credentials, network policy, immutable runtime images, and an\noperator smoke are present.\n'''
    if "## StreemPilot media-router continuity" in text:
        raise SystemExit("README section already exists")
    README.write_text(text.rstrip() + "\n" + appendix, encoding="utf-8")


def write_contract() -> None:
    CONTRACT.write_text(
        '''import assert from "node:assert/strict";\nimport { readFileSync } from "node:fs";\nimport test from "node:test";\n\nconst config = readFileSync("remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml", "utf8");\nconst patch = readFileSync("remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml", "utf8");\nconst profiles = readFileSync("remote/deployments/build-server-rs/src/profiles.rs", "utf8");\nconst planner = readFileSync("remote/deployments/gha-clone-server-rs/src/lib.rs", "utf8");\n\ntest("StreemPilot media-router continuity is exact and inert", () => {\n  assert.match(config, /StreemPilot\\/streempilot-monorepo/);\n  assert.match(config, /\\.github\\/workflows\\/gha-clone-media-router\\.yml/);\n  assert.equal((config.match(/StreemPilot\\/streempilot-monorepo/g) || []).length, 2);\n  assert.match(patch, /streempilot-media-router-starter-verify/);\n  assert.match(patch, /https:\\/\\/github\\.com\\/StreemPilot\\/streempilot-monorepo\\.git/);\n  assert.match(profiles, /python3 -m unittest tests\\.test_media_router_starter -v/);\n  assert.match(profiles, /cargo clippy --locked --offline --all-targets -- -D warnings/);\n  assert.match(planner, /text\\.contains\\("streempilot-media-router-starter-verify"\\)/);\n  assert.doesNotMatch(config, /streempilot-media-router-starter-verify-adjacent/);\n});\n''',
        encoding="utf-8",
    )


def verify_config_json() -> None:
    text = (ROOT / "remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml").read_text(encoding="utf-8")
    marker = "  GHA_CLONE_WORKFLOW_RULES_JSON: |\n"
    block = text.split(marker, 1)[1]
    json_text = "\n".join(line[4:] for line in block.splitlines() if line.startswith("    "))
    rules = json.loads(json_text)
    if rules.get("StreemPilot/streempilot-monorepo") != [WORKFLOW_PATH]:
        raise SystemExit("clone-server workflow rule is not exact")


def main() -> None:
    update_profiles()
    update_planner()
    update_patch()
    update_readme()
    write_contract()
    verify_config_json()
    SELF.unlink()
    WORKFLOW.unlink()


if __name__ == "__main__":
    main()
