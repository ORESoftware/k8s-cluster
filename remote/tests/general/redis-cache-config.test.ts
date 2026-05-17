import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/dd-next-runtime/kustomization.yaml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('redis cache is deployed as an ephemeral cluster-local service', async () => {
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-redis-cache.deployment.yaml',
  );
  const config = await readRepoFile('remote/argocd/dd-next-runtime/dd-redis-cache.configmap.yaml');
  const networkPolicy = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-redis-cache.networkpolicy.yaml',
  );
  const service = await readRepoFile('remote/argocd/dd-next-runtime/dd-redis-cache.service.yaml');
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');

  assert.match(deployment, /name:\s*dd-redis-cache/);
  assert.match(deployment, /replicas:\s*1/);
  assert.match(deployment, /strategy:[\s\S]*type:\s*Recreate/);
  assert.match(deployment, /image:\s*redis:7\.4\.9-alpine/);
  assert.match(deployment, /command:[\s\S]*-\s*redis-server/);
  assert.match(deployment, /args:[\s\S]*-\s*\/usr\/local\/etc\/redis\/redis\.conf/);
  assert.match(deployment, /name:\s*redis[\s\S]*containerPort:\s*6379/);
  assert.match(config, /appendonly\s+no/);
  assert.match(config, /save\s+""/);
  assert.match(config, /maxmemory\s+256mb/);
  assert.match(config, /maxmemory-policy\s+allkeys-lru/);
  assert.match(config, /aclfile\s+\/usr\/local\/etc\/redis\/users\.acl/);
  assert.match(config, /user default on nopass/);
  assert.match(config, /-@admin/);
  assert.match(config, /-@dangerous/);
  assert.match(config, /-eval\s+-evalsha/);
  assert.match(deployment, /startupProbe:[\s\S]*redis-cli[\s\S]*ping/);
  assert.match(deployment, /readinessProbe:[\s\S]*redis-cli[\s\S]*ping/);
  assert.match(deployment, /livenessProbe:[\s\S]*redis-cli[\s\S]*ping/);
  assert.match(deployment, /automountServiceAccountToken:\s*false/);
  assert.match(deployment, /fsGroup:\s*999/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /runAsNonRoot:\s*true/);
  assert.match(deployment, /runAsUser:\s*999/);
  assert.match(deployment, /runAsGroup:\s*999/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(deployment, /seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(deployment, /mountPath:\s*\/usr\/local\/etc\/redis[\s\S]*readOnly:\s*true/);
  assert.match(deployment, /mountPath:\s*\/data/);
  assert.match(deployment, /mountPath:\s*\/tmp/);
  assert.match(deployment, /name:\s*data[\s\S]*emptyDir:[\s\S]*sizeLimit:\s*512Mi/);
  assert.match(deployment, /name:\s*tmp[\s\S]*emptyDir:[\s\S]*sizeLimit:\s*64Mi/);
  assert.match(networkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(networkPolicy, /name:\s*dd-redis-cache/);
  assert.match(networkPolicy, /policyTypes:[\s\S]*-\s*Ingress[\s\S]*-\s*Egress/);
  assert.match(networkPolicy, /dd\.dev\/redis-cache-client:\s*'true'/);
  assert.match(networkPolicy, /port:\s*6379/);
  assert.match(networkPolicy, /egress:\s*\[\]/);
  assert.match(service, /name:\s*dd-redis-cache/);
  assert.match(service, /type:\s*ClusterIP/);
  assert.match(service, /name:\s*redis[\s\S]*port:\s*6379[\s\S]*targetPort:\s*redis/);
  assert.match(kustomization, /dd-redis-cache\.configmap\.yaml/);
  assert.match(kustomization, /dd-redis-cache\.deployment\.yaml/);
  assert.match(kustomization, /dd-redis-cache\.networkpolicy\.yaml/);
  assert.match(kustomization, /dd-redis-cache\.service\.yaml/);
  assert.match(runtimeReadme, /`dd-redis-cache`/);
  assert.match(runtimeReadme, /dd-redis-cache\.default\.svc\.cluster\.local:6379/);
  assert.match(runtimeReadme, /allkeys-lru/);
  assert.match(runtimeReadme, /dd\.dev\/redis-cache-client/);
});
