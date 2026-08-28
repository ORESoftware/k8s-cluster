import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import zlib from 'node:zlib';

const ledgerUrl = new URL(
  '../../../docs/audits/google-chat/alex-alex-me-delta-2026-08-04--2026-08-08/ledger.json.gz.base64',
  import.meta.url,
);
const encodedLedger = fs.readFileSync(ledgerUrl, 'utf8').trim();
const ledgerText = zlib
  .gunzipSync(Buffer.from(encodedLedger, 'base64'))
  .toString('utf8');
const ledger = JSON.parse(ledgerText);

const expectedRecordKeys = new Set([
  'ordinal',
  'sourceKey',
  'createTime',
  'category',
  'disposition',
  'issueIds',
  'sensitiveClass',
]);

const allowedUnmappedDispositions = new Set([
  'quarantined-private-contact',
  'quarantined-private-output',
  'excluded-private-personal',
]);

function sortedCounts(records, field) {
  const counts = {};
  for (const record of records) {
    const value = record[field];
    counts[value] = (counts[value] ?? 0) + 1;
  }
  return Object.fromEntries(
    Object.entries(counts).sort(([left], [right]) => left.localeCompare(right)),
  );
}

test('the August 4-8 delta has exact deterministic accounting', () => {
  assert.equal(ledger.schemaVersion, 'google-chat-reconciliation-delta.v1');
  assert.equal(ledger.space, 'spaces/AAQAoHKdzvI');
  assert.equal(ledger.displayName, 'alex-alex-me');

  assert.deepEqual(ledger.sourceExport, {
    relayProtocol: 3,
    transport: 'POST',
    relayRunId: '31280125517-1',
    exportedAt: '2026-08-08T21:48:15Z',
    pages: 6,
    totalMessages: 1273,
    'messagesSince2026-06-05': 1090,
    lastMessageTime: '2026-08-08T21:38:08.363263Z',
    relayCiphertextSha256:
      '33387c544a08cec565fe383391b5047a910b5bea0195b5e6a4434b6c4d1e9487',
    manifestSha256:
      '28841fc5702e1427f35b7da15489a596c090afe955e2da4023142fa45388bf84',
    encryptedArchiveSha256:
      'e6edf89181a016f417646e7af06c93fc47cc81dad9ef37d76011f7df1cc14448',
    relayHardeningPullRequest: 'ORESoftware/k8s-cluster#1213',
  });

  assert.equal(ledger.delta.afterExclusive, '2026-08-04T21:21:07.950927Z');
  assert.equal(ledger.delta.throughInclusive, '2026-08-08T21:38:08.363263Z');
  assert.equal(ledger.delta.recordCount, 77);
  assert.equal(ledger.records.length, 77);

  const sourceKeys = ledger.records.map((record) => record.sourceKey);
  assert.equal(new Set(sourceKeys).size, 77);
  assert.ok(
    sourceKeys.every((sourceKey) =>
      /^google-chat:AAQAoHKdzvI:spaces\/AAQAoHKdzvI\/messages\/[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/.test(
        sourceKey,
      ),
    ),
  );

  const ordinals = ledger.records.map((record) => record.ordinal);
  assert.deepEqual(ordinals, Array.from({ length: 77 }, (_, index) => index + 1));

  const times = ledger.records.map((record) => record.createTime);
  assert.deepEqual(times, [...times].sort());
  assert.equal(times[0], '2026-08-04T23:35:07.611711Z');
  assert.equal(times.at(-1), '2026-08-08T21:38:08.363263Z');

  assert.deepEqual(
    sortedCounts(ledger.records, 'disposition'),
    Object.fromEntries(
      Object.entries(ledger.delta.dispositionCounts).sort(([left], [right]) =>
        left.localeCompare(right),
      ),
    ),
  );
  assert.equal(
    Object.values(ledger.delta.dispositionCounts).reduce(
      (sum, count) => sum + count,
      0,
    ),
    77,
  );
});

test('the exact rolling 20-day window is covered by four contiguous ledgers', () => {
  assert.equal(ledger.rollingWindow.durationDays, 20);
  assert.equal(ledger.rollingWindow.startInclusive, '2026-07-19T21:48:15Z');
  assert.equal(
    ledger.rollingWindow.throughInclusive,
    '2026-08-08T21:38:08.363263Z',
  );
  assert.equal(ledger.rollingWindow.recordCount, 332);

  const segments = ledger.rollingWindow.ledgerSegments;
  assert.deepEqual(
    segments.map((segment) => segment.recordCount),
    [197, 48, 10, 77],
  );
  assert.equal(
    segments.reduce((sum, segment) => sum + segment.recordCount, 0),
    332,
  );
  assert.equal(
    segments[0].throughInclusive,
    segments[1].afterExclusive,
  );
  assert.equal(
    segments[1].throughInclusive,
    segments[2].afterExclusive,
  );
  assert.equal(
    segments[2].throughInclusive,
    segments[3].afterExclusive,
  );
  assert.equal(
    segments[3].throughInclusive,
    ledger.rollingWindow.throughInclusive,
  );

  assert.equal(
    955 + 48 + 10 + 77,
    ledger.sourceExport['messagesSince2026-06-05'],
  );
});

test('every actionable record maps to canonical Linear work', () => {
  for (const record of ledger.records) {
    assert.ok(record.category.length > 0);
    assert.ok(record.disposition.length > 0);
    assert.ok(Array.isArray(record.issueIds));

    if (!allowedUnmappedDispositions.has(record.disposition)) {
      assert.ok(
        record.issueIds.length > 0,
        `missing issue mapping for ordinal ${record.ordinal}`,
      );
    }

    for (const issueId of record.issueIds) {
      assert.match(issueId, /^DEN-\d+$/);
    }

    for (const key of Object.keys(record)) {
      assert.ok(expectedRecordKeys.has(key), `unexpected record field ${key}`);
    }
  }

  assert.deepEqual(ledger.delta.newCanonicalIssueIds, [
    'DEN-3175',
    'DEN-3176',
  ]);

  const gracefulStart = ledger.records[71];
  const gracefulFollowup = ledger.records[72];
  const contextAndShutdown = ledger.records[74];

  assert.equal(
    gracefulStart.sourceKey,
    'google-chat:AAQAoHKdzvI:spaces/AAQAoHKdzvI/messages/WdeAzPFAg8s.WdeAzPFAg8s',
  );
  assert.equal(gracefulStart.disposition, 'created-new');
  assert.deepEqual(gracefulStart.issueIds, ['DEN-3175']);

  assert.equal(
    gracefulFollowup.sourceKey,
    'google-chat:AAQAoHKdzvI:spaces/AAQAoHKdzvI/messages/LzWmpPcFd-I.LzWmpPcFd-I',
  );
  assert.equal(gracefulFollowup.disposition, 'attached-to-new');
  assert.deepEqual(gracefulFollowup.issueIds, ['DEN-3175']);

  assert.equal(
    contextAndShutdown.sourceKey,
    'google-chat:AAQAoHKdzvI:spaces/AAQAoHKdzvI/messages/WolGPwlHU1U.WolGPwlHU1U',
  );
  assert.equal(
    contextAndShutdown.disposition,
    'created-and-attached-secret-quarantined',
  );
  assert.ok(contextAndShutdown.issueIds.includes('DEN-3175'));
  assert.ok(contextAndShutdown.issueIds.includes('DEN-3176'));
});

test('the committed audit is content-free and secret-shape-free', () => {
  assert.deepEqual(ledger.privacy, {
    containsMessageBodies: false,
    containsSenderIdentities: false,
    containsCredentials: false,
    containsContactValues: false,
    containsPrivateOutputDestinations: false,
    credentialBearingRecords: 22,
    privateContactRecords: 4,
    privateOutputTransportRecords: 1,
    excludedPrivatePersonalRecords: 2,
    credentialIncidentIssueIds: ['DEN-1230', 'DEN-3053', 'DEN-2836'],
  });

  assert.equal(
    ledger.records.filter(
      (record) => record.sensitiveClass === 'credential-bearing',
    ).length,
    22,
  );
  assert.equal(
    ledger.records.filter(
      (record) => record.sensitiveClass === 'private-contact',
    ).length,
    4,
  );
  assert.equal(
    ledger.records.filter(
      (record) => record.sensitiveClass === 'private-output-destination',
    ).length,
    1,
  );
  assert.equal(
    ledger.records.filter(
      (record) => record.sensitiveClass === 'private-personal',
    ).length,
    2,
  );

  assert.doesNotMatch(ledgerText, /gh[pousr]_[A-Za-z0-9]{20,}/);
  assert.doesNotMatch(ledgerText, /lin_api_[A-Za-z0-9_-]+/i);
  assert.doesNotMatch(ledgerText, /cfat_[A-Za-z0-9_-]+/i);
  assert.doesNotMatch(ledgerText, /\bAKIA[A-Z0-9]{16}\b/);
  assert.doesNotMatch(
    ledgerText,
    /CHAT_BRIDGE_TOKEN\s*=|LINEAR_API_(?:KEY|TOKEN)\s*=|Secret Access Key/i,
  );
  assert.doesNotMatch(
    ledgerText,
    /[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/i,
  );

  for (const record of ledger.records) {
    for (const forbidden of [
      'text',
      'body',
      'sender',
      'senderId',
      'email',
      'phone',
      'token',
      'credential',
      'contactValue',
      'destination',
    ]) {
      assert.equal(
        Object.hasOwn(record, forbidden),
        false,
        `forbidden field ${forbidden} on ordinal ${record.ordinal}`,
      );
    }
  }
});
