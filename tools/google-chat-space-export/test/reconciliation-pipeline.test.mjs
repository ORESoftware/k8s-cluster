import assert from 'node:assert/strict';
import test from 'node:test';

import { buildImportPlan, EXPECTED_SPACE_NAME, START_TIME_INCLUSIVE } from '../import-plan.mjs';
import { sanitizeDocuments } from '../sanitize-export.mjs';

function message(id, createTime, text) {
  return {
    sourceKey: `google-chat:AAQAoHKdzvI:${id}`,
    name: `spaces/AAQAoHKdzvI/messages/${id}`,
    spaceName: EXPECTED_SPACE_NAME,
    thread: {
      name: 'spaces/AAQAoHKdzvI/threads/pipeline',
      sourceKey: 'google-chat:AAQAoHKdzvI:spaces/AAQAoHKdzvI/threads/pipeline',
    },
    sender: { name: 'users/123', displayName: 'Alex', type: 'HUMAN' },
    createTime,
    text,
    attachments: [{ name: `spaces/AAQAoHKdzvI/messages/${id}/attachments/1` }],
  };
}

function exportPage(messages) {
  return {
    exportVersion: 1,
    runId: 'pipeline-fixture',
    partNumber: 1,
    source: {
      spaceName: EXPECTED_SPACE_NAME,
      startTimeInclusive: START_TIME_INCLUSIVE,
    },
    messages,
  };
}

test('mandatory sanitizer-to-planner pipeline preserves provenance and removes private content', async () => {
  const secret = ['ghp', 'A'.repeat(36)].join('_');
  const phone = ['WhatsApp', '+1', '415', '555', '0123'].join(' ');
  const documents = [
    {
      filePath: '/fixture/page-1.json',
      document: exportPage([
        message('secret', '2026-06-10T12:00:00.000Z', `Use ${secret} for the migration`),
        message('contact', '2026-06-10T12:01:00.000Z', phone),
        message(
          'safe',
          '2026-06-10T12:02:00.000Z',
          'Audit github.com/ORESoftware/k8s-cluster and add deterministic tests.',
        ),
      ]),
    },
  ];

  const { sanitized, report } = await sanitizeDocuments(documents, {
    since: '2026-06-05T04:00:00.000Z',
  });

  assert.equal(report.messagesSeen, 3);
  assert.equal(report.messagesInWindow, 3);
  assert.equal(report.sensitiveMessages, 1);
  assert.equal(report.privateContactMessages, 1);
  assert.equal(report.safeMessages, 1);
  assert.equal(report.quarantined.length, 2);
  assert.ok(report.quarantined.every((entry) => entry.sourceKey.startsWith('google-chat:')));
  assert.doesNotMatch(JSON.stringify(report), new RegExp(secret));
  assert.doesNotMatch(JSON.stringify(report), /415[\s.-]*555[\s.-]*0123/);

  const outputMessages = sanitized[0].document.messages;
  assert.equal(outputMessages[0].text, '');
  assert.deepEqual(outputMessages[0].attachments, []);
  assert.equal(outputMessages[0].safety.classification, 'sensitive-secret');
  assert.equal(outputMessages[1].text, '');
  assert.deepEqual(outputMessages[1].attachments, []);
  assert.equal(outputMessages[1].safety.classification, 'private-contact');
  assert.match(outputMessages[2].text, /deterministic tests/);

  const plan = buildImportPlan(
    sanitized.map(({ document, filePath }) => ({ document, path: filePath })),
    { since: '2026-06-05T04:00:00.000Z' },
  );
  const rendered = JSON.stringify(plan);

  assert.equal(plan.stats.uniqueMessages, 3);
  assert.equal(plan.stats.plannedMessages, 3);
  assert.equal(plan.stats.windowedOutMessages, 0);
  assert.equal(plan.candidates.length, 1);
  assert.equal(plan.candidates[0].action, 'create');
  assert.doesNotMatch(rendered, new RegExp(secret));
  assert.doesNotMatch(rendered, /415[\s.-]*555[\s.-]*0123/);
});
