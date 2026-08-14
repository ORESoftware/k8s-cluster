import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import {
  chicagoUtcOffset,
  shouldRunDailyReconciliation,
} from '../daily-schedule-gate.mjs';

test('selects exactly one UTC schedule for Central daylight and standard time', () => {
  assert.equal(
    shouldRunDailyReconciliation({
      eventName: 'schedule',
      eventSchedule: '0 1 * * *',
      utcOffset: '-0500',
    }),
    true,
  );
  assert.equal(
    shouldRunDailyReconciliation({
      eventName: 'schedule',
      eventSchedule: '0 2 * * *',
      utcOffset: '-0500',
    }),
    false,
  );
  assert.equal(
    shouldRunDailyReconciliation({
      eventName: 'schedule',
      eventSchedule: '0 1 * * *',
      utcOffset: '-0600',
    }),
    false,
  );
  assert.equal(
    shouldRunDailyReconciliation({
      eventName: 'schedule',
      eventSchedule: '0 2 * * *',
      utcOffset: '-0600',
    }),
    true,
  );
});

test('does not depend on runner start minute and resolves real Chicago DST offsets', () => {
  assert.equal(chicagoUtcOffset(new Date('2026-08-11T01:37:00.000Z')), '-0500');
  assert.equal(chicagoUtcOffset(new Date('2026-01-11T02:43:00.000Z')), '-0600');
});

test('runs pull-request and manual validation independent of schedule metadata', () => {
  assert.equal(
    shouldRunDailyReconciliation({
      eventName: 'pull_request',
      eventSchedule: '',
      utcOffset: '-0500',
    }),
    true,
  );
  assert.equal(
    shouldRunDailyReconciliation({
      eventName: 'workflow_dispatch',
      eventSchedule: '',
      utcOffset: '-0600',
    }),
    true,
  );
});

test('fails closed for unexpected cron expressions and time-zone offsets', () => {
  assert.throws(
    () =>
      shouldRunDailyReconciliation({
        eventName: 'schedule',
        eventSchedule: '0 3 * * *',
        utcOffset: '-0600',
      }),
    /Unrecognized/,
  );
  assert.throws(
    () =>
      shouldRunDailyReconciliation({
        eventName: 'schedule',
        eventSchedule: '0 2 * * *',
        utcOffset: '+0000',
      }),
    /Unsupported/,
  );
});

test('workflow gates the reconciliation job instead of only exiting one step', () => {
  const workflow = readFileSync(
    new URL('../../../.github/workflows/google-chat-daily-reconciliation.yml', import.meta.url),
    'utf8',
  );
  assert.match(workflow, /schedule-gate:\n/);
  assert.match(workflow, /needs: schedule-gate/);
  assert.match(workflow, /needs\.schedule-gate\.outputs\.should-run == 'true'/);
  assert.match(workflow, /run: node tools\/google-chat-space-export\/daily-schedule-gate\.mjs/);
  assert.doesNotMatch(workflow, /local_minute|date \+%M/);
});

test('scheduled and manual runs fetch privately, publish only content-free evidence, and fail closed', () => {
  const workflow = readFileSync(
    new URL('../../../.github/workflows/google-chat-daily-reconciliation.yml', import.meta.url),
    'utf8',
  );

  assert.match(workflow, /live-reconciliation-receipt:\n/);
  assert.match(workflow, /github\.event_name != 'pull_request'/);
  assert.match(workflow, /CHAT_BRIDGE_TOKEN: \$\{\{ secrets\.CHAT_BRIDGE_TOKEN \}\}/);
  assert.match(workflow, /fetch-bridge-pages\.mjs \\\n\s+--out/);
  assert.match(workflow, /sanitize-export\.mjs \\\n/);
  assert.match(workflow, /import-plan\.mjs \\\n/);
  assert.match(workflow, /--since "\$\{GOOGLE_CHAT_WINDOW_SINCE\}"/);
  assert.match(workflow, /--until "\$\{GOOGLE_CHAT_WINDOW_UNTIL\}"/);
  assert.match(workflow, /reconciliation-receipt\.mjs \\\n/);
  assert.match(workflow, /if \(!receipt\.counts\?\.complete\)/);

  const uploadBlock = workflow.match(
    /name: Upload content-free live reconciliation evidence[\s\S]*?retention-days: 7/,
  )?.[0];
  assert.ok(uploadBlock, 'live evidence upload step must exist');
  assert.match(uploadBlock, /fetch-summary\.json/);
  assert.match(uploadBlock, /safety-report\.json/);
  assert.match(uploadBlock, /reconciliation-receipt\.json/);
  assert.doesNotMatch(uploadBlock, /\/raw(?:\s|$)|\/sanitized(?:\s|$)|import-plan|coverage-evidence/);
});
