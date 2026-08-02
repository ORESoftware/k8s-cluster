#!/usr/bin/env node

import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const allowedTools = new Set(['playwright', 'puppeteer', 'selenium']);
const tool = process.env.BROWSER_TEST_TOOL ?? '';
assert.ok(allowedTools.has(tool), `BROWSER_TEST_TOOL must be one of ${[...allowedTools].join(', ')}`);

const serviceDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const serverEntry = resolve(serviceDir, 'dist/server.js');
const authSecret = 'browser-driver-e2e-secret';
const artifactDir = resolve(process.env.BROWSER_DRIVER_ARTIFACT_DIR ?? '.browser-driver-artifacts');
const maxLogBytes = 128 * 1024;

let serviceStdout = '';
let serviceStderr = '';
let lastFixtureHeaders = {};

function appendBounded(current, chunk) {
  const next = `${current}${String(chunk)}`;
  return next.length <= maxLogBytes ? next : next.slice(next.length - maxLogBytes);
}

function listen(server) {
  return new Promise((resolveListen, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      assert.ok(address && typeof address === 'object');
      resolveListen(`http://127.0.0.1:${address.port}`);
    });
  });
}

function closeServer(server) {
  server.closeIdleConnections?.();
  server.closeAllConnections?.();
  return new Promise((resolveClose, reject) => {
    server.close((error) => (error ? reject(error) : resolveClose()));
  });
}

function fixtureHtml() {
  return `<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>browser driver fixture</title></head>
<body>
  <label>Name <input id="name" autocomplete="off"></label>
  <label>Flavor
    <select id="flavor">
      <option value="vanilla">vanilla</option>
      <option value="chocolate">chocolate</option>
    </select>
  </label>
  <button id="apply" type="button">Apply</button>
  <output id="result" data-state="idle">idle</output>
  <script>
    console.log('browser-driver-fixture-ready');
    document.querySelector('#apply').addEventListener('click', () => {
      const name = document.querySelector('#name').value;
      const flavor = document.querySelector('#flavor').value;
      const result = document.querySelector('#result');
      result.textContent = name + ':' + flavor;
      result.dataset.state = 'done';
      document.title = 'done:' + name;
    });
  </script>
</body>
</html>`;
}

const fixtureServer = createServer((request, response) => {
  if (request.method !== 'GET') {
    response.writeHead(405, { allow: 'GET', 'content-type': 'text/plain; charset=utf-8' });
    response.end('method not allowed');
    return;
  }
  const url = new URL(request.url ?? '/', 'http://127.0.0.1');
  if (url.pathname === '/fixture') {
    lastFixtureHeaders = request.headers;
    response.writeHead(200, {
      'cache-control': 'no-store',
      'content-type': 'text/html; charset=utf-8',
      'x-content-type-options': 'nosniff',
    });
    response.end(fixtureHtml());
    return;
  }
  response.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
  response.end('not found');
});

