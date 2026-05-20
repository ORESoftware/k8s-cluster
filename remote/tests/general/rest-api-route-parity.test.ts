import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/libs/pg-defs/schema/schema.sql'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

test('generated API docs stay current and classify REST API route families', () => {
  const result = spawnSync('node', ['remote/tests/check-rest-api-route-parity.mjs'], {
    cwd: repoRoot,
    encoding: 'utf8',
    timeout: 60_000,
  });

  assert.equal(
    result.status,
    0,
    `route parity checker failed (exit ${result.status}).\nSTDOUT:\n${result.stdout}\nSTDERR:\n${result.stderr}`,
  );
  assert.match(
    result.stdout,
    /checked API docs for \d+ service\(s\)/,
  );
});
