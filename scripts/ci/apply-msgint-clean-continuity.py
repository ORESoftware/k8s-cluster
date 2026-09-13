from __future__ import annotations

import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SOURCE_SHA = "826342499235e6938c019aca33ddf95119982b4c"

PRODUCT_PATHS = [
    "docs/gha-profile-repository-admission.md",
    "remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml",
    "remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml",
    "remote/deployments/build-server-rs/readme.md",
    "remote/deployments/build-server-rs/src/profiles.rs",
    "remote/deployments/build-server-rs/src/validation.rs",
    "remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile/package-lock.json",
    "remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile/package.json",
    "remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile/src/operator-config.mjs",
    "remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile/test/operator-config.test.mjs",
    "remote/deployments/gha-clone-server-rs/src/lib.rs",
    "remote/deployments/gha-clone-server-rs/tests/fixtures/msgint-operator-config.yml",
    "remote/deployments/gha-clone-server-rs/tests/msgint_fixture_contract.rs",
    "remote/deployments/gha-clone-server-rs/tests/msgint_operator_config.rs",
    "remote/deployments/gha-clone-server-rs/tests/planner_adversarial.rs",
]


def run(*args: str) -> str:
    completed = subprocess.run(
        args,
        cwd=ROOT,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return completed.stdout


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, content: str) -> None:
    target = ROOT / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


def git_show(path: str) -> str:
    return run("git", "show", f"{SOURCE_SHA}:{path}")


# Copy only validated product files. Do not import any staging/finalizer workflow.
run("git", "checkout", SOURCE_SHA, "--", *PRODUCT_PATHS)

# Semantically merge the Messaging Intel documentation into current dev's newer
# continuity architecture instead of replacing the file wholesale.
readme_path = "remote/deployments/gha-clone-server-rs/README.md"
readme = read(readme_path)
source_readme = git_show(readme_path)
section_start = source_readme.index("## Messaging Intel mirror and adversarial proof\n")
section_end = source_readme.index("## Fail-closed exclusions\n", section_start)
msgint_section = source_readme[section_start:section_end].rstrip() + "\n\n"
if "## Messaging Intel mirror and adversarial proof" not in readme:
    marker = "## Fail-closed exclusions\n"
    readme = replace_once(readme, marker, msgint_section + marker, "Messaging Intel section")

profile_anchor = "| npm/pnpm/yarn/Node tests | `node-verify` |\n"
profile_rows = (
    "| npm install-script suppression + operator checks + high-severity audit | `node-hardened-verify` |\n"
    "| npm install-script suppression + complete repository tests | `node-hardened-test` |\n"
)
if "`node-hardened-verify`" not in readme:
    readme = replace_once(readme, profile_anchor, profile_anchor + profile_rows, "hardened profile rows")

readme = readme.replace(
    "- arbitrary marketplace actions;",
    "- arbitrary marketplace actions or setup actions referenced by mutable tags/branches;",
)
readme = readme.replace(
    "- environments, deployments, reusable workflows, and caller-selected commands.",
    "- environments, deployments, reusable workflows, caller-selected environment variables, and commands outside an exact reviewed hardened sequence.",
)
write(readme_path, readme)

# Extend the current dev workflow in place. Keep its parity/webhook work and add
# only the hermetic Messaging Intel contracts; private repository access remains
# a separate manual gate.
workflow_path = ".github/workflows/gha-clone-server.yml"
workflow = read(workflow_path)
for event in ("pull_request", "push"):
    del event  # the same path list fragment occurs once in each trigger.
path_anchor = "      - 'remote/tests/general/gha-clone-webhook-config.test.ts'\n"
if workflow.count(path_anchor) != 2:
    raise RuntimeError("workflow test path anchor must occur once per push/PR trigger")
workflow = workflow.replace(
    path_anchor,
    path_anchor
    + "      - 'remote/tests/general/gha-clone-msgint-config.test.ts'\n"
    + "      - 'docs/gha-profile-repository-admission.md'\n"
    + "      - 'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml'\n",
)

