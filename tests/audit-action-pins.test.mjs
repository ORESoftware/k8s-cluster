import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';

const dir = '.github/workflows';

test('every workflow action is pinned to an immutable digest', () => {
  for (const f of readdirSync(dir).filter((f) => f.endsWith('.yml') || f.endsWith('.yaml'))) {
    const text = readFileSync(`${dir}/${f}`, 'utf8');
    for (const [, ref] of text.matchAll(/uses:\s*(\S+)/g)) {
      const ok = /@[0-9a-f]{40}(\s|$)/.test(ref) || /^docker:\/\/.+@sha256:[0-9a-f]{64}$/.test(ref);
      assert.ok(ok, `${f}: unpinned action ${ref}`);
    }
  }
});
