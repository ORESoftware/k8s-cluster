#!/usr/bin/env node

import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const testDirectory = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(testDirectory, '..', '..');
const tool = resolve(repoRoot, 'remote/tools/check-live-api-contract.mjs');
const contractPath = resolve(
  repoRoot,
  'remote/deployments/dd-embeddings-rs/generated/api-docs.json',
);
const service = 'dd-embeddings-rs';
const publicHtml = '<!doctype html><title>API</title><script>window.spec="/openapi.json"</script>';

function runTool(args) {
  return new Promise((resolveResult) => {
    const child = spawn(process.execPath, [tool, ...args], {
      cwd: repoRoot,
      env: { ...process.env, NO_COLOR: '1' },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';
    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    child.on('close', (status, signal) => {
      resolveResult({ status, signal, stdout, stderr });
    });
  });
}

async function withServer(handler, callback) {
  const server = createServer(handler);
  await new Promise((resolveListen, rejectListen) => {
    server.once('error', rejectListen);
    server.listen(0, '127.0.0.1', resolveListen);
  });
  const address = server.address();
  assert.ok(address && typeof address === 'object');
  try {
    return await callback(`http://127.0.0.1:${address.port}`);
  } finally {
    await new Promise((resolveClose, rejectClose) => {
      server.close((error) => (error ? rejectClose(error) : resolveClose()));
    });
  }
}

function fixtureHandler(contractBytes, options = {}) {
  return (request, response) => {
    const route = new URL(request.url ?? '/', 'http://127.0.0.1').pathname;
    if (route === '/openapi.json' || route === '/api/docs.json') {
      const body =
        route === '/api/docs.json' && options.driftJson
          ? Buffer.concat([contractBytes, Buffer.from(' ')])
          : contractBytes;
      response.writeHead(200, {
        'content-type': 'application/openapi+json; charset=utf-8',
        'content-length': body.length,
      });
      response.end(body);
      return;
    }
    if (route === '/api/docs' || route === '/docs/api') {
      const body = Buffer.from(
        options.leakInternal
          ? `${publicHtml}<script>window.internal="/internal/openapi.json"</script>`
          : publicHtml,
      );
      response.writeHead(200, {
        'content-type': 'text/html; charset=utf-8',
        'content-length': body.length,
      });
      response.end(body);
      return;
    }
    response.writeHead(404).end();
  };
}

const contractBytes = await readFile(contractPath);

await withServer(fixtureHandler(contractBytes), async (baseUrl) => {
  const result = await runTool(['--service', service, '--base-url', baseUrl, '--json']);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.signal, null);
  const report = JSON.parse(result.stdout);
  assert.equal(report.schemaVersion, 1);
  assert.equal(report.service, service);
  assert.equal(report.routes.length, 4);
  assert.deepEqual(
    report.routes.map((entry) => entry.route),
    ['/openapi.json', '/api/docs.json', '/api/docs', '/docs/api'],
  );
  assert.ok(report.routes.every((entry) => entry.status === 200));
});

await withServer(fixtureHandler(contractBytes, { driftJson: true }), async (baseUrl) => {
  const result = await runTool(['--service', service, '--base-url', baseUrl]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /GET \/api\/docs\.json bytes differ/);
});

await withServer(fixtureHandler(contractBytes, { leakInternal: true }), async (baseUrl) => {
  const result = await runTool(['--service', service, '--base-url', baseUrl]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /exposes internal documentation reference/);
});

await withServer(
  (_request, response) => {
    response.writeHead(302, { location: '/openapi.json' }).end();
  },
  async (baseUrl) => {
    const result = await runTool(['--service', service, '--base-url', baseUrl]);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /fetch failed|redirect/i);
  },
);

console.log('live API contract conformance harness tests passed');
