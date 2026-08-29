import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

const root = path.resolve(import.meta.dirname, '../../..');
const read = (relative) => fs.readFileSync(path.join(root, relative), 'utf8');

const activatorPath = path.join(root, 'scripts/ops/activate_gha_test_org_fallback.py');
const bootstrapPath = path.join(root, 'scripts/ops/bootstrap_gha_test_org_fallback.py');
const activator = read('scripts/ops/activate_gha_test_org_fallback.py');
const bootstrap = read('scripts/ops/bootstrap_gha_test_org_fallback.py');
const workflow = read('.github/workflows/ops-activate-gha-test-fallback.yml');
const profileRunner = read(
  'remote/argocd/dd-next-runtime/dd-ci-profile-runner.deployment.yaml',
);

test('final privileged runner admits only the two exact test repositories as rust-verify', () => {
  assert.match(
    profileRunner,
    /"gha-indie-worker-test\/gha-clone-server\.rs":"rust-verify"/,
  );
  assert.match(
    profileRunner,
    /"gha-indie-worker-test\/gha-indie-worker\.rs":"rust-verify"/,
  );
  assert.doesNotMatch(profileRunner, /gha-indie-worker-test\/\*/);
  assert.doesNotMatch(profileRunner, /"gha-indie-worker-test":"rust-verify"/);
});

test('activator pins exact reviewed heads and validates every live enforcement layer', () => {
  for (const value of [
    '129723d26294933b7b4ccff2d30323acd2235679',
    '7fb5aed82cea31771e26d3bd908456017a286533',
    '.github/workflows/gha-indie-worker-custom.yml',
    '.github/workflows/gha-clone-server-meta.yml',
    'BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON',
    'CI_PROFILE_RUNNER_RULES_JSON',
    'GHA_CLONE_WORKFLOW_RULES_JSON',
    'GHA_CLONE_WEBHOOK_FAILURE_CONCLUSIONS',
    'github_webhook_secret',
    'auth_secret',
    'github_token',
    'GH_PAT',
  ]) {
    assert.ok(activator.includes(value), `activator missing ${value}`);
  }
  assert.match(activator, /identity\.get\("login"\) != "ORESoftware"/);
  assert.match(activator, /membership\.get\("role"\) != "admin"/);
  assert.match(activator, /"events": \["workflow_run"\]/);
  assert.match(activator, /different active workflow_run hook/);
  assert.match(activator, /ambiguous duplicate callback hooks/);
  assert.match(activator, /pingStatus/);
  assert.match(activator, /delivery\.get\("status_code"\) != 202/);
  assert.match(activator, /invalid HMAC/);
  assert.match(activator, /delivery replay was not suppressed/);
  assert.match(activator, /deactivate_hooks/);
  assert.match(activator, /"githubWorkflowRunDeliveryProven": False/);
  assert.match(activator, /"githubPingDeliveryProven": True/);
  assert.match(activator, /"billingExhaustionProven": False/);
  assert.doesNotMatch(
    activator,
    /kubectl[^\n]*\b(?:apply|create|delete|patch|replace|scale|set)\b/,
  );
  assert.doesNotMatch(activator, /\brm\b/);
  assert.doesNotMatch(activator, /print\([^\n]*(?:admin_token|runtime_token|webhook_secret|clone_auth)/);
});

test('SSM bootstrap fetches and hashes the activator at the exact workflow SHA', () => {
  assert.match(bootstrap, /SHA_RE = re\.compile/);
  assert.match(bootstrap, /DIGEST_RE = re\.compile/);
  assert.match(bootstrap, /\["data"\]\["GH_PAT"\]/);
  assert.match(bootstrap, /urllib\.parse\.quote\(trusted_sha\)/);
  assert.match(bootstrap, /hashlib\.sha256\(source\)\.hexdigest\(\) != expected_digest/);
  assert.match(bootstrap, /compile\(source_text, SCRIPT_PATH, "exec"\)/);
  assert.doesNotMatch(bootstrap, /print\([^\n]*token/);
  assert.doesNotMatch(bootstrap, /\brm\b/);
});

test('activation workflow uses one marked dev push and short-lived OIDC to execute through SSM', () => {
  for (const value of [
    'branches: [dev]',
    '[activate-gha-test-fallback]',
    'id-token: write',
    'persist-credentials: false',
    'aws-actions/configure-aws-credentials@e6de054238d6b7531b4efff3b6587d9aade6a06c',
    'aws ssm send-command',
    'scripts/ops/bootstrap_gha_test_org_fallback.py',
    'scripts/ops/activate_gha_test_org_fallback.py',
    'KUBECONFIG=/etc/kubernetes/admin.conf',
    'sha256sum',
  ]) {
    assert.ok(workflow.includes(value), `activation workflow missing ${value}`);
  }
  assert.doesNotMatch(workflow, /secrets\.(?:GH_PAT|GITHUB_TOKEN|GITHUB_WEBHOOK_SECRET)/);
  assert.doesNotMatch(
    workflow,
    /kubectl\s+(?:apply|create|delete|patch|replace|scale|set)\b/,
  );
  assert.doesNotMatch(workflow, /\brm\b/);
});

test('activation programs reject malformed entrypoint arguments before protected access', () => {
  const bootstrapResult = spawnSync('python3', [bootstrapPath], {
    cwd: root,
    encoding: 'utf8',
  });
  assert.notEqual(bootstrapResult.status, 0);
  assert.match(bootstrapResult.stderr, /expected trusted SHA/);

  const activatorResult = spawnSync(
    'python3',
    [activatorPath, '--callback-url', 'http://127.0.0.1/gha-webhooks/github'],
    { cwd: root, encoding: 'utf8' },
  );
  assert.notEqual(activatorResult.status, 0);
  assert.match(activatorResult.stderr, /exact credential-free HTTPS/);
  assert.doesNotMatch(activatorResult.stderr, /Traceback/);
});
