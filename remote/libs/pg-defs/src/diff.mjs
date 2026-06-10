// IMPORTANT FOR CODING AGENTS:
// - schema/schema.sql is the final source of truth for database shape.
// - Generated ORM/client files are adapters only; never treat them as migration authority.
// - This script only generates reviewable SQL. Never run or apply migrations automatically.
// - Wait for explicit user approval before any database write in any environment.
// - COVERAGE GAP: tables, columns, CHECK constraints, and indexes are diffed. Functions and
//   triggers (e.g. notify_presence_member_change, presence_conv_members_notify) are NOT — they
//   must be verified manually via psql. The sql-contract parser silently skips
//   `create or replace function` / `create trigger` statements today.
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
const DEFAULT_DATABASE_URL_ENV_KEYS = [
  "AGENT_TASKS_RDS_DATABASE_URL",
  "RDS_DATABASE_URL",
  "DATABASE_URL",
  "PG_DATABASE_URL",
];
const includeExtraTables = args.includes("--include-extra-tables");
const outDir = path.resolve(packageRoot, "tmp", "migrations", env);
const outputPath = path.resolve(outDir, "pg-defs-diff.sql");
const desiredSqlPath = path.resolve(outDir, "desired-schema.sql");
const { contract, sourceSql, schemaPath } = await loadSqlContract(packageRoot);

if (args.includes("--parse-only")) {
  console.log(
    `Parsed ${contract.tables.length} table(s), ${contract.routines.length} routine(s), and ${contract.triggers.length} trigger(s) from ${path.relative(process.cwd(), schemaPath)}: ${contract.tables
      .map((table) => `${table.name}(${table.columns.length} columns)`)
      .join(", ")}`,
  );
  console.log(
    `Parsed ${(contract.routines ?? []).length} routine(s): ${(contract.routines ?? [])
      .map((routine) => routineSignature(routine))
      .join(", ")}`,
  );
  console.log(
    `Parsed ${(contract.triggers ?? []).length} trigger(s): ${(contract.triggers ?? [])
      .map((trigger) => `${trigger.tableName}.${trigger.name}`)
      .join(", ")}`,
  );
  console.log("No database connection opened and no migration SQL written.");
  process.exit(0);
}

// Schemas the contract owns. `public` is always included (routines/triggers live there); any
// non-public schema (e.g. `benefactor`) is discovered from the parsed tables so introspection and
// the emitted diff cover it.
const contractSchemas = [...new Set(["public", ...contract.tables.map((table) => table.schema ?? "public")])];

const catalogJsonPath = argValue("--catalog-json");
const actualSchema = catalogJsonPath
  ? hydrateActualSchema(JSON.parse(await readFile(path.resolve(process.cwd(), catalogJsonPath), "utf8")))
  : await introspectDatabase(await resolveDatabaseUrl(), contractSchemas);
const diffSql = generateDiffSql({
  contract,
  actualSchema,
  env,
  includeExtraTables,
  schemaPath,
});

await mkdir(outDir, { recursive: true });
await writeFile(desiredSqlPath, sourceSql.endsWith("\n") ? sourceSql : `${sourceSql}\n`);
await writeFile(outputPath, diffSql);

console.log(`Generated remote pg-defs diff for ${env}:`);
console.log(`  desired: ${path.relative(process.cwd(), desiredSqlPath)}`);
console.log(`  diff:    ${path.relative(process.cwd(), outputPath)}`);
console.log("Review this SQL manually. This tool does not apply migrations.");

