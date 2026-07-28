import assert from 'node:assert/strict';
import test from 'node:test';
import { CATALOG_SHA256, OPERATIONS, SDK_SCOPE, buildRequest } from '../dist/index.js';

test('generated internal fleet SDK builds a canonical docs request', () => {
  assert.equal(SDK_SCOPE, "internal");
  assert.equal(CATALOG_SHA256, "936fd83e3f4c565e871b07b4608d3191ab9a0235d5d4a970bf0d613854e18b94");
  assert.equal(OPERATIONS.length, 914);
  const request = buildRequest({
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  });
  assert.equal(request.method, "GET");
  assert.equal(request.url, 'https://example.test/api/docs');
});
