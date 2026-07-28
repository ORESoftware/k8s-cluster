import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test, { after } from 'node:test';

process.env.SERVER_AUTH_SECRET = 'browser-test-contract-secret';
process.env.BROWSER_TEST_ALLOW_UNAUTHENTICATED = 'false';

const { buildServer } = await import('../dist/server.js');
const app = await buildServer();
const publicContract = await readFile(new URL('../generated/api-docs.json', import.meta.url), 'utf8');
const internalContract = await readFile(
  new URL('../generated/openapi.json', import.meta.url),
  'utf8',
);
const authorization = {
  authorization: `Bearer ${process.env.SERVER_AUTH_SECRET}`,
};

const operationEntries = (document) => {
  const entries = [];
  for (const [path, pathItem] of Object.entries(document.paths ?? {})) {
    for (const method of ['get', 'post', 'put', 'patch', 'delete', 'head', 'options', 'trace']) {
      if (pathItem?.[method]) entries.push({ path, method, operation: pathItem[method] });
    }
  }
  return entries;
};

after(async () => {
  await app.close();
});

test('standard public JSON routes serve the exact committed projection', async () => {
  for (const url of ['/openapi.json', '/api/docs.json']) {
    const response = await app.inject({ method: 'GET', url });
    assert.equal(response.statusCode, 200, url);
    assert.match(response.headers['content-type'] ?? '', /^application\/json\b/);
    assert.equal(response.body, publicContract, url);
  }

  const document = JSON.parse(publicContract);
  assert.equal(document.openapi, '3.1.0');
  assert.equal(document['x-dd-contract-scope'], 'public');
  assert.equal(document.paths['/run'], undefined);
  assert.equal(document.paths['/internal/openapi.json'], undefined);
  assert.deepEqual(
    Object.keys(document.paths).sort(),
    ['/api/docs', '/api/docs.json', '/docs/api', '/healthz', '/metrics', '/openapi.json'],
  );
  for (const { path, method, operation } of operationEntries(document)) {
    assert.equal(operation['x-dd-visibility'], 'public', `${method.toUpperCase()} ${path}`);
    assert.deepEqual(operation.security, [], `${method.toUpperCase()} ${path}`);
    for (const privateExtension of [
      'x-dd-auth',
      'x-dd-handlers',
      'x-dd-implementation',
      'x-dd-source-files',
      'x-dd-source-path',
      'x-dd-source-paths',
    ]) {
      assert.equal(operation[privateExtension], undefined, `${method.toUpperCase()} ${path}`);
    }
  }
});

test('standard public Scalar aliases are backed only by the public contract', async () => {
  for (const url of ['/api/docs', '/docs/api']) {
    const response = await app.inject({ method: 'GET', url });
    assert.equal(response.statusCode, 200, url);
    assert.match(response.headers['content-type'] ?? '', /^text\/html\b/);
    assert.match(response.body, /Scalar\.createApiReference/);
    assert.match(response.body, /\/openapi\.json/);
    assert.doesNotMatch(response.body, /runBrowserScenario/);
  }
});

test('complete OpenAPI and Scalar reference fail closed and require bearer auth', async () => {
  for (const url of ['/internal/openapi.json', '/internal/docs/api']) {
    const denied = await app.inject({ method: 'GET', url });
    assert.equal(denied.statusCode, 401, url);
    assert.deepEqual(denied.json(), { ok: false, error: 'unauthorized' });
  }

  const openapi = await app.inject({
    method: 'GET',
    url: '/internal/openapi.json',
    headers: authorization,
  });
  assert.equal(openapi.statusCode, 200);
  assert.equal(openapi.body, internalContract);
  const document = openapi.json();
  assert.equal(document['x-dd-contract-scope'], 'internal');
  assert.ok(document.paths['/run']?.post);
  assert.deepEqual(document.paths['/run'].post.security, [{ bearer_auth: [] }]);

  const scalar = await app.inject({
    method: 'GET',
    url: '/internal/docs/api',
    headers: authorization,
  });
  assert.equal(scalar.statusCode, 200);
  assert.match(scalar.body, /Scalar\.createApiReference/);
  assert.match(scalar.body, /runBrowserScenario/);
});

test('operational routes and aliases inherit auth from executable schema metadata', async () => {
  for (const url of [
    '/',
    '/browser-test',
    '/tools',
    '/browser-test/tools',
    '/status',
    '/browser-test/status',
    '/browser-test/healthz',
    '/browser-test/metrics',
  ]) {
    const denied = await app.inject({ method: 'GET', url });
    assert.equal(denied.statusCode, 401, url);
  }

  for (const url of ['/healthz', '/metrics']) {
    const response = await app.inject({ method: 'GET', url });
    assert.equal(response.statusCode, 200, url);
  }

  const descriptor = await app.inject({ method: 'GET', url: '/', headers: authorization });
  assert.equal(descriptor.statusCode, 200);
  assert.equal(descriptor.json().endpoints.publicOpenApi, 'GET /openapi.json');
  assert.equal(descriptor.json().endpoints.internalOpenApi, 'GET /internal/openapi.json');

  const tools = await app.inject({ method: 'GET', url: '/tools', headers: authorization });
  assert.equal(tools.statusCode, 200);
  assert.equal(tools.json().defaultTool, 'playwright');
  assert.equal(Object.hasOwn(tools.json(), 'default'), false);
});

test('TypeBox rejects invalid run requests before any browser process starts', async () => {
  const response = await app.inject({
    method: 'POST',
    url: '/run',
    headers: {
      ...authorization,
      'content-type': 'application/json',
    },
    payload: { steps: [] },
  });
  assert.equal(response.statusCode, 400);
  const body = response.json();
  assert.equal(body.ok, false);
  assert.equal(body.error, 'request validation failed');
  assert.ok(Array.isArray(body.details));
});

test('the full contract has unique operation IDs and fail-closed security', () => {
  const document = JSON.parse(internalContract);
  const toolsResponseSchema =
    document.paths['/tools'].get.responses['200'].content['application/json'].schema;
  assert.ok(toolsResponseSchema.properties.defaultTool);
  assert.equal(toolsResponseSchema.properties.default, undefined);

  const seen = new Set();
  for (const { path, method, operation } of operationEntries(document)) {
    assert.equal(typeof operation.operationId, 'string', `${method.toUpperCase()} ${path}`);
    assert.equal(seen.has(operation.operationId), false, operation.operationId);
    seen.add(operation.operationId);
    if (operation['x-dd-visibility'] === 'public') {
      assert.deepEqual(operation.security, [], `${method.toUpperCase()} ${path}`);
    } else {
      assert.deepEqual(
        operation.security,
        [{ bearer_auth: [] }],
        `${method.toUpperCase()} ${path}`,
      );
    }
  }
  assert.equal(seen.size, document['x-dd-operation-count']);
});
