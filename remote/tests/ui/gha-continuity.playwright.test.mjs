import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { readFile } from 'node:fs/promises';
import net from 'node:net';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { chromium, request as playwrightRequest } from 'playwright';

const AUTH_SECRET = 'browser-test-server-auth';
const REPOSITORY = 'ORESoftware/k8s-cluster';
const REVISION = '0123456789abcdef0123456789abcdef01234567';
const WORKFLOW_PATH = '.github/workflows/gha-continuity-parity.yml';
const REPOSITORY_ROOT =
  process.env.GHA_CONTINUITY_REPOSITORY_ROOT ??
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const FIXTURE_PATH = path.join(
  REPOSITORY_ROOT,
  'remote/deployments/gha-clone-server-rs/tests/fixtures/parity-rust-node.yml',
);

async function unusedPort() {
  const server = net.createServer();
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();
  assert(address && typeof address === 'object');
  const { port } = address;
  server.close();
  await once(server, 'close');
  return port;
}

async function waitForReady(baseURL, process) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    assert.equal(process.exitCode, null, 'gha-clone-server exited before readiness');
    try {
      const response = await fetch(`${baseURL}/healthz`);
      if (response.ok) return;
    } catch {
      // The listener is not ready yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error('gha-clone-server did not become ready');
}

function planRequest(workflowYaml, revision = REVISION) {
  return {
    repository: REPOSITORY,
    revision,
    workflowPath: WORKFLOW_PATH,
    workflowYaml,
  };
}

test('Chromium and APIRequest exercise the real fail-closed continuity server', async (t) => {
  const binary = process.env.GHA_CLONE_SERVER_BIN;
  assert(binary, 'GHA_CLONE_SERVER_BIN is required');
  const workflowYaml = await readFile(FIXTURE_PATH, 'utf8');
  const port = await unusedPort();
  const baseURL = `http://127.0.0.1:${port}`;
  let stderr = '';
  const server = spawn(binary, [], {
    env: {
      ...process.env,
      HOST: '127.0.0.1',
      PORT: String(port),
      RUST_LOG: 'error',
      GHA_CLONE_AUTH_SECRET: AUTH_SECRET,
      GHA_CLONE_ALLOWED_REPOSITORIES: REPOSITORY,
      GHA_CLONE_EXECUTION_ENABLED: 'false',
      GHA_CLONE_WEBHOOK_EXECUTION_ENABLED: 'false',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  server.stderr.setEncoding('utf8');
  server.stderr.on('data', (chunk) => {
    stderr += chunk;
  });
  t.after(async () => {
    if (server.exitCode === null) server.kill('SIGTERM');
    await Promise.race([
      once(server, 'exit'),
      new Promise((resolve) => setTimeout(resolve, 2_000)),
    ]);
    assert(!stderr.includes(AUTH_SECRET), 'server logs reflected the auth secret');
  });
  await waitForReady(baseURL, server);

  const browser = await chromium.launch({ headless: true });
  t.after(() => browser.close());
  const page = await browser.newPage();
  const navigation = await page.goto(`${baseURL}/`);
  assert(navigation);
  assert.equal(navigation.status(), 200);
  const descriptor = JSON.parse(await page.locator('body').innerText());
  assert.equal(descriptor.service, 'gha-clone-server');
  assert.equal(descriptor.endpoints.plan, 'POST /v1/plans');
  assert.match(descriptor.purpose, /ARC parity/i);

  const api = await playwrightRequest.newContext({ baseURL });
  t.after(() => api.dispose());

  const health = await api.get('/healthz');
  assert.equal(health.status(), 200);
  assert.deepEqual(
    {
      executionEnabled: (await health.json()).executionEnabled,
      allowedRepositories: (await (await api.get('/healthz')).json()).allowedRepositories,
    },
    { executionEnabled: false, allowedRepositories: 1 },
  );

  const readiness = await api.get('/readyz');
  assert.equal(readiness.status(), 200);
  assert.equal((await readiness.json()).executionReady, true);

  const capabilities = await api.get('/v1/capabilities');
  assert.equal(capabilities.status(), 200);
  const capabilityBody = await capabilities.json();
  assert.equal(capabilityBody.planSchemaVersion, 'gha-clone-plan.v1');
  assert.match(capabilityBody.architecture.nativeParityLane, /Runner Controller/);
  assert.match(capabilityBody.architecture.independentLane, /fixed dd-build-server profiles/);

  const unauthenticated = await api.post('/v1/plans', {
    data: planRequest(workflowYaml),
  });
  assert.equal(unauthenticated.status(), 401);
  assert.equal((await unauthenticated.json()).error, 'unauthorized');

  const authorized = await api.post('/v1/plans', {
    headers: { 'x-server-auth': AUTH_SECRET },
    data: planRequest(workflowYaml),
  });
  assert.equal(authorized.status(), 200);
  const plan = await authorized.json();
  assert.equal(plan.repository, REPOSITORY);
  assert.equal(plan.revision, REVISION);
  assert.equal(plan.immutableRevision, true);
  assert.equal(plan.arcFullyCovered, true);
  assert.equal(plan.independentExecutable, true);
  assert.deepEqual(plan.topologicalOrder, ['rust', 'node']);
  assert.equal(plan.jobs[0].independentProfile, 'rust-verify');
  assert.equal(plan.jobs[1].independentProfile, 'node-verify');

  const mutable = await api.post('/v1/plans', {
    headers: { 'x-server-auth': AUTH_SECRET },
    data: planRequest(workflowYaml, 'main'),
  });
  assert.equal(mutable.status(), 200);
  const mutablePlan = await mutable.json();
  assert.equal(mutablePlan.immutableRevision, false);
  assert.equal(mutablePlan.independentExecutable, false);
  assert.match(mutablePlan.warnings.join('\n'), /40-hex commit SHA/);

  const disabledRun = await api.post('/v1/runs', {
    headers: { 'x-server-auth': AUTH_SECRET },
    data: planRequest(workflowYaml),
  });
  assert.equal(disabledRun.status(), 503);
  const disabledBody = await disabledRun.text();
  assert.match(disabledBody, /independent execution is disabled/);
  assert(!disabledBody.includes(AUTH_SECRET));

  const missingRun = await api.get('/v1/runs/00000000-0000-0000-0000-000000000000', {
    headers: { 'x-server-auth': AUTH_SECRET },
  });
  assert.equal(missingRun.status(), 404);
  assert(!((await missingRun.text()).includes(AUTH_SECRET)));

  const wrongMethod = await api.post('/healthz', { data: { auth: AUTH_SECRET } });
  assert.equal(wrongMethod.status(), 405);
  assert(!((await wrongMethod.text()).includes(AUTH_SECRET)));
});
