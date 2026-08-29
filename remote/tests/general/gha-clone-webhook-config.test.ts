import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '../../..');
const read = (path: string) => readFileSync(join(root, path), 'utf8');

test('workflow_run fallback is failure-only, exact-path, loop-safe, and deduplicated', () => {
  const server = read('remote/deployments/gha-clone-server-rs/src/main.rs');
  assert.match(server, /action != "completed"/);
  assert.ok(server.includes('/workflow_run/conclusion'));
  assert.ok(server.includes('/workflow_run/path'));
  assert.match(server, /excluded to prevent fallback recursion/);
  assert.match(server, /x-github-delivery/);
  assert.match(server, /remember_webhook_delivery/);
  assert.match(server, /duplicate GitHub delivery/);
  assert.match(server, /GHA_CLONE_GITHUB_API_BASE_URL/);
  assert.match(server, /HTTP is allowed only for loopback tests/);
});

test('deployment declares bounded webhook policy while remaining dormant', () => {
  const deployment = read(
    'remote/argocd/dd-next-runtime/dd-gha-clone-server.deployment.yaml',
  );
  for (const name of [
    'GHA_CLONE_GITHUB_API_BASE_URL',
    'GHA_CLONE_WEBHOOK_FAILURE_CONCLUSIONS',
    'GHA_CLONE_WEBHOOK_IGNORED_WORKFLOWS',
    'GHA_CLONE_WEBHOOK_DELIVERY_TTL_SECONDS',
    'GHA_CLONE_MAX_WEBHOOK_DELIVERIES',
  ]) {
    assert.match(deployment, new RegExp(`name: ${name}`));
  }
  assert.match(
    deployment,
    /GHA_CLONE_GITHUB_API_BASE_URL\s+value: https:\/\/api\.github\.com/,
  );
  assert.match(
    deployment,
    /GHA_CLONE_WEBHOOK_EXECUTION_ENABLED\s+value: "false"/,
  );
  assert.match(
    deployment,
    /GHA_CLONE_EXECUTION_ENABLED\s+value: "false"/,
  );
  assert.match(deployment, /\breplicas:\s*0\b/);
  assert.match(deployment, /GHA continuity server/);
});

test('registration script upserts only workflow_run through stdin without echoing secrets', () => {
  const script = read(
    'remote/deployments/gha-clone-server-rs/scripts/register-github-webhook.sh',
  );
  assert.match(script, /events: \["workflow_run"\]/);
  assert.match(script, /--repo\|--org/);
  assert.match(script, /method='PATCH'/);
  assert.match(script, /method='POST'/);
  assert.match(
    script,
    /gh api --method "\$method" "\$hook_endpoint" --input -/,
  );
  assert.doesNotMatch(script, /set -x/);
  assert.ok(!script.includes('echo "$GH_TOKEN"'));
  assert.ok(!script.includes('echo "${GH_TOKEN}"'));
  assert.ok(!script.includes('echo "$GITHUB_WEBHOOK_SECRET"'));
  assert.ok(!script.includes('echo "${GITHUB_WEBHOOK_SECRET}"'));
  assert.doesNotMatch(script, /events:.*push|events:.*pull_request/);
});
