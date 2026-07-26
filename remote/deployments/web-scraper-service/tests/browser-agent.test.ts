import assert from 'node:assert/strict';
import { test } from 'node:test';

import { __test } from '../src/browser-agent.js';

const {
  ActRequestSchema,
  ObserveRequestSchema,
  domainAllowed,
  requestUrlAllowedByDomain,
  effectiveAllowedDomains,
  validAllowedDomain,
  actionDigest,
  assertUrlAllowed,
  CONSEQUENTIAL_RE,
} = __test;

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

test('browser_state caps long-poll and text budgets', () => {
  assert.equal(ObserveRequestSchema.safeParse({ session_id: 's', wait_ms: 25_001 }).success, false);
  assert.equal(ObserveRequestSchema.safeParse({ session_id: 's', max_visible_text_chars: 30_001 }).success, false);
  assert.equal(ObserveRequestSchema.safeParse({ session_id: 's', wait_ms: 25_000 }).success, true);
});

test('browser_act accepts the complete declarative action surface', () => {
  const actions = [
    { type: 'type', target: { ref: 'e1' }, value: { literal: 'safe text' }, clear_first: true },
    { type: 'submit', target: { ref: 'e2' } },
    { type: 'scroll', delta_y: 500 },
    { type: 'screenshot' },
    { type: 'extract', include: ['visible_text', 'accessibility_snapshot', 'forms'] },
  ];
  assert.equal(
    ActRequestSchema.safeParse({
      request_id: 'r-actions',
      intent: 'exercise declarative actions',
      session_id: 's',
      actions,
    }).success,
    true,
  );
});

test('upload accepts exactly one bounded source and rejects unsafe inline files', () => {
  const base = {
    request_id: 'r-upload',
    intent: 'attach a harmless text file',
    session_id: 's',
  };
  const target = { label: 'Attachment' };
  assert.equal(
    ActRequestSchema.safeParse({
      ...base,
      actions: [
        {
          type: 'upload',
          target,
          inline_file: {
            file_name: 'note.txt',
            mime_type: 'text/plain',
            data_base64: Buffer.from('hello').toString('base64'),
          },
        },
      ],
    }).success,
    true,
  );
  assert.equal(
    ActRequestSchema.safeParse({
      ...base,
      actions: [{ type: 'upload', target, file_token: 'token-123' }],
    }).success,
    true,
  );
  assert.equal(
    ActRequestSchema.safeParse({
      ...base,
      actions: [
        {
          type: 'upload',
          target,
          file_token: 'token-123',
          inline_file: { file_name: 'note.txt', data_base64: 'aGVsbG8=' },
        },
      ],
    }).success,
    false,
  );
  assert.equal(
    ActRequestSchema.safeParse({
      ...base,
      actions: [
        {
          type: 'upload',
          target,
          inline_file: {
            file_name: '../secret.txt',
            data_base64: 'aGVsbG8=',
          },
        },
      ],
    }).success,
    false,
  );
  assert.equal(
    ActRequestSchema.safeParse({
      ...base,
      actions: [
        {
          type: 'upload',
          target,
          inline_file: {
            file_name: 'too-large.bin',
            data_base64: Buffer.alloc(256 * 1024 + 1).toString('base64'),
          },
        },
      ],
    }).success,
    false,
  );
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

test('requested domains cannot widen the process-level allowlist', () => {
  assert.deepEqual(
    effectiveAllowedDomains(['benefactor.cc', 'evil.example'], ['benefactor.cc']),
    ['benefactor.cc'],
  );
  assert.deepEqual(
    effectiveAllowedDomains(['www.benefactor.cc'], ['benefactor.cc']),
    ['www.benefactor.cc'],
  );
  assert.deepEqual(
    effectiveAllowedDomains(['benefactor.cc'], ['www.benefactor.cc']),
    ['www.benefactor.cc'],
  );
  assert.deepEqual(effectiveAllowedDomains(['evil.example'], ['benefactor.cc']), []);
  assert.equal(validAllowedDomain('benefactor.cc'), true);
  assert.equal(validAllowedDomain('https://benefactor.cc'), false);
  assert.equal(validAllowedDomain('benefactor.cc:443'), false);
});

test('all browser-created network requests remain inside the hostname ceiling', () => {
  const allowlist = ['benefactor.cc'];
  assert.equal(requestUrlAllowedByDomain('https://benefactor.cc/contact', allowlist), true);
  assert.equal(requestUrlAllowedByDomain('https://www.benefactor.cc/app.js', allowlist), true);
  assert.equal(requestUrlAllowedByDomain('wss://benefactor.cc/socket', allowlist), false);
  assert.equal(requestUrlAllowedByDomain('https://example.com/', allowlist), false);
  assert.equal(requestUrlAllowedByDomain('wss://example.com/socket', allowlist), false);
  assert.equal(requestUrlAllowedByDomain('https://benefactor.cc:8443/', allowlist), false);
  assert.equal(requestUrlAllowedByDomain('https://user:pass@benefactor.cc/', allowlist), false);
  assert.equal(requestUrlAllowedByDomain('file:///etc/passwd', allowlist), false);
});

test('assertUrlAllowed blocks dangerous schemes and off-allowlist hosts', async () => {
  await assert.rejects(assertUrlAllowed('file:///etc/passwd', [], noPrivate), /scheme/);
  await assert.rejects(assertUrlAllowed('http://plain.example/', [], noPrivate), /https/);
  await assert.rejects(assertUrlAllowed('https://evil.com/', ['irs.gov'], noPrivate), /allowlist/);
  await assert.rejects(
    assertUrlAllowed('https://benefactor.cc:8443/', ['benefactor.cc'], noPrivate),
    /explicit port/,
  );
  await assert.rejects(
    assertUrlAllowed('https://user:pass@benefactor.cc/', ['benefactor.cc'], noPrivate),
    /credentials/,
  );
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
