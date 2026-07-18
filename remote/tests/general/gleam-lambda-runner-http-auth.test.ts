import assert from 'node:assert/strict';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { existsSync } from 'node:fs';
import { createServer } from 'node:net';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/gleam-lambda-runner/gleam.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const runnerCwd = resolve(repoRoot, 'remote/deployments/gleam-lambda-runner');

function sleep(ms: number): Promise<void> {
  return new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
}

async function openPort(): Promise<number> {
  return new Promise((resolvePort, reject) => {
    const server = createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      assert.ok(address && typeof address === 'object');
      const port = address.port;
      server.close((error) => {
        if (error) {
          reject(error);
        } else {
          resolvePort(port);
        }
      });
    });
  });
}

async function fetchJson(port: number, path: string, init?: RequestInit) {
  const response = await fetch(`http://127.0.0.1:${port}${path}`, init);
  const text = await response.text();
  let body: unknown = text;
  try {
    body = JSON.parse(text);
  } catch {
    // Metrics and error pages may be plain text; keep the raw text.
  }
  return { status: response.status, body, headers: response.headers };
}

function runnerEnv(port: number, overrides: Record<string, string> = {}): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {
    ...process.env,
    HOST: '127.0.0.1',
    PORT: String(port),
    NATS_URL: '',
    LAMBDA_DATABASE_URL: '',
    ...overrides,
  };
  for (const key of ['LAMBDA_SERVER_AUTH_SECRET', 'SERVER_AUTH_SECRET', 'REMOTE_DEV_SERVER_SECRET']) {
    if (!(key in overrides)) {
      delete env[key];
    }
  }
  return env;
}

async function stopRunner(processHandle: ChildProcessWithoutNullStreams): Promise<void> {
  if (processHandle.exitCode !== null || processHandle.signalCode !== null) {
    return;
  }
  const pid = processHandle.pid;
  if (!pid) {
    return;
  }
  try {
    process.kill(-pid, 'SIGTERM');
  } catch {
    processHandle.kill('SIGTERM');
  }
  await Promise.race([
    new Promise((resolveExit) => processHandle.once('exit', resolveExit)),
    sleep(5_000).then(() => {
      try {
        process.kill(-pid, 'SIGKILL');
      } catch {
        processHandle.kill('SIGKILL');
      }
    }),
  ]);
}

async function withRunner(
  overrides: Record<string, string>,
  callback: (port: number) => Promise<void>,
): Promise<void> {
  const port = await openPort();
  const processHandle = spawn('gleam', ['run'], {
    cwd: runnerCwd,
    detached: true,
    env: runnerEnv(port, overrides),
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = '';
  processHandle.stdout.on('data', (chunk) => {
    output += chunk.toString();
  });
  processHandle.stderr.on('data', (chunk) => {
    output += chunk.toString();
  });

  try {
    const startedAt = Date.now();
    let lastHealthError = 'health endpoint never returned a non-200 response';
    while (Date.now() - startedAt < 30_000) {
      assert.equal(processHandle.exitCode, null, `runner exited early:\n${output}`);
      let healthy = false;
      try {
        const health = await fetchJson(port, '/healthz');
        if (health.status === 200) {
          healthy = true;
        } else {
          lastHealthError = `health endpoint returned HTTP ${health.status}`;
        }
      } catch (error) {
        lastHealthError = error instanceof Error ? `${error.name}: ${error.message}` : String(error);
      }
      if (healthy) {
        await callback(port);
        return;
      }
      await sleep(250);
    }
    assert.fail(`runner did not become healthy (${lastHealthError}):\n${output}`);
  } finally {
    await stopRunner(processHandle);
  }
}

test('gleam lambda runner direct HTTP routes fail closed without shared auth', { timeout: 60_000 }, async () => {
  await withRunner({}, async (port) => {
    const health = await fetchJson(port, '/healthz');
    assert.equal(health.status, 200);
    assert.deepEqual(health.body, {
      ok: true,
      service: 'dd-gleam-lambda-runner',
      authConfigured: false,
      postgresConfigured: false,
      natsConfigured: false,
      workflowEngineEnabled: false,
    });

    const invoke = await fetchJson(port, '/invoke/00000000-0000-0000-0000-000000000000', {
      method: 'POST',
      body: '{}',
      headers: { 'content-type': 'application/json' },
    });
    assert.equal(invoke.status, 503);
    assert.deepEqual(invoke.body, {
      ok: false,
      error: 'SERVER_AUTH_SECRET is not configured',
    });
  });
});

test('gleam lambda runner direct HTTP routes require the shared auth header', { timeout: 60_000 }, async () => {
  const secret = 'gleam-http-auth-test-secret';
  await withRunner({ SERVER_AUTH_SECRET: secret }, async (port) => {
    const health = await fetchJson(port, '/healthz');
    assert.equal(health.status, 200);
    assert.equal((health.body as { authConfigured?: boolean }).authConfigured, true);

    const metrics = await fetchJson(port, '/metrics');
    assert.equal(metrics.status, 200);
    assert.match(String(metrics.body), /dd_lambda_runner_/);

    const getInvoke = await fetchJson(port, '/invoke/00000000-0000-0000-0000-000000000000');
    assert.equal(getInvoke.status, 405);
    assert.equal(getInvoke.headers.get('allow'), 'POST');

    const missing = await fetchJson(port, '/invoke/00000000-0000-0000-0000-000000000000', {
      method: 'POST',
      body: '{}',
      headers: { 'content-type': 'application/json' },
    });
    assert.equal(missing.status, 401);
    assert.deepEqual(missing.body, { ok: false, error: 'unauthorized' });

    const wrong = await fetchJson(port, '/invoke/00000000-0000-0000-0000-000000000000', {
      method: 'POST',
      body: '{}',
      headers: { 'content-type': 'application/json', 'X-Server-Auth': 'wrong' },
    });
    assert.equal(wrong.status, 401);

    const authorized = await fetchJson(port, '/invoke/00000000-0000-0000-0000-000000000000', {
      method: 'POST',
      body: '{}',
      headers: { 'content-type': 'application/json', 'X-Server-Auth': secret },
    });
    assert.notEqual(authorized.status, 401);
    assert.notEqual(authorized.status, 503);
    assert.equal((authorized.body as { ok?: boolean }).ok, false);
  });
});

test('gleam lambda runner check route keeps child stderr out of protocol output', { timeout: 60_000 }, async () => {
  const secret = 'gleam-http-check-stderr-test-secret';
  const cases = [
    {
      runtime: 'python3',
      functionBody: 'if :\n  result = 1',
    },
    {
      runtime: 'ruby',
      functionBody: 'def broken(',
    },
  ];

  await withRunner(
    {
      SERVER_AUTH_SECRET: secret,
      LAMBDA_ALLOW_HOST_RUNTIMES: 'nodejs,python3,ruby,bash',
    },
    async (port) => {
      for (const testCase of cases) {
        const checked = await fetchJson(port, '/check', {
          method: 'POST',
          body: JSON.stringify({
            slug: `invalid-${testCase.runtime}`,
            runtime: testCase.runtime,
            functionBody: testCase.functionBody,
            containerized: false,
            status: 'draft',
          }),
          headers: {
            'content-type': 'application/json',
            'X-Server-Auth': secret,
          },
        });

        assert.equal(checked.status, 422, `${testCase.runtime} invalid check should use HTTP 422`);
        assert.equal((checked.body as { ok?: boolean }).ok, false);
        assert.equal(typeof (checked.body as { error?: string }).error, 'string');
      }
    },
  );
});
