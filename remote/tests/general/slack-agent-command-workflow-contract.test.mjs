import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';

const workflowPath = path.resolve(
  process.cwd(),
  '.github/workflows/slack-agent-command-browser-e2e.yml',
);
const workflow = readFileSync(workflowPath, 'utf8');
const expectedBridge = '63d4d9be4343d3eeba02205ec64dd443716c5249';
const expectedCoordinator = 'f0adef6f384bac8024da59a26cb05e2fb9caac98';

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

test('cross-repository Slack canary dependencies are current, immutable, and provenance-aligned', () => {
  const bridge = checkoutRef('ORESoftware/ai-agent-bridge.rs');
  const coordinator = checkoutRef('ORESoftware/ai-agent-coordinator.rs');

  assert.equal(bridge, expectedBridge);
  assert.equal(coordinator, expectedCoordinator);
  assert.equal(recordedRef('BRIDGE_REF'), bridge);
  assert.equal(recordedRef('COORDINATOR_REF'), coordinator);
  assert.doesNotMatch(workflow, /inputs\.(?:bridge_ref|coordinator_ref)/);
  assert.doesNotMatch(
    workflow,
    /^\s*ref:\s*(?:main|master|dev|agent\/|feature\/|feat\/|fix\/)/m,
    'cross-repository checkouts must not use mutable branch refs',
  );
});

test('recorded revisions are verified against checked-out commits before evidence upload', () => {
  assert.match(workflow, /bridge_commit="\$\(git -C \.e2e\/bridge rev-parse HEAD\)"/);
  assert.match(
    workflow,
    /coordinator_commit="\$\(git -C \.e2e\/coordinator rev-parse HEAD\)"/,
  );
  assert.match(workflow, /test "\$bridge_commit" = "\$BRIDGE_REF"/);
  assert.match(workflow, /test "\$coordinator_commit" = "\$COORDINATOR_REF"/);
  assert.match(workflow, /schema_version:\s*4/);
  assert.match(workflow, /idempotency_contract:\s*"header_equals_payload_run_id"/);
});

test('the workflow fails closed on stale Slack run idempotency source', () => {
  assert.match(
    workflow,
    /grep -F '\.header\("idempotency-key", &request\.run_id\)'/,
  );
  assert.match(workflow, /grep -F 'slack-command:\{\}'/);
  assert.match(workflow, /bridge still prefixes the Slack run idempotency key/);
  assert.match(workflow, /grep -R -F 'payload\.run_id' \.e2e\/coordinator\/src/);
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
