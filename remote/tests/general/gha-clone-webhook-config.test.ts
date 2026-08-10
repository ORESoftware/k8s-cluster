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
  assert.match(server, /only workflow_run events may trigger the failure fallback/);
});

test('deployment activates the exact signed workflow_run pilot', () => {
  const deployment = read(
    'remote/argocd/dd-next-runtime/dd-gha-clone-server.deployment.yaml',
  );
  const config = read(
    'remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml',
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
    /GHA_CLONE_WEBHOOK_EXECUTION_ENABLED\s+value: "true"/,
  );
  assert.match(
    deployment,
    /GHA_CLONE_EXECUTION_ENABLED\s+value: "true"/,
  );
  assert.match(deployment, /\breplicas:\s*1\b/);
  assert.match(deployment, /signed-workflow-run-pilot/);
  assert.match(deployment, /GHA continuity server/);
  assert.match(
    deployment,
    /ghcr\.io\/oresoftware\/gha-clone-server@sha256:44684171d909f96fe216d529bfc14f6f32a11e87c0f339d1877ac20606223c97/,
  );
  assert.match(config, /GHA_CLONE_ALLOWED_REPOSITORIES: ORESoftware\/k8s-cluster/);
  assert.match(config, /\.github\/workflows\/gha-continuity-parity\.yml/);
  assert.match(config, /\.github\/workflows\/remote-k8s-browser-suite\.yml/);
});

test('dedicated ingress preserves signed raw-body delivery to the clone server', () => {
  const route = read(
    'remote/argocd/dd-next-runtime/dd-gha-clone-server.networkpolicy.yaml',
  );
  for (const value of [
    'name: dd-gha-clone-server-webhook',
    'ingressClassName: nginx',
    'hello.95-217-171-250.sslip.io',
    'path: /gha-webhooks/github',
    'pathType: Exact',
    'nginx.ingress.kubernetes.io/rewrite-target: /webhooks/github',
    'nginx.ingress.kubernetes.io/proxy-buffering: "off"',
    'nginx.ingress.kubernetes.io/proxy-request-buffering: "off"',
    'name: dd-gha-clone-server',
    'number: 8125',
    'kubernetes.io/metadata.name: ingress-nginx',
  ]) {
    assert.ok(route.includes(value), `webhook route missing ${value}`);
  }
  assert.doesNotMatch(route, /path:\s*\/webhooks\/\s*$/m);
});

test('registration script upserts only workflow_run through stdin without echoing secrets', () => {
  const script = read(
    'remote/deployments/gha-clone-server-rs/scripts/register-github-webhook.sh',
  );
  assert.match(script, /events: \["workflow_run"\]/);
  assert.match(script, /\/gha-webhooks\/github/);
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

test('live verifier is read-only and never decodes secret values', () => {
  const script = read('scripts/ops/verify_gha_workflow_run_fallback.sh');
  for (const value of [
    'rollout status',
    'dd-gha-clone-server-secrets',
    'dd-gha-executor-router-secrets',
    'GHA_CLONE_WEBHOOK_EXECUTION_ENABLED',
    'GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED',
    '/healthz',
    '/readyz',
    'external route expected application HMAC rejection 401',
  ]) {
    assert.ok(script.includes(value), `verifier missing ${value}`);
  }
  assert.doesNotMatch(script, /base64\s+(?:--decode|-d)/);
  assert.doesNotMatch(script, /kubectl\s+(?:apply|create|delete|patch|replace|scale|set)/);
  assert.doesNotMatch(script, /set -x/);
});
