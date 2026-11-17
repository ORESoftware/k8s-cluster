import assert from 'node:assert/strict';
import test from 'node:test';
import { buildMessageInventory, canonicalChatInstant, reconcileMessageCoverage } from '../message-coverage.mjs';

const H = 'a'.repeat(64);
const key = new Uint8Array(32).fill(7); // synthetic test provenance, not a credential
const start = '2026-08-08T05:00:00Z';
const end = '2026-09-08T22:00:00Z';
const source = { kind: 'bridge-export', artifactSha256: H, paginationComplete: true, snapshotTime: end };
const record = (id = 1, extra = {}) => ({ messageName: `spaces/AAQAoHKdzvI/messages/m${id}.m${id}`,
  threadName: 'spaces/AAQAoHKdzvI/threads/root', createTime: '2026-08-09T12:00:00.123456789Z',
  lastUpdateTime: null, bodySha256: H, attachmentCount: 0, deleted: false, ...extra });
const build = (records = [record()], extra = {}) => buildMessageInventory({ records, source,
  windowStartInclusive: start, windowEndExclusive: end, provenanceKey: key, ...extra });
const requirement = (extra = {}) => ({ key: 'REQ_1', issues: ['https://github.com/test-org/test-repo/issues/1'],
  pullRequests: [{ url: 'https://github.com/test-org/test-repo/pull/2', headSha: 'b'.repeat(40), kind: 'implementation' }], ...extra });
const entry = (row, extra = {}) => ({ messageKey: row.messageKey, versionDigest: row.versionDigest,
  disposition: 'actionable', requirements: [requirement()], attachmentReview: { reviewedCount: 0, evidenceDigest: null }, ...extra });
const review = (inv, entries = inv.records.map((r) => entry(r))) => ({ schemaVersion: 1, inventoryDigest: inv.inventoryDigest, entries });
const reconcile = (inv, entries) => reconcileMessageCoverage({ inventory: inv, review: review(inv, entries) });

for (const [input, expected] of [
  ['2026-08-08T00:00:00Z', '2026-08-08T00:00:00.000000000Z'],
  ['2026-08-08T00:00:00.1Z', '2026-08-08T00:00:00.100000000Z'],
  ['2026-08-08T00:00:00.123456789Z', '2026-08-08T00:00:00.123456789Z'],
  ['2024-02-29T00:00:00Z', '2024-02-29T00:00:00.000000000Z'],
]) test(`canonical UTC precision: ${input}`, () => assert.equal(canonicalChatInstant(input), expected));
for (const value of [null, 123, '', 'yesterday', '2026-02-29T00:00:00Z', '2026-08-32T00:00:00Z',
  '2026-08-08T24:00:00Z', '2026-08-08T00:00:60Z', '2026-08-08T00:00:00.1234567890Z',
  '2026-08-08T00:00:00-05:00', '2026-08-08', ' 2026-08-08T00:00:00Z']) {
  test(`rejects malformed/noncanonical time: ${String(value)}`, () => assert.throws(() => canonicalChatInstant(value), /invalid_instant/));
}
test('accepts the explicit August 8 to September 8 window, not only fifteen days', () => {
  assert.equal(build().counts.inWindow, 1);
  assert.equal(reconcile(build()).sourceCompletenessReported, true);
});
test('uses exact nanosecond inclusive start and exclusive end', () => {
  const inv = build([record(1, { createTime: '2026-08-09T00:00:00.123456788Z' }),
    record(2, { createTime: '2026-08-09T00:00:00.123456789Z' }),
    record(3, { createTime: '2026-08-09T00:00:00.123456790Z' })],
  { windowStartInclusive: '2026-08-09T00:00:00.123456789Z', windowEndExclusive: '2026-08-09T00:00:00.123456790Z' });
  assert.equal(inv.counts.inWindow, 1);
  assert.equal(inv.records[0].createTime, '2026-08-09T00:00:00.123456789Z');
});
for (const extra of [{ windowStartInclusive: '2026-05-09T00:00:00Z' }, { windowStartInclusive: end }, { windowEndExclusive: start }]) {
  test(`rejects widened or empty window ${JSON.stringify(extra)}`, () => assert.throws(() => build([], extra), /invalid_window/));
}
test('includes replies with an older thread root outside the audit window', () => {
  const inv = build([record(1, { createTime: '2026-08-07T00:00:00Z' }), record(2)]);
  assert.deepEqual(inv.counts, { input: 2, unique: 2, duplicates: 0, inWindow: 1, outsideWindow: 1 });
});
test('preserves distinct message identities even for identical body hashes', () => assert.equal(build([record(1), record(2)]).counts.inWindow, 2));
test('counts exact page replay without manufacturing new messages', () => assert.deepEqual(build([record(), record()]).counts,
  { input: 2, unique: 1, duplicates: 1, inWindow: 1, outsideWindow: 0 }));
