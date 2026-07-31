# `migrations/` — schema-change reference (not auto-applied)

This service does **not** own ordered, auto-run migrations. The authoritative portable Postgres
schema is declared once in `ORESoftware/k8s-libs-and-shared-defs` at
`pg-defs/schema/schema.sql`; Supabase project-specific definitions live separately under
`supabase-defs/projects/<project-ref>/schemas/`. SQL needed to reach the portable Postgres contract
is **computed by diffing** that contract against the live database.
The `dd-sound-recorder-rs` process never runs DDL on startup — a human reviews the generated diff and
applies it manually against RDS.

The `.sql` files here are kept only as reviewed, copy-pasteable references for specific changes. If a
file ever disagrees with `schema.sql` + the runtime diff, **`schema.sql` and the diff win.**

## Files

- **`RUNBOOK.md`** — start here. The declarative apply procedure: how the diff is generated,
  reviewed, and applied, and how to confirm the live database matches the contract afterward.
- **`0001_use_case_and_pinned_at.sql`** — adds `upload_sessions.use_case` (musician/meeting capture
  intent, with its `CHECK`) and `segments.pinned_at` (permanent-save marker exempt from the retention
  sweep). Idempotent, forward-only.
- **`0002_device_transfer_state.sql`** — adds per-device transfer-gate columns on
  `sound_recorder_devices` (`transfer_paused`, `transfer_pause_reason`, `network_policy`,
  `battery_level`, `charging`, `transfer_state_updated_at`, plus `CHECK`s and a partial index) so the
  app can pause cloud streaming (low battery / network policy) and have server-managed copies defer in
  lockstep. Idempotent, forward-only.
- **`0003_segment_mirror_index.sql`** — performance-only partial index for the S3→R2 storage-mirror
  drain (`/internal/storage-mirror/drain`). Mirror state lives in `segments.meta_data` (server-owned
  keys), so there are no column changes; the backend works without this index, just slower at scale.
  Idempotent, forward-only.
- **`0004_cloud_connection_projection.sql`** — expands desktop/provider checks
  and installs the durable connection projection outbox, trigger, indexes, and
  existing-row backfill. Idempotent, forward-only.

Each file's header comment documents the exact `psql` invocation and the post-apply verification
command.
