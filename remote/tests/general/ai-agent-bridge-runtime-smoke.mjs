import assert from 'node:assert/strict';
import net from 'node:net';
import { randomUUID } from 'node:crypto';

const httpBase = process.env.AI_BRIDGE_HTTP_URL;
const tcpHost = process.env.AI_BRIDGE_TCP_HOST ?? '127.0.0.1';
const tcpPort = Number(process.env.AI_BRIDGE_TCP_PORT);
const token = process.env.AI_BRIDGE_TOKEN;

assert.ok(httpBase, 'AI_BRIDGE_HTTP_URL is required');
assert.ok(Number.isInteger(tcpPort) && tcpPort > 0, 'AI_BRIDGE_TCP_PORT is required');
assert.ok(token, 'AI_BRIDGE_TOKEN is required');

const authHeaders = {
  authorization: `Bearer ${token}`,
};

async function requestJson(
  path,
  { method = 'GET', body, auth = true, expected = 200, signal } = {},
) {
  const headers = {
    ...(auth ? authHeaders : {}),
    ...(body === undefined ? {} : { 'content-type': 'application/json' }),
  };
  const response = await fetch(new URL(path, httpBase), {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
    signal,
  });
  const text = await response.text();
  let json = null;
  if (text.trim()) {
    try {
      json = JSON.parse(text);
    } catch {
      throw new Error(
        `${method} ${path} returned non-JSON HTTP ${response.status}: ${text.slice(0, 200)}`,
      );
    }
  }
  assert.equal(
    response.status,
    expected,
    `${method} ${path} expected ${expected}, received ${response.status}: ${text.slice(0, 300)}`,
  );
  return json;
}

async function registerAgent(agentKey, kind) {
  const body = await requestJson('/agents/register', {
    method: 'POST',
    body: {
      agent_key: agentKey,
      display_name: `Smoke ${agentKey}`,
      kind,
      meta: {
        capabilities: ['runtime-smoke'],
        source: 'k8s-cluster-ci',
      },
    },
  });
  assert.equal(body.ok, true);
  assert.equal(body.agent.agent_key, agentKey);
}

async function createWorkflow(mode, options = {}) {
  const body = await requestJson('/workflows', {
    method: 'POST',
    body: {
      title: `Kubernetes ${mode} smoke`,
      prompt: `Prove the ${mode} workflow contract without a real provider call.`,
      created_by: 'coordinator',
      mode,
      agent_keys: options.agentKeys,
      ...(options.reviewerAgentKey
        ? { reviewer_agent_key: options.reviewerAgentKey }
        : {}),
      ...(options.workerCount ? { worker_count: options.workerCount } : {}),
      meta: {
        test_run: randomUUID(),
        source: 'k8s-cluster-ci',
      },
    },
  });
  assert.equal(body.ok, true);
  assert.equal(body.workflow.plan.mode, mode);
  return body.workflow;
}

async function submit(workflowId, agentKey, suffix) {
  const body = await requestJson(`/workflows/${workflowId}/submissions`, {
    method: 'POST',
    body: {
      agent_key: agentKey,
      content: `${agentKey} deterministic smoke result ${suffix}`,
      meta: {
        source: 'k8s-cluster-ci',
      },
    },
  });
  assert.equal(body.ok, true);
  return body.workflow;
}

