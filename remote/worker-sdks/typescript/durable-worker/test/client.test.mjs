import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import test from 'node:test';

import {
  DurableWorkerClient,
  DurableWorkerError,
  LeaseLostError,
  sleep,
} from '../index.mjs';

async function readJson(request) {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  if (chunks.length === 0) return undefined;
  return JSON.parse(Buffer.concat(chunks).toString('utf8'));
}

function sendJson(response, status, body) {
  response.writeHead(status, { 'content-type': 'application/json' });
  response.end(JSON.stringify(body));
}

async function withServer(route, callback) {
  const errors = [];
  const server = createServer(async (request, response) => {
    try {
      const body = await readJson(request);
      await route({ request, response, body });
    } catch (error) {
      errors.push(error);
      if (!response.headersSent) {
        sendJson(response, 500, {
          code: 'mock_server_error',
          message: error.message,
          retryable: false,
        });
      } else {
        response.destroy(error);
      }
    }
  });
  await new Promise((resolveListen, rejectListen) => {
    server.once('error', rejectListen);
    server.listen(0, '127.0.0.1', resolveListen);
  });
  const address = server.address();
  const baseUrl = `http://127.0.0.1:${address.port}`;
  try {
    await callback(baseUrl);
    assert.deepEqual(errors, []);
  } finally {
    await new Promise((resolveClose, rejectClose) => {
      server.close((error) => (error ? rejectClose(error) : resolveClose()));
    });
  }
}

function client(baseUrl, overrides = {}) {
  return new DurableWorkerClient({
    baseUrl,
    authSecret: 'ephemeral-test-secret',
    requestTimeoutMs: 2_000,
    retry: {
      maxRetries: 2,
      initialDelayMs: 1,
      maxDelayMs: 2,
      multiplier: 1,
    },
    randomUUID: () => 'deterministic-chunk-id',
    ...overrides,
  });
}

function workerRecord(body, drain = false) {
  return {
    workerId: body.workerId,
    queues: body.queues ?? [],
    capabilities: body.capabilities ?? [],
    labels: body.labels ?? {},
    slots: body.slots ?? 1,
    ttlMs: body.ttlMs ?? 60_000,
    status: drain ? 'draining' : 'online',
    registeredAtMs: Date.now(),
    lastHeartbeatMs: Date.now(),
  };
}

function assignment(overrides = {}) {
  return {
    runId: 'run-1',
    stepId: 'step-1',
    stepKey: 'task',
    taskType: 'agent:test',
    queue: 'agents',
    input: { value: 41 },
    attempt: 1,
    leaseToken: 'lease-token-1',
    leaseGeneration: 1,
    fencingToken: 1,
    leaseExpiresAtMs: Date.now() + 2_000,
    timeoutMs: 10_000,
    affinityKey: null,
    ...overrides,
  };
}

function mutation(stepId = 'step-1', status = 'running') {
  return { ok: true, runId: 'run-1', stepId, status };
}

test('retries only idempotently bound submissions and preserves worker authentication', async () => {
  const attempts = new Map();
  await withServer(async ({ request, response, body }) => {
    assert.equal(request.headers['x-worker-auth'], 'ephemeral-test-secret');
    assert.equal(request.url, '/api/v1/tasks');
    const key = body.idempotencyKey ?? 'unbound';
    attempts.set(key, (attempts.get(key) ?? 0) + 1);
    if (attempts.get(key) === 1) {
      sendJson(response, 503, {
        code: 'backend_unavailable',
        message: 'synthetic outage',
        retryable: true,
      });
      return;
    }
    sendJson(response, 200, {
      runId: `run-${key}`,
      status: 'pending',
      idempotentReplay: false,
    });
  }, async (baseUrl) => {
    const sdk = client(baseUrl);
    const accepted = await sdk.submitTask({
      idempotencyKey: 'safe-retry',
      taskType: 'agent:test',
      input: {},
    });
    assert.equal(accepted.runId, 'run-safe-retry');
    assert.equal(attempts.get('safe-retry'), 2);

    await assert.rejects(
      sdk.submitTask({ taskType: 'agent:test', input: {} }),
      (error) => {
        assert.ok(error instanceof DurableWorkerError);
        assert.equal(error.status, 503);
        assert.equal(error.retryable, true);
        return true;
      },
    );
    assert.equal(attempts.get('unbound'), 1);
  });
});

