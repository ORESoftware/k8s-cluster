# `remote/databases/pg/seeds`

Runtime data fixtures (`INSERT … ON CONFLICT … DO UPDATE`) that populate
application-config rows in the shared RDS Postgres database.

These are NOT schema. The single source of truth for table DDL is
[`remote/libs/pg-defs/schema/schema.sql`](../../../libs/pg-defs/schema/schema.sql),
which drives every generated adapter under `remote/libs/pg-defs/generated/`
(Drizzle / TypeORM / Prisma / SQLAlchemy / GORM / Bun / Dart / Diesel / SeaORM /
Gleam / Erlang / raw Rust + sqlx).

Apply the schema first, then apply any seeds your service needs:

```bash
# 1. schema (review the diff first; never apply blindly to a live DB)
psql "$RDS_DATABASE_URL" -f remote/libs/pg-defs/schema/schema.sql

# 2. seeds (idempotent inserts into app_config)
psql "$RDS_DATABASE_URL" -f remote/databases/pg/seeds/container-pool-app-config.sql
psql "$RDS_DATABASE_URL" -f remote/databases/pg/seeds/trading-platform-app-config.sql
```

## SQL file layout in this repo

There are exactly two homes for `.sql` files; the convention is enforced by
`remote/tests/general/pg-sql-centralized.test.ts`:

| Location | Purpose | Files |
| --- | --- | --- |
| `remote/libs/pg-defs/schema/schema.sql` | Canonical table DDL (every shared table, indexes, FKs, check constraints) | 1 file |
| `remote/databases/pg/seeds/*.sql` | Idempotent app-data fixtures (runtime config, never schema) | this folder |

Per-table DDL files such as `tables/app-config-table.sql`,
`tables/container-pool-configs-table.sql`, and `tables/lambda-functions-table.sql`
were retired because they duplicated blocks already in `schema.sql` and let
the two sources of truth drift apart.

## Adding a new seed

1. Make sure every table the seed writes into is already in
   `remote/libs/pg-defs/schema/schema.sql`. Generate a diff first via
   `node remote/libs/pg-defs/src/diff.mjs --env=<env>` and get the migration
   reviewed and applied before the seed runs.
2. Add the seed file here with an `INSERT … ON CONFLICT (...) DO UPDATE SET ...`
   so re-application is safe.
3. Reference the source file in the seed's header comment (so an operator
   reading the row in production can trace it back).
4. Update the centralization test if you genuinely need a new SQL location;
   otherwise this folder is the right home.