async function pollUntilReady(baseUrl, child) {
  const deadline = Date.now() + 45_000;
  let lastError;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`browser-test-server exited before readiness with ${child.exitCode}`);
    }
    try {
      const response = await fetch(`${baseUrl}/healthz`, { signal: AbortSignal.timeout(1_500) });
      if (response.status === 200) return;
      lastError = new Error(`healthz returned ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 150));
  }
  throw new Error(`browser-test-server readiness timed out: ${String(lastError)}`);
}

async function requestJson(baseUrl, path, options = {}) {
  const response = await fetch(`${baseUrl}${path}`, {
    redirect: 'manual',
    signal: AbortSignal.timeout(60_000),
    ...options,
  });
  const text = await response.text();
  let body;
  try {
    body = JSON.parse(text);
  } catch (error) {
    throw new Error(`${path} returned non-JSON HTTP ${response.status}: ${String(error)}\n${text}`);
  }
  return { status: response.status, body };
}

async function stopChild(child) {
  if (child.exitCode !== null) return;
  child.kill('SIGTERM');
  const result = await Promise.race([
    new Promise((resolveExit) => child.once('exit', (code, signal) => resolveExit({ code, signal }))),
    new Promise((resolveTimeout) => setTimeout(() => resolveTimeout({ timeout: true }), 10_000)),
  ]);
  if ('timeout' in result) {
    child.kill('SIGKILL');
    throw new Error('browser-test-server did not stop after SIGTERM');
  }
  assert.equal(result.signal, null, `server terminated by ${result.signal}`);
  assert.equal(result.code, 0, `server exited with ${result.code}`);
}

function assertScreenshot(screenshot) {
  assert.ok(screenshot.name);
  assert.match(screenshot.contentType, /^image\/(?:jpeg|png)$/);
  assert.ok(Number.isInteger(screenshot.bytes) && screenshot.bytes > 100);
  const decoded = Buffer.from(screenshot.base64, 'base64');
  assert.ok(decoded.length > 100, 'screenshot base64 must decode to image bytes');
}

const fixtureOrigin = await listen(fixtureServer);
const servicePortServer = createServer();
const serviceOrigin = await listen(servicePortServer);
const servicePort = new URL(serviceOrigin).port;
await closeServer(servicePortServer);

const child = spawn(process.execPath, [serverEntry], {
  cwd: serviceDir,
  env: {
    ...process.env,
    HOST: '127.0.0.1',
    PORT: servicePort,
    SERVER_AUTH_SECRET: authSecret,
    BROWSER_TEST_ALLOW_UNAUTHENTICATED: 'false',
    BROWSER_TEST_ALLOW_EVALUATE: 'false',
    BROWSER_TEST_DEFAULT_TOOL: tool,
    BROWSER_TEST_MAX_CONCURRENT: '1',
    BROWSER_TEST_DEFAULT_TIMEOUT_MS: '45000',
    BROWSER_TEST_MAX_TIMEOUT_MS: '60000',
    BROWSER_TEST_STEP_TIMEOUT_MS: '15000',
    BROWSER_TEST_MAX_SCREENSHOT_BYTES: '2000000',
    OTEL_SDK_DISABLED: 'true',
  },
  stdio: ['ignore', 'pipe', 'pipe'],
});
child.stdout.setEncoding('utf8');
child.stderr.setEncoding('utf8');
child.stdout.on('data', (chunk) => {
  serviceStdout = appendBounded(serviceStdout, chunk);
});
child.stderr.on('data', (chunk) => {
  serviceStderr = appendBounded(serviceStderr, chunk);
});

let resultRecord = { tool, status: 'not_started' };
try {
  await pollUntilReady(serviceOrigin, child);

  const requestId = `browser-driver-${tool}`;
  const run = await requestJson(serviceOrigin, '/run', {
    method: 'POST',
    headers: {
      authorization: `Bearer ${authSecret}`,
      'content-type': 'application/json',
    },
    body: JSON.stringify({
      requestId,
      tool,
      url: `${fixtureOrigin}/fixture`,
      viewport: { width: 960, height: 720 },
      userAgent: `dd-browser-driver-e2e/${tool}`,
      extraHeaders: { 'x-browser-driver-e2e': tool },
      captureFinalScreenshot: true,
      failOnConsoleError: true,
      timeoutMs: 45_000,
      steps: [
        { action: 'waitForSelector', selector: '#name', state: 'visible' },
        { action: 'fill', selector: '#name', value: 'Ada' },
        { action: 'select', selector: '#flavor', value: 'chocolate' },
        { action: 'click', selector: '#apply' },
        { action: 'waitForSelector', selector: '#result[data-state="done"]', state: 'visible' },
        { action: 'extractText', selector: '#result', name: 'result' },
        { action: 'extractAttribute', selector: '#result', attribute: 'data-state', name: 'state' },
        { action: 'screenshot', name: 'interaction' },
      ],
    }),
  });

  assert.equal(run.status, 200, JSON.stringify(run.body, null, 2));
  assert.equal(run.body.ok, true);
  assert.equal(run.body.requestId, requestId);
  assert.equal(run.body.tool, tool);
  assert.equal(run.body.finalTitle, 'done:Ada');
  assert.equal(run.body.extracted.result, 'Ada:chocolate');
  assert.equal(run.body.extracted.state, 'done');
  assert.equal(run.body.steps.length, 8);
  assert.ok(run.body.steps.every((step) => step.status === 'ok'));
  assert.equal(run.body.pageErrors.length, 0);
  assert.ok(run.body.screenshots.length >= 2, 'step and final screenshots must both be returned');
  for (const screenshot of run.body.screenshots) assertScreenshot(screenshot);
  assert.match(lastFixtureHeaders['user-agent'] ?? '', new RegExp(`dd-browser-driver-e2e/${tool}`));

  if (tool !== 'selenium') {
    assert.equal(lastFixtureHeaders['x-browser-driver-e2e'], tool);
    assert.ok(
      run.body.consoleEntries.some((entry) => entry.text.includes('browser-driver-fixture-ready')),
      `${tool} must return the fixture console entry`,
    );
  }

  const metricsResponse = await fetch(`${serviceOrigin}/metrics`, {
    signal: AbortSignal.timeout(5_000),
  });
  assert.equal(metricsResponse.status, 200);
  const metrics = await metricsResponse.text();
  assert.match(metrics, new RegExp(`browser_test_runs_total\\{tool="${tool}",status="ok"\\} 1`));
  assert.match(metrics, /browser_test_in_flight 0/);

  resultRecord = {
    tool,
    status: 'passed',
    requestId,
    durationMs: run.body.durationMs,
    screenshots: run.body.screenshots.map(({ name, contentType, bytes, truncated = false }) => ({
      name,
      contentType,
      bytes,
      truncated,
    })),
    stepCount: run.body.steps.length,
  };
  console.log(`browser-test real driver smoke passed for ${tool}`);
} catch (error) {
  resultRecord = {
    tool,
    status: 'failed',
    error: error instanceof Error ? error.stack ?? error.message : String(error),
  };
  throw error;
} finally {
  await mkdir(artifactDir, { recursive: true });
  await Promise.all([
    writeFile(resolve(artifactDir, `${tool}-result.json`), `${JSON.stringify(resultRecord, null, 2)}\n`, 'utf8'),
    writeFile(resolve(artifactDir, `${tool}-service.stdout.log`), serviceStdout, 'utf8'),
    writeFile(resolve(artifactDir, `${tool}-service.stderr.log`), serviceStderr, 'utf8'),
  ]);
  await Promise.allSettled([stopChild(child), closeServer(fixtureServer)]);
}
