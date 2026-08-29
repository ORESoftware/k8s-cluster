#!/usr/bin/env node

import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const serviceDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const serverEntry = resolve(serviceDir, 'dist/server.js');
const publicContractPath = resolve(serviceDir, 'generated/api-docs.json');
const internalContractPath = resolve(serviceDir, 'generated/openapi.json');
const authSecret = 'browser-test-runtime-contract-secret';
const maxLogBytes = 128 * 1024;

function appendBounded(current, chunk) {
  const next = `${current}${String(chunk)}`;
  return next.length <= maxLogBytes ? next : next.slice(next.length - maxLogBytes);
}

async function reservePort() {
  const socket = createServer();
  await new Promise((resolveListen, reject) => {
    socket.once('error', reject);
    socket.listen(0, '127.0.0.1', resolveListen);
  });
  const address = socket.address();
  assert.ok(address && typeof address === 'object');
  const port = address.port;
  await new Promise((resolveClose, reject) => {
    socket.close((error) => (error ? reject(error) : resolveClose()));
  });
  return port;
}

async function pollUntilReady(baseUrl, child, logs) {
  const deadline = Date.now() + 30_000;
  let lastError;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(
        `browser-test-server exited before readiness with code ${child.exitCode}\n${logs()}`,
      );
    }
    try {
      const response = await fetch(`${baseUrl}/healthz`, {
        signal: AbortSignal.timeout(1_500),
      });
      if (response.status === 200) return;
      lastError = new Error(`healthz returned ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 150));
  }
  throw new Error(`browser-test-server readiness timed out: ${String(lastError)}\n${logs()}`);
}

async function request(baseUrl, path, options = {}) {
  const response = await fetch(`${baseUrl}${path}`, {
    redirect: 'manual',
    signal: AbortSignal.timeout(10_000),
    ...options,
  });
  return {
    status: response.status,
    contentType: response.headers.get('content-type') ?? '',
    body: await response.text(),
  };
}

function parseJson(response, label) {
  try {
    return JSON.parse(response.body);
  } catch (error) {
    throw new Error(`${label} did not return JSON: ${String(error)}\n${response.body}`);
  }
}

function assertScalarBootstrap(response, expectedSpecUrl, label) {
  assert.equal(response.status, 200, label);
  assert.match(response.contentType, /text\/html/, label);
  assert.match(response.body, /id=["']api-reference["']/, label);
  assert.match(response.body, /@scalar\/api-reference/, label);
  assert.match(
    response.body,
    new RegExp(`data-url=["']${expectedSpecUrl.replaceAll('/', '\\/')}["']`),
    label,
  );
}

async function assertAuthenticatedAliasParity(baseUrl, canonicalPath, aliasPath, headers) {
  const [canonical, alias] = await Promise.all([
    request(baseUrl, canonicalPath, { headers }),
    request(baseUrl, aliasPath, { headers }),
  ]);
  assert.equal(canonical.status, 200, canonicalPath);
  assert.equal(alias.status, canonical.status, `${aliasPath} status must match ${canonicalPath}`);
  assert.equal(
    alias.contentType,
    canonical.contentType,
    `${aliasPath} content type must match ${canonicalPath}`,
  );
  assert.equal(alias.body, canonical.body, `${aliasPath} body must match ${canonicalPath}`);
  return canonical;
}

async function stopChild(child, logs) {
  if (child.exitCode !== null) return;
  child.kill('SIGTERM');
  const result = await Promise.race([
    new Promise((resolveExit) => child.once('exit', (code, signal) => resolveExit({ code, signal }))),
    new Promise((resolveTimeout) =>
      setTimeout(() => resolveTimeout({ timeout: true }), 10_000),
    ),
  ]);
  if ('timeout' in result) {
    child.kill('SIGKILL');
    throw new Error(`browser-test-server did not stop after SIGTERM\n${logs()}`);
  }
  assert.equal(result.signal, null, `server terminated by ${result.signal}\n${logs()}`);
  assert.equal(result.code, 0, `server exited with ${result.code}\n${logs()}`);
}

const port = await reservePort();
const baseUrl = `http://127.0.0.1:${port}`;
let stdout = '';
let stderr = '';
const child = spawn(process.execPath, [serverEntry], {
  cwd: serviceDir,
  env: {
    ...process.env,
    HOST: '127.0.0.1',
    PORT: String(port),
    SERVER_AUTH_SECRET: authSecret,
    BROWSER_TEST_ALLOW_UNAUTHENTICATED: 'false',
    BROWSER_TEST_ALLOW_EVALUATE: 'false',
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD: '1',
    PUPPETEER_SKIP_DOWNLOAD: 'true',
    OTEL_SDK_DISABLED: 'true',
  },
  stdio: ['ignore', 'pipe', 'pipe'],
});
child.stdout.setEncoding('utf8');
child.stderr.setEncoding('utf8');
child.stdout.on('data', (chunk) => {
  stdout = appendBounded(stdout, chunk);
});
child.stderr.on('data', (chunk) => {
  stderr = appendBounded(stderr, chunk);
});
const logs = () => `--- stdout ---\n${stdout}\n--- stderr ---\n${stderr}`;

try {
  await pollUntilReady(baseUrl, child, logs);

  const [publicContract, internalContract] = await Promise.all([
    readFile(publicContractPath, 'utf8'),
    readFile(internalContractPath, 'utf8'),
  ]);

  for (const path of ['/openapi.json', '/api/docs.json']) {
    const response = await request(baseUrl, path);
    assert.equal(response.status, 200, path);
    assert.match(response.contentType, /openapi\+json|application\/json/, path);
    assert.equal(response.body, publicContract, `${path} must serve committed public bytes`);
  }

  const publicDocument = JSON.parse(publicContract);
  assert.equal(publicDocument['x-dd-contract-scope'], 'public');
  assert.equal(publicDocument.paths['/run'], undefined);
  assert.equal(publicDocument.paths['/internal/openapi.json'], undefined);

  for (const path of ['/docs/api', '/api/docs']) {
    const response = await request(baseUrl, path);
    assertScalarBootstrap(response, '/openapi.json', path);
    assert.doesNotMatch(response.body, /internal\/openapi\.json/, path);
    assert.doesNotMatch(response.body, /runBrowserScenario/, path);
  }

  const health = await request(baseUrl, '/healthz');
  assert.equal(health.status, 200);
  assert.equal(parseJson(health, 'healthz').service, 'dd-browser-test-server');

  const metrics = await request(baseUrl, '/metrics');
  assert.equal(metrics.status, 200);
  assert.match(metrics.contentType, /text\/plain/);
  assert.match(metrics.body, /browser_test_in_flight 0/);

  for (const path of [
    '/',
    '/browser-test',
    '/tools',
    '/browser-test/tools',
    '/status',
    '/browser-test/status',
    '/run',
    '/browser-test/healthz',
    '/browser-test/metrics',
    '/internal/openapi.json',
    '/internal/docs/api',
  ]) {
    const options = path === '/run'
      ? {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ steps: [{ action: 'waitForTimeout', ms: 0 }] }),
        }
      : {};
    const denied = await request(baseUrl, path, options);
    assert.equal(denied.status, 401, `${path} must fail closed without auth`);
    assert.deepEqual(parseJson(denied, path), { ok: false, error: 'unauthorized' });
  }

  const wrongSameLengthSecret = `${authSecret.slice(0, -1)}${authSecret.endsWith('x') ? 'y' : 'x'}`;
  assert.equal(wrongSameLengthSecret.length, authSecret.length);
  const wrongSecret = await request(baseUrl, '/internal/openapi.json', {
    headers: { 'x-server-auth': wrongSameLengthSecret },
  });
  assert.equal(wrongSecret.status, 401);
  assert.deepEqual(parseJson(wrongSecret, 'wrong same-length secret'), {
    ok: false,
    error: 'unauthorized',
  });

  for (const headers of [
    { 'x-server-auth': authSecret },
    { authorization: `Bearer ${authSecret}` },
    { 'x-auth': authSecret },
  ]) {
    const response = await request(baseUrl, '/internal/openapi.json', { headers });
    assert.equal(response.status, 200);
    assert.equal(response.body, internalContract);
  }

  const internalDocument = JSON.parse(internalContract);
  assert.equal(internalDocument['x-dd-contract-scope'], 'internal');
  assert.ok(internalDocument.paths['/run']?.post);

  const internalDocs = await request(baseUrl, '/internal/docs/api', {
    headers: { 'x-server-auth': authSecret },
  });
  assertScalarBootstrap(internalDocs, '/internal/openapi.json', '/internal/docs/api');
  assert.doesNotMatch(internalDocs.body, /data-url=["']\/openapi\.json["']/);
  assert.doesNotMatch(internalDocs.body, /runBrowserScenario/);

  const aliasHeaders = { authorization: `Bearer ${authSecret}` };
  const descriptor = await assertAuthenticatedAliasParity(
    baseUrl,
    '/',
    '/browser-test',
    aliasHeaders,
  );
  const descriptorBody = parseJson(descriptor, 'service descriptor');
  assert.equal(descriptorBody.endpoints.openapi, 'GET /openapi.json');
  assert.equal(descriptorBody.endpoints.docs, 'GET /docs/api');

  const tools = await assertAuthenticatedAliasParity(
    baseUrl,
    '/tools',
    '/browser-test/tools',
    aliasHeaders,
  );
  const toolsBody = parseJson(tools, 'tools');
  assert.equal(toolsBody.default, 'playwright');
  assert.deepEqual(
    toolsBody.tools.map((tool) => tool.name),
    ['playwright', 'puppeteer', 'selenium'],
  );

  const status = await assertAuthenticatedAliasParity(
    baseUrl,
    '/status',
    '/browser-test/status',
    aliasHeaders,
  );
  const statusBody = parseJson(status, 'status');
  assert.equal(statusBody.ok, true);
  assert.equal(statusBody.inFlight, 0);
  assert.ok(statusBody.maxConcurrent > 0);

  const authenticatedHealth = await assertAuthenticatedAliasParity(
    baseUrl,
    '/healthz',
    '/browser-test/healthz',
    aliasHeaders,
  );
  assert.equal(parseJson(authenticatedHealth, 'authenticated health').ok, true);

  const authenticatedMetrics = await assertAuthenticatedAliasParity(
    baseUrl,
    '/metrics',
    '/browser-test/metrics',
    aliasHeaders,
  );
  assert.match(authenticatedMetrics.body, /browser_test_in_flight 0/);

  const invalidRun = await request(baseUrl, '/run', {
    method: 'POST',
    headers: {
      authorization: `Bearer ${authSecret}`,
      'content-type': 'application/json',
    },
    body: JSON.stringify({ steps: [] }),
  });
  assert.equal(invalidRun.status, 400);
  const invalidBody = parseJson(invalidRun, 'invalid run');
  assert.equal(invalidBody.error, 'invalid_request');
  assert.ok(Array.isArray(invalidBody.issues));

  const unknownProperty = await request(baseUrl, '/run', {
    method: 'POST',
    headers: {
      'x-server-auth': authSecret,
      'content-type': 'application/json',
    },
    body: JSON.stringify({
      steps: [{ action: 'waitForTimeout', ms: 0 }],
      undocumented: true,
    }),
  });
  assert.equal(unknownProperty.status, 400);
  assert.equal(parseJson(unknownProperty, 'unknown property').error, 'invalid_request');

  const missing = await request(baseUrl, '/definitely-not-a-route');
  assert.equal(missing.status, 404);
} catch (error) {
  console.error(logs());
  throw error;
} finally {
  await stopChild(child, logs);
}

console.log('browser-test production HTTP contract smoke passed');
