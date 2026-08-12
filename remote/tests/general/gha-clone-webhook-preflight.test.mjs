import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

const root = resolve(import.meta.dirname, '../../..');
const script = readFileSync(
  resolve(root, 'scripts/ops/preflight_gha_clone_webhook.sh'),
  'utf8',
);
const cloneDigest =
  'ghcr.io/oresoftware/gha-clone-server@sha256:c141b374acc4b49a9108317e78c06b9d726bcb99903c00c01a2cf200f98432e4';
const routerDigest =
  'ghcr.io/oresoftware/gha-executor-router@sha256:e87bee0e28911fbdc096d2fec0c1a65811b7d2173594d81c377dc437ac658e8f';

function requireAll(text, values) {
  for (const value of values) {
    assert.ok(text.includes(value), `preflight is missing ${value}`);
  }
}

test('preflight requires the exact reviewed images and explicit inert or active gates', () => {
  requireAll(script, [
    cloneDigest,
    routerDigest,
    '--expect-active',
    "expected_replicas=0",
    "expected_replicas=1",
    'GHA_CLONE_EXECUTION_ENABLED',
    'GHA_CLONE_WEBHOOK_EXECUTION_ENABLED',
    'GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED',
  ]);
  assert.match(script, /GHA_CLONE_EXECUTION_ENABLED\)" == "\$expected_execution"/);
  assert.match(script, /GHA_CLONE_WEBHOOK_EXECUTION_ENABLED\)" == "\$expected_execution"/);
  assert.match(script, /GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED\)" == "\$expected_execution"/);
});

test('preflight models split clone, router, and AWS build-server authority', () => {
  requireAll(script, [
    'secret_has_keys "$clone_secret" auth_secret github_webhook_secret github_token',
    'secret_has_keys "$router_secret" inbound_auth',
    'secret_has_keys "$build_secret" SERVER_AUTH_SECRET',
    'GHA_CLONE_BUILD_SERVER_AUTH "$router_secret" inbound_auth',
    'dd-agent-secrets',
    'aws-build-server-auth',
  ]);
  assert.doesNotMatch(
    script,
    /secret_has_keys "\$clone_secret"[^\n]*build_server_auth/,
  );
});

test('preflight enforces exact AWS-only router placement and no direct clone executor path', () => {
  requireAll(script, [
    'id:"aws-primary", provider:"aws", enabled:true',
    'id:"hetzner-secondary", provider:"hetzner", enabled:false',
    'clone server must address only the executor router',
    'no direct build-server path',
    'no public/Hetzner path',
    'build_policy="$(get_json networkpolicy "$build_name")"',
    'build-server NetworkPolicy must admit the continuity router on TCP 8100',
  ]);
  assert.match(
    script,
    /BUILD_SERVER_URL='http:\/\/dd-build-server\.default\.svc\.cluster\.local:8100'/,
  );
  assert.match(
    script,
    /ROUTER_URL='http:\/\/dd-gha-executor-router\.default\.svc\.cluster\.local:8126'/,
  );
});

test('preflight never mutates Kubernetes or decodes secret values', () => {
  assert.doesNotMatch(
    script,
    /kubectl\s+(?:apply|create|delete|edit|patch|replace|rollout|scale|set)\b/,
  );
  assert.doesNotMatch(script, /base64\s+(?:-d|--decode)/);
  assert.doesNotMatch(script, /\.data\[[^\]]+\]\s*\|\s*@base64d/);
  assert.match(script, /no webhook or Kubernetes write was performed/);
});

test('live probe verifies both services in selected inert or active ready mode', () => {
  requireAll(script, [
    'service/$clone_name',
    'service/$router_name',
    'clone-health.json',
    'clone-ready.json',
    'router-health.json',
    'router-ready.json',
    '.readyExecutors == []',
    'any(.[]?; .id == "aws-primary" and .provider == "aws")',
    'live clone and router health/readiness passed in $mode_name mode',
  ]);
  assert.doesNotMatch(script, /readyExecutors \| index\("aws-primary"\)/);
});
