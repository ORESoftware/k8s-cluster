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

test('live-mutex broker is deployed as a singleton cluster-local TCP service', async () => {
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-live-mutex.deployment.yaml',
  );
  const service = await readRepoFile('remote/argocd/dd-next-runtime/dd-live-mutex.service.yaml');
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');

  assert.match(deployment, /name:\s*dd-live-mutex/);
  assert.match(deployment, /replicas:\s*1/);
  assert.match(deployment, /strategy:[\s\S]*type:\s*Recreate/);
  assert.match(deployment, /docker\.io\/oresoftware\/live-mutex-broker:latest/);
  assert.match(deployment, /imagePullPolicy:\s*Always/);
  assert.match(deployment, /name:\s*lmx[\s\S]*containerPort:\s*6970/);
  assert.match(deployment, /startupProbe:[\s\S]*tcpSocket:[\s\S]*port:\s*lmx/);
  assert.match(deployment, /readinessProbe:[\s\S]*tcpSocket:[\s\S]*port:\s*lmx/);
  assert.match(deployment, /livenessProbe:[\s\S]*tcpSocket:[\s\S]*port:\s*lmx/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(deployment, /seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(service, /name:\s*dd-live-mutex/);
  assert.match(service, /name:\s*lmx[\s\S]*port:\s*6970[\s\S]*targetPort:\s*lmx/);
  assert.match(kustomization, /dd-live-mutex\.deployment\.yaml/);
  assert.match(kustomization, /dd-live-mutex\.service\.yaml/);
  assert.match(runtimeReadme, /`dd-live-mutex`/);
  assert.match(runtimeReadme, /dd-live-mutex\.default\.svc\.cluster\.local:6970/);
  assert.match(runtimeReadme, /`replicas: 1` with `strategy: Recreate`/);
});

test('node lock loadtest runs 1k aggregate rps across three processes and five keys', async () => {
  const packageJson = await readRepoFile('remote/live-mutex-loadtest-node/package.json');
  const packageLock = await readRepoFile('remote/live-mutex-loadtest-node/package-lock.json');
  const config = await readRepoFile('remote/live-mutex-loadtest-node/src/config.js');
  const supervisor = await readRepoFile('remote/live-mutex-loadtest-node/src/main.js');
  const worker = await readRepoFile('remote/live-mutex-loadtest-node/src/worker.js');
  const compare = await readRepoFile('remote/live-mutex-loadtest-node/src/compare.js');
  const readme = await readRepoFile('remote/live-mutex-loadtest-node/README.md');
  const liveMutexDeployment = await readRepoFile(
    'remote/live-mutex-loadtest-node/k8s/ec2/dd-live-mutex-loadtest-node.deployment.yaml',
  );
  const redisDeployment = await readRepoFile(
    'remote/live-mutex-loadtest-node/k8s/ec2/dd-redis-lock-loadtest-node.deployment.yaml',
  );
  const kustomization = await readRepoFile(
    'remote/live-mutex-loadtest-node/k8s/ec2/kustomization.yaml',
  );
  const app = await readRepoFile(
    'remote/argocd/apps/dd-live-mutex-loadtest-node.application.yaml',
  );

  assert.match(packageJson, /"live-mutex":\s*"0\.2\.25"/);
  assert.match(packageJson, /"redis":\s*"5\.12\.1"/);
  assert.match(packageLock, /"node_modules\/live-mutex"/);
  assert.match(packageLock, /"node_modules\/redis"/);
  assert.match(config, /lockBackend:\s*parseLockBackend\(env\.LOCK_BACKEND\)/);
  assert.match(config, /redisHost:\s*env\.REDIS_HOST \|\| 'dd-redis-cache\.default\.svc\.cluster\.local'/);
  assert.match(config, /redisPort:\s*parsePositiveInteger\('REDIS_PORT', 6379/);
  assert.match(config, /requestsPerSecond:\s*parsePositiveInteger\('REQUESTS_PER_SECOND', 1000/);
  assert.match(config, /Math\.max\(3, parsePositiveInteger\('WORKER_PROCESSES', 3/);
  assert.match(config, /lmx-loadtest-a/);
  assert.match(config, /lmx-loadtest-e/);
  assert.match(supervisor, /fork\(workerPath/);
  assert.match(supervisor, /distributeRate\(config\.requestsPerSecond, config\.workerProcesses\)/);
  assert.match(supervisor, /worker_pids=/);
  assert.match(worker, /const \{ Client \} = require\('live-mutex'\)/);
  assert.match(worker, /const \{ createClient \} = require\('redis'\)/);
  assert.match(worker, /await client\.ensure\(\)/);
  assert.match(worker, /await client\.acquire\(lockKey/);
  assert.match(worker, /await client\.release\(lock\.key \|\| lockKey/);
  assert.match(worker, /await client\.set\(key, token,[\s\S]*NX:\s*true,[\s\S]*PX:\s*config\.lockTtlMs/);
  assert.match(worker, /await client\.eval\(redisReleaseScript/);
  assert.match(worker, /redis\.call\("get", KEYS\[1\]\) == ARGV\[1\]/);
  assert.match(worker, /process\.send/);
  assert.match(compare, /for \(const backend of \['live-mutex', 'redis'\]\)/);
  assert.match(compare, /lock-loadtest-compare winner backend=/);
  assert.match(readme, /starts `3` separate Node\.js worker processes/);
  assert.match(readme, /`1,000` aggregate lock\/acquire\/release cycles per second/);
  assert.match(readme, /spreads traffic over `5` distinct lock keys/);
  assert.match(readme, /`SET key token NX PX <ttl>`/);

  assert.match(liveMutexDeployment, /name:\s*dd-live-mutex-loadtest-node/);
  assert.match(liveMutexDeployment, /image:\s*docker\.io\/library\/node:22-bookworm-slim/);
  assert.match(liveMutexDeployment, /npm ci --omit=dev --ignore-scripts/);
  assert.match(liveMutexDeployment, /exec node src\/main\.js/);
  assert.match(liveMutexDeployment, /name:\s*LOCK_BACKEND[\s\S]*value:\s*live-mutex/);
  assert.match(
    liveMutexDeployment,
    /name:\s*BROKER_HOST[\s\S]*value:\s*dd-live-mutex\.default\.svc\.cluster\.local/,
  );
  assert.match(liveMutexDeployment, /name:\s*BROKER_PORT[\s\S]*value:\s*"6970"/);
  assert.match(liveMutexDeployment, /name:\s*REDIS_HOST[\s\S]*value:\s*dd-redis-cache\.default\.svc\.cluster\.local/);
  assert.match(liveMutexDeployment, /name:\s*REQUESTS_PER_SECOND[\s\S]*value:\s*"1000"/);
  assert.match(liveMutexDeployment, /name:\s*WORKER_PROCESSES[\s\S]*value:\s*"3"/);
  assert.match(liveMutexDeployment, /name:\s*CLIENTS_PER_WORKER[\s\S]*value:\s*"12"/);
  assert.match(
    liveMutexDeployment,
    /name:\s*LOCK_KEYS[\s\S]*value:\s*lmx-loadtest-a,lmx-loadtest-b,lmx-loadtest-c,lmx-loadtest-d,lmx-loadtest-e/,
  );
  assert.match(liveMutexDeployment, /name:\s*LOCK_MAX_RETRIES[\s\S]*value:\s*"0"/);
  assert.match(redisDeployment, /name:\s*dd-redis-lock-loadtest-node/);
  assert.match(redisDeployment, /name:\s*LOCK_BACKEND[\s\S]*value:\s*redis/);
  assert.match(redisDeployment, /name:\s*REDIS_HOST[\s\S]*value:\s*dd-redis-cache\.default\.svc\.cluster\.local/);
  assert.match(redisDeployment, /name:\s*REDIS_PORT[\s\S]*value:\s*"6379"/);
  assert.match(redisDeployment, /name:\s*REQUESTS_PER_SECOND[\s\S]*value:\s*"1000"/);
  assert.match(redisDeployment, /name:\s*WORKER_PROCESSES[\s\S]*value:\s*"3"/);
  assert.match(kustomization, /dd-live-mutex-loadtest-node\.deployment\.yaml/);
  assert.match(kustomization, /dd-redis-lock-loadtest-node\.deployment\.yaml/);
  assert.match(app, /name:\s*dd-live-mutex-loadtest-node/);
  assert.match(app, /path:\s*remote\/live-mutex-loadtest-node\/k8s\/ec2/);
});
