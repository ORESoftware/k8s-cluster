#!/usr/bin/env python3
from __future__ import annotations

import json
import re
from pathlib import Path

PROFILE = "streempilot-media-router-starter-verify"
REPOSITORY = "StreemPilot/streempilot-monorepo"
REPOSITORY_URL = "https://github.com/StreemPilot/streempilot-monorepo.git"
WORKFLOW_PATH = ".github/workflows/gha-clone-media-router.yml"

PROFILES = Path("remote/deployments/build-server-rs/src/profiles.rs")
PLANNER = Path("remote/deployments/gha-clone-server-rs/src/lib.rs")
PATCH = Path("remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml")
CONFIG = Path("remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml")
README = Path("remote/deployments/gha-clone-server-rs/README.md")
STREEMPILOT_DOC = Path("docs/streempilot-ci-continuity.md")
POLICY_DOC = Path("docs/build-server-profile-repository-policy.md")
POLICY_TEST = Path("remote/tests/general/build-server-profile-repository-policy.test.mjs")
CONTRACT_TEST = Path("remote/tests/general/streempilot-media-router-continuity.test.mjs")
PERMANENT_WORKFLOW = Path(".github/workflows/gha-clone-server.yml")


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def write(path: Path, content: str) -> None:
    path.write_text(content, encoding="utf-8")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one anchor, found {count}")
    return text.replace(old, new, 1)


def insert_before(text: str, marker: str, addition: str, label: str) -> str:
    if addition.strip() in text:
        raise SystemExit(f"{label}: addition already exists")
    return replace_once(text, marker, addition + marker, label)


