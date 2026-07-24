import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/secrets/common/kustomization.yaml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('common secrets bundle renders the Fiducia webhook store and bootstrap credentials', () => {
  const rendered = execFileSync('kubectl', ['kustomize', 'remote/argocd/secrets/common'], {
    cwd: repoRoot,
    encoding: 'utf8',
  });

  assert.match(rendered, /kind:\s*ClusterSecretStore[\s\S]*name:\s*dd-fiducia-kv/);
  assert.match(rendered, /dd\.dev\/secret-backend:\s*fiducia-kv/);
  assert.match(
    rendered,
    /url:\s*http:\/\/fiducia-load-balance\.fiducia\.svc\.cluster\.local:8088\/v1\/kv\?key=\{\{[\s\n]*\.remoteRef\.key \}\}/,
  );
  assert.match(rendered, /jsonPath:\s*\$\.entry\.value/);
  assert.match(rendered, /Authorization:\s*Bearer \{\{ \.auth\.token \}\}/);
  assert.match(rendered, /name:\s*fiducia-eso-reader[\s\S]*namespace:\s*external-secrets/);
  assert.match(rendered, /key:\s*dd\/remote-dev\/fiducia-eso-reader/);
  assert.match(rendered, /property:\s*FIDUCIA_API_KEY/);
  assert.match(rendered, /external-secrets\.io\/type:\s*webhook/);
  assert.doesNotMatch(rendered, /urlquery/);
  assert.doesNotMatch(rendered, /fdc_(?:live|test)_[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/);
});

test('Fiducia nodes require the cloud-bootstrapped versioned encryption keyring', async () => {
  const [bootstrap, node] = await Promise.all([
    readRepoFile('remote/argocd/fiducia/fiducia-kv-protection.externalsecret.yaml'),
    readRepoFile('remote/argocd/fiducia/fiducia-node.statefulset.yaml'),
  ]);

  assert.match(bootstrap, /key:\s*dd\/remote-dev\/fiducia-kv-protection/);
  assert.match(bootstrap, /property:\s*FIDUCIA_KV_ENCRYPTION_KEYS/);
  assert.match(bootstrap, /property:\s*FIDUCIA_KV_ENCRYPTION_ACTIVE_KEY_ID/);
  assert.match(bootstrap, /argocd\.argoproj\.io\/sync-wave:\s*"-1"/);
  assert.match(node, /name:\s*FIDUCIA_KV_ENCRYPTION_KEYS/);
  assert.match(node, /name:\s*fiducia-kv-protection\s*\n\s*key:\s*encryption-keys\s*\n\s*optional:\s*false/);
  assert.match(node, /name:\s*FIDUCIA_KV_ENCRYPTION_ACTIVE_KEY_ID/);
  assert.match(node, /name:\s*fiducia-kv-protection\s*\n\s*key:\s*active-key-id\s*\n\s*optional:\s*false/);
});

test('Fiducia load balancer admits only the ESO controller from its namespace', async () => {
  const policy = await readRepoFile(
    'remote/argocd/fiducia/fiducia-load-balance.networkpolicy.yaml',
  );

  assert.match(
    policy,
    /kubernetes\.io\/metadata\.name:\s*external-secrets[\s\S]{0,180}app\.kubernetes\.io\/name:\s*external-secrets/,
  );
  assert.match(policy, /protocol:\s*TCP\s*\n\s*port:\s*8088/);
});

test('runbook shows the app-owned ExternalSecret, env injection, and restart behavior', async () => {
  const runbook = await readRepoFile('docs/fiducia-secret-delivery.md');

  assert.match(runbook, /name:\s*dd-fiducia-kv/);
  assert.match(runbook, /key:\s*k8s\/default\/example-api\/DATABASE_URL/);
  assert.match(runbook, /envFrom:[\s\S]*secretRef:[\s\S]*name:\s*example-api-secrets/);
  assert.match(runbook, /secret\.reloader\.stakater\.com\/reload:\s*example-api-secrets/);
  assert.match(runbook, /environment variables do not change until the pod restarts/i);
  assert.match(runbook, /never give ESO write scope/i);
});
