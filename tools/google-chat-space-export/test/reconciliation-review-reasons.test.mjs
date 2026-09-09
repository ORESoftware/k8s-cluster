import assert from 'node:assert/strict';
import test from 'node:test';

import { computePlanId } from '../import-plan.mjs';
import { buildReconciliationReceipt } from '../reconciliation-receipt.mjs';

const reasons = [
  'privacy_sensitive',
  'unsafe_or_deceptive',
  'context_only',
];

const candidates = reasons.map((reasonCode, index) => ({
  candidateKey: `google-chat:AAQAoHKdzvI:${String(index + 1).repeat(24)}`,
  action: 'manual-review',
  messageCount: 1,
  reasonCode,
}));

const plan = {
  schemaVersion: 1,
  planId: '',
  source: {
    spaceName: 'spaces/AAQAoHKdzvI',
    spaceId: 'AAQAoHKdzvI',
    windowStartInclusive: '2026-08-24T00:00:00.000Z',
    windowEndExclusive: '2026-09-08T00:00:00.000Z',
  },
  stats: { plannedMessages: candidates.length },
  candidates,
};
plan.planId = computePlanId({
  spaceName: plan.source.spaceName,
  windowStartInclusive: plan.source.windowStartInclusive,
  windowEndExclusive: plan.source.windowEndExclusive,
  candidates: plan.candidates,
});

test('pre-mutation review exclusions produce a complete content-free receipt', () => {
  const evidence = {
    schemaVersion: 1,
    planId: plan.planId,
    entries: candidates.map((candidate) => ({
      candidateKey: candidate.candidateKey,
      disposition: 'excluded',
      reasonCode: candidate.reasonCode,
    })),
  };

  const receipt = buildReconciliationReceipt(plan, evidence);
  assert.equal(receipt.counts.complete, true);
  assert.equal(receipt.counts.excluded, reasons.length);
  assert.equal(receipt.counts.gaps, 0);
  assert.deepEqual(
    receipt.dispositions.map((entry) => entry.reasonCode).sort(),
    [...reasons].sort(),
  );
});
