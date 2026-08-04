from pathlib import Path

MSGINT_REVISION = "952623b07fd83caa3a83ee27bdea293f6bd4372f"
APP_TOKEN_ACTION = "bcd2ba49218906704ab6c1aa796996da409d3eb1"


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


def replace_count(text: str, old: str, new: str, count: int, label: str) -> str:
    actual = text.count(old)
    if actual != count:
        raise RuntimeError(f"{label}: expected {count} matches, found {actual}")
    return text.replace(old, new)


workflow_path = Path(".github/workflows/gha-clone-server.yml")
workflow = workflow_path.read_text(encoding="utf-8")
workflow = replace_once(
    workflow,
    "run: cargo test --locked --lib -- --nocapture",
    "run: cargo test --locked --bin dd-build-server -- --nocapture",
    "binary-only build-server test target",
)
workflow = replace_once(
    workflow,
    "MSGINT_REVISION: 7d905806b2000479bdacb9b206f33b26a707ba5e",
    f"MSGINT_REVISION: {MSGINT_REVISION}",
    "immutable Messaging Intel revision",
)
workflow = replace_once(
    workflow,
    "actions/create-github-app-token@fee1f7d63c2ff003460e3d139729b119787bc349 # v2.2.2",
    f"actions/create-github-app-token@{APP_TOKEN_ACTION} # v3.2.0",
    "GitHub App token action pin",
)
workflow = replace_once(
    workflow,
    "          app-id: ${{ secrets.K8S_SUBMODULE_APP_ID }}\n",
    "          client-id: ${{ secrets.MSGINT_READER_APP_CLIENT_ID }}\n",
    "dedicated Messaging Intel App client ID",
)
workflow = replace_once(
    workflow,
    "          private-key: ${{ secrets.K8S_SUBMODULE_APP_PRIVATE_KEY }}\n",
    "          private-key: ${{ secrets.MSGINT_READER_APP_PRIVATE_KEY }}\n",
    "dedicated Messaging Intel App private key",
)
workflow = replace_once(
    workflow,
    "          repositories: msgint-connectors\n",
    "          repositories: msgint-connectors\n          permission-contents: read\n",
    "least-privilege repository token permission",
)
fixture_path = "      - 'remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile/**'\n"
workflow = replace_count(
    workflow,
    "      - 'remote/deployments/build-server-rs/src/validation.rs'\n",
    "      - 'remote/deployments/build-server-rs/src/validation.rs'\n" + fixture_path,
    2,
    "fixture workflow path filters",
)
hermetic_job = """  node-profile-hermetic-smoke:
    needs: [rust, build-server-profile]
    runs-on: ubuntu-24.04
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020 # v7
        with:
          node-version: '22'
      - name: Execute the lifecycle-script-free fixed-profile contract
        working-directory: remote/deployments/build-server-rs/tests/fixtures/node-hardened-profile
        run: |
          set -euo pipefail
          npm ci --ignore-scripts
          npm run check
          npm run test:operator-config
          npm audit --audit-level=high
          npm test

"""
workflow = replace_once(
    workflow,
    "  msgint-profile-smoke:\n",
    hermetic_job + "  msgint-profile-smoke:\n",
    "credential-free profile parity job",
)
workflow = replace_once(
    workflow,
    "    needs: [rust, build-server-profile]\n    runs-on: ubuntu-24.04\n    timeout-minutes: 30\n    env:\n      MSGINT_REVISION:",
    "    needs: [rust, build-server-profile, node-profile-hermetic-smoke]\n    runs-on: ubuntu-24.04\n    timeout-minutes: 30\n    env:\n      MSGINT_REVISION:",
    "private smoke dependency",
)
workflow_path.write_text(workflow, encoding="utf-8")

contract_path = Path("remote/tests/general/gha-clone-server-config.test.ts")
contract = contract_path.read_text(encoding="utf-8")
addition = f"""

test('Messaging Intel continuity has a hermetic lane and least-privilege private smoke', () => {{
  const workflow = read(workflowPath);
  assert.match(workflow, /node-profile-hermetic-smoke/);
  assert.match(workflow, /tests\\/fixtures\\/node-hardened-profile/);
  assert.match(workflow, /npm ci --ignore-scripts/);
  assert.match(workflow, /npm run test:operator-config/);
  assert.match(workflow, /actions\\/create-github-app-token@{APP_TOKEN_ACTION}/);
  assert.match(workflow, /client-id:\\s*\\$\\{{\\{{ secrets\\.MSGINT_READER_APP_CLIENT_ID \\}}\\}}/);
  assert.match(workflow, /private-key:\\s*\\$\\{{\\{{ secrets\\.MSGINT_READER_APP_PRIVATE_KEY \\}}\\}}/);
  assert.match(workflow, /permission-contents:\\s*read/);
  assert.match(workflow, /owner:\\s*messaging-intel/);
  assert.match(workflow, /repositories:\\s*msgint-connectors/);
  assert.match(workflow, /MSGINT_REVISION:\\s*{MSGINT_REVISION}/);
  assert.match(workflow, /github\\.event_name == 'workflow_dispatch'/);
  assert.doesNotMatch(workflow, /ghp_[A-Za-z0-9]{{20,}}|github_pat_/);
}});
"""
if "least-privilege private smoke" in contract:
    raise RuntimeError("least-privilege workflow contract already exists")
contract_path.write_text(contract.rstrip() + addition, encoding="utf-8")

readme_path = Path("remote/deployments/gha-clone-server-rs/README.md")
readme = readme_path.read_text(encoding="utf-8")
note = """

### Messaging Intel private-source smoke

Pull requests always execute the credential-free `node-profile-hermetic-smoke` lane.
The private repository smoke is manual-only and mints a one-hour installation token
for exactly `messaging-intel/msgint-connectors` with `contents:read`. Configure the
`MSGINT_READER_APP_CLIENT_ID` and `MSGINT_READER_APP_PRIVATE_KEY` Actions secrets
from a GitHub App installed on that repository. The workflow never falls back to a
PAT or the ambient `GITHUB_TOKEN`.
"""
if "### Messaging Intel private-source smoke" not in readme:
    readme_path.write_text(readme.rstrip() + note, encoding="utf-8")
