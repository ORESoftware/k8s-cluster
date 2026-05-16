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

For a parser-only sanity check that opens no database connection:

```sh
node src/diff.mjs --parse-only
```

## Runtime Adapters

- TypeScript: Drizzle table definitions plus Zod row/insert/update schemas, TypeORM entities, and
  Prisma schema output.
- Python/FastAPI: SQLAlchemy declarative models plus Pydantic row/insert schemas.
- Go: GORM models and Bun models.
- Dart: runtime models, validators, JSON helpers, and SQL constants.
- Rust: `sqlx`-friendly structs, Diesel schema/models, and SeaORM entities.
- Gleam: `pog`-friendly SQL constants, typed records, enum parsers, and validators.
- Erlang: `pgo`-style SQL constants and enum validators.

Each service should depend on this library through a local file dependency or copy the generated
adapter into its build context. Services should not own migrations; migrations remain owned by the
schema authority and release pipeline.
