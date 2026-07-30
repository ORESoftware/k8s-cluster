import assert from 'node:assert/strict';
import test from 'node:test';
import { CATALOG_SHA256, OPERATIONS, SDK_SCOPE, buildRequest } from '../dist/index.js';

test('generated public fleet SDK builds a canonical docs request', () => {
  assert.equal(SDK_SCOPE, "public");
  assert.equal(CATALOG_SHA256, "c9762b2640f40bc77601c40d27435d12096ec4ba96aab05d84f9e0526dcd2da9");
  assert.equal(OPERATIONS.length, 279);
  const request = buildRequest({
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  });
  assert.equal(request.method, "GET");
  assert.equal(request.url, 'https://example.test/api/docs');
});
