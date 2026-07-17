# migrations/ — FROZEN historical record

This directory is retired and **must not grow**. The sixteen timestamped files
(`20260518000001_init.sql` … `20260609000016_partner_providers.sql`) are the
imperative sqlx migrations that built the billing database up to 2026-06-09.
They are kept only as an audit trail of how the schema evolved; they are no
longer applied by anyone — not by the server, not by CI, not by operators.

## Where the schema lives now

[`../schema/schema.sql`](../schema/schema.sql) is the declarative source of
truth: the exact final state produced by applying every file here in
timestamp order. The live database converges onto that file via
[dpm](https://github.com/declarative-migrations/declarative-postgres-migrate.rs)
(declarative-postgres-migrate), wrapped by
[`../scripts/dpm.sh`](../scripts/dpm.sh):

```sh
export SHADOW_DATABASE_URL=postgres://...   # throwaway-DB server for dpm
export TARGET_DATABASE_URL=postgres://...   # or BILLING_DATABASE_URL / DATABASE_URL

scripts/dpm.sh diff        # print the migration SQL (never executes)
scripts/dpm.sh verify      # rehearse on a shadow replica, prove convergence
scripts/dpm.sh review      # diff + AI review
scripts/dpm.sh apply       # generate + execute (interactive confirm)
scripts/dpm.sh bootstrap   # full DDL for an empty database
```

Migrations are **never applied automatically**. A human reviews the generated
SQL before any database write; destructive statements are emitted
commented-out and refused at apply time unless dpm's two consent flags are
passed explicitly.

## Rules

* Do NOT add new files here. Schema changes are made by editing
  `schema/schema.sql` and generating a reviewed dpm migration.
* Do NOT run these files against any database. Re-applying them to a
  database that has diverged would corrupt state; `scripts/dpm.sh bootstrap`
  is the supported way to build a fresh database.
* The server no longer runs migrations at boot — `sqlx::migrate!` and the
  `BILLING_RUN_MIGRATIONS` env var were removed when the service moved to
  SeaORM + dpm (see `src/db.rs`).
