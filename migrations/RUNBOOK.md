# dd-sound-recorder-rs — schema apply runbook (declarative)

This service does **not** own ordered migration files. The desired schema is declared once in
[`remote/libs/pg-defs/schema/schema.sql`](../../../libs/pg-defs/schema/schema.sql) (the source of
truth), and the SQL needed to reach it is **computed at runtime** by diffing that contract against
the live database. Nothing is applied automatically — a human reviews the generated SQL and runs it.

> The hand-written [`0001_use_case_and_pinned_at.sql`](./0001_use_case_and_pinned_at.sql) is kept
> only as a reviewed, copy-pasteable reference for the change described below. The **authoritative**
> path is the runtime diff in steps 2–4. If they ever disagree, `schema.sql` + the diff win.

## What this change introduces

Relative to the pre-2026-06 schema, `schema.sql` adds:

| Object | Table | Purpose |
| --- | --- | --- |
| `use_case varchar(32) not null default 'security'` | `sound_recorder_upload_sessions` | musician/meeting capture intent |
| `sound_recorder_upload_sessions_use_case_chk` | `sound_recorder_upload_sessions` | `use_case in (security, music, meeting, voice_note, ambient)` |
| `pinned_at timestamptz` (nullable) | `sound_recorder_segments` | permanent-save marker; `not null` ⇒ exempt from the retention sweep |
| `sound_recorder_segments_expiry_idx` predicate gains `and pinned_at is null` | `sound_recorder_segments` | keep the sweep's partial index aligned with its `where` clause |

## Prerequisites

- `psql` and `node` (≥ 18) on PATH.
- Read/write access to the target database. The service resolves its DB from
  `SOUND_RECORDER_RDS_DATABASE_URL` (see `dd-agent-secrets`); export it locally:
  ```sh
  export SOUND_RECORDER_RDS_DATABASE_URL="postgres://…"   # never commit this
  ```
- Work from the contract package:
  ```sh
  cd remote/libs/pg-defs
  ```

## Step 0 — parser sanity (no DB connection)

```sh
node src/diff.mjs --parse-only
```
Confirms `schema.sql` parses and lists the tables it owns. Opens no connection, writes nothing.

## Step 1 — confirm the contract is the intended shape

```sh
grep -n "use_case\|pinned_at\|sound_recorder_segments_expiry_idx" schema/schema.sql
```
You should see the column, the check, and the `pinned_at is null` partial index. If not, fix
`schema.sql` first (and regenerate adapters with `node src/generate.mjs`) — do not edit the DB ahead
of the contract.

## Step 2 — generate the diff against **stage** and review

```sh
node src/diff.mjs --env=stage \
  --database-url-env=SOUND_RECORDER_RDS_DATABASE_URL
```
Writes `tmp/migrations/stage/pg-defs-diff.sql` (and `desired-schema.sql`). **Read it.** For this
change, on a database that predates these objects, the emitted, single-transaction SQL is:

```sql
BEGIN;

-- Add missing column: sound_recorder_upload_sessions.use_case
alter table "sound_recorder_upload_sessions"
  add column if not exists use_case varchar(32) default 'security' not null;

-- Add missing check constraint: sound_recorder_upload_sessions_use_case_chk
alter table "sound_recorder_upload_sessions"
  add constraint "sound_recorder_upload_sessions_use_case_chk"
  check (use_case in ('security','music','meeting','voice_note','ambient')) not valid;
alter table "sound_recorder_upload_sessions"
  validate constraint "sound_recorder_upload_sessions_use_case_chk";

-- Add missing column: sound_recorder_segments.pinned_at
alter table "sound_recorder_segments"
  add column if not exists pinned_at timestamptz;

-- Add missing index: sound_recorder_segments_expiry_idx   (only if the name is absent)
create index if not exists sound_recorder_segments_expiry_idx
  on sound_recorder_segments (expires_at asc)
  where status in ('pending', 'uploaded') and pinned_at is null;

COMMIT;
-- Change items emitted: N
```

If the diff instead reports `-- No schema differences detected`, the database is already in sync —
skip to Step 6.

### ⚠️ Step 2a — the index caveat (read this)