workflow = replace_once(
    workflow,
    """      - name: Check the modified idempotency source formatting
        working-directory: remote/deployments/build-server-rs
        run: rustfmt --edition 2021 --check src/jobs.rs
""",
    """      - name: Check modified build-server source formatting
        working-directory: remote/deployments/build-server-rs
        run: rustfmt --edition 2021 --check src/jobs.rs src/profiles.rs src/validation.rs
""",
    "build-server formatting step",
)
workflow = replace_once(
    workflow,
    """      - name: Test the fixed-profile registry and meta fallback
        working-directory: remote/deployments/build-server-rs
        run: cargo test --locked profiles::tests -- --nocapture
      - name: Test build request idempotency and retry semantics
        working-directory: remote/deployments/build-server-rs
        run: cargo test --locked idempotency_tests -- --nocapture
""",
    """      - name: Test idempotency, fixed profiles, and exact repository admission
        working-directory: remote/deployments/build-server-rs
        run: cargo test --locked --bin dd-build-server -- --nocapture
      - name: Execute credential-free hardened Node fixtures
        run: |
          set -euo pipefail
          python3 <<'PY'
          import os
          import re
          from pathlib import Path

          source = Path('remote/deployments/build-server-rs/src/profiles.rs').read_text(encoding='utf-8')
          image = re.search(r'const NODE_IMAGE: &str = "([^"]+)";', source)
          if image is None or '@sha256:' not in image.group(1):
              raise SystemExit('NODE_IMAGE is not digest-pinned')
          output = Path(os.environ['RUNNER_TEMP'])
          (output / 'node-profile-image').write_text(image.group(1), encoding='utf-8')
          for profile, constant in (
              ('node-hardened-verify', 'NODE_HARDENED_VERIFY_STEPS'),
              ('node-hardened-test', 'NODE_HARDENED_TEST_STEPS'),
          ):
              match = re.search(rf'const {constant}:.*?script: r#"(.*?)"#,\\n\\}}\\];', source, flags=re.DOTALL)
              if match is None:
                  raise SystemExit(f'{constant} script was not found')
              script = output / f'{profile}.sh'
              script.write_text(match.group(1) + '\\n', encoding='utf-8')
              script.chmod(0o500)
          PY
          profile_image="$(cat "$RUNNER_TEMP/node-profile-image")"
          docker pull "$profile_image"
          fixture="$GITHUB_WORKSPACE/remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile"
          for profile in node-hardened-verify node-hardened-test; do
            worktree="$(mktemp -d "$RUNNER_TEMP/${profile}.XXXXXX")"
            cp -R "$fixture/." "$worktree/"
            docker run --rm --pull=never \
              --cap-drop=ALL \
              --security-opt=no-new-privileges \
              --pids-limit=256 \
              --memory=2g \
              --cpus=2 \
              --read-only \
              --tmpfs /tmp:rw,nosuid,nodev,noexec,size=512m \
              --network=bridge \
              -e CI=true \
              -e HOME=/tmp/home \
              -v "$worktree:/workspace:rw" \
              -v "$RUNNER_TEMP/${profile}.sh:/profile.sh:ro" \
              -w /workspace \
              "$profile_image" \
              bash /profile.sh
          done
""",
    "build-server validation steps",
)
workflow = replace_once(
    workflow,
    """      - name: Validate bounded meta workflow syntax
        uses: docker://rhysd/actionlint@sha256:b1934ee5f1c509618f2508e6eb47ee0d3520686341fec936f3b79331f9315667
        with:
          args: .github/workflows/gha-clone-server-meta.yml
""",
    """      - name: Validate bounded meta workflow syntax
        uses: docker://rhysd/actionlint@sha256:b1934ee5f1c509618f2508e6eb47ee0d3520686341fec936f3b79331f9315667
        with:
          args: .github/workflows/gha-clone-server-meta.yml
      - name: Validate Messaging Intel bounded fixture syntax
        uses: docker://rhysd/actionlint@sha256:b1934ee5f1c509618f2508e6eb47ee0d3520686341fec936f3b79331f9315667
        with:
          args: remote/deployments/gha-clone-server-rs/tests/fixtures/msgint-operator-config.yml
""",
    "Messaging Intel actionlint step",
)
workflow = replace_once(
    workflow,
    """          pnpm exec tsx --test \\
            general/gha-clone-server-config.test.ts \\
            general/gha-clone-webhook-config.test.ts
""",
    """          pnpm exec tsx --test \\
            general/gha-clone-server-config.test.ts \\
            general/gha-clone-webhook-config.test.ts \\
            general/gha-clone-msgint-config.test.ts
""",
    "Messaging Intel contract test command",
)
write(workflow_path, workflow)

