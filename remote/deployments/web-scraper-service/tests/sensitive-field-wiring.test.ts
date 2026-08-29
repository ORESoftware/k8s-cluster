import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

const source = readFileSync(resolve(import.meta.dirname, '../src/browser-agent.ts'), 'utf8');

test('browser worker enforces sensitive-field policy before every write path', () => {
  assert.match(source, /from '\.\/sensitive-field-policy\.js'/);
  assert.match(source, /sensitive_field_blocked/);

  const calls = source.match(/await assertResolvedFieldWriteAllowed\(/g) ?? [];
  assert.equal(calls.length, 3, 'fill, type, and fill_form must each enforce the target policy');

  for (const marker of ["case 'fill':", "case 'type':", "case 'fill_form':"]) {
    const start = source.indexOf(marker);
    assert.notEqual(start, -1, `missing ${marker}`);
    const block = source.slice(start, start + 900);
    const guard = block.indexOf('await assertResolvedFieldWriteAllowed(');
    const mutations = ['await loc.fill(', 'await loc.pressSequentially(']
      .map((needle) => block.indexOf(needle))
      .filter((index) => index >= 0);
    assert.ok(mutations.length > 0, `${marker} must contain a DOM mutation`);
    const mutation = Math.min(...mutations);
    assert.ok(guard >= 0, `${marker} is missing the sensitive-field guard`);
    assert.ok(guard < mutation, `${marker} must guard before the DOM is mutated`);
  }
});

test('browser worker accepts only domain-bound secret entries', () => {
  assert.doesNotMatch(source, /if \(typeof entry === 'string'\) return entry/);
  assert.match(source, /secret entries must be domain-bound objects/);
  assert.match(source, /secret entry must declare at least one permitted domain/);
});

test('sensitive-field policy errors are returned as human-completion blockers', () => {
  assert.match(source, /export type BlockerType =[\s\S]*?'sensitive_field_blocked'/);
  assert.match(source, /e\.code === 'sensitive_field_blocked'/);
  assert.match(source, /\? 'sensitive_field_blocked'/);
});
