import assert from "node:assert/strict";
import test from "node:test";

import {
  SeaOrmPolicyError,
  validateSeaOrmPolicy,
} from "./seaorm-policy.mjs";

const sharedCommit = "3c84cab532b27d328378f09fba5841f02644ae3b";
const valid = {
  manifest: `
[dependencies]
dd-pg-defs-sea-orm = { path = "../../libs/pg-defs/generated/rust/sea-orm" }
sea-orm = { version = "1.1.20", features = ["sqlx-postgres"] }
`,
  coordination: `
use sea_orm::{ConnectOptions, DatabaseConnection, DatabaseTransaction, Statement};
use tokio::sync::OnceCell;
struct Inner { database: OnceCell<DatabaseConnection> }
struct Lease { transaction: Option<DatabaseTransaction> }
fn query(advisory_key: i64) {
  let _ = Statement::from_sql_and_values(
    sea_orm::DbBackend::Postgres,
    "select pg_try_advisory_xact_lock($1) as acquired",
    [advisory_key.into()],
  );
}
async fn run(db: &DatabaseConnection) {
  let _ = db.begin().await;
  let _ = transaction.rollback().await;
}
fn options(mut options: ConnectOptions) { options.sqlx_logging(false); }
`,
  main: "let coordination = coordination::CoordinationState::from_env(rpc_client.clone());",
  sharedContract: {
    $schema: "./rust-server-contract.schema.json",
    version: 1,
    policyIssue: "fleet-sqlx-to-seaorm",
    schemaAuthority: {
      repository: "ORESoftware/k8s-libs-and-shared-defs",
      path: "pg-defs/schema/schema.sql",
      generatedAdaptersAreMigrationSources: false,
      serviceBootMigrations: false,
      humanReviewRequired: true,
    },
    migration: {
      tool: "dpm",
      repository: "declarative-migrations/declarative-postgres-migrate.rs",
    },
    rust: {
      applicationOrm: "SeaORM",
      directSqlxDependency: "forbidden",
    },
    consumer: {},
    verification: [],
  },
  sharedCommit,
};

function clone(value) {
  return structuredClone(value);
}

function expectInvalid(input, pattern) {
  assert.throws(
    () => validateSeaOrmPolicy(input),
    (error) => {
      assert.ok(error instanceof SeaOrmPolicyError);
      assert.match(error.message, pattern);
      return true;
    },
  );
}

test("the valid fixture preserves SeaORM, advisory fencing, and shared ownership", () => {
  assert.deepEqual(validateSeaOrmPolicy(valid), {
    valid: true,
    service: "dd-contract-service",
    applicationOrm: "SeaORM",
    sharedCommit,
    directSqlx: false,
    bootMigrations: false,
    advisoryLockStatement: true,
  });
});

test("direct SQLx and tokio-postgres dependencies fail closed", () => {
  const sqlx = clone(valid);
  sqlx.manifest += '\nsqlx = { version = "0.8" }\n';
  expectInvalid(sqlx, /must not directly depend on SQLx/);

  const tokioPostgres = clone(valid);
  tokioPostgres.manifest += '\ntokio-postgres = "0.7"\n';
  expectInvalid(tokioPostgres, /must not directly depend on tokio-postgres/);

  const query = clone(valid);
  query.coordination += '\nlet _ = sqlx::query("select 1");\n';
  expectInvalid(query, /forbidden persistence path/);
});

test("advisory locking must remain parameterized and transaction-scoped", () => {
  const interpolation = clone(valid);
  interpolation.coordination = interpolation.coordination.replace(
    "[advisory_key.into()]",
    "[]",
  );
  expectInvalid(interpolation, /missing "\[advisory_key\.into\(\)\]"/);

  const transaction = clone(valid);
  transaction.coordination = transaction.coordination.replace(
    "transaction: Option<DatabaseTransaction>",
    "transaction: Option<()> ",
  );
  expectInvalid(transaction, /lock lifetime must remain bound/);

  const lazy = clone(valid);
  lazy.coordination = lazy.coordination.replace(
    "database: OnceCell<DatabaseConnection>",
    "database: DatabaseConnection",
  );
  expectInvalid(lazy, /preserve lazy database initialization/);
});

test("service boot migrations and mutable shared references are rejected", () => {
  const migration = clone(valid);
  migration.coordination += "\nMigrator::up(db, None).await;\n";
  expectInvalid(migration, /forbidden persistence path/);

  const mutable = clone(valid);
  mutable.sharedCommit = "main";
  expectInvalid(mutable, /immutable commit SHA/);

  const missingAdapter = clone(valid);
  missingAdapter.manifest = missingAdapter.manifest.replace(
    /dd-pg-defs-sea-orm[^\n]+\n/u,
    "",
  );
  expectInvalid(missingAdapter, /generated shared SeaORM adapter/);
});

test("shared schema authority and DPM repository cannot drift", () => {
  const schema = clone(valid);
  schema.sharedContract.schemaAuthority.path = "service/migrations";
  expectInvalid(schema, /schema authority path drifted/);

  const orm = clone(valid);
  orm.sharedContract.rust.applicationOrm = "SQLx";
  expectInvalid(orm, /shared contract must require SeaORM/);

  const dpm = clone(valid);
  dpm.sharedContract.migration.repository = "other/migrator";
  expectInvalid(dpm, /shared migration repository drifted/);
});
