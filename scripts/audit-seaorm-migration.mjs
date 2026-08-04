#!/usr/bin/env node

import { lstatSync, readFileSync, readdirSync, realpathSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = realpathSync(path.resolve(path.dirname(fileURLToPath(import.meta.url)), ".."));
const contractPath = path.join(repositoryRoot, "seaorm-migration.json");
const MAX_FILE_BYTES = 4 * 1024 * 1024;
const EXCLUDED_DIRECTORIES = new Set([
  ".git",
  ".idea",
  ".vscode",
  "node_modules",
  "target",
  "test-results",
  "vendor",
]);
const ALLOWED_EXTENSIONS = new Set([".rs", ".toml", ".sql"]);
const DIRECT_SQLX_PATTERNS = [
  { id: "sqlx-path", pattern: /\bsqlx::/gu },
  { id: "pg-pool", pattern: /\bPgPool(?:Options)?\b/gu },
  { id: "sqlx-migrate", pattern: /\bsqlx::migrate!\s*\(/gu },
  { id: "cargo-sqlx-dependency", pattern: /^\s*sqlx\s*=\s*/gmu },
];
const DIRECT_TOKIO_POSTGRES_PATTERNS = [
  { id: "tokio-postgres-path", pattern: /\btokio_postgres::/gu },
  { id: "cargo-tokio-postgres-dependency", pattern: /^\s*tokio-postgres\s*=\s*/gmu },
];
const STARTUP_MIGRATION_PATTERNS = [
  { id: "sqlx-startup-migration", pattern: /sqlx::migrate!\s*\([^)]*\)\s*\.run\s*\(/gsu },
  { id: "seaorm-startup-migration", pattern: /\bMigrator::(?:up|fresh|refresh)\s*\(/gu },
  { id: "refinery-startup-migration", pattern: /\bRunner::new\s*\([^)]*\).*?\.run\s*\(/gsu },
];

export class MigrationAuditError extends Error {
  constructor(errors) {
    super(`SeaORM migration audit failed:\n- ${errors.join("\n- ")}`);
    this.name = "MigrationAuditError";
    this.errors = errors;
  }
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function safeRelativePath(value) {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    !path.isAbsolute(value) &&
    !value.split(/[\\/]/u).includes("..") &&
    !/[\u0000-\u001f\u007f]/u.test(value)
  );
}

function walk(directory, files = []) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.isSymbolicLink()) continue;
    const absolute = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      if (!EXCLUDED_DIRECTORIES.has(entry.name)) walk(absolute, files);
      continue;
    }
    if (!entry.isFile() || !ALLOWED_EXTENSIONS.has(path.extname(entry.name))) continue;
    const relative = path.relative(repositoryRoot, absolute).split(path.sep).join("/");
    if (safeRelativePath(relative)) files.push({ absolute, relative });
  }
  return files;
}

function boundedText(file) {
  const metadata = statSync(file.absolute);
  if (!metadata.isFile()) throw new Error(`${file.relative}: not a regular file`);
  if (metadata.size > MAX_FILE_BYTES) {
    throw new Error(`${file.relative}: exceeds ${MAX_FILE_BYTES} byte audit bound`);
  }
  if (lstatSync(file.absolute).isSymbolicLink()) {
    throw new Error(`${file.relative}: symbolic links are outside the audit boundary`);
  }
  return readFileSync(file.absolute, "utf8");
}

function lineNumber(text, offset) {
  let line = 1;
  for (let index = 0; index < offset; index += 1) {
    if (text.charCodeAt(index) === 10) line += 1;
  }
  return line;
}

function matches(file, text, patterns) {
  const findings = [];
  for (const { id, pattern } of patterns) {
    pattern.lastIndex = 0;
    for (const match of text.matchAll(pattern)) {
      findings.push({
        kind: id,
        path: file.relative,
        line: lineNumber(text, match.index ?? 0),
      });
    }
  }
  return findings;
}

