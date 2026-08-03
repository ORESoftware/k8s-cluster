import assert from 'node:assert/strict';
import test from 'node:test';

import Fastify from 'fastify';
import { z } from 'zod';

import { ApiContractRegistry } from './api-contract.js';
import { buildApp } from './server.js';

type JsonObject = Record<string, unknown>;

function operationKeys(document: JsonObject): string[] {
  const methods = new Set(['get', 'post', 'put', 'patch', 'delete', 'head', 'options', 'trace']);
  const result: string[] = [];
  for (const [path, rawPathItem] of Object.entries((document.paths as JsonObject | undefined) ?? {})) {
    for (const [method] of Object.entries(rawPathItem as JsonObject)) {
      if (methods.has(method)) result.push(`${method.toUpperCase()} ${path}`);
    }
  }
  return result.sort();
}

function assertNoDebugExtensions(value: unknown): void {
  const forbidden = new Set([
    'x-dd-auth',
    'x-dd-handlers',
    'x-dd-implementation',
    'x-dd-source-files',
    'x-dd-source-path',
    'x-dd-source-paths',
  ]);
  const stack = [value];
  while (stack.length > 0) {
    const current = stack.pop();
    if (current === null || typeof current !== 'object') continue;
    if (Array.isArray(current)) {
      stack.push(...current);
      continue;
    }
    for (const [key, item] of Object.entries(current as JsonObject)) {
      assert.ok(!forbidden.has(key), `public contract leaked ${key}`);
      stack.push(item);
    }
  }
}

test('contract export is deterministic and registered route keys match OpenAPI', async (context) => {
  const first = await buildApp({ authSecret: 'contract-test-secret', instrumentTelemetry: false });
  const second = await buildApp({ authSecret: 'contract-test-secret', instrumentTelemetry: false });
  context.after(async () => {
    await first.app.close();
    await second.app.close();
  });

  assert.equal(first.documents.internalJson, second.documents.internalJson);
  assert.equal(first.documents.publicJson, second.documents.publicJson);
  assert.deepEqual(first.documents.routeKeys, operationKeys(first.documents.internalDocument));
  assert.equal(first.documents.internalDocument['x-dd-contract-scope'], 'internal');
  assert.equal(first.documents.publicDocument['x-dd-contract-scope'], 'public');
});

test('public document is an exact fail-closed subset without private metadata', async (context) => {
  const built = await buildApp({ authSecret: 'contract-test-secret', instrumentTelemetry: false });
  context.after(() => built.app.close());

  const internalPaths = built.documents.internalDocument.paths as JsonObject;
  const publicPaths = built.documents.publicDocument.paths as JsonObject;
  assert.ok((internalPaths['/run'] as JsonObject | undefined)?.post);
  assert.equal(publicPaths['/run'], undefined);
  assert.ok((publicPaths['/openapi.json'] as JsonObject | undefined)?.get);
  assert.ok((publicPaths['/healthz'] as JsonObject | undefined)?.get);
  assert.deepEqual(built.documents.publicDocument.components, {});
  assertNoDebugExtensions(built.documents.publicDocument);

  const expected = Object.entries(internalPaths)
    .flatMap(([path, rawPathItem]) =>
      Object.entries(rawPathItem as JsonObject)
        .filter(([, rawOperation]) => (rawOperation as JsonObject)['x-dd-visibility'] === 'public')
        .map(([method]) => `${method.toUpperCase()} ${path}`),
    )
    .sort();
  assert.deepEqual(operationKeys(built.documents.publicDocument), expected);
});

test('standard documentation aliases serve exact canonical public bytes', async (context) => {
  const built = await buildApp({ authSecret: 'contract-test-secret', instrumentTelemetry: false });
  context.after(() => built.app.close());

  for (const url of ['/openapi.json', '/api/docs.json']) {
    const response = await built.app.inject({ method: 'GET', url });
    assert.equal(response.statusCode, 200);
    assert.equal(response.body, built.documents.publicJson);
    assert.match(response.headers['content-type'] ?? '', /openapi\+json|application\/json/);
  }
  for (const url of ['/docs/api', '/api/docs']) {
    const response = await built.app.inject({ method: 'GET', url });
    assert.equal(response.statusCode, 200);
    assert.equal(response.body, built.documents.publicHtml);
    assert.match(response.headers['content-type'] ?? '', /text\/html/);
  }
});

test('internal contract and run endpoint fail closed without service auth', async (context) => {
  const built = await buildApp({ authSecret: 'contract-test-secret', instrumentTelemetry: false });
  context.after(() => built.app.close());

  const internalDenied = await built.app.inject({ method: 'GET', url: '/internal/openapi.json' });
  assert.equal(internalDenied.statusCode, 401);
  assert.deepEqual(internalDenied.json(), { ok: false, error: 'unauthorized' });

  const runDenied = await built.app.inject({
    method: 'POST',
    url: '/run',
    payload: { steps: [{ action: 'waitForTimeout', ms: 0 }] },
  });
  assert.equal(runDenied.statusCode, 401);

  const internalAllowed = await built.app.inject({
    method: 'GET',
    url: '/internal/openapi.json',
    headers: { 'x-server-auth': 'contract-test-secret' },
  });
  assert.equal(internalAllowed.statusCode, 200);
  assert.equal(internalAllowed.body, built.documents.internalJson);
});

test('invalid authenticated requests are rejected before a browser is launched', async (context) => {
  const built = await buildApp({ authSecret: 'contract-test-secret', instrumentTelemetry: false });
  context.after(() => built.app.close());

  const response = await built.app.inject({
    method: 'POST',
    url: '/run',
    headers: { authorization: 'Bearer contract-test-secret' },
    payload: { steps: [] },
  });
  assert.equal(response.statusCode, 400);
  assert.equal(response.json().error, 'invalid_request');
  assert.ok(Array.isArray(response.json().issues));
});

test('Zod discriminated-union bodies reach handlers without AJV branch mutation', async (context) => {
  const app = Fastify();
  context.after(() => app.close());

  const body = z.discriminatedUnion('action', [
    z
      .object({
        action: z.literal('goto'),
        url: z.string().url(),
      })
      .strict(),
    z
      .object({
        action: z.literal('fill'),
        selector: z.string().min(1),
        value: z.string(),
      })
      .strict(),
  ]);
  const response = z
    .object({
      action: z.literal('fill'),
      selector: z.string(),
      value: z.string(),
    })
    .strict();

  const registry = new ApiContractRegistry();
  registry.register(app, {
    method: 'POST',
    path: '/union',
    operationId: 'echoDiscriminatedUnion',
    summary: 'Echo a discriminated-union request.',
    tags: ['test'],
    visibility: 'internal',
    auth: 'public',
    routeType: 'user-generated',
    body,
    responses: {
      '200': {
        description: 'Echoed fill request.',
        schema: response,
      },
    },
    handler: async (request) => request.body,
  });
  await app.ready();

  const payload = { action: 'fill', selector: '#name', value: 'Ada' };
  const result = await app.inject({
    method: 'POST',
    url: '/union',
    payload,
  });

  assert.equal(result.statusCode, 200, result.body);
  assert.deepEqual(result.json(), payload);
});