async function exerciseWorkflowModes() {
  let workflow = await createWorkflow('single', {
    agentKeys: ['codex'],
  });
  workflow = await submit(workflow.plan.id, 'codex', 'single');
  assert.equal(workflow.status.stage, 'completed');
  assert.equal(workflow.submissions.length, 1);

  workflow = await createWorkflow('sequential', {
    agentKeys: ['codex', 'claude'],
  });
  const sequentialAssignments = [...workflow.plan.assignments].sort(
    (left, right) => left.phase - right.phase || left.ordinal - right.ordinal,
  );
  for (const [index, assignment] of sequentialAssignments.entries()) {
    workflow = await submit(
      workflow.plan.id,
      assignment.agent_key,
      `sequential-${index}`,
    );
    if (index < sequentialAssignments.length - 1) {
      assert.notEqual(workflow.status.stage, 'completed');
      assert.equal(
        workflow.status.current_agent_key,
        sequentialAssignments[index + 1].agent_key,
      );
    }
  }
  assert.equal(workflow.status.stage, 'completed');

  workflow = await createWorkflow('competitive', {
    agentKeys: ['codex', 'claude'],
  });
  const competitors = workflow.plan.assignments.filter(
    (assignment) => assignment.role === 'worker',
  );
  for (const [index, assignment] of competitors.entries()) {
    workflow = await submit(
      workflow.plan.id,
      assignment.agent_key,
      `competitive-${index}`,
    );
  }
  assert.equal(workflow.status.stage, 'completed');
  assert.equal(workflow.submissions.length, competitors.length);

  workflow = await createWorkflow('consensus', {
    agentKeys: ['codex', 'claude', 'gemini'],
    reviewerAgentKey: 'gemini',
    workerCount: 2,
  });
  const workers = workflow.plan.assignments.filter(
    (assignment) => assignment.role === 'worker',
  );
  const reviewers = workflow.plan.assignments.filter(
    (assignment) => assignment.role === 'reviewer',
  );
  assert.equal(workers.length, 2);
  assert.equal(reviewers.length, 1);
  assert.equal(reviewers[0].agent_key, 'gemini');
  for (const [index, assignment] of workers.entries()) {
    workflow = await submit(
      workflow.plan.id,
      assignment.agent_key,
      `consensus-worker-${index}`,
    );
  }
  assert.equal(workflow.status.stage, 'awaiting_review');
  assert.equal(workflow.status.current_agent_key, 'gemini');
  workflow = await submit(workflow.plan.id, 'gemini', 'consensus-review');
  assert.equal(workflow.status.stage, 'completed');
  assert.equal(workflow.submissions.length, 3);
}

async function waitForSseMarker(channel, marker) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 10_000);
  try {
    const response = await fetch(
      new URL(`/channels/${encodeURIComponent(channel)}/stream`, httpBase),
      {
        headers: authHeaders,
        signal: controller.signal,
      },
    );
    assert.equal(response.status, 200);
    assert.ok(response.body, 'SSE response body is missing');
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffered = '';
    for (;;) {
      const { value, done } = await reader.read();
      if (done) {
        throw new Error('SSE stream ended before the marker arrived');
      }
      buffered += decoder.decode(value, { stream: true });
      const lines = buffered.split('\n');
      buffered = lines.pop() ?? '';
      for (const line of lines) {
        if (!line.startsWith('data:')) continue;
        if (line.includes(marker)) {
          controller.abort();
          return;
        }
      }
    }
  } finally {
    clearTimeout(timer);
  }
}

async function exerciseSse() {
  const channel = `k8s-sse-${Date.now()}`;
  const created = await requestJson('/channels', {
    method: 'POST',
    body: {
      slug: channel,
      topic: 'Kubernetes SSE smoke',
      created_by: 'coordinator',
    },
  });
  assert.equal(created.ok, true);

  const marker = `sse-marker-${randomUUID()}`;
  const pending = waitForSseMarker(channel, marker);
  await new Promise((resolve) => setTimeout(resolve, 250));
  const posted = await requestJson(
    `/channels/${encodeURIComponent(channel)}/messages`,
    {
      method: 'POST',
      body: {
        from: 'coordinator',
        role: 'user',
        content: marker,
        meta: { source: 'k8s-cluster-ci' },
      },
    },
  );
  assert.equal(posted.ok, true);
  await pending;
}

function connectTcp() {
  const socket = net.createConnection({ host: tcpHost, port: tcpPort });
  socket.setEncoding('utf8');
  let buffered = '';
  const waiters = [];
  const queued = [];

  socket.on('data', (chunk) => {
    buffered += chunk;
    for (;;) {
      const newline = buffered.indexOf('\n');
      if (newline === -1) break;
      const line = buffered.slice(0, newline).trim();
      buffered = buffered.slice(newline + 1);
      if (!line) continue;
      const value = JSON.parse(line);
      const waiter = waiters.shift();
      if (waiter) waiter.resolve(value);
      else queued.push(value);
    }
  });
  socket.on('error', (error) => {
    while (waiters.length) waiters.shift().reject(error);
  });

  function readLine(timeoutMs = 5_000) {
    if (queued.length) return Promise.resolve(queued.shift());
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('TCP response timeout')), timeoutMs);
      waiters.push({
        resolve(value) {
          clearTimeout(timer);
          resolve(value);
        },
        reject(error) {
          clearTimeout(timer);
          reject(error);
        },
      });
    });
  }

  async function send(value) {
    socket.write(`${JSON.stringify(value)}\n`);
    return readLine();
  }

  return { socket, readLine, send };
}

