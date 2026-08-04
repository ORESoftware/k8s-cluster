import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  ConsumerInventoryError,
  validateRustServerConsumers,
} from "./check-rust-server-consumers.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const inventory = JSON.parse(
  readFileSync(path.join(root, "pg-defs", "rust-server-consumers.json"), "utf8"),
);

function clone(value) {
  return structuredClone(value);
}

function invalid(value, pattern) {
  assert.throws(
    () => validateRustServerConsumers(value),
    error => {
      assert.ok(error instanceof ConsumerInventoryError);
      assert.match(error.message, pattern);
      return true;
    },
  );
}

test("committed inventory pins all current consumers to one authority", () => {
  const summary = validateRustServerConsumers(inventory);
  assert.deepEqual(summary, {
    valid: true,
    authorityCommit: "3c84cab532b27d328378f09fba5841f02644ae3b",
    consumerCount: 3,
    statusCounts: {
      inventory: 1,
      conversion: 0,
      verification: 2,
      "seaorm-only": 0,
    },
    directSqlxTarget: 0,
    rendererSpecificPersistence: false,
  });
});

test("schema and DPM authorities cannot drift", () => {
  const repository = clone(inventory);
  repository.authority.repository = "ORESoftware/k8s-cluster";
  invalid(repository, /shared k8s authority/);

  const commit = clone(inventory);
  commit.authority.commit = "main";
  invalid(commit, /must equal 3c84cab/);

  const schema = clone(inventory);
  schema.authority.schemaPath = "service/migrations/schema.sql";
  invalid(schema, /must remain pg-defs\/schema\/schema.sql/);

  const dpm = clone(inventory);
  dpm.declarativeMigrations.repository = "some-other/tool";
  invalid(dpm, /unexpected migration authority/);
});

test("startup DDL and direct-driver targets remain prohibited", () => {
  const startup = clone(inventory);
  startup.declarativeMigrations.serviceStartupDdl = true;
  invalid(startup, /must remain false/);

  const sqlx = clone(inventory);
  sqlx.applicationStandard.directSqlxTarget = 1;
  invalid(sqlx, /directSqlxTarget: must remain zero/);

  const postgres = clone(inventory);
  postgres.applicationStandard.directTokioPostgresTarget = 1;
  invalid(postgres, /directTokioPostgresTarget: must remain zero/);
});

test("Leptos and Dioxus cannot fork persistence", () => {
  const rendererStorage = clone(inventory);
  rendererStorage.uiPolicy.rendererSpecificPersistence = true;
  invalid(rendererStorage, /rendererSpecificPersistence: must remain false/);

  const noSharedBoundary = clone(inventory);
  noSharedBoundary.uiPolicy.sharedRepositoryBoundaryRequired = false;
  invalid(noSharedBoundary, /sharedRepositoryBoundaryRequired: must remain true/);
});

test("consumer identity, paths, statuses, and schema commits are exact", () => {
  const repository = clone(inventory);
  repository.consumers[0].repository = "ORESoftware/other";
  invalid(repository, /repository: must equal ORESoftware\/k8s-cluster/);

  const pathMutation = clone(inventory);
  pathMutation.consumers[0].servicePath = "../outside";
  invalid(pathMutation, /servicePath: must equal remote\/deployments\/contract-service-rs/);

  const duplicate = clone(inventory);
  duplicate.consumers[2] = clone(duplicate.consumers[1]);
  invalid(duplicate, /duplicated consumer|must contain exactly/);

  const commit = clone(inventory);
  commit.consumers[1].schemaCommit = "f".repeat(40);
  invalid(commit, /must equal the inventory authority commit/);

  const status = clone(inventory);
  status.consumers[2].migrationStatus = "almost-done";
  invalid(status, /unsupported status/);
});

test("seaorm-only requires complete evidence and a PR identity", () => {
  const complete = clone(inventory);
  const consumer = complete.consumers[2];
  consumer.migrationStatus = "seaorm-only";
  consumer.pullRequest = 99;
  consumer.requiredEvidence = [...complete.completionGate.seaormOnlyRequires];
  const summary = validateRustServerConsumers(complete);
  assert.equal(summary.statusCounts["seaorm-only"], 1);

  const missingEvidence = clone(complete);
  missingEvidence.consumers[2].requiredEvidence.pop();
  invalid(missingEvidence, /seaorm-only is missing/);

  const noPr = clone(complete);
  noPr.consumers[2].pullRequest = null;
  invalid(noPr, /seaorm-only requires a completed positive PR number/);
});

test("secret-bearing fields are rejected anywhere in inventory", () => {
  const secret = clone(inventory);
  secret.consumers[0].databaseUrl = "postgres://redacted";
  invalid(secret, /secret-bearing inventory fields are prohibited/);

  const token = clone(inventory);
  token.declarativeMigrations.token = "redacted";
  invalid(token, /secret-bearing inventory fields are prohibited/);
});

test("completion status and evidence sets stay complete", () => {
  const statuses = clone(inventory);
  statuses.completionGate.allowedStatuses.pop();
  invalid(statuses, /must contain exactly inventory, conversion, verification, seaorm-only/);

  const evidence = clone(inventory);
  evidence.completionGate.seaormOnlyRequires.pop();
  invalid(evidence, /must contain exactly/);
});
