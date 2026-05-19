import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { existsSync } from 'node:fs';
import test from 'node:test';
import { promisify } from 'node:util';

const execFileAsync = promisify(execFile);

const baseUrl = (process.env.REMOTE_DEV_BASE_URL ?? 'https://54.91.17.58').replace(/\/+$/, '');
const serverSecret = process.env.REMOTE_DEV_SERVER_SECRET ?? 'dd-k8s-home';
const sshHost = process.env.REMOTE_DEV_EC2_HOST ?? '54.91.17.58';
const sshUser = process.env.REMOTE_DEV_EC2_USER ?? 'ec2-user';
const sshKeyPath =
  process.env.REMOTE_DEV_EC2_KEY_PATH ?? '/Users/maca5/Downloads/main-key-pair.pem';
const k8sNamespace = process.env.REMOTE_DEV_K8S_NAMESPACE ?? 'default';
const k8sDeployment = process.env.REMOTE_DEV_K8S_DEPLOYMENT ?? 'dd-dev-server-api';
const canRunK8sLifecycle = existsSync(sshKeyPath);

type SseEvent = {
  id: number | null;
  event: string;
  data: JsonValue;
};

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

function hasEventKind(event: SseEvent, kind: string): event is SseEvent & { data: JsonObject } {
  return isJsonObject(event.data) && event.data.kind === kind;
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

async function sleep(ms: number): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, ms));
}

async function fetchText(
  path: string,
  init?: RequestInit,
  timeoutMs = 30_000,
): Promise<{ status: number; body: string; headers: Headers }> {
  const response = await fetch(`${baseUrl}${path}`, {
    ...init,
    signal: AbortSignal.timeout(timeoutMs),
  });
  return {
    status: response.status,
    body: await response.text(),
    headers: response.headers,
  };
}

async function fetchJson(
  path: string,
  init?: RequestInit,
  timeoutMs = 30_000,
): Promise<{ status: number; body: JsonBody; headers: Headers }> {
  const response = await fetch(`${baseUrl}${path}`, {
    ...init,
    signal: AbortSignal.timeout(timeoutMs),
  });
  const text = await response.text();
  const body = parseJsonBody(text);
  return {
    status: response.status,
    body,
    headers: response.headers,
  };
}

