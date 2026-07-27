import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

function repoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/nats-bridge/src/bin/nats_http_ingress.rs'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repository root from ${process.cwd()}`);
}

const root = repoRoot();
const source = readFileSync(
  resolve(root, 'remote/nats-bridge/src/bin/nats_http_ingress.rs'),
  'utf8',
);
const deployment = readFileSync(
  resolve(root, 'remote/argocd/messaging/nats-bridge.deployment.yaml'),
  'utf8',
);
const routes = readFileSync(
  resolve(root, 'remote/argocd/messaging/nats-bridge.configmap.yaml'),
  'utf8',
);
const externalSecret = readFileSync(
  resolve(root, 'remote/argocd/messaging/nats-bridge.externalsecret.yaml'),
  'utf8',
);
const service = readFileSync(
  resolve(root, 'remote/argocd/messaging/nats-bridge.service.yaml'),
  'utf8',
);
const kustomization = readFileSync(
  resolve(root, 'remote/argocd/messaging/kustomization.yaml'),
  'utf8',
);
const ingressTemplate = readFileSync(
  resolve(root, 'remote/argocd/messaging/nats-bridge.ingress.template.yaml'),
  'utf8',
);
const runbook = readFileSync(resolve(root, 'docs/nats-external-http-ingress.md'), 'utf8');

test('external callers use named HTTP routes and cannot choose NATS subjects', () => {
  assert.match(source, /route\("\/v1\/queues\/:route", post\(enqueue\)\)/);
  assert.doesNotMatch(source, /\/publish\/:subject/);
  assert.doesNotMatch(source, /Path\(subject\)/);
  assert.match(source, /routes\s*\.get\(route_name\)/);
  assert.match(source, /route\s*\.subject\s*\.clone\(\)/);
  assert.match(source, /validate_subject\(&route\.subject\)/);
  assert.match(source, /subject\.starts_with\('\$'\)/);
  assert.match(source, /token == "\*"/);
  assert.match(source, /token == ">"/);
});

test('external ingress is JetStream-only, acknowledged, and idempotent', () => {
  assert.match(source, /Nats-Msg-Id/);
  assert.match(source, /Nats-Expected-Stream/);
  assert.match(source, /publish_with_headers/);
  assert.match(source, /acknowledgement\s*\.await/);
  assert.match(source, /Idempotency-Key|idempotency-key/);
  assert.doesNotMatch(source, /\.nats\s*\.publish\(/);
  assert.doesNotMatch(source, /durable:\s*false/);
  assert.doesNotMatch(source, /PublishError::NoStream/);
});

test('client credentials are scoped by route and mounted as a secret file', () => {
  assert.match(source, /BRIDGE_CLIENTS_FILE/);
  assert.match(source, /client\s*\.routes\s*\.contains\(route_name\)/);
  assert.match(source, /client tokens must be unique/);
  assert.match(source, /MIN_TOKEN_BYTES: usize = 32/);
  assert.match(deployment, /--bin nats_http_ingress/);
  assert.match(deployment, /BRIDGE_CLIENTS_FILE/);
  assert.match(deployment, /BRIDGE_ROUTES_FILE/);
  assert.match(deployment, /key: BRIDGE_CLIENTS_JSON/);
  assert.doesNotMatch(deployment, /name: BRIDGE_TOKEN/);
  assert.doesNotMatch(deployment, /BRIDGE_SUBJECT_PREFIXES/);
  assert.match(externalSecret, /BRIDGE_CLIENTS_JSON/);
  assert.doesNotMatch(externalSecret, /BRIDGE_TOKEN \(≥16 chars/);
});

test('route configuration maps a public name to one exact internal queue', () => {
  assert.match(routes, /"vapi-task"/);
  assert.match(routes, /"subject": "dd\.vapi\.tasks\.external"/);
  assert.match(routes, /"stream": "DD_VAPI_TASKS"/);
  assert.match(routes, /"max_body_bytes": 262144/);
  assert.match(kustomization, /nats-bridge\.configmap\.yaml/);
});

test('public ingress remains inert until TLS and secrets are reviewed', () => {
  assert.match(service, /type: ClusterIP/);
  assert.doesNotMatch(kustomization, /nats-bridge\.ingress\.template\.yaml/);
  assert.match(ingressTemplate, /INERT TEMPLATE/);
  assert.match(ingressTemplate, /nats-ingress\.example\.invalid/);
  assert.match(ingressTemplate, /ssl-redirect: 'true'/);
  assert.match(ingressTemplate, /proxy-body-size: 1m/);
  assert.match(ingressTemplate, /limit-rps: '10'/);
  assert.match(ingressTemplate, /path: \/v1\/queues/);
  assert.match(runbook, /Public ingress activation gates/);
  assert.match(runbook, /Do not expose NATS ports 4222\/6222\/8222\/7777/);
});

test('deployment and runbook prohibit direct NATS for external servers', () => {
  assert.match(runbook, /must not receive NATS\s+credentials or network access/);
  assert.match(runbook, /import a NATS\s+client library/);
  assert.match(runbook, /remove the NATS library, `NATS_\*` variables/);
  assert.match(source, /invalid_json_object/);
  assert.match(source, /body_too_large/);
  assert.match(source, /publish_timeout/);
  assert.match(source, /overloaded/);
  assert.doesNotMatch(source, /error\.to_string\(\).*Json/);
});
