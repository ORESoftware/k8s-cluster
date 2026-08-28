import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';

const ledgerUrl = new URL(
  '../../../docs/audits/google-chat/alex-alex-me-delta-2026-08-04/ledger.json',
  import.meta.url,
);
const ledgerText = fs.readFileSync(ledgerUrl, 'utf8');
const ledger = JSON.parse(ledgerText);

const expectedRecordKeys = new Set([
  'ordinal',
  'sourceKey',
  'createTime',
  'category',
  'disposition',
  'issueIds',
  'sensitiveClass',
  'duplicateOf',
]);

function dispositionCounts(records) {
  const counts = {};
  for (const record of records) {
    counts[record.disposition] = (counts[record.disposition] ?? 0) + 1;
  }
  return Object.fromEntries(Object.entries(counts).sort(([left], [right]) => left.localeCompare(right)));
}

test('the August 4 delta has exact, deterministic one-to-one accounting', () => {
  assert.equal(ledger.schemaVersion, 'google-chat-reconciliation-delta.v1');
  assert.equal(ledger.space, 'spaces/AAQAoHKdzvI');
  assert.equal(ledger.displayName, 'alex-alex-me');
  assert.deepEqual(ledger.sourceExport, {
    relayRunId: '30956776367-1',
    exportedAt: '2026-08-04T22:40:17Z',
    totalMessages: 1196,
    'messagesSince2026-06-05': 1013,
  });
  assert.equal(ledger.delta.afterExclusive, '2026-08-04T02:53:06.997967Z');
  assert.equal(ledger.delta.throughInclusive, '2026-08-04T21:21:07.950927Z');
  assert.equal(ledger.delta.recordCount, 10);
  assert.equal(ledger.records.length, 10);

  const sourceKeys = ledger.records.map((record) => record.sourceKey);
  assert.equal(new Set(sourceKeys).size, 10);
  assert.ok(
    sourceKeys.every((sourceKey) =>
      /^google-chat:AAQAoHKdzvI:spaces\/AAQAoHKdzvI\/messages\/[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/.test(
        sourceKey,
      ),
    ),
  );

  const ordinals = ledger.records.map((record) => record.ordinal);
  assert.deepEqual(ordinals, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
  const times = ledger.records.map((record) => record.createTime);
  assert.deepEqual(times, [...times].sort());

  assert.deepEqual(
    dispositionCounts(ledger.records),
    Object.fromEntries(
      Object.entries(ledger.delta.dispositionCounts).sort(([left], [right]) =>
        left.localeCompare(right),
      ),
    ),
  );
  assert.equal(
    Object.values(ledger.delta.dispositionCounts).reduce((sum, count) => sum + count, 0),
    10,
  );
});

test('the ledger maps actionable records and records the duplicate explicitly', () => {
  for (const record of ledger.records) {
    assert.ok(record.category.length > 0);
    assert.ok(record.disposition.length > 0);
    assert.ok(Array.isArray(record.issueIds));
    if (record.disposition !== 'duplicate') {
      assert.ok(record.issueIds.length > 0, `missing issue mapping for ordinal ${record.ordinal}`);
    }
    for (const key of Object.keys(record)) {
      assert.ok(expectedRecordKeys.has(key), `unexpected record field ${key}`);
    }
  }

  const duplicate = ledger.records.at(-1);
  const canonical = ledger.records.at(-2);
  assert.equal(duplicate.disposition, 'duplicate');
  assert.equal(duplicate.duplicateOf, canonical.sourceKey);
  assert.deepEqual(duplicate.issueIds, []);
  assert.deepEqual(ledger.delta.newCanonicalIssueIds, [
    'DEN-1948',
    'DEN-1949',
    'DEN-1950',
    'DEN-1951',
  ]);
  assert.deepEqual(ledger.delta.reopenedIssueIds, ['DEN-1889']);
});

test('the committed audit is content-free and secret-shape-free', () => {
  assert.deepEqual(ledger.privacy, {
    containsMessageBodies: false,
    containsSenderIdentities: false,
    containsCredentials: false,
    containsContactValues: false,
    containsPrivateOutputDestinations: false,
    credentialBearingRecords: 1,
    privateOutputTransportRecords: 1,
    credentialIncidentIssueIds: ['DEN-1230', 'DEN-27'],
  });

  assert.doesNotMatch(ledgerText, /gh[pousr]_[A-Za-z0-9]{20,}/);
  assert.doesNotMatch(ledgerText, /lin_api_[A-Za-z0-9_-]+/i);
  assert.doesNotMatch(ledgerText, /CHAT_BRIDGE_TOKEN|LINEAR_API_KEY|LINEAR_API_TOKEN/);
  assert.doesNotMatch(ledgerText, /[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/i);

  for (const record of ledger.records) {
    assert.equal(Object.hasOwn(record, 'text'), false);
    assert.equal(Object.hasOwn(record, 'body'), false);
    assert.equal(Object.hasOwn(record, 'sender'), false);
    assert.equal(Object.hasOwn(record, 'email'), false);
    assert.equal(Object.hasOwn(record, 'phone'), false);
    assert.equal(Object.hasOwn(record, 'token'), false);
  }
});
