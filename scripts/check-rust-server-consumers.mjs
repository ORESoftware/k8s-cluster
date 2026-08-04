#!/usr/bin/env node

import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const inventoryPath = path.join(root, "pg-defs", "rust-server-consumers.json");
const AUTHORITY_COMMIT = "3c84cab532b27d328378f09fba5841f02644ae3b";
const EXPECTED_CONSUMERS = new Map([
  ["contract-service", { repository: "ORESoftware/k8s-cluster", servicePath: "remote/deployments/contract-service-rs" }],
  ["ai-agent-bridge", { repository: "ORESoftware/ai-agent-bridge.rs", servicePath: "." }],
  ["mip-solver-node", { repository: "ORESoftware/mip-solver-node.rs", servicePath: "." }],
]);
const STATUS_ORDER = ["inventory", "conversion", "verification", "seaorm-only"];
const REQUIRED_COMPLETION_EVIDENCE = [
  "zero-direct-sqlx",
  "zero-direct-tokio-postgres",
  "no-service-startup-ddl",
  "immutable-shared-schema-pin",
  "generated-lockfile",
  "restart-tests",
  "concurrency-tests",
  "dpm-postgres-17-convergence",
];
const FORBIDDEN_KEYS = new Set([
  "databaseUrl",
  "database_url",
  "password",
  "secret",
  "token",
  "serviceRoleKey",
  "service_role_key",
]);

