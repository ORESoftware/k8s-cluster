import assert from 'node:assert/strict';
import test from 'node:test';

import { materializePlan } from '../reaper-core.mjs';

function candidate(overrides = {}) {
  return {
    candidateKey: 'google-chat:AAQAoHKdzvI:111111111111111111111111',
    action: 'manual-review',
    title: 'Ambiguous multi-project request',
    sourceKeys: ['google-chat:AAQAoHKdzvI:spaces/AAQAoHKdzvI/messages/abc.abc'],
    messageCount: 1,
    substantiveMessageCount: 1,
    firstCreateTime: '2026-08-19T00:00:00.000Z',
    lastCreateTime: '2026-08-19T00:00:00.000Z',
    githubReferences: { repositories: [], organizations: [] },
    exactExistingIssues: [],
    ...overrides,
  };
}

function plan(item) {
  return {
    planId: 'google-chat-import-plan:222222222222222222222222',
    candidates: [item],
  };
}

function neverCalledDependencies() {
  let calls = 0;
  const fail = async () => {
    calls += 1;
    throw new Error('external dependency must not be called');
  };
  return {
    calls: () => calls,
    dependencies: {
      linear: {
        getIssue: fail,
        findByCandidate: fail,
        createIssue: fail,
        appendSection: fail,
      },
      github: { findEvidence: fail },
    },
  };
}

async function assertExcluded(item, reasonCode) {
  const guard = neverCalledDependencies();
  const result = await materializePlan(plan(item), guard.dependencies);
  assert.equal(guard.calls(), 0);
  assert.deepEqual(result.evidence.entries, [{
    candidateKey: item.candidateKey,
    disposition: 'excluded',
    reasonCode,
  }]);
  assert.equal(result.summary.counts.linearCreated, 0);
  assert.equal(result.summary.counts.linearUpdated, 0);
  assert.equal(result.summary.counts.linearReused, 0);
  assert.equal(result.summary.counts.excluded, 1);
}

test('privacy-bearing review candidates are excluded before Linear mutation', async () => {
  await assertExcluded(
    candidate({ action: 'create', title: 'Contact alex@example.com at +1 (512) 555-1212' }),
    'privacy_sensitive',
  );
});

test('immigration-evasion review candidates are excluded before Linear mutation', async () => {
  await assertExcluded(
    candidate({ title: 'Immigration issue: bypass the visa or border process' }),
    'unsafe_or_deceptive',
  );
});

test('deceptive remote-location review candidates are excluded before Linear mutation', async () => {
  await assertExcluded(
    candidate({ title: 'Remote in any country: pretend the worker is somewhere else' }),
    'unsafe_or_deceptive',
  );
});

test('agent-coordination-only review candidates are excluded before Linear mutation', async () => {
  await assertExcluded(
    candidate({ title: 'ok coordinate with other agents but do not duplicate work' }),
    'context_only',
  );
});

test('valid ambiguous engineering candidates still receive one quarantined Linear owner', async () => {
  let creates = 0;
  const issue = {
    id: 'issue-1',
    identifier: 'DEN-9001',
    title: '[Google Chat review] Ambiguous multi-project request',
    description: '',
    attachments: [],
  };
  const linear = {
    async getIssue() { return null; },
    async findByCandidate() { return []; },
    async createIssue({ description }) {
      creates += 1;
      issue.description = description;
      return issue;
    },
    async appendSection() { throw new Error('must not update'); },
  };
  const github = {
    async findEvidence() { throw new Error('must not query GitHub for quarantine'); },
  };
  const result = await materializePlan(plan(candidate()), { linear, github });
  assert.equal(creates, 1);
  assert.equal(result.evidence.entries[0].disposition, 'quarantined');
  assert.equal(result.evidence.entries[0].reasonCode, 'requires_human_review');
  assert.equal(result.summary.counts.quarantined, 1);
});
