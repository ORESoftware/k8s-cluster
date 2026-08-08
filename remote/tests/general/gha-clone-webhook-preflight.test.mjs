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
  'ghcr.io/oresoftware/gha-clone-server@sha256:44684171d909f96fe216d529bfc14f6f32a11e87c0f339d1877ac20606223c97';
const routerDigest =
  'ghcr.io/oresoftware/gha-executor-router@sha256:59a31a496e5c528f89acb7643b8ced1ea14bc6c15b1d83b22a37f4ba529708e6';

function requireAll(values) {
  for (const value of values) {
    assert.ok(script.includes(value), `preflight is missing ${value}`);
  }
}

test('preflight requires the exact reviewed images and inert replica gates', () => {
  requireAll([
    cloneDigest,
    routerDigest,
    'static preflight requires clone/router replicas=0',
    '--probe-live requires clone/router replicas=1',
    'GHA_CLONE_EXECUTION_ENABLED',
    'GHA_CLONE_WEBHOOK_EXECUTION_ENABLED',
    'GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED',
  ]);
  assert.match(script, /GHA_CLONE_EXECUTION_ENABLED\)" == false/);
  assert.match(script, /GHA_CLONE_WEBHOOK_EXECUTION_ENABLED\)" == false/);
  assert.match(script, /GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED\)" == false/);
});

test('preflight models split clone, router, and AWS build-server authority', () => {
  requireAll([
    'secret_has_keys "$clone_secret" auth_secret github_webhook_secret github_app_installation_token',
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
  requireAll([
    'id:"aws-primary", provider:"aws", enabled:true',
    'id:"hetzner-secondary", provider:"hetzner", enabled:false',
    'clone server must address only the executor router',
    'no direct build-server path',
    'no public/Hetzner path',
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

test('live probe verifies both services in execution-disabled ready mode', () => {
  requireAll([
    'service/$clone_name',
    'service/$router_name',
    'clone-health.json',
    'clone-ready.json',
    'router-health.json',
    'router-ready.json',
    '.readyExecutors == []',
    'live clone and router health/readiness passed with every execution gate false',
  ]);
});