# Supplement current dev's existing deployment/webhook tests instead of replacing
# them with an older branch copy.
write(
    "remote/tests/general/gha-clone-msgint-config.test.ts",
    r'''import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '../../..');
const read = (path: string) => readFileSync(join(root, path), 'utf8');

const fixture = read(
  'remote/deployments/gha-clone-server-rs/tests/fixtures/msgint-operator-config.yml',
);
const planner = read('remote/deployments/gha-clone-server-rs/src/lib.rs');
const liveTest = read(
  'remote/deployments/gha-clone-server-rs/tests/msgint_operator_config.rs',
);
const fixtureContract = read(
  'remote/deployments/gha-clone-server-rs/tests/msgint_fixture_contract.rs',
);
const profiles = read('remote/deployments/build-server-rs/src/profiles.rs');
const validation = read('remote/deployments/build-server-rs/src/validation.rs');
const buildPatch = read(
  'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml',
);
const cloneConfig = read(
  'remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml',
);
const workflow = read('.github/workflows/gha-clone-server.yml');
const readme = read('remote/deployments/gha-clone-server-rs/README.md');

test('Messaging Intel fixture maps only exact lifecycle-script-free sequences', () => {
  assert.equal((fixture.match(/actions\/checkout@[a-f0-9]{40}/g) ?? []).length, 2);
  assert.equal((fixture.match(/persist-credentials:\s*false/g) ?? []).length, 2);
  assert.equal((fixture.match(/npm ci --ignore-scripts/g) ?? []).length, 2);
  assert.equal((fixture.match(/npm run check/g) ?? []).length, 1);
  assert.equal((fixture.match(/npm run test:operator-config/g) ?? []).length, 1);
  assert.equal((fixture.match(/npm audit --audit-level=high/g) ?? []).length, 1);
  assert.equal((fixture.match(/\bnpm test\b/g) ?? []).length, 1);
  assert.doesNotMatch(fixture, /\$\{\{|secrets\.|github\.token|npm publish/);
});

test('planner and live server fail closed before build dispatch', () => {
  for (const evidence of [
    'node-hardened-verify',
    'node-hardened-test',
    'exact reviewed command sequence',
    'exact 40-hex commit SHA',
    'fixed profiles do not forward caller-selected variables',
  ]) {
    assert.ok(planner.includes(evidence), `planner missing ${evidence}`);
  }
  for (const evidence of [
    'UNPROCESSABLE_ENTITY',
    'npm publish',
    'reordered hardened commands',
    'actions/checkout@main',
    'PROD_TOKEN',
    'NODE_ENV: test',
    'dispatched a build despite rejection',
  ]) {
    assert.ok(liveTest.includes(evidence), `live test missing ${evidence}`);
  }
  assert.match(fixtureContract, /fixture anchor drifted/);
  assert.match(fixtureContract, /mutation became a no-op/);
});

test('build server admits only the exact repository and fixed profiles', () => {
  assert.match(profiles, /name: "node-hardened-verify"/);
  assert.match(profiles, /name: "node-hardened-test"/);
  assert.match(profiles, /npm ci --ignore-scripts/);
  assert.match(profiles, /npm audit --audit-level=high/);
  assert.match(validation, /Exact repository admission/);
  assert.match(validation, /messaging-intel\/msgint-connectors/);
  assert.match(validation, /msgint-connectors\.git-evil/);
  assert.match(validation, /msgint-connectors-extra\.git/);
  assert.match(buildPatch, /node-hardened-verify/);
  assert.match(buildPatch, /node-hardened-test/);
});

test('cluster and workflow contracts stay credential-free and hermetic', () => {
  assert.match(cloneConfig, /messaging-intel\/msgint-connectors/);
  assert.match(cloneConfig, /gha-clone-operator-config\.yml/);
  assert.match(workflow, /Validate Messaging Intel bounded fixture syntax/);
  assert.match(workflow, /Execute credential-free hardened Node fixtures/);
  assert.match(workflow, /gha-clone-msgint-config\.test\.ts/);
  assert.doesNotMatch(
    workflow,
    /ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}/,
  );
  assert.match(readme, /## Messaging Intel mirror and adversarial proof/);
  assert.match(readme, /This hermetic proof needs neither the private Messaging Intel repository nor a Kubernetes context/);
});
''',
)

print("clean Messaging Intel continuity delta applied")
