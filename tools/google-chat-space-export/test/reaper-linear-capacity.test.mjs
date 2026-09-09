import assert from 'node:assert/strict';
import test from 'node:test';

import { computePlanId } from '../import-plan.mjs';
import { assertCoverageEvidenceContract } from '../reaper-contracts.mjs';
import { createLinearClient } from '../reaper-linear.mjs';
import { buildLinearCapacityEvidence } from '../reaper-materializer.mjs';
import { buildReconciliationReceipt } from '../reconciliation-receipt.mjs';

const PLAN_ID = 'google-chat-import-plan:cccccccccccccccccccccccc';
const ACTIONABLE_KEY = 'google-chat:AAQAoHKdzvI:aaaaaaaaaaaaaaaaaaaaaaaa';
const NON_ACTIONABLE_KEY = 'google-chat:AAQAoHKdzvI:bbbbbbbbbbbbbbbbbbbbbbbb';

function capacityPlan() {
  const plan = {
    schemaVersion: 1,
    planId: '',
    source: {
      spaceName: 'spaces/AAQAoHKdzvI',
      spaceId: 'AAQAoHKdzvI',
      windowStartInclusive: '2026-08-25T00:00:00.000Z',
      windowEndExclusive: '2026-09-09T00:00:00.000Z',
    },
    stats: { plannedMessages: 3 },
    candidates: [
      {
        candidateKey: ACTIONABLE_KEY,
        action: 'create',
        messageCount: 2,
        title: 'private source content must not enter capacity evidence',
        sourceKeys: ['google-chat:AAQAoHKdzvI:private-source-key'],
      },
      {
        candidateKey: NON_ACTIONABLE_KEY,
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
  return plan;
}

test('Linear workspace usage-limit failures carry a stable capacity code', async () => {
  let calls = 0;
  const linear = createLinearClient({
    apiKey: 'test-key',
    teamId: 'test-team',
    fetchImpl: async () => {
      calls += 1;
      return new Response(
        JSON.stringify({ errors: [{ message: 'usage limit exceeded' }] }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      );
    },
  });

  await assert.rejects(
    linear.createIssue({ title: 'Capacity probe', description: 'content-free test' }),
    (error) => {
      assert.equal(error.code, 'linear_issue_limit');
      assert.match(error.message, /usage limit exceeded/);
      return true;
    },
  );
  assert.equal(calls, 1);
});

test('capacity fallback never fabricates ownership for actionable candidates', () => {
  const evidence = buildLinearCapacityEvidence({
    planId: PLAN_ID,
    candidates: [
      { candidateKey: ACTIONABLE_KEY, action: 'create' },
      { candidateKey: NON_ACTIONABLE_KEY, action: 'skip-non-actionable' },
    ],
  });

  assert.deepEqual(evidence, {
    schemaVersion: 1,
    planId: PLAN_ID,
    entries: [
      {
        candidateKey: NON_ACTIONABLE_KEY,
        disposition: 'excluded',
        reasonCode: 'non_actionable',
      },
    ],
  });
  assert.doesNotThrow(() => assertCoverageEvidenceContract(evidence));
  assert.equal(
    evidence.entries.some((entry) => entry.candidateKey === ACTIONABLE_KEY),
    false,
    'actionable candidates must remain absent so receipt generation marks them as gaps',
  );
});

test('capacity fallback survives through the final receipt as an explicit content-free gap', () => {
  const plan = capacityPlan();
  const evidence = buildLinearCapacityEvidence(plan);
  const receipt = buildReconciliationReceipt(plan, evidence);
  const actionable = receipt.dispositions.find((entry) => entry.candidateKey === ACTIONABLE_KEY);
  const nonActionable = receipt.dispositions.find((entry) => entry.candidateKey === NON_ACTIONABLE_KEY);

  assert.equal(receipt.counts.complete, false);
  assert.equal(receipt.counts.scanned, 3);
  assert.equal(receipt.counts.actionable, 2);
  assert.equal(receipt.counts.gaps, 2);
  assert.equal(receipt.counts.excluded, 1);
  assert.equal(receipt.counts.candidates.gaps, 1);
  assert.equal(actionable.disposition, 'gap');
  assert.equal(actionable.reasonCode, 'missing_evidence');
  assert.deepEqual(actionable.linearIssues, []);
  assert.deepEqual(actionable.pullRequests, []);
  assert.deepEqual(actionable.defaultBranchCommits, []);
  assert.equal(nonActionable.disposition, 'excluded');
  assert.equal(nonActionable.reasonCode, 'non_actionable');

  const serialized = JSON.stringify(receipt);
  assert.doesNotMatch(serialized, /private source content|private-source-key/);
  assert.deepEqual(buildReconciliationReceipt(plan, evidence), receipt, 'capacity receipt must be deterministic');
});

test('capacity fallback rejects malformed plan identifiers before writing evidence', () => {
  assert.throws(
    () => buildLinearCapacityEvidence({ planId: 'not-a-plan', candidates: [] }),
    /plan\.planId is invalid/,
  );
});
