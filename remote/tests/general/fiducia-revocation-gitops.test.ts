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

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

function renderFiducia(): string {
  return execFileSync('kubectl', ['kustomize', 'remote/argocd/fiducia'], {
    cwd: repoRoot,
    encoding: 'utf8',
  });
}

function documentFor(rendered: string, kind: string, name: string): string {
  const document = rendered
    .split(/^---\s*$/m)
    .find(
      (candidate) =>
        new RegExp(`^kind:\\s*${kind}\\s*$`, 'm').test(candidate) &&
        new RegExp(`^\\s*name:\\s*${name}\\s*$`, 'm').test(candidate),
    );
  assert.ok(document, `missing rendered ${kind}/${name}`);
  return document;
}

test('revocation credentials remain distinct and cloud-backed', async () => {
  const externalSecret = await readRepoFile(
    'remote/argocd/fiducia/fiducia-revocation-secrets.externalsecret.yaml',
  );

  assert.match(externalSecret, /target:[\s\S]*name:\s*fiducia-revocation-secrets/);
  assert.match(externalSecret, /creationPolicy:\s*Owner/);
  assert.match(externalSecret, /deletionPolicy:\s*Retain/);
  assert.match(externalSecret, /secretKey:\s*admin-secret[\s\S]*property:\s*FIDUCIA_REVOCATION_ADMIN_SECRET/);
  assert.match(externalSecret, /secretKey:\s*reader-secret[\s\S]*property:\s*FIDUCIA_REVOCATION_READER_SECRET/);
  assert.match(externalSecret, /key:\s*dd\/remote-dev\/fiducia-revocation/);
  assert.doesNotMatch(externalSecret, /(?:admin|reader)-secret-reader-secret|fdc_(?:live|test)_/);
});

test('revocation authority renders hardened and pinned', async () => {
  const [rendered, source] = await Promise.all([
    Promise.resolve(renderFiducia()),
    readRepoFile('remote/argocd/fiducia/fiducia-revocation-admin.deployment.yaml'),
  ]);
  const deployment = documentFor(rendered, 'Deployment', 'fiducia-revocation-admin');
  const service = documentFor(rendered, 'Service', 'fiducia-revocation-admin');

  assert.match(deployment, /automountServiceAccountToken:\s*false/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /runAsNonRoot:\s*true/);
  assert.match(deployment, /seccompProfile:[\s\S]{0,80}type:\s*RuntimeDefault/);
  assert.match(deployment, /capabilities:[\s\S]{0,80}drop:[\s\S]{0,40}- ALL/);
  assert.match(deployment, /name:\s*FIDUCIA_REVOCATION_PORT[\s\S]{0,80}value:\s*["']?8098["']?/);
  assert.match(deployment, /name:\s*FIDUCIA_REVOCATION_ADMIN_SECRET[\s\S]{0,220}name:\s*fiducia-revocation-secrets[\s\S]{0,120}key:\s*admin-secret[\s\S]{0,120}optional:\s*false/);
  assert.match(deployment, /name:\s*FIDUCIA_REVOCATION_READER_SECRET[\s\S]{0,220}name:\s*fiducia-revocation-secrets[\s\S]{0,120}key:\s*reader-secret[\s\S]{0,120}optional:\s*false/);
  assert.match(deployment, /name:\s*FIDUCIA_KV_ORG_ID[\s\S]{0,80}value:\s*fiducia-revocation/);
  assert.match(deployment, /path:\s*\/healthz/);
  assert.match(service, /port:\s*8098/);

  for (const commit of [
    'bd718cd72d72aa330534f3688f8fb1ce90c19d10',
    'ff635ebee5fcdde6c9c56492a2265eedad7bdd25',
  ]) {
    assert.match(source, new RegExp(`fetch --depth 1 origin ${commit}`));
    assert.match(source, /checkout --detach FETCH_HEAD/);
  }
  assert.match(source, /cargo run[\s\S]*--locked[\s\S]*--release[\s\S]*--bin fiducia-revocation-admin/);
  assert.doesNotMatch(source, /--branch\s+(?:main|dev)|git checkout\s+(?:main|dev)/);
});

test('load balancer receives reader capability only', async () => {
  const rendered = renderFiducia();
  const loadBalancer = documentFor(rendered, 'Deployment', 'fiducia-load-balance');

  assert.match(loadBalancer, /name:\s*FIDUCIA_REVOCATION_CHECK_URL[\s\S]{0,160}\/v1\/revocations\/check/);
  assert.match(loadBalancer, /name:\s*FIDUCIA_REVOCATION_READER_SECRET[\s\S]{0,220}name:\s*fiducia-revocation-secrets[\s\S]{0,120}key:\s*reader-secret[\s\S]{0,120}optional:\s*false/);
  assert.match(loadBalancer, /name:\s*FIDUCIA_REVOCATION_CACHE_FRESHNESS_SECS[\s\S]{0,80}value:\s*["']?30["']?/);
  assert.match(loadBalancer, /name:\s*FIDUCIA_REVOCATION_TIMEOUT_MILLIS[\s\S]{0,80}value:\s*["']?2000["']?/);
  assert.doesNotMatch(loadBalancer, /FIDUCIA_REVOCATION_ADMIN_SECRET|key:\s*admin-secret/);
});

test('network policy exposes only the reader path from the load balancer', async () => {
  const rendered = renderFiducia();
  const authorityPolicy = documentFor(
    rendered,
    'NetworkPolicy',
    'fiducia-revocation-admin',
  );
  const readerPolicy = documentFor(
    rendered,
    'NetworkPolicy',
    'fiducia-load-balance-to-revocation-reader',
  );

  assert.match(authorityPolicy, /podSelector:[\s\S]{0,120}app\.kubernetes\.io\/name:\s*fiducia-revocation-admin/);
  assert.match(authorityPolicy, /ingress:[\s\S]{0,220}app:\s*fiducia-load-balance[\s\S]{0,100}port:\s*8098/);
  assert.match(authorityPolicy, /egress:[\s\S]{0,220}app:\s*fiducia-load-balance[\s\S]{0,100}port:\s*8088/);
  assert.match(readerPolicy, /app:\s*fiducia-load-balance[\s\S]{0,260}app\.kubernetes\.io\/name:\s*fiducia-revocation-admin[\s\S]{0,100}port:\s*8098/);
  assert.doesNotMatch(authorityPolicy, /namespaceSelector:\s*\{\}|podSelector:\s*\{\}/);
});

test('rendered revocation contract contains no credential material', () => {
  const rendered = renderFiducia();
  assert.doesNotMatch(rendered, /fdc_(?:live|test)_[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/);
  assert.doesNotMatch(rendered, /reader-secret-reader-secret|admin-secret-admin-secret/);
  assert.doesNotMatch(rendered, /FIDUCIA_REVOCATION_(?:ADMIN|READER)_SECRET\s*:\s*[^\n]+/);
});