`diff.mjs` detects indexes **by name only**; it does not compare index *definitions*. So if
`sound_recorder_segments_expiry_idx` **already exists with the old predicate** (no `pinned_at is
null`), the diff will **not** emit anything for it, and the partial index will stay stale. This is a
performance/correctness-of-optimization issue (the sweep's own `where` clause still excludes pinned
rows, so retention stays correct), but the index should match the contract. Check and, if stale,
recreate it manually:

```sh
psql "$SOUND_RECORDER_RDS_DATABASE_URL" -X -c \
  "select indexdef from pg_indexes where indexname = 'sound_recorder_segments_expiry_idx';"
```
If the printed `indexdef` is missing `pinned_at IS NULL`, recreate it without holding a long write
lock (run **outside** a transaction — `CONCURRENTLY` cannot run inside one):
```sql
drop index concurrently if exists sound_recorder_segments_expiry_idx;
create index concurrently if not exists sound_recorder_segments_expiry_idx
  on sound_recorder_segments (expires_at asc)
  where status in ('pending', 'uploaded') and pinned_at is null;
```

## Step 3 — apply to stage

```sh
psql "$SOUND_RECORDER_RDS_DATABASE_URL" -X -v ON_ERROR_STOP=1 \
  -f tmp/migrations/stage/pg-defs-diff.sql
```

Locking notes (Postgres ≥ 11):
- Adding `use_case` with a **constant** default (`'security'`) is a metadata-only change — no table
  rewrite, brief `ACCESS EXCLUSIVE` lock only.
- `validate constraint` takes `SHARE UPDATE EXCLUSIVE` and scans the table but does not block
  reads/writes; fine on this table's size.
- `pinned_at` is nullable with no default — instant.

## Step 4 — re-diff stage to prove convergence

```sh
node src/diff.mjs --env=stage --database-url-env=SOUND_RECORDER_RDS_DATABASE_URL
cat tmp/migrations/stage/pg-defs-diff.sql   # expect: "No schema differences detected"
```
A clean re-diff is the success signal of the declarative model. If anything remains, review and
repeat — do not hand-patch around it.

## Step 5 — production

Repeat Steps 2–4 with `--env=prod` and the prod `SOUND_RECORDER_RDS_DATABASE_URL`. Review the prod
diff independently (its starting state may differ from stage). Apply in a low-traffic window; the
retention sweep (`POST /internal/retention/sweep`) reads `pinned_at`, so applying the column before
the next sweep keeps pins honored.

## Step 6 — verification

```sh
psql "$SOUND_RECORDER_RDS_DATABASE_URL" -X <<'SQL'
\echo use_case column + check:
select column_name, data_type, column_default, is_nullable
  from information_schema.columns
 where table_name = 'sound_recorder_upload_sessions' and column_name = 'use_case';
select conname, pg_get_constraintdef(oid)
  from pg_constraint where conname = 'sound_recorder_upload_sessions_use_case_chk';

\echo pinned_at column:
select column_name, data_type, is_nullable
  from information_schema.columns
 where table_name = 'sound_recorder_segments' and column_name = 'pinned_at';

\echo expiry index predicate (must contain pinned_at IS NULL):
select indexdef from pg_indexes where indexname = 'sound_recorder_segments_expiry_idx';
SQL
```

Application-level smoke test (optional): a session create with `"useCase":"music"` succeeds, and
`POST /api/mobile/v1/permanent-saves` sets `pinned_at` on the referenced segments.

## Rollback

The change is additive and forward-only; rolling back is normally unnecessary. If you must revert
**before** any client writes `use_case` values other than `'security'` or sets `pinned_at`:

```sql
begin;
drop index concurrently if exists sound_recorder_segments_expiry_idx;  -- run outside txn
alter table sound_recorder_upload_sessions
  drop constraint if exists sound_recorder_upload_sessions_use_case_chk;
alter table sound_recorder_upload_sessions drop column if exists use_case;
alter table sound_recorder_segments drop column if exists pinned_at;
commit;
```
Dropping `pinned_at` discards which segments were pinned (permanent saves revert to normal retention
and may be swept). Re-create the pre-change expiry index afterward if you dropped it. Only do this if
the running `dd-sound-recorder-rs` build also predates these columns — the current binary references
both.
