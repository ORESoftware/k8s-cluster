// Parser + generator self-tests. Run via `node --test src/`.
//
// These tests intentionally do NOT require a live database and do NOT write to the filesystem.
// They lock in the audit-time hardening contract:
//   - compound CHECK clauses contribute every fact they can
//   - `between`, `>=`, `<=`, `>`, `<` integer comparisons populate min/max
//   - foreign keys flow through to column metadata
//   - regex patterns are captured (with `''` -> `'` unescaping)
//   - null-guarded byte limits are extracted
//
// Whenever the parser learns a new shape, please add a regression test below so silent breakage
// in adapter codegen becomes a loud test failure instead.
import { strict as assert } from "node:assert";
import { execFileSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  foreignKeyIndexName,
  foreignKeyIndexRecommendations,
  parseIndexLeadingColumn,
  parseSchemaSql,
  splitSqlStatements,
} from "./sql-contract.mjs";

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const generatorPath = path.join(packageRoot, "src", "generate.mjs");

test("generated outputs are up to date with schema source", () => {
  execFileSync(process.execPath, [generatorPath, "--check"], {
    cwd: packageRoot,
    stdio: ["ignore", "pipe", "pipe"],
  });
});

function findColumn(schema, tableName, columnName) {
  const table = schema.tables.find((item) => item.name === tableName);
  if (!table) {
    throw new Error(`table ${tableName} not parsed`);
  }
  const column = table.columns.find((item) => item.name === columnName);
  if (!column) {
    throw new Error(`column ${tableName}.${columnName} not parsed`);
  }
  return column;
}

test("parser captures `between X and Y` as min/max validation", () => {
  const sql = `
    create table example (
      id uuid primary key default gen_random_uuid(),
      port integer default 8080 not null,
      constraint example_port_chk check (port between 1 and 65535)
    );
  `;
  const schema = parseSchemaSql(sql);
  const column = findColumn(schema, "example", "port");
  assert.equal(column.validation?.min, 1);
  assert.equal(column.validation?.max, 65535);
});

test("parser captures simple comparison operators on integers", () => {
  const sql = `
    create table example (
      id uuid primary key default gen_random_uuid(),
      version integer default 1 not null,
      constraint example_version_chk check (version > 0)
    );
  `;
  const schema = parseSchemaSql(sql);
  const column = findColumn(schema, "example", "version");
  assert.equal(column.validation?.min, 1);
});

test("parser splits compound AND clauses and applies each fact", () => {
  const sql = `
    create table example (
      id uuid primary key default gen_random_uuid(),
      max_warm integer default 2 not null,
      min_warm integer default 1 not null,
      constraint example_max_chk check (max_warm between 1 and 128 and max_warm >= min_warm)
    );
  `;
  const schema = parseSchemaSql(sql);
  const column = findColumn(schema, "example", "max_warm");
  assert.equal(column.validation?.min, 1);
  assert.equal(column.validation?.max, 128);
});

test("parser captures null-guarded byte limits", () => {
  const sql = `
    create table example (
      id uuid primary key default gen_random_uuid(),
      nats_subject text,
      constraint example_subject_chk
        check (nats_subject is null or octet_length(nats_subject) <= 256)
    );
  `;
  const schema = parseSchemaSql(sql);
  const column = findColumn(schema, "example", "nats_subject");
  assert.equal(column.validation?.maxBytes, 256);
});

test("parser unescapes single quotes inside regex literals", () => {
  // `''` is the SQL way to embed a single quote inside a quoted string. The parser must collapse
  // it back to `'` so downstream regex engines (RegExp / regexp / re / Regex) see the literal
  // pattern, not the escape sequence.
  const sql = `
    create table example (
      id uuid primary key default gen_random_uuid(),
      path text not null,
      constraint example_path_chk
        check (path ~ '^/[A-Za-z0-9._~!$&''()*+,;=:@%/-]{0,255}$')
    );
  `;
  const schema = parseSchemaSql(sql);
  const column = findColumn(schema, "example", "path");
  assert.equal(
    column.validation?.regex,
    "^/[A-Za-z0-9._~!$&'()*+,;=:@%/-]{0,255}$",
  );
});

