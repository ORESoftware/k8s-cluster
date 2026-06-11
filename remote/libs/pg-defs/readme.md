# Remote Postgres Definitions

`remote/libs/pg-defs` is the shared schema contract for remote services that read or write directly
to Postgres from different runtimes.

The canonical source is [`schema/schema.sql`](./schema/schema.sql). Generated adapters live under
`generated/` and are adapters only.

```sh
node src/generate.mjs
```

Check generated files without rewriting them:

```sh
node src/generate.mjs --check
```

Run the parser/generator self-tests (no DB connection, no file writes):

```sh
node --test src/*.test.mjs
```

Print SQL DDL for review or declarative diff tooling:

```sh
node src/generate.mjs --print-sql
```

Generate a reviewable diff against a live database using read-only catalog queries:

```sh
node src/diff.mjs --env=prod
```

The diff script writes to `tmp/migrations/<env>/pg-defs-diff.sql` under this package. It does not
apply SQL. Never apply migrations automatically; a human must review the generated SQL and provide
explicit approval before any database write.

By default, diffs only report tables owned by `schema/schema.sql`. Use
`--include-extra-tables` when you intentionally want an audit of unrelated public tables in a shared
database.

Connection lookup defaults to the shared remote RDS env names first:
`AGENT_TASKS_RDS_DATABASE_URL`, `RDS_DATABASE_URL`, `DATABASE_URL`, then `PG_DATABASE_URL`. Pass
`--database-url-env=NAME` when auditing a service-specific database.

The live diff also checks pg-def owned routines and triggers, including the sharded
`presence_conv_members` LISTEN/NOTIFY + outbox contract (`presence_notify_shards`,
`notify_presence_member_change`, `presence_shard_of`, `presence_conv_members_notify`, and related
checkpoint/event tables). This is intentional: a table-only diff can miss the exact drift that
breaks presence cache fan-out.

The diff additionally:

- **Derives foreign-key supporting indexes.** Every foreign key should have an index on its
  referencing column, or cascading deletes scan the child table and child writes contend with the
  parent. These indexes are NOT declared in `schema.sql` — the diff generates idempotent
  `create index if not exists <table>_<col>_fk_idx on <table> (<col>);` for any FK not already
  covered by a contract-declared leading index, the primary key, or a live index whose leading
  column is the FK column. Once applied, the next diff sees the live coverage and stops proposing
  it (the generator converges). See `foreignKeyIndexRecommendations` in `src/sql-contract.mjs`.
- **Compares index uniqueness, not just names.** A same-named index that is `UNIQUE` in the
  contract but non-unique live (or vice versa) is a data-integrity gap that name-only diffing
  silently accepts; the diff now introspects `pg_index.indisunique` and surfaces the drift for
  manual rebuild (dropping a unique index can transiently admit duplicate rows).

For a parser-only sanity check that opens no database connection:

```sh
node src/diff.mjs --parse-only
```

## Runtime Adapters

Each language target ships pre-generated artifacts so consumers never have to install a code-gen
toolchain just to read or write the canonical tables. Constraints the SQL contract can express
that the adapter cannot (partial / GIN indexes, JSONB CHECKs, FK cycles) are captured as comments
or `customConstraint` shims and remain enforced by the database.

### TypeScript / Node

- `generated/typescript/drizzle.ts` — Drizzle table definitions plus Zod row/insert/update schemas.
- `generated/typescript/typeorm.ts` — TypeORM entity classes.
- `generated/prisma/schema.prisma` — Prisma model definitions.

### Python

- `generated/python/sqlalchemy_models.py` — SQLAlchemy declarative models plus Pydantic
  row/insert schemas with full constraint enforcement.

### Go

- `generated/go/gorm/pg_defs.go` — GORM models with a `Validate()` method that enforces every
  CHECK constraint we can statically derive (regex, byte length, enum, integer ranges).
- `generated/go/bun/pg_defs.go` — Bun models with the same `Validate()` contract.
- `generated/go/ent/schema/` — One ent Schema struct per canonical table, ready for
  `go generate ./ent` from a parent module.