def update_profiles() -> None:
    text = read(PROFILES)
    if PROFILE in text:
        raise SystemExit("profiles.rs already contains the media-router profile")

    steps = r'''const STREEMPILOT_MEDIA_ROUTER_STEPS: &[ProfileStep] = &[
    ProfileStep {
        name: "Validate and render the StreemPilot media-router starter",
        image: PYTHON_IMAGE,
        subdirectory: ".",
        script: r#"set -euo pipefail
for required in \
  services/wave2-rust-services.v1.json \
  scripts/render-media-router-starter.py \
  scripts/render-formatted-media-router-starter.py \
  tests/test_media_router_starter.py \
  .github/workflows/gha-clone-media-router.yml
do
  if [ ! -f "$required" ]; then
    echo "streempilot-media-router-starter-verify requires $required" >&2
    exit 2
  fi
done
python3 -m py_compile \
  scripts/render-media-router-starter.py \
  scripts/render-formatted-media-router-starter.py
python3 -m unittest tests.test_media_router_starter -v
rm -rf .gha-continuity
mkdir -p .gha-continuity
python3 scripts/render-media-router-starter.py \
  --output .gha-continuity/media-router
test -s .gha-continuity/media-router/Cargo.toml
test -s .gha-continuity/media-router/Cargo.lock"#,
    },
    ProfileStep {
        name: "Format and verify the generated media-router crate offline",
        image: RUST_IMAGE,
        subdirectory: ".gha-continuity/media-router",
        script: r#"set -euo pipefail
rustup component add rustfmt clippy
cargo metadata --locked --offline --no-deps --format-version 1 >/dev/null
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --locked --offline --all-targets -- -D warnings
cargo test --locked --offline --all-targets
set +e
timeout --signal=TERM 10s env \
  STREEMPILOT_MEDIA_ROUTER_MAX_DESTINATIONS=2 \
  cargo run --locked --offline --bin streempilot-media-router
status=$?
set -e
case "$status" in
  0|124|143) ;;
  *) exit "$status" ;;
esac"#,
    },
    ProfileStep {
        name: "Archive the verified media-router starter deterministically",
        image: PYTHON_IMAGE,
        subdirectory: ".",
        script: r#"set -euo pipefail
python3 - <<'PY'
from __future__ import annotations

import gzip
import hashlib
import tarfile
from pathlib import Path

root = Path(".gha-continuity/media-router")
archive = Path(".gha-continuity/streempilot-media-router.tar.gz")
checksum = Path(f"{archive}.sha256")
if not root.is_dir():
    raise SystemExit("verified media-router output is missing")

with archive.open("wb") as raw:
    with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as tar:
            for path in sorted(root.rglob("*"), key=lambda candidate: candidate.as_posix()):
                if path.is_symlink() or (not path.is_dir() and not path.is_file()):
                    raise SystemExit(f"unsupported generated entry: {path}")
                relative = path.relative_to(root).as_posix()
                info = tar.gettarinfo(str(path), arcname=f"media-router/{relative}")
                info.uid = 0
                info.gid = 0
                info.uname = ""
                info.gname = ""
                info.mtime = 0
                info.mode = 0o755 if path.is_dir() else 0o644
                if path.is_file():
                    with path.open("rb") as source:
                        tar.addfile(info, source)
                else:
                    tar.addfile(info)

digest = hashlib.sha256(archive.read_bytes()).hexdigest()
checksum.write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
PY
test -s .gha-continuity/streempilot-media-router.tar.gz
test -s .gha-continuity/streempilot-media-router.tar.gz.sha256"#,
    },
];

'''
    text = insert_before(
        text,
        "const FLUTTER_VERIFY_STEPS: &[ProfileStep]",
        steps,
        "insert media-router profile steps",
    )

    spec = r'''    ProfileSpec {
        name: "streempilot-media-router-starter-verify",
        platform: "linux",
        description:
            "Deterministic StreemPilot media-router starter rendering and offline Rust verification",
        steps: STREEMPILOT_MEDIA_ROUTER_STEPS,
        artifact_paths: &[
            ".gha-continuity/streempilot-media-router.tar.gz",
            ".gha-continuity/streempilot-media-router.tar.gz.sha256",
        ],
    },
'''
    text = insert_before(
        text,
        '    ProfileSpec {\n        name: "node-verify",',
        spec,
        "insert media-router ProfileSpec",
    )

    text = replace_once(
        text,
        '            "rust-generated-verify",\n            "node-verify",',
        '            "rust-generated-verify",\n'
        '            "streempilot-media-router-starter-verify",\n'
        '            "node-verify",',
        "install media-router profile",
    )

    test = r'''    #[test]
    fn streempilot_media_router_profile_is_fixed_offline_bounded_and_non_publishing() {
        let profile = find("streempilot-media-router-starter-verify")
            .expect("StreemPilot media-router profile");
        assert_eq!(profile.steps.len(), 3);
        assert_eq!(profile.steps[0].image, PYTHON_IMAGE);
        assert_eq!(profile.steps[0].subdirectory, ".");
        assert_eq!(profile.steps[1].image, RUST_IMAGE);
        assert_eq!(
            profile.steps[1].subdirectory,
            ".gha-continuity/media-router"
        );
        assert_eq!(profile.steps[2].image, PYTHON_IMAGE);
        assert_eq!(profile.steps[2].subdirectory, ".");
        assert_eq!(
            profile.artifact_paths,
            [
                ".gha-continuity/streempilot-media-router.tar.gz",
                ".gha-continuity/streempilot-media-router.tar.gz.sha256",
            ]
        );

        let render = profile.steps[0].script;
        for evidence in [
            "services/wave2-rust-services.v1.json",
            "scripts/render-media-router-starter.py",
            "scripts/render-formatted-media-router-starter.py",
            "tests/test_media_router_starter.py",
            ".github/workflows/gha-clone-media-router.yml",
            "python3 -m py_compile",
            "python3 -m unittest tests.test_media_router_starter -v",
            "--output .gha-continuity/media-router",
        ] {
            assert!(render.contains(evidence), "missing render evidence: {evidence}");
        }

        let verify = profile.steps[1].script;
        for evidence in [
            "cargo metadata --locked --offline --no-deps",
            "cargo fmt --all -- --check",
            "cargo clippy --locked --offline --all-targets -- -D warnings",
            "cargo test --locked --offline --all-targets",
            "timeout --signal=TERM 10s",
            "STREEMPILOT_MEDIA_ROUTER_MAX_DESTINATIONS=2",
            "cargo run --locked --offline --bin streempilot-media-router",
            "0|124|143",
        ] {
            assert!(verify.contains(evidence), "missing verify evidence: {evidence}");
        }

        let archive = profile.steps[2].script;
        for evidence in [
            "gzip.GzipFile",
            "mtime=0",
            "tarfile.PAX_FORMAT",
            "info.uid = 0",
            "info.gid = 0",
            "hashlib.sha256",
        ] {
            assert!(archive.contains(evidence), "missing archive evidence: {evidence}");
        }

        let all_scripts = profile
            .steps
            .iter()
            .map(|step| step.script)
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in [
            "cargo publish",
            "kubectl",
            "curl ",
            "wget ",
            "apt-get",
            "pip install",
            "git clone",
            "docker ",
            "nerdctl",
            "|| true",
        ] {
            assert!(
                !all_scripts.contains(forbidden),
                "forbidden media-router command: {forbidden}"
            );
        }
    }

'''
    text = insert_before(
        text,
        "    #[test]\n    fn generated_rust_profile_is_locked_ordered_and_non_publishing()",
        test,
        "insert media-router profile unit test",
    )
    write(PROFILES, text)


