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

type AppAllowlist = {
  schema_version: number;
  required_permissions: Record<string, string>;
  repositories_by_owner: Record<string, string[]>;
};

const root = repoRoot();
const workflow = readFileSync(resolve(root, '.github/workflows/repo-checks.yml'), 'utf8');
const gitmodules = readFileSync(resolve(root, '.gitmodules'), 'utf8');
const helper = readFileSync(resolve(root, 'scripts/ci/init-submodules-with-report.sh'), 'utf8');
const appInitializer = readFileSync(
  resolve(root, 'scripts/ci/init-submodules-with-github-app.sh'),
  'utf8',
);
const tokenMinter = readFileSync(
  resolve(root, 'scripts/ci/mint-github-app-installation-token.sh'),
  'utf8',
);
const appRunbook = readFileSync(resolve(root, 'docs/k8s-submodule-github-app.md'), 'utf8');
const allowlist = JSON.parse(
  readFileSync(resolve(root, 'config/ci/k8s-submodule-github-app-allowlist.json'), 'utf8'),
) as AppAllowlist;
const privateDeploymentTests = [
  'general/cluster-hardening.test.ts',
  'general/fabrication-cad-source-intake.test.ts',
  'general/gateway-service-directory.test.ts',
  'general/gleam-lambda-runner-config.test.ts',
  'general/observability-config.test.ts',
] as const;

function normalizeRepository(url: string): string {
  return url
    .trim()
    .replace(/^git@github\.com:/, '')
    .replace(/^ssh:\/\/git@github\.com\//, '')
    .replace(/^https:\/\/github\.com\//, '')
    .replace(/\.git$/, '');
}

function deploymentRepositories(): string[] {
  const repositories: string[] = [];
  const blocks = gitmodules.split(/^\[submodule /m).slice(1);
  for (const block of blocks) {
    const path = block.match(/^\s*path\s*=\s*(\S+)\s*$/m)?.[1];
    const url = block.match(/^\s*url\s*=\s*(\S+)\s*$/m)?.[1];
    if (path?.startsWith('remote/deployments/') && url) {
      repositories.push(normalizeRepository(url));
    }
  }
  return repositories.sort();
}

function allowlistedRepositories(): string[] {
  return Object.entries(allowlist.repositories_by_owner)
    .flatMap(([owner, repositories]) => repositories.map((repository) => `${owner}/${repository}`))
    .sort();
}

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

test('private deployment source readers run only after the App-backed checkout', () => {
  const staticJob = workflow.slice(
    workflow.indexOf('  static-contracts:'),
    workflow.indexOf('  backend-contracts:'),
  );
  const backendJob = workflow.slice(
    workflow.indexOf('  backend-contracts:'),
    workflow.indexOf('  kustomize-render:'),
  );

  for (const testFile of privateDeploymentTests) {
    assert.ok(!staticJob.includes(testFile), `${testFile} must not run in the remote/libs-only job`);
    assert.ok(backendJob.includes(testFile), `${testFile} must run in the App-backed job`);
  }

  const appCheckout = backendJob.indexOf('init-submodules-with-github-app.sh remote/deployments');
  const privateTests = backendJob.indexOf('Verify contracts that read private deployment sources');
  assert.ok(appCheckout >= 0, 'backend job must initialize private deployment gitlinks');
  assert.ok(privateTests > appCheckout, 'private source tests must run only after App checkout');
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
  assert.match(
    backendJob,
    /actions\/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7\.0\.1/,
  );
  assert.match(backendJob, /name:\s*backend-submodule-access-report/);
  assert.match(backendJob, /steps\.backend-submodules\.outcome == 'failure'/);
  assert.doesNotMatch(backendJob, /K8S_SUBMODULE_TOKEN:\s*\$\{\{ secrets\./);
});

test('the App repository allowlist exactly matches every deployment gitlink', () => {
  assert.equal(allowlist.schema_version, 1);
  assert.deepEqual(allowlist.required_permissions, {
    contents: 'read',
    metadata: 'read',
  });

  for (const [owner, repositories] of Object.entries(allowlist.repositories_by_owner)) {
    assert.ok(owner.length > 0);
    assert.ok(repositories.length > 0, `${owner} must include at least one repository`);
    assert.deepEqual(
      repositories,
      [...new Set(repositories)].sort(),
      `${owner} repositories must be sorted and unique`,
    );
    for (const repository of repositories) {
      assert.doesNotMatch(repository, /\//, `${owner}/${repository} must be an owner-local name`);
    }
  }

  const declared = deploymentRepositories();
  const approved = allowlistedRepositories();
  assert.equal(declared.length, 30, 'expected the complete pinned deployment fleet');
  assert.deepEqual(
    approved,
    declared,
    'Update the reviewed GitHub App allowlist in the same PR as any deployment gitlink change.',
  );
  assert.match(appInitializer, /K8S_SUBMODULE_ALLOWLIST/);
  assert.match(appInitializer, /Repository not allowlisted/);
  assert.match(appInitializer, /required_permissions/);
});

test('installation tokens are owner-scoped, repository-restricted, and revoked', () => {
  assert.match(appInitializer, /cut -f1 "\$records_file" \| LC_ALL=C sort -u/);
  assert.match(appInitializer, /for owner in "\$\{owners\[@\]\}"/);
  assert.match(appInitializer, /bash "\$mint_script" "\$owner" "\$token_file" "\$\{repositories\[@\]\}"/);
  assert.match(appInitializer, /bash "\$init_script" "\$\{repository_paths\[@\]\}"/);
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

test('the GitHub App recovery runbook is current and fail closed', () => {
  for (const required of [
    'Linear: DEN-255, DEN-370, DEN-1537',
    'K8S_SUBMODULE_APP_ID',
    'K8S_SUBMODULE_APP_PRIVATE_KEY',
    'backend pins + private deployment contracts',
    'backend-submodule-access-report',
    'trusted runs',
    'untrusted fork',
    'Re-run only the failed job first',
    'complete `repo checks`',
    'at least every 90 days',
    'Do not fall back to an exposed or broadly scoped PAT',
    'A personal access token supplied in chat',
    'current authoritative commit',
  ]) {
    assert.ok(appRunbook.includes(required), `GitHub App runbook missing ${required}`);
  }

  assert.match(appRunbook, /\| Metadata \| Read \|/);
  assert.match(appRunbook, /\| Contents \| Read \|/);
  assert.match(
    appRunbook,
    /config\/ci\/k8s-submodule-github-app-allowlist\.json/,
  );
  assert.match(appRunbook, /mode-`0600` temporary file/);
  assert.match(appRunbook, /Never put either value in Linear, chat/);
  assert.match(appRunbook, /Leave the required check red\./);
  assert.doesNotMatch(appRunbook, /\bPR #\d+\b/);
  assert.doesNotMatch(
    appRunbook,
    /ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY/,
  );
});
