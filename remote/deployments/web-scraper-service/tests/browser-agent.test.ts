import assert from 'node:assert/strict';
import { test } from 'node:test';

import { __test } from '../src/browser-agent.js';

const { ActRequestSchema, ObserveRequestSchema, domainAllowed, actionDigest, assertUrlAllowed, CONSEQUENTIAL_RE } =
  __test;

const noPrivate = (_addr: string) => false;
const isPrivate = (addr: string) => addr.startsWith('127.') || addr.startsWith('10.') || addr === '169.254.169.254';

test('browser_act request requires intent and at least one action', () => {
  assert.equal(ActRequestSchema.safeParse({ request_id: 'r1', intent: 'go', actions: [] }).success, false);
  const ok = ActRequestSchema.safeParse({
    request_id: 'r1',
    intent: 'start and navigate',
    actions: [{ type: 'start', initial_url: 'https://sos.state.co.us/' }],
  });
  assert.equal(ok.success, true);
});

test('browser_act rejects unknown action types and oversized batches', () => {
  assert.equal(
    ActRequestSchema.safeParse({ request_id: 'r', intent: 'x', actions: [{ type: 'evaluate', script: 'x' }] }).success,
    false,
  );
  const tooMany = Array.from({ length: 21 }, () => ({ type: 'reload' as const }));
  assert.equal(ActRequestSchema.safeParse({ request_id: 'r', intent: 'x', actions: tooMany }).success, false);
});

test('secret_ref is accepted in fill values but literal and secret_ref are mutually exclusive', () => {
  const withSecret = ActRequestSchema.safeParse({
    request_id: 'r',
    intent: 'fill',
    session_id: 's',
    actions: [{ type: 'fill', target: { ref: 'e1' }, value: { secret_ref: 'vault://x' } }],
  });
  assert.equal(withSecret.success, true);
  const both = ActRequestSchema.safeParse({
    request_id: 'r',
    intent: 'fill',
    session_id: 's',
    actions: [{ type: 'fill', target: { ref: 'e1' }, value: { literal: 'a', secret_ref: 'b' } }],
  });
  assert.equal(both.success, false);
});

test('confirmation must assert explicit approval', () => {
  const bad = ActRequestSchema.safeParse({
    request_id: 'r',
    intent: 'submit',
    session_id: 's',
    actions: [{ type: 'click', target: { ref: 'e1' } }],
    confirmation: { action_digest: 'sha256:x', confirmed_revision: 3, user_explicitly_approved: false },
  });
  assert.equal(bad.success, false);
});

test('browser_observe caps long-poll and text budgets', () => {
  assert.equal(ObserveRequestSchema.safeParse({ session_id: 's', wait_ms: 25_001 }).success, false);
  assert.equal(ObserveRequestSchema.safeParse({ session_id: 's', max_visible_text_chars: 30_001 }).success, false);
  assert.equal(ObserveRequestSchema.safeParse({ session_id: 's', wait_ms: 25_000 }).success, true);
});

test('domain allowlist matches host and subdomains only', () => {
  const list = ['sos.state.co.us', 'irs.gov'];
  assert.equal(domainAllowed('sos.state.co.us', list), true);
  assert.equal(domainAllowed('www.irs.gov', list), true);
  assert.equal(domainAllowed('evil.com', list), false);
  assert.equal(domainAllowed('notirs.gov', list), false);
  // empty allowlist = any host (dev mode)
  assert.equal(domainAllowed('anything.example', []), true);
});

test('assertUrlAllowed blocks dangerous schemes and off-allowlist hosts', async () => {
  await assert.rejects(assertUrlAllowed('file:///etc/passwd', [], noPrivate), /scheme/);
  await assert.rejects(assertUrlAllowed('http://plain.example/', [], noPrivate), /https/);
  await assert.rejects(assertUrlAllowed('https://evil.com/', ['irs.gov'], noPrivate), /allowlist/);
});

test('assertUrlAllowed blocks IP-literals in private ranges and the metadata IP', async () => {
  await assert.rejects(assertUrlAllowed('https://127.0.0.1/', [], isPrivate), /private/);
  await assert.rejects(assertUrlAllowed('https://169.254.169.254/latest/meta-data/', [], isPrivate), /private/);
});

test('action digest is stable for identical inputs and changes with revision', () => {
  const a = actionDigest('sess', 4, 'https://x/y', 'click:Submit');
  const b = actionDigest('sess', 4, 'https://x/y', 'click:Submit');
  const c = actionDigest('sess', 5, 'https://x/y', 'click:Submit');
  assert.equal(a, b);
  assert.notEqual(a, c);
  assert.match(a, /^sha256:[0-9a-f]{64}$/);
});

test('consequential-action heuristic flags submissions but not benign clicks', () => {
  assert.equal(CONSEQUENTIAL_RE.test('Submit filing'), true);
  assert.equal(CONSEQUENTIAL_RE.test('Pay now'), true);
  assert.equal(CONSEQUENTIAL_RE.test('Next'), false);
  assert.equal(CONSEQUENTIAL_RE.test('Show more'), false);
});
