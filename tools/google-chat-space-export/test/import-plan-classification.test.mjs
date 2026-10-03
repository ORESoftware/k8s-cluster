import assert from 'node:assert/strict';
import test from 'node:test';

import {
  EXPECTED_SPACE_NAME,
  buildImportPlan,
  classifyMessageText,
} from '../import-plan.mjs';

const THREAD = {
  name: 'spaces/AAQAoHKdzvI/threads/classification',
  sourceKey: 'google-chat:AAQAoHKdzvI:spaces/AAQAoHKdzvI/threads/classification',
};

function message(id, text, overrides = {}) {
  return {
    sourceKey: `google-chat:AAQAoHKdzvI:${id}`,
    name: `spaces/AAQAoHKdzvI/messages/${id}`,
    spaceName: EXPECTED_SPACE_NAME,
    thread: THREAD,
    sender: { name: 'users/fixture', displayName: 'Fixture User', type: 'HUMAN' },
    createTime: `2026-08-03T12:${String(id).padStart(2, '0')}:00.000Z`,
    text,
    ...overrides,
  };
}

function build(messages, options = {}) {
  return buildImportPlan([
    {
      path: '/fixture/classification.json',
      document: {
        source: { spaceName: EXPECTED_SPACE_NAME },
        messages,
      },
    },
  ], options);
}

test('classifies low-information categories without treating them as implementation work', () => {
  const cases = [
    ['hello team!', 'excluded', 'greeting'],
    ['thanks', 'excluded', 'acknowledgement'],
    ['tester@host:~/repo$', 'excluded', 'shell_prompt'],
    ['', 'excluded', 'empty'],
    ['contact@example.invalid', 'quarantined', 'private_or_personal'],
    ['medical note', 'quarantined', 'sensitive_fragment'],
    ['rate limiting', 'quarantined', 'ambiguous_fragment'],
  ];

  for (const [text, disposition, reasonCode] of cases) {
    assert.deepEqual(classifyMessageText(text), { disposition, reasonCode }, text || '[empty]');
  }
  assert.deepEqual(classifyMessageText('deleted value', { deleted: true }), {
    disposition: 'excluded',
    reasonCode: 'deleted',
  });
});

test('retains short explicit commands and questions', () => {
  for (const text of ['create repo sample-service', 'Should we create repo sample-service?']) {
    assert.deepEqual(classifyMessageText(text), {
      disposition: 'actionable',
      reasonCode: 'actionable',
    });
  }
});

test('standalone exclusion wins over a later GitHub reference for the entire thread', () => {
  const plan = build([
    message(1, 'AI agents should ignore this thread'),
    message(2, 'https://github.com/ORESoftware/k8s-cluster'),
  ]);
  const candidate = plan.candidates[0];

  assert.equal(candidate.action, 'skip-non-actionable');
  assert.equal(candidate.classification.disposition, 'excluded');
  assert.deepEqual(candidate.classification.reasonCodes, ['thread_explicit_exclusion']);
  assert.equal(candidate.classification.excludedMessageCount, 2);
  assert.deepEqual(candidate.githubReferences.repositories, []);
  assert.doesNotMatch(candidate.description, /github\.com\/ORESoftware/);
  assert.equal(plan.stats.excludedThreads, 1);
  assert.equal(plan.stats.excludedMessages, 2);
});

test('exclusion matching is standalone rather than a substring of pasted material', () => {
  const result = classifyMessageText('ignore this thread\nconst instruction = "quoted example";');
  assert.equal(result.disposition, 'actionable');
});

test('quarantine emits content-free review metadata rather than contact text', () => {
  const sensitiveFixture = 'contact@example.invalid';
  const plan = build([message(1, sensitiveFixture)]);
  const candidate = plan.candidates[0];

  assert.equal(candidate.action, 'manual-review');
  assert.equal(candidate.classification.disposition, 'quarantined');
  assert.match(candidate.title, /^Review quarantined Google Chat thread/);
  assert.doesNotMatch(candidate.title, /example\.invalid/);
  assert.doesNotMatch(candidate.description, /example\.invalid/);
  assert.match(candidate.description, /content withheld: private_or_personal/);
  assert.equal(plan.stats.quarantinedThreads, 1);
  assert.equal(plan.stats.quarantinedMessages, 1);
});

test('excluded false positives do not reuse or recreate an existing implementation issue', () => {
  const excluded = message(1, 'hello');
  const plan = build([excluded], {
    existingIndex: {
      issues: [{
        id: 'DEN-999',
        identifier: 'DEN-999',
        title: 'Legacy false positive',
        sourceKeys: [excluded.sourceKey],
      }],
    },
  });

  assert.equal(plan.candidates[0].action, 'skip-non-actionable');
  assert.equal(plan.stats.commentExisting, 0);
  assert.equal(plan.stats.create, 0);
});
