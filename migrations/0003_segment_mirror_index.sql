-- dd-sound-recorder-rs migration 0003
-- Performance-only partial index for the storage-mirror drain
-- (POST /internal/storage-mirror/drain), which scans for uploaded segments not
-- yet copied to the mirror bucket. Mirror state itself lives in
-- segments.meta_data (server-owned keys: mirrorState, mirrorBucket,
-- mirrorFingerprint, mirrorMirroredAt, mirrorAttempts, mirrorClaimId,
-- mirrorClaimedAt, mirrorLastError, mirrorNextAttemptAt) — no column changes
-- are needed, and the backend works correctly (just slower) without this index.
--
-- Reviewed, idempotent, and forward-only. Apply manually against RDS:
--
--   psql "$SOUND_RECORDER_RDS_DATABASE_URL" \
--     -v ON_ERROR_STOP=1 \
--     -f remote/deployments/dd-sound-recorder-rs/migrations/0003_segment_mirror_index.sql
--
-- To confirm the live database matches schema/schema.sql afterward:
--   (cd remote/libs/pg-defs && node src/diff.mjs --env=rds)

begin;

-- Rows the drain claims: uploaded, physically stored, and not yet mirrored.
-- The predicate must stay aligned with the claim query in main.rs
-- (mirror_drain); `is distinct from 'mirrored'` also covers rows with no
-- mirror bookkeeping at all.
create index if not exists sound_recorder_segments_mirror_pending_idx
  on sound_recorder_segments (uploaded_at asc)
  where status = 'uploaded'
    and storage_bucket <> ''
    and storage_key <> ''
    and (meta_data->>'mirrorState') is distinct from 'mirrored';

commit;
