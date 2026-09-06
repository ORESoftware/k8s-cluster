-- dd-sound-recorder-rs migration 0002
-- Adds per-device transfer-gate state so the mobile app can pause cloud
-- streaming (low battery / network policy) and have server-managed copies
-- (Google Drive / OneDrive) defer in lockstep, resuming when the device clears
-- the pause. Local capture of the rolling window is unaffected by these fields.
--
-- Reviewed, idempotent, and forward-only. Apply manually against RDS:
--
--   psql "$SOUND_RECORDER_RDS_DATABASE_URL" \
--     -v ON_ERROR_STOP=1 \
--     -f remote/deployments/dd-sound-recorder-rs/migrations/0002_device_transfer_state.sql
--
-- To confirm the live database matches schema/schema.sql afterward:
--   (cd remote/libs/pg-defs && node src/diff.mjs --env=rds)

begin;

alter table sound_recorder_devices
  add column if not exists transfer_paused boolean not null default false;

alter table sound_recorder_devices
  add column if not exists transfer_pause_reason varchar(40);

alter table sound_recorder_devices
  add column if not exists network_policy varchar(20) not null default 'any';

-- Last reported battery level (0..100) and charging flag; advisory only.
alter table sound_recorder_devices
  add column if not exists battery_level smallint;

alter table sound_recorder_devices
  add column if not exists charging boolean;

alter table sound_recorder_devices
  add column if not exists transfer_state_updated_at timestamptz;

alter table sound_recorder_devices
  drop constraint if exists sound_recorder_devices_network_policy_chk;
alter table sound_recorder_devices
  add constraint sound_recorder_devices_network_policy_chk
  check (network_policy in ('any', 'wifi_only', 'cellular_only'));

alter table sound_recorder_devices
  drop constraint if exists sound_recorder_devices_pause_reason_chk;
alter table sound_recorder_devices
  add constraint sound_recorder_devices_pause_reason_chk
  check (
    transfer_pause_reason is null
    or transfer_pause_reason in ('low_battery', 'network_constraint', 'offline', 'manual')
  );

alter table sound_recorder_devices
  drop constraint if exists sound_recorder_devices_battery_level_chk;
alter table sound_recorder_devices
  add constraint sound_recorder_devices_battery_level_chk
  check (battery_level is null or battery_level between 0 and 100);

-- Lets the cloud-copy drain cheaply skip segments produced by paused devices.
create index if not exists sound_recorder_devices_transfer_paused_idx
  on sound_recorder_devices (id)
  where transfer_paused = true;

commit;
