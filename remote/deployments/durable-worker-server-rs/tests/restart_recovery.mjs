import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';

const baseUrl = process.env.DURABLE_WORKER_URL ?? 'http://127.0.0.1:8152';
const secret = process.env.DURABLE_WORKER_AUTH_SECRET;
const phase = process.argv[2];
const statePath = process.argv[3];
const workerId = 'gha-restart-recovery-worker';

if (!secret) throw new Error('DURABLE_WORKER_AUTH_SECRET is required');
if (!['prepare', 'verify'].includes(phase) || !statePath) {
  throw new Error('usage: node restart_recovery.mjs <prepare|verify> <state-file>');
}

const taskRequest = {
  idempotencyKey: 'gha-durable-restart-recovery-v1',
  name: 'restart and redelivery recovery',
  taskType: 'agent:restart-recovery',
  queue: 'restart-recovery',
  input: { proof: 'jetstream-state-survives-process-and-broker-restart' },
  metadata: { source: 'github-actions', scenario: 'restart-recovery' },
  requiredCapabilities: ['restart-recovery'],
  retry: {
    maxAttempts: 2,
    initialBackoffMs: 100,
    maxBackoffMs: 100,
    multiplier: 1,
  },
  timeoutMs: 90000,
  leaseMs: 30000,
  concurrency: { key: 'gha:restart-recovery', limit: 1 },
};

function headers(extra = {}) {
  return {
    'content-type': 'application/json',
    'x-worker-auth': secret,
    ...extra,
  };
}

async function responseBody(response) {
  const contentType = response.headers.get('content-type') ?? '';
  if (contentType.includes('json')) return response.json();
  return response.text();
}

async function rawRequest(path, options = {}) {
  const response = await fetch(`${baseUrl}${path}`, {
    ...options,
    headers: headers(options.headers ?? {}),
  });
  return { response, body: await responseBody(response) };
}

async function request(path, options = {}) {
  const { response, body } = await rawRequest(path, options);
  if (!response.ok) {
    throw new Error(
      `${options.method ?? 'GET'} ${path} -> ${response.status}: ${JSON.stringify(body)}`,
    );
  }
  return body;
}

function leaseFrom(assignment) {
  return {
    workerId,
    leaseToken: assignment.leaseToken,
    leaseGeneration: assignment.leaseGeneration,
  };
}

async function registerWorker() {
  return request('/api/v1/workers/register', {
    method: 'POST',
    body: JSON.stringify({
      workerId,
      queues: ['restart-recovery'],
      capabilities: ['restart-recovery'],
      labels: { runtime: 'nodejs', scenario: 'broker-and-server-restart' },
      slots: 1,
      ttlMs: 180000,
    }),
  });
}

async function pollUntilAssigned(deadlineMs) {
  const path = `/api/v1/workers/${encodeURIComponent(workerId)}/poll?waitMs=2000`;
  while (Date.now() < deadlineMs) {
    const polled = await request(path, { method: 'POST' });
    if (polled.assignment) return polled.assignment;
    await new Promise((resolveDelay) =>
      setTimeout(resolveDelay, Math.max(50, Math.min(polled.retryAfterMs ?? 100, 500))),
    );
  }
  throw new Error('timed out waiting for the recovered assignment');
}

function assertRunningMutation(mutation, runId, stepId) {
  assert.equal(mutation.ok, true);
  assert.equal(mutation.runId, runId);
  assert.equal(mutation.stepId, stepId);
  assert.equal(mutation.status, 'running');
}

async function prepare() {
  await registerWorker();
  const submitted = await request('/api/v1/tasks', {
    method: 'POST',
    body: JSON.stringify(taskRequest),
  });
  assert.ok(submitted.runId);
  assert.equal(submitted.idempotentReplay, false);

  const assignment = await pollUntilAssigned(Date.now() + 10000);
  assert.equal(assignment.runId, submitted.runId);
  assert.equal(assignment.stepKey, 'task');
  assert.equal(assignment.attempt, 1);
  const lease = leaseFrom(assignment);

  await request(`/api/v1/steps/${assignment.stepId}/start`, {
    method: 'POST',
    body: JSON.stringify(lease),
  });

  const outputBody = {
    ...lease,
    chunkId: 'before-restart-checkpoint',
    stream: 'checkpoint',
    chunk: 'persisted-before-server-and-broker-restart',
    finalChunk: false,
  };
  const outputMutation = await request(`/api/v1/steps/${assignment.stepId}/output`, {
    method: 'POST',
    body: JSON.stringify(outputBody),
  });
  assertRunningMutation(outputMutation, submitted.runId, assignment.stepId);

  const snapshot = await request(`/api/v1/runs/${submitted.runId}`);
  assert.equal(snapshot.run.status, 'running');
  assert.equal(snapshot.steps[0].status, 'running');
  assert.equal(snapshot.steps[0].outputSequence, 1);
  assert.equal(snapshot.steps[0].lastOutput.chunkId, outputBody.chunkId);
  assert.equal(snapshot.steps[0].lease.generation, assignment.leaseGeneration);

  await writeFile(
    statePath,
    `${JSON.stringify({
      submitted,
      assignment,
      lease,
      outputBody,
      outputMutation,
      taskRequest,
    })}\n`,
    'utf8',
  );
  console.log(
    JSON.stringify({
      phase,
      runId: submitted.runId,
      stepId: assignment.stepId,
      leaseGeneration: assignment.leaseGeneration,
      fencingToken: assignment.fencingToken,
    }),
  );
}