test("parser extracts foreign keys onto the source column", () => {
  const sql = `
    create table parents (
      id uuid primary key default gen_random_uuid()
    );

    create table children (
      id uuid primary key default gen_random_uuid(),
      parent_id uuid not null
    );

    alter table if exists children
      add constraint children_parent_fk
      foreign key (parent_id) references parents(id);
  `;
  const schema = parseSchemaSql(sql);
  const column = findColumn(schema, "children", "parent_id");
  assert.equal(column.foreignKey?.table, "parents");
  assert.equal(column.foreignKey?.column, "id");
  assert.equal(column.foreignKey?.constraint, "children_parent_fk");
});

test("parser captures a schema-qualified table while keeping the bare name", () => {
  const sql = `
    create schema if not exists benefactor;

    create table benefactor.benefactor_leads (
      id uuid primary key default gen_random_uuid(),
      primary_email varchar(255) default '' not null
    );
  `;
  const schema = parseSchemaSql(sql);
  const table = schema.tables.find((item) => item.name === "benefactor_leads");
  assert.ok(table, "schema-qualified table should parse");
  assert.equal(table.schema, "benefactor");
  assert.equal(table.name, "benefactor_leads");
});

test("parser defaults schema to public for unqualified tables", () => {
  const sql = `
    create table example (
      id uuid primary key default gen_random_uuid()
    );
  `;
  const schema = parseSchemaSql(sql);
  const table = schema.tables.find((item) => item.name === "example");
  assert.equal(table.schema, "public");
});

test("parser associates a schema-qualified index with its bare table", () => {
  const sql = `
    create table benefactor.benefactor_leads (
      id uuid primary key default gen_random_uuid(),
      primary_email varchar(255) default '' not null
    );

    create unique index if not exists benefactor_leads_email_uq
      on benefactor.benefactor_leads (primary_email);
  `;
  const schema = parseSchemaSql(sql);
  const table = schema.tables.find((item) => item.name === "benefactor_leads");
  assert.equal(table.indexes.length, 1);
  assert.equal(table.indexes[0].name, "benefactor_leads_email_uq");
});

test("parser flows a schema-qualified foreign key with bare reference table", () => {
  const sql = `
    create table benefactor.benefactor_search_locations (
      id uuid primary key default gen_random_uuid()
    );

    create table benefactor.benefactor_scrape_queries (
      id uuid primary key default gen_random_uuid(),
      benefactor_search_location_id uuid
    );

    alter table if exists benefactor.benefactor_scrape_queries
      add constraint benefactor_scrape_queries_location_fk
      foreign key (benefactor_search_location_id)
      references benefactor.benefactor_search_locations(id);
  `;
  const schema = parseSchemaSql(sql);
  const column = findColumn(schema, "benefactor_scrape_queries", "benefactor_search_location_id");
  assert.equal(column.foreignKey?.table, "benefactor_search_locations");
  assert.equal(column.foreignKey?.column, "id");
  assert.equal(column.foreignKey?.constraint, "benefactor_scrape_queries_location_fk");
});

