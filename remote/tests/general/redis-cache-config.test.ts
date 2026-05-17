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
  const service = await readRepoFile('remote/argocd/dd-next-runtime/dd-redis-cache.service.yaml');
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');

  assert.match(deployment, /name:\s*dd-redis-cache/);
  assert.match(deployment, /replicas:\s*1/);
  assert.match(deployment, /strategy:[\s\S]*type:\s*Recreate/);
  assert.match(deployment, /image:\s*redis:7\.4-alpine/);
  assert.match(deployment, /name:\s*redis[\s\S]*containerPort:\s*6379/);
  assert.match(deployment, /--appendonly[\s\S]*-\s*'no'/);
  assert.match(deployment, /--save[\s\S]*-\s*''/);
  assert.match(deployment, /--maxmemory[\s\S]*-\s*256mb/);
  assert.match(deployment, /--maxmemory-policy[\s\S]*-\s*allkeys-lru/);
  assert.match(deployment, /startupProbe:[\s\S]*redis-cli[\s\S]*ping/);
  assert.match(deployment, /readinessProbe:[\s\S]*redis-cli[\s\S]*ping/);
  assert.match(deployment, /livenessProbe:[\s\S]*redis-cli[\s\S]*ping/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /runAsNonRoot:\s*true/);
  assert.match(deployment, /runAsUser:\s*999/);
  assert.match(deployment, /runAsGroup:\s*999/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(deployment, /seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(service, /name:\s*dd-redis-cache/);
  assert.match(service, /type:\s*ClusterIP/);
  assert.match(service, /name:\s*redis[\s\S]*port:\s*6379[\s\S]*targetPort:\s*redis/);
  assert.match(kustomization, /dd-redis-cache\.deployment\.yaml/);
  assert.match(kustomization, /dd-redis-cache\.service\.yaml/);
  assert.match(runtimeReadme, /`dd-redis-cache`/);
  assert.match(runtimeReadme, /dd-redis-cache\.default\.svc\.cluster\.local:6379/);
  assert.match(runtimeReadme, /allkeys-lru/);
});
