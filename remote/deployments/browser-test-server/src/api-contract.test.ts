import assert from 'node:assert/strict';
import { after, test } from 'node:test';

process.env.SERVER_AUTH_SECRET = 'contract-test-secret';
process.env.BROWSER_TEST_ALLOW_UNAUTHENTICATED = 'false';

const { fastify } = await import('./server.js');
await fastify.ready();

after(async () => {
  await fastify.close();
});

test('standard OpenAPI aliases expose the same executable contract', async () => {
  const canonical = await fastify.inject({ method: 'GET', url: '/openapi.json' });
  const compatibility = await fastify.inject({ method: 'GET', url: '/api/docs.json' });

  assert.equal(canonical.statusCode, 200);
  assert.equal(compatibility.statusCode, 200);
  assert.match(canonical.headers['content-type'] ?? '', /application\/json/);
  assert.deepEqual(compatibility.json(), canonical.json());

  const document = canonical.json() as {
    openapi: string;
    paths: Record<string, Record<string, { operationId?: string; requestBody?: unknown }>>;
  };
  assert.equal(document.openapi, '3.1.0');

  const expectedOperations = new Map([
    ['GET /', 'getBrowserTestService'],
    ['GET /browser-test', 'getBrowserTestServiceAlias'],
    ['GET /tools', 'listBrowserTools'],
    ['GET /browser-test/tools', 'listBrowserToolsAlias'],
    ['GET /status', 'getBrowserTestStatus'],
    ['GET /browser-test/status', 'getBrowserTestStatusAlias'],
    ['GET /healthz', 'getBrowserTestHealth'],
    ['GET /browser-test/healthz', 'getBrowserTestHealthAlias'],
    ['GET /metrics', 'getBrowserTestMetrics'],
    ['GET /browser-test/metrics', 'getBrowserTestMetricsAlias'],
    ['POST /run', 'runBrowserScenario'],
  ]);

  const seenOperationIds = new Set<string>();
  for (const [route, expectedOperationId] of expectedOperations) {
    const [method, path] = route.split(' ');
    assert.ok(method && path);
    const operation = document.paths[path]?.[method.toLowerCase()];
    assert.ok(operation, `OpenAPI operation missing for ${route}`);
    assert.equal(operation.operationId, expectedOperationId);
    assert.ok(!seenOperationIds.has(expectedOperationId), `duplicate operationId ${expectedOperationId}`);
    seenOperationIds.add(expectedOperationId);
  }

  assert.ok(document.paths['/run']?.post?.requestBody, 'POST /run must have a typed request body');
  assert.equal(document.paths['/api/docs'], undefined, 'documentation UI must not document itself');
  assert.equal(document.paths['/openapi.json'], undefined, 'contract endpoint must not document itself');
});

test('Scalar documentation is mounted at both standard HTML routes', async () => {
  for (const url of ['/api/docs', '/docs/api']) {
    const response = await fastify.inject({ method: 'GET', url });
    assert.equal(response.statusCode, 200, `${url} should render directly`);
    assert.match(response.headers['content-type'] ?? '', /text\/html/);
    assert.match(response.body.toLowerCase(), /scalar|api reference/);
  }
});

test('run route is fail-closed before browser execution', async () => {
  const response = await fastify.inject({
    method: 'POST',
    url: '/run',
    payload: { steps: [{ action: 'waitForTimeout', ms: 0 }] },
  });

  assert.equal(response.statusCode, 401);
  assert.deepEqual(response.json(), { ok: false, error: 'unauthorized' });
});

test('Zod is the runtime validator for the documented request body', async () => {
  const response = await fastify.inject({
    method: 'POST',
    url: '/run',
    headers: {
      'x-server-auth': 'contract-test-secret',
      'content-type': 'application/json',
    },
    payload: { steps: [] },
  });

  assert.equal(response.statusCode, 400);
  const body = response.json() as { ok?: boolean; error?: string; details?: unknown };
  assert.equal(body.ok, false);
  assert.equal(body.error, 'request_validation_failed');
  assert.ok(body.details);
});

test('patched Fastify cannot bypass body validation with content-type whitespace', async () => {
  const response = await fastify.inject({
    method: 'POST',
    url: '/run',
    headers: {
      'x-server-auth': 'contract-test-secret',
      'content-type': ' application/json',
    },
    payload: JSON.stringify({ steps: [] }),
  });

  assert.notEqual(response.statusCode, 200);
  assert.notEqual(response.statusCode, 422);
});