test('runs one bounded assignment with automatic heartbeats, progress, completion, and drain', async () => {
  const calls = [];
  let polled = false;
  await withServer(async ({ request, response, body }) => {
    calls.push({ method: request.method, url: request.url, body });
    if (request.url === '/api/v1/workers/register') {
      sendJson(response, 200, workerRecord(body));
      return;
    }
    if (request.url === '/api/v1/workers/sdk-worker/heartbeat') {
      sendJson(response, 200, workerRecord({ workerId: 'sdk-worker' }, body.drain));
      return;
    }
    if (request.url.startsWith('/api/v1/workers/sdk-worker/poll')) {
      assert.equal(polled, false);
      polled = true;
      sendJson(response, 200, { assignment: assignment(), retryAfterMs: 0 });
      return;
    }
    if (request.url === '/api/v1/steps/step-1/start') {
      sendJson(response, 200, mutation());
      return;
    }
    if (request.url === '/api/v1/steps/step-1/heartbeat') {
      sendJson(response, 200, mutation());
      return;
    }
    if (request.url === '/api/v1/steps/step-1/output') {
      assert.equal(body.chunkId, 'checkpoint-1');
      assert.equal(body.chunk, 'halfway');
      assert.equal(body.leaseGeneration, 1);
      sendJson(response, 200, mutation());
      return;
    }
    if (request.url === '/api/v1/steps/step-1/complete') {
      assert.deepEqual(body.result, { answer: 42 });
      sendJson(response, 200, mutation('step-1', 'succeeded'));
      return;
    }
    throw new Error(`unexpected route ${request.method} ${request.url}`);
  }, async (baseUrl) => {
    const sdk = client(baseUrl);
    const summary = await sdk.runWorker({
      workerId: 'sdk-worker',
      queues: ['agents'],
      capabilities: ['llm'],
      slots: 1,
      ttlMs: 10_000,
      workerHeartbeatMs: 25,
      leaseHeartbeatFraction: 0.2,
      maxAssignments: 1,
      handlers: {
        'agent:test': async (input, context) => {
          assert.equal(input.value, 41);
          assert.equal(context.fencingToken, 1);
          await context.progress('halfway', {
            chunkId: 'checkpoint-1',
            stream: 'progress',
          });
          await sleep(450);
          assert.equal(context.signal.aborted, false);
          return { answer: input.value + 1 };
        },
      },
    });

    assert.deepEqual(summary, { accepted: 1, succeeded: 1, failed: 0, leaseLost: 0 });
    assert.ok(calls.some((call) => call.url === '/api/v1/steps/step-1/heartbeat'));
    assert.equal(
      calls.filter((call) => call.url === '/api/v1/steps/step-1/complete').length,
      1,
    );
    assert.equal(
      calls.at(-1).url,
      '/api/v1/workers/sdk-worker/heartbeat',
    );
    assert.equal(calls.at(-1).body.drain, true);
  });
});

test('maps handler failures into one explicit non-retryable failure mutation', async () => {
  const failures = [];
  await withServer(async ({ request, response, body }) => {
    if (request.url === '/api/v1/workers/register') {
      sendJson(response, 200, workerRecord(body));
      return;
    }
    if (request.url === '/api/v1/workers/failing-worker/heartbeat') {
      sendJson(response, 200, workerRecord({ workerId: 'failing-worker' }, body.drain));
      return;
    }
    if (request.url.startsWith('/api/v1/workers/failing-worker/poll')) {
      sendJson(response, 200, {
        assignment: assignment({ taskType: 'agent:fail' }),
        retryAfterMs: 0,
      });
      return;
    }
    if (request.url === '/api/v1/steps/step-1/start') {
      sendJson(response, 200, mutation());
      return;
    }
    if (request.url === '/api/v1/steps/step-1/fail') {
      failures.push(body);
      sendJson(response, 200, mutation('step-1', 'failed'));
      return;
    }
    throw new Error(`unexpected route ${request.method} ${request.url}`);
  }, async (baseUrl) => {
    const sdk = client(baseUrl);
    const error = Object.assign(new Error('input rejected'), {
      code: 'validation_failed',
      retryable: false,
    });
    const summary = await sdk.runWorker({
      workerId: 'failing-worker',
      queues: ['agents'],
      maxAssignments: 1,
      handlers: {
        'agent:fail': async () => {
          throw error;
        },
      },
    });

    assert.deepEqual(summary, { accepted: 1, succeeded: 0, failed: 1, leaseLost: 0 });
    assert.equal(failures.length, 1);
    assert.equal(failures[0].code, 'validation_failed');
    assert.equal(failures[0].retryable, false);
    assert.equal(failures[0].message, 'input rejected');
  });
});

