// IMPORTANT FOR CODING AGENTS:
// - schema/schema.sql is the final source of truth for database shape.
// - Generated ORM/client files are adapters only; never treat them as migration authority.
// - This script only generates reviewable SQL. Never run or apply migrations automatically.
// - Wait for explicit user approval before any database write in any environment.
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { loadSqlContract } from "./sql-contract.mjs";

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(packageRoot, "..", "..", "..");
const args = process.argv.slice(2);
const env = argValue("--env") ?? "dev";
const outDir = path.resolve(packageRoot, "tmp", "migrations", env);
const outputPath = path.resolve(outDir, "pg-defs-diff.sql");
const desiredSqlPath = path.resolve(outDir, "desired-schema.sql");
const { contract, sourceSql, schemaPath } = await loadSqlContract(packageRoot);

if (args.includes("--parse-only")) {
  console.log(
    `Parsed ${contract.tables.length} table(s) from ${path.relative(process.cwd(), schemaPath)}: ${contract.tables
      .map((table) => `${table.name}(${table.columns.length} columns)`)
      .join(", ")}`,
  );
  console.log("No database connection opened and no migration SQL written.");
  process.exit(0);
}

const databaseUrl = await resolveDatabaseUrl();
const actualSchema = await introspectDatabase(databaseUrl);
const diffSql = generateDiffSql({
  contract,
  actualSchema,
  env,
  schemaPath,
});

await mkdir(outDir, { recursive: true });
await writeFile(desiredSqlPath, sourceSql.endsWith("\n") ? sourceSql : `${sourceSql}\n`);
await writeFile(outputPath, diffSql);

console.log(`Generated remote pg-defs diff for ${env}:`);
console.log(`  desired: ${path.relative(process.cwd(), desiredSqlPath)}`);
console.log(`  diff:    ${path.relative(process.cwd(), outputPath)}`);
console.log("Review this SQL manually. This tool does not apply migrations.");

function generateDiffSql({ contract, actualSchema, env, schemaPath }) {
  const lines = [
    "-- Remote pg-defs SQL diff",
    `-- Environment: ${env}`,
    `-- Desired schema source: ${path.relative(repoRoot, schemaPath)}`,
    "-- SOURCE OF TRUTH: remote/libs/pg-defs/schema/schema.sql",
    "-- Generated ORM/client files are adapters only and must not drive migrations.",
    "-- SAFETY: review this file manually. Do not apply automatically.",
    `-- Generated: ${new Date().toISOString()}`,
    "",
    "BEGIN;",
    "",
  ];
  let changeCount = 0;

  for (const table of contract.tables) {
    const actualTable = actualSchema.tables.get(table.name);
    if (!actualTable) {
      lines.push(`-- Create missing table: ${table.name}`);
      lines.push(ensureSemicolon(table.createStatement));
      lines.push("");
      for (const tableIndex of table.indexes ?? []) {
        lines.push(ensureSemicolon(tableIndex.createStatement));
        lines.push("");
      }
      changeCount += 1;
      continue;
    }

    for (const column of table.columns) {
      const actualColumn = actualTable.columns.get(column.name);
      if (!actualColumn) {
        if (column.primaryKey) {
          lines.push(`-- MANUAL REVIEW: missing primary key column ${table.name}.${column.name}.`);
          lines.push(`-- Desired definition: ${column.definitionSql}`);
          lines.push("");
          changeCount += 1;
          continue;
        }

        lines.push(`-- Add missing column: ${table.name}.${column.name}`);
        lines.push(`alter table ${quoteIdent(table.name)} add column if not exists ${column.definitionSql};`);
        lines.push("");
        changeCount += 1;
        continue;
      }

      const desiredType = columnTypeSql(column);
      const actualType = normalizeActualType(actualColumn);
      if (desiredType !== actualType) {
        lines.push(`-- MANUAL REVIEW: type differs for ${table.name}.${column.name}.`);
        lines.push(`-- Desired: ${desiredType}`);
        lines.push(`-- Actual:  ${actualType}`);
        lines.push("-- No ALTER TYPE generated automatically because this can rewrite or truncate data.");
        lines.push("");
        changeCount += 1;
      }

      const desiredNullable = !column.notNull;
      if (desiredNullable !== actualColumn.isNullable) {
        lines.push(`-- MANUAL REVIEW: nullability differs for ${table.name}.${column.name}.`);
        lines.push(`-- Desired nullable: ${desiredNullable}`);
        lines.push(`-- Actual nullable:  ${actualColumn.isNullable}`);
        lines.push("-- No nullability ALTER generated automatically because existing data must be checked first.");
        lines.push("");
        changeCount += 1;
      }

      if (!defaultsEquivalent(column.defaultSql, actualColumn.defaultSql)) {
        lines.push(`-- MANUAL REVIEW: default differs for ${table.name}.${column.name}.`);
        lines.push(`-- Desired default: ${column.defaultSql ?? "none"}`);
        lines.push(`-- Actual default:  ${actualColumn.defaultSql ?? "none"}`);
        lines.push("-- No default ALTER generated automatically; confirm intent before changing write behavior.");
        lines.push("");
        changeCount += 1;
      }
    }

    for (const checkConstraint of table.checks ?? []) {
      const actualCheck = actualTable.checks.get(checkConstraint.name);
      if (!actualCheck) {
        lines.push(`-- Add missing check constraint: ${checkConstraint.name}`);
        lines.push(
          `alter table ${quoteIdent(table.name)} add constraint ${quoteIdent(checkConstraint.name)} check (${checkConstraint.sql}) not valid;`,
        );
        lines.push(`alter table ${quoteIdent(table.name)} validate constraint ${quoteIdent(checkConstraint.name)};`);
        lines.push("");
        changeCount += 1;
        continue;
      }

      if (!checkEquivalent(checkConstraint.sql, actualCheck.definition)) {
        lines.push(`-- MANUAL REVIEW: check constraint differs for ${checkConstraint.name}.`);
        lines.push(`-- Desired: check (${checkConstraint.sql})`);
        lines.push(`-- Actual:  ${actualCheck.definition}`);
        lines.push("-- No DROP/replace generated automatically.");
        lines.push("");
        changeCount += 1;
      }
    }

    for (const tableIndex of table.indexes ?? []) {
      if (!actualTable.indexes.has(tableIndex.name)) {
        lines.push(`-- Add missing index: ${tableIndex.name}`);
        lines.push(ensureSemicolon(tableIndex.createStatement));
        lines.push("");
        changeCount += 1;
      }
    }

    for (const actualColumn of actualTable.columns.values()) {
      if (!table.columns.some((column) => column.name === actualColumn.name)) {
        lines.push(`-- MANUAL REVIEW: database has extra column ${table.name}.${actualColumn.name}.`);
        lines.push("-- No DROP COLUMN generated automatically.");
        lines.push("");
        changeCount += 1;
      }
    }
  }

  for (const actualTable of actualSchema.tables.values()) {
    if (!contract.tables.some((table) => table.name === actualTable.name)) {
      lines.push(`-- MANUAL REVIEW: database has extra table ${actualTable.name}.`);
      lines.push("-- No DROP TABLE generated automatically.");
      lines.push("");
      changeCount += 1;
    }
  }

  if (changeCount === 0) {
    lines.push("-- No schema differences detected for pg-defs-owned tables.");
    lines.push("");
  }

  lines.push("COMMIT;");
  lines.push("");
  lines.push(`-- Change items emitted: ${changeCount}`);
  return `${lines.join("\n")}\n`;
}

