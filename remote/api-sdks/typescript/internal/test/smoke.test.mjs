import assert from 'node:assert/strict';
import test from 'node:test';
import { CATALOG_SHA256, OPERATIONS, SDK_SCOPE, buildRequest } from '../dist/index.js';

test('generated internal fleet SDK builds a canonical docs request', () => {
  assert.equal(SDK_SCOPE, "internal");
  assert.equal(CATALOG_SHA256, "8bd3ddbda3bbf663edfd3bf887213540cfff2e7b5ae13692663a390cbf59c4b4");
  assert.equal(OPERATIONS.length, 942);
  const request = buildRequest({
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  });
  assert.equal(request.method, "GET");
  assert.equal(request.url, 'https://example.test/api/docs');
});
