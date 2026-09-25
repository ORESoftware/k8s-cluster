-- dd-sound-recorder-rs migration 0004
-- Expands desktop/cloud-provider checks and adds the durable Postgres-to-
-- Supabase cloud connection projection outbox.
--
-- Reviewed, idempotent, and forward-only. Apply manually against RDS:
--
--   psql "$SOUND_RECORDER_RDS_DATABASE_URL" \
--     -v ON_ERROR_STOP=1 \
--     -f migrations/0004_cloud_connection_projection.sql

begin;

alter table sound_recorder_devices
  drop constraint if exists sound_recorder_devices_platform_chk;
alter table sound_recorder_devices
  add constraint sound_recorder_devices_platform_chk
  check (platform in ('ios', 'android', 'macos', 'windows', 'linux')) not valid;
alter table sound_recorder_devices
  validate constraint sound_recorder_devices_platform_chk;

alter table sound_recorder_oauth_states
  drop constraint if exists sound_recorder_oauth_states_provider_chk;
alter table sound_recorder_oauth_states
  add constraint sound_recorder_oauth_states_provider_chk
  check (
    provider in (
      'google_drive',
      'microsoft_onedrive',
      'apple_icloud',
      'dropbox',
      'amazon_s3',
      'cloudflare_r2'
    )
  ) not valid;
alter table sound_recorder_oauth_states
  validate constraint sound_recorder_oauth_states_provider_chk;

alter table sound_recorder_cloud_connections
  drop constraint if exists sound_recorder_cloud_connections_provider_chk;
alter table sound_recorder_cloud_connections
  add constraint sound_recorder_cloud_connections_provider_chk
  check (
    provider in (
      'google_drive',
      'microsoft_onedrive',
      'apple_icloud',
      'dropbox',
      'amazon_s3',
      'cloudflare_r2'
    )
  ) not valid;
alter table sound_recorder_cloud_connections
  validate constraint sound_recorder_cloud_connections_provider_chk;

alter table sound_recorder_cloud_copy_jobs
  drop constraint if exists sound_recorder_cloud_copy_jobs_provider_chk;
alter table sound_recorder_cloud_copy_jobs
  add constraint sound_recorder_cloud_copy_jobs_provider_chk
  check (
    provider in (
      'google_drive',
      'microsoft_onedrive',
      'apple_icloud',
      'dropbox',
      'amazon_s3',
      'cloudflare_r2'
    )
  ) not valid;
alter table sound_recorder_cloud_copy_jobs
  validate constraint sound_recorder_cloud_copy_jobs_provider_chk;

create table if not exists sound_recorder_cloud_connection_projection_outbox (
  seq bigserial primary key,
  connection_id uuid not null,
  attempts integer default 0 not null,
  available_at timestamptz default now() not null,
  locked_until timestamptz,
  processed_at timestamptz,
  last_error varchar(500),
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint sound_recorder_cloud_connection_projection_outbox_attempts_chk
    check (attempts between 0 and 50),
  constraint sound_recorder_cloud_projection_outbox_last_error_chk
    check (last_error is null or octet_length(last_error) between 1 and 500),
  constraint sound_recorder_cloud_connection_projection_outbox_connection_fk
    foreign key (connection_id) references sound_recorder_cloud_connections(id)
);

create unique index if not exists sound_recorder_cloud_connection_projection_outbox_pending_uq
  on sound_recorder_cloud_connection_projection_outbox (connection_id)
  where processed_at is null;

create index if not exists sound_recorder_cloud_connection_projection_outbox_ready_idx
  on sound_recorder_cloud_connection_projection_outbox (available_at asc, seq asc)
  where processed_at is null;

create or replace function enqueue_sound_recorder_cloud_connection_projection()
returns trigger
language plpgsql
as $$
begin
  insert into sound_recorder_cloud_connection_projection_outbox
    (connection_id, attempts, available_at, locked_until, processed_at, last_error, updated_at)
  values
    (new.id, 0, now(), null, null, null, now())
  on conflict (connection_id) where processed_at is null
  do update set
    attempts = 0,
    available_at = now(),
    locked_until = null,
    last_error = null,
    updated_at = now();
  return new;
end;
$$;

drop trigger if exists sound_recorder_cloud_connections_project
  on sound_recorder_cloud_connections;

create trigger sound_recorder_cloud_connections_project
  after insert or update on sound_recorder_cloud_connections
  for each row
  execute function enqueue_sound_recorder_cloud_connection_projection();

insert into sound_recorder_cloud_connection_projection_outbox (connection_id)
select id
from sound_recorder_cloud_connections
on conflict (connection_id) where processed_at is null
do nothing;

commit;
