import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/fiducia/kustomization.yaml'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

function renderFiducia(): string {
  return execFileSync('kubectl', ['kustomize', 'remote/argocd/fiducia'], {
    cwd: repoRoot,
    encoding: 'utf8',
  });
}

function renderedDocument(rendered: string, kind: string, name: string): string {
  const document = rendered
    .split(/^---\s*$/m)
    .find(
      (candidate) =>
        new RegExp(`kind:\\s*${kind}\\b`).test(candidate) &&
        new RegExp(`name:\\s*${name}\\b`).test(candidate),
    );
  assert.ok(document, `rendered ${kind}/${name} must exist`);
  return document;
}

test('cert-manager owns the private CA and hostname-scoped service certificate', () => {
  const rendered = renderFiducia();
  const server = renderedDocument(rendered, 'Certificate', 'fiducia-load-balance-tls');

  assert.match(server, /secretName:\s*fiducia-load-balance-tls/);
  assert.match(server, /rotationPolicy:\s*Always/);
  assert.match(server, /renewBefore:\s*360h/);
  assert.match(server, /fiducia-load-balance\.fiducia\.svc\.cluster\.local/);
  assert.match(server, /usages:\s*\n\s*- server auth/);
  assert.doesNotMatch(rendered, /BEGIN (?:EC |RSA )?PRIVATE KEY/);
  assert.doesNotMatch(rendered, /BEGIN CERTIFICATE/);
});

test('load balancer renders dual listeners with read-only generated TLS material', () => {
  const rendered = renderFiducia();
  const deployment = renderedDocument(rendered, 'Deployment', 'fiducia-load-balance');
  const service = renderedDocument(rendered, 'Service', 'fiducia-load-balance');

  assert.match(deployment, /name:\s*FIDUCIA_TLS_CERT_PATH[\s\S]*value:\s*\/etc\/fiducia\/tls\/tls\.crt/);
  assert.match(deployment, /name:\s*FIDUCIA_TLS_KEY_PATH[\s\S]*value:\s*\/etc\/fiducia\/tls\/tls\.key/);
  assert.match(deployment, /name:\s*TLS_PORT[\s\S]*value:\s*['"]?8443['"]?/);
  assert.match(deployment, /name:\s*https\s*\n\s*containerPort:\s*8443/);
  assert.match(deployment, /mountPath:\s*\/etc\/fiducia\/tls\s*\n\s*readOnly:\s*true/);
  assert.match(deployment, /secretName:\s*fiducia-load-balance-tls/);
  assert.match(deployment, /optional:\s*false/);
  assert.match(deployment, /startupProbe:[\s\S]*port:\s*http/);
  assert.match(service, /name:\s*https[\s\S]*port:\s*8443[\s\S]*targetPort:\s*https/);
  assert.match(service, /name:\s*http[\s\S]*port:\s*8088/);
});

test('ESO uses verified HTTPS and pins only the public CA field', async () => {
  const webhook = await readFile(
    resolve(repoRoot, 'remote/argocd/secrets/common/fiducia-webhook.yaml'),
    'utf8',
  );

  assert.match(
    webhook,
    /url:\s*['"]https:\/\/fiducia-load-balance\.fiducia\.svc\.cluster\.local:8443\/v1\/kv/,
  );
  assert.match(webhook, /caProvider:\s*\n\s*type:\s*Secret/);
  assert.match(webhook, /name:\s*fiducia-load-balance-tls/);
  assert.match(webhook, /namespace:\s*fiducia/);
  assert.match(webhook, /key:\s*ca\.crt/);
  assert.doesNotMatch(webhook, /url:\s*['"]http:\/\/fiducia-load-balance/);
  assert.doesNotMatch(webhook, /tls\.key|BEGIN PRIVATE KEY/);
});

test('NetworkPolicy gives ESO only the encrypted listener', async () => {
  const policy = await readFile(
    resolve(repoRoot, 'remote/argocd/fiducia/fiducia-load-balance.networkpolicy.yaml'),
    'utf8',
  );
  const esoRule = policy.match(
    /# ESO is the first migrated direct client\.[\s\S]*?(?=\n\s*# Application clients migrate)/,
  )?.[0];

  assert.ok(esoRule, 'ESO ingress rule must be present');
  assert.match(esoRule, /kubernetes\.io\/metadata\.name:\s*external-secrets/);
  assert.match(esoRule, /port:\s*8443/);
  assert.doesNotMatch(esoRule, /port:\s*8088/);
});

test('runbook retains downgrade removal and rotation as explicit production gates', async () => {
  const runbook = await readFile(
    resolve(repoRoot, 'docs/fiducia-internal-tls-runbook.md'),
    'utf8',
  );
  assert.match(runbook, /unknown CA/i);
  assert.match(runbook, /hostname mismatch/i);
  assert.match(runbook, /overlap/i);
  assert.match(runbook, /426 Upgrade Required/i);
  assert.match(runbook, /remove port 8088/i);
  assert.match(runbook, /production gate/i);
});
