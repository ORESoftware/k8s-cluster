import assert from 'node:assert/strict';

const baseUrl = process.env.DURABLE_WORKER_URL ?? 'http://127.0.0.1:8152';
const secret = process.env.DURABLE_WORKER_AUTH_SECRET ?? '0123456789abcdef-test';

async function request(path, options = {}) {
  const response = await fetch(`${baseUrl}${path}`, {
    ...options,
    headers: {
      'content-type': 'application/json',
      'x-worker-auth': secret,
      ...(options.headers ?? {}),
    },
  });
  const contentType = response.headers.get('content-type') ?? '';
  const body = contentType.includes('json') ? await response.json() : await response.text();
  if (!response.ok) {
    throw new Error(`${options.method ?? 'GET'} ${path} -> ${response.status}: ${JSON.stringify(body)}`);
  }
  return body;
}

const health = await request('/healthz', { headers: {} });
assert.equal(health.status, 'ok');

const unauthorized = await fetch(`${baseUrl}/api/v1/runs/not-a-run`);
assert.equal(unauthorized.status, 401);

const publicOpenApi = await request('/api/docs.json', { headers: {} });
assert.equal(publicOpenApi['x-dd-contract-scope'], 'public');
assert.equal(publicOpenApi.paths['/api/v1/runs'], undefined);

const internalOpenApi = await request('/internal/openapi.json');
assert.equal(internalOpenApi['x-dd-contract-scope'], 'internal');
assert.ok(internalOpenApi.paths['/api/v1/runs']);
assert.ok(internalOpenApi.components.securitySchemes.workerAuth);

await request('/api/v1/workers/register', {
  method: 'POST',
  body: JSON.stringify({
    workerId: 'gha-node-worker',
    queues: ['agents'],
    capabilities: ['llm', 'browser'],
    labels: { runtime: 'nodejs' },
    slots: 2,
    ttlMs: 30000,
  }),
});

const runRequest = {
  idempotencyKey: 'gha-durable-run-v1',
  name: 'research and summarize',
  metadata: { source: 'github-actions' },
  steps: [
    {
      key: 'research',
      taskType: 'agent:research',
      queue: 'agents',
      input: { query: 'durable execution' },
      priority: 50,
      requiredCapabilities: ['browser'],
      retry: { maxAttempts: 3, initialBackoffMs: 250, maxBackoffMs: 2000, multiplier: 2 },
      timeoutMs: 60000,
      leaseMs: 5000,
    },
    {
      key: 'summarize',
      taskType: 'agent:summarize',
      queue: 'agents',
      input: { format: 'markdown' },
      dependsOn: ['research'],
      requiredCapabilities: ['llm'],
      retry: { maxAttempts: 2, initialBackoffMs: 250, maxBackoffMs: 1000, multiplier: 2 },
      timeoutMs: 60000,
      leaseMs: 5000,
      concurrency: { key: 'gha:llm', limit: 1 },
    },
  ],
};

const submitted = await request('/api/v1/runs', {
  method: 'POST',
  body: JSON.stringify(runRequest),
});
assert.ok(submitted.runId);
assert.equal(submitted.idempotentReplay, false);
const replay = await request('/api/v1/runs', {
  method: 'POST',
  body: JSON.stringify(runRequest),
});
assert.equal(replay.runId, submitted.runId);
assert.equal(replay.idempotentReplay, true);

async function poll() {
  const response = await request('/api/v1/workers/gha-node-worker/poll?waitMs=2000', {
    method: 'POST',
  });
  assert.ok(response.assignment, 'expected a worker assignment');
  return response.assignment;
}

async function finish(assignment, result) {
  const lease = {
    workerId: 'gha-node-worker',
    leaseToken: assignment.leaseToken,
    leaseGeneration: assignment.leaseGeneration,
  };
  await request(`/api/v1/steps/${assignment.stepId}/start`, {
    method: 'POST',
    body: JSON.stringify(lease),
  });
  await request(`/api/v1/steps/${assignment.stepId}/output`, {
    method: 'POST',
    body: JSON.stringify({
      ...lease,
      chunkId: `${assignment.stepId}-output-1`,
      stream: 'stdout',
      chunk: `running ${assignment.stepKey}`,
    }),
  });
  await request(`/api/v1/steps/${assignment.stepId}/complete`, {
    method: 'POST',
    body: JSON.stringify({ ...lease, result }),
  });
}

const research = await poll();
assert.equal(research.stepKey, 'research');
await finish(research, { sources: 3 });
const summarize = await poll();
assert.equal(summarize.stepKey, 'summarize');
await finish(summarize, { document: 'done' });

const snapshot = await request(`/api/v1/runs/${submitted.runId}`);
assert.equal(snapshot.run.status, 'succeeded');
assert.equal(snapshot.run.counts.succeeded, 2);
assert.equal(snapshot.steps.length, 2);

console.log(JSON.stringify({ ok: true, runId: submitted.runId, status: snapshot.run.status }));