export class ConsumerInventoryError extends Error {
  constructor(errors) {
    super(`Rust-server consumer inventory is invalid:\n- ${errors.join("\n- ")}`);
    this.name = "ConsumerInventoryError";
    this.errors = errors;
  }
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function exactSet(actual, expected, location, errors) {
  if (!Array.isArray(actual)) {
    errors.push(`${location}: must be an array`);
    return;
  }
  if (actual.length !== expected.length || new Set(actual).size !== actual.length) {
    errors.push(`${location}: must contain exactly ${expected.join(", ")}`);
    return;
  }
  for (const item of expected) {
    if (!actual.includes(item)) errors.push(`${location}: missing ${item}`);
  }
}

function findForbiddenKeys(value, location, errors) {
  if (Array.isArray(value)) {
    value.forEach((item, index) => findForbiddenKeys(item, `${location}[${index}]`, errors));
    return;
  }
  if (!isObject(value)) return;
  for (const [key, child] of Object.entries(value)) {
    if (FORBIDDEN_KEYS.has(key)) errors.push(`${location}.${key}: secret-bearing inventory fields are prohibited`);
    findForbiddenKeys(child, `${location}.${key}`, errors);
  }
}

export function validateRustServerConsumers(input) {
  const inventory = structuredClone(input);
  const errors = [];
  if (!isObject(inventory)) throw new ConsumerInventoryError(["inventory: must be an object"]);
  findForbiddenKeys(inventory, "inventory", errors);

  if (inventory.$schema !== "./rust-server-consumers.schema.json") {
    errors.push("inventory.$schema: must equal ./rust-server-consumers.schema.json");
  }
  if (inventory.version !== 1) errors.push("inventory.version: must equal 1");
  if (inventory.authority?.repository !== "ORESoftware/k8s-libs-and-shared-defs") {
    errors.push("inventory.authority.repository: must remain the shared k8s authority");
  }
  if (inventory.authority?.commit !== AUTHORITY_COMMIT) {
    errors.push(`inventory.authority.commit: must equal ${AUTHORITY_COMMIT}`);
  }
  if (inventory.authority?.schemaPath !== "pg-defs/schema/schema.sql") {
    errors.push("inventory.authority.schemaPath: must remain pg-defs/schema/schema.sql");
  }
  if (inventory.authority?.seaOrmAdapterPath !== "pg-defs/rust/sea-orm") {
    errors.push("inventory.authority.seaOrmAdapterPath: must remain pg-defs/rust/sea-orm");
  }

  if (inventory.declarativeMigrations?.repository !== "declarative-migrations/declarative-migrations") {
    errors.push("inventory.declarativeMigrations.repository: unexpected migration authority");
  }
  if (inventory.declarativeMigrations?.version !== "1.4.2") {
    errors.push("inventory.declarativeMigrations.version: must equal 1.4.2");
  }
  if (!/^[0-9a-f]{64}$/u.test(inventory.declarativeMigrations?.linuxX8664Sha256 ?? "")) {
    errors.push("inventory.declarativeMigrations.linuxX8664Sha256: must be a lowercase SHA-256 digest");
  }
  if (inventory.declarativeMigrations?.serviceStartupDdl !== false) {
    errors.push("inventory.declarativeMigrations.serviceStartupDdl: must remain false");
  }

  if (inventory.applicationStandard?.directSqlxTarget !== 0) {
    errors.push("inventory.applicationStandard.directSqlxTarget: must remain zero");
  }
  if (inventory.applicationStandard?.directTokioPostgresTarget !== 0) {
    errors.push("inventory.applicationStandard.directTokioPostgresTarget: must remain zero");
  }
  if (inventory.applicationStandard?.restartTestsRequired !== true) {
    errors.push("inventory.applicationStandard.restartTestsRequired: must remain true");
  }
  if (inventory.applicationStandard?.concurrencyTestsRequired !== true) {
    errors.push("inventory.applicationStandard.concurrencyTestsRequired: must remain true");
  }

  if (inventory.uiPolicy?.rendererSpecificPersistence !== false) {
    errors.push("inventory.uiPolicy.rendererSpecificPersistence: must remain false");
  }
  if (inventory.uiPolicy?.sharedRepositoryBoundaryRequired !== true) {
    errors.push("inventory.uiPolicy.sharedRepositoryBoundaryRequired: must remain true");
  }
  for (const key of ["maudHtmx", "leptos", "dioxus"]) {
    if (typeof inventory.uiPolicy?.[key] !== "string" || !inventory.uiPolicy[key].trim()) {
      errors.push(`inventory.uiPolicy.${key}: must be non-empty`);
    }
  }

  if (!Array.isArray(inventory.consumers) || inventory.consumers.length !== EXPECTED_CONSUMERS.size) {
    errors.push(`inventory.consumers: must contain exactly ${EXPECTED_CONSUMERS.size} current consumers`);
  }
  const ids = new Set();
  for (const [index, consumer] of (inventory.consumers ?? []).entries()) {
    const location = `inventory.consumers[${index}]`;
    if (!isObject(consumer)) {
      errors.push(`${location}: must be an object`);
      continue;
    }
    const expected = EXPECTED_CONSUMERS.get(consumer.id);
    if (!expected) {
      errors.push(`${location}.id: unsupported consumer ${consumer.id}`);
      continue;
    }
    if (ids.has(consumer.id)) errors.push(`${location}.id: duplicated consumer`);
    ids.add(consumer.id);
    if (consumer.repository !== expected.repository) {
      errors.push(`${location}.repository: must equal ${expected.repository}`);
    }
    if (consumer.servicePath !== expected.servicePath) {
      errors.push(`${location}.servicePath: must equal ${expected.servicePath}`);
    }
    if (!STATUS_ORDER.includes(consumer.migrationStatus)) {
      errors.push(`${location}.migrationStatus: unsupported status`);
    }
    if (consumer.schemaCommit !== AUTHORITY_COMMIT) {
      errors.push(`${location}.schemaCommit: must equal the inventory authority commit`);
    }
    if (consumer.lockfileRequired !== true) {
      errors.push(`${location}.lockfileRequired: must remain true`);
    }
    if (!Array.isArray(consumer.requiredEvidence) || consumer.requiredEvidence.length === 0) {
      errors.push(`${location}.requiredEvidence: must be a non-empty array`);
    } else if (new Set(consumer.requiredEvidence).size !== consumer.requiredEvidence.length) {
      errors.push(`${location}.requiredEvidence: must be unique`);
    }
    if (consumer.migrationStatus === "seaorm-only") {
      for (const evidence of REQUIRED_COMPLETION_EVIDENCE) {
        if (!consumer.requiredEvidence.includes(evidence)) {
          errors.push(`${location}.requiredEvidence: seaorm-only is missing ${evidence}`);
        }
      }
      if (!Number.isSafeInteger(consumer.pullRequest) || consumer.pullRequest < 1) {
        errors.push(`${location}.pullRequest: seaorm-only requires a completed positive PR number`);
      }
    }
  }
  exactSet([...ids], [...EXPECTED_CONSUMERS.keys()], "inventory consumer ids", errors);

  exactSet(inventory.completionGate?.allowedStatuses, STATUS_ORDER, "inventory.completionGate.allowedStatuses", errors);
  exactSet(
    inventory.completionGate?.seaormOnlyRequires,
    REQUIRED_COMPLETION_EVIDENCE,
    "inventory.completionGate.seaormOnlyRequires",
    errors,
  );

  if (errors.length > 0) throw new ConsumerInventoryError(errors);
  const statusCounts = Object.fromEntries(STATUS_ORDER.map(status => [status, 0]));
  for (const consumer of inventory.consumers) statusCounts[consumer.migrationStatus] += 1;
  return {
    valid: true,
    authorityCommit: inventory.authority.commit,
    consumerCount: inventory.consumers.length,
    statusCounts,
    directSqlxTarget: inventory.applicationStandard.directSqlxTarget,
    rendererSpecificPersistence: inventory.uiPolicy.rendererSpecificPersistence,
  };
}

function parseArguments(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (key !== "--report" || !value || values.has(key)) {
      throw new Error("usage: check-rust-server-consumers.mjs [--report <path>]");
    }
    values.set(key, value);
  }
  return values;
}

const summary = validateRustServerConsumers(JSON.parse(readFileSync(inventoryPath, "utf8")));
const arguments_ = parseArguments(process.argv.slice(2));
const reportPath = arguments_.get("--report");
if (reportPath) {
  if (path.isAbsolute(reportPath) || reportPath.split(/[\\/]/u).includes("..")) {
    throw new Error("report path must remain repository relative");
  }
  const absolute = path.resolve(root, reportPath);
  if (!absolute.startsWith(`${root}${path.sep}`)) throw new Error("report path escapes the repository");
  writeFileSync(absolute, `${JSON.stringify(summary, null, 2)}\n`, { flag: "w", mode: 0o600 });
}
console.log(JSON.stringify(summary, null, 2));
