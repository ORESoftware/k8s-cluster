import assert from 'node:assert/strict';
import test from 'node:test';
import { CATALOG_SHA256, OPERATIONS, SDK_SCOPE, buildRequest } from '../dist/index.js';

test('generated internal fleet SDK builds a canonical docs request', () => {
  assert.equal(SDK_SCOPE, "internal");
  assert.equal(CATALOG_SHA256, "331bacb861a9309b41c9d5f6dc6f8bfa58b745c91bf44fdd49ba4ebc5822063a");
  assert.equal(OPERATIONS.length, 941);
  const request = buildRequest({
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  });
  assert.equal(request.method, "GET");
  assert.equal(request.url, 'https://example.test/api/docs');
});
