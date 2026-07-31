import assert from 'node:assert/strict';
import test from 'node:test';
import { CATALOG_SHA256, OPERATIONS, SDK_SCOPE, buildRequest } from '../dist/index.js';

test('generated public fleet SDK builds a canonical docs request', () => {
  assert.equal(SDK_SCOPE, "public");
  assert.equal(CATALOG_SHA256, "dd6cd2dfce13705381cfcc79c99d9f2b7e7307fc846e28609defe908e9859689");
  assert.equal(OPERATIONS.length, 279);
  const request = buildRequest({
    baseUrl: 'https://example.test/',
    operationId: "agent_worker_broker_rs_get_api_docs_2fc0dbab70df",
  });
  assert.equal(request.method, "GET");
  assert.equal(request.url, 'https://example.test/api/docs');
});
