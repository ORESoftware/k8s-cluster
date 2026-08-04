import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const flags = readFileSync(new URL('../.cli-flags.toml', import.meta.url), 'utf8');
const ci = readFileSync(new URL('../.github/workflows/ci.yml', import.meta.url), 'utf8');

function flagSection(name) {
  const marker = `[flags.${name}]`;
  const start = flags.indexOf(marker);
  assert.notEqual(start, -1, `missing ${marker}`);
  const next = flags.indexOf('\n[flags.', start + marker.length);
  return flags.slice(start, next === -1 ? flags.length : next);
}

test('host runtime execution stays opt-in', () => {
  const section = flagSection('lambda_allow_host_runtimes');
  assert.match(section, /env\s*=\s*"LAMBDA_ALLOW_HOST_RUNTIMES"/);
  assert.doesNotMatch(section, /^default\s*=\s*"?nodejs"?\s*$/m);
});

test('host-networked containers require an explicit acknowledgement', () => {
  const section = flagSection('lambda_allow_container_host_network');
  assert.match(section, /type\s*=\s*"boolean"/);
  assert.match(section, /^default\s*=\s*false\s*$/m);
});

test('per-function JavaScript sandboxes remain bounded', () => {
  const section = flagSection('lambda_sandbox_cache_max');
  assert.match(section, /type\s*=\s*"integer"/);
  assert.match(section, /^default\s*=\s*64\s*$/m);
});

test('source formatting is a required CI gate', () => {
  const formatJob = ci.match(/\n  fmt:[\s\S]*?(?=\n  [a-zA-Z0-9_-]+:|$)/)?.[0];
  assert.ok(formatJob, 'fmt job must exist');
  assert.doesNotMatch(formatJob, /continue-on-error:\s*true/);
  assert.match(formatJob, /gleam format --check src test/);
});
