import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { existsSync } from 'node:fs';
import { promisify } from 'node:util';
import test from 'node:test';

const execFileAsync = promisify(execFile);

const baseUrl = (process.env.REMOTE_DEV_BASE_URL ?? 'http://54.91.17.58').replace(/\/+$/, '');
const serverSecret = process.env.REMOTE_DEV_SERVER_SECRET ?? 'dd-k8s-home';
const sshHost = process.env.REMOTE_DEV_EC2_HOST ?? '54.91.17.58';
const sshUser = process.env.REMOTE_DEV_EC2_USER ?? 'ec2-user';
const sshKeyPath =
  process.env.REMOTE_DEV_EC2_KEY_PATH ?? '/Users/maca5/Downloads/main-key-pair.pem';
const k8sNamespace = process.env.REMOTE_DEV_K8S_NAMESPACE ?? 'default';
const k8sDeployment = process.env.REMOTE_DEV_K8S_DEPLOYMENT ?? 'dd-dev-server-api';
const canRunK8sLifecycle = existsSync(sshKeyPath);

type JsonPrimitive = string | number | boolean | null;
type JsonObject = { [key: string]: JsonValue | undefined };
type JsonValue = JsonPrimitive | JsonObject | JsonValue[];
type RemoteTaskRow = JsonObject & {
  taskId?: string | number | null;
  threadId?: string | number | null;
};
type JsonBody = JsonObject & {
  boundThreadId?: string | number | null;
  error?: string;
  ok?: boolean;
  pinnedThreadId?: string | number | null;
  raw?: JsonValue;
  serverInstanceId?: string | number | null;
  taskId?: string | number | null;
  tasks?: RemoteTaskRow[];
};