def update_planner() -> None:
    text = read(PLANNER)
    if PROFILE in text:
        raise SystemExit("planner already contains the media-router marker")

    text = replace_once(
        text,
        '        independent_profiles: vec![\n            "rust-verify".to_string(),\n            "node-verify".to_string(),',
        '        independent_profiles: vec![\n'
        '            "rust-verify".to_string(),\n'
        '            "streempilot-media-router-starter-verify".to_string(),\n'
        '            "node-verify".to_string(),',
        "advertise media-router capability",
    )

    helper = r'''const STREEMPILOT_MEDIA_ROUTER_PROFILE: &str =
    "streempilot-media-router-starter-verify";

fn contains_exact_profile_marker(text: &str, marker: &str) -> bool {
    text.split(|character: char| {
        !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    })
    .any(|token| token == marker)
}

'''
    text = insert_before(
        text,
        "fn classify_profile(text: &str) -> Option<String> {",
        helper,
        "insert exact profile-marker helper",
    )
    text = replace_once(
        text,
        "fn classify_profile(text: &str) -> Option<String> {\n    if text.contains(\"flutter\") {",
        "fn classify_profile(text: &str) -> Option<String> {\n"
        "    if contains_exact_profile_marker(text, STREEMPILOT_MEDIA_ROUTER_PROFILE) {\n"
        "        return Some(STREEMPILOT_MEDIA_ROUTER_PROFILE.into());\n"
        "    }\n"
        "    if text.contains(\"flutter\") {",
        "classify exact media-router marker first",
    )

    test = r'''    #[test]
    fn maps_only_the_exact_streempilot_media_router_marker() {
        let exact = PlanRequest {
            repository: "StreemPilot/streempilot-monorepo".into(),
            revision: "0123456789abcdef0123456789abcdef01234567".into(),
            workflow_path: ".github/workflows/gha-clone-media-router.yml".into(),
            workflow_yaml: r#"
name: StreemPilot media-router continuity
on:
  workflow_dispatch:
jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - run: echo "streempilot-media-router-starter-verify"
"#
            .into(),
        };
        let plan =
            build_plan(&exact, &PlannerLimits::default()).expect("exact marker should plan");
        assert!(plan.independent_executable);
        assert_eq!(
            plan.jobs[0].independent_profile.as_deref(),
            Some("streempilot-media-router-starter-verify")
        );

        let mut adjacent = exact;
        adjacent.workflow_yaml = adjacent.workflow_yaml.replace(
            "streempilot-media-router-starter-verify",
            "streempilot-media-router-starter-verify-adjacent",
        );
        let plan = build_plan(&adjacent, &PlannerLimits::default())
            .expect("adjacent marker should still produce a reviewable plan");
        assert!(!plan.independent_executable);
        assert_eq!(plan.jobs[0].independent_profile, None);
        assert!(plan.jobs[0]
            .independent_reasons
            .iter()
            .any(|reason| reason.contains("no fixed build-server profile")));
    }

'''
    text = insert_before(
        text,
        "    #[test]\n    fn maps_static_rust_node_python_dag_to_fixed_profiles()",
        test,
        "insert exact media-router planner test",
    )
    write(PLANNER, text)