async function waitFor<T>(
  label: string,
  fn: () => Promise<T | null>,
  timeoutMs = 180_000,
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

async function dispatchTask(
  taskId: string,
  threadId: string,
  prompt: string,
): Promise<RemoteTaskRow> {
  const response = await fetchJson('/tasks', {
    method: 'POST',
    headers: authHeaders({ 'Content-Type': 'application/json' }),
    body: JSON.stringify({
      taskId,
      threadId,
      prompt,
    }),
  });
  assert.equal(response.status, 200, `dispatch failed: ${JSON.stringify(response.body)}`);
  assert.equal(response.body?.taskId, taskId, 'dispatch returned unexpected task id');
  return response.body;
}

async function collectSseUntilDone(taskId: string, timeoutMs = 240_000): Promise<SseEvent[]> {
  const response = await fetch(`${baseUrl}/stream/${encodeURIComponent(taskId)}`, {
    headers: authHeaders(),
    signal: AbortSignal.timeout(timeoutMs),
  });
  assert.equal(response.status, 200, `stream should be 200, got ${response.status}`);
  assert.ok(response.body, 'stream response missing body');

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  const events: SseEvent[] = [];
  let buffer = '';

  while (true) {
    const { done, value } = await reader.read();
    if (done) {
      break;
    }
    buffer += decoder.decode(value, { stream: true }).replaceAll('\r', '');

    for (;;) {
      const boundary = buffer.indexOf('\n\n');
      if (boundary < 0) {
        break;
      }
      const raw = buffer.slice(0, boundary);
      buffer = buffer.slice(boundary + 2);

      if (!raw || raw.startsWith(':')) {
        continue;
      }

      let eventName = 'message';
      let eventId: number | null = null;
      const dataLines: string[] = [];

      for (const line of raw.split('\n')) {
        if (line.startsWith('event:')) {
          eventName = line.slice(6).trim();
          continue;
        }
        if (line.startsWith('id:')) {
          const parsedId = Number(line.slice(3).trim());
          eventId = Number.isFinite(parsedId) ? parsedId : null;
          continue;
        }
        if (line.startsWith('data:')) {
          dataLines.push(line.slice(5).trimStart());
        }
      }

      const dataText = dataLines.join('\n');
      let data: JsonValue = dataText;
      try {
        data = dataText ? (JSON.parse(dataText) as JsonValue) : null;
      } catch {
        data = dataText;
      }

      const event: SseEvent = { id: eventId, event: eventName, data };
      events.push(event);

      if (isJsonObject(data) && data.kind === 'done') {
        await reader.cancel().catch(() => undefined);
        return events;
      }
    }
  }

  return events;
}

async function waitForTaskSnapshot(taskId: string): Promise<RemoteTaskRow> {
  return await waitFor(
    `task ${taskId} in /tasks`,
    async () => {
      const snapshot = await fetchJson('/tasks', { headers: authHeaders() });
      const tasks = taskRowsFromBody(snapshot.body);
      if (snapshot.status !== 200 || tasks === null) {
        return null;
      }
      const found = tasks.find((row) => row.taskId === taskId);
      return found ?? null;
    },
    120_000,
    2_000,
  );
}

async function cancelTask(taskId: string): Promise<void> {
  await fetchJson(`/tasks/${encodeURIComponent(taskId)}/cancel`, {
    method: 'POST',
    headers: authHeaders(),
  }).catch(() => undefined);
}

test(
  'remote dev cli deep lifecycle: routing, stream, reuse, duplicate, sleep/wake',
  { timeout: 15 * 60_000 },
  async (t) => {
    console.log(`[cli-deep] base=${baseUrl}`);
    const createdTaskIds: string[] = [];

    const healthBefore = await fetchJson('/healthz').catch((error) => {
      const message = error instanceof Error ? error.message : String(error);
      console.log(`[cli-deep] skipping live remote test: healthz fetch failed: ${message}`);
      return null;
    });
    if (!healthBefore || healthBefore.status !== 200 || healthBefore.body?.ok !== true) {
      const status = healthBefore?.status ?? 'fetch-error';
      console.log(`[cli-deep] skipping live remote test: healthz unavailable (${status})`);
      t.skip(`remote healthz unavailable: ${status}`);
      return;
    }

    const root = await fetchText('/', { redirect: 'manual' });
    assert.equal(root.status, 302, `GET / expected 302, got ${root.status}`);
    assert.equal(root.headers.get('location'), '/home');

    const home = await fetchText('/home');
    assert.equal(home.status, 200, `GET /home expected 200, got ${home.status}`);
    assert.match(
      home.body,
      /dd remote service directory/i,
      'home page should include server title',
    );

    assert.equal(healthBefore.status, 200, 'healthz should be up');
    assert.equal(healthBefore.body?.ok, true, 'healthz ok should be true');
    const beforeInstanceId = String(healthBefore.body?.serverInstanceId ?? '');
    const pinnedThreadId = String(healthBefore.body?.pinnedThreadId ?? '');
    assert.match(beforeInstanceId, /^[0-9a-f-]{36}$/i);
    assert.match(pinnedThreadId, /^[0-9a-f-]{36}$/i);

    const status = await fetchJson('/status', { headers: authHeaders() });
    assert.equal(status.status, 200, `GET /status expected 200, got ${status.status}`);
    assert.equal(status.body?.pinnedThreadId, pinnedThreadId);

    const taskIdA = randomUUID();
    createdTaskIds.push(taskIdA);
    await dispatchTask(
      taskIdA,
      pinnedThreadId,
      'Deep test task A: perform a short no-op repo check and summarize in one sentence.',
    );

    const duplicateA = await fetchJson('/tasks', {
      method: 'POST',
      headers: authHeaders({ 'Content-Type': 'application/json' }),
      body: JSON.stringify({
        taskId: taskIdA,
        threadId: pinnedThreadId,
        prompt: 'Duplicate task id check',
      }),
    });
    assert.equal(duplicateA.status, 409, 'duplicate task id should return 409');
    assert.equal(duplicateA.body?.error, 'task exists');

    const streamA = await collectSseUntilDone(taskIdA);
    assert.ok(streamA.length > 0, 'task A stream should contain events');
    assert.ok(
      streamA.some((event) => hasEventKind(event, 'status')),
      'task A stream should include status events',
    );
    const doneA = streamA.find((event) => hasEventKind(event, 'done'));
    assert.ok(doneA, 'task A stream should include done');
    assert.match(
      String(doneA.data.branch ?? ''),
      /^agent\/k8s\/openai-5\.5\//,
      'done event should include thread branch',
    );
    assert.ok(
      ['completed', 'failed', 'cancelled'].includes(String(doneA.data.exitReason)),
      'done event should contain a recognized exit reason',
    );

    const snapshotA = await waitForTaskSnapshot(taskIdA);
    assert.equal(snapshotA.threadId, pinnedThreadId, 'task A should remain on pinned thread');

    const taskIdB = randomUUID();
    createdTaskIds.push(taskIdB);
    await dispatchTask(
      taskIdB,
      pinnedThreadId,
      'Deep test task B: run a tiny follow-up command and report completion.',
    );
    const streamB = await collectSseUntilDone(taskIdB);
    const doneB = streamB.find((event) => hasEventKind(event, 'done'));
    assert.ok(doneB, 'task B stream should include done');

    const snapshotB = await waitForTaskSnapshot(taskIdB);
    assert.equal(snapshotB.threadId, pinnedThreadId, 'task B should stay on pinned thread');
    assert.equal(
      snapshotA.sessionId,
      snapshotB.sessionId,
      'tasks should reuse same session for same UUID',
    );

    const mismatchThreadId = '00000000-0000-4000-8000-000000000099';
    const mismatch = await fetchJson('/tasks', {
      method: 'POST',
      headers: authHeaders({ 'Content-Type': 'application/json' }),
      body: JSON.stringify({
        taskId: randomUUID(),
        threadId: mismatchThreadId,
        prompt: 'Deep mismatch check',
      }),
    });
    assert.equal(mismatch.status, 409, 'mismatched thread should return 409');
    assert.equal(mismatch.body?.boundThreadId, pinnedThreadId);

    if (!canRunK8sLifecycle) {
      console.log(`[cli-deep] skipping sleep/wake: SSH key not found at ${sshKeyPath}`);
      await Promise.all(createdTaskIds.map((taskId) => cancelTask(taskId)));
      return;
    }

    try {
      console.log('[cli-deep] scaling deployment to 0');
      await scaleDeployment(0);

      await waitFor(
        'health endpoint down after scale=0',
        async () => {
          try {
            const probe = await fetchJson('/healthz', undefined, 4_000);
            return probe.status >= 500 ? true : null;
          } catch {
            return true;
          }
        },
        120_000,
        3_000,
      );

      console.log('[cli-deep] scaling deployment to 1');
      await scaleDeployment(1);
      await rolloutReady();

      const healthAfterWake = await waitFor(
        'health endpoint up after scale=1',
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
      assert.match(afterInstanceId, /^[0-9a-f-]{36}$/i);
      assert.notEqual(
        afterInstanceId,
        beforeInstanceId,
        'instance id should change after sleep/wake redeploy',
      );
    } finally {
      await scaleDeployment(1).catch(() => undefined);
      await rolloutReady().catch(() => undefined);
    }

    const taskIdC = randomUUID();
    createdTaskIds.push(taskIdC);
    await dispatchTask(
      taskIdC,
      pinnedThreadId,
      'Deep test task C after wake: verify task dispatch and stream completion.',
    );
    const streamC = await collectSseUntilDone(taskIdC);
    const doneC = streamC.find((event) => hasEventKind(event, 'done'));
    assert.ok(doneC, 'task C stream should include done');

    await Promise.all(createdTaskIds.map((taskId) => cancelTask(taskId)));
  },
);
