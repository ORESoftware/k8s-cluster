import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';

const workflowPath = path.resolve(
  process.cwd(),
  '.github/workflows/slack-agent-command-browser-e2e.yml',
);
const browserTestPath = path.resolve(
  process.cwd(),
  'remote/tests/ui/slack-agent-command.playwright.test.mjs',
);
const workflow = readFileSync(workflowPath, 'utf8');
const browserTest = readFileSync(browserTestPath, 'utf8');

const expectedAliases = [
  '/ores-claude',
  '/x-claude',
  '/my-claude',
  '/ores-chatgpt',
  '/x-chatgpt',
  '/my-chatgpt',
];

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function checkoutRef(repository) {
  const block = new RegExp(
    `repository:\\s*${escapeRegExp(repository)}[\\s\\S]{0,400}?\\n\\s*ref:\\s*([0-9a-f]{40})(?:\\s|$)`,
  ).exec(workflow);
  assert.ok(block, `${repository} must be checked out at an exact 40-character commit SHA`);
  return block[1];
}

function recordedRef(name) {
  const match = new RegExp(`\\n\\s*${name}:\\s*([0-9a-f]{40})(?:\\s|$)`).exec(workflow);
  assert.ok(match, `${name} must record an exact 40-character commit SHA`);
  return match[1];
}

test('cross-repository Slack canary dependencies are immutable and provenance-aligned', () => {
  const bridge = checkoutRef('ORESoftware/ai-agent-bridge.rs');
  const coordinator = checkoutRef('ORESoftware/ai-agent-coordinator.rs');

  assert.equal(recordedRef('BRIDGE_REF'), bridge);
  assert.equal(recordedRef('COORDINATOR_REF'), coordinator);
  assert.doesNotMatch(workflow, /inputs\.(?:bridge_ref|coordinator_ref)/);
  assert.doesNotMatch(
    workflow,
    /^\s*ref:\s*(?:main|master|dev|agent\/|feature\/|feat\/|fix\/)/m,
    'cross-repository checkouts must not use mutable branch refs',
  );
});

test('recorded revisions are verified against the checked-out commits before evidence upload', () => {
  assert.match(workflow, /bridge_commit="\$\(git -C \.e2e\/bridge rev-parse HEAD\)"/);
  assert.match(
    workflow,
    /coordinator_commit="\$\(git -C \.e2e\/coordinator rev-parse HEAD\)"/,
  );
  assert.match(workflow, /test "\$bridge_commit" = "\$BRIDGE_REF"/);
  assert.match(workflow, /test "\$coordinator_commit" = "\$COORDINATOR_REF"/);
  assert.match(workflow, /schema_version:\s*5/);
  assert.match(workflow, /alias_contract:\s*"six_manifest_commands_to_two_canonical_endpoints"/);
  assert.match(workflow, /idempotency_contract:\s*"header_equals_payload_run_id"/);
  assert.match(workflow, /observable_event_contract:\s*"validated_sanitized_task_created_v1"/);
});

test('the immutable-ref contract participates in pull-request and push path filters', () => {
  const occurrences = workflow.match(
    /remote\/tests\/general\/slack-agent-command-workflow-contract\.test\.mjs/g,
  );
  assert.ok(occurrences, 'workflow contract path is missing');
  assert.ok(
    occurrences.length >= 3,
    `expected the contract in pull-request, push, and execution blocks; found ${occurrences.length}`,
  );
});

test('browser coverage declares exactly the six installed slash-command aliases', () => {
  const matrix = /const aliasMatrix = Object\.freeze\(\[([\s\S]*?)\]\);/.exec(browserTest);
  assert.ok(matrix, 'browser test must declare one explicit alias matrix');
  const declaredAliases = [...matrix[1].matchAll(/command:\s*'([^']+)'/g)].map(
    (match) => match[1],
  );
  assert.deepEqual(declaredAliases.sort(), [...expectedAliases].sort());
  assert.match(
    browserTest,
    /all six Slack slash-command aliases traverse browser, modal, bridge, coordinator, PostgreSQL, and Slack contracts/,
  );
  assert.match(browserTest, /const modalAlias = alias\('\/my-claude'\)/);
  assert.match(browserTest, /payload\.observable_event/);
  assert.match(browserTest, /'idempotency-key': ids\.run/);
  assert.match(browserTest, /'idempotency-key': `slack-command:\$\{ids\.run\}`/);
});

test('workflow validates alias routing, observable events, and exact run idempotency in checked-out source', () => {
  assert.match(
    workflow,
    /Verify Slack alias, observable-event, and idempotency source contracts/,
  );
  for (const command of expectedAliases) {
    assert.match(workflow, new RegExp(escapeRegExp(command)));
  }
  assert.match(workflow, /observable_event/);
  assert.match(workflow, /Idempotency-Key must equal payload\.run_id/);
});