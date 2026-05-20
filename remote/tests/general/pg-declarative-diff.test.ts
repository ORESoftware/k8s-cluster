import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
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
const diffScript = 'scripts/pg/diff/rds-vs-pg-defs.mjs';
const fixturePath = 'tmp/tests/pg-diff/live-empty.json';

async function writeLiveFixture() {
  const absolutePath = resolve(repoRoot, fixturePath);
  await mkdir(dirname(absolutePath), { recursive: true });
  await writeFile(
    absolutePath,
    `${JSON.stringify({
      source: 'fixture',
      schemas: ['public'],
      tables: [],
      routines: [],
      triggers: [],
    })}\n`,
  );
}

test('RDS/pg-defs diff emits a report, not SQL migrations', async () => {
  await writeLiveFixture();

  const result = spawnSync(
    'node',
    [diffScript, '--from-live-json', fixturePath, '--format', 'json', '--output', '-'],
    {
      cwd: repoRoot,
      encoding: 'utf8',
      timeout: 60_000,
    },
  );

  assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(result.stdout);
  assert.equal(report.policy.generatesSql, false);
  assert.equal(report.policy.generatedMigrationFiles, false);
  assert.ok(report.diff.missingTables.length > 0, 'fixture should report missing pg-defs tables');
});

test('RDS/pg-defs diff refuses .sql output paths', async () => {
  await writeLiveFixture();
  const sqlOutput = 'tmp/tests/pg-diff/should-not-exist.sql';
  await rm(resolve(repoRoot, sqlOutput), { force: true });

  const result = spawnSync(
    'node',
    [diffScript, '--from-live-json', fixturePath, '--output', sqlOutput],
    {
      cwd: repoRoot,
      encoding: 'utf8',
      timeout: 60_000,
    },
  );

  assert.notEqual(result.status, 0, 'script should reject .sql output paths');
  assert.match(result.stderr, /Refusing to write a \.sql file/);
  assert.equal(existsSync(resolve(repoRoot, sqlOutput)), false);
});

test('AGENTS.md documents report-only declarative RDS diffs', async () => {
  const agentContext = await readFile(resolve(repoRoot, 'AGENTS.md'), 'utf8');
  assert.match(
    agentContext,
    /scripts\/pg\/diff\/rds-vs-pg-defs\.mjs/,
    'AGENTS.md should point operators at the declarative RDS/pg-defs diff script.',
  );
  assert.match(
    agentContext,
    /does not generate\s+`\.sql` migration files/,
    'AGENTS.md should state that the declarative diff script does not generate SQL migration files.',
  );
});