def update_patch() -> None:
    text = read(PATCH)
    if PROFILE in text or REPOSITORY_URL in text:
        raise SystemExit("build-server patch already contains media-router policy")

    lines = text.splitlines()
    allowed_index = next(
        index
        for index, line in enumerate(lines)
        if line.strip() == "- name: BUILD_SERVER_ALLOWED_PROFILES"
    )
    value_index = allowed_index + 1
    prefix = lines[value_index].split("value:", 1)[0] + "value: "
    profiles = lines[value_index].split("value:", 1)[1].strip().split(",")
    insertion = profiles.index("rust-generated-verify") + 1
    profiles.insert(insertion, PROFILE)
    lines[value_index] = prefix + ",".join(profiles)

    rules_index = next(
        index
        for index, line in enumerate(lines)
        if line.strip() == "- name: BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON"
    )
    json_index = rules_index + 2
    indent = lines[json_index][:-len(lines[json_index].lstrip())]
    rules = json.loads(lines[json_index].strip())
    if any(rule["repository"].lower() == REPOSITORY_URL.lower() for rule in rules):
        raise SystemExit("media-router exact repository rule already exists")
    zed_index = next(
        (
            index
            for index, rule in enumerate(rules)
            if rule["repository"].endswith("/zed-pkg/zed-cli.git")
        ),
        len(rules),
    )
    rules.insert(zed_index, {"repository": REPOSITORY_URL, "profiles": [PROFILE]})
    lines[json_index] = indent + json.dumps(rules, separators=(",", ":"))
    write(PATCH, "\n".join(lines) + "\n")


def update_config() -> None:
    text = read(CONFIG)
    if REPOSITORY in text:
        raise SystemExit("clone-server config already contains StreemPilot monorepo")

    text = replace_once(
        text,
        "    StreemPilot/streempilot-interfaces,\n    3FA-app/3fa-interfaces,",
        "    StreemPilot/streempilot-interfaces,\n"
        "    StreemPilot/streempilot-monorepo,\n"
        "    3FA-app/3fa-interfaces,",
        "allowlist StreemPilot monorepo",
    )
    text = replace_once(
        text,
        '      "StreemPilot/streempilot-interfaces": [".github/workflows/ci-mirror.yml"],\n'
        '      "3FA-app/3fa-interfaces":',
        '      "StreemPilot/streempilot-interfaces": [".github/workflows/ci-mirror.yml"],\n'
        '      "StreemPilot/streempilot-monorepo": [".github/workflows/gha-clone-media-router.yml"],\n'
        '      "3FA-app/3fa-interfaces":',
        "bind exact media-router workflow path",
    )
    write(CONFIG, text)


def update_readme() -> None:
    text = read(README)
    if PROFILE in text:
        raise SystemExit("README already documents media-router profile")

    text = replace_once(
        text,
        "| Python compile/pytest | `python-verify` |\n"
        "| Flutter analyze/tests | `flutter-verify` |",
        "| Python compile/pytest | `python-verify` |\n"
        "| Exact StreemPilot media-router marker | `streempilot-media-router-starter-verify` |\n"
        "| Flutter analyze/tests | `flutter-verify` |",
        "add media-router profile table row",
    )
    section = f'''### StreemPilot media-router starter continuity

Only `{REPOSITORY}` at `{WORKFLOW_PATH}` may request the exact static marker
`{PROFILE}`. The marker is a token, not a substring: lookalike or suffixed
markers do not select the profile.

The fixed profile never executes workflow-provided commands. It uses a Python
step to validate and render the reviewed starter, a Rust step to format and
verify the generated crate with locked offline Cargo commands and a bounded
startup probe, and a final Python step to create a metadata-normalized archive
plus SHA-256 checksum. The only exported artifacts are:

- `.gha-continuity/streempilot-media-router.tar.gz`;
- `.gha-continuity/streempilot-media-router.tar.gz.sha256`.

The build-server policy binds the exact canonical monorepo URL to this profile
alone. It does not grant sibling repositories, generic Rust profiles,
caller-selected images, providers, credentials, commands, publication, or
deployment authority.

'''
    text = insert_before(
        text,
        "## Accepted governance keys",
        section,
        "insert media-router README section",
    )
    write(README, text)


