import assert from 'node:assert/strict';
import test from 'node:test';
import { CATALOG_SHA256, OPERATIONS, SDK_SCOPE, buildRequest } from '../dist/index.js';

test('generated internal fleet SDK builds a canonical docs request', () => {
  assert.equal(SDK_SCOPE, "internal");
  assert.equal(CATALOG_SHA256, "d20ebafb4afe13d8d63c370d4e0ad98262b17faffb751a756b6ca3b946b9030c");
  assert.equal(OPERATIONS.length, 937);
  const request = buildRequest({
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  });
  assert.equal(request.method, "GET");
  assert.equal(request.url, 'https://example.test/api/docs');
});
