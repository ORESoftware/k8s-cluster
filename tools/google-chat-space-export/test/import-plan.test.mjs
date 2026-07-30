import assert from 'node:assert/strict';
import test from 'node:test';

import {
  EXPECTED_SPACE_NAME,
  START_TIME_INCLUSIVE,
  buildImportPlan,
  renderMarkdown,
} from '../import-plan.mjs';

function message(overrides = {}) {
  const id = overrides.id || 'm1';
  return {
    sourceKey: `google-chat:AAQAoHKdzvI:${id}`,
    name: `spaces/AAQAoHKdzvI/messages/${id}`,
    spaceName: EXPECTED_SPACE_NAME,
    thread: {
      name: 'spaces/AAQAoHKdzvI/threads/t1',
      sourceKey: 'google-chat:AAQAoHKdzvI:spaces/AAQAoHKdzvI/threads/t1',
    },
    sender: {
      name: 'users/123',
      displayName: 'Alex',
      type: 'HUMAN',
    },
    createTime: '2026-05-11T12:00:00.000Z',
    text: 'Audit github.com/ORESoftware/k8s-cluster and create a tested implementation.',
    ...overrides,
  };
}

function emailPage(messages, overrides = {}) {
  return {
    exportVersion: 1,
    runId: 'run-email',
    partNumber: 1,
    source: {
      spaceName: EXPECTED_SPACE_NAME,
      startTimeInclusive: START_TIME_INCLUSIVE,
    },
    messages,
    ...overrides,
  };
}

function bridgePage(messages, overrides = {}) {
  return {
    ok: true,
    data: {
      target: {
        spaceName: EXPECTED_SPACE_NAME,
        startTimeInclusive: START_TIME_INCLUSIVE,
      },
      messages,
      ...overrides,
    },
  };
}

function build(documents, options = {}) {
  return buildImportPlan(
    documents.map((document, index) => ({ path: `/fixture/page-${index + 1}.json`, document })),
    options,
  );
}

test('deduplicates bridge and email wrappers and produces stable candidate keys', () => {
  const substantive = message();
  const acknowledgement = message({
    id: 'm2',
    createTime: '2026-05-11T12:01:00.000Z',
    text: 'ok',
  });
  const documents = [emailPage([substantive, acknowledgement]), bridgePage([substantive])];

  const first = build(documents);
  const second = build([...documents].reverse());

  assert.equal(first.stats.rawMessages, 3);
  assert.equal(first.stats.uniqueMessages, 2);
  assert.equal(first.stats.duplicateMessages, 1);
  assert.equal(first.stats.threads, 1);
  assert.equal(first.candidates.length, 1);
  assert.equal(first.candidates[0].action, 'create');
  assert.equal(first.candidates[0].substantiveMessageCount, 1);
  assert.equal(first.candidates[0].candidateKey, second.candidates[0].candidateKey);
  assert.deepEqual(first.candidates[0].sourceKeys, second.candidates[0].sourceKeys);
});

test('fails closed for wrong-space messages and messages before the boundary', () => {
  assert.throws(
    () =>
      build([
        emailPage([
          message({
            spaceName: 'spaces/WRONG',
          }),
        ]),
      ]),
    /Wrong Google Chat space/,
  );

  assert.throws(
    () =>
      build([
        emailPage([
          message({
            createTime: '2026-05-10T03:59:59.999Z',
          }),
        ]),
      ]),
    /predates/,
  );
});

test('uses explicit repository mapping and proposes commenting on exact existing issue', () => {
  const source = message();
  const plan = build([emailPage([source])], {
    projectMap: {
      repositories: {
        'ORESoftware/k8s-cluster': 'github.com/ORESoftware',
      },
    },
    existingIndex: {
      issues: [
        {
          id: 'DEN-266',
          identifier: 'DEN-266',
          title: 'Import Google Chat messages',
          project: 'github.com/ORESoftware',
          sourceKeys: [source.sourceKey],
        },
      ],
    },
  });

  const candidate = plan.candidates[0];
  assert.equal(candidate.action, 'comment-existing');
  assert.equal(candidate.proposedProject, 'github.com/ORESoftware');
  assert.equal(candidate.projectConfidence, 1);
  assert.deepEqual(candidate.githubReferences.repositories, ['ORESoftware/k8s-cluster']);
  assert.equal(candidate.exactExistingIssues[0].identifier, 'DEN-266');
});

test('skips acknowledgement-only threads', () => {
  const plan = build([
    emailPage([
      message({ text: 'Thanks!' }),
      message({ id: 'm2', createTime: '2026-05-11T12:01:00.000Z', text: '👍' }),
    ]),
  ]);

  assert.equal(plan.stats.skippedNonActionable, 1);
  assert.equal(plan.candidates[0].action, 'skip-non-actionable');
});

test('requires manual review for normalized title duplicates and unresolved projects', () => {
  const source = message({ text: 'Create a reliable export reconciler' });
  const plan = build([bridgePage([source])], {
    existingIndex: [
      {
        identifier: 'DEN-100',
        title: 'Create a reliable export reconciler',
        description: 'Existing work without Google Chat provenance.',
      },
    ],
  });

  const candidate = plan.candidates[0];
  assert.equal(candidate.action, 'manual-review');
  assert.equal(candidate.titleExistingIssues[0].identifier, 'DEN-100');
  assert.ok(candidate.manualReviewReasons.some((reason) => reason.includes('Normalized title')));
  assert.ok(candidate.manualReviewReasons.some((reason) => reason.includes('No explicit GitHub')));
});

test('rejects conflicting duplicate source keys', () => {
  assert.throws(
    () =>
      build([
        emailPage([message({ text: 'First instruction' })]),
        bridgePage([message({ text: 'Conflicting instruction' })]),
      ]),
    /Conflicting duplicate message/,
  );
});

test('renders a reviewable Markdown reconciliation report', () => {
  const plan = build([emailPage([message()])], {
    projectMap: {
      repositories: {
        'ORESoftware/k8s-cluster': 'github.com/ORESoftware',
      },
    },
  });
  const markdown = renderMarkdown(plan);
  assert.match(markdown, /Google Chat → Linear dry-run plan/);
  assert.match(markdown, /Reconciliation summary/);
  assert.match(markdown, /github\.com\/ORESoftware/);
  assert.match(markdown, /google-chat:AAQAoHKdzvI:/);
});