def update_streempilot_documentation() -> None:
    text = read(STREEMPILOT_DOC)
    if PROFILE in text:
        raise SystemExit("StreemPilot continuity doc already contains media-router profile")

    text = replace_once(
        text,
        "| `StreemPilot/streempilot-interfaces` | `.github/workflows/ci-mirror.yml` | lockfile-strict Node contracts/TypeScript, then generated Rust bindings |\n",
        "| `StreemPilot/streempilot-interfaces` | `.github/workflows/ci-mirror.yml` | lockfile-strict Node contracts/TypeScript, then generated Rust bindings |\n"
        "| `StreemPilot/streempilot-monorepo` | `.github/workflows/gha-clone-media-router.yml` | exact marker → deterministic media-router starter render, offline Rust verification, archive and checksum |\n",
        "add media-router repository coverage",
    )

    section = f'''## Media-router starter contract

The monorepo path is a dedicated fixed-profile contract, not generic monorepo
execution. The planner recognizes only the exact token `{PROFILE}`. A
prefix, suffix, or lookalike marker does not select the profile.

`dd-build-server` then enforces a second independent boundary: the canonical
`{REPOSITORY_URL}` identity is bound only to `{PROFILE}`. Even if a lookalike
workflow maps to `rust-verify`, the exact repository policy rejects that
downgrade.

The profile uses separate pinned Python and Rust images so no step downloads a
second language toolchain. It validates the reviewed service manifest, renderers,
unit test, and workflow path; renders into an ephemeral `.gha-continuity` tree;
formats and verifies the generated crate with locked offline Cargo commands; runs
a ten-second bounded startup probe; and archives the verified tree with normalized
ownership, modes, and timestamps. It cannot publish, deploy, clone another
repository, select a provider, or execute workflow-supplied commands.

'''
    text = insert_before(
        text,
        "## Deliberate exclusions",
        section,
        "insert media-router contract documentation",
    )

    text = replace_once(
        text,
        "- eight Rust planner tests against the exact three mirror fixtures;\n"
        "- five TypeScript deployment/profile contracts;",
        "- planner tests against the three mirror fixtures plus exact-token and adjacent-marker media-router cases;\n"
        "- deployment/profile contracts for the mirror repositories and the media-router exact policy;",
        "update StreemPilot test inventory",
    )
    write(STREEMPILOT_DOC, text)


def update_policy_documentation() -> None:
    text = read(POLICY_DOC)
    if REPOSITORY_URL in text:
        raise SystemExit("profile-policy documentation already contains StreemPilot monorepo")

    text = replace_once(
        text,
        '  {\n'
        '    "repository": "https://github.com/zed-pkg/zed-cli.git",\n'
        '    "profiles": ["rust-verify"]\n'
        "  }\n",
        '  {\n'
        f'    "repository": "{REPOSITORY_URL}",\n'
        f'    "profiles": ["{PROFILE}"]\n'
        "  },\n"
        '  {\n'
        '    "repository": "https://github.com/zed-pkg/zed-cli.git",\n'
        '    "profiles": ["rust-verify"]\n'
        "  }\n",
        "add media-router JSON policy example",
    )

    section = f'''The StreemPilot media-router rule binds:

```text
{REPOSITORY_URL} -> {PROFILE}
```

This binding accepts only the dedicated compiled profile. It rejects generic
`rust-verify`, Node, Python, browser, Flutter, sibling StreemPilot repositories,
and lookalike monorepo names. The profile renders a reviewed generated crate,
verifies it with locked offline Cargo commands, performs a bounded startup probe,
and emits only a deterministic archive and checksum. Workflow text supplies the
exact marker but never supplies executable commands, images, provider placement,
credentials, publication, or deployment behavior.

'''
    text = insert_before(
        text,
        "The Zed CLI continuity rule binds:",
        section,
        "add reviewed media-router binding",
    )

    text = replace_once(
        text,
        "The GitOps contract test parses the complete JSON policy and verifies that `k8s-cluster` receives only `rust-verify`, `msgint-connectors` receives only `node-hardened-verify` and `node-hardened-test`, `3fa-interfaces` receives only `node-hardened-test` and `rust-generated-verify`, and `zed-cli` receives only `rust-verify`.",
        "The GitOps contract test parses the complete JSON policy and verifies that `k8s-cluster` receives only `rust-verify`, `msgint-connectors` receives only `node-hardened-verify` and `node-hardened-test`, `3fa-interfaces` receives only `node-hardened-test` and `rust-generated-verify`, `streempilot-monorepo` receives only `streempilot-media-router-starter-verify`, and `zed-cli` receives only `rust-verify`.",
        "update policy test-contract prose",
    )
    write(POLICY_DOC, text)