test('aborts the handler and suppresses completion when the lease heartbeat is fenced', async () => {
  let completeCalls = 0;
  let failureCalls = 0;
  const observedErrors = [];
  await withServer(async ({ request, response, body }) => {
    if (request.url === '/api/v1/workers/register') {
      sendJson(response, 200, workerRecord(body));
      return;
    }
    if (request.url === '/api/v1/workers/fenced-worker/heartbeat') {
      sendJson(response, 200, workerRecord({ workerId: 'fenced-worker' }, body.drain));
      return;
    }
    if (request.url.startsWith('/api/v1/workers/fenced-worker/poll')) {
      sendJson(response, 200, {
        assignment: assignment({ taskType: 'agent:fenced', leaseExpiresAtMs: Date.now() + 700 }),
        retryAfterMs: 0,
      });
      return;
    }
    if (request.url === '/api/v1/steps/step-1/start') {
      sendJson(response, 200, mutation());
      return;
    }
    if (request.url === '/api/v1/steps/step-1/heartbeat') {
      sendJson(response, 409, {
        code: 'state_conflict',
        message: 'synthetic stale lease',
        retryable: true,
      });
      return;
    }
    if (request.url === '/api/v1/steps/step-1/complete') {
      completeCalls += 1;
      sendJson(response, 500, { code: 'unexpected', message: 'must not complete', retryable: false });
      return;
    }
    if (request.url === '/api/v1/steps/step-1/fail') {
      failureCalls += 1;
      sendJson(response, 500, { code: 'unexpected', message: 'must not fail', retryable: false });
      return;
    }
    throw new Error(`unexpected route ${request.method} ${request.url}`);
  }, async (baseUrl) => {
    const sdk = client(baseUrl);
    const summary = await sdk.runWorker({
      workerId: 'fenced-worker',
      queues: ['agents'],
      maxAssignments: 1,
      leaseHeartbeatFraction: 0.2,
      handlers: {
        'agent:fenced': async (_input, context) => {
          await new Promise((resolve, reject) => {
            if (context.signal.aborted) {
              reject(context.signal.reason);
              return;
            }
            context.signal.addEventListener(
              'abort',
              () => reject(context.signal.reason),
              { once: true },
            );
          });
        },
      },
      onError(error) {
        observedErrors.push(error);
      },
    });

    assert.deepEqual(summary, { accepted: 1, succeeded: 0, failed: 0, leaseLost: 1 });
    assert.equal(completeCalls, 0);
    assert.equal(failureCalls, 0);
    assert.ok(observedErrors.some((error) => error instanceof LeaseLostError));
  });
});


test('does not retry ambiguous polls or signals and refuses redirects', async () => {
  let pollAttempts = 0;
  const pollClient = client('https://workers.example.test', {
    fetch: async (_url, options) => {
      pollAttempts += 1;
      assert.equal(options.redirect, 'manual');
      throw new Error('connection reset after poll write');
    },
  });
  await assert.rejects(
    pollClient.pollWorker('worker-1', { waitMs: 1 }),
    (error) => error instanceof DurableWorkerError && error.retryable === true,
  );
  assert.equal(pollAttempts, 1);

  let signalAttempts = 0;
  const signalClient = client('https://workers.example.test', {
    fetch: async () => {
      signalAttempts += 1;
      throw new Error('connection reset after signal write');
    },
  });
  await assert.rejects(
    signalClient.signalRun('run-1', 'approval', { approved: true }),
    (error) => error instanceof DurableWorkerError && error.retryable === true,
  );
  assert.equal(signalAttempts, 1);

  let redirectAttempts = 0;
  const redirectClient = client('https://workers.example.test', {
    fetch: async (_url, options) => {
      redirectAttempts += 1;
      assert.equal(options.redirect, 'manual');
      return new Response('', {
        status: 302,
        headers: { location: 'https://untrusted.example.test/steal' },
      });
    },
  });
  await assert.rejects(
    redirectClient.getRun('run-1'),
    (error) => error instanceof DurableWorkerError && error.status === 302,
  );
  assert.equal(redirectAttempts, 1);
});

test('stops the worker loop after an ambiguous poll outcome', async () => {
  let pollAttempts = 0;
  const sdk = client('https://workers.example.test', {
    fetch: async (url, options) => {
      if (url.endsWith('/api/v1/workers/register')) {
        return new Response(JSON.stringify(workerRecord({ workerId: 'safe-worker' })), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      }
      if (url.includes('/api/v1/workers/safe-worker/poll')) {
        pollAttempts += 1;
        throw new Error('lost poll response');
      }
      if (url.endsWith('/api/v1/workers/safe-worker/heartbeat')) {
        return new Response(JSON.stringify(workerRecord({ workerId: 'safe-worker' }, true)), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        });
      }
      throw new Error(`unexpected request ${options.method} ${url}`);
    },
  });

  await assert.rejects(
    sdk.runWorker({
      workerId: 'safe-worker',
      queues: ['default'],
      workerHeartbeatMs: 10_000,
      handlers: {},
    }),
    (error) => error instanceof DurableWorkerError && error.retryable === true,
  );
  assert.equal(pollAttempts, 1);
});

test('rejects credential-bearing base URLs and multiline secrets', () => {
  assert.throws(
    () => client('https://user:pass@workers.example.test'),
    /must not contain credentials/u,
  );
  assert.throws(
    () => client('https://workers.example.test?secret=value'),
    /must not contain credentials/u,
  );
  assert.throws(
    () => client('https://workers.example.test', { authSecret: 'secret\nInjected: value' }),
    /single-line/u,
  );
});
