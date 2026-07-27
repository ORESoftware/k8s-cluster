import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

function repoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, '.github/workflows/repo-checks.yml'))) return candidate;
  }
  throw new Error(`Unable to locate repository root from ${process.cwd()}`);
}

const root = repoRoot();
const workflow = readFileSync(resolve(root, '.github/workflows/repo-checks.yml'), 'utf8');
const helper = readFileSync(resolve(root, 'scripts/ci/init-submodules-with-report.sh'), 'utf8');

test('repo checks never place private-submodule tokens in Git URLs', () => {
  assert.doesNotMatch(workflow, /x-access-token:\$\{?K8S_SUBMODULE_TOKEN/);
  assert.doesNotMatch(workflow, /token_url=.*K8S_SUBMODULE_TOKEN/);
  assert.doesNotMatch(helper, /https:\/\/[^\s"']*\$K8S_SUBMODULE_TOKEN[^\s"']*@github\.com/);
  assert.match(helper, /GIT_ASKPASS/);
  assert.match(helper, /GIT_TERMINAL_PROMPT=0/);
});

test('the narrow remote-libs job uses the dedicated read-only deploy key', () => {
  const staticJob = workflow.slice(
    workflow.indexOf('  static-contracts:'),
    workflow.indexOf('  backend-contracts:'),
  );
  assert.match(staticJob, /K8S_LIBS_DEPLOY_KEY:\s*\$\{\{ secrets\.K8S_LIBS_DEPLOY_KEY \}\}/);
  assert.match(staticJob, /ssh-key:\s*\$\{\{ secrets\.K8S_LIBS_DEPLOY_KEY \}\}/);
  assert.match(staticJob, /SUBMODULE_AUTH_MODE:\s*ssh/);
  assert.match(staticJob, /init-submodules-with-report\.sh remote\/libs/);
  assert.doesNotMatch(staticJob, /REMOTE_DEV_GH_PAT/);
});

test('the cross-org backend job reports every inaccessible repository safely', () => {
  const backendJob = workflow.slice(
    workflow.indexOf('  backend-contracts:'),
    workflow.indexOf('  kustomize-render:'),
  );
  assert.match(backendJob, /K8S_SUBMODULE_TOKEN:\s*\$\{\{ secrets\.REMOTE_DEV_GH_PAT \}\}/);
  assert.match(backendJob, /SUBMODULE_AUTH_MODE:\s*https-token/);
  assert.match(backendJob, /init-submodules-with-report\.sh remote\/deployments/);
  assert.match(helper, /::error title=Submodule unavailable::/);
  assert.match(helper, /repository-missing-or-inaccessible/);
  assert.match(helper, /permission-denied/);
  assert.match(helper, /No credential values or credential-bearing URLs were written to the report/);
  assert.doesNotMatch(helper, /set -x/);
});

test('the helper verifies checkout commits match superproject gitlinks', () => {
  assert.match(helper, /git ls-files --stage/);
  assert.match(helper, /git -C "\$path" rev-parse HEAD/);
  assert.match(helper, /pinned-commit-mismatch/);
});
