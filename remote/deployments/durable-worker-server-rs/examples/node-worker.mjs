const server = process.env.DURABLE_WORKER_URL ?? 'http://127.0.0.1:8152';
const secret = process.env.DURABLE_WORKER_AUTH_SECRET;
const workerId = process.env.WORKER_ID ?? `node-${process.pid}`;

if (!secret) throw new Error('DURABLE_WORKER_AUTH_SECRET is required');

async function call(path, options = {}) {
  const response = await fetch(`${server}${path}`, {
    ...options,
    headers: {
      'content-type': 'application/json',
      'x-worker-auth': secret,
      ...(options.headers ?? {}),
    },
  });
  const body = await response.json();
  if (!response.ok) throw new Error(`${response.status}: ${JSON.stringify(body)}`);
  return body;
}

await call('/api/v1/workers/register', {
  method: 'POST',
  body: JSON.stringify({
    workerId,
    queues: ['agents'],
    capabilities: ['nodejs', 'llm'],
    labels: { runtime: process.version },
    slots: 2,
    ttlMs: 45000,
  }),
});

for (;;) {
  const { assignment, retryAfterMs } = await call(
    `/api/v1/workers/${encodeURIComponent(workerId)}/poll?waitMs=30000`,
    { method: 'POST' },
  );
  if (!assignment) {
    await new Promise((resolve) => setTimeout(resolve, retryAfterMs));
    continue;
  }

  const lease = {
    workerId,
    leaseToken: assignment.leaseToken,
    leaseGeneration: assignment.leaseGeneration,
  };
  try {
    await call(`/api/v1/steps/${assignment.stepId}/start`, {
      method: 'POST',
      body: JSON.stringify(lease),
    });

    // Replace this with the real Node.js agent task dispatch.
    const result = { echoedInput: assignment.input, taskType: assignment.taskType };

    await call(`/api/v1/steps/${assignment.stepId}/complete`, {
      method: 'POST',
      body: JSON.stringify({ ...lease, result }),
    });
  } catch (error) {
    await call(`/api/v1/steps/${assignment.stepId}/fail`, {
      method: 'POST',
      body: JSON.stringify({
        ...lease,
        code: 'worker_error',
        message: error instanceof Error ? error.message : String(error),
        retryable: true,
      }),
    }).catch(() => {});
  }
}
