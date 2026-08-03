import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

const AUTH_RELEASE = '6984b584e5350c1a82a2e5d5ff0195e124aa4542';
const AUTH_DIGEST =
  'sha256:b9377ca8bc5f1298b7adf705563e7a80ab97727337a301578dea42b208102d6c';
const AUTH_IMAGE = 'ghcr.io/fiducia-cloud/fiducia-revocation-admin';
const AUTH_REF = `${AUTH_IMAGE}@${AUTH_DIGEST}`;
const REGISTRY_SECRET = 'fiducia-revocation-ghcr-pull';

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

function literalCount(source: string, literal: string): number {
  return source.split(literal).length - 1;
}

test('runtime and registry credentials remain separated and cloud-backed', async () => {
  const [runtimeSecret, registrySecret] = await Promise.all([
    readRepoFile('remote/argocd/fiducia/fiducia-revocation-secrets.externalsecret.yaml'),
    readRepoFile(
      'remote/argocd/fiducia/fiducia-revocation-registry.externalsecret.yaml',
    ),
  ]);

  assert.match(runtimeSecret, /refreshInterval:\s*5m/);
  assert.match(
    runtimeSecret,
    /secretStoreRef:[\s\S]*kind:\s*ClusterSecretStore[\s\S]*name:\s*dd-fiducia-kv/,
  );
  assert.match(runtimeSecret, /target:[\s\S]*name:\s*fiducia-revocation-secrets/);
  assert.match(runtimeSecret, /creationPolicy:\s*Owner/);
  assert.match(runtimeSecret, /deletionPolicy:\s*Retain/);
  assert.match(
    runtimeSecret,
    /secretKey:\s*admin-secret[\s\S]*key:\s*dd\/remote-dev\/fiducia-revocation[\s\S]*property:\s*FIDUCIA_REVOCATION_ADMIN_SECRET/,
  );
  assert.match(
    runtimeSecret,
    /secretKey:\s*reader-secret[\s\S]*key:\s*dd\/remote-dev\/fiducia-revocation[\s\S]*property:\s*FIDUCIA_REVOCATION_READER_SECRET/,
  );
  assert.equal(literalCount(runtimeSecret, 'secretKey: admin-secret'), 1);
  assert.equal(literalCount(runtimeSecret, 'secretKey: reader-secret'), 1);

  assert.match(registrySecret, /refreshInterval:\s*15m/);
  assert.match(
    registrySecret,
    /secretStoreRef:[\s\S]*kind:\s*ClusterSecretStore[\s\S]*name:\s*dd-cluster-secrets/,
  );
  assert.match(
    registrySecret,
    /target:[\s\S]*name:\s*fiducia-revocation-ghcr-pull[\s\S]*creationPolicy:\s*Owner/,
  );
  assert.match(registrySecret, /type:\s*kubernetes\.io\/dockerconfigjson/);
  assert.match(
    registrySecret,
    /secretKey:\s*\.dockerconfigjson[\s\S]*key:\s*dd\/remote-dev\/fiducia-revocation-ghcr-pull[\s\S]*property:\s*dockerconfigjson/,
  );
  assert.equal(literalCount(registrySecret, 'secretKey: .dockerconfigjson'), 1);
  assert.doesNotMatch(
    registrySecret,
    /admin-secret|reader-secret|FIDUCIA_REVOCATION_(?:ADMIN|READER)_SECRET|FIDUCIA_INTERNAL_SECRET/,
  );
  assert.doesNotMatch(
    registrySecret,
    /^\s+(?:value|stringData):\s*|"auths"|username:|password:|identitytoken:/im,
  );

  assert.doesNotMatch(
    `${runtimeSecret}\n${registrySecret}`,
    /(?:ghp_|github_pat_|fdc_(?:live|test)_|BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY)/,
  );
});

