import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readdir, readFile, stat } from 'node:fs/promises';
import { join, relative, resolve, sep } from 'node:path';
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

// Only two homes for SQL files in this repo. See
// remote/databases/pg/seeds/readme.md for the convention.
const ALLOWED_SCHEMA_FILES: ReadonlyArray<string> = [
  'remote/libs/pg-defs/schema/schema.sql',
];
const ALLOWED_SEED_DIR = 'remote/databases/pg/seeds';

// Directories under remote/** that may contain `.sql` files for legitimate
// reasons unrelated to RDS schema (e.g. generated artifacts inside ignored
// build trees, third-party vendored sources). Keep this list empty to enforce
// the strict policy and only add entries when the asymmetry is justified.
const ALLOWED_OTHER_DIRS: ReadonlyArray<string> = [
  // The generated sqlc adapter mirrors schema.sql + emits a starter query catalogue. These
  // .sql files are codegen INPUTS (consumed by `sqlc generate`), not authoritative DDL — the
  // pg-defs `--check` workflow keeps them locked to schema/schema.sql.
  'remote/libs/pg-defs/generated/go/sqlc',
  // Billing owns a separate SQLx-managed ledger database; those migrations are
  // intentionally outside the shared pg-defs RDS schema.
  'remote/deployments/billing-server-rs/migrations',
  // The F# websocket demo owns a service-local PostgreSQL data plane and runs
  // this idempotent schema at boot. It is intentionally disjoint from shared
  // RDS pg-defs tables.
  'remote/deployments/fsharp-ws-server/sql',
];

const IGNORED_DIRS = new Set([
  '.git',
  '.cursor',
  '.vscode',
  '.idea',
  'node_modules',
  'target',
  'bin',
  'obj',
  'build',
  '_build',
  'dist',
  '.terraform',
  '.pnpm-store',
  'agent-transcripts',
  // `tmp` and `temp` are ignored at the repo root .gitignore level. Tooling
  // such as scripts/pg/diff/rds-vs-pg-defs.mjs writes scratch *.sql diffs into
  // remote/libs/pg-defs/tmp/migrations/, and those should NOT trip this guard
  // because they are not part of the source contract.
  'tmp',
  'temp',
]);