export function validateContract(contract) {
  const errors = [];
  if (!isObject(contract)) throw new MigrationAuditError(["contract must be an object"]);
  if (contract.version !== 1) errors.push("version must equal 1");
  if (contract.status !== "migration-in-progress" && contract.status !== "seaorm-only") {
    errors.push("status must be migration-in-progress or seaorm-only");
  }
  if (contract.schemaAuthority?.repository !== "ORESoftware/k8s-libs-and-shared-defs") {
    errors.push("schema authority repository is incorrect");
  }
  if (!/^[0-9a-f]{40}$/u.test(contract.schemaAuthority?.commit ?? "")) {
    errors.push("schema authority commit must be a full lowercase SHA");
  }
  if (contract.schemaAuthority?.schemaPath !== "pg-defs/schema/schema.sql") {
    errors.push("schema authority path must remain pg-defs/schema/schema.sql");
  }
  if (contract.schemaAuthority?.seaOrmAdapterPath !== "pg-defs/rust/sea-orm") {
    errors.push("SeaORM adapter path must remain pg-defs/rust/sea-orm");
  }
  if (contract.declarativeMigrations?.repository !== "declarative-migrations/declarative-migrations") {
    errors.push("declarative-migrations repository is incorrect");
  }
  if (contract.declarativeMigrations?.serviceStartupMigrations !== false) {
    errors.push("service startup migrations must remain disabled");
  }
  if (contract.applicationPersistence?.targetOrm !== "SeaORM") {
    errors.push("target ORM must be SeaORM");
  }
  if (contract.applicationPersistence?.directSqlxTarget !== 0) {
    errors.push("direct SQLx target must remain zero");
  }
  if (contract.applicationPersistence?.directTokioPostgresTarget !== 0) {
    errors.push("direct tokio-postgres target must remain zero");
  }
  if (contract.uiPolicy?.rendererSpecificPersistence !== false) {
    errors.push("renderer-specific persistence must remain disabled");
  }
  if (errors.length > 0) throw new MigrationAuditError(errors);
  return structuredClone(contract);
}

export function auditRepository(root = repositoryRoot) {
  const contract = validateContract(JSON.parse(readFileSync(path.join(root, "seaorm-migration.json"), "utf8")));
  const findings = {
    directSqlx: [],
    directTokioPostgres: [],
    startupMigrations: [],
  };
  const scannedFiles = [];
  for (const file of walk(root)) {
    const text = boundedText(file);
    scannedFiles.push(file.relative);
    findings.directSqlx.push(...matches(file, text, DIRECT_SQLX_PATTERNS));
    findings.directTokioPostgres.push(...matches(file, text, DIRECT_TOKIO_POSTGRES_PATTERNS));
    if (file.relative !== "scripts/audit-seaorm-migration.mjs") {
      findings.startupMigrations.push(...matches(file, text, STARTUP_MIGRATION_PATTERNS));
    }
  }

  const errors = [];
  if (findings.startupMigrations.length > 0) {
    errors.push(`service startup migration calls detected: ${findings.startupMigrations.length}`);
  }
  if (contract.status === "seaorm-only") {
    if (findings.directSqlx.length > 0) {
      errors.push(`direct SQLx findings remain in seaorm-only mode: ${findings.directSqlx.length}`);
    }
    if (findings.directTokioPostgres.length > 0) {
      errors.push(
        `direct tokio-postgres findings remain in seaorm-only mode: ${findings.directTokioPostgres.length}`,
      );
    }
  }

  const report = {
    version: 1,
    status: contract.status,
    schemaAuthority: structuredClone(contract.schemaAuthority),
    declarativeMigrations: {
      repository: contract.declarativeMigrations.repository,
      version: contract.declarativeMigrations.version,
    },
    scannedFileCount: scannedFiles.length,
    counts: {
      directSqlx: findings.directSqlx.length,
      directTokioPostgres: findings.directTokioPostgres.length,
      startupMigrations: findings.startupMigrations.length,
    },
    findings,
    migrationComplete:
      findings.directSqlx.length === 0 &&
      findings.directTokioPostgres.length === 0 &&
      findings.startupMigrations.length === 0,
  };

  if (errors.length > 0) {
    const error = new MigrationAuditError(errors);
    error.report = report;
    throw error;
  }
  return report;
}

function parseArguments(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (key !== "--report" || !value || values.has(key)) {
      throw new Error("usage: audit-seaorm-migration.mjs [--report <repository-relative-path>]");
    }
    values.set(key, value);
  }
  return values;
}

if (process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const arguments_ = parseArguments(process.argv.slice(2));
  const report = auditRepository();
  const outputPath = arguments_.get("--report");
  if (outputPath) {
    if (!safeRelativePath(outputPath)) throw new Error("report path must remain repository relative");
    const absolute = path.resolve(repositoryRoot, outputPath);
    const parent = realpathSync(path.dirname(absolute));
    if (!parent.startsWith(`${repositoryRoot}${path.sep}`) && parent !== repositoryRoot) {
      throw new Error("report parent escapes the repository");
    }
    writeFileSync(absolute, `${JSON.stringify(report, null, 2)}\n`, { flag: "w", mode: 0o600 });
  }
  console.log(JSON.stringify(report, null, 2));
}
