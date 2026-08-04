import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  RustServerContractError,
  validateRustServerContract,
} from "./rust-server-contract.mjs";

const pgDefsRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const contract = JSON.parse(
  await readFile(path.join(pgDefsRoot, "rust-server-contract.json"), "utf8"),
);
const files = {
  schemaSql: await readFile(path.join(pgDefsRoot, "schema", "schema.sql"), "utf8"),
  generatedManifest: await readFile(
    path.join(pgDefsRoot, "generated", "rust", "sea-orm", "Cargo.toml"),
    "utf8",
  ),
  generatedSource: await readFile(
    path.join(pgDefsRoot, "generated", "rust", "sea-orm", "src", "lib.rs"),
    "utf8",
  ),
  dpmScript: await readFile(path.join(pgDefsRoot, "scripts", "dpm.sh"), "utf8"),
};

function clone(value) {
  return structuredClone(value);
}

function expectInvalid(contractInput, filesInput, pattern) {
  assert.throws(
    () => validateRustServerContract(contractInput, filesInput),
    (error) => {
      assert.ok(error instanceof RustServerContractError);
      assert.match(error.message, pattern);
      return true;
    },
  );
}

test("the committed contract binds SeaORM consumers to schema.sql and DPM", () => {
  assert.deepEqual(validateRustServerContract(contract, files), {
    valid: true,
    schemaAuthority: "ORESoftware/k8s-libs-and-shared-defs",
    schemaPath: "pg-defs/schema/schema.sql",
    applicationOrm: "SeaORM",
    migrationTool: "dpm",
    minimumDpmVersion: "0.3.2",
    generatedCrate: "dd-pg-defs-sea-orm",
    sanctionedStatementCases: 6,
    verificationGates: 7,
  });
});

test("direct SQLx, tokio-postgres, and boot migrations cannot be re-enabled", () => {
  const sqlx = clone(contract);
  sqlx.rust.directSqlxDependency = "allowed";
  expectInvalid(sqlx, files, /directSqlxDependency: must equal "forbidden"/);

  const tokioPostgres = clone(contract);
  tokioPostgres.rust.rawTokioPostgresDependency = "allowed";
  expectInvalid(tokioPostgres, files, /rawTokioPostgresDependency: must equal "forbidden"/);

  const bootMigration = clone(contract);
  bootMigration.schemaAuthority.serviceBootMigrations = true;
  expectInvalid(bootMigration, files, /serviceBootMigrations: must equal false/);
});

test("DPM remains pinned, review-gated, and double-consent destructive", () => {
  const tool = clone(contract);
  tool.migration.repository = "other/migrator";
  expectInvalid(tool, files, /migration.repository: must equal/);

  const version = clone(contract);
  version.migration.minimumVersion = "0.1.0";
  expectInvalid(version, files, /minimumVersion: must equal "0.3.2"/);

  const review = clone(contract);
  review.migration.applyRequiresHumanReview = false;
  expectInvalid(review, files, /applyRequiresHumanReview: must equal true/);

  const consent = clone(contract);
  consent.migration.destructiveConsents = ["--allow-destructive"];
  expectInvalid(consent, files, /destructiveConsents: must contain exactly/);
});

test("the generated SeaORM adapter remains an adapter rather than a migration engine", () => {
  const manifest = clone(files);
  manifest.generatedManifest = manifest.generatedManifest.replace(
    'name = "dd-pg-defs-sea-orm"',
    'name = "copied-service-entities"',
  );
  expectInvalid(contract, manifest, /package name does not match/);

  const source = clone(files);
  source.generatedSource += "\nfn bad(pool: &sqlx::PgPool) { let _ = sqlx::query(\"select 1\"); }\n";
  expectInvalid(contract, source, /must not execute direct SQLx queries or migrations/);

  const marker = clone(files);
  marker.generatedSource = marker.generatedSource.replace(
    "SOURCE OF TRUTH: schema/schema.sql defines the database contract.",
    "Generated source owns migrations.",
  );
  expectInvalid(contract, marker, /missing generated adapter marker/);
});

test("the DPM wrapper cannot lose its review, shadow, or installer-pin safeguards", () => {
  const shadow = clone(files);
  shadow.dpmScript = shadow.dpmScript.replace("SHADOW_DATABASE_URL is required", "shadow optional");
  expectInvalid(contract, shadow, /missing safety marker/);

  const review = clone(files);
  review.dpmScript = review.dpmScript.replace(
    "Never apply migrations automatically; a human reviews the SQL first.",
    "automatic apply is fine",
  );
  expectInvalid(contract, review, /missing safety marker/);

  const pin = clone(files);
  pin.dpmScript = pin.dpmScript.replace(contract.migration.installerCommit, "a".repeat(40));
  expectInvalid(contract, pin, /installer pin does not match/);

  const ormMigration = clone(files);
  ormMigration.dpmScript += "\nsqlx migrate run\n";
  expectInvalid(contract, ormMigration, /ORM-owned migration execution is prohibited/);
});

test("consumer paths and verification gates fail closed on drift", () => {
  const traversal = clone(contract);
  traversal.consumer.mount = "../outside";
  expectInvalid(traversal, files, /consumer.mount: must be a bounded repository-relative path/);

  const mutable = clone(contract);
  mutable.consumer.pinMustBeImmutable = false;
  expectInvalid(mutable, files, /pinMustBeImmutable: must equal true/);

  const missingGate = clone(contract);
  missingGate.verification.pop();
  expectInvalid(missingGate, files, /verification: must contain exactly/);

  const missingStatementCase = clone(contract);
  missingStatementCase.rust.sanctionedStatementCases.pop();
  expectInvalid(missingStatementCase, files, /sanctionedStatementCases: must contain exactly/);
});