def update_policy_test() -> None:
    text = read(POLICY_TEST)
    if REPOSITORY_URL in text:
        raise SystemExit("profile-policy static test already contains StreemPilot rule")

    text = replace_once(
        text,
        "    {\n"
        "      repository: 'https://github.com/zed-pkg/zed-cli.git',\n"
        "      profiles: ['rust-verify'],\n"
        "    },\n",
        "    {\n"
        f"      repository: '{REPOSITORY_URL}',\n"
        f"      profiles: ['{PROFILE}'],\n"
        "    },\n"
        "    {\n"
        "      repository: 'https://github.com/zed-pkg/zed-cli.git',\n"
        "      profiles: ['rust-verify'],\n"
        "    },\n",
        "expect media-router exact rule",
    )

    denial = f'''  for (const denied of [
    'rust-verify',
    'node-verify',
    'python-verify',
    'playwright',
    'flutter-verify',
  ]) {{
    assert.equal(
      byRepository.get('{REPOSITORY_URL}').includes(denied),
      false,
      `StreemPilot monorepo unexpectedly admits ${{denied}}`,
    );
  }}
'''
    text = insert_before(
        text,
        "  for (const denied of ['node-verify', 'playwright', 'python-verify', 'flutter-verify']) {",
        denial,
        "add media-router downgrade denials",
    )

    text = replace_once(
        text,
        "  assert.doesNotMatch(patch, /messaging-intel\\/\\*|3FA-app\\/\\*|zed-pkg\\/\\*/);",
        "  assert.doesNotMatch(\n"
        "    patch,\n"
        "    /messaging-intel\\/\\*|3FA-app\\/\\*|StreemPilot\\/\\*|zed-pkg\\/\\*/,\n"
        "  );",
        "reject StreemPilot wildcard policy",
    )

    text = replace_once(
        text,
        "  assert.match(documentation, /zed-cli\\.git -> rust-verify/);\n",
        "  assert.match(\n"
        "    documentation,\n"
        "    /streempilot-monorepo\\.git -> streempilot-media-router-starter-verify/,\n"
        "  );\n"
        "  assert.match(documentation, /zed-cli\\.git -> rust-verify/);\n",
        "require media-router policy documentation",
    )
    write(POLICY_TEST, text)


