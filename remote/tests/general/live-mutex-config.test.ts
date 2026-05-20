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

  assert.match(deployment, /name:\s*dd-live-mutex/);
  assert.match(deployment, /replicas:\s*1/);
  assert.match(deployment, /strategy:[\s\S]*type:\s*Recreate/);
  assert.match(deployment, /docker\.io\/library\/node:22-bookworm-slim/);
  assert.match(deployment, /imagePullPolicy:\s*IfNotPresent/);
  assert.match(deployment, /npm install --global --omit=dev --ignore-scripts live-mutex@0\.2\.25/);
  assert.match(deployment, /live_mutex_host=0\.0\.0\.0 live_mutex_port=6970 lmx_start_server/);
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
});

test('node lock loadtest is request-triggered and can compare live-mutex with redis', async () => {
  const packageJson = await readRepoFile('remote/deployments/live-mutex-loadtest-node/package.json');
  const packageLock = await readRepoFile('remote/deployments/live-mutex-loadtest-node/package-lock.json');
  const config = await readRepoFile('remote/deployments/live-mutex-loadtest-node/src/config.js');
  const server = await readRepoFile('remote/deployments/live-mutex-loadtest-node/src/server.js');
  const supervisor = await readRepoFile('remote/deployments/live-mutex-loadtest-node/src/main.js');
  const worker = await readRepoFile('remote/deployments/live-mutex-loadtest-node/src/worker.js');
  const compare = await readRepoFile('remote/deployments/live-mutex-loadtest-node/src/compare.js');
  const readme = await readRepoFile('remote/deployments/live-mutex-loadtest-node/README.md');
  const triggerDeployment = await readRepoFile(
    'remote/deployments/live-mutex-loadtest-node/k8s/ec2/dd-lock-loadtest-trigger.deployment.yaml',
  );
  const triggerService = await readRepoFile(
    'remote/deployments/live-mutex-loadtest-node/k8s/ec2/dd-lock-loadtest-trigger.service.yaml',
  );
  const kustomization = await readRepoFile(
    'remote/deployments/live-mutex-loadtest-node/k8s/ec2/kustomization.yaml',
  );
  const app = await readRepoFile('remote/argocd/apps/dd-lock-loadtest-node.application.yaml');

  assert.match(packageJson, /"live-mutex":\s*"0\.2\.25"/);
  assert.match(packageJson, /"redis":\s*"5\.12\.1"/);
  assert.match(packageJson, /"start":\s*"node src\/server\.js"/);
  assert.match(packageJson, /"loadtest":\s*"node src\/main\.js"/);
  assert.match(packageLock, /"node_modules\/live-mutex"/);
  assert.match(packageLock, /"node_modules\/redis"/);
  assert.match(config, /lockBackend:\s*parseLockBackend\(env\.LOCK_BACKEND\)/);
  assert.match(config, /redisHost:\s*env\.REDIS_HOST \|\| 'dd-redis-cache\.default\.svc\.cluster\.local'/);
  assert.match(config, /redisPort:\s*parsePositiveInteger\('REDIS_PORT', 6379/);
  assert.match(config, /requestsPerSecond:\s*parsePositiveInteger\('REQUESTS_PER_SECOND', 1000/);
  assert.match(config, /Math\.max\(3, parsePositiveInteger\('WORKER_PROCESSES', 3/);
  assert.match(config, /lmx-loadtest-a/);
  assert.match(config, /lmx-loadtest-e/);
  assert.match(server, /http\.createServer/);
  assert.match(server, /POST' && url\.pathname === '\/runs'/);
  assert.match(server, /GET' && url\.pathname === '\/runs\/active'/);
  assert.match(server, /GET' && url\.pathname === '\/runs\/last'/);
  assert.match(server, /a load test is already running/);
  assert.match(server, /spawn\(process\.execPath, \[mode === 'compare' \? comparePath : mainPath\]/);
  assert.match(server, /DEFAULT_TEST_DURATION_SECONDS/);
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
  assert.match(readme, /idle by default/);
  assert.match(readme, /receives `POST \/runs`/);
  assert.match(readme, /starts `3` separate Node\.js worker processes/);
  assert.match(readme, /`1,000` aggregate lock\/acquire\/release cycles per second/);
  assert.match(readme, /spreads traffic over `5` distinct lock keys/);
  assert.match(readme, /`SET key token NX PX <ttl>`/);

  assert.match(triggerDeployment, /name:\s*dd-lock-loadtest-trigger/);
  assert.match(triggerDeployment, /image:\s*docker\.io\/library\/node:22-bookworm-slim/);
  assert.match(triggerDeployment, /npm ci --omit=dev --ignore-scripts/);
  assert.match(triggerDeployment, /exec node src\/server\.js/);
  assert.match(triggerDeployment, /containerPort:\s*8110/);
  assert.match(triggerDeployment, /name:\s*DEFAULT_TEST_DURATION_SECONDS[\s\S]*value:\s*"60"/);
  assert.match(
    triggerDeployment,
    /name:\s*BROKER_HOST[\s\S]*value:\s*dd-live-mutex\.default\.svc\.cluster\.local/,
  );
  assert.match(triggerDeployment, /name:\s*BROKER_PORT[\s\S]*value:\s*"6970"/);
  assert.match(triggerDeployment, /name:\s*REDIS_HOST[\s\S]*value:\s*dd-redis-cache\.default\.svc\.cluster\.local/);
  assert.match(triggerDeployment, /name:\s*REQUESTS_PER_SECOND[\s\S]*value:\s*"1000"/);
  assert.match(triggerDeployment, /name:\s*WORKER_PROCESSES[\s\S]*value:\s*"3"/);
  assert.match(triggerDeployment, /name:\s*CLIENTS_PER_WORKER[\s\S]*value:\s*"12"/);
  assert.match(
    triggerDeployment,
    /name:\s*LOCK_KEYS[\s\S]*value:\s*lmx-loadtest-a,lmx-loadtest-b,lmx-loadtest-c,lmx-loadtest-d,lmx-loadtest-e/,
  );
  assert.match(triggerDeployment, /name:\s*LOCK_MAX_RETRIES[\s\S]*value:\s*"0"/);
  assert.match(triggerService, /name:\s*dd-lock-loadtest-trigger/);
  assert.match(triggerService, /port:\s*8110/);
  assert.match(triggerService, /targetPort:\s*http/);
  assert.match(kustomization, /dd-lock-loadtest-trigger\.deployment\.yaml/);
  assert.match(kustomization, /dd-lock-loadtest-trigger\.service\.yaml/);
  assert.doesNotMatch(kustomization, /dd-live-mutex-loadtest-node\.deployment\.yaml/);
  assert.doesNotMatch(kustomization, /dd-redis-lock-loadtest-node\.deployment\.yaml/);
  assert.match(app, /name:\s*dd-lock-loadtest-node/);
  assert.match(app, /path:\s*remote\/deployments\/live-mutex-loadtest-node\/k8s\/ec2/);
});
