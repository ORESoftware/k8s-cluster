import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

const deploymentPath = resolve(
  process.cwd(),
  'remote/argocd/dd-next-runtime/dd-sound-recorder-rs.deployment.yaml',
);

test('Sonus backend prefers the read-only deploy key with pinned GitHub host identity', async () => {
  const deployment = await readFile(deploymentPath, 'utf8');

  assert.match(
    deployment,
    /name:\s*GH_DEPLOY_KEY[\s\S]*name:\s*dd-agent-secrets[\s\S]*key:\s*GH_DEPLOY_KEY/,
  );
  assert.match(
    deployment,
    /github\.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl/,
  );
  assert.match(deployment, /StrictHostKeyChecking=yes/);
  assert.match(deployment, /IdentitiesOnly=yes/);
  assert.match(deployment, /UserKnownHostsFile=\$\{known_hosts_path\}/);
  assert.match(
    deployment,
    /git_auth_args=\(-c "url\.git@github\.com:\.insteadOf=https:\/\/github\.com\/"\)/,
  );

  const deployKeyBranch = deployment.indexOf('if [ -n "${GH_DEPLOY_KEY:-}" ]');
  const patFallback = deployment.indexOf('elif [ -n "${GH_PAT:-}" ]');
  assert.ok(deployKeyBranch >= 0, 'deploy-key branch must exist');
  assert.ok(patFallback > deployKeyBranch, 'GH_PAT must remain fallback-only');

  assert.doesNotMatch(deployment, /https:\/\/[^/\s]+@github\.com/);
  assert.doesNotMatch(deployment, /StrictHostKeyChecking=accept-new/);
});