async function exerciseTcp() {
  const client = connectTcp();
  await new Promise((resolve, reject) => {
    client.socket.once('connect', resolve);
    client.socket.once('error', reject);
  });
  try {
    const hello = await client.readLine();
    assert.equal(hello.ok, true);
    assert.equal(hello.hello, 'ai-agent-bridge');
    assert.equal(hello.needs_auth, true);

    const ping = await client.send({ op: 'ping' });
    assert.equal(ping.pong, true);

    const unauthorized = await client.send({ op: 'list_channels' });
    assert.equal(unauthorized.error, 'unauthorized');

    const auth = await client.send({ op: 'auth', token });
    assert.equal(auth.ok, true);
    assert.equal(auth.auth.principal, 'operator');
    assert.ok(!JSON.stringify(auth).includes(token));

    const channels = await client.send({ op: 'list_channels' });
    assert.equal(channels.ok, true);
    assert.ok(Array.isArray(channels.channels));
  } finally {
    client.socket.end();
  }
}

async function exerciseLocalLease() {
  const acquired = await requestJson('/file-leases', {
    method: 'POST',
    body: {
      repository: 'ORESoftware/k8s-cluster',
      path: 'remote/tests/general/ai-agent-bridge-runtime-smoke.mjs',
      agent_key: 'codex',
      ttl_ms: 10_000,
      recursive: false,
      purpose: 'kubernetes runtime smoke',
      meta: { source: 'k8s-cluster-ci' },
    },
  });
  assert.equal(acquired.ok, true);
  assert.ok(acquired.lease.id);
  assert.ok(acquired.lease.fencing_token > 0);

  const renewed = await requestJson(
    `/file-leases/${acquired.lease.id}/renew`,
    {
      method: 'POST',
      body: {
        agent_key: 'codex',
        fencing_token: acquired.lease.fencing_token,
        ttl_ms: 10_000,
      },
    },
  );
  assert.equal(renewed.ok, true);
  assert.equal(renewed.lease.fencing_token, acquired.lease.fencing_token);

  const released = await requestJson(
    `/file-leases/${acquired.lease.id}/release`,
    {
      method: 'POST',
      body: {
        agent_key: 'codex',
        fencing_token: acquired.lease.fencing_token,
        ttl_ms: 10_000,
      },
    },
  );
  assert.equal(released.released, true);
}

const health = await requestJson('/healthz', { auth: false });
assert.equal(health.ok, true);
assert.equal(health.service, 'ai-agent-bridge');
const ready = await requestJson('/readyz', { auth: false });
assert.equal(ready.ok, true);

const unauthorized = await requestJson('/agents', {
  auth: false,
  expected: 401,
});
assert.equal(unauthorized.error, 'unauthorized');

const legacyUnauthorized = await requestJson('/claude', {
  method: 'POST',
  auth: false,
  expected: 401,
  body: { from: 'codex', topic: 'smoke', prompt: 'unauthorized' },
});
assert.equal(legacyUnauthorized.error, 'unauthorized');

for (const [agent, kind] of [
  ['coordinator', 'other'],
  ['codex', 'codex'],
  ['claude', 'claude'],
  ['gemini', 'gemini'],
]) {
  await registerAgent(agent, kind);
}

await exerciseWorkflowModes();
await exerciseSse();
await exerciseTcp();
await exerciseLocalLease();

const legacy = await requestJson('/claude', {
  method: 'POST',
  body: {
    from: 'codex',
    topic: 'kubernetes smoke',
    prompt: 'authorized legacy inbox smoke',
  },
});
assert.equal(legacy.queued, true);

console.log(
  JSON.stringify({
    ok: true,
    service: 'ai-agent-bridge',
    transports: ['http', 'sse', 'tcp'],
    workflow_modes: ['single', 'sequential', 'competitive', 'consensus'],
    local_lease_cycle: true,
    legacy_inbox_auth: true,
  }),
);
