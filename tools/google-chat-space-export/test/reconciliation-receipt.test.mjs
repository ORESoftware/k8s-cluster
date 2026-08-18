import assert from 'node:assert/strict';
import test from 'node:test';

import { computePlanId } from '../import-plan.mjs';
import { buildReconciliationReceipt } from '../reconciliation-receipt.mjs';

const CANDIDATE_A1 = 'google-chat:AAQAoHKdzvI:aaaaaaaaaaaaaaaaaaaaaaaa';
const CANDIDATE_A2 = 'google-chat:AAQAoHKdzvI:bbbbbbbbbbbbbbbbbbbbbbbb';
const CANDIDATE_A3 = 'google-chat:AAQAoHKdzvI:cccccccccccccccccccccccc';

const plan = {
  schemaVersion: 1,
  planId: '',
  source: {
    spaceName: 'spaces/AAQAoHKdzvI',
    spaceId: 'AAQAoHKdzvI',
    windowStartInclusive: '2026-07-27T00:00:00.000Z',
    windowEndExclusive: '2026-08-11T00:00:00.000Z',
  },
  stats: { plannedMessages: 4 },
  candidates: [
    {
      candidateKey: CANDIDATE_A1,
      action: 'create',
      messageCount: 2,
      title: 'private prompt text must never enter the receipt',
      sourceKeys: ['google-chat:AAQAoHKdzvI:message-secret'],
    },
    {
      candidateKey: CANDIDATE_A2,
      action: 'comment-existing',
      messageCount: 1,
    },
    {
      candidateKey: CANDIDATE_A3,
      action: 'skip-non-actionable',
      messageCount: 1,
    },
  ],
};
plan.planId = computePlanId({
  spaceName: plan.source.spaceName,
  windowStartInclusive: plan.source.windowStartInclusive,
  windowEndExclusive: plan.source.windowEndExclusive,
  candidates: plan.candidates,
});

function completeEvidence() {
  return {
    schemaVersion: 1,
    planId: plan.planId,
    entries: [
      {
        candidateKey: CANDIDATE_A1,
        disposition: 'covered',
        linearIssues: ['DEN-3473'],
        pullRequests: ['ORESoftware/k8s-cluster#1307'],
      },
      {
        candidateKey: CANDIDATE_A2,
        disposition: 'covered',
        linearIssues: ['DEN-3420', 'DEN-3472'],
        defaultBranchCommits: ['zed-pkg/zed-web-server.rs@0123456789abcdef0123456789abcdef01234567'],
      },
    ],
  };
}

test('emits a complete, content-free receipt with one or two Linear issues per covered prompt', () => {
  const receipt = buildReconciliationReceipt(plan, completeEvidence());

  assert.deepEqual(receipt.counts, {
    scanned: 4,
    actionable: 3,
    covered: 3,
    excluded: 1,
    quarantined: 0,
    gaps: 0,
    candidates: {
      total: 3,
      actionable: 2,
      covered: 2,
      excluded: 1,
      quarantined: 0,
      gaps: 0,
    },
    complete: true,
  });
  assert.equal(receipt.source.windowEndExclusive, '2026-08-11T00:00:00.000Z');
  assert.match(receipt.receiptId, /^google-chat-reconciliation-receipt:[0-9a-f]{24}$/);
  const serialized = JSON.stringify(receipt);
  assert.doesNotMatch(serialized, /private prompt text|message-secret/);
  assert.doesNotMatch(serialized, /token|password|description|title/i);
  assert.deepEqual(
    buildReconciliationReceipt(plan, completeEvidence()),
    receipt,
    'rerunning the same exact window must be byte-stable',
  );
});

