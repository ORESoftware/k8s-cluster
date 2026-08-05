import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildBatchReport,
  makeBatchId,
  normalizeCategoryRows,
  parseBoolean,
  parseBoundedInteger,
  planCategoryTargets,
  validateBatchId,
} from './contact-batch-lib.mjs';

test('bounded configuration fails closed', () => {
  assert.equal(parseBoolean('true'), true);
  assert.equal(parseBoolean('off', true), false);
  assert.throws(() => parseBoolean('maybe'), /invalid boolean/);
  assert.equal(parseBoundedInteger('TARGET', '250', { defaultValue: 250, min: 200, max: 300 }), 250);
  assert.throws(
    () => parseBoundedInteger('TARGET', '301', { defaultValue: 250, min: 200, max: 300 }),
    /between 200 and 300/,
  );
});

test('batch ids are bounded and validated', () => {
  const batchId = makeBatchId(new Date('2026-08-05T20:15:00.000Z'), () => Buffer.from('01020304', 'hex'));
  assert.equal(batchId, 'benefactor-20260805T201500Z-01020304');
  assert.equal(validateBatchId(batchId), batchId);
  assert.throws(() => validateBatchId('not/a/batch'), /unsupported format/);
});

test('category normalization is deterministic and honors allowlists', () => {
  const rows = [
    { service_category: ' Roofing ', query_count: '8', max_priority: 4 },
    { service_category: 'plumbing', query_count: 3, max_priority: 7 },
    { service_category: 'roofing', query_count: 100, max_priority: 1 },
  ];
  assert.deepEqual(normalizeCategoryRows(rows), [
    { category: 'plumbing', queryCount: 3, priority: 7 },
    { category: 'roofing', queryCount: 8, priority: 4 },
  ]);
  assert.deepEqual(normalizeCategoryRows(rows, ['roofing']), [
    { category: 'roofing', queryCount: 8, priority: 4 },
  ]);
});

test('a 250-contact plan stays bounded and rotates categories', () => {
  const categories = normalizeCategoryRows([
    { service_category: 'roofing', query_count: 10, max_priority: 5 },
    { service_category: 'hvac', query_count: 9, max_priority: 4 },
    { service_category: 'plumbing', query_count: 8, max_priority: 3 },
  ]);
  const plan = planCategoryTargets(categories, 250, 40);
  assert.equal(plan.reduce((sum, item) => sum + item.target, 0), 250);
  assert.ok(plan.every((item) => item.target <= 40));
  assert.ok(plan.some((item) => item.pass > 0));
});

test('reports never imply that discovery authorizes outreach', () => {
  const report = buildBatchReport({
    batchId: 'benefactor-20260805T201500Z-01020304',
    dryRun: false,
    targetContacts: 250,
    minimumContacts: 200,
    maximumContacts: 300,
    categoriesPlanned: 6,
    categoriesRun: 5,
    contactsTagged: 244,
    approvedForOutreach: 0,
    status: 'collected',
  });
  assert.equal(report.outreachDispatchRequested, false);
  assert.equal(report.approvedForOutreach, 0);
  assert.equal(report.batchDigest.length, 64);
});