function isJsonObject(value: JsonValue | undefined): value is JsonObject {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function parseJsonBody(text: string): JsonBody {
  if (!text) {
    return {};
  }
  try {
    const parsed = JSON.parse(text) as JsonValue;
    if (isJsonObject(parsed)) {
      return parsed;
    }
    return { raw: parsed };
  } catch {
    return { raw: text };
  }
}

function taskRowsFromBody(body: JsonBody): RemoteTaskRow[] | null {
  const tasks = body.tasks;
  if (!Array.isArray(tasks)) {
    return null;
  }
  return tasks.filter((row): row is RemoteTaskRow => isJsonObject(row));
}

function authHeaders(extra?: Record<string, string>): Record<string, string> {
  return {
    'X-Server-Auth': serverSecret,
    ...(extra ?? {}),
  };
}

function safeName(value: string, label: string): string {
  if (!/^[a-z0-9-]+$/i.test(value)) {
    throw new Error(`invalid ${label}: ${value}`);
  }
  return value;
}

async function fetchJson(
  path: string,
  init?: RequestInit,
  timeoutMs = 30_000,
): Promise<{ status: number; body: JsonBody }> {
  const response = await fetch(`${baseUrl}${path}`, {
    ...init,
    signal: AbortSignal.timeout(timeoutMs),
  });
  const text = await response.text();
  const body = parseJsonBody(text);
  return { status: response.status, body };
}

async function sleep(ms: number): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitFor<T>(
  label: string,
  fn: () => Promise<T | null>,
  timeoutMs = 120_000,
  everyMs = 2_500,
): Promise<T> {
  const started = Date.now();
  for (;;) {
    const value = await fn();
    if (value !== null) {
      return value;
    }
    if (Date.now() - started > timeoutMs) {
      throw new Error(`timed out waiting for ${label}`);
    }
    await sleep(everyMs);
  }
}

async function sshRun(command: string): Promise<string> {
  const target = `${sshUser}@${sshHost}`;
  const { stdout, stderr } = await execFileAsync('ssh', [
    '-i',
    sshKeyPath,
    '-o',
    'StrictHostKeyChecking=accept-new',
    '-o',
    'BatchMode=yes',
    '-o',
    'ConnectTimeout=20',
    target,
    command,
  ]);
  return `${stdout}${stderr}`.trim();
}

async function scaleDeployment(replicas: 0 | 1): Promise<void> {
  const ns = safeName(k8sNamespace, 'k8s namespace');
  const dep = safeName(k8sDeployment, 'k8s deployment');
  await sshRun(`kubectl scale deployment/${dep} --replicas=${replicas} -n ${ns}`);
}

async function rolloutReady(): Promise<void> {
  const ns = safeName(k8sNamespace, 'k8s namespace');
  const dep = safeName(k8sDeployment, 'k8s deployment');
  await sshRun(`kubectl rollout status deployment/${dep} -n ${ns} --timeout=180s`);
}

async function cancelTask(taskId: string): Promise<void> {
  await fetchJson(`/tasks/${encodeURIComponent(taskId)}/cancel`, {
    method: 'POST',
    headers: authHeaders(),
  }).catch(() => undefined);
}

test('remote dev cli: uuid reuse + sleep/wake', async (t) => {
  console.log(`[cli-general] base=${baseUrl}`);
  const createdTaskIds: string[] = [];

  const healthBefore = await fetchJson('/healthz').catch((error) => {
    const message = error instanceof Error ? error.message : String(error);
    console.log(`[cli-general] skipping live remote test: healthz fetch failed: ${message}`);
    return null;
  });
  if (!healthBefore || healthBefore.status !== 200 || healthBefore.body?.ok !== true) {
    const status = healthBefore?.status ?? 'fetch-error';
    console.log(`[cli-general] skipping live remote test: healthz unavailable (${status})`);
    t.skip(`remote healthz unavailable: ${status}`);
    return;
  }

  assert.equal(healthBefore.status, 200, 'healthz should be reachable before test');
  assert.equal(healthBefore.body?.ok, true, 'healthz should report ok=true');
  const beforeInstanceId = String(healthBefore.body?.serverInstanceId ?? '');
  const pinnedThreadId = String(healthBefore.body?.pinnedThreadId ?? '');
  assert.match(beforeInstanceId, /^[0-9a-f-]{36}$/i, 'serverInstanceId should be UUID');
  assert.match(pinnedThreadId, /^[0-9a-f-]{36}$/i, 'pinnedThreadId should be UUID');
  console.log(`[cli-general] pinnedThreadId=${pinnedThreadId}`);

  const firstTaskId = randomUUID();
  createdTaskIds.push(firstTaskId);
  const firstDispatch = await fetchJson('/tasks', {
    method: 'POST',
    headers: authHeaders({ 'Content-Type': 'application/json' }),
    body: JSON.stringify({
      taskId: firstTaskId,
      threadId: pinnedThreadId,
      prompt: 'CLI test run #1: reply with one short status line.',
    }),
  });
  assert.equal(
    firstDispatch.status,
    200,
    `first dispatch failed: ${JSON.stringify(firstDispatch.body)}`,
  );
  assert.equal(firstDispatch.body?.taskId, firstTaskId, 'first dispatch returned wrong task id');

  const secondTaskId = randomUUID();
  createdTaskIds.push(secondTaskId);
  const secondDispatch = await fetchJson('/tasks', {
    method: 'POST',
    headers: authHeaders({ 'Content-Type': 'application/json' }),
    body: JSON.stringify({
      taskId: secondTaskId,
      threadId: pinnedThreadId,
      prompt: 'CLI test run #2: follow-up in same thread session.',
    }),
  });
  assert.equal(
    secondDispatch.status,
    200,
    `second dispatch failed: ${JSON.stringify(secondDispatch.body)}`,
  );
  assert.equal(
    secondDispatch.body?.taskId,
    secondTaskId,
    'second dispatch returned wrong task id',
  );

  const tasksSnapshot = await waitFor(
    'tasks snapshot to include first+second tasks',
    async () => {
      const snapshot = await fetchJson('/tasks', { headers: authHeaders() });
      const tasks = taskRowsFromBody(snapshot.body);
      if (snapshot.status !== 200 || tasks === null) {
        return null;
      }
      const byId = new Map(tasks.map((row) => [String(row.taskId ?? ''), row]));
      const first = byId.get(firstTaskId);
      const second = byId.get(secondTaskId);
      if (!first || !second) {
        return null;
      }
      return { first, second };
    },
    90_000,
    2_000,
  );

  assert.equal(
    tasksSnapshot.first.threadId,
    pinnedThreadId,
    'first task should stay on pinned thread',
  );
  assert.equal(
    tasksSnapshot.second.threadId,
    pinnedThreadId,
    'second task should reuse pinned thread',
  );
  console.log('[cli-general] thread reuse validated with two task dispatches');

  const mismatchThreadId = '00000000-0000-4000-8000-000000000099';
  const mismatchDispatch = await fetchJson('/tasks', {
    method: 'POST',
    headers: authHeaders({ 'Content-Type': 'application/json' }),
    body: JSON.stringify({
      taskId: randomUUID(),
      threadId: mismatchThreadId,
      prompt: 'CLI test mismatch thread check',
    }),
  });
  assert.equal(mismatchDispatch.status, 409, 'mismatched thread should be rejected');
  assert.equal(
    mismatchDispatch.body?.boundThreadId,
    pinnedThreadId,
    'mismatch response should identify bound thread id',
  );
  console.log('[cli-general] mismatched thread UUID rejected as expected');

  if (!canRunK8sLifecycle) {
    console.log(`[cli-general] skipping sleep/wake: SSH key not found at ${sshKeyPath}`);
    await Promise.all(createdTaskIds.map((taskId) => cancelTask(taskId)));
    return;
  }

  try {
    console.log('[cli-general] scaling deployment to 0 (sleep)');
    await scaleDeployment(0);

    await waitFor(
      'service to go down after sleep',
      async () => {
        try {
          const probe = await fetchJson('/healthz', undefined, 4_000);
          if (probe.status >= 500) {
            return true;
          }
          return null;
        } catch {
          return true;
        }
      },
      120_000,
      3_000,
    );
    console.log('[cli-general] container slept (healthz unavailable)');

    console.log('[cli-general] scaling deployment to 1 (wake)');
    await scaleDeployment(1);
    await rolloutReady();

    const healthAfterWake = await waitFor(
      'service to wake and report health',
      async () => {
        const probe = await fetchJson('/healthz', undefined, 8_000).catch(() => null);
        if (!probe || probe.status !== 200 || probe.body?.ok !== true) {
          return null;
        }
        return probe.body;
      },
      180_000,
      3_000,
    );
    const afterInstanceId = String(healthAfterWake.serverInstanceId ?? '');
    assert.match(afterInstanceId, /^[0-9a-f-]{36}$/i, 'post-wake serverInstanceId should be UUID');
    assert.notEqual(
      afterInstanceId,
      beforeInstanceId,
      'server instance id should change after sleep/wake',
    );
    console.log('[cli-general] wake confirmed with new serverInstanceId');
  } finally {
    await scaleDeployment(1).catch(() => undefined);
    await rolloutReady().catch(() => undefined);
  }

  const thirdTaskId = randomUUID();
  createdTaskIds.push(thirdTaskId);
  const thirdDispatch = await fetchJson('/tasks', {
    method: 'POST',
    headers: authHeaders({ 'Content-Type': 'application/json' }),
    body: JSON.stringify({
      taskId: thirdTaskId,
      threadId: pinnedThreadId,
      prompt: 'CLI test run #3 after wake: confirm dispatch works again.',
    }),
  });
  assert.equal(
    thirdDispatch.status,
    200,
    `post-wake dispatch failed: ${JSON.stringify(thirdDispatch.body)}`,
  );
  assert.equal(
    thirdDispatch.body?.taskId,
    thirdTaskId,
    'post-wake dispatch returned wrong task id',
  );
  console.log('[cli-general] post-wake dispatch succeeded');

  await Promise.all(createdTaskIds.map((taskId) => cancelTask(taskId)));
});