async function introspectDatabase(databaseUrl) {
  const rows = await queryJson(databaseUrl, `
    select json_build_object(
      'tables', coalesce((
        select json_agg(row_to_json(t) order by t.table_name)
        from (
          select table_name
          from information_schema.tables
          where table_schema = 'public'
            and table_type = 'BASE TABLE'
        ) t
      ), '[]'::json),
      'columns', coalesce((
        select json_agg(row_to_json(c) order by c.table_name, c.ordinal_position)
        from (
          select
            table_name,
            column_name,
            ordinal_position,
            data_type,
            udt_name,
            is_nullable,
            column_default,
            character_maximum_length
          from information_schema.columns
          where table_schema = 'public'
        ) c
      ), '[]'::json),
      'checks', coalesce((
        select json_agg(row_to_json(ch) order by ch.table_name, ch.constraint_name)
        from (
          select
            rel.relname as table_name,
            con.conname as constraint_name,
            pg_get_constraintdef(con.oid) as definition
          from pg_constraint con
          join pg_class rel on rel.oid = con.conrelid
          join pg_namespace nsp on nsp.oid = rel.relnamespace
          where nsp.nspname = 'public'
            and con.contype = 'c'
        ) ch
      ), '[]'::json),
      'indexes', coalesce((
        select json_agg(row_to_json(i) order by i.table_name, i.index_name)
        from (
          select
            tab.relname as table_name,
            idx.relname as index_name,
            pg_get_indexdef(idx.oid) as definition
          from pg_index ind
          join pg_class idx on idx.oid = ind.indexrelid
          join pg_class tab on tab.oid = ind.indrelid
          join pg_namespace nsp on nsp.oid = tab.relnamespace
          where nsp.nspname = 'public'
            and not ind.indisprimary
        ) i
      ), '[]'::json)
    ) as schema_json;
  `);

  const tables = new Map();
  for (const table of rows.tables) {
    tables.set(table.table_name, {
      name: table.table_name,
      columns: new Map(),
      checks: new Map(),
      indexes: new Map(),
    });
  }

  for (const column of rows.columns) {
    const table = tables.get(column.table_name);
    if (!table) {
      continue;
    }
    table.columns.set(column.column_name, {
      name: column.column_name,
      dataType: column.data_type,
      udtName: column.udt_name,
      isNullable: column.is_nullable === "YES",
      defaultSql: column.column_default,
      maxLength: column.character_maximum_length,
    });
  }

  for (const check of rows.checks) {
    tables.get(check.table_name)?.checks.set(check.constraint_name, {
      name: check.constraint_name,
      definition: check.definition,
    });
  }

  for (const index of rows.indexes) {
    tables.get(index.table_name)?.indexes.set(index.index_name, {
      name: index.index_name,
      definition: index.definition,
    });
  }

  return { tables };
}