- `generated/go/sqlc/` — Self-contained sqlc workspace (schema mirror + starter query catalogue +
  `sqlc.yaml`); run `sqlc generate` inside the directory to produce typed Go bindings.

### Dart

- `generated/dart/lib/pg_defs.dart` — Runtime models, validators, JSON helpers, and SQL constants.
  The smallest and most portable adapter — no third-party deps.
- `generated/dart-drift/lib/pg_defs_drift.dart` — Drift `Table` subclasses ready for
  `@DriftDatabase(tables: registeredDriftTables)`.
- `generated/dart-objectbox/lib/pg_defs_objectbox.dart` — ObjectBox entity classes for
  offline-first Flutter clients mirroring server rows.

### Rust

- `generated/rust/src/lib.rs` — `sqlx`-friendly structs, table/select-SQL constants, enum types,
  and `validate_<table>_row` / `validate_<table>_insert` helpers.
- `generated/rust/diesel/src/lib.rs` — Diesel `table!` blocks plus `Queryable` / `Insertable`
  structs.
- `generated/rust/sea-orm/src/lib.rs` — SeaORM `DeriveEntityModel` entries.

### Gleam

- `generated/gleam/src/pg_defs.gleam` — Dependency-light SQL constants, typed records, enum
  parsers, and validators. Backed by a smoke-test harness under `generated/gleam/test/`.

### Erlang

- `generated/erlang/src/pg_defs.erl` — `pgo`-style binary SQL constants and enum validators.
- `generated/erlang/src/pg_defs_mnesia.erl` — Mnesia record declarations and `*_table_def/0`
  helpers wired to `mnesia:create_table/2` defaults.

### Elixir

- `generated/elixir/lib/dd_pg_defs.ex` — Umbrella module with a `tables/0` accessor.
- `generated/elixir/lib/dd_pg_defs/<table>.ex` — One `Ecto.Schema` module per canonical table with
  a `changeset/2` that applies every constraint exposed by the SQL contract (length, regex,
  enum membership, byte limits, integer ranges).

### JVM (Java / Scala / Vert.x / Spring Boot)

- `generated/jvm/jooq/src/main/java/dd/pgdefs/jooq/Tables.java` — Run-time jOOQ table + column
  references. Compatible with plain JDBC, Vert.x, Spring Boot, Quarkus, Micronaut, or Kotlin.
- `generated/jvm/hibernate/src/main/java/dd/pgdefs/hibernate/` — JPA-annotated entity classes for
  Hibernate / Spring Data / Hibernate Reactive.

Each service should depend on this library through a local file dependency or copy the generated
adapter into its build context. Services should not own migrations; migrations remain owned by the
schema authority and release pipeline.

## Hardening + Future-Proofing

The parser is intentionally conservative — it walks a tiny subset of Postgres DDL but covers
enough shapes that adding a new CHECK constraint to `schema.sql` flows through to every adapter
without code changes:

- **Compound CHECK clauses** are split on top-level `AND`, so `max_warm between 1 and 128 and
  max_warm >= min_warm` contributes its `between` fact to validation even though the cross-column
  comparison is left to the database.
- **`between X and Y`** and **simple comparison operators** (`>=`, `<=`, `>`, `<`) on integer
  columns populate `validation.min` / `validation.max` for every adapter.
- **Regex enforcement** runs in every adapter that has a native regex engine (TypeScript / Zod,
  Python / Pydantic, Go, Dart, Ecto). Slug-shaped patterns also keep their hand-rolled byte
  validators in Rust to avoid pulling a regex crate.
- **Foreign keys** declared via `alter table … add constraint … foreign key` are extracted onto
  the column metadata so adapters that can model relationships (Drift, ent, Hibernate) emit the
  reference constraint instead of forgetting it.
- **Null guards** like `col is null or octet_length(col) <= N` capture their inner constraint
  without losing nullability semantics.

If you teach the parser a new shape, add a corresponding case to `src/generate.test.mjs` so the
contract stays locked in.