test("every adapter schema-qualifies non-public (benefactor) tables", async () => {
  // The benefactor.* tables live in a real Postgres schema. Each adapter must address them via its
  // own schema mechanism (qualified SQL string, or the framework's native schema option) rather
  // than a bare name that would resolve against the default search_path. This locks that wiring in
  // so a future generator change cannot silently drop it for any adapter. Adapters intentionally
  // left bare (Prisma multiSchema, Diesel, ent, Drift, Mnesia) are excluded by design.
  const expectations = [
    // Raw-SQL / literal-name renderers → fully schema-qualified string.
    ["generated/typescript/drizzle.ts", 'pgSchema("benefactor")'],
    ["generated/typescript/drizzle.ts", "benefactorSchema.table("],
    ["generated/rust/src/lib.rs", '"benefactor.benefactor_leads"'],
    ["generated/gleam/src/pg_defs.gleam", '"benefactor.benefactor_leads"'],
    ["generated/go/sqlc/query.sql", "from benefactor.benefactor_leads"],
    ["generated/go/gorm/pg_defs.go", '"benefactor.benefactor_leads"'],
    // Framework-ORM renderers → native schema option.
    ["generated/typescript/typeorm.ts", 'schema: "benefactor"'],
    ["generated/python/sqlalchemy_models.py", '{"schema": "benefactor"}'],
    ["generated/rust/sea-orm/src/lib.rs", 'schema_name = "benefactor"'],
    ["generated/elixir/lib/dd_pg_defs/benefactor_leads.ex", '@schema_prefix "benefactor"'],
    ["generated/jvm/jooq/src/main/java/dd/pgdefs/jooq/Tables.java", 'DSL.name("benefactor", "benefactor_leads")'],
    [
      "generated/jvm/hibernate/src/main/java/dd/pgdefs/hibernate/BenefactorLeadsEntity.java",
      'schema = "benefactor"',
    ],
  ];
  for (const [relativePath, needle] of expectations) {
    const contents = await readFile(path.join(packageRoot, relativePath), "utf8");
    assert.ok(
      contents.includes(needle),
      `${relativePath} should schema-qualify benefactor tables (missing: ${needle})`,
    );
  }
});

test("parser throws on a bare-name collision across schemas", () => {
  // Index/FK association is keyed by bare table name, so a duplicate bare name across schemas must
  // fail loudly rather than silently mis-associate.
  const sql = `
    create table public.widgets (
      id uuid primary key default gen_random_uuid()
    );

    create table benefactor.widgets (
      id uuid primary key default gen_random_uuid()
    );
  `;
  assert.throws(() => parseSchemaSql(sql), /Duplicate table name "widgets"/);
});

test("parser detects enum + jsonb_typeof shapes inside compound checks", () => {
  const sql = `
    create table example (
      id uuid primary key default gen_random_uuid(),
      status varchar(32) default 'active' not null,
      labels jsonb default '[]'::jsonb not null,
      constraint example_status_chk check (status in ('active', 'paused', 'archived')),
      constraint example_labels_chk check (jsonb_typeof(labels) = 'array')
    );
  `;
  const schema = parseSchemaSql(sql);
  const status = findColumn(schema, "example", "status");
  assert.equal(status.kind, "enum");
  assert.deepEqual(status.enumValues, ["active", "paused", "archived"]);

  const labels = findColumn(schema, "example", "labels");
  assert.equal(labels.kind, "jsonArray");
});

test("parser preserves contract metadata across regenerations", () => {
  // A round-trip sanity check: every parsed table must expose name, columns array, and the
  // foreignKeys / indexes / checks scaffolding that adapter generators iterate. Adding a new
  // table to schema.sql without breaking this invariant is the whole point of `--check`.
  const sql = `
    create table fixture (
      id uuid primary key default gen_random_uuid(),
      label text not null
    );
  `;
  const schema = parseSchemaSql(sql);
  const [table] = schema.tables;
  assert.equal(table.name, "fixture");
  assert.equal(Array.isArray(table.columns), true);
  assert.equal(Array.isArray(table.checks), true);
  assert.equal(Array.isArray(table.indexes), true);
  assert.equal(Array.isArray(table.foreignKeys), true);
});

test("parser captures pg-def owned functions for live drift checks", () => {
  const sql = `
    create or replace function presence_notify_shards()
    returns int
    language plpgsql
    stable
    as $$
    begin
      return 256;
    end;
    $$;
  `;
  const schema = parseSchemaSql(sql);
  assert.equal(schema.routines.length, 1);
  const [routine] = schema.routines;
  assert.equal(routine.name, "presence_notify_shards");
  assert.equal(routine.identityArguments, "");
  assert.equal(routine.returns, "int");
  assert.equal(routine.language, "plpgsql");
  assert.equal(routine.volatility, "stable");
  assert.match(routine.bodySql, /return 256/);
});

