# RDS vs pg-defs diff

`remote/libs/pg-defs/schema/schema.sql` is the desired database contract. The live RDS database is
the actual state. The diff script in this directory compares those two sources and emits a report
only.

It does not generate `.sql` migration files, and it refuses `.sql` output paths. Use the report to
decide what manual/declarative migration work is needed, then review and apply that work through the
normal human-owned database path.

```bash
node scripts/pg/diff/rds-vs-pg-defs.mjs --format text

node scripts/pg/diff/rds-vs-pg-defs.mjs \
  --schema public \
  --format json \
  --output tmp/pg-diff/rds-vs-pg-defs.json

node scripts/pg/diff/rds-vs-pg-defs.mjs --check
```

Database URL resolution order:

1. `AGENT_TASKS_RDS_DATABASE_URL`
2. `RDS_DATABASE_URL`
3. `AGENT_TASKS_DATABASE_URL`
4. `DATABASE_URL`

You can also pass `--database-url`. For tests and offline review, use `--from-live-json` with a
captured catalog snapshot.