test('normalizes equivalent timestamp spellings before replay comparison', () => {
  const inv = build([record(1, { createTime: '2026-08-09T00:00:00Z' }), record(1, { createTime: '2026-08-09T00:00:00.000Z' })]);
  assert.equal(inv.counts.duplicates, 1);
});
for (const extra of [{ bodySha256: 'b'.repeat(64) }, { attachmentCount: 1 }, { deleted: true },
  { lastUpdateTime: '2026-08-10T00:00:00Z' }, { threadName: 'spaces/AAQAoHKdzvI/threads/other' }]) {
  test(`rejects conflicting duplicate record ${Object.keys(extra)[0]}`, () => assert.throws(() => build([record(), record(1, extra)]), /conflicting_message_version/));
}
test('rejects conflicting duplicates even outside the selected date window', () => assert.throws(() => build([
  record(1, { createTime: '2026-08-07T00:00:00Z' }), record(1, { createTime: '2026-08-07T00:00:00Z', bodySha256: 'b'.repeat(64) })]), /conflicting_message_version/));
for (const extra of [{ messageName: 'spaces/OTHER/messages/m.m' }, { threadName: 'spaces/OTHER/threads/a' },
  { attachmentCount: -1 }, { attachmentCount: 101 }, { deleted: 'false' }, { bodySha256: 'not-a-hash' },
  { lastUpdateTime: '2026-08-01T00:00:00Z' }, { createTime: '2026-09-09T00:00:00Z' }]) {
  test(`rejects invalid descriptor ${JSON.stringify(extra)}`, () => assert.throws(() => build([record(1, extra)])));
}
for (const value of [undefined, 'a'.repeat(64), new Uint8Array(31), new Uint8Array(65)]) {
  test(`rejects invalid provenance key ${value?.length}`, () => assert.throws(() => build([], { provenanceKey: value }), /invalid_provenance_key/));
}
test('unknown fields produce a value-free diagnostic', () => {
  assert.throws(() => build([record(1, { SECRET_VALUE_DO_NOT_PRINT: 'do not echo' })]), (e) => e.message === 'unknown_field');
});
test('validates all inputs before date filtering', () => assert.throws(() => build([record(1, { createTime: '2026-08-07T00:00:00Z', bodySha256: 'bad' })]), /invalid_digest/));
test('is independent of input order and does not mutate descriptors', () => {
  const records = [record(2), record(1)]; const original = structuredClone(records);
  assert.deepEqual(build(records), build([...records].reverse()));
  assert.deepEqual(records, original);
});
test('keyed provenance does not emit raw resource names or body hashes', () => {
  const text = JSON.stringify(build());
  assert.ok(!text.includes('/messages/')); assert.ok(!text.includes('/threads/'));
  assert.ok(!text.includes('bodySha256'));
  assert.notEqual(build().records[0].versionDigest, H);
});
test('distinct audit keys unlink message and version identifiers', () => {
  const other = build([record()], { provenanceKey: new Uint8Array(32).fill(8) });
  assert.notEqual(build().records[0].messageKey, other.records[0].messageKey);
  assert.notEqual(build().records[0].versionDigest, other.records[0].versionDigest);
});
test('empty input is never a completion claim', () => {
  const out = reconcile(build([]));
  assert.equal(out.accountingComplete, false); assert.equal(out.completionClaimAllowed, false);
});
test('records with no review remain individually visible and unreviewed', () => {
  const out = reconcile(build([record(1), record(2)]), []);
  assert.equal(out.counts.unreviewed, 2); assert.equal(out.records[0].disposition, 'unreviewed');
});
test('all links remain annotations, not proof of implementation or issue coverage', () => {
  const out = reconcile(build());
  assert.equal(out.accountingComplete, true); assert.equal(out.counts.requirementsWithIssueLinks, 1);
  assert.equal(out.counts.requirementsWithImplementationPrLinks, 1);
  for (const field of ['sourceCompletenessIndependentlyVerified', 'issueCoverageVerified', 'implementationCoverageVerified', 'completionClaimAllowed']) assert.equal(out[field], false);
});
test('an audit PR does not count as an implementation PR', () => {
  const inv = build(); const r = requirement(); r.pullRequests[0].kind = 'audit';
  assert.equal(reconcile(inv, [entry(inv.records[0], { requirements: [r] })]).counts.requirementsWithImplementationPrLinks, 0);
});
test('retained ledger never becomes a live complete export', () => {
  const inv = build([record()], { source: { ...source, kind: 'retained-ledger' } });
  assert.equal(reconcile(inv).sourceCompletenessReported, false);
});
test('incomplete pagination never becomes a complete source', () => {
  assert.equal(reconcile(build([record()], { source: { ...source, paginationComplete: false } })).sourceCompletenessReported, false);
});
test('a snapshot before the requested end leaves the source incomplete', () => {
  assert.equal(reconcile(build([record()], { source: { ...source, snapshotTime: '2026-08-10T00:00:00Z' } })).sourceCompletenessReported, false);
});
test('metadata-only images remain attachment blockers even on acknowledgement messages', () => {
  const inv = build([record(1, { attachmentCount: 3 })]);
  const out = reconcile(inv, [entry(inv.records[0], { disposition: 'acknowledgement', requirements: [] })]);
  assert.equal(out.counts.unreviewedAttachments, 3); assert.equal(out.attachmentReviewRecorded, false);
});
test('recorded attachment review requires a digest, without claiming binary verification', () => {
  const inv = build([record(1, { attachmentCount: 1 })]);
  assert.throws(() => reconcile(inv, [entry(inv.records[0], { attachmentReview: { reviewedCount: 1, evidenceDigest: null } })]), /invalid_digest/);
  const out = reconcile(inv, [entry(inv.records[0], { attachmentReview: { reviewedCount: 1, evidenceDigest: H } })]);
  assert.equal(out.attachmentReviewRecorded, true); assert.equal(out.completionClaimAllowed, false);
});
test('rejects an attachment review count beyond available metadata', () => {
  const inv = build(); assert.throws(() => reconcile(inv, [entry(inv.records[0], { attachmentReview: { reviewedCount: 1, evidenceDigest: H } })]), /invalid_attachment_count/);
});
test('deleted tombstones preserve an unavailable-content counter', () => {
  const inv = build([record(1, { deleted: true })]);
  assert.throws(() => reconcile(inv), /deleted_disposition_mismatch/);
  assert.equal(reconcile(inv, [entry(inv.records[0], { disposition: 'deleted', requirements: [] })]).counts.deletedContentUnavailable, 1);
});
test('requires one or more explicit requirement rows for actionable messages', () => {
  const inv = build(); assert.throws(() => reconcile(inv, [entry(inv.records[0], { requirements: [] })]), /invalid_requirement_disposition/);
});
test('keeps multiple requirements per message rather than one issue per prompt', () => {
  const inv = build(); const out = reconcile(inv, [entry(inv.records[0], { requirements: [requirement(), requirement({ key: 'REQ_2', issues: [], pullRequests: [] })] })]);
  assert.equal(out.counts.requirements, 2); assert.equal(out.counts.requirementsWithIssueLinks, 1);
});
test('does not allow requirements on excluded context', () => {
  const inv = build(); assert.throws(() => reconcile(inv, [entry(inv.records[0], { disposition: 'context' })]), /invalid_requirement_disposition/);
});
test('rejects duplicate review rows', () => {
  const inv = build(); const e = entry(inv.records[0]); assert.throws(() => reconcile(inv, [e, e]), /duplicate_review_message/);
});
test('rejects unknown review rows', () => {
  const inv = build(); assert.throws(() => reconcile(inv, [entry(inv.records[0], { messageKey: H })]), /unknown_review_message/);
});
test('rejects stale review of an edited message', () => {
  const inv = build(); assert.throws(() => reconcile(inv, [entry(inv.records[0], { versionDigest: H })]), /stale_message_review/);
});
test('rejects review copied from another export/window', () => {
  const inv = build(); const r = review(inv); r.inventoryDigest = H;
  assert.throws(() => reconcileMessageCoverage({ inventory: inv, review: r }), /review_inventory_mismatch/);
});
test('rejects tampered inventory bytes even with plausible counts', () => {
  const inv = build(); inv.records[0].attachmentCount = 1; assert.throws(() => reconcile(inv), /inventory_digest_mismatch/);
});
test('rejects invalid inventory count reconciliation', () => {
  const inv = build(); inv.counts.unique = 2; assert.throws(() => reconcile(inv), /invalid_inventory_counts/);
});
test('rejects a forged implementation-complete field instead of copying it', () => {
  const inv = build(); const r = review(inv); r.implementationCoverageVerified = true;
  assert.throws(() => reconcileMessageCoverage({ inventory: inv, review: r }), /unknown_field/);
});
for (const url of ['https://github.com/test-org/test-repo/pull/1', 'https://github.com/a/b/issues/1?secret=x',
  'https://github.com/a/b/issues/1#comment', 'https://github.com/a/b/issues/1trailing',
  'https://github.com/a/../issues/1', 'https://github.com/a/%2e%2e/issues/1',
  'https://u:p@github.com/a/b/issues/1', 'https://github.com.evil.invalid/a/b/issues/1',
  'http://github.com/a/b/issues/1', 'https://github.com/a/b/issues/0', 'https://github.com/a/b/issues/999999999999999999999']) {
  test(`rejects unsafe/wrong-kind issue reference ${url}`, () => {
    const inv = build(); assert.throws(() => reconcile(inv, [entry(inv.records[0], { requirements: [requirement({ issues: [url] })] })]), /invalid_reference/);
  });
}
test('rejects mutable branch names in place of exact PR head SHA', () => {
  const inv = build(); const r = requirement(); r.pullRequests[0].headSha = 'main';
  assert.throws(() => reconcile(inv, [entry(inv.records[0], { requirements: [r] })]), /invalid_head/);
});
test('rejects duplicate requirements and conflicting cross-message links', () => {
  const inv = build([record(1), record(2)]);
  assert.throws(() => reconcile(inv, [entry(inv.records[0], { requirements: [requirement(), requirement()] })]), /duplicate_requirement/);
  assert.throws(() => reconcile(inv, [entry(inv.records[0]), entry(inv.records[1], { requirements: [requirement({ issues: [] })] })]), /conflicting_requirement_links/);
});
test('review order does not change receipt identity', () => {
  const inv = build([record(1), record(2)]); const entries = inv.records.map((r) => entry(r));
  assert.deepEqual(reconcile(inv, entries), reconcile(inv, [...entries].reverse()));
});
function superseding(inv, from, to) {
  return entry(inv.records[from], { disposition: 'superseded', requirements: [], supersededBy: inv.records[to].messageKey });
}
test('superseded messages inherit requirements but remain individually accounted', () => {
  const inv = build([record(1), record(2), record(3)]);
  const out = reconcile(inv, [superseding(inv, 0, 1), superseding(inv, 1, 2), entry(inv.records[2])]);
  assert.equal(out.counts.superseded, 2); assert.equal(out.counts.requirements, 1);
  assert.deepEqual(out.records[0].requirementKeys, ['REQ_1']);
});
test('supersession cannot hide an unreviewed target', () => {
  const inv = build([record(1), record(2)]); assert.throws(() => reconcile(inv, [superseding(inv, 0, 1)]), /unreviewed_supersession_target/);
});
test('supersession cannot terminate in a nonactionable acknowledgement', () => {
  const inv = build([record(1), record(2)]);
  assert.throws(() => reconcile(inv, [superseding(inv, 0, 1), entry(inv.records[1], { disposition: 'acknowledgement', requirements: [] })]), /nonactionable_supersession_target/);
});
test('rejects cyclic supersession', () => {
  const inv = build([record(1), record(2)]);
  assert.throws(() => reconcile(inv, [superseding(inv, 0, 1), superseding(inv, 1, 0)]), /supersession_cycle/);
});
test('rejects self supersession', () => {
  const inv = build(); assert.throws(() => reconcile(inv, [superseding(inv, 0, 0)]), /invalid_supersession/);
});
test('a superseded image still needs its own attachment review', () => {
  const inv = build([record(1, { attachmentCount: 1 }), record(2)]);
  const imageIndex = inv.records.findIndex((r) => r.attachmentCount === 1);
  const other = 1 - imageIndex;
  const out = reconcile(inv, [superseding(inv, imageIndex, other), entry(inv.records[other])]);
  assert.equal(out.counts.unreviewedAttachments, 1);
});
test('source identity changes invalidate all prior review bindings', () => {
  const old = build(); const fresh = build([record()], { source: { ...source, artifactSha256: 'c'.repeat(64) } });
  assert.throws(() => reconcileMessageCoverage({ inventory: fresh, review: review(old) }), /review_inventory_mismatch/);
});
test('blocks spoofed schema versions', () => {
  const inv = build(); const r = review(inv); r.schemaVersion = 2;
  assert.throws(() => reconcileMessageCoverage({ inventory: inv, review: r }), /review_inventory_mismatch/);
});
test('rejects excessive descriptor inventory before iterating', () => assert.throws(() => build(Array(100001)), /invalid_array/));
test('a blocked canonical message stays blocked when aliases refer to it', () => {
  const inv = build([record(1), record(2)]);
  const out = reconcile(inv, [superseding(inv, 0, 1), entry(inv.records[1], { disposition: 'blocked' })]);
  assert.equal(out.counts.blocked, 1); assert.equal(out.completionClaimAllowed, false);
});
test('handles long supersession chains without recursive stack overflow', () => {
  const inv = build(Array.from({ length: 2000 }, (_, i) => record(i + 1)));
  const entries = inv.records.map((row, i) => i + 1 === inv.records.length ? entry(row) : superseding(inv, i, i + 1));
  const out = reconcile(inv, entries);
  assert.equal(out.counts.superseded, 1999); assert.equal(out.counts.requirements, 1);
});
test('permutation property: equal input/review sets produce equal receipts', () => {
  const originals = Array.from({ length: 30 }, (_, i) => record(i + 1));
  const baseline = reconcile(build(originals));
  for (let i = 1; i < 30; i += 1) {
    const rotated = originals.slice(i).concat(originals.slice(0, i));
    const inv = build(rotated); const entries = inv.records.map((row) => entry(row)).reverse();
    assert.deepEqual(reconcile(inv, entries), baseline);
  }
});
test('exhausts all declared dispositions without accidental completion', () => {
  for (const disposition of ['context', 'credential_only', 'private', 'reference', 'acknowledgement']) {
    const inv = build(); const out = reconcile(inv, [entry(inv.records[0], { disposition, requirements: [] })]);
    assert.equal(out.counts.reviewed, 1); assert.equal(out.counts.requirements, 0);
    assert.equal(out.completionClaimAllowed, false);
  }
});
