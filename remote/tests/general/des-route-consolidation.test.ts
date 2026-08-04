import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, '../../..');
const runtimeDir = join(repoRoot, 'remote/argocd/dd-next-runtime');

const read = (path: string): string => readFileSync(path, 'utf8');
const runtime = (name: string): string => read(join(runtimeDir, name));

const deployment = runtime('dd-des-web.deployment.yaml');
const service = runtime('dd-des-web.service.yaml');
const compatibilityService = runtime('dd-des-simulator.service.yaml');
const networkPolicy = runtime('dd-des-web.networkpolicy.yaml');
const pdb = runtime('dd-des-web.pdb.yaml');
const kustomization = runtime('kustomization.yaml');
const gateway = runtime('dd-remote-gateway.configmap.yaml');
const docs = read(join(repoRoot, 'docs/des-route-consolidation.md'));

test('dd-next-runtime includes the complete canonical DES web bundle', () => {
  for (const resource of [
    'dd-des-web.deployment.yaml',
    'dd-des-web.networkpolicy.yaml',
    'dd-des-web.pdb.yaml',
    'dd-des-web.service.yaml',
    'dd-des-simulator.service.yaml',
  ]) {
    assert.match(
      kustomization,
      new RegExp(`^\\s*- ${resource.replaceAll('.', '\\.')}$`, 'm'),
      `${resource} must be rendered by the runtime kustomization`,
    );
  }
});

test('DES web runs the exact digest-pinned DES-org image as a hardened HA workload', () => {
  assert.match(deployment, /name: dd-des-web/);
  assert.match(deployment, /replicas: 2/);
  assert.match(deployment, /maxUnavailable: 0/);
  assert.match(deployment, /runAsNonRoot: true/);
  assert.match(deployment, /readOnlyRootFilesystem: true/);
  assert.match(deployment, /automountServiceAccountToken: false/);
  assert.match(deployment, /drop:\n\s+- ALL/);

  const image = deployment.match(
    /image: ghcr\.io\/discrete-event-systems\/des-web\.rs:sha-([0-9a-f]{40})@sha256:([0-9a-f]{64})/,
  );
  assert.ok(image, 'DES web image must use an exact source tag plus digest');
  const sourceRevision = image[1];
  const imageDigest = image[2];

  const sourceAnnotations = [
    ...deployment.matchAll(/dd\.dev\/source-revision: '([0-9a-f]{40})'/g),
  ].map((match) => match[1]);
  assert.ok(sourceAnnotations.length >= 2);
  assert.deepEqual([...new Set(sourceAnnotations)], [sourceRevision]);

  const digestAnnotations = [
    ...deployment.matchAll(
      /dd\.dev\/source-image-digest: 'sha256:([0-9a-f]{64})'/g,
    ),
  ].map((match) => match[1]);
  assert.ok(digestAnnotations.length >= 2);
  assert.deepEqual([...new Set(digestAnnotations)], [imageDigest]);

  assert.doesNotMatch(deployment, /des-web\.rs:(?:main|latest)\b/);
  assert.match(
    deployment,
    /dd\.dev\/source-repository: https:\/\/github\.com\/discrete-event-systems\/des-web\.rs/,
  );
  assert.match(deployment, /name: DES_PUBLIC_PATH_MODE\n\s+value: mounted/);
  assert.match(
    deployment,
    /name: DES_UPSTREAM_URL\n\s+value: http:\/\/dd-des-rs\.default\.svc\.cluster\.local:8112/,
  );
  assert.match(
    deployment,
    /name: DATABASE_URL[\s\S]*name: dd-remote-rest-api-secrets[\s\S]*key: RDS_DATABASE_URL[\s\S]*optional: true/,
  );
  assert.match(deployment, /path: \/healthz/);
  assert.match(deployment, /path: \/readyz/);
  assert.ok(docs.includes(sourceRevision));
  assert.ok(docs.includes(`sha256:${imageDigest}`));
});

test('canonical and compatibility Services select the same DES web pods', () => {
  assert.match(service, /name: dd-des-web/);
  assert.match(service, /selector:\n\s+app: dd-des-web/);
  assert.match(service, /port: 8130\n\s+targetPort: http/);

  assert.match(compatibilityService, /name: dd-des-simulator/);
  assert.match(compatibilityService, /dd\.dev\/compatibility-alias: 'true'/);
  assert.match(compatibilityService, /dd\.dev\/canonical-service: dd-des-web/);
  assert.match(compatibilityService, /selector:\n\s+app: dd-des-web/);
  assert.match(compatibilityService, /port: 8099\n\s+targetPort: http/);
});

test('the existing main gateway keeps /des while its stable upstream is cut over behind the Service', () => {
  assert.match(gateway, /location = \/des \{/);
  assert.match(gateway, /return 302 \/des\//);
  assert.match(gateway, /location \/des\/ \{/);
  assert.match(
    gateway,
    /proxy_pass http:\/\/dd-des-simulator\.default\.svc\.cluster\.local:8099\//,
  );
});

test('network and disruption policies match the DES web ownership boundary', () => {
  assert.match(networkPolicy, /app: dd-remote-gateway/);
  assert.match(networkPolicy, /port: 8130/);
  assert.match(networkPolicy, /app: dd-des-rs/);
  assert.match(networkPolicy, /port: 8112/);
  assert.match(networkPolicy, /port: 5432/);
  assert.match(networkPolicy, /port: 443/);
  assert.doesNotMatch(networkPolicy, /port: 22/);

  assert.match(pdb, /name: dd-des-web/);
  assert.match(pdb, /minAvailable: 1/);
  assert.match(pdb, /app: dd-des-web/);
});

test('route documentation names every canonical public family and the compatibility exception', () => {
  for (const route of [
    '/des/',
    '/des/models',
    '/des/games/soccer',
    '/des/games/elevator',
    '/des/tools/routing',
    '/des/labs/factory-floor-track3t',
    '/des/runs/{run_id}',
    '/des/artifacts/{artifact_id}',
    '/des/api/v1/catalog',
  ]) {
    assert.ok(docs.includes(`\`${route}\``), `${route} must be documented`);
  }
  assert.ok(docs.includes('`/des-rs/*`'));
  assert.ok(docs.includes('`/out/*`'));
  assert.ok(docs.includes('`/des/music`'));
  assert.ok(docs.includes('discrete-event-systems/des-web.rs'));
});