def create_contract_test() -> None:
    if CONTRACT_TEST.exists():
        raise SystemExit("media-router static contract test already exists")
    content = r'''import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';

const root = join(import.meta.dirname, '../../..');
const read = (path) => readFileSync(join(root, path), 'utf8');

const profileName = 'streempilot-media-router-starter-verify';
const repository = 'StreemPilot/streempilot-monorepo';
const repositoryUrl = 'https://github.com/StreemPilot/streempilot-monorepo.git';
const workflowPath = '.github/workflows/gha-clone-media-router.yml';

const profiles = read('remote/deployments/build-server-rs/src/profiles.rs');
const planner = read('remote/deployments/gha-clone-server-rs/src/lib.rs');
const patch = read(
  'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml',
);
const config = read(
  'remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml',
);
const workflow = read('.github/workflows/gha-clone-server.yml');
const readme = read('remote/deployments/gha-clone-server-rs/README.md');
const rollout = read('docs/streempilot-ci-continuity.md');
const policyDocumentation = read(
  'docs/build-server-profile-repository-policy.md',
);

function profileRulesFromPatch() {
  const match = patch.match(
    /name:\s*BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON[\s\S]*?value:\s*>-\s*\n\s*(\[[^\n]+\])/,
  );
  assert.ok(match, 'exact profile repository policy is missing');
  return JSON.parse(match[1]);
}

function profileBody() {
  const marker =
    'const STREEMPILOT_MEDIA_ROUTER_STEPS: &[ProfileStep] = &[';
  const start = profiles.indexOf(marker);
  assert.ok(start >= 0, 'media-router profile steps are missing');
  const end = profiles.indexOf('\n];', start);
  assert.ok(end > start, 'media-router profile steps are unterminated');
  return profiles.slice(start, end + 3);
}

test('fixed profile uses separate pinned Python and Rust phases', () => {
  const body = profileBody();
  assert.match(profiles, new RegExp(`name: "${profileName}"`));
  assert.match(
    profiles,
    /artifact_paths:\s*&\[\s*"\.gha-continuity\/streempilot-media-router\.tar\.gz",\s*"\.gha-continuity\/streempilot-media-router\.tar\.gz\.sha256"/,
  );
  assert.equal((body.match(/image: PYTHON_IMAGE/g) ?? []).length, 2);
  assert.equal((body.match(/image: RUST_IMAGE/g) ?? []).length, 1);
  for (const evidence of [
    'services/wave2-rust-services.v1.json',
    'scripts/render-media-router-starter.py',
    'scripts/render-formatted-media-router-starter.py',
    'tests/test_media_router_starter.py',
    workflowPath,
    'python3 -m unittest tests.test_media_router_starter -v',
    'cargo metadata --locked --offline --no-deps',
    'cargo clippy --locked --offline --all-targets -- -D warnings',
    'cargo test --locked --offline --all-targets',
    'timeout --signal=TERM 10s',
    'STREEMPILOT_MEDIA_ROUTER_MAX_DESTINATIONS=2',
    'gzip.GzipFile',
    'mtime=0',
    'hashlib.sha256',
  ]) {
    assert.ok(body.includes(evidence), `missing fixed-profile evidence: ${evidence}`);
  }
  assert.doesNotMatch(
    body,
    /cargo publish|kubectl|curl |wget |apt-get|pip install|git clone|docker |nerdctl|\|\| true/,
  );
});

test('planner selects only the exact token and tests an adjacent lookalike', () => {
  assert.match(
    planner,
    /const STREEMPILOT_MEDIA_ROUTER_PROFILE: &str =\s*"streempilot-media-router-starter-verify"/,
  );
  assert.match(planner, /fn contains_exact_profile_marker/);
  assert.match(
    planner,
    /contains_exact_profile_marker\(\s*text,\s*STREEMPILOT_MEDIA_ROUTER_PROFILE,\s*\)/,
  );
  assert.match(planner, /maps_only_the_exact_streempilot_media_router_marker/);
  assert.match(planner, /streempilot-media-router-starter-verify-adjacent/);
  assert.match(
    planner,
    /"streempilot-media-router-starter-verify"\.to_string\(\)/,
  );
  assert.doesNotMatch(
    planner,
    /text\.contains\("streempilot-media-router-starter-verify"\)/,
  );
});

test('clone admission is exact to the monorepo and media-router workflow', () => {
  assert.ok(config.includes(repository));
  assert.match(
    config,
    /"StreemPilot\/streempilot-monorepo": \["\.github\/workflows\/gha-clone-media-router\.yml"\]/,
  );
  assert.doesNotMatch(config, /StreemPilot\/\*|"StreemPilot"\s*:/);
  assert.equal((config.match(/StreemPilot\/streempilot-monorepo/g) ?? []).length, 2);
});

test('build-server exact policy permits only the dedicated profile', () => {
  const rules = profileRulesFromPatch();
  assert.deepEqual(
    rules.find(({ repository: candidate }) => candidate === repositoryUrl),
    { repository: repositoryUrl, profiles: [profileName] },
  );
  const allowedLine = patch
    .split('\n')
    .find((line) => line.includes('value: rust-verify,'));
  assert.ok(allowedLine?.split(',').includes(profileName));
  for (const denied of [
    'rust-verify',
    'node-verify',
    'python-verify',
    'playwright',
    'flutter-verify',
  ]) {
    assert.equal(
      rules
        .find(({ repository: candidate }) => candidate === repositoryUrl)
        .profiles.includes(denied),
      false,
      `monorepo unexpectedly admits ${denied}`,
    );
  }
});

test('permanent GHA coverage runs Rust, policy, and static contracts', () => {
  assert.match(workflow, /cargo clippy --locked --all-targets -- -D warnings/);
  assert.match(workflow, /cargo test --locked --all-targets/);
  assert.match(workflow, /profiles::tests/);
  assert.match(workflow, /config::profile_policy::tests/);
  assert.match(workflow, /streempilot-media-router-continuity\.test\.mjs/);
  assert.match(workflow, /dd-build-server-gha-continuity\.patch\.yaml/);
});

test('documentation preserves the fixed-profile and no-authority-expansion model', () => {
  for (const source of [readme, rollout, policyDocumentation]) {
    assert.ok(source.includes(repository));
    assert.ok(source.includes(profileName));
  }
  assert.match(readme, /marker is a token, not a substring/);
  assert.match(rollout, /locked offline Cargo commands/);
  assert.match(policyDocumentation, /Workflow text supplies the exact marker/);
  assert.match(policyDocumentation, /never supplies executable commands/);
});

test('temporary finalizers are absent from the review surface', () => {
  for (const path of [
    '.github/workflows/finalize-den-1757-streempilot-media-router-current-dev.yml',
    'scripts/finalize-den-1757-streempilot-media-router-current-dev.py',
  ]) {
    assert.equal(existsSync(join(root, path)), false, `${path} must self-delete`);
  }
});
'''
    write(CONTRACT_TEST, content)


