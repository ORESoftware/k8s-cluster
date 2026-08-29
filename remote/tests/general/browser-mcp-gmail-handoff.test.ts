import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

const repoRoot = resolve(import.meta.dirname, '../../..');
const main = readFileSync(
  resolve(repoRoot, 'remote/deployments/browser-mcp-rs/src/main.rs'),
  'utf8',
);
const handoff = readFileSync(
  resolve(repoRoot, 'remote/deployments/browser-mcp-rs/src/email_handoff.rs'),
  'utf8',
);
const dockerfile = readFileSync(
  resolve(repoRoot, 'remote/deployments/browser-mcp-rs/Dockerfile'),
  'utf8',
);
const workflow = readFileSync(
  resolve(repoRoot, '.github/workflows/browser-mcp-gmail-handoff.yml'),
  'utf8',
);

test('Gmail provenance is validated and removed at the public MCP boundary', () => {
  assert.match(main, /mod email_handoff;/);
  assert.match(main, /"source_context"/);
  assert.match(main, /validate_gmail_handoff/);
  assert.match(main, /map\.remove\("source_context"\)/);
  assert.match(main, /event = "browser_gmail_handoff"/);
  assert.match(main, /request_ref/);
  assert.doesNotMatch(main, /"email_body"|"message_body"|"raw_subject"/);
});

test('Gmail handoff policy is fail closed for sensitive and phishing signals', () => {
  for (const signal of [
    'lookalike_domain',
    'requests_credentials',
    'requests_remote_access',
    'requests_crypto_or_gift_card',
    'requests_payment_or_bank',
    'requests_ssn_or_tax_id',
    'requests_identity_document_upload',
  ]) {
    assert.match(handoff, new RegExp(`"${signal}"`));
  }
  for (const host of [
    'gmail.com',
    'mail.google.com',
    'accounts.google.com',
    'outlook.com',
    'login.microsoftonline.com',
  ]) {
    assert.match(handoff, new RegExp(`"${host.replaceAll('.', '\\.')}"`));
  }
  assert.match(handoff, /url\.scheme\(\) != "https"/);
  assert.match(handoff, /Some\(Host::Ipv4\(_\)\) \| Some\(Host::Ipv6\(_\)\)/);
  assert.match(handoff, /hash_reference/);
  assert.doesNotMatch(handoff, /subject|message[_ ]body|email[_ ]body/i);
});

test('Browser MCP image compilation runs Rust unit tests first', () => {
  const testIndex = dockerfile.indexOf('cargo test --locked');
  const buildIndex = dockerfile.indexOf('cargo build --release --locked');
  assert.ok(testIndex >= 0, 'Dockerfile must run Rust unit tests');
  assert.ok(buildIndex > testIndex, 'Rust tests must run before release compilation');
});

test('Gmail handoff CI is read-only and uses immutable action revisions', () => {
  assert.match(workflow, /permissions:\s*\n\s*contents: read/);
  assert.match(workflow, /browser-mcp-gmail-handoff\.test\.ts/);
  assert.doesNotMatch(workflow, /uses:\s*[^\n]+@(main|master|v\d+|stable)\s*(?:#.*)?$/m);
  assert.doesNotMatch(workflow, /contents:\s*write|pull-requests:\s*write/);
});
