import assert from 'node:assert/strict';
import test from 'node:test';
import { CATALOG_SHA256, OPERATIONS, SDK_SCOPE, buildRequest } from '../dist/index.js';

test('generated public fleet SDK builds a canonical docs request', () => {
  assert.equal(SDK_SCOPE, "public");
  assert.equal(CATALOG_SHA256, "6efa34dcaa32999c9265b34c503b99e9a824be3cda1c398871bb7a174f8e7721");
  assert.equal(OPERATIONS.length, 281);
  const request = buildRequest({
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  });
  assert.equal(request.method, "GET");
  assert.equal(request.url, 'https://example.test/api/docs');
});
