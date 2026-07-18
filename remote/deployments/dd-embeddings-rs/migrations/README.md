# Frozen — historical record only

This directory is **frozen**. The boot-time `sqlx::migrate!` path that ran
these files was removed when the service moved from sqlx to SeaORM; nothing
reads this directory at build or run time anymore.

The search-index schema is now **declarative**:

- Source of truth: [`../schema/schema.sql`](../schema/schema.sql) — the
  consolidated final state of `0001_init.sql` (every DDL statement in that
  file is present, verbatim in semantics, in `schema/schema.sql`).
- Workflow: [`../scripts/dpm.sh`](../scripts/dpm.sh) `{diff|verify|review|apply}`
  (dpm — declarative-postgres-migrate), mirroring `billing-server-rs` and
  `remote/libs/pg-defs/scripts/dpm.sh`. Migrations are generated, reviewed by
  a human, and applied by an operator — never by the server at boot.

This search database is the service's own (separate from the shared pg-defs
RDS contract): the pg-defs generators cannot represent `vector`/`tsvector`
columns and that contract intentionally excludes pgvector tables, so the
schema lives here, per the billing-server-rs own-database pattern.

Do not add new migration files here; edit `../schema/schema.sql` and use dpm.
