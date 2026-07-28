import assert from 'node:assert/strict';
import test from 'node:test';
import { CATALOG_SHA256, OPERATIONS, SDK_SCOPE, buildRequest } from '../dist/index.js';

test('generated public fleet SDK builds a canonical docs request', () => {
  assert.equal(SDK_SCOPE, "public");
  assert.equal(CATALOG_SHA256, "8b13a51a18433272657ee6c5e51b159c0a1d68a03ff6ae964d9bc553af5d2510");
  assert.equal(OPERATIONS.length, 281);
  const request = buildRequest({
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  });
  assert.equal(request.method, "GET");
  assert.equal(request.url, 'https://example.test/api/docs');
});