async function queryJson(databaseUrl, sql) {
  const output = await runPsql(databaseUrl, sql);
  const trimmed = output.trim();
  if (!trimmed) {
    throw new Error("psql returned no JSON output");
  }
  return JSON.parse(trimmed);
}

function runPsql(databaseUrl, sql) {
  return new Promise((resolve, reject) => {
    const child = spawn("psql", [
      databaseUrl,
      "-X",
      "-q",
      "-t",
      "-A",
      "-v",
      "ON_ERROR_STOP=1",
      "-c",
      sql,
    ]);
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    child.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) {
        resolve(stdout);
        return;
      }
      reject(new Error(`psql catalog query failed with code ${code}: ${stderr.trim()}`));
    });
  });
}

async function resolveDatabaseUrl() {
  const explicit = argValue("--database-url");
  if (explicit) {
    return explicit;
  }

  const envKey = argValue("--database-url-env") ?? "DATABASE_URL";
  const envFile = await resolveEnvFile();
  if (envFile) {
    const envValues = parseEnvFile(await readFile(envFile, "utf8"));
    if (envValues[envKey]) {
      return envValues[envKey];
    }
  }

  if (process.env[envKey]) {
    return process.env[envKey];
  }

  throw new Error(
    `No database URL found. Set ${envKey}, pass --database-url, or provide --env-file with ${envKey}.`,
  );
}

async function resolveEnvFile() {
  const explicit = argValue("--env-file");
  if (explicit) {
    return path.resolve(process.cwd(), explicit);
  }

  const candidates = [
    path.resolve(process.cwd(), "env", `.${env}.env`),
    path.resolve(packageRoot, "env", `.${env}.env`),
    path.resolve(packageRoot, "..", "..", "env", `.${env}.env`),
    path.resolve(repoRoot, "env", `.${env}.env`),
  ];

  return candidates.find((candidate) => existsSync(candidate));
}

function parseEnvFile(contents) {
  const values = {};
  for (const rawLine of contents.split("\n")) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#")) {
      continue;
    }
    const equalsIndex = line.indexOf("=");
    if (equalsIndex === -1) {
      continue;
    }
    const key = line.slice(0, equalsIndex).trim();
    let value = line.slice(equalsIndex + 1).trim();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
      value = value.slice(1, -1);
    }
    values[key] = value;
  }
  return values;
}

function argValue(name) {
  const match = args.find((arg) => arg === name || arg.startsWith(`${name}=`));
  if (!match) {
    return undefined;
  }
  if (match === name) {
    const index = args.indexOf(match);
    return args[index + 1];
  }
  return match.slice(name.length + 1);
}

function normalizeActualType(column) {
  if (column.udtName === "varchar") {
    return `varchar(${column.maxLength})`;
  }
  if (column.udtName === "timestamptz") {
    return "timestamptz";
  }
  if (column.udtName === "int4") {
    return "integer";
  }
  if (column.udtName === "bool") {
    return "boolean";
  }
  return column.udtName;
}

function defaultsEquivalent(desiredDefault, actualDefault) {
  if (!desiredDefault && !actualDefault) {
    return true;
  }
  if (!desiredDefault || !actualDefault) {
    return false;
  }
  return normalizeDefault(desiredDefault) === normalizeDefault(actualDefault);
}

function normalizeDefault(value) {
  return value
    .replace(/::[\w\s.[\]"]+/g, "")
    .replace(/\bpublic\./g, "")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();
}

function checkEquivalent(desired, actualDefinition) {
  const normalizedActual = actualDefinition.replace(/^CHECK\s*\(/i, "").replace(/\)$/i, "");
  return normalizeCheck(desired) === normalizeCheck(normalizedActual);
}

function normalizeCheck(value) {
  return value.replace(/\s+/g, " ").replace(/"/g, "").trim().toLowerCase();
}

function columnTypeSql(column) {
  if (column.sqlType === "varchar") {
    return `varchar(${column.maxLength})`;
  }
  return column.sqlType;
}

function quoteIdent(value) {
  return `"${value.replace(/"/g, '""')}"`;
}

function ensureSemicolon(value) {
  const trimmed = value.trim();
  return trimmed.endsWith(";") ? trimmed : `${trimmed};`;
}
