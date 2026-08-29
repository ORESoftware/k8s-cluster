import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

const source = readFileSync(resolve(import.meta.dirname, '../src/browser-agent.ts'), 'utf8');

test('DEN-267 runtime guard is integrated before this branch is mergeable', () => {
  const required = [
    "from './sensitive-field-policy.js'",
    "'sensitive_field_blocked'",
    'async function assertResolvedFieldWriteAllowed(',
    'await assertResolvedFieldWriteAllowed(loc, action.value);',
    'await assertResolvedFieldWriteAllowed(loc, field.value);',
    'secret entries must be domain-bound objects',
  ];
  for (const marker of required) {
    assert.ok(source.includes(marker), `browser-agent.ts is missing DEN-267 integration marker: ${marker}`);
  }
});