test('revocation authority is an immutable least-privilege runtime', async () => {
  const [rendered, source] = await Promise.all([
    Promise.resolve(renderFiducia()),
    readRepoFile('remote/argocd/fiducia/fiducia-revocation-admin.deployment.yaml'),
  ]);
  const deployment = documentFor(rendered, 'Deployment', 'fiducia-revocation-admin');
  const service = documentFor(rendered, 'Service', 'fiducia-revocation-admin');

  assert.ok(source.includes(`image: ${AUTH_REF}`));
  assert.ok(deployment.includes(`image: ${AUTH_REF}`));
  assert.equal(literalCount(source, `image: ${AUTH_REF}`), 1);
  assert.match(source, new RegExp(`fiducia\\.cloud/release-sha: ["']${AUTH_RELEASE}["']`));
  assert.match(source, new RegExp(`fiducia\\.cloud/release-digest: ["']${AUTH_DIGEST}["']`));
  assert.match(
    source,
    /fiducia\.cloud\/release-ledger:\s*["']fiducia-cloud\/fiducia-auth\.rs#38["']/,
  );
  assert.match(deployment, new RegExp(`fiducia\\.cloud/release-sha: ["']${AUTH_RELEASE}["']`));
  assert.match(
    deployment,
    new RegExp(`fiducia\\.cloud/release-digest: ["']${AUTH_DIGEST}["']`),
  );

  assert.match(deployment, /automountServiceAccountToken:\s*false/);
  assert.match(deployment, /enableServiceLinks:\s*false/);
  assert.match(
    deployment,
    new RegExp(`imagePullSecrets:[\\s\\S]{0,80}- name:\\s*${REGISTRY_SECRET}`),
  );
  assert.equal(literalCount(source, `- name: ${REGISTRY_SECRET}`), 1);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /runAsNonRoot:\s*true/);
  assert.match(deployment, /runAsUser:\s*65532/);
  assert.match(deployment, /runAsGroup:\s*65532/);
  assert.match(deployment, /seccompProfile:[\s\S]{0,80}type:\s*RuntimeDefault/);
  assert.match(deployment, /capabilities:[\s\S]{0,80}drop:[\s\S]{0,40}- ALL/);
  assert.match(deployment, /imagePullPolicy:\s*IfNotPresent/);
  assert.match(
    deployment,
    /name:\s*FIDUCIA_REVOCATION_PORT[\s\S]{0,80}value:\s*["']?8098["']?/,
  );
  assert.match(
    deployment,
    /name:\s*FIDUCIA_REVOCATION_ADMIN_SECRET[\s\S]{0,220}name:\s*fiducia-revocation-secrets[\s\S]{0,120}key:\s*admin-secret[\s\S]{0,120}optional:\s*false/,
  );
  assert.match(
    deployment,
    /name:\s*FIDUCIA_REVOCATION_READER_SECRET[\s\S]{0,220}name:\s*fiducia-revocation-secrets[\s\S]{0,120}key:\s*reader-secret[\s\S]{0,120}optional:\s*false/,
  );
  assert.match(
    deployment,
    /name:\s*FIDUCIA_INTERNAL_SECRET[\s\S]{0,220}name:\s*fiducia-cluster-secrets[\s\S]{0,120}key:\s*internal-secret[\s\S]{0,120}optional:\s*false/,
  );
  assert.match(
    deployment,
    /name:\s*FIDUCIA_KV_ORG_ID[\s\S]{0,80}value:\s*fiducia-revocation/,
  );
  assert.match(deployment, /path:\s*\/healthz/);
  assert.match(
    deployment,
    /requests:[\s\S]{0,100}cpu:\s*50m[\s\S]{0,80}memory:\s*128Mi/,
  );
  assert.match(
    deployment,
    /limits:[\s\S]{0,100}cpu:\s*["']?1["']?[\s\S]{0,80}memory:\s*512Mi/,
  );
  assert.match(service, /port:\s*8098/);
  assert.match(service, /targetPort:\s*http/);

  assert.doesNotMatch(
    source,
    /docker\.io\/library\/rust|\bgit\s+(?:clone|fetch|checkout)\b|\bcargo\s+(?:run|build)\b|\/workspace|CARGO_HOME|CARGO_TARGET_DIR|hostPath:|emptyDir:/,
  );
  assert.doesNotMatch(source, /^\s+(?:command|args|volumeMounts|volumes):/m);
  assert.doesNotMatch(
    source,
    /ghcr\.io\/fiducia-cloud\/fiducia-revocation-admin:[^\s@]+/,
  );
  assert.doesNotMatch(
    source,
    /(?:secretRef|secretKeyRef):[\s\S]{0,120}name:\s*fiducia-revocation-ghcr-pull/,
  );
});

test('load balancer receives the reader capability only', () => {
  const rendered = renderFiducia();
  const loadBalancer = documentFor(rendered, 'Deployment', 'fiducia-load-balance');

  assert.match(
    loadBalancer,
    /dd\.dev\/fiducia-revocation-contract:\s*den-1119-reader-only-v2/,
  );
  assert.match(
    loadBalancer,
    /name:\s*FIDUCIA_REVOCATION_CHECK_URL[\s\S]{0,180}\/v1\/revocations\/check/,
  );
  assert.match(
    loadBalancer,
    /name:\s*FIDUCIA_REVOCATION_READER_SECRET[\s\S]{0,220}name:\s*fiducia-revocation-secrets[\s\S]{0,120}key:\s*reader-secret[\s\S]{0,120}optional:\s*false/,
  );
  assert.match(
    loadBalancer,
    /name:\s*FIDUCIA_REVOCATION_CACHE_FRESHNESS_SECS[\s\S]{0,80}value:\s*["']?30["']?/,
  );
  assert.match(
    loadBalancer,
    /name:\s*FIDUCIA_REVOCATION_CACHE_CAPACITY[\s\S]{0,80}value:\s*["']?65536["']?/,
  );
  assert.match(
    loadBalancer,
    /name:\s*FIDUCIA_REVOCATION_TIMEOUT_MILLIS[\s\S]{0,80}value:\s*["']?2000["']?/,
  );
  assert.equal(literalCount(loadBalancer, 'FIDUCIA_REVOCATION_READER_SECRET'), 1);
  assert.doesNotMatch(
    loadBalancer,
    /FIDUCIA_REVOCATION_ADMIN_SECRET|key:\s*admin-secret|fiducia-revocation-ghcr-pull/,
  );
});

test('network policy has no public bootstrap egress', async () => {
  const [rendered, source] = await Promise.all([
    Promise.resolve(renderFiducia()),
    readRepoFile('remote/argocd/fiducia/fiducia-revocation-admin.networkpolicy.yaml'),
  ]);
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

  assert.match(
    authorityPolicy,
    /podSelector:[\s\S]{0,120}app\.kubernetes\.io\/name:\s*fiducia-revocation-admin/,
  );
  assert.match(
    authorityPolicy,
    /ingress:[\s\S]{0,240}app:\s*fiducia-load-balance[\s\S]{0,100}port:\s*8098/,
  );
  assert.match(
    authorityPolicy,
    /egress:[\s\S]{0,240}app:\s*fiducia-load-balance[\s\S]{0,100}port:\s*8088/,
  );
  assert.match(
    authorityPolicy,
    /kubernetes\.io\/metadata\.name:\s*kube-system[\s\S]{0,180}k8s-app:\s*kube-dns[\s\S]{0,180}port:\s*53/,
  );
  assert.match(
    readerPolicy,
    /app:\s*fiducia-load-balance[\s\S]{0,260}app\.kubernetes\.io\/name:\s*fiducia-revocation-admin[\s\S]{0,100}port:\s*8098/,
  );
  assert.equal(source.match(/^kind:\s*NetworkPolicy\s*$/gm)?.length, 2);
  assert.doesNotMatch(source, /ipBlock:|0\.0\.0\.0\/0/);
  assert.doesNotMatch(source, /^\s+port:\s*(?:80|443)\s*$/m);
  assert.doesNotMatch(source, /namespaceSelector:\s*\{\}|podSelector:\s*\{\}/);
});

test('kustomization owns every revocation resource exactly once', async () => {
  const kustomization = await readRepoFile('remote/argocd/fiducia/kustomization.yaml');
  const resources = [
    'fiducia-revocation-secrets.externalsecret.yaml',
    'fiducia-revocation-registry.externalsecret.yaml',
    'fiducia-revocation-admin.deployment.yaml',
    'fiducia-revocation-admin.networkpolicy.yaml',
  ];

  for (const resource of resources) {
    assert.equal(literalCount(kustomization, `  - ${resource}`), 1, resource);
  }
  assert.equal(
    literalCount(kustomization, '  - path: fiducia-load-balance.revocation.patch.yaml'),
    1,
  );
  assert.match(
    kustomization,
    /path:\s*fiducia-load-balance\.revocation\.patch\.yaml[\s\S]{0,180}kind:\s*Deployment[\s\S]{0,80}name:\s*fiducia-load-balance/,
  );

  const rendered = renderFiducia();
  documentFor(rendered, 'ExternalSecret', 'fiducia-revocation-secrets');
  documentFor(rendered, 'ExternalSecret', REGISTRY_SECRET);
  documentFor(rendered, 'Deployment', 'fiducia-revocation-admin');
  documentFor(rendered, 'Service', 'fiducia-revocation-admin');
  documentFor(rendered, 'NetworkPolicy', 'fiducia-revocation-admin');
  documentFor(rendered, 'NetworkPolicy', 'fiducia-load-balance-to-revocation-reader');
});

test('rendered revocation contract contains no credential material', () => {
  const rendered = renderFiducia();
  const registrySecret = documentFor(rendered, 'ExternalSecret', REGISTRY_SECRET);

  assert.doesNotMatch(
    rendered,
    /ghp_[A-Za-z0-9]+|github_pat_[A-Za-z0-9_]+|fdc_(?:live|test)_[A-Za-z0-9_.-]+|BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY/,
  );
  assert.doesNotMatch(
    rendered,
    /reader-secret-reader-secret|admin-secret-admin-secret/,
  );
  assert.doesNotMatch(
    rendered,
    /name:\s*FIDUCIA_REVOCATION_(?:ADMIN|READER)_SECRET\s*\n\s*value:\s*[^\n]+/,
  );
  assert.doesNotMatch(
    rendered,
    /name:\s*FIDUCIA_INTERNAL_SECRET\s*\n\s*value:\s*[^\n]+/,
  );
  assert.doesNotMatch(
    registrySecret,
    /^\s+(?:value|stringData):\s*|"auths"|username:|password:|identitytoken:/im,
  );
});

test('runbook binds live evidence to exact release and pull boundaries', async () => {
  const [runbook, deployment] = await Promise.all([
    readRepoFile('docs/fiducia-revocation-deployment-runbook.md'),
    readRepoFile('remote/argocd/fiducia/fiducia-revocation-admin.deployment.yaml'),
  ]);

  for (const required of [
    AUTH_RELEASE,
    AUTH_DIGEST,
    AUTH_REF,
    'fiducia-cloud/fiducia-auth.rs#38',
    'fiducia-revocation-ghcr-pull',
    'dd/remote-dev/fiducia-revocation-ghcr-pull',
    'kubernetes.io/dockerconfigjson',
    'registry-only',
    'K8S_SUBMODULE_APP_ID',
    'K8S_SUBMODULE_APP_PRIVATE_KEY',
    'Two-verifier propagation exercise',
    'Authority-loss, malformed-response, and concurrency exercise',
    'Runtime credential rotation',
    'Registry credential rotation',
    'Rollback',
    'independent reviewer',
  ]) {
    assert.ok(runbook.includes(required), `runbook missing ${required}`);
  }
  assert.ok(deployment.includes(AUTH_REF));
  assert.ok(deployment.includes(`- name: ${REGISTRY_SECRET}`));
  assert.match(runbook, /DEN-1119 remains open until/);
  assert.match(runbook, /no Rust builder image/);
  assert.match(
    runbook,
    /no authority egress rule contains `ipBlock`, `0\.0\.0\.0\/0`/,
  );
  assert.doesNotMatch(runbook, /bypass revocation to restore availability/i);
});