async function walk(currentRoot: string, repoRootDir: string, sink: string[]): Promise<void> {
  let entries;
  try {
    entries = await readdir(currentRoot, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    if (IGNORED_DIRS.has(entry.name)) {
      continue;
    }
    const absolutePath = join(currentRoot, entry.name);
    if (entry.isSymbolicLink()) {
      continue;
    }
    if (entry.isDirectory()) {
      await walk(absolutePath, repoRootDir, sink);
      continue;
    }
    if (entry.isFile() && entry.name.toLowerCase().endsWith('.sql')) {
      sink.push(relative(repoRootDir, absolutePath).split(sep).join('/'));
    }
  }
}

test('every .sql file under remote/ lives in schema.sql or seeds/', async () => {
  const remoteDir = resolve(repoRoot, 'remote');
  // Walk `remote/` but report paths relative to repoRoot so they line up with
  // ALLOWED_SCHEMA_FILES / ALLOWED_SEED_DIR (which are repo-root-relative).
  const sqlFiles: string[] = [];
  await walk(remoteDir, repoRoot, sqlFiles);
  sqlFiles.sort();

  const allowed = (relativePath: string): boolean => {
    if (ALLOWED_SCHEMA_FILES.includes(relativePath)) {
      return true;
    }
    if (relativePath.startsWith(`${ALLOWED_SEED_DIR}/`) && relativePath.endsWith('.sql')) {
      return true;
    }
    return ALLOWED_OTHER_DIRS.some((dir) => relativePath.startsWith(`${dir}/`));
  };

  const stray = sqlFiles.filter((relativePath) => !allowed(relativePath));
  assert.deepEqual(
    stray,
    [],
    `Stray .sql files found outside the allowed homes. Move table DDL into ` +
      `remote/libs/pg-defs/schema/schema.sql and data fixtures into ` +
      `remote/databases/pg/seeds/. See ` +
      `remote/databases/pg/seeds/readme.md for the convention.\nStray files:\n` +
      stray.map((file) => `  - ${file}`).join('\n'),
  );
});

test('the retired per-table SQL files have not crept back in', async () => {
  const retired = [
    'remote/databases/pg/tables/app-config-table.sql',
    'remote/databases/pg/tables/container-pool-configs-table.sql',
    'remote/databases/pg/tables/lambda-functions-table.sql',
  ];
  for (const relativePath of retired) {
    const absolutePath = resolve(repoRoot, relativePath);
    assert.ok(
      !existsSync(absolutePath),
      `${relativePath} was retired; its block lives in remote/libs/pg-defs/schema/schema.sql now. Re-introducing it would cause schema drift.`,
    );
  }
});

test('schema.sql still defines every table referenced by the generated bindings', async () => {
  const schemaSql = await readFile(
    resolve(repoRoot, 'remote/libs/pg-defs/schema/schema.sql'),
    'utf8',
  );

  for (const table of [
    'app_config',
    'container_pool_configs',
    'known_git_repos',
    'agent_remote_dev_threads',
    'agent_remote_dev_tasks',
    'agent_remote_dev_events',
    'agent_remote_dev_breadcrumbs',
    'agent_remote_dev_artifacts',
    'agent_remote_dev_runtime_locks',
    'lambda_functions',
  ]) {
    assert.match(
      schemaSql,
      new RegExp(`create table if not exists ${table}\\b`),
      `schema.sql is missing the ${table} table; the generated bindings depend on it.`,
    );
  }
});

test('every seed file declares schema.sql as its prerequisite', async () => {
  const seedsDir = resolve(repoRoot, ALLOWED_SEED_DIR);
  const entries = await readdir(seedsDir, { withFileTypes: true });
  const seedFiles = entries
    .filter((entry) => entry.isFile() && entry.name.endsWith('.sql'))
    .map((entry) => entry.name);
  assert.ok(seedFiles.length > 0, 'expected at least one seed file');
  for (const seedFile of seedFiles) {
    const contents = await readFile(join(seedsDir, seedFile), 'utf8');
    assert.match(
      contents,
      /remote\/libs\/pg-defs\/schema\/schema\.sql/,
      `${seedFile} must reference remote/libs/pg-defs/schema/schema.sql in its header comment so operators can trace the table contract back to the canonical schema.`,
    );
  }
});

test('the tables/ folder is fully retired (no orphan tracking files)', async () => {
  const tablesDir = resolve(repoRoot, 'remote/databases/pg/tables');
  let info;
  try {
    info = await stat(tablesDir);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
      return;
    }
    throw error;
  }
  if (info.isDirectory()) {
    const remaining = await readdir(tablesDir);
    assert.deepEqual(
      remaining,
      [],
      `remote/databases/pg/tables/ should be empty (its DDL now lives in schema.sql); found: ${remaining.join(', ')}`,
    );
  }
});

test('AGENTS.md makes pg-defs the schema source instead of Rust route code', async () => {
  const agentContext = await readFile(resolve(repoRoot, 'AGENTS.md'), 'utf8');
  assert.match(
    agentContext,
    /RDS Postgres plus `remote\/libs\/pg-defs\/schema\/schema\.sql` are the database contract\./,
    'AGENTS.md must name RDS plus remote/libs/pg-defs/schema/schema.sql as the database contract.',
  );
  assert.match(
    agentContext,
    /Do not generate\s+SQL, migrations, or table DDL from Rust code, API route handlers, Rust structs, or other application\s+code\./,
    'AGENTS.md must keep the global rule: never generate SQL from Rust/API code.',
  );
});