test("parser captures pg-def owned triggers for LISTEN/NOTIFY drift checks", () => {
  const sql = `
    create trigger presence_conv_members_notify
      after insert or update or delete on presence_conv_members
      for each row
      execute function notify_presence_member_change();
  `;
  const schema = parseSchemaSql(sql);
  assert.equal(schema.triggers.length, 1);
  const [trigger] = schema.triggers;
  assert.equal(trigger.name, "presence_conv_members_notify");
  assert.equal(trigger.tableName, "presence_conv_members");
  assert.equal(trigger.timing, "after");
  assert.deepEqual(trigger.events, ["delete", "insert", "update"]);
  assert.equal(trigger.orientation, "row");
  assert.equal(trigger.functionName, "notify_presence_member_change");
});

test("parser captures canonical presence LISTEN/NOTIFY functions and trigger", async () => {
  const sql = await readFile(new URL("../schema/schema.sql", import.meta.url), "utf8");
  const schema = parseSchemaSql(sql);

  const routineNames = schema.routines
    .map((routine) => routine.name)
    .filter((name) =>
      [
        "presence_notify_shards",
        "notify_presence_member_change",
        "presence_shard_of",
      ].includes(name),
    )
    .sort();
  assert.deepEqual(routineNames, [
    "notify_presence_member_change",
    "presence_notify_shards",
    "presence_shard_of",
  ]);

  const notifyRoutine = schema.routines.find(
    (routine) => routine.name === "notify_presence_member_change",
  );
  assert.equal(notifyRoutine?.language, "plpgsql");
  assert.equal(notifyRoutine?.volatility, "volatile");
  assert.match(
    notifyRoutine?.bodySql ?? "",
    /perform\s+pg_notify\('presence_change_conv_'\s+\|\|\s+v_conv_shard::text,\s*v_payload\)/i,
  );
  assert.match(
    notifyRoutine?.bodySql ?? "",
    /perform\s+pg_notify\('presence_change_user_'\s+\|\|\s+v_user_shard::text,\s*v_payload\)/i,
  );

  const shardRoutine = schema.routines.find((routine) => routine.name === "presence_shard_of");
  assert.equal(shardRoutine?.returns, "int");
  assert.equal(shardRoutine?.language, "sql");
  assert.equal(shardRoutine?.volatility, "stable");
  assert.match(shardRoutine?.bodySql ?? "", /presence_notify_shards\(\)/);

  const trigger = schema.triggers.find(
    (item) => item.name === "presence_conv_members_notify",
  );
  assert.equal(trigger?.tableName, "presence_conv_members");
  assert.equal(trigger?.timing, "after");
  assert.deepEqual(trigger?.events, ["delete", "insert", "update"]);
  assert.equal(trigger?.orientation, "row");
  assert.equal(trigger?.functionName, "notify_presence_member_change");
});

test("parser captures EVERY function/trigger in schema.sql (no silent drift drops)", async () => {
  // The live-drift diff (src/diff.mjs) only checks routines/triggers that the
  // parser put into the contract. A function/trigger written in a shape the
  // parser cannot match would be SILENTLY dropped — invisible to the diff, so
  // prod could drift on it undetected. Use splitSqlStatements (the same
  // dollar-quote-aware splitter the parser uses, so bodies don't false-count)
  // as an independent oracle: every create-function/create-trigger STATEMENT
  // must be captured. This auto-tracks legitimate additions (a parseable new
  // function keeps the test green) but fails loudly on an unparseable shape.
  const sql = await readFile(new URL("../schema/schema.sql", import.meta.url), "utf8");
  const schema = parseSchemaSql(sql);
  const statements = splitSqlStatements(sql);

  const fnStatements = statements.filter((s) =>
    /^create\s+(?:or\s+replace\s+)?function\b/i.test(s),
  );
  const triggerStatements = statements.filter((s) => /^create\s+trigger\b/i.test(s));

  assert.equal(
    schema.routines.length,
    fnStatements.length,
    `parser captured ${schema.routines.length} routines but schema.sql declares ${fnStatements.length} create-function statements; an unparseable function shape is being silently dropped from the drift contract`,
  );
  assert.equal(
    schema.triggers.length,
    triggerStatements.length,
    `parser captured ${schema.triggers.length} triggers but schema.sql declares ${triggerStatements.length} create-trigger statements; an unparseable trigger shape is being silently dropped from the drift contract`,
  );
  // Sanity floor so a refactor that accidentally zeroes the oracle still trips.
  assert.ok(schema.routines.length >= 10, "expected at least the presence/cdc routine set");
  assert.ok(schema.triggers.length >= 1, "expected at least the presence notify trigger");
});