async function verify() {
  const state = JSON.parse(await readFile(statePath, 'utf8'));
  const snapshotAfterRestart = await request(`/api/v1/runs/${state.submitted.runId}`);
  assert.equal(snapshotAfterRestart.run.id, state.submitted.runId);
  assert.equal(snapshotAfterRestart.run.status, 'running');
  assert.equal(snapshotAfterRestart.steps[0].id, state.assignment.stepId);
  assert.equal(snapshotAfterRestart.steps[0].status, 'running');
  assert.equal(snapshotAfterRestart.steps[0].outputSequence, 1);
  assert.equal(
    snapshotAfterRestart.steps[0].lastOutput.chunkId,
    state.outputBody.chunkId,
  );
  assert.equal(
    snapshotAfterRestart.steps[0].lease.generation,
    state.assignment.leaseGeneration,
  );

  const replay = await request('/api/v1/tasks', {
    method: 'POST',
    body: JSON.stringify(state.taskRequest),
  });
  assert.equal(replay.runId, state.submitted.runId);
  assert.equal(replay.idempotentReplay, true);

  const outputReplay = await request(`/api/v1/steps/${state.assignment.stepId}/output`, {
    method: 'POST',
    body: JSON.stringify(state.outputBody),
  });
  assert.deepEqual(outputReplay, state.outputMutation);

  const changedOutput = await rawRequest(
    `/api/v1/steps/${state.assignment.stepId}/output`,
    {
      method: 'POST',
      body: JSON.stringify({
        ...state.outputBody,
        chunk: 'same-chunk-id-with-different-payload-must-conflict',
      }),
    },
  );
  assert.equal(changedOutput.response.status, 400);
  assert.equal(changedOutput.body.code, 'invalid_request');
  assert.match(changedOutput.body.message, /chunkId was already used/);

  const heartbeat = await request(
    `/api/v1/workers/${encodeURIComponent(workerId)}/heartbeat`,
    {
      method: 'POST',
      body: JSON.stringify({ drain: false }),
    },
  );
  assert.equal(heartbeat.workerId, workerId);

  const recovered = await pollUntilAssigned(Date.now() + 60000);
  assert.equal(recovered.runId, state.submitted.runId);
  assert.equal(recovered.stepId, state.assignment.stepId);
  assert.equal(recovered.attempt, 2);
  assert.ok(recovered.leaseGeneration > state.assignment.leaseGeneration);
  assert.ok(recovered.fencingToken > state.assignment.fencingToken);
  assert.notEqual(recovered.leaseToken, state.assignment.leaseToken);

  const staleCompletion = await rawRequest(
    `/api/v1/steps/${state.assignment.stepId}/complete`,
    {
      method: 'POST',
      body: JSON.stringify({
        ...state.lease,
        result: { stale: true, mustNotCommit: true },
      }),
    },
  );
  assert.equal(staleCompletion.response.status, 409);
  assert.equal(staleCompletion.body.code, 'state_conflict');

  const recoveredLease = leaseFrom(recovered);
  await request(`/api/v1/steps/${recovered.stepId}/start`, {
    method: 'POST',
    body: JSON.stringify(recoveredLease),
  });
  const recoveredOutput = await request(`/api/v1/steps/${recovered.stepId}/output`, {
    method: 'POST',
    body: JSON.stringify({
      ...recoveredLease,
      chunkId: 'after-restart-checkpoint',
      stream: 'checkpoint',
      chunk: 'completed-under-the-new-fencing-generation',
      finalChunk: true,
    }),
  });
  assertRunningMutation(recoveredOutput, state.submitted.runId, recovered.stepId);
  await request(`/api/v1/steps/${recovered.stepId}/complete`, {
    method: 'POST',
    body: JSON.stringify({
      ...recoveredLease,
      result: {
        recovered: true,
        previousGeneration: state.assignment.leaseGeneration,
        finalGeneration: recovered.leaseGeneration,
      },
    }),
  });

  const finalSnapshot = await request(`/api/v1/runs/${state.submitted.runId}`);
  assert.equal(finalSnapshot.run.status, 'succeeded');
  assert.equal(finalSnapshot.run.counts.succeeded, 1);
  assert.equal(finalSnapshot.steps[0].status, 'succeeded');
  assert.equal(finalSnapshot.steps[0].attempt, 2);
  assert.equal(finalSnapshot.steps[0].outputSequence, 2);

  const metrics = await request('/metrics', { headers: {} });
  assert.match(metrics, /dd_durable_lease_expirations_total [1-9][0-9]*/);
  assert.match(metrics, /dd_durable_idempotent_replays_total [1-9][0-9]*/);

  console.log(
    JSON.stringify({
      phase,
      runId: finalSnapshot.run.id,
      status: finalSnapshot.run.status,
      oldGeneration: state.assignment.leaseGeneration,
      newGeneration: recovered.leaseGeneration,
      oldFencingToken: state.assignment.fencingToken,
      newFencingToken: recovered.fencingToken,
    }),
  );
}

if (phase === 'prepare') {
  await prepare();
} else {
  await verify();
}