def update_permanent_workflow() -> None:
    text = read(PERMANENT_WORKFLOW)
    test_path = "remote/tests/general/streempilot-media-router-continuity.test.mjs"
    if test_path in text:
        raise SystemExit("permanent workflow already contains media-router test")

    anchor = (
        "      - 'remote/tests/general/build-server-profile-repository-policy.test.mjs'\n"
        "      - 'docs/gha-continuity-architecture.md'"
    )
    replacement = (
        "      - 'remote/tests/general/build-server-profile-repository-policy.test.mjs'\n"
        f"      - '{test_path}'\n"
        "      - 'docs/gha-continuity-architecture.md'"
    )
    text = replace_once(text, anchor, replacement, "add pull-request test path")
    text = replace_once(text, anchor, replacement, "add push test path")
    text = replace_once(
        text,
        "            general/gha-executor-router-activation.test.mjs \\\n"
        "            general/build-server-profile-repository-policy.test.mjs",
        "            general/gha-executor-router-activation.test.mjs \\\n"
        "            general/build-server-profile-repository-policy.test.mjs \\\n"
        "            general/streempilot-media-router-continuity.test.mjs",
        "run media-router static contract",
    )
    write(PERMANENT_WORKFLOW, text)


def validate_result() -> None:
    for path in [
        PROFILES,
        PLANNER,
        PATCH,
        CONFIG,
        README,
        STREEMPILOT_DOC,
        POLICY_DOC,
        POLICY_TEST,
        CONTRACT_TEST,
        PERMANENT_WORKFLOW,
    ]:
        if PROFILE not in read(path):
            raise SystemExit(f"{path}: missing profile marker after transformation")

    patch_text = read(PATCH)
    policy_match = re.search(
        r"name:\s*BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON[\s\S]*?"
        r"value:\s*>-\s*\n\s*(\[[^\n]+\])",
        patch_text,
    )
    if not policy_match:
        raise SystemExit("unable to parse exact profile policy")
    rules = json.loads(policy_match.group(1))
    rule = next(
        (candidate for candidate in rules if candidate["repository"] == REPOSITORY_URL),
        None,
    )
    if rule != {"repository": REPOSITORY_URL, "profiles": [PROFILE]}:
        raise SystemExit("media-router exact repository policy is not least privilege")

    config_text = read(CONFIG)
    rules_match = re.search(
        r"GHA_CLONE_WORKFLOW_RULES_JSON:\s*\|\n(?P<body>(?: {4}.*\n?)+)$",
        config_text,
    )
    if not rules_match:
        raise SystemExit("unable to parse clone workflow rules")
    config_rules = json.loads(
        "\n".join(line[4:] for line in rules_match.group("body").splitlines())
    )
    if config_rules.get(REPOSITORY) != [WORKFLOW_PATH]:
        raise SystemExit("media-router clone workflow policy is not exact")


def main() -> None:
    update_profiles()
    update_planner()
    update_patch()
    update_config()
    update_readme()
    update_streempilot_documentation()
    update_policy_documentation()
    update_policy_test()
    create_contract_test()
    update_permanent_workflow()
    validate_result()


if __name__ == "__main__":
    main()