function generateDiffSql({ contract, actualSchema, env, includeExtraTables, schemaPath }) {
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

  // Ensure any non-public schema the contract owns exists before its tables are created — but only
  // when that schema actually has a missing table, so a fully-synced database does not emit
  // spurious `create schema` lines above a "no differences" footer. Idempotent on a fresh DB.
  const schemasWithMissingTables = new Set(
    contract.tables
      .filter((table) => (table.schema ?? "public") !== "public")
      .filter((table) => !actualSchema.tables.has(tableKey(table)))
      .map((table) => table.schema),
  );
  for (const schema of schemasWithMissingTables) {
    lines.push(`create schema if not exists ${quoteIdent(schema)};`);
    lines.push("");
    changeCount += 1;
  }

  for (const table of contract.tables) {
    const actualTable = actualSchema.tables.get(tableKey(table));
    if (!actualTable) {
      lines.push(`-- Create missing table: ${qualifiedTableLabel(table)}`);
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
        lines.push(`alter table ${qualifiedTable(table)} add column if not exists ${column.definitionSql};`);
        lines.push("");
        changeCount += 1;
        continue;
      }

      const desiredType = columnTypeSql(column);
      const actualType = normalizeActualType(actualColumn);
      if (!typesEquivalent(column, actualType)) {
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

      if (!defaultsEquivalent(column, actualColumn.defaultSql)) {
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
          `alter table ${qualifiedTable(table)} add constraint ${quoteIdent(checkConstraint.name)} check (${checkConstraint.sql}) not valid;`,
        );
        lines.push(`alter table ${qualifiedTable(table)} validate constraint ${quoteIdent(checkConstraint.name)};`);
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

  if (includeExtraTables) {
    for (const actualTable of actualSchema.tables.values()) {
      if (!contract.tables.some((table) => tableKey(table) === tableKey(actualTable))) {
        lines.push(`-- MANUAL REVIEW: database has extra table ${qualifiedTableLabel(actualTable)}.`);
        lines.push("-- No DROP TABLE generated automatically.");
        lines.push("");
        changeCount += 1;
      }
    }
  }

  for (const routine of contract.routines ?? []) {
    const actualRoutine = actualSchema.routines.get(routineKey(routine));
    if (!actualRoutine) {
      lines.push(`-- Create missing function: ${routineSignature(routine)}`);
      lines.push(ensureSemicolon(routine.createStatement));
      lines.push("");
      changeCount += 1;
      continue;
    }

    if (!routineEquivalent(routine, actualRoutine)) {
      lines.push(`-- Replace differing function: ${routineSignature(routine)}`);
      lines.push(ensureSemicolon(routine.createStatement));
      lines.push("");
      changeCount += 1;
    }
  }

  for (const trigger of contract.triggers ?? []) {
    const actualTrigger = actualSchema.triggers.get(triggerKey(trigger));
    if (!actualTrigger) {
      lines.push(`-- Create missing trigger: ${trigger.tableName}.${trigger.name}`);
      lines.push(dropTriggerSql(trigger));
      lines.push(ensureSemicolon(trigger.createStatement));
      lines.push("");
      changeCount += 1;
      continue;
    }

    if (!triggerEquivalent(trigger, actualTrigger)) {
      lines.push(`-- Replace differing trigger: ${trigger.tableName}.${trigger.name}`);
      lines.push(dropTriggerSql(trigger));
      lines.push(ensureSemicolon(trigger.createStatement));
      lines.push("");
      changeCount += 1;
    }
  }

  if (changeCount === 0) {
    lines.push("-- No schema differences detected for pg-defs-owned tables, routines, or triggers.");
    lines.push("");
  }

  lines.push("COMMIT;");
  lines.push("");
  lines.push(`-- Change items emitted: ${changeCount}`);
  return `${lines.join("\n")}\n`;
}

async function introspectDatabase(databaseUrl, schemas = ["public"]) {
  // Build a SQL string list from the contract's schemas. Names originate from the trusted
  // schema.sql source, so single-quoting them is sufficient. Tables/columns/checks/indexes are
  // introspected across all owned schemas; routines and triggers remain public-only.
  const schemaList = schemas.map((schema) => `'${String(schema).replace(/'/g, "''")}'`).join(", ");
  const rows = await queryJson(databaseUrl, `
    select json_build_object(
      'tables', coalesce((
        select json_agg(row_to_json(t) order by t.table_schema, t.table_name)
        from (
          select table_schema, table_name
          from information_schema.tables
          where table_schema in (${schemaList})
            and table_type = 'BASE TABLE'
        ) t
      ), '[]'::json),
      'columns', coalesce((
        select json_agg(row_to_json(c) order by c.table_schema, c.table_name, c.ordinal_position)
        from (
          select
            table_schema,
            table_name,
            column_name,
            ordinal_position,
            data_type,
            udt_name,
            is_nullable,
            column_default,
            character_maximum_length
          from information_schema.columns
          where table_schema in (${schemaList})
        ) c
      ), '[]'::json),
      'checks', coalesce((
        select json_agg(row_to_json(ch) order by ch.table_schema, ch.table_name, ch.constraint_name)
        from (
          select
            nsp.nspname as table_schema,
            rel.relname as table_name,
            con.conname as constraint_name,
            pg_get_constraintdef(con.oid) as definition
          from pg_constraint con
          join pg_class rel on rel.oid = con.conrelid
          join pg_namespace nsp on nsp.oid = rel.relnamespace
          where nsp.nspname in (${schemaList})
            and con.contype = 'c'
        ) ch
      ), '[]'::json),
      'indexes', coalesce((
        select json_agg(row_to_json(i) order by i.table_schema, i.table_name, i.index_name)
        from (
          select
            nsp.nspname as table_schema,
            tab.relname as table_name,
            idx.relname as index_name,
            pg_get_indexdef(idx.oid) as definition
          from pg_index ind
          join pg_class idx on idx.oid = ind.indexrelid
          join pg_class tab on tab.oid = ind.indrelid
          join pg_namespace nsp on nsp.oid = tab.relnamespace
          where nsp.nspname in (${schemaList})
            and not ind.indisprimary
        ) i
      ), '[]'::json),
      'routines', coalesce((
        select json_agg(row_to_json(r) order by r.routine_name, r.identity_arguments)
        from (
          select
            proc.proname as routine_name,
            pg_get_function_identity_arguments(proc.oid) as identity_arguments,
            pg_get_function_result(proc.oid) as result_type,
            lang.lanname as language,
            case proc.provolatile
              when 'i' then 'immutable'
              when 's' then 'stable'
              else 'volatile'
            end as volatility,
            proc.prosrc as body_sql
          from pg_proc proc
          join pg_namespace nsp on nsp.oid = proc.pronamespace
          join pg_language lang on lang.oid = proc.prolang
          where nsp.nspname = 'public'
        ) r
      ), '[]'::json),
      'triggers', coalesce((
        select json_agg(row_to_json(tg) order by tg.table_name, tg.trigger_name)
        from (
          select
            event_object_table as table_name,
            trigger_name,
            lower(action_timing) as timing,
            array_agg(lower(event_manipulation) order by lower(event_manipulation)) as events,
            lower(action_orientation) as orientation,
            action_statement
          from information_schema.triggers
          where trigger_schema = 'public'
          group by event_object_table, trigger_name, action_timing, action_orientation, action_statement
        ) tg
      ), '[]'::json)
    ) as schema_json;
  `);

  return hydrateActualSchema(rows);
}

function hydrateActualSchema(rows) {
  const tables = new Map();
  const routines = new Map();
  const triggers = new Map();
  // Tables are keyed by `schema.table` so a non-public table cannot collide with a public table of
  // the same bare name. `table_schema` defaults to `public` for older catalog-json fixtures.
  for (const table of rows.tables ?? []) {
    const schema = table.table_schema ?? "public";
    tables.set(actualTableKey(schema, table.table_name), {
      schema,
      name: table.table_name,
      columns: new Map(),
      checks: new Map(),
      indexes: new Map(),
    });
  }

  for (const column of rows.columns ?? []) {
    const table = tables.get(actualTableKey(column.table_schema ?? "public", column.table_name));
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

  for (const check of rows.checks ?? []) {
    tables
      .get(actualTableKey(check.table_schema ?? "public", check.table_name))
      ?.checks.set(check.constraint_name, {
        name: check.constraint_name,
        definition: check.definition,
      });
  }

  for (const index of rows.indexes ?? []) {
    tables
      .get(actualTableKey(index.table_schema ?? "public", index.table_name))
      ?.indexes.set(index.index_name, {
        name: index.index_name,
        definition: index.definition,
      });
  }

  for (const routine of rows.routines ?? []) {
    const normalized = {
      name: routine.routine_name,
      identityArguments: normalizeRoutineArgs(routine.identity_arguments ?? ""),
      returns: routine.result_type,
      language: routine.language,
      volatility: routine.volatility,
      bodySql: routine.body_sql ?? "",
    };
    routines.set(routineKey(normalized), normalized);
  }

  for (const trigger of rows.triggers ?? []) {
    const normalized = {
      name: trigger.trigger_name,
      tableName: trigger.table_name,
      timing: trigger.timing,
      events: trigger.events ?? [],
      orientation: trigger.orientation,
      functionName: triggerFunctionName(trigger.action_statement),
      functionArguments: "",
      actionStatement: trigger.action_statement,
    };
    triggers.set(triggerKey(normalized), normalized);
  }

  return { tables, routines, triggers };
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

  const explicitEnvKey = argValue("--database-url-env");
  const envKeys = explicitEnvKey ? [explicitEnvKey] : DEFAULT_DATABASE_URL_ENV_KEYS;
  const envFile = await resolveEnvFile();
  if (envFile) {
    const envValues = parseEnvFile(await readFile(envFile, "utf8"));
    for (const envKey of envKeys) {
      if (envValues[envKey]) {
        return envValues[envKey];
      }
    }
  }

  for (const envKey of envKeys) {
    if (process.env[envKey]) {
      return process.env[envKey];
    }
  }

  throw new Error(
    `No database URL found. Set one of ${envKeys.join(", ")}, pass --database-url, or provide --env-file with one of those keys.`,
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
  if (column.udtName === "int8") {
    return "bigint";
  }
  if (column.udtName === "bool") {
    return "boolean";
  }
  return column.udtName;
}

function defaultsEquivalent(column, actualDefault) {
  if ((column.sqlType === "bigserial" || column.sqlType === "serial") && isSequenceDefault(actualDefault)) {
    return true;
  }

  const desiredDefault = column.defaultSql;
  if (!desiredDefault && !actualDefault) {
    return true;
  }
  if (!desiredDefault || !actualDefault) {
    return false;
  }
  return normalizeDefault(desiredDefault) === normalizeDefault(actualDefault);
}

function normalizeDefault(value) {
  const normalized = value
    .replace(/::[\w\s.[\]"]+/g, "")
    .replace(/\bpublic\./g, "")
    .replace(/\s+/g, " ")
    .trim();
  const unwrapped = stripOuterParens(normalized);
  const numericString = unwrapped.match(/^'(-?\d+)'$/);
  if (numericString) {
    return numericString[1];
  }
  const booleanString = unwrapped.match(/^'(true|false)'$/i);
  if (booleanString) {
    return booleanString[1].toLowerCase();
  }
  return unwrapped
    .toLowerCase();
}

function checkEquivalent(desired, actualDefinition) {
  return normalizeCheck(desired) === normalizeCheck(actualDefinition);
}

function routineEquivalent(desired, actual) {
  return (
    normalizeRoutineArgs(desired.identityArguments) === normalizeRoutineArgs(actual.identityArguments) &&
    normalizePgType(desired.returns) === normalizePgType(actual.returns) &&
    desired.language.toLowerCase() === actual.language.toLowerCase() &&
    desired.volatility.toLowerCase() === actual.volatility.toLowerCase() &&
    normalizeRoutineBody(desired.bodySql) === normalizeRoutineBody(actual.bodySql)
  );
}

function triggerEquivalent(desired, actual) {
  return (
    desired.timing.toLowerCase() === actual.timing.toLowerCase() &&
    normalizeStringList(desired.events).join(",") === normalizeStringList(actual.events).join(",") &&
    desired.orientation.toLowerCase() === actual.orientation.toLowerCase() &&
    desired.functionName.toLowerCase() === actual.functionName.toLowerCase()
  );
}

function routineKey(routine) {
  return `${routine.name}(${normalizeRoutineArgs(routine.identityArguments ?? routine.argumentsSql ?? "")})`;
}

function routineSignature(routine) {
  return `${routine.name}(${routine.identityArguments ?? ""})`;
}

function triggerKey(trigger) {
  return `${trigger.tableName}.${trigger.name}`;
}

function dropTriggerSql(trigger) {
  return `drop trigger if exists ${quoteIdent(trigger.name)} on ${quoteIdent(trigger.tableName)};`;
}

function triggerFunctionName(actionStatement) {
  const match = actionStatement.match(/\b(?:function|procedure)\s+("?[\w]+"?)\s*\(/i);
  return match ? match[1].replace(/^"|"$/g, "") : actionStatement;
}

function normalizeRoutineArgs(value) {
  return String(value ?? "")
    .replace(/\s+default\s+(?:'[^']*(?:''[^']*)*'|[^,\s)]+)/gi, "")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();
}

function normalizeStringList(value) {
  return (value ?? []).map((item) => String(item).toLowerCase()).sort();
}

function normalizePgType(value) {
  const normalized = String(value ?? "")
    .replace(/\bpublic\./g, "")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();
  switch (normalized) {
    case "int":
    case "int4":
      return "integer";
    case "bool":
      return "boolean";
    case "timestamp with time zone":
      return "timestamptz";
    default:
      return normalized;
  }
}

function normalizeRoutineBody(value) {
  return String(value ?? "")
    .split("\n")
    .filter((line) => !line.trim().startsWith("--"))
    .join("\n")
    .replace(/\bpublic\./g, "")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();
}

function normalizeCheck(value) {
  // Postgres deparses stored CHECK definitions in a heavily parenthesised,
  // cast-annotated form that's semantically identical to the lossy SQL we
  // ship in schema.sql. Walk through the deparse-noise transforms in a
  // fixed order until the string stabilises so simple shapes like
  // `between`, `in`, `is null`, and `varchar` cast wraps don't show up
  // as false-positive drift.
  let normalized = value
    .replace(/^CHECK\s*/i, "")
    // Strip type casts including their `[]` array suffix so `::text[]`
    // doesn't leave behind a stray `[]`.
    .replace(
      /::(?:text|character varying|integer|int|int4|int8|bigint|boolean|bool|jsonb|json|uuid|numeric|real|double precision|timestamp with time zone|timestamp without time zone|timestamptz|timestamp|date|smallint|bytea)(\s*\[\])?/gi,
      "",
    )
    .replace(/::[\w.[\]"]+/g, "")
    .replace(/'(-?\d+)'/g, "$1")
    .replace(/'(true|false)'/gi, (_, value) => value.toLowerCase())
    .replace(/"/g, "")
    .replace(/\s+/g, " ")
    .trim();

  // Pre-pass: strip parens around bare identifiers so the LHS of an
  // `(col)::text = ANY (...)` has its bare `col` exposed before the
  // `= ANY` → `IN` rewrite tries to match.
  normalized = normalized.replace(/(?<![a-z0-9_])\(\s*([a-z_][\w]*)\s*\)/gi, "$1");

  // `col = any ((array[...])[])` (PG deparse) → `col in (...)`. Tolerates
  // the optional `[]` suffix the array-cast strip leaves behind. Done
  // before the paren-flattening loop so subsequent passes can strip the
  // resulting `(col in (...))` wrap with the same rule that strips
  // other paren-wrapped clauses.
  normalized = normalized.replace(
    /([a-z_][\w]*)\s*=\s*any\s*\(\s*\(?\s*array\s*\[([^\]]+)\]\s*\)?\s*(?:\[\])?\s*\)/gi,
    "$1 in ($2)",
  );
  normalized = normalized.replace(
    /([a-z_][\w]*)\s*=\s*any\s*\(\s*array\s*\[([^\]]+)\]\s*\)/gi,
    "$1 in ($2)",
  );

  // Repeatedly apply paren-flattening passes until idempotent so nested
  // wraps like `(((x is null)))` collapse cleanly without depth-specific
  // regexes.
  for (let i = 0; i < 8; i += 1) {
    const before = normalized;
    normalized = stripOuterParens(normalized);
    // `(ident)` — but ONLY when not glued to a function name. The previous
    // version dropped the open paren of `octet_length(display_name)`.
    normalized = normalized.replace(/(?<![a-z0-9_])\(\s*([a-z_][\w]*)\s*\)/gi, "$1");
    // `(col is null)` / `(col is not null)`
    normalized = normalized.replace(
      /\(\s*([a-z_][\w]*)\s+(is\s+(?:not\s+)?null)\s*\)/gi,
      "$1 $2",
    );
    // `(LHS OP RHS)` — strip parens around simple binary comparisons where
    // LHS is an ident or a balanced function call (`octet_length(col)`),
    // RHS is anything without inner parens. Done in a paren-context-safe
    // way so `(octet_length(col) <= N)` becomes `octet_length(col) <= N`.
    normalized = normalized.replace(
      /\(\s*([a-z_][\w]*(?:\([^()]*\))?)\s*(>=|<=|<>|!=|=|>|<)\s*([^()]+?)\s*\)/gi,
      "$1 $2 $3",
    );
    // `(col in (...))` and `(func(col) in (...))`
    normalized = normalized.replace(
      /\(\s*([a-z_][\w]*(?:\([^()]*\))?\s+in\s*\([^()]*\))\s*\)/gi,
      "$1",
    );
    normalized = normalized.replace(
      /([a-z_][\w]*)\s*=\s*any\s*\(\s*array\s*\[([^\]]+)\]\s*\)/gi,
      "$1 in ($2)",
    );
    // `(X) AND (Y)` / `(X) OR (Y)` — strip parens around top-level boolean
    // operands when each operand is itself a simple comparison/predicate.
    normalized = normalized.replace(
      /\(\s*([a-z_][\w]*(?:\([^()]*\))?\s+(?:is\s+(?:not\s+)?null|in\s*\([^()]*\)|(?:>=|<=|<>|!=|=|>|<|between)[^()]+?))\s*\)/gi,
      "$1",
    );
    if (normalized === before) break;
  }

  // `(LHS >= N) AND (LHS <= M)` → `LHS between N and M`. LHS may be a
  // bare identifier or a balanced function call. Bounds must be integer
  // literals so this stays safe (no cross-column comparisons).
  normalized = normalized.replace(
    /([a-z_][\w]*(?:\([^()]*\))?)\s*>=\s*(-?\d+)\s+and\s+\1\s*<=\s*(-?\d+)/gi,
    "$1 between $2 and $3",
  );

  // Final cleanup pass: paren strip + outer wrap + ws collapse.
  for (let i = 0; i < 4; i += 1) {
    const before = normalized;
    normalized = stripOuterParens(normalized);
    normalized = normalized.replace(/(?<![a-z0-9_])\(\s*([a-z_][\w]*)\s*\)/gi, "$1");
    normalized = normalized.replace(/\s+/g, " ").trim();
    if (normalized === before) break;
  }
  normalized = normalized.replace(
    /^\(\s*([a-z_][\w]*(?:\([^()]*\))?\s+in\s*\([^()]*\))$/i,
    "$1",
  );
  return normalized.toLowerCase();
}

function columnTypeSql(column) {
  if (column.sqlType === "varchar") {
    return `varchar(${column.maxLength})`;
  }
  return column.sqlType;
}

function typesEquivalent(column, actualType) {
  if (column.sqlType === "bigserial") {
    return actualType === "bigint";
  }
  if (column.sqlType === "serial") {
    return actualType === "integer";
  }
  return columnTypeSql(column) === actualType;
}

function isSequenceDefault(value) {
  return typeof value === "string" && /^nextval\('/i.test(value.trim());
}

function stripOuterParens(value) {
  let current = value.trim();
  while (current.startsWith("(") && current.endsWith(")") && enclosesWholeExpression(current)) {
    current = current.slice(1, -1).trim();
  }
  return current;
}

function enclosesWholeExpression(value) {
  let depth = 0;
  let singleQuoted = false;
  for (let index = 0; index < value.length; index += 1) {
    const char = value[index];
    const next = value[index + 1];
    if (char === "'") {
      if (singleQuoted && next === "'") {
        index += 1;
        continue;
      }
      singleQuoted = !singleQuoted;
      continue;
    }
    if (singleQuoted) {
      continue;
    }
    if (char === "(") {
      depth += 1;
    }
    if (char === ")") {
      depth -= 1;
      if (depth === 0 && index < value.length - 1) {
        return false;
      }
    }
    if (depth < 0) {
      return false;
    }
  }
  return depth === 0;
}

function quoteIdent(value) {
  return `"${value.replace(/"/g, '""')}"`;
}

// Composite key for matching a desired contract table against an introspected one. Keying by
// `schema.table` keeps a non-public table distinct from a public table of the same bare name.
function tableKey(table) {
  return `${table.schema ?? "public"}.${table.name}`;
}

function actualTableKey(schema, name) {
  return `${schema ?? "public"}.${name}`;
}

// Quoted, schema-qualified table reference for emitted DDL (bare for the default public schema).
function qualifiedTable(table) {
  return table.schema && table.schema !== "public"
    ? `${quoteIdent(table.schema)}.${quoteIdent(table.name)}`
    : quoteIdent(table.name);
}

// Human-readable schema.table label for diff comments (bare for the default public schema).
function qualifiedTableLabel(table) {
  return table.schema && table.schema !== "public" ? `${table.schema}.${table.name}` : table.name;
}

function ensureSemicolon(value) {
  const trimmed = value.trim();
  return trimmed.endsWith(";") ? trimmed : `${trimmed};`;
}