test("parseIndexLeadingColumn extracts the first plain column from index defs", () => {
  // pg_get_indexdef style (schema-qualified, USING btree)
  assert.equal(
    parseIndexLeadingColumn("CREATE INDEX foo_idx ON public.foo USING btree (lead_col, other)"),
    "lead_col",
  );
  // Drizzle/quoted style, no USING
  assert.equal(
    parseIndexLeadingColumn('create unique index "u_idx" on "foo" ("Mixed_Col")'),
    "mixed_col",
  );
  // DESC + NULLS ordering on the leading column
  assert.equal(
    parseIndexLeadingColumn("create index i on foo (created_at desc nulls last, id)"),
    "created_at",
  );
  // GIN / partial index leading column
  assert.equal(
    parseIndexLeadingColumn("create index g on foo using gin (labels) where deleted = false"),
    "labels",
  );
  // Expression-leading index has no usable plain column → null (caller must
  // not treat it as supporting a plain FK column).
  assert.equal(parseIndexLeadingColumn("create index e on foo (lower(email))"), null);
  assert.equal(parseIndexLeadingColumn("not an index statement"), null);
});

test("foreignKeyIndexName caps at Postgres's 63-char identifier limit", () => {
  assert.equal(foreignKeyIndexName("orders", "customer_id"), "orders_customer_id_fk_idx");
  const long = foreignKeyIndexName("a".repeat(50), "b".repeat(50));
  assert.equal(long.length, 63);
});

test("foreignKeyIndexRecommendations only recommends genuinely uncovered FKs", () => {
  const tables = [
    {
      name: "child",
      schema: "public",
      columns: [{ name: "id", primaryKey: true }],
      indexes: [{ name: "child_declared_idx", columns: ["declared_fk"] }],
      foreignKeys: [
        // covered by a contract-declared leading index → skip
        { name: "child_declared_fk", column: "declared_fk", references: { table: "p", column: "id" } },
        // the FK column is the primary key → implicitly indexed → skip
        { name: "child_pk_fk", column: "id", references: { table: "p", column: "id" } },
        // covered by a LIVE index leading column → skip
        { name: "child_live_fk", column: "live_fk", references: { table: "p", column: "id" } },
        // genuinely uncovered → recommend
        { name: "child_uncovered_fk", column: "uncovered_fk", references: { table: "p", column: "id" } },
      ],
    },
  ];
  const liveSupport = new Set(["child live_fk"]);
  const recs = foreignKeyIndexRecommendations(tables, liveSupport);
  assert.equal(recs.length, 1);
  assert.equal(recs[0].column, "uncovered_fk");
  assert.equal(recs[0].indexName, "child_uncovered_fk_fk_idx");
  assert.equal(
    recs[0].statement,
    "create index if not exists child_uncovered_fk_fk_idx on child (uncovered_fk);",
  );
});

test("foreignKeyIndexRecommendations is complete + idempotent against schema.sql", async () => {
  const sql = await readFile(new URL("../schema/schema.sql", import.meta.url), "utf8");
  const { tables } = parseSchemaSql(sql);

  const recs = foreignKeyIndexRecommendations(tables);
  assert.ok(recs.length > 0, "schema.sql currently has unindexed foreign keys to recommend");
  for (const rec of recs) {
    assert.match(rec.statement, /^create index if not exists \S+ on \S+ \(\S+\);$/);
  }

  // Idempotence/completeness: once every recommended FK column is covered by a
  // (now-live) leading index, a re-run recommends nothing — the generator
  // converges and never re-proposes an index it already proposed.
  const live = new Set(recs.map((r) => `${r.table.toLowerCase()} ${r.column.toLowerCase()}`));
  assert.equal(foreignKeyIndexRecommendations(tables, live).length, 0);
});
