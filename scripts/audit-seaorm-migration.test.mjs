import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  MigrationAuditError,
  validateContract,
} from "./audit-seaorm-migration.mjs";

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const contract = JSON.parse(
  readFileSync(path.join(repositoryRoot, "seaorm-migration.json"), "utf8"),
);

function clone(value) {
  return structuredClone(value);
}

function invalid(value, pattern) {
  assert.throws(
    () => validateContract(value),
    (error) => {
      assert.ok(error instanceof MigrationAuditError);
      assert.match(error.message, pattern);
      return true;
    },
  );
}

test("committed migration contract names the immutable shared authorities", () => {
  const validated = validateContract(contract);
  assert.equal(validated.status, "migration-in-progress");
  assert.equal(
    validated.schemaAuthority.commit,
    "3c84cab532b27d328378f09fba5841f02644ae3b",
  );
  assert.equal(
    validated.declarativeMigrations.repository,
    "declarative-migrations/declarative-migrations",
  );
  assert.equal(validated.declarativeMigrations.serviceStartupMigrations, false);
  assert.equal(validated.applicationPersistence.targetOrm, "SeaORM");
});

test("schema authority cannot drift to a service-owned copy or mutable ref", () => {
  const repository = clone(contract);
  repository.schemaAuthority.repository = "ORESoftware/mip-solver-node.rs";
  invalid(repository, /schema authority repository is incorrect/);

  const commit = clone(contract);
  commit.schemaAuthority.commit = "main";
  invalid(commit, /full lowercase SHA/);

  const pathMutation = clone(contract);
  pathMutation.schemaAuthority.schemaPath = "migrations/schema.sql";
  invalid(pathMutation, /schema authority path/);
});

test("declarative migrations remains external to service startup", () => {
  const repository = clone(contract);
  repository.declarativeMigrations.repository = "some-other/tool";
  invalid(repository, /declarative-migrations repository is incorrect/);

  const startup = clone(contract);
  startup.declarativeMigrations.serviceStartupMigrations = true;
  invalid(startup, /startup migrations must remain disabled/);
});

test("the target remains SeaORM with zero direct driver usage", () => {
  const orm = clone(contract);
  orm.applicationPersistence.targetOrm = "SQLx";
  invalid(orm, /target ORM must be SeaORM/);

  const sqlx = clone(contract);
  sqlx.applicationPersistence.directSqlxTarget = 1;
  invalid(sqlx, /direct SQLx target must remain zero/);

  const postgres = clone(contract);
  postgres.applicationPersistence.directTokioPostgresTarget = 1;
  invalid(postgres, /direct tokio-postgres target must remain zero/);
});

test("Leptos and Dioxus cannot create renderer-specific persistence", () => {
  const rendererStorage = clone(contract);
  rendererStorage.uiPolicy.rendererSpecificPersistence = true;
  invalid(rendererStorage, /renderer-specific persistence must remain disabled/);
});

test("only reviewed migration lifecycle states are accepted", () => {
  const complete = clone(contract);
  complete.status = "seaorm-only";
  assert.equal(validateContract(complete).status, "seaorm-only");

  const invalidStatus = clone(contract);
  invalidStatus.status = "done-ish";
  invalid(invalidStatus, /status must be migration-in-progress or seaorm-only/);
});
