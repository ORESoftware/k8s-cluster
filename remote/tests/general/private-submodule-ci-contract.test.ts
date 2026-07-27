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
const appInitializer = readFileSync(
  resolve(root, 'scripts/ci/init-submodules-with-github-app.sh'),
  'utf8',
);
const tokenMinter = readFileSync(
  resolve(root, 'scripts/ci/mint-github-app-installation-token.sh'),
  'utf8',
);

test('repo checks never place private-submodule credentials in Git URLs', () => {
  assert.doesNotMatch(workflow, /REMOTE_DEV_GH_PAT/);
  assert.doesNotMatch(workflow, /x-access-token:\$\{?/);
  assert.doesNotMatch(workflow, /token_url=/);
  assert.doesNotMatch(helper, /https:\/\/[^\s"']*\$K8S_SUBMODULE_TOKEN[^\s"']*@github\.com/);
  assert.doesNotMatch(appInitializer, /https:\/\/[^\s"']*installation_token[^\s"']*@github\.com/);
  assert.match(helper, /GIT_ASKPASS/);
  assert.match(helper, /GIT_TERMINAL_PROMPT=0/);
  assert.doesNotMatch(helper, /set -x/);
  assert.doesNotMatch(appInitializer, /set -x/);
  assert.doesNotMatch(tokenMinter, /set -x/);
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
  assert.doesNotMatch(staticJob, /K8S_SUBMODULE_APP_PRIVATE_KEY/);
});

test('the cross-org backend job requires a GitHub App and produces a sanitized report', () => {
  const backendJob = workflow.slice(
    workflow.indexOf('  backend-contracts:'),
    workflow.indexOf('  kustomize-render:'),
  );
  assert.match(backendJob, /K8S_SUBMODULE_APP_ID:\s*\$\{\{ secrets\.K8S_SUBMODULE_APP_ID \}\}/);
  assert.match(
    backendJob,
    /K8S_SUBMODULE_APP_PRIVATE_KEY:\s*\$\{\{ secrets\.K8S_SUBMODULE_APP_PRIVATE_KEY \}\}/,
  );
  assert.match(backendJob, /GITHUB_API_VERSION:\s*'2026-03-10'/);
  assert.match(backendJob, /SUBMODULE_REPORT_PATH:\s*\$\{\{ runner\.temp \}\}\/backend-submodule-access\.tsv/);
  assert.match(backendJob, /init-submodules-with-github-app\.sh remote\/deployments/);
  assert.match(backendJob, /continue-on-error:\s*true/);
  assert.match(backendJob, /actions\/upload-artifact@v6/);
  assert.match(backendJob, /name:\s*backend-submodule-access-report/);
  assert.match(backendJob, /steps\.backend-submodules\.outcome == 'failure'/);
  assert.doesNotMatch(backendJob, /K8S_SUBMODULE_TOKEN:\s*\$\{\{ secrets\./);
});

test('installation tokens are owner-scoped, repository-restricted, and revoked', () => {
  assert.match(appInitializer, /cut -f1 "\$records_file" \| LC_ALL=C sort -u/);
  assert.match(appInitializer, /for owner in "\$\{owners\[@\]\}"/);
  assert.match(appInitializer, /"\$mint_script" "\$owner" "\$token_file" "\$\{repositories\[@\]\}"/);
  assert.match(appInitializer, /SUBMODULE_REPORT_MODE=append/);
  assert.match(appInitializer, /"\$\{api_url%\/\}\/installation\/token"/);
  assert.match(appInitializer, /unset installation_token/);
  assert.match(tokenMinter, /\/repos\/\$\{owner\}\/\$\{first_repository\}\/installation/);
  assert.match(tokenMinter, /\/app\/installations\/\$\{installation_id\}\/access_tokens/);
  assert.match(tokenMinter, /"repositories": repositories/);
  assert.match(tokenMinter, /"permissions": \{"contents": "read"\}/);
  assert.match(tokenMinter, /exp="\$\(\(now \+ 540\)\)"/);
  assert.match(tokenMinter, /chmod 600 "\$token_output"/);
  assert.doesNotMatch(tokenMinter, /length\([^)]*token|\$\{#installation_token\}/);
});

test('the sanitized report contains only repository metadata and commit state', () => {
  assert.match(helper, /status\\trepository\\tpath\\tcategory\\tcommit/);
  assert.match(helper, /SUBMODULE_REPORT_MODE/);
  assert.match(helper, /record_result failure/);
  assert.match(helper, /record_result success/);
  assert.match(helper, /invalid username or token/);
  assert.doesNotMatch(helper, /record_result[^\n]*K8S_SUBMODULE_TOKEN/);
  assert.match(appInitializer, /installation-token-unavailable/);
  assert.match(
    appInitializer,
    /No App JWT, installation token, private key, or credential-bearing URL was written to the report/,
  );
});

test('the helper verifies checkout commits match superproject gitlinks', () => {
  assert.match(helper, /git ls-files --stage/);
  assert.match(helper, /git -C "\$path" rev-parse HEAD/);
  assert.match(helper, /pinned-commit-mismatch/);
});
