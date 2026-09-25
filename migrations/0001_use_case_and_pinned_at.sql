-- dd-sound-recorder-rs migration 0001
-- Adds musician capture intent (upload_sessions.use_case) and permanent-save
-- pinning (segments.pinned_at), matching schema/schema.sql in remote/libs/pg-defs.
--
-- Reviewed, idempotent, and forward-only. Apply manually against RDS:
--
--   psql "$SOUND_RECORDER_RDS_DATABASE_URL" \
--     -v ON_ERROR_STOP=1 \
--     -f remote/deployments/dd-sound-recorder-rs/migrations/0001_use_case_and_pinned_at.sql
--
-- To confirm the live database matches schema/schema.sql afterward:
--   (cd remote/libs/pg-defs && node src/diff.mjs --env=rds)

begin;

-- 1) Capture intent on upload sessions (security default; music/meeting/etc.).
alter table sound_recorder_upload_sessions
  add column if not exists use_case varchar(32) not null default 'security';

alter table sound_recorder_upload_sessions
  drop constraint if exists sound_recorder_upload_sessions_use_case_chk;
alter table sound_recorder_upload_sessions
  add constraint sound_recorder_upload_sessions_use_case_chk
  check (use_case in ('security', 'music', 'meeting', 'voice_note', 'ambient'));

-- 2) Permanent-save pin marker on segments. NULL = subject to the rolling
--    retention sweep; non-NULL = exempt (pinned by /permanent-saves).
alter table sound_recorder_segments
  add column if not exists pinned_at timestamptz;

-- 3) Keep the retention-sweep index aligned with the new predicate so pinned
--    rows are excluded from the expiry scan.
drop index if exists sound_recorder_segments_expiry_idx;
create index if not exists sound_recorder_segments_expiry_idx
  on sound_recorder_segments (expires_at asc)
  where status in ('pending', 'uploaded') and pinned_at is null;

commit;
