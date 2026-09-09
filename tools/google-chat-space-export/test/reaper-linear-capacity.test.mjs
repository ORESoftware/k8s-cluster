import assert from 'node:assert/strict';
import test from 'node:test';

import { assertCoverageEvidenceContract } from '../reaper-contracts.mjs';
import { createLinearClient } from '../reaper-linear.mjs';
import { buildLinearCapacityEvidence } from '../reaper-materializer.mjs';

const PLAN_ID = 'google-chat-import-plan:cccccccccccccccccccccccc';
const ACTIONABLE_KEY = 'google-chat:AAQAoHKdzvI:aaaaaaaaaaaaaaaaaaaaaaaa';
const NON_ACTIONABLE_KEY = 'google-chat:AAQAoHKdzvI:bbbbbbbbbbbbbbbbbbbbbbbb';

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

test('capacity fallback rejects malformed plan identifiers before writing evidence', () => {
  assert.throws(
    () => buildLinearCapacityEvidence({ planId: 'not-a-plan', candidates: [] }),
    /plan\.planId is invalid/,
  );
});