test('reports missing implementation evidence as a gap without copying prompt content', () => {
  const evidence = completeEvidence();
  delete evidence.entries[0].pullRequests;
  const receipt = buildReconciliationReceipt(plan, evidence);
  const entry = receipt.dispositions.find((item) => item.candidateKey === CANDIDATE_A1);

  assert.equal(receipt.counts.complete, false);
  assert.equal(receipt.counts.gaps, 2);
  assert.equal(entry.disposition, 'gap');
  assert.equal(entry.reasonCode, 'missing_implementation_evidence');
});

test('reports an actionable candidate with no evidence as a gap', () => {
  const evidence = completeEvidence();
  evidence.entries = evidence.entries.filter((entry) => entry.candidateKey !== CANDIDATE_A2);
  const receipt = buildReconciliationReceipt(plan, evidence);
  assert.equal(receipt.counts.gaps, 1);
  assert.ok(receipt.dispositions.some((entry) => entry.reasonCode === 'missing_evidence'));
});

test('rejects more than two Linear issues and unknown freeform fields', () => {
  const tooMany = completeEvidence();
  tooMany.entries[0].linearIssues = ['DEN-1', 'DEN-2', 'DEN-3'];
  assert.throws(() => buildReconciliationReceipt(plan, tooMany), /at most 2/);

  const withPrompt = completeEvidence();
  withPrompt.entries[0].promptText = 'copying source text must fail';
  assert.throws(() => buildReconciliationReceipt(plan, withPrompt), /forbidden field promptText/);
});

test('rejects mismatched plans, duplicate entries, and unapproved exclusion reasons', () => {
  const mismatch = completeEvidence();
  mismatch.planId = 'google-chat-import-plan:222222222222222222222222';
  assert.throws(() => buildReconciliationReceipt(plan, mismatch), /does not match/);

  const duplicate = completeEvidence();
  duplicate.entries.push({ ...duplicate.entries[0] });
  assert.throws(() => buildReconciliationReceipt(plan, duplicate), /duplicate evidence entry/);

  const badReason = completeEvidence();
  badReason.entries[0] = {
    candidateKey: badReason.entries[0].candidateKey,
    disposition: 'excluded',
    reasonCode: 'because_i_said_so',
  };
  assert.throws(() => buildReconciliationReceipt(plan, badReason), /not allowed for excluded/);
});

test('rejects non-canonical identifiers, non-15-day windows, and incomplete message accounting', () => {
  const contentBearingKey = structuredClone(plan);
  contentBearingKey.candidates[0].candidateKey = 'google-chat:AAQAoHKdzvI:raw-prompt-text';
  assert.throws(
    () => buildReconciliationReceipt(contentBearingKey, completeEvidence()),
    /content-free candidateKey/,
  );

  const wrongWindow = structuredClone(plan);
  wrongWindow.source.windowEndExclusive = '2026-08-10T00:00:00.000Z';
  assert.throws(
    () => buildReconciliationReceipt(wrongWindow, completeEvidence()),
    /exactly 15 days/,
  );

  const incompleteAccounting = structuredClone(plan);
  incompleteAccounting.stats.plannedMessages = 5;
  assert.throws(
    () => buildReconciliationReceipt(incompleteAccounting, completeEvidence()),
    /message counts do not equal/,
  );

  const reboundCandidate = structuredClone(plan);
  reboundCandidate.candidates[0].messageCount = 3;
  reboundCandidate.stats.plannedMessages = 5;
  assert.throws(
    () => buildReconciliationReceipt(reboundCandidate, completeEvidence()),
    /planId does not match/,
  );
});

test('allows fail-closed quarantine but never counts it as covered', () => {
  const evidence = completeEvidence();
  evidence.entries[0] = {
    candidateKey: evidence.entries[0].candidateKey,
    disposition: 'quarantined',
    reasonCode: 'sensitive_content',
  };
  const receipt = buildReconciliationReceipt(plan, evidence);
  assert.equal(receipt.counts.quarantined, 2);
  assert.equal(receipt.counts.covered, 1);
  assert.equal(receipt.counts.gaps, 0);
});
