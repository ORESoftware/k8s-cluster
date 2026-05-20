import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
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

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('dd-remote-rest-api declares dd-pg-defs as a local path dependency', async () => {
  const cargoToml = await readRepoFile('remote/deployments/rest-api-rs/Cargo.toml');
  // Single source of truth for the shared RDS Postgres contract. Without
  // this path-dep the service can drift from schema.sql.
  assert.match(
    cargoToml,
    /dd-pg-defs\s*=\s*\{\s*path\s*=\s*"\.\.\/\.\.\/libs\/pg-defs\/generated\/rust"\s*\}/,
    'remote/deployments/rest-api-rs/Cargo.toml is missing the dd-pg-defs path-dep entry.',
  );
});

test('dd-remote-rest-api exposes a pg_contract module that re-exports dd_pg_defs', async () => {
  const pgContractPath = resolve(repoRoot, 'remote/deployments/rest-api-rs/src/pg_contract.rs');
  assert.ok(
    existsSync(pgContractPath),
    'remote/deployments/rest-api-rs/src/pg_contract.rs is missing. See the file in remote/deployments/rest-api-rs for the reference pattern (re-export + assert_canonical_schema_matches_local_reads).',
  );
  const source = await readFile(pgContractPath, 'utf8');
  assert.match(
    source,
    /^pub use dd_pg_defs::/m,
    'pg_contract.rs must `pub use dd_pg_defs::*` so the path-dep is exercised at compile time.',
  );
  assert.match(
    source,
    /pub fn assert_canonical_schema_matches_local_reads\s*\(/,
    'pg_contract.rs must expose assert_canonical_schema_matches_local_reads() so main.rs can call it at startup.',
  );
  // The lambda function read columns must be enumerated (and the test
  // suite below enforces the subset relationship).
  assert.match(
    source,
    /LOCAL_LAMBDA_FUNCTIONS_READ_COLUMNS\s*:\s*&\[&str\]\s*=\s*&\[/,
    'pg_contract.rs must list the lambda_functions columns this service reads, so the subset assertion can detect drift in schema.sql.',
  );
});

test('dd-remote-rest-api wires pg_contract::assert into main()', async () => {
  const source = await readRepoFile('remote/deployments/rest-api-rs/src/main.rs');
  assert.match(
    source,
    /^mod pg_contract;$/m,
    'main.rs must declare `mod pg_contract;` so the module compiles.',
  );
  assert.match(
    source,
    /pg_contract::assert_canonical_schema_matches_local_reads\s*\(\s*\)\s*;/,
    'main.rs must call pg_contract::assert_canonical_schema_matches_local_reads() at startup so schema drift fails fast.',
  );
});

test('dd-remote-web-home stays free of direct Postgres dependencies', async () => {
  // dd-remote-web-home is intentionally a pure HTML/JS server that
  // delegates all RDS reads to dd-remote-rest-api via /api/*. Anyone who
  // wants to read Postgres directly from web-home should route through
  // dd-remote-rest-api instead (so the canonical pg-defs surface stays
  // single). If you must add a direct PG client, also add a pg_contract
  // module like remote/deployments/rest-api-rs/src/pg_contract.rs and update this
  // test to enforce the same wiring.
  const cargoToml = await readRepoFile('remote/deployments/web-home-rs/Cargo.toml');
  for (const forbidden of [
    'tokio-postgres',
    'sqlx',
    'diesel',
    'sea-orm',
    'postgres ',
  ]) {
    assert.doesNotMatch(
      cargoToml,
      new RegExp(`^\\s*${forbidden.replace(/[.*+?^${}()|[\\]\\\\]/g, '\\$&')}`, 'm'),
      `remote/deployments/web-home-rs/Cargo.toml unexpectedly declares ${forbidden}. If web-home truly needs direct DB access, wire it through a new pg_contract module against dd-pg-defs (mirror remote/deployments/rest-api-rs/src/pg_contract.rs) and update this test accordingly.`,
    );
  }
  const source = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');
  // No direct PG client usage in the Rust source either; web-home stays
  // a pure HTML server today.
  for (const forbidden of [/tokio_postgres::/, /use tokio_postgres/, /::Postgres\b/]) {
    assert.doesNotMatch(
      source,
      forbidden,
      `remote/deployments/web-home-rs/src/main.rs imports a Postgres client (${forbidden}). Route DB reads through dd-remote-rest-api instead, or wire pg_contract.`,
    );
  }
});

test('cargo check passes for dd-remote-rest-api (proves dd-pg-defs path-dep resolves)', { timeout: 600_000 }, async (t) => {
  const cargo = spawnSync('cargo', ['--version'], { encoding: 'utf8' });
  if (cargo.status !== 0) {
    t.skip(
      'cargo is not on PATH. Install via `rustup default stable`. CI installs ' +
        'Rust before running this test, so this skip is local-only.',
    );
    return;
  }

  const result = spawnSync(
    'cargo',
    ['check', '--quiet', '--message-format=short'],
    {
      cwd: resolve(repoRoot, 'remote/deployments/rest-api-rs'),
      encoding: 'utf8',
      timeout: 540_000,
    },
  );
  assert.equal(
    result.status,
    0,
    `cargo check on dd-remote-rest-api failed (exit ${result.status}).\nSTDOUT:\n${result.stdout}\nSTDERR:\n${result.stderr}`,
  );
});

test('cargo test passes the pg_contract unit suite (column subset + table name + status round-trip)', { timeout: 600_000 }, async (t) => {
  const cargo = spawnSync('cargo', ['--version'], { encoding: 'utf8' });
  if (cargo.status !== 0) {
    t.skip('cargo is not on PATH; CI gates this test on cargo availability.');
    return;
  }

  const result = spawnSync(
    'cargo',
    ['test', '--quiet', '--bin', 'dd-remote-rest-api', 'pg_contract'],
    {
      cwd: resolve(repoRoot, 'remote/deployments/rest-api-rs'),
      encoding: 'utf8',
      timeout: 540_000,
    },
  );
  assert.equal(
    result.status,
    0,
    `cargo test pg_contract failed (exit ${result.status}).\nSTDOUT:\n${result.stdout}\nSTDERR:\n${result.stderr}`,
  );
  assert.match(
    `${result.stdout}\n${result.stderr}`,
    /\b3 passed\b/,
    'Expected three pg_contract tests to pass (lambda subset, table name, status round-trip). Output:\n' +
      `${result.stdout}\n${result.stderr}`,
  );
});
