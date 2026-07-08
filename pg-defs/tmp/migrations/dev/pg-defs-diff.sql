-- Remote pg-defs SQL diff
-- Environment: dev
-- Desired schema source: remote/libs/pg-defs/schema/schema.sql
-- SOURCE OF TRUTH: remote/libs/pg-defs/schema/schema.sql
-- Generated ORM/client files are adapters only and must not drive migrations.
-- SAFETY: review this file manually. Do not apply automatically.
-- Generated: 2026-06-22T06:37:02.372Z

BEGIN;

-- Create missing table: vapi_phone_call_events
create table if not exists vapi_phone_call_events (
  id uuid primary key default gen_random_uuid(),
  call_id varchar(160) not null,
  event_type varchar(80) not null,
  payload_hash varchar(64) not null,
  caller_hash varchar(64),
  called_number_hash varchar(64),
  ended_reason varchar(160),
  duration_seconds integer,
  summary text,
  payload jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  constraint vapi_phone_call_events_call_id_size_chk
    check (octet_length(call_id) between 1 and 160),
  constraint vapi_phone_call_events_event_type_format_chk
    check (event_type ~ '^[A-Za-z0-9._:/-]{1,80}$'),
  constraint vapi_phone_call_events_payload_hash_chk
    check (payload_hash ~ '^[a-f0-9]{64}$'),
  constraint vapi_phone_call_events_caller_hash_chk
    check (caller_hash is null or caller_hash ~ '^[a-f0-9]{64}$'),
  constraint vapi_phone_call_events_called_number_hash_chk
    check (called_number_hash is null or called_number_hash ~ '^[a-f0-9]{64}$'),
  constraint vapi_phone_call_events_duration_chk
    check (duration_seconds is null or duration_seconds between 0 and 86400),
  constraint vapi_phone_call_events_summary_size_chk
    check (summary is null or octet_length(summary) <= 4000),
  constraint vapi_phone_call_events_payload_object_chk
    check (jsonb_typeof(payload) = 'object')
);

create unique index if not exists vapi_phone_call_events_payload_hash_uq
  on vapi_phone_call_events (payload_hash);

create index if not exists vapi_phone_call_events_call_id_created_at_idx
  on vapi_phone_call_events (call_id, created_at desc);

create index if not exists vapi_phone_call_events_caller_hash_created_at_idx
  on vapi_phone_call_events (caller_hash, created_at desc)
  where caller_hash is not null;

create index if not exists vapi_phone_call_events_event_type_created_at_idx
  on vapi_phone_call_events (event_type, created_at desc);

-- Create missing table: music_songs
create table if not exists music_songs (
  id uuid primary key default gen_random_uuid(),
  title varchar(200) not null,
  slug varchar(220) not null,
  status varchar(32) default 'generated' not null,
  seed bigint not null,
  generation_date varchar(10) default to_char(current_date, 'YYYY-MM-DD') not null,
  storage_provider varchar(32),
  storage_bucket varchar(200),
  storage_key text,
  audio_url text,
  content_type varchar(120),
  duration_millis integer default 180000 not null,
  sample_rate integer default 44100 not null,
  bpm_millis integer default 128000 not null,
  genre varchar(80) default 'electronica' not null,
  peak_micros integer default 0 not null,
  rms_micros integer default 0 not null,
  spectral_centroid_millihz bigint default 0 not null,
  listenability_score_micros integer default 0 not null,
  vote_score integer default 0 not null,
  up_votes integer default 0 not null,
  down_votes integer default 0 not null,
  play_count integer default 0 not null,
  summary jsonb default '{}'::jsonb not null,
  meta_data jsonb default '{}'::jsonb not null,
  published_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint music_songs_title_size_chk
    check (octet_length(title) between 1 and 200),
  constraint music_songs_slug_format_chk
    check (slug ~ '^[a-z0-9][a-z0-9-]{0,218}[a-z0-9]$'),
  constraint music_songs_status_chk
    check (status in ('generated', 'published', 'discarded', 'failed', 'archived')),
  constraint music_songs_generation_date_chk
    check (generation_date ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint music_songs_storage_provider_chk
    check (storage_provider is null or storage_provider in ('s3', 'r2', 'gcs', 'drive', 'local')),
  constraint music_songs_storage_bucket_size_chk
    check (storage_bucket is null or octet_length(storage_bucket) <= 200),
  constraint music_songs_storage_key_size_chk
    check (storage_key is null or octet_length(storage_key) <= 2048),
  constraint music_songs_audio_url_size_chk
    check (audio_url is null or octet_length(audio_url) <= 4096),
  constraint music_songs_content_type_size_chk
    check (content_type is null or octet_length(content_type) <= 120),
  constraint music_songs_duration_chk
    check (duration_millis between 1 and 1800000),
  constraint music_songs_sample_rate_chk
    check (sample_rate between 8000 and 192000),
  constraint music_songs_bpm_chk
    check (bpm_millis between 1 and 300000),
  constraint music_songs_genre_size_chk
    check (octet_length(genre) between 1 and 80),
  constraint music_songs_metric_nonnegative_chk
    check (
      peak_micros >= 0
      and rms_micros >= 0
      and spectral_centroid_millihz >= 0
      and listenability_score_micros between 0 and 1000000
      and up_votes >= 0
      and down_votes >= 0
      and play_count >= 0
    ),
  constraint music_songs_summary_object_chk
    check (jsonb_typeof(summary) = 'object'),
  constraint music_songs_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object'),
  constraint music_songs_published_audio_chk
    check (status <> 'published' or audio_url is not null)
);

create unique index if not exists music_songs_slug_uq
  on music_songs (slug);

create index if not exists music_songs_published_at_idx
  on music_songs (published_at desc)
  where status = 'published';

create index if not exists music_songs_generation_date_status_idx
  on music_songs (generation_date desc, status);

create index if not exists music_songs_vote_score_idx
  on music_songs (vote_score desc, published_at desc)
  where status = 'published';

-- Create missing table: music_song_votes
create table if not exists music_song_votes (
  id uuid primary key default gen_random_uuid(),
  song_id uuid not null,
  visitor_hash varchar(64) not null,
  user_agent_hash varchar(64),
  vote_value integer not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint music_song_votes_visitor_hash_chk
    check (visitor_hash ~ '^[a-f0-9]{64}$'),
  constraint music_song_votes_user_agent_hash_chk
    check (user_agent_hash is null or user_agent_hash ~ '^[a-f0-9]{64}$'),
  constraint music_song_votes_value_chk
    check (vote_value >= -1 and vote_value <= 1 and vote_value <> 0)
);

create unique index if not exists music_song_votes_song_visitor_uq
  on music_song_votes (song_id, visitor_hash);

create index if not exists music_song_votes_song_created_at_idx
  on music_song_votes (song_id, created_at desc);

-- MANUAL REVIEW: check constraint differs for sound_recorder_accounts_external_subject_size_chk.
-- Desired: check (external_subject is null or octet_length(external_subject) between 1 and 240)
-- Actual:  CHECK (((external_subject IS NULL) OR ((octet_length((external_subject)::text) >= 1) AND (octet_length((external_subject)::text) <= 240))))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_accounts_display_name_size_chk.
-- Desired: check (display_name is null or octet_length(display_name) between 1 and 160)
-- Actual:  CHECK (((display_name IS NULL) OR ((octet_length((display_name)::text) >= 1) AND (octet_length((display_name)::text) <= 160))))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_accounts_legal_region_format_chk.
-- Desired: check (legal_region is null or legal_region ~ '^[A-Za-z0-9._:/-]{1,64}$')
-- Actual:  CHECK (((legal_region IS NULL) OR ((legal_region)::text ~ '^[A-Za-z0-9._:/-]{1,64}$'::text)))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: type differs for sound_recorder_devices.battery_level.
-- Desired: smallint
-- Actual:  int2
-- No ALTER TYPE generated automatically because this can rewrite or truncate data.

-- MANUAL REVIEW: check constraint differs for sound_recorder_devices_device_label_size_chk.
-- Desired: check (device_label is null or octet_length(device_label) between 1 and 160)
-- Actual:  CHECK (((device_label IS NULL) OR ((octet_length((device_label)::text) >= 1) AND (octet_length((device_label)::text) <= 160))))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_devices_app_version_size_chk.
-- Desired: check (app_version is null or octet_length(app_version) between 1 and 80)
-- Actual:  CHECK (((app_version IS NULL) OR ((octet_length((app_version)::text) >= 1) AND (octet_length((app_version)::text) <= 80))))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_devices_os_version_size_chk.
-- Desired: check (os_version is null or octet_length(os_version) between 1 and 80)
-- Actual:  CHECK (((os_version IS NULL) OR ((octet_length((os_version)::text) >= 1) AND (octet_length((os_version)::text) <= 80))))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_upload_sessions_storage_provider_chk.
-- Desired: check (storage_provider in ('s3'))
-- Actual:  CHECK (((storage_provider)::text = 's3'::text))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_upload_sessions_codec_size_chk.
-- Desired: check (codec is null or octet_length(codec) between 1 and 80)
-- Actual:  CHECK (((codec IS NULL) OR ((octet_length((codec)::text) >= 1) AND (octet_length((codec)::text) <= 80))))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_upload_sessions_client_timezone_size_chk.
-- Desired: check (client_timezone is null or octet_length(client_timezone) between 1 and 80)
-- Actual:  CHECK (((client_timezone IS NULL) OR ((octet_length((client_timezone)::text) >= 1) AND (octet_length((client_timezone)::text) <= 80))))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_upload_sessions_legal_region_format_chk.
-- Desired: check (legal_region is null or legal_region ~ '^[A-Za-z0-9._:/-]{1,64}$')
-- Actual:  CHECK (((legal_region IS NULL) OR ((legal_region)::text ~ '^[A-Za-z0-9._:/-]{1,64}$'::text)))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_segments_storage_provider_chk.
-- Desired: check (storage_provider in ('s3'))
-- Actual:  CHECK (((storage_provider)::text = 's3'::text))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_segments_codec_size_chk.
-- Desired: check (codec is null or octet_length(codec) between 1 and 80)
-- Actual:  CHECK (((codec IS NULL) OR ((octet_length((codec)::text) >= 1) AND (octet_length((codec)::text) <= 80))))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_segments_sha256_chk.
-- Desired: check (sha256_hex is null or sha256_hex ~ '^[a-f0-9]{64}$')
-- Actual:  CHECK (((sha256_hex IS NULL) OR ((sha256_hex)::text ~ '^[a-f0-9]{64}$'::text)))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_segments_etag_size_chk.
-- Desired: check (etag is null or octet_length(etag) between 1 and 160)
-- Actual:  CHECK (((etag IS NULL) OR ((octet_length((etag)::text) >= 1) AND (octet_length((etag)::text) <= 160))))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_oauth_states_folder_path_size_chk.
-- Desired: check (folder_path is null or octet_length(folder_path) between 1 and 512)
-- Actual:  CHECK (((folder_path IS NULL) OR ((octet_length((folder_path)::text) >= 1) AND (octet_length((folder_path)::text) <= 512))))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_cloud_connections_display_name_size_chk.
-- Desired: check (display_name is null or octet_length(display_name) between 1 and 160)
-- Actual:  CHECK (((display_name IS NULL) OR ((octet_length((display_name)::text) >= 1) AND (octet_length((display_name)::text) <= 160))))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_cloud_connections_provider_account_id_size_chk.
-- Desired: check (provider_account_id is null or octet_length(provider_account_id) between 1 and 240)
-- Actual:  CHECK (((provider_account_id IS NULL) OR ((octet_length((provider_account_id)::text) >= 1) AND (octet_length((provider_account_id)::text) <= 240))))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_cloud_connections_subject_hash_chk.
-- Desired: check (provider_subject_hash is null or provider_subject_hash ~ '^[a-f0-9]{64}$')
-- Actual:  CHECK (((provider_subject_hash IS NULL) OR ((provider_subject_hash)::text ~ '^[a-f0-9]{64}$'::text)))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_cloud_connections_root_folder_id_size_chk.
-- Desired: check (root_folder_id is null or octet_length(root_folder_id) between 1 and 512)
-- Actual:  CHECK (((root_folder_id IS NULL) OR ((octet_length((root_folder_id)::text) >= 1) AND (octet_length((root_folder_id)::text) <= 512))))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_cloud_copy_jobs_provider_file_id_size_chk.
-- Desired: check (provider_file_id is null or octet_length(provider_file_id) between 1 and 512)
-- Actual:  CHECK (((provider_file_id IS NULL) OR ((octet_length((provider_file_id)::text) >= 1) AND (octet_length((provider_file_id)::text) <= 512))))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: check constraint differs for sound_recorder_cloud_copy_jobs_last_error_size_chk.
-- Desired: check (last_error is null or octet_length(last_error) between 1 and 500)
-- Actual:  CHECK (((last_error IS NULL) OR ((octet_length((last_error)::text) >= 1) AND (octet_length((last_error)::text) <= 500))))
-- No DROP/replace generated automatically.

-- MANUAL REVIEW: default differs for lambda_functions.entry_command.
-- Desired default: 'env -i PATH="$PATH" NODE_ENV=production NODE_NO_WARNINGS=1 node --permission --allow-net child-runtimes/js-function-runner.mjs'
-- Actual default:  'env -i PATH="$PATH" NODE_ENV=production node --permission --allow-net child-runtimes/js-function-runner.mjs'::text
-- No default ALTER generated automatically; confirm intent before changing write behavior.

-- MANUAL REVIEW: check constraint differs for lambda_functions_runtime_chk.
-- Desired: check (runtime in ('nodejs', 'javascript', 'typescript', 'python3', 'python', 'ruby', 'bash', 'shell', 'golang', 'go', 'dart', 'erlang', 'erl', 'elixir', 'ex', 'java', 'jvm'))
-- Actual:  CHECK (((runtime)::text = ANY ((ARRAY['nodejs'::character varying, 'javascript'::character varying, 'typescript'::character varying, 'python3'::character varying, 'python'::character varying, 'ruby'::character varying, 'bash'::character varying, 'shell'::character varying])::text[])))
-- No DROP/replace generated automatically.

-- Create missing table: workflow_definitions
create table if not exists workflow_definitions (
  id uuid primary key default gen_random_uuid(),
  slug varchar(120) not null,
  display_name varchar(200) not null,
  description text default '' not null,
  steps jsonb not null,
  default_retry jsonb default '{}'::jsonb not null,
  status varchar(32) default 'draft' not null,
  labels jsonb default '[]'::jsonb not null,
  meta_data jsonb default '{}'::jsonb not null,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint workflow_definitions_slug_format_chk
    check (slug ~ '^[a-z0-9][a-z0-9-]{1,118}[a-z0-9]$'),
  constraint workflow_definitions_steps_array_chk
    check (jsonb_typeof(steps) = 'array'),
  constraint workflow_definitions_steps_size_chk
    check (octet_length(steps::text) <= 262144),
  constraint workflow_definitions_default_retry_object_chk
    check (jsonb_typeof(default_retry) = 'object'),
  constraint workflow_definitions_labels_array_chk
    check (jsonb_typeof(labels) = 'array'),
  constraint workflow_definitions_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object'),
  constraint workflow_definitions_status_chk
    check (status in ('draft', 'active', 'paused', 'archived'))
);

create unique index if not exists workflow_definitions_slug_active_uq
  on workflow_definitions (slug)
  where is_soft_deleted = false;

create index if not exists workflow_definitions_status_idx
  on workflow_definitions (status)
  where is_soft_deleted = false;

create index if not exists workflow_definitions_updated_at_idx
  on workflow_definitions (updated_at desc)
  where is_soft_deleted = false;

-- Create missing table: workflow_runs
create table if not exists workflow_runs (
  id uuid primary key default gen_random_uuid(),
  definition_id uuid not null,
  definition_slug varchar(120) not null,
  status varchar(32) default 'pending' not null,
  current_step_index integer default 0 not null,
  attempt integer default 0 not null,
  input jsonb default 'null'::jsonb not null,
  context jsonb default '{}'::jsonb not null,
  output jsonb,
  last_error text,
  wake_at timestamptz,
  wait_deadline timestamptz,
  lease_until timestamptz,
  signals jsonb default '[]'::jsonb not null,
  idempotency_key varchar(200),
  started_at timestamptz,
  finished_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  constraint workflow_runs_status_chk
    check (status in ('pending', 'running', 'sleeping', 'waiting', 'completed', 'failed', 'canceled')),
  constraint workflow_runs_current_step_index_chk
    check (current_step_index >= 0),
  constraint workflow_runs_attempt_chk
    check (attempt >= 0),
  constraint workflow_runs_context_object_chk
    check (jsonb_typeof(context) = 'object'),
  constraint workflow_runs_signals_array_chk
    check (jsonb_typeof(signals) = 'array'),
  constraint workflow_runs_last_error_size_chk
    check (last_error is null or octet_length(last_error) <= 8192)
);

create index if not exists workflow_runs_definition_id_idx
  on workflow_runs (definition_id);

create index if not exists workflow_runs_status_idx
  on workflow_runs (status);

create index if not exists workflow_runs_due_idx
  on workflow_runs (wake_at)
  where status in ('pending', 'running', 'sleeping', 'waiting');

create unique index if not exists workflow_runs_idempotency_key_uq
  on workflow_runs (definition_id, idempotency_key)
  where idempotency_key is not null;

-- Create missing table: workflow_step_runs
create table if not exists workflow_step_runs (
  id uuid primary key default gen_random_uuid(),
  run_id uuid not null,
  step_index integer not null,
  step_name varchar(200) not null,
  step_type varchar(32) default 'activity' not null,
  function_ref varchar(200) default '' not null,
  attempt integer not null,
  status varchar(32) not null,
  input jsonb,
  output jsonb,
  error text,
  duration_ms integer,
  started_at timestamptz default now() not null,
  finished_at timestamptz,
  constraint workflow_step_runs_status_chk
    check (status in ('running', 'succeeded', 'failed')),
  constraint workflow_step_runs_step_index_chk
    check (step_index >= 0),
  constraint workflow_step_runs_attempt_chk
    check (attempt >= 0),
  constraint workflow_step_runs_error_size_chk
    check (error is null or octet_length(error) <= 8192)
);

create index if not exists workflow_step_runs_run_id_idx
  on workflow_step_runs (run_id, step_index, attempt);

-- Add missing index: des_soccer_learning_policy_versions_single_active_uq
create unique index if not exists des_soccer_learning_policy_versions_single_active_uq
  on des_soccer_learning_policy_versions (experiment_id)
  where status = 'active';

-- MANUAL REVIEW: type differs for des_soccer_tournaments.wall_time_seconds.
-- Desired: double
-- Actual:  float8
-- No ALTER TYPE generated automatically because this can rewrite or truncate data.

-- Add missing check constraint: des_soccer_tournaments_status_chk
alter table "des_soccer_tournaments" add constraint "des_soccer_tournaments_status_chk" check (status in ('running', 'completed', 'failed', 'aborted')) not valid;
alter table "des_soccer_tournaments" validate constraint "des_soccer_tournaments_status_chk";

-- MANUAL REVIEW: database has extra column des_soccer_tournament_matches.tournament_id.
-- No DROP COLUMN generated automatically.

-- MANUAL REVIEW: database has extra column des_soccer_tournament_team_brains.tournament_id.
-- No DROP COLUMN generated automatically.

-- Create missing table: des_soccer_learning_set_play_runs
create table if not exists des_soccer_learning_set_play_runs (
  run_id uuid primary key references des_soccer_learning_runs(id) on delete cascade,
  policy_version_id uuid not null references des_soccer_learning_policy_versions(id) on delete cascade,
  primary_restart varchar(40) not null,
  team varchar(8) not null,
  spot_x_micros bigint not null,
  spot_y_micros bigint not null,
  duration_seconds_micros bigint not null,
  episode_count integer not null,
  goals integer not null,
  goal_rate_micros bigint not null,
  first_window_goal_rate_micros bigint not null,
  last_window_goal_rate_micros bigint not null,
  goal_rate_delta_micros bigint not null,
  created_at timestamptz default now() not null,
  constraint des_soccer_learning_set_play_runs_restart_chk
    check (primary_restart in ('direct-free-kick', 'indirect-free-kick')),
  constraint des_soccer_learning_set_play_runs_team_chk
    check (team in ('home', 'away')),
  constraint des_soccer_learning_set_play_runs_duration_chk
    check (duration_seconds_micros >= 0),
  constraint des_soccer_learning_set_play_runs_episode_chk
    check (episode_count >= 0),
  constraint des_soccer_learning_set_play_runs_goals_chk
    check (goals >= 0),
  constraint des_soccer_learning_set_play_runs_goal_rate_chk
    check (goal_rate_micros between 0 and 1000000)
);

-- Create missing table: des_soccer_learning_set_play_restart_mix
create table if not exists des_soccer_learning_set_play_restart_mix (
  run_id uuid not null references des_soccer_learning_set_play_runs(run_id) on delete cascade,
  ordinal integer not null,
  restart varchar(40) not null,
  primary key (run_id, ordinal),
  constraint des_soccer_learning_set_play_restart_mix_ordinal_chk
    check (ordinal >= 0),
  constraint des_soccer_learning_set_play_restart_mix_restart_chk
    check (restart in ('direct-free-kick', 'indirect-free-kick'))
);

-- Create missing table: des_soccer_learning_set_play_episode_metrics
create table if not exists des_soccer_learning_set_play_episode_metrics (
  run_id uuid not null references des_soccer_learning_set_play_runs(run_id) on delete cascade,
  episode_index integer not null,
  seed bigint not null,
  restart varchar(40) not null,
  routine varchar(80),
  scored boolean not null,
  score_delta_for_team integer not null,
  ticks bigint not null,
  simulated_seconds_micros bigint not null,
  policy_updates bigint not null,
  home_policy_entries integer not null,
  home_policy_target_entries integer not null,
  away_policy_entries integer not null,
  away_policy_target_entries integer not null,
  neural_training_steps integer not null,
  neural_samples bigint not null,
  neural_replay_samples integer not null,
  neural_last_loss_micros bigint,
  cumulative_goals integer not null,
  goal_rate_so_far_micros bigint not null,
  primary key (run_id, episode_index),
  constraint des_soccer_learning_set_play_episode_idx_chk
    check (episode_index >= 0),
  constraint des_soccer_learning_set_play_episode_seed_chk
    check (seed >= 0),
  constraint des_soccer_learning_set_play_episode_restart_chk
    check (restart in ('direct-free-kick', 'indirect-free-kick')),
  constraint des_soccer_learning_set_play_episode_ticks_chk
    check (ticks >= 0),
  constraint des_soccer_learning_set_play_episode_seconds_chk
    check (simulated_seconds_micros >= 0),
  constraint des_soccer_learning_set_play_episode_policy_updates_chk
    check (policy_updates >= 0),
  constraint des_soccer_learning_set_play_episode_entries_chk
    check (
      home_policy_entries >= 0
      and home_policy_target_entries >= 0
      and away_policy_entries >= 0
      and away_policy_target_entries >= 0
    ),
  constraint des_soccer_learning_set_play_episode_neural_chk
    check (
      neural_training_steps >= 0
      and neural_samples >= 0
      and neural_replay_samples >= 0
    ),
  constraint des_soccer_learning_set_play_episode_goals_chk
    check (cumulative_goals >= 0),
  constraint des_soccer_learning_set_play_episode_goal_rate_chk
    check (goal_rate_so_far_micros between 0 and 1000000)
);

create index if not exists des_soccer_learning_set_play_episode_restart_idx
  on des_soccer_learning_set_play_episode_metrics (restart, scored, episode_index);

-- Create missing table: des_soccer_learning_neural_run_metrics
create table if not exists des_soccer_learning_neural_run_metrics (
  run_id uuid primary key references des_soccer_learning_runs(id) on delete cascade,
  policy_version_id uuid not null references des_soccer_learning_policy_versions(id) on delete cascade,
  enabled boolean not null,
  backend varchar(32) not null,
  training_steps integer not null,
  samples bigint not null,
  pending_batches integer not null,
  dropped_batches integer not null,
  replay_samples integer not null,
  replay_capacity integer not null,
  parameter_count integer not null,
  target_clip_micros bigint not null,
  last_loss_micros bigint,
  average_loss_micros bigint,
  created_at timestamptz default now() not null,
  constraint des_soccer_learning_neural_run_backend_chk
    check (backend in ('inline', 'threaded')),
  constraint des_soccer_learning_neural_run_counts_chk
    check (
      training_steps >= 0
      and samples >= 0
      and pending_batches >= 0
      and dropped_batches >= 0
      and replay_samples >= 0
      and replay_capacity >= 0
      and parameter_count >= 0
    )
);

create index if not exists des_soccer_learning_neural_run_steps_idx
  on des_soccer_learning_neural_run_metrics (training_steps desc, samples desc);

-- Create missing table: des_soccer_learning_pass_metrics
create table if not exists des_soccer_learning_pass_metrics (
  git_commit varchar(64) primary key,
  runs bigint default 0 not null,
  passes_attempted bigint default 0 not null,
  passes_completed bigint default 0 not null,
  completed_pass_gain_yards_micros bigint default 0 not null,
  pass_chains bigint default 0 not null,
  pass_chain_gain_yards_micros bigint default 0 not null,
  pass_chains_net_loss bigint default 0 not null,
  shots_on_target bigint default 0 not null,
  shots_after_pass bigint default 0 not null,
  first_seen_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint des_soccer_learning_pass_metrics_counts_chk
    check (
      runs >= 0
      and passes_attempted >= 0
      and passes_completed >= 0
      and pass_chains >= 0
      and pass_chains_net_loss >= 0
      and shots_on_target >= 0
      and shots_after_pass >= 0
    )
);

create index if not exists des_soccer_learning_pass_metrics_updated_idx
  on des_soccer_learning_pass_metrics (updated_at desc);

-- Create missing table: benefactor_marketing_clients
create table if not exists benefactor_marketing_clients (
  id uuid primary key default gen_random_uuid(),
  status varchar(32) default 'onboarding' not null,
  name varchar(200) not null,
  slug varchar(220) not null,
  industry varchar(120),
  website_url text,
  billing_email varchar(240),
  owner_user_id uuid,
  service_package varchar(120),
  onboarding_stage varchar(80) default 'intake' not null,
  portal_enabled boolean default true not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_clients_status_chk
    check (status in ('onboarding', 'active', 'paused', 'archived')),
  constraint benefactor_marketing_clients_name_size_chk
    check (octet_length(name) between 1 and 200),
  constraint benefactor_marketing_clients_slug_format_chk
    check (slug ~ '^[a-z0-9][a-z0-9-]{1,218}[a-z0-9]$'),
  constraint benefactor_marketing_clients_industry_size_chk
    check (industry is null or octet_length(industry) between 1 and 120),
  constraint benefactor_marketing_clients_website_size_chk
    check (website_url is null or octet_length(website_url) <= 2048),
  constraint benefactor_marketing_clients_billing_email_size_chk
    check (billing_email is null or octet_length(billing_email) <= 240),
  constraint benefactor_marketing_clients_service_package_size_chk
    check (service_package is null or octet_length(service_package) <= 120),
  constraint benefactor_marketing_clients_onboarding_stage_chk
    check (onboarding_stage ~ '^[A-Za-z0-9._:/-]{1,80}$'),
  constraint benefactor_marketing_clients_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists benefactor_marketing_clients_slug_uq
  on benefactor_marketing_clients (slug);

create index if not exists benefactor_marketing_clients_status_updated_at_idx
  on benefactor_marketing_clients (status, updated_at desc);

-- Create missing table: benefactor_marketing_contacts
create table if not exists benefactor_marketing_contacts (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  status varchar(32) default 'active' not null,
  first_name varchar(120),
  last_name varchar(120),
  email varchar(240),
  phone varchar(80),
  job_title varchar(160),
  lifecycle_role varchar(40) default 'other' not null,
  consent_status varchar(32) default 'unknown' not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_contacts_status_chk
    check (status in ('active', 'inactive', 'bounced', 'unsubscribed')),
  constraint benefactor_marketing_contacts_first_name_size_chk
    check (first_name is null or octet_length(first_name) between 1 and 120),
  constraint benefactor_marketing_contacts_last_name_size_chk
    check (last_name is null or octet_length(last_name) between 1 and 120),
  constraint benefactor_marketing_contacts_email_size_chk
    check (email is null or octet_length(email) <= 240),
  constraint benefactor_marketing_contacts_phone_size_chk
    check (phone is null or octet_length(phone) <= 80),
  constraint benefactor_marketing_contacts_job_title_size_chk
    check (job_title is null or octet_length(job_title) <= 160),
  constraint benefactor_marketing_contacts_lifecycle_role_chk
    check (lifecycle_role in ('primary', 'decision_maker', 'billing', 'technical', 'marketing', 'other')),
  constraint benefactor_marketing_contacts_consent_status_chk
    check (consent_status in ('unknown', 'opted_in', 'opted_out')),
  constraint benefactor_marketing_contacts_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists benefactor_marketing_contacts_client_status_idx
  on benefactor_marketing_contacts (client_id, status, updated_at desc);

create unique index if not exists benefactor_marketing_contacts_client_email_uq
  on benefactor_marketing_contacts (client_id, email)
  where email is not null;

-- Create missing table: benefactor_marketing_service_packages
create table if not exists benefactor_marketing_service_packages (
  id uuid primary key default gen_random_uuid(),
  status varchar(32) default 'active' not null,
  code varchar(120) not null,
  name varchar(200) not null,
  channel_mix jsonb default '[]'::jsonb not null,
  deliverables jsonb default '[]'::jsonb not null,
  monthly_budget_cents integer default 0 not null,
  retainer_cents integer default 0 not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_service_packages_status_chk
    check (status in ('active', 'retired')),
  constraint benefactor_marketing_service_packages_code_chk
    check (code ~ '^[A-Za-z0-9._:/-]{1,120}$'),
  constraint benefactor_marketing_service_packages_name_size_chk
    check (octet_length(name) between 1 and 200),
  constraint benefactor_marketing_service_packages_channel_mix_array_chk
    check (jsonb_typeof(channel_mix) = 'array'),
  constraint benefactor_marketing_service_packages_deliverables_array_chk
    check (jsonb_typeof(deliverables) = 'array'),
  constraint benefactor_marketing_service_packages_money_chk
    check (monthly_budget_cents >= 0 and retainer_cents >= 0),
  constraint benefactor_marketing_service_packages_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists benefactor_marketing_service_packages_code_uq
  on benefactor_marketing_service_packages (code);

-- Create missing table: benefactor_marketing_contracts
create table if not exists benefactor_marketing_contracts (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  package_id uuid,
  status varchar(32) default 'draft' not null,
  contract_number varchar(120),
  starts_on varchar(10),
  ends_on varchar(10),
  billing_terms jsonb default '{}'::jsonb not null,
  total_value_cents integer default 0 not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_contracts_status_chk
    check (status in ('draft', 'active', 'renewal', 'expired', 'terminated')),
  constraint benefactor_marketing_contracts_number_size_chk
    check (contract_number is null or octet_length(contract_number) <= 120),
  constraint benefactor_marketing_contracts_starts_on_chk
    check (starts_on is null or starts_on ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint benefactor_marketing_contracts_ends_on_chk
    check (ends_on is null or ends_on ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint benefactor_marketing_contracts_billing_terms_object_chk
    check (jsonb_typeof(billing_terms) = 'object'),
  constraint benefactor_marketing_contracts_total_value_chk
    check (total_value_cents >= 0),
  constraint benefactor_marketing_contracts_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists benefactor_marketing_contracts_client_status_idx
  on benefactor_marketing_contracts (client_id, status, updated_at desc);

-- Create missing table: benefactor_marketing_invoices
create table if not exists benefactor_marketing_invoices (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  contract_id uuid,
  status varchar(32) default 'draft' not null,
  invoice_number varchar(120),
  due_on varchar(10),
  amount_cents integer default 0 not null,
  paid_at timestamptz,
  line_items jsonb default '[]'::jsonb not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_invoices_status_chk
    check (status in ('draft', 'sent', 'paid', 'overdue', 'void')),
  constraint benefactor_marketing_invoices_number_size_chk
    check (invoice_number is null or octet_length(invoice_number) <= 120),
  constraint benefactor_marketing_invoices_due_on_chk
    check (due_on is null or due_on ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint benefactor_marketing_invoices_amount_chk
    check (amount_cents >= 0),
  constraint benefactor_marketing_invoices_line_items_array_chk
    check (jsonb_typeof(line_items) = 'array'),
  constraint benefactor_marketing_invoices_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists benefactor_marketing_invoices_client_status_idx
  on benefactor_marketing_invoices (client_id, status, updated_at desc);

create unique index if not exists benefactor_marketing_invoices_client_number_uq
  on benefactor_marketing_invoices (client_id, invoice_number)
  where invoice_number is not null;

-- Create missing table: benefactor_marketing_integrations
create table if not exists benefactor_marketing_integrations (
  id uuid primary key default gen_random_uuid(),
  client_id uuid,
  platform varchar(64) not null,
  status varchar(32) default 'connected' not null,
  auth_kind varchar(32) default 'manual' not null,
  external_account_id varchar(200),
  sync_cursor text,
  config jsonb default '{}'::jsonb not null,
  last_sync_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_integrations_platform_chk
    check (platform in ('salesforce', 'hubspot', 'apollo', 'zoominfo', 'google_analytics', 'google_ads', 'linkedin_ads', 'meta_ads', 'mailchimp', 'sendgrid', 'scraper', 'custom')),
  constraint benefactor_marketing_integrations_status_chk
    check (status in ('connected', 'disabled', 'error')),
  constraint benefactor_marketing_integrations_auth_kind_chk
    check (auth_kind in ('oauth2', 'api_key', 'webhook', 'manual')),
  constraint benefactor_marketing_integrations_external_account_size_chk
    check (external_account_id is null or octet_length(external_account_id) <= 200),
  constraint benefactor_marketing_integrations_sync_cursor_size_chk
    check (sync_cursor is null or octet_length(sync_cursor) <= 4000),
  constraint benefactor_marketing_integrations_config_object_chk
    check (jsonb_typeof(config) = 'object')
);

create index if not exists benefactor_marketing_integrations_client_platform_idx
  on benefactor_marketing_integrations (client_id, platform, status);

-- Create missing table: benefactor_marketing_leads
create table if not exists benefactor_marketing_leads (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  source_integration_id uuid,
  status varchar(32) default 'new' not null,
  company_name varchar(240) not null,
  domain varchar(240),
  contact_name varchar(200),
  contact_email varchar(240),
  contact_title varchar(160),
  country_code varchar(8),
  lead_score integer default 0 not null,
  icp_fit_score integer default 0 not null,
  verification_status varchar(32) default 'unknown' not null,
  enrichment_status varchar(32) default 'pending' not null,
  company_profile jsonb default '{}'::jsonb not null,
  signals jsonb default '[]'::jsonb not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_leads_status_chk
    check (status in ('new', 'researching', 'qualified', 'disqualified', 'contacted', 'converted')),
  constraint benefactor_marketing_leads_company_name_size_chk
    check (octet_length(company_name) between 1 and 240),
  constraint benefactor_marketing_leads_domain_size_chk
    check (domain is null or octet_length(domain) <= 240),
  constraint benefactor_marketing_leads_contact_name_size_chk
    check (contact_name is null or octet_length(contact_name) <= 200),
  constraint benefactor_marketing_leads_contact_email_size_chk
    check (contact_email is null or octet_length(contact_email) <= 240),
  constraint benefactor_marketing_leads_contact_title_size_chk
    check (contact_title is null or octet_length(contact_title) <= 160),
  constraint benefactor_marketing_leads_country_code_size_chk
    check (country_code is null or octet_length(country_code) <= 8),
  constraint benefactor_marketing_leads_score_chk
    check (lead_score between 0 and 100 and icp_fit_score between 0 and 100),
  constraint benefactor_marketing_leads_verification_status_chk
    check (verification_status in ('unknown', 'verified', 'invalid', 'risky')),
  constraint benefactor_marketing_leads_enrichment_status_chk
    check (enrichment_status in ('pending', 'running', 'completed', 'failed')),
  constraint benefactor_marketing_leads_company_profile_object_chk
    check (jsonb_typeof(company_profile) = 'object'),
  constraint benefactor_marketing_leads_signals_array_chk
    check (jsonb_typeof(signals) = 'array'),
  constraint benefactor_marketing_leads_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists benefactor_marketing_leads_client_status_score_idx
  on benefactor_marketing_leads (client_id, status, lead_score desc, updated_at desc);

create index if not exists benefactor_marketing_leads_domain_idx
  on benefactor_marketing_leads (domain)
  where domain is not null;

-- Create missing table: benefactor_marketing_enrichment_jobs
create table if not exists benefactor_marketing_enrichment_jobs (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  lead_id uuid,
  job_kind varchar(48) not null,
  status varchar(32) default 'queued' not null,
  external_job_id varchar(200),
  scraper_handoff_url text,
  input jsonb default '{}'::jsonb not null,
  result jsonb default '{}'::jsonb not null,
  error_summary text,
  queued_at timestamptz default now() not null,
  started_at timestamptz,
  completed_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_enrichment_jobs_kind_chk
    check (job_kind in ('lead_enrichment', 'company_research', 'contact_verification', 'prospect_scrape', 'competitive_intel')),
  constraint benefactor_marketing_enrichment_jobs_status_chk
    check (status in ('queued', 'running', 'completed', 'failed', 'canceled')),
  constraint benefactor_marketing_enrichment_jobs_external_job_id_size_chk
    check (external_job_id is null or octet_length(external_job_id) <= 200),
  constraint benefactor_marketing_enrichment_jobs_scraper_url_size_chk
    check (scraper_handoff_url is null or octet_length(scraper_handoff_url) <= 2048),
  constraint benefactor_marketing_enrichment_jobs_input_object_chk
    check (jsonb_typeof(input) = 'object'),
  constraint benefactor_marketing_enrichment_jobs_result_object_chk
    check (jsonb_typeof(result) = 'object'),
  constraint benefactor_marketing_enrichment_jobs_error_summary_size_chk
    check (error_summary is null or octet_length(error_summary) <= 4000)
);

create index if not exists benefactor_marketing_enrichment_jobs_client_status_idx
  on benefactor_marketing_enrichment_jobs (client_id, status, queued_at desc);

-- Create missing table: benefactor_marketing_campaigns
create table if not exists benefactor_marketing_campaigns (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  status varchar(32) default 'draft' not null,
  campaign_kind varchar(48) default 'multi_channel' not null,
  name varchar(220) not null,
  objective text,
  budget_cents integer default 0 not null,
  starts_on varchar(10),
  ends_on varchar(10),
  target_segments jsonb default '[]'::jsonb not null,
  kpis jsonb default '{}'::jsonb not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_campaigns_status_chk
    check (status in ('draft', 'active', 'paused', 'completed', 'archived')),
  constraint benefactor_marketing_campaigns_kind_chk
    check (campaign_kind in ('social_media', 'seo_aeo', 'email', 'outreach', 'paid_ads', 'content', 'multi_channel')),
  constraint benefactor_marketing_campaigns_name_size_chk
    check (octet_length(name) between 1 and 220),
  constraint benefactor_marketing_campaigns_objective_size_chk
    check (objective is null or octet_length(objective) <= 4000),
  constraint benefactor_marketing_campaigns_budget_chk
    check (budget_cents >= 0),
  constraint benefactor_marketing_campaigns_starts_on_chk
    check (starts_on is null or starts_on ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint benefactor_marketing_campaigns_ends_on_chk
    check (ends_on is null or ends_on ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint benefactor_marketing_campaigns_segments_array_chk
    check (jsonb_typeof(target_segments) = 'array'),
  constraint benefactor_marketing_campaigns_kpis_object_chk
    check (jsonb_typeof(kpis) = 'object'),
  constraint benefactor_marketing_campaigns_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists benefactor_marketing_campaigns_client_status_idx
  on benefactor_marketing_campaigns (client_id, status, updated_at desc);

-- Create missing table: benefactor_marketing_campaign_channels
create table if not exists benefactor_marketing_campaign_channels (
  id uuid primary key default gen_random_uuid(),
  campaign_id uuid not null,
  channel varchar(48) not null,
  status varchar(32) default 'draft' not null,
  external_campaign_id varchar(200),
  strategy jsonb default '{}'::jsonb not null,
  schedule jsonb default '{}'::jsonb not null,
  metrics_snapshot jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_campaign_channels_channel_chk
    check (channel in ('social', 'linkedin', 'email', 'sms', 'seo', 'aeo', 'google_ads', 'meta_ads', 'landing_page', 'content')),
  constraint benefactor_marketing_campaign_channels_status_chk
    check (status in ('draft', 'scheduled', 'live', 'paused', 'completed')),
  constraint benefactor_marketing_campaign_channels_external_id_size_chk
    check (external_campaign_id is null or octet_length(external_campaign_id) <= 200),
  constraint benefactor_marketing_campaign_channels_strategy_object_chk
    check (jsonb_typeof(strategy) = 'object'),
  constraint benefactor_marketing_campaign_channels_schedule_object_chk
    check (jsonb_typeof(schedule) = 'object'),
  constraint benefactor_marketing_campaign_channels_metrics_object_chk
    check (jsonb_typeof(metrics_snapshot) = 'object')
);

create index if not exists benefactor_marketing_campaign_channels_campaign_idx
  on benefactor_marketing_campaign_channels (campaign_id, channel, status);

-- Create missing table: benefactor_marketing_campaign_experiments
create table if not exists benefactor_marketing_campaign_experiments (
  id uuid primary key default gen_random_uuid(),
  campaign_id uuid not null,
  status varchar(32) default 'draft' not null,
  experiment_kind varchar(48) not null,
  hypothesis text,
  variants jsonb default '[]'::jsonb not null,
  winning_variant varchar(120),
  result_summary jsonb default '{}'::jsonb not null,
  started_at timestamptz,
  ended_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_campaign_experiments_status_chk
    check (status in ('draft', 'running', 'winner_selected', 'stopped')),
  constraint benefactor_marketing_campaign_experiments_kind_chk
    check (experiment_kind in ('subject_line', 'creative', 'copy', 'landing_page', 'audience', 'budget')),
  constraint benefactor_marketing_campaign_experiments_hypothesis_size_chk
    check (hypothesis is null or octet_length(hypothesis) <= 4000),
  constraint benefactor_marketing_campaign_experiments_variants_array_chk
    check (jsonb_typeof(variants) = 'array'),
  constraint benefactor_marketing_campaign_experiments_winner_size_chk
    check (winning_variant is null or octet_length(winning_variant) <= 120),
  constraint benefactor_marketing_campaign_experiments_result_object_chk
    check (jsonb_typeof(result_summary) = 'object')
);

create index if not exists benefactor_marketing_campaign_experiments_campaign_idx
  on benefactor_marketing_campaign_experiments (campaign_id, status, updated_at desc);

-- Create missing table: benefactor_marketing_automation_workflows
create table if not exists benefactor_marketing_automation_workflows (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  status varchar(32) default 'draft' not null,
  name varchar(220) not null,
  trigger_kind varchar(64) not null,
  trigger_config jsonb default '{}'::jsonb not null,
  action_graph jsonb default '{}'::jsonb not null,
  last_run_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_automation_workflows_status_chk
    check (status in ('draft', 'active', 'paused', 'archived')),
  constraint benefactor_marketing_automation_workflows_name_size_chk
    check (octet_length(name) between 1 and 220),
  constraint benefactor_marketing_automation_workflows_trigger_kind_chk
    check (trigger_kind in ('lead_created', 'score_changed', 'form_submit', 'email_event', 'campaign_event', 'manual', 'schedule', 'webhook')),
  constraint benefactor_marketing_automation_workflows_trigger_object_chk
    check (jsonb_typeof(trigger_config) = 'object'),
  constraint benefactor_marketing_automation_workflows_action_object_chk
    check (jsonb_typeof(action_graph) = 'object')
);

create index if not exists benefactor_marketing_automation_workflows_client_status_idx
  on benefactor_marketing_automation_workflows (client_id, status, updated_at desc);

-- Create missing table: benefactor_marketing_automation_events
create table if not exists benefactor_marketing_automation_events (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  workflow_id uuid,
  lead_id uuid,
  event_kind varchar(80) not null,
  status varchar(32) default 'received' not null,
  payload jsonb default '{}'::jsonb not null,
  error_summary text,
  created_at timestamptz default now() not null,
  constraint benefactor_marketing_automation_events_kind_chk
    check (event_kind ~ '^[A-Za-z0-9._:/-]{1,80}$'),
  constraint benefactor_marketing_automation_events_status_chk
    check (status in ('received', 'processed', 'failed', 'skipped')),
  constraint benefactor_marketing_automation_events_payload_object_chk
    check (jsonb_typeof(payload) = 'object'),
  constraint benefactor_marketing_automation_events_error_summary_size_chk
    check (error_summary is null or octet_length(error_summary) <= 4000)
);

create index if not exists benefactor_marketing_automation_events_client_created_idx
  on benefactor_marketing_automation_events (client_id, created_at desc);

-- Create missing table: benefactor_marketing_reports
create table if not exists benefactor_marketing_reports (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  campaign_id uuid,
  report_kind varchar(48) default 'dashboard' not null,
  status varchar(32) default 'draft' not null,
  period_start varchar(10),
  period_end varchar(10),
  metrics jsonb default '{}'::jsonb not null,
  narrative text,
  delivery_targets jsonb default '[]'::jsonb not null,
  generated_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_reports_kind_chk
    check (report_kind in ('dashboard', 'executive_summary', 'attribution', 'funnel', 'roi', 'seo_aeo', 'client_portal')),
  constraint benefactor_marketing_reports_status_chk
    check (status in ('draft', 'ready', 'sent', 'archived')),
  constraint benefactor_marketing_reports_period_start_chk
    check (period_start is null or period_start ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint benefactor_marketing_reports_period_end_chk
    check (period_end is null or period_end ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint benefactor_marketing_reports_metrics_object_chk
    check (jsonb_typeof(metrics) = 'object'),
  constraint benefactor_marketing_reports_narrative_size_chk
    check (narrative is null or octet_length(narrative) <= 20000),
  constraint benefactor_marketing_reports_delivery_targets_array_chk
    check (jsonb_typeof(delivery_targets) = 'array')
);

create index if not exists benefactor_marketing_reports_client_kind_idx
  on benefactor_marketing_reports (client_id, report_kind, updated_at desc);

-- Create missing table: benefactor_marketing_attribution_events
create table if not exists benefactor_marketing_attribution_events (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  campaign_id uuid,
  lead_id uuid,
  event_type varchar(64) not null,
  source_platform varchar(64),
  source_event_id varchar(200),
  occurred_at timestamptz default now() not null,
  value_cents integer default 0 not null,
  payload jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  constraint benefactor_marketing_attribution_events_type_chk
    check (event_type in ('impression', 'click', 'form_submit', 'email_open', 'email_click', 'meeting_booked', 'opportunity_created', 'deal_won', 'revenue')),
  constraint benefactor_marketing_attribution_events_source_platform_size_chk
    check (source_platform is null or octet_length(source_platform) <= 64),
  constraint benefactor_marketing_attribution_events_source_event_id_size_chk
    check (source_event_id is null or octet_length(source_event_id) <= 200),
  constraint benefactor_marketing_attribution_events_value_chk
    check (value_cents >= 0),
  constraint benefactor_marketing_attribution_events_payload_object_chk
    check (jsonb_typeof(payload) = 'object')
);

create index if not exists benefactor_marketing_attribution_events_client_type_idx
  on benefactor_marketing_attribution_events (client_id, event_type, occurred_at desc);

create unique index if not exists benefactor_marketing_attribution_events_source_uq
  on benefactor_marketing_attribution_events (source_platform, source_event_id)
  where source_platform is not null and source_event_id is not null;

-- Create missing table: benefactor_marketing_opportunities
create table if not exists benefactor_marketing_opportunities (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  lead_id uuid,
  status varchar(32) default 'open' not null,
  stage varchar(48) default 'prospecting' not null,
  name varchar(220) not null,
  amount_cents integer default 0 not null,
  probability_micros integer default 0 not null,
  expected_close_on varchar(10),
  owner_user_id uuid,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_opportunities_status_chk
    check (status in ('open', 'won', 'lost', 'paused')),
  constraint benefactor_marketing_opportunities_stage_chk
    check (stage in ('prospecting', 'qualified', 'meeting', 'proposal', 'negotiation', 'closed')),
  constraint benefactor_marketing_opportunities_name_size_chk
    check (octet_length(name) between 1 and 220),
  constraint benefactor_marketing_opportunities_amount_chk
    check (amount_cents >= 0),
  constraint benefactor_marketing_opportunities_probability_chk
    check (probability_micros between 0 and 1000000),
  constraint benefactor_marketing_opportunities_expected_close_chk
    check (expected_close_on is null or expected_close_on ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint benefactor_marketing_opportunities_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists benefactor_marketing_opportunities_client_stage_idx
  on benefactor_marketing_opportunities (client_id, stage, updated_at desc);

-- Create missing table: benefactor_marketing_content_assets
create table if not exists benefactor_marketing_content_assets (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  campaign_id uuid,
  status varchar(32) default 'draft' not null,
  asset_kind varchar(48) not null,
  title varchar(240) not null,
  channel varchar(64),
  body text,
  asset_uri text,
  seo_keywords jsonb default '[]'::jsonb not null,
  approval_status varchar(32) default 'pending' not null,
  publish_at timestamptz,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_content_assets_status_chk
    check (status in ('draft', 'in_review', 'approved', 'scheduled', 'published', 'archived')),
  constraint benefactor_marketing_content_assets_kind_chk
    check (asset_kind in ('blog', 'social_post', 'email', 'landing_page', 'ad_creative', 'video', 'script', 'proposal', 'report')),
  constraint benefactor_marketing_content_assets_title_size_chk
    check (octet_length(title) between 1 and 240),
  constraint benefactor_marketing_content_assets_channel_size_chk
    check (channel is null or octet_length(channel) <= 64),
  constraint benefactor_marketing_content_assets_body_size_chk
    check (body is null or octet_length(body) <= 100000),
  constraint benefactor_marketing_content_assets_asset_uri_size_chk
    check (asset_uri is null or octet_length(asset_uri) <= 2048),
  constraint benefactor_marketing_content_assets_keywords_array_chk
    check (jsonb_typeof(seo_keywords) = 'array'),
  constraint benefactor_marketing_content_assets_approval_status_chk
    check (approval_status in ('pending', 'approved', 'rejected', 'changes_requested')),
  constraint benefactor_marketing_content_assets_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists benefactor_marketing_content_assets_client_status_idx
  on benefactor_marketing_content_assets (client_id, status, updated_at desc);

-- Create missing table: benefactor_marketing_project_tasks
create table if not exists benefactor_marketing_project_tasks (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  campaign_id uuid,
  content_asset_id uuid,
  status varchar(32) default 'todo' not null,
  priority varchar(32) default 'normal' not null,
  title varchar(240) not null,
  description text,
  assigned_to uuid,
  due_on varchar(10),
  sla_due_at timestamptz,
  time_spent_minutes integer default 0 not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_project_tasks_status_chk
    check (status in ('todo', 'in_progress', 'blocked', 'done', 'canceled')),
  constraint benefactor_marketing_project_tasks_priority_chk
    check (priority in ('low', 'normal', 'high', 'urgent')),
  constraint benefactor_marketing_project_tasks_title_size_chk
    check (octet_length(title) between 1 and 240),
  constraint benefactor_marketing_project_tasks_description_size_chk
    check (description is null or octet_length(description) <= 20000),
  constraint benefactor_marketing_project_tasks_due_on_chk
    check (due_on is null or due_on ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint benefactor_marketing_project_tasks_time_spent_chk
    check (time_spent_minutes >= 0),
  constraint benefactor_marketing_project_tasks_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists benefactor_marketing_project_tasks_client_status_idx
  on benefactor_marketing_project_tasks (client_id, status, priority, updated_at desc);

-- Create missing table: benefactor_marketing_client_approvals
create table if not exists benefactor_marketing_client_approvals (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  campaign_id uuid,
  content_asset_id uuid,
  requested_by uuid,
  status varchar(32) default 'pending' not null,
  approval_kind varchar(48) not null,
  title varchar(240) not null,
  request_payload jsonb default '{}'::jsonb not null,
  response_note text,
  due_at timestamptz,
  decided_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_client_approvals_status_chk
    check (status in ('pending', 'approved', 'rejected', 'expired', 'canceled')),
  constraint benefactor_marketing_client_approvals_kind_chk
    check (approval_kind in ('campaign_launch', 'content_publish', 'budget_change', 'report_send', 'lead_list')),
  constraint benefactor_marketing_client_approvals_title_size_chk
    check (octet_length(title) between 1 and 240),
  constraint benefactor_marketing_client_approvals_payload_object_chk
    check (jsonb_typeof(request_payload) = 'object'),
  constraint benefactor_marketing_client_approvals_response_note_size_chk
    check (response_note is null or octet_length(response_note) <= 4000)
);

create index if not exists benefactor_marketing_client_approvals_client_status_idx
  on benefactor_marketing_client_approvals (client_id, status, updated_at desc);

-- Create missing table: benefactor_marketing_tickets
create table if not exists benefactor_marketing_tickets (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  status varchar(32) default 'open' not null,
  priority varchar(32) default 'normal' not null,
  subject varchar(240) not null,
  description text,
  source varchar(32) default 'portal' not null,
  assigned_to uuid,
  last_activity_at timestamptz default now() not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_tickets_status_chk
    check (status in ('open', 'pending_client', 'pending_agency', 'resolved', 'closed')),
  constraint benefactor_marketing_tickets_priority_chk
    check (priority in ('low', 'normal', 'high', 'urgent')),
  constraint benefactor_marketing_tickets_subject_size_chk
    check (octet_length(subject) between 1 and 240),
  constraint benefactor_marketing_tickets_description_size_chk
    check (description is null or octet_length(description) <= 20000),
  constraint benefactor_marketing_tickets_source_chk
    check (source in ('portal', 'email', 'internal')),
  constraint benefactor_marketing_tickets_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists benefactor_marketing_tickets_client_status_idx
  on benefactor_marketing_tickets (client_id, status, priority, updated_at desc);

-- Create missing table: benefactor_marketing_meetings
create table if not exists benefactor_marketing_meetings (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  lead_id uuid,
  opportunity_id uuid,
  status varchar(32) default 'scheduled' not null,
  meeting_kind varchar(48) not null,
  title varchar(240) not null,
  scheduled_at timestamptz not null,
  duration_minutes integer default 30 not null,
  notes text,
  recording_uri text,
  transcript_summary jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_meetings_status_chk
    check (status in ('scheduled', 'completed', 'canceled', 'no_show')),
  constraint benefactor_marketing_meetings_kind_chk
    check (meeting_kind in ('onboarding', 'report_review', 'sales_discovery', 'strategy', 'content_review', 'support')),
  constraint benefactor_marketing_meetings_title_size_chk
    check (octet_length(title) between 1 and 240),
  constraint benefactor_marketing_meetings_duration_chk
    check (duration_minutes between 1 and 1440),
  constraint benefactor_marketing_meetings_notes_size_chk
    check (notes is null or octet_length(notes) <= 20000),
  constraint benefactor_marketing_meetings_recording_uri_size_chk
    check (recording_uri is null or octet_length(recording_uri) <= 2048),
  constraint benefactor_marketing_meetings_transcript_summary_object_chk
    check (jsonb_typeof(transcript_summary) = 'object')
);

create index if not exists benefactor_marketing_meetings_client_scheduled_idx
  on benefactor_marketing_meetings (client_id, scheduled_at desc);

-- Create missing table: benefactor_marketing_team_allocations
create table if not exists benefactor_marketing_team_allocations (
  id uuid primary key default gen_random_uuid(),
  client_id uuid,
  campaign_id uuid,
  user_id uuid not null,
  role varchar(48) not null,
  allocation_percent integer default 100 not null,
  starts_on varchar(10),
  ends_on varchar(10),
  billable boolean default true not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_team_allocations_role_chk
    check (role in ('strategist', 'designer', 'copywriter', 'analyst', 'sdr', 'account_manager', 'seo_specialist')),
  constraint benefactor_marketing_team_allocations_percent_chk
    check (allocation_percent between 0 and 100),
  constraint benefactor_marketing_team_allocations_starts_on_chk
    check (starts_on is null or starts_on ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint benefactor_marketing_team_allocations_ends_on_chk
    check (ends_on is null or ends_on ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$')
);

create index if not exists benefactor_marketing_team_allocations_user_idx
  on benefactor_marketing_team_allocations (user_id, starts_on, ends_on);

create index if not exists benefactor_marketing_team_allocations_client_idx
  on benefactor_marketing_team_allocations (client_id, role)
  where client_id is not null;

-- Create missing table: benefactor_marketing_integration_sync_runs
create table if not exists benefactor_marketing_integration_sync_runs (
  id uuid primary key default gen_random_uuid(),
  integration_id uuid not null,
  client_id uuid,
  sync_kind varchar(48) default 'incremental' not null,
  direction varchar(24) default 'import' not null,
  status varchar(32) default 'queued' not null,
  records_seen integer default 0 not null,
  records_changed integer default 0 not null,
  cursor_before text,
  cursor_after text,
  payload jsonb default '{}'::jsonb not null,
  error_summary text,
  started_at timestamptz,
  completed_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_integration_sync_runs_kind_chk
    check (sync_kind in ('incremental', 'full', 'webhook', 'backfill', 'export')),
  constraint benefactor_marketing_integration_sync_runs_direction_chk
    check (direction in ('import', 'export', 'bidirectional')),
  constraint benefactor_marketing_integration_sync_runs_status_chk
    check (status in ('queued', 'running', 'succeeded', 'failed', 'canceled')),
  constraint benefactor_marketing_integration_sync_runs_counts_chk
    check (records_seen >= 0 and records_changed >= 0),
  constraint benefactor_marketing_integration_sync_runs_cursor_before_size_chk
    check (cursor_before is null or octet_length(cursor_before) <= 4000),
  constraint benefactor_marketing_integration_sync_runs_cursor_after_size_chk
    check (cursor_after is null or octet_length(cursor_after) <= 4000),
  constraint benefactor_marketing_integration_sync_runs_payload_object_chk
    check (jsonb_typeof(payload) = 'object'),
  constraint benefactor_marketing_integration_sync_runs_error_summary_size_chk
    check (error_summary is null or octet_length(error_summary) <= 4000)
);

create index if not exists benefactor_marketing_integration_sync_runs_integration_idx
  on benefactor_marketing_integration_sync_runs (integration_id, status, created_at desc);

create index if not exists benefactor_marketing_integration_sync_runs_client_idx
  on benefactor_marketing_integration_sync_runs (client_id, created_at desc)
  where client_id is not null;

-- Create missing table: benefactor_marketing_outreach_sequences
create table if not exists benefactor_marketing_outreach_sequences (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  campaign_id uuid,
  status varchar(32) default 'draft' not null,
  channel varchar(32) default 'email' not null,
  name varchar(220) not null,
  audience_filter jsonb default '{}'::jsonb not null,
  cadence jsonb default '{}'::jsonb not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_outreach_sequences_status_chk
    check (status in ('draft', 'active', 'paused', 'completed', 'archived')),
  constraint benefactor_marketing_outreach_sequences_channel_chk
    check (channel in ('email', 'linkedin', 'sms', 'phone', 'multi_channel')),
  constraint benefactor_marketing_outreach_sequences_name_size_chk
    check (octet_length(name) between 1 and 220),
  constraint benefactor_marketing_outreach_sequences_audience_object_chk
    check (jsonb_typeof(audience_filter) = 'object'),
  constraint benefactor_marketing_outreach_sequences_cadence_object_chk
    check (jsonb_typeof(cadence) = 'object'),
  constraint benefactor_marketing_outreach_sequences_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists benefactor_marketing_outreach_sequences_client_status_idx
  on benefactor_marketing_outreach_sequences (client_id, status, updated_at desc);

-- Create missing table: benefactor_marketing_outreach_steps
create table if not exists benefactor_marketing_outreach_steps (
  id uuid primary key default gen_random_uuid(),
  sequence_id uuid not null,
  status varchar(32) default 'active' not null,
  step_order integer not null,
  channel varchar(32) not null,
  delay_minutes integer default 0 not null,
  subject varchar(240),
  body_template text,
  personalization_hints jsonb default '[]'::jsonb not null,
  experiment_key varchar(120),
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_outreach_steps_status_chk
    check (status in ('active', 'disabled', 'archived')),
  constraint benefactor_marketing_outreach_steps_order_chk
    check (step_order between 1 and 100),
  constraint benefactor_marketing_outreach_steps_channel_chk
    check (channel in ('email', 'linkedin', 'sms', 'phone', 'task')),
  constraint benefactor_marketing_outreach_steps_delay_chk
    check (delay_minutes between 0 and 525600),
  constraint benefactor_marketing_outreach_steps_subject_size_chk
    check (subject is null or octet_length(subject) <= 240),
  constraint benefactor_marketing_outreach_steps_body_size_chk
    check (body_template is null or octet_length(body_template) <= 100000),
  constraint benefactor_marketing_outreach_steps_hints_array_chk
    check (jsonb_typeof(personalization_hints) = 'array'),
  constraint benefactor_marketing_outreach_steps_experiment_key_size_chk
    check (experiment_key is null or octet_length(experiment_key) <= 120)
);

create unique index if not exists benefactor_marketing_outreach_steps_sequence_order_uq
  on benefactor_marketing_outreach_steps (sequence_id, step_order);

-- Create missing table: benefactor_marketing_outreach_enrollments
create table if not exists benefactor_marketing_outreach_enrollments (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  sequence_id uuid not null,
  lead_id uuid,
  contact_id uuid,
  status varchar(32) default 'active' not null,
  current_step_order integer default 1 not null,
  enrollment_context jsonb default '{}'::jsonb not null,
  last_touch_at timestamptz,
  next_touch_at timestamptz,
  outcome varchar(64),
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_outreach_enrollments_target_chk
    check (lead_id is not null or contact_id is not null),
  constraint benefactor_marketing_outreach_enrollments_status_chk
    check (status in ('active', 'paused', 'completed', 'bounced', 'unsubscribed', 'failed')),
  constraint benefactor_marketing_outreach_enrollments_step_chk
    check (current_step_order between 1 and 100),
  constraint benefactor_marketing_outreach_enrollments_context_object_chk
    check (jsonb_typeof(enrollment_context) = 'object'),
  constraint benefactor_marketing_outreach_enrollments_outcome_size_chk
    check (outcome is null or octet_length(outcome) <= 64)
);

create index if not exists benefactor_marketing_outreach_enrollments_sequence_idx
  on benefactor_marketing_outreach_enrollments (sequence_id, status, next_touch_at);

create index if not exists benefactor_marketing_outreach_enrollments_client_idx
  on benefactor_marketing_outreach_enrollments (client_id, status, updated_at desc);

-- Create missing table: benefactor_marketing_outreach_touchpoints
create table if not exists benefactor_marketing_outreach_touchpoints (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  sequence_id uuid,
  enrollment_id uuid,
  campaign_id uuid,
  lead_id uuid,
  contact_id uuid,
  channel varchar(32) not null,
  direction varchar(24) default 'outbound' not null,
  status varchar(32) default 'planned' not null,
  subject varchar(240),
  body_excerpt text,
  external_message_id varchar(200),
  occurred_at timestamptz default now() not null,
  payload jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  constraint benefactor_marketing_outreach_touchpoints_channel_chk
    check (channel in ('email', 'linkedin', 'sms', 'phone', 'task', 'meeting')),
  constraint benefactor_marketing_outreach_touchpoints_direction_chk
    check (direction in ('outbound', 'inbound', 'internal')),
  constraint benefactor_marketing_outreach_touchpoints_status_chk
    check (status in ('planned', 'sent', 'delivered', 'opened', 'clicked', 'replied', 'failed', 'bounced')),
  constraint benefactor_marketing_outreach_touchpoints_subject_size_chk
    check (subject is null or octet_length(subject) <= 240),
  constraint benefactor_marketing_outreach_touchpoints_body_size_chk
    check (body_excerpt is null or octet_length(body_excerpt) <= 4000),
  constraint benefactor_marketing_outreach_touchpoints_external_message_size_chk
    check (external_message_id is null or octet_length(external_message_id) <= 200),
  constraint benefactor_marketing_outreach_touchpoints_payload_object_chk
    check (jsonb_typeof(payload) = 'object')
);

create index if not exists benefactor_marketing_outreach_touchpoints_client_idx
  on benefactor_marketing_outreach_touchpoints (client_id, occurred_at desc);

create unique index if not exists benefactor_marketing_outreach_touchpoints_external_uq
  on benefactor_marketing_outreach_touchpoints (channel, external_message_id)
  where external_message_id is not null;

-- Create missing table: benefactor_marketing_prospect_research_briefs
create table if not exists benefactor_marketing_prospect_research_briefs (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  lead_id uuid,
  status varchar(32) default 'draft' not null,
  research_kind varchar(48) default 'account_research' not null,
  source varchar(48) default 'ai_assisted' not null,
  summary text,
  findings jsonb default '[]'::jsonb not null,
  recommended_actions jsonb default '[]'::jsonb not null,
  confidence_micros integer default 0 not null,
  model_name varchar(120),
  generated_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_prospect_research_briefs_status_chk
    check (status in ('draft', 'ready', 'stale', 'failed')),
  constraint benefactor_marketing_prospect_research_briefs_kind_chk
    check (research_kind in ('account_research', 'contact_research', 'competitive_intel', 'proposal_brief', 'outreach_personalization')),
  constraint benefactor_marketing_prospect_research_briefs_source_chk
    check (source in ('ai_assisted', 'analyst', 'scraper', 'integration')),
  constraint benefactor_marketing_prospect_research_briefs_summary_size_chk
    check (summary is null or octet_length(summary) <= 20000),
  constraint benefactor_marketing_prospect_research_briefs_findings_array_chk
    check (jsonb_typeof(findings) = 'array'),
  constraint benefactor_marketing_prospect_research_briefs_actions_array_chk
    check (jsonb_typeof(recommended_actions) = 'array'),
  constraint benefactor_marketing_prospect_research_briefs_confidence_chk
    check (confidence_micros between 0 and 1000000),
  constraint benefactor_marketing_prospect_research_briefs_model_size_chk
    check (model_name is null or octet_length(model_name) <= 120)
);

create index if not exists benefactor_marketing_prospect_research_briefs_client_idx
  on benefactor_marketing_prospect_research_briefs (client_id, status, updated_at desc);

create index if not exists benefactor_marketing_prospect_research_briefs_lead_idx
  on benefactor_marketing_prospect_research_briefs (lead_id, generated_at desc)
  where lead_id is not null;

-- Create missing table: benefactor_marketing_conversion_events
create table if not exists benefactor_marketing_conversion_events (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  campaign_id uuid,
  lead_id uuid,
  content_asset_id uuid,
  event_type varchar(64) not null,
  source_platform varchar(64),
  source_event_id varchar(200),
  session_id varchar(200),
  visitor_key varchar(200),
  occurred_at timestamptz default now() not null,
  value_cents integer default 0 not null,
  utm jsonb default '{}'::jsonb not null,
  payload jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  constraint benefactor_marketing_conversion_events_type_chk
    check (event_type in ('landing_page_view', 'form_submit', 'chat_started', 'calendar_booked', 'asset_download', 'trial_signup', 'purchase', 'custom')),
  constraint benefactor_marketing_conversion_events_source_platform_size_chk
    check (source_platform is null or octet_length(source_platform) <= 64),
  constraint benefactor_marketing_conversion_events_source_event_id_size_chk
    check (source_event_id is null or octet_length(source_event_id) <= 200),
  constraint benefactor_marketing_conversion_events_session_size_chk
    check (session_id is null or octet_length(session_id) <= 200),
  constraint benefactor_marketing_conversion_events_visitor_size_chk
    check (visitor_key is null or octet_length(visitor_key) <= 200),
  constraint benefactor_marketing_conversion_events_value_chk
    check (value_cents >= 0),
  constraint benefactor_marketing_conversion_events_utm_object_chk
    check (jsonb_typeof(utm) = 'object'),
  constraint benefactor_marketing_conversion_events_payload_object_chk
    check (jsonb_typeof(payload) = 'object')
);

create index if not exists benefactor_marketing_conversion_events_client_type_idx
  on benefactor_marketing_conversion_events (client_id, event_type, occurred_at desc);

create unique index if not exists benefactor_marketing_conversion_events_source_uq
  on benefactor_marketing_conversion_events (source_platform, source_event_id)
  where source_platform is not null and source_event_id is not null;

-- Create missing table: benefactor_marketing_portal_members
create table if not exists benefactor_marketing_portal_members (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  contact_id uuid,
  user_id uuid,
  email varchar(240) not null,
  status varchar(32) default 'invited' not null,
  role varchar(32) default 'viewer' not null,
  access_scope jsonb default '{}'::jsonb not null,
  last_seen_at timestamptz,
  invited_at timestamptz default now() not null,
  accepted_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_portal_members_email_size_chk
    check (octet_length(email) between 3 and 240),
  constraint benefactor_marketing_portal_members_status_chk
    check (status in ('invited', 'active', 'disabled', 'revoked')),
  constraint benefactor_marketing_portal_members_role_chk
    check (role in ('owner', 'approver', 'viewer', 'billing', 'collaborator')),
  constraint benefactor_marketing_portal_members_access_scope_object_chk
    check (jsonb_typeof(access_scope) = 'object')
);

create unique index if not exists benefactor_marketing_portal_members_client_email_uq
  on benefactor_marketing_portal_members (client_id, email);

-- Create missing table: benefactor_marketing_shared_documents
create table if not exists benefactor_marketing_shared_documents (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  campaign_id uuid,
  content_asset_id uuid,
  status varchar(32) default 'active' not null,
  document_kind varchar(48) not null,
  title varchar(240) not null,
  storage_uri text not null,
  mime_type varchar(120),
  visibility varchar(32) default 'client_portal' not null,
  uploaded_by uuid,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_shared_documents_status_chk
    check (status in ('active', 'archived', 'deleted')),
  constraint benefactor_marketing_shared_documents_kind_chk
    check (document_kind in ('contract', 'invoice', 'report', 'creative', 'brand_asset', 'proposal', 'meeting_notes', 'other')),
  constraint benefactor_marketing_shared_documents_title_size_chk
    check (octet_length(title) between 1 and 240),
  constraint benefactor_marketing_shared_documents_uri_size_chk
    check (octet_length(storage_uri) between 1 and 2048),
  constraint benefactor_marketing_shared_documents_mime_size_chk
    check (mime_type is null or octet_length(mime_type) <= 120),
  constraint benefactor_marketing_shared_documents_visibility_chk
    check (visibility in ('internal', 'client_portal', 'public_link')),
  constraint benefactor_marketing_shared_documents_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists benefactor_marketing_shared_documents_client_idx
  on benefactor_marketing_shared_documents (client_id, status, updated_at desc);

-- Create missing table: benefactor_marketing_collaboration_comments
create table if not exists benefactor_marketing_collaboration_comments (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  parent_comment_id uuid,
  resource_type varchar(64) not null,
  resource_id uuid,
  author_user_id uuid,
  author_contact_id uuid,
  body text not null,
  status varchar(32) default 'open' not null,
  visibility varchar(32) default 'client_portal' not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_collaboration_comments_resource_type_chk
    check (resource_type in ('client', 'campaign', 'content_asset', 'approval', 'ticket', 'document', 'report', 'meeting')),
  constraint benefactor_marketing_collaboration_comments_body_size_chk
    check (octet_length(body) between 1 and 20000),
  constraint benefactor_marketing_collaboration_comments_status_chk
    check (status in ('open', 'resolved', 'archived')),
  constraint benefactor_marketing_collaboration_comments_visibility_chk
    check (visibility in ('internal', 'client_portal')),
  constraint benefactor_marketing_collaboration_comments_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists benefactor_marketing_collaboration_comments_client_idx
  on benefactor_marketing_collaboration_comments (client_id, resource_type, updated_at desc);

-- Create missing table: benefactor_marketing_notifications
create table if not exists benefactor_marketing_notifications (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  recipient_user_id uuid,
  recipient_contact_id uuid,
  channel varchar(32) default 'email' not null,
  status varchar(32) default 'queued' not null,
  notification_kind varchar(64) not null,
  title varchar(240) not null,
  body text,
  payload jsonb default '{}'::jsonb not null,
  scheduled_at timestamptz,
  sent_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_notifications_recipient_chk
    check (recipient_user_id is not null or recipient_contact_id is not null),
  constraint benefactor_marketing_notifications_channel_chk
    check (channel in ('email', 'sms', 'portal', 'slack', 'webhook')),
  constraint benefactor_marketing_notifications_status_chk
    check (status in ('queued', 'scheduled', 'sent', 'failed', 'canceled')),
  constraint benefactor_marketing_notifications_kind_chk
    check (notification_kind in ('approval_request', 'comment', 'report_ready', 'ticket_update', 'meeting_reminder', 'budget_alert', 'custom')),
  constraint benefactor_marketing_notifications_title_size_chk
    check (octet_length(title) between 1 and 240),
  constraint benefactor_marketing_notifications_body_size_chk
    check (body is null or octet_length(body) <= 20000),
  constraint benefactor_marketing_notifications_payload_object_chk
    check (jsonb_typeof(payload) = 'object')
);

create index if not exists benefactor_marketing_notifications_client_status_idx
  on benefactor_marketing_notifications (client_id, status, scheduled_at desc);

-- Create missing table: benefactor_marketing_time_entries
create table if not exists benefactor_marketing_time_entries (
  id uuid primary key default gen_random_uuid(),
  client_id uuid,
  campaign_id uuid,
  project_task_id uuid,
  user_id uuid not null,
  entry_date varchar(10) not null,
  minutes integer not null,
  billable boolean default true not null,
  rate_cents integer default 0 not null,
  cost_cents integer default 0 not null,
  notes text,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_time_entries_date_chk
    check (entry_date ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint benefactor_marketing_time_entries_minutes_chk
    check (minutes between 1 and 1440),
  constraint benefactor_marketing_time_entries_money_chk
    check (rate_cents >= 0 and cost_cents >= 0),
  constraint benefactor_marketing_time_entries_notes_size_chk
    check (notes is null or octet_length(notes) <= 4000),
  constraint benefactor_marketing_time_entries_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists benefactor_marketing_time_entries_client_date_idx
  on benefactor_marketing_time_entries (client_id, entry_date desc)
  where client_id is not null;

-- Create missing table: benefactor_marketing_vendor_costs
create table if not exists benefactor_marketing_vendor_costs (
  id uuid primary key default gen_random_uuid(),
  client_id uuid,
  campaign_id uuid,
  vendor_name varchar(200) not null,
  category varchar(64) not null,
  status varchar(32) default 'planned' not null,
  amount_cents integer not null,
  incurred_on varchar(10),
  invoice_ref varchar(120),
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_vendor_costs_vendor_size_chk
    check (octet_length(vendor_name) between 1 and 200),
  constraint benefactor_marketing_vendor_costs_category_chk
    check (category in ('ads', 'creative', 'data', 'software', 'contractor', 'events', 'other')),
  constraint benefactor_marketing_vendor_costs_status_chk
    check (status in ('planned', 'approved', 'incurred', 'invoiced', 'paid', 'canceled')),
  constraint benefactor_marketing_vendor_costs_amount_chk
    check (amount_cents >= 0),
  constraint benefactor_marketing_vendor_costs_incurred_on_chk
    check (incurred_on is null or incurred_on ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint benefactor_marketing_vendor_costs_invoice_ref_size_chk
    check (invoice_ref is null or octet_length(invoice_ref) <= 120),
  constraint benefactor_marketing_vendor_costs_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists benefactor_marketing_vendor_costs_client_idx
  on benefactor_marketing_vendor_costs (client_id, status, updated_at desc)
  where client_id is not null;

-- Create missing table: benefactor_marketing_commission_entries
create table if not exists benefactor_marketing_commission_entries (
  id uuid primary key default gen_random_uuid(),
  client_id uuid,
  opportunity_id uuid,
  user_id uuid not null,
  status varchar(32) default 'pending' not null,
  commission_kind varchar(48) default 'deal' not null,
  basis_cents integer default 0 not null,
  rate_micros integer default 0 not null,
  amount_cents integer default 0 not null,
  earned_on varchar(10),
  paid_at timestamptz,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_commission_entries_status_chk
    check (status in ('pending', 'approved', 'paid', 'void')),
  constraint benefactor_marketing_commission_entries_kind_chk
    check (commission_kind in ('deal', 'retainer', 'renewal', 'upsell', 'appointment')),
  constraint benefactor_marketing_commission_entries_money_chk
    check (basis_cents >= 0 and amount_cents >= 0),
  constraint benefactor_marketing_commission_entries_rate_chk
    check (rate_micros between 0 and 1000000),
  constraint benefactor_marketing_commission_entries_earned_on_chk
    check (earned_on is null or earned_on ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint benefactor_marketing_commission_entries_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists benefactor_marketing_commission_entries_user_idx
  on benefactor_marketing_commission_entries (user_id, status, updated_at desc);

create index if not exists benefactor_marketing_commission_entries_client_idx
  on benefactor_marketing_commission_entries (client_id, status, updated_at desc)
  where client_id is not null;

-- Create missing table: benefactor_marketing_budget_forecasts
create table if not exists benefactor_marketing_budget_forecasts (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  campaign_id uuid,
  forecast_kind varchar(48) default 'monthly' not null,
  period_start varchar(10) not null,
  period_end varchar(10) not null,
  status varchar(32) default 'draft' not null,
  revenue_cents integer default 0 not null,
  media_spend_cents integer default 0 not null,
  labor_cost_cents integer default 0 not null,
  vendor_cost_cents integer default 0 not null,
  gross_margin_cents integer default 0 not null,
  assumptions jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_budget_forecasts_kind_chk
    check (forecast_kind in ('monthly', 'quarterly', 'campaign', 'annual')),
  constraint benefactor_marketing_budget_forecasts_period_start_chk
    check (period_start ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint benefactor_marketing_budget_forecasts_period_end_chk
    check (period_end ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint benefactor_marketing_budget_forecasts_status_chk
    check (status in ('draft', 'approved', 'locked', 'archived')),
  constraint benefactor_marketing_budget_forecasts_money_chk
    check (revenue_cents >= 0 and media_spend_cents >= 0 and labor_cost_cents >= 0 and vendor_cost_cents >= 0),
  constraint benefactor_marketing_budget_forecasts_assumptions_object_chk
    check (jsonb_typeof(assumptions) = 'object')
);

create index if not exists benefactor_marketing_budget_forecasts_client_period_idx
  on benefactor_marketing_budget_forecasts (client_id, period_start desc, status);

-- Create missing table: benefactor_marketing_call_insights
create table if not exists benefactor_marketing_call_insights (
  id uuid primary key default gen_random_uuid(),
  client_id uuid not null,
  meeting_id uuid,
  lead_id uuid,
  opportunity_id uuid,
  status varchar(32) default 'ready' not null,
  provider varchar(64),
  transcript_uri text,
  summary text,
  sentiment varchar(32),
  action_items jsonb default '[]'::jsonb not null,
  objections jsonb default '[]'::jsonb not null,
  next_steps jsonb default '[]'::jsonb not null,
  confidence_micros integer default 0 not null,
  analyzed_at timestamptz default now() not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint benefactor_marketing_call_insights_status_chk
    check (status in ('processing', 'ready', 'failed', 'archived')),
  constraint benefactor_marketing_call_insights_provider_size_chk
    check (provider is null or octet_length(provider) <= 64),
  constraint benefactor_marketing_call_insights_transcript_uri_size_chk
    check (transcript_uri is null or octet_length(transcript_uri) <= 2048),
  constraint benefactor_marketing_call_insights_summary_size_chk
    check (summary is null or octet_length(summary) <= 20000),
  constraint benefactor_marketing_call_insights_sentiment_chk
    check (sentiment is null or sentiment in ('positive', 'neutral', 'negative', 'mixed')),
  constraint benefactor_marketing_call_insights_action_items_array_chk
    check (jsonb_typeof(action_items) = 'array'),
  constraint benefactor_marketing_call_insights_objections_array_chk
    check (jsonb_typeof(objections) = 'array'),
  constraint benefactor_marketing_call_insights_next_steps_array_chk
    check (jsonb_typeof(next_steps) = 'array'),
  constraint benefactor_marketing_call_insights_confidence_chk
    check (confidence_micros between 0 and 1000000)
);

create index if not exists benefactor_marketing_call_insights_client_idx
  on benefactor_marketing_call_insights (client_id, analyzed_at desc);

-- Create missing table: usacc_users
create table if not exists usacc_users (
  id uuid primary key default gen_random_uuid(),
  external_subject varchar(240),
  email_hash varchar(64),
  display_name varchar(200) not null,
  user_kind varchar(48) default 'natural_person' not null,
  status varchar(32) default 'active' not null,
  kyc_level varchar(32) default 'none' not null,
  roles jsonb default '{}'::jsonb not null,
  is_legal_entity boolean default false not null,
  legal_region varchar(64),
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint usacc_users_external_subject_size_chk
    check (external_subject is null or octet_length(external_subject) between 1 and 240),
  constraint usacc_users_email_hash_chk
    check (email_hash is null or email_hash ~ '^[a-f0-9]{64}$'),
  constraint usacc_users_display_name_size_chk
    check (octet_length(display_name) between 1 and 200),
  constraint usacc_users_kind_chk
    check (user_kind in ('natural_person', 'legal_entity', 'service_account', 'sim_agent')),
  constraint usacc_users_status_chk
    check (status in ('active', 'pending', 'suspended', 'banned', 'alumni', 'archived')),
  constraint usacc_users_kyc_level_chk
    check (kyc_level in ('none', 'light', 'medium', 'high')),
  constraint usacc_users_legal_region_format_chk
    check (legal_region is null or legal_region ~ '^[A-Za-z0-9._:/-]{1,64}$'),
  constraint usacc_users_roles_object_chk
    check (jsonb_typeof(roles) = 'object'),
  constraint usacc_users_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists usacc_users_external_subject_uq
  on usacc_users (external_subject)
  where external_subject is not null;

create unique index if not exists usacc_users_email_hash_uq
  on usacc_users (email_hash)
  where email_hash is not null;

create index if not exists usacc_users_status_updated_at_idx
  on usacc_users (status, updated_at desc);

create index if not exists usacc_users_roles_gin_idx
  on usacc_users using gin (roles);

-- Create missing table: usacc_cases
create table if not exists usacc_cases (
  id uuid primary key default gen_random_uuid(),
  case_number varchar(80) not null,
  title varchar(240) not null,
  status varchar(40) default 'draft' not null,
  filing_tier varchar(40) default 'screen' not null,
  plaintiff_user_id uuid,
  defendant_summary text not null,
  conduct_summary text not null,
  conduct_fingerprint varchar(128),
  conduct_window_start varchar(10),
  conduct_window_end varchar(10),
  priority_score_micros integer default 0 not null,
  meta_data jsonb default '{}'::jsonb not null,
  opened_at timestamptz,
  closed_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint usacc_cases_case_number_format_chk
    check (case_number ~ '^[A-Za-z0-9._:/-]{1,80}$'),
  constraint usacc_cases_title_size_chk
    check (octet_length(title) between 1 and 240),
  constraint usacc_cases_status_chk
    check (status in ('draft', 'signature_collection', 'screening', 'inquiry', 'admission_review', 'trial', 'appeal', 'resolved', 'canceled', 'archived')),
  constraint usacc_cases_filing_tier_chk
    check (filing_tier in ('screen', 'inquiry', 'trial_1', 'trial_2', 'trial_3', 'trial_5', 'trial_10')),
  constraint usacc_cases_defendant_summary_size_chk
    check (octet_length(defendant_summary) between 1 and 4000),
  constraint usacc_cases_conduct_summary_size_chk
    check (octet_length(conduct_summary) between 1 and 12000),
  constraint usacc_cases_conduct_fingerprint_chk
    check (conduct_fingerprint is null or conduct_fingerprint ~ '^[A-Za-z0-9._:/-]{1,128}$'),
  constraint usacc_cases_conduct_window_start_chk
    check (conduct_window_start is null or conduct_window_start ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint usacc_cases_conduct_window_end_chk
    check (conduct_window_end is null or conduct_window_end ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}$'),
  constraint usacc_cases_priority_score_chk
    check (priority_score_micros between 0 and 1000000),
  constraint usacc_cases_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists usacc_cases_case_number_uq
  on usacc_cases (case_number);

create index if not exists usacc_cases_status_updated_at_idx
  on usacc_cases (status, updated_at desc);

create index if not exists usacc_cases_plaintiff_idx
  on usacc_cases (plaintiff_user_id, created_at desc)
  where plaintiff_user_id is not null;

-- Create missing table: usacc_case_participants
create table if not exists usacc_case_participants (
  id uuid primary key default gen_random_uuid(),
  case_id uuid not null,
  user_id uuid not null,
  role varchar(48) not null,
  status varchar(32) default 'active' not null,
  granted_by uuid,
  granted_by_policy_version varchar(120),
  ended_at timestamptz,
  ended_reason varchar(240),
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint usacc_case_participants_role_chk
    check (role in ('plaintiff', 'defendant', 'sponsor', 'witness', 'judge', 'panel_juror', 'appeal_judge', 'presiding_juror', 'paralegal', 'investigator', 'intake_reviewer', 'clerk_of_court', 'compliance_monitor', 'counsel', 'oversight_board', 'auditor', 'ombuds')),
  constraint usacc_case_participants_status_chk
    check (status in ('active', 'pending', 'declined', 'suspended', 'ended', 'banned')),
  constraint usacc_case_participants_policy_version_chk
    check (granted_by_policy_version is null or granted_by_policy_version ~ '^[A-Za-z0-9._:/-]{1,120}$'),
  constraint usacc_case_participants_ended_reason_size_chk
    check (ended_reason is null or octet_length(ended_reason) <= 240),
  constraint usacc_case_participants_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists usacc_case_participants_case_user_role_uq
  on usacc_case_participants (case_id, user_id, role);

create index if not exists usacc_case_participants_user_idx
  on usacc_case_participants (user_id, status, updated_at desc);

create index if not exists usacc_case_participants_case_role_idx
  on usacc_case_participants (case_id, role, status);

-- Create missing table: usacc_case_stages
create table if not exists usacc_case_stages (
  id uuid primary key default gen_random_uuid(),
  case_id uuid not null,
  stage_key varchar(64) not null,
  stage_order integer not null,
  title varchar(200) not null,
  status varchar(32) default 'pending' not null,
  assigned_user_id uuid,
  opened_at timestamptz,
  due_at timestamptz,
  closed_at timestamptz,
  decision_summary text,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint usacc_case_stages_stage_key_format_chk
    check (stage_key ~ '^[A-Za-z0-9._:/-]{1,64}$'),
  constraint usacc_case_stages_stage_order_chk
    check (stage_order between 0 and 1000),
  constraint usacc_case_stages_title_size_chk
    check (octet_length(title) between 1 and 200),
  constraint usacc_case_stages_status_chk
    check (status in ('pending', 'open', 'blocked', 'complete', 'skipped', 'canceled')),
  constraint usacc_case_stages_decision_summary_size_chk
    check (decision_summary is null or octet_length(decision_summary) <= 12000),
  constraint usacc_case_stages_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists usacc_case_stages_case_stage_key_uq
  on usacc_case_stages (case_id, stage_key);

create index if not exists usacc_case_stages_case_order_idx
  on usacc_case_stages (case_id, stage_order);

-- Create missing table: usacc_elections
create table if not exists usacc_elections (
  id uuid primary key default gen_random_uuid(),
  case_id uuid,
  stage_id uuid,
  election_kind varchar(48) not null,
  title varchar(220) not null,
  status varchar(32) default 'draft' not null,
  quorum_count integer default 1 not null,
  threshold_micros integer default 500000 not null,
  opens_at timestamptz,
  closes_at timestamptz,
  sealed_until timestamptz,
  tally jsonb default '{}'::jsonb not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint usacc_elections_kind_chk
    check (election_kind in ('priority', 'admission', 'panel_verdict', 'appeal', 'oversight', 'policy', 'assignment_acceptance')),
  constraint usacc_elections_title_size_chk
    check (octet_length(title) between 1 and 220),
  constraint usacc_elections_status_chk
    check (status in ('draft', 'open', 'sealed', 'tallying', 'certified', 'void', 'archived')),
  constraint usacc_elections_quorum_chk
    check (quorum_count between 1 and 1000000),
  constraint usacc_elections_threshold_chk
    check (threshold_micros between 1 and 1000000),
  constraint usacc_elections_tally_object_chk
    check (jsonb_typeof(tally) = 'object'),
  constraint usacc_elections_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists usacc_elections_case_status_idx
  on usacc_elections (case_id, status, updated_at desc)
  where case_id is not null;

create index if not exists usacc_elections_stage_idx
  on usacc_elections (stage_id, created_at desc)
  where stage_id is not null;

-- Create missing table: usacc_votes
create table if not exists usacc_votes (
  id uuid primary key default gen_random_uuid(),
  election_id uuid not null,
  case_id uuid,
  voter_user_id uuid not null,
  vote_kind varchar(48) default 'choice' not null,
  vote_value varchar(80) not null,
  weight_micros integer default 1000000 not null,
  commitment_hash varchar(128),
  sealed_payload jsonb,
  revealed_at timestamptz,
  contract_digest varchar(160),
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint usacc_votes_kind_chk
    check (vote_kind in ('choice', 'priority_dollar_weighted', 'verdict', 'approval', 'assignment_response')),
  constraint usacc_votes_vote_value_format_chk
    check (vote_value ~ '^[A-Za-z0-9._:/-]{1,80}$'),
  constraint usacc_votes_weight_chk
    check (weight_micros between 0 and 1000000000),
  constraint usacc_votes_commitment_hash_chk
    check (commitment_hash is null or commitment_hash ~ '^[A-Za-z0-9._:/-]{1,128}$'),
  constraint usacc_votes_sealed_payload_object_chk
    check (sealed_payload is null or jsonb_typeof(sealed_payload) = 'object'),
  constraint usacc_votes_contract_digest_size_chk
    check (contract_digest is null or octet_length(contract_digest) <= 160),
  constraint usacc_votes_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists usacc_votes_election_voter_uq
  on usacc_votes (election_id, voter_user_id);

create index if not exists usacc_votes_case_idx
  on usacc_votes (case_id, created_at desc)
  where case_id is not null;

create index if not exists usacc_votes_voter_idx
  on usacc_votes (voter_user_id, created_at desc);

-- Create missing table: usacc_escrow_accounts
create table if not exists usacc_escrow_accounts (
  id uuid primary key default gen_random_uuid(),
  case_id uuid not null,
  status varchar(32) default 'pending' not null,
  provider varchar(48) default 'stripe_treasury' not null,
  provider_account_ref varchar(240),
  currency varchar(12) default 'USD' not null,
  target_amount_cents bigint default 0 not null,
  committed_amount_cents bigint default 0 not null,
  captured_amount_cents bigint default 0 not null,
  disbursed_amount_cents bigint default 0 not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint usacc_escrow_accounts_status_chk
    check (status in ('pending', 'open', 'funding', 'locked', 'disbursing', 'closed', 'canceled')),
  constraint usacc_escrow_accounts_provider_chk
    check (provider in ('stripe_treasury', 'stripe_connect', 'column', 'evolve', 'mercury', 'trust_company', 'manual')),
  constraint usacc_escrow_accounts_provider_ref_size_chk
    check (provider_account_ref is null or octet_length(provider_account_ref) <= 240),
  constraint usacc_escrow_accounts_currency_chk
    check (currency ~ '^[A-Z]{3,12}$'),
  constraint usacc_escrow_accounts_money_chk
    check (target_amount_cents >= 0 and committed_amount_cents >= 0 and captured_amount_cents >= 0 and disbursed_amount_cents >= 0),
  constraint usacc_escrow_accounts_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists usacc_escrow_accounts_case_provider_uq
  on usacc_escrow_accounts (case_id, provider);

-- Create missing table: usacc_ledger_entries
create table if not exists usacc_ledger_entries (
  id uuid primary key default gen_random_uuid(),
  case_id uuid,
  escrow_account_id uuid,
  user_id uuid,
  entry_kind varchar(48) not null,
  direction varchar(16) not null,
  amount_cents bigint not null,
  currency varchar(12) default 'USD' not null,
  provider_ref varchar(240),
  contract_digest varchar(160),
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  constraint usacc_ledger_entries_kind_chk
    check (entry_kind in ('pledge', 'authorization', 'capture', 'refund', 'disbursement', 'fee', 'adjustment')),
  constraint usacc_ledger_entries_direction_chk
    check (direction in ('debit', 'credit')),
  constraint usacc_ledger_entries_amount_chk
    check (amount_cents >= 0),
  constraint usacc_ledger_entries_currency_chk
    check (currency ~ '^[A-Z]{3,12}$'),
  constraint usacc_ledger_entries_provider_ref_size_chk
    check (provider_ref is null or octet_length(provider_ref) <= 240),
  constraint usacc_ledger_entries_contract_digest_size_chk
    check (contract_digest is null or octet_length(contract_digest) <= 160),
  constraint usacc_ledger_entries_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists usacc_ledger_entries_case_created_idx
  on usacc_ledger_entries (case_id, created_at desc)
  where case_id is not null;

create index if not exists usacc_ledger_entries_user_created_idx
  on usacc_ledger_entries (user_id, created_at desc)
  where user_id is not null;

-- Create missing table: usacc_contract_operations
create table if not exists usacc_contract_operations (
  id uuid primary key default gen_random_uuid(),
  case_id uuid,
  election_id uuid,
  vote_id uuid,
  request_id varchar(160) not null,
  operation_kind varchar(48) not null,
  status varchar(32) default 'pending' not null,
  program_id varchar(128),
  digest varchar(160),
  envelope jsonb default '{}'::jsonb not null,
  response jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint usacc_contract_operations_request_id_size_chk
    check (octet_length(request_id) between 1 and 160),
  constraint usacc_contract_operations_kind_chk
    check (operation_kind in ('validate_envelope', 'simulate_transaction', 'send_transaction', 'vote_commitment', 'escrow_notary')),
  constraint usacc_contract_operations_status_chk
    check (status in ('pending', 'validated', 'simulated', 'sent', 'failed', 'canceled')),
  constraint usacc_contract_operations_program_id_size_chk
    check (program_id is null or octet_length(program_id) <= 128),
  constraint usacc_contract_operations_digest_size_chk
    check (digest is null or octet_length(digest) <= 160),
  constraint usacc_contract_operations_envelope_object_chk
    check (jsonb_typeof(envelope) = 'object'),
  constraint usacc_contract_operations_response_object_chk
    check (jsonb_typeof(response) = 'object')
);

create unique index if not exists usacc_contract_operations_request_id_uq
  on usacc_contract_operations (request_id);

create index if not exists usacc_contract_operations_case_idx
  on usacc_contract_operations (case_id, created_at desc)
  where case_id is not null;

-- Create missing table: usacc_simulation_runs
create table if not exists usacc_simulation_runs (
  id uuid primary key default gen_random_uuid(),
  case_id uuid,
  status varchar(32) default 'queued' not null,
  mode varchar(32) default 'sim' not null,
  seed bigint not null,
  horizon_days integer default 180 not null,
  actor_count integer default 0 not null,
  event_count integer default 0 not null,
  metrics jsonb default '{}'::jsonb not null,
  trace jsonb default '[]'::jsonb not null,
  input jsonb default '{}'::jsonb not null,
  started_at timestamptz,
  finished_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint usacc_simulation_runs_status_chk
    check (status in ('queued', 'running', 'succeeded', 'failed', 'canceled')),
  constraint usacc_simulation_runs_mode_chk
    check (mode in ('sim', 'live_shadow', 'replay')),
  constraint usacc_simulation_runs_horizon_chk
    check (horizon_days between 1 and 3650),
  constraint usacc_simulation_runs_counts_chk
    check (actor_count >= 0 and event_count >= 0),
  constraint usacc_simulation_runs_metrics_object_chk
    check (jsonb_typeof(metrics) = 'object'),
  constraint usacc_simulation_runs_trace_array_chk
    check (jsonb_typeof(trace) = 'array'),
  constraint usacc_simulation_runs_input_object_chk
    check (jsonb_typeof(input) = 'object')
);

create index if not exists usacc_simulation_runs_case_created_idx
  on usacc_simulation_runs (case_id, created_at desc)
  where case_id is not null;

create index if not exists usacc_simulation_runs_status_created_idx
  on usacc_simulation_runs (status, created_at desc);

-- Create missing table: usacc_audit_events
create table if not exists usacc_audit_events (
  id uuid primary key default gen_random_uuid(),
  case_id uuid,
  actor_user_id uuid,
  event_type varchar(96) not null,
  event_hash varchar(128) not null,
  source varchar(80) default 'usacc-rest-api-backend-rs' not null,
  payload jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  constraint usacc_audit_events_type_format_chk
    check (event_type ~ '^[A-Za-z0-9._:/-]{1,96}$'),
  constraint usacc_audit_events_hash_format_chk
    check (event_hash ~ '^[A-Za-z0-9._:/-]{1,128}$'),
  constraint usacc_audit_events_source_format_chk
    check (source ~ '^[A-Za-z0-9._:/-]{1,80}$'),
  constraint usacc_audit_events_payload_object_chk
    check (jsonb_typeof(payload) = 'object')
);

create unique index if not exists usacc_audit_events_hash_uq
  on usacc_audit_events (event_hash);

create index if not exists usacc_audit_events_case_created_idx
  on usacc_audit_events (case_id, created_at desc)
  where case_id is not null;

-- Create missing table: vcs_repositories
create table if not exists vcs_repositories (
  id uuid primary key default gen_random_uuid(),
  slug varchar(120) not null,
  display_name varchar(200) not null,
  vcs_kind varchar(20) default 'git' not null,
  remote_url text not null,
  default_branch varchar(160) default 'main' not null,
  mirror_path text,
  mirror_status varchar(32) default 'pending' not null,
  visibility varchar(20) default 'private' not null,
  last_synced_at timestamptz,
  last_error text,
  size_bytes bigint default 0 not null,
  ref_count integer default 0 not null,
  meta_data jsonb default '{}'::jsonb not null,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint vcs_repositories_slug_format_chk
    check (slug ~ '^[a-z0-9][a-z0-9._-]{0,119}$'),
  constraint vcs_repositories_display_name_size_chk
    check (octet_length(display_name) <= 200),
  constraint vcs_repositories_remote_url_size_chk
    check (octet_length(remote_url) <= 2048),
  constraint vcs_repositories_default_branch_format_chk
    check (default_branch ~ '^[A-Za-z0-9._/-]{1,160}$'),
  constraint vcs_repositories_vcs_kind_chk
    check (vcs_kind in ('git', 'hg', 'svn', 'fossil')),
  constraint vcs_repositories_mirror_status_chk
    check (mirror_status in ('pending', 'mirroring', 'ready', 'error', 'disabled')),
  constraint vcs_repositories_visibility_chk
    check (visibility in ('private', 'internal', 'public')),
  constraint vcs_repositories_size_bytes_chk
    check (size_bytes >= 0),
  constraint vcs_repositories_ref_count_chk
    check (ref_count >= 0),
  constraint vcs_repositories_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists vcs_repositories_slug_active_uq
  on vcs_repositories (slug)
  where is_soft_deleted = false;

create index if not exists vcs_repositories_vcs_kind_idx
  on vcs_repositories (vcs_kind)
  where is_soft_deleted = false;

create index if not exists vcs_repositories_mirror_status_idx
  on vcs_repositories (mirror_status)
  where is_soft_deleted = false;

create index if not exists vcs_repositories_updated_at_idx
  on vcs_repositories (updated_at desc)
  where is_soft_deleted = false;

-- Create missing table: vcs_refs
create table if not exists vcs_refs (
  id uuid primary key default gen_random_uuid(),
  repository_id uuid not null,
  ref_name varchar(255) not null,
  ref_type varchar(20) default 'branch' not null,
  target_revision varchar(120) not null,
  is_default boolean default false not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint vcs_refs_ref_name_size_chk
    check (octet_length(ref_name) <= 255),
  constraint vcs_refs_ref_type_chk
    check (ref_type in ('branch', 'tag', 'bookmark', 'head', 'other')),
  constraint vcs_refs_target_revision_size_chk
    check (octet_length(target_revision) between 1 and 120),
  constraint vcs_refs_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists vcs_refs_repo_name_uq
  on vcs_refs (repository_id, ref_name);

create index if not exists vcs_refs_repository_id_idx
  on vcs_refs (repository_id);

-- Create missing table: vcs_operations
create table if not exists vcs_operations (
  id uuid primary key default gen_random_uuid(),
  repository_id uuid,
  vcs_kind varchar(20) default 'git' not null,
  op_type varchar(32) not null,
  status varchar(20) default 'pending' not null,
  params jsonb default '{}'::jsonb not null,
  result_summary jsonb default '{}'::jsonb not null,
  error text,
  duration_ms integer,
  requested_by varchar(200),
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint vcs_operations_vcs_kind_chk
    check (vcs_kind in ('git', 'hg', 'svn', 'fossil')),
  constraint vcs_operations_op_type_chk
    check (op_type in ('mirror', 'fetch', 'refs', 'log', 'show', 'diff', 'tree', 'blob', 'probe', 'remove')),
  constraint vcs_operations_status_chk
    check (status in ('pending', 'running', 'success', 'error')),
  constraint vcs_operations_duration_chk
    check (duration_ms is null or duration_ms >= 0),
  constraint vcs_operations_params_object_chk
    check (jsonb_typeof(params) = 'object'),
  constraint vcs_operations_result_object_chk
    check (jsonb_typeof(result_summary) = 'object')
);

create index if not exists vcs_operations_repository_id_idx
  on vcs_operations (repository_id);

create index if not exists vcs_operations_op_type_idx
  on vcs_operations (op_type);

create index if not exists vcs_operations_created_at_idx
  on vcs_operations (created_at desc);

-- =============================================================
-- Missing foreign-key supporting indexes (56)
-- Derived from foreign keys, not declared in schema.sql. Idempotent.
-- =============================================================

-- sound_recorder_segments.device_id (sound_recorder_segments_device_fk)
create index if not exists sound_recorder_segments_device_id_fk_idx on sound_recorder_segments (device_id);

-- sound_recorder_evidence_exports.device_id (sound_recorder_evidence_exports_device_fk)
create index if not exists sound_recorder_evidence_exports_device_id_fk_idx on sound_recorder_evidence_exports (device_id);

-- sound_recorder_evidence_exports.created_by_device_id (sound_recorder_evidence_exports_created_by_device_fk)
create index if not exists sound_recorder_evidence_exports_created_by_device_id_fk_idx on sound_recorder_evidence_exports (created_by_device_id);

-- sound_recorder_audit_events.device_id (sound_recorder_audit_events_device_fk)
create index if not exists sound_recorder_audit_events_device_id_fk_idx on sound_recorder_audit_events (device_id);

-- sound_recorder_oauth_states.device_id (sound_recorder_oauth_states_device_fk)
create index if not exists sound_recorder_oauth_states_device_id_fk_idx on sound_recorder_oauth_states (device_id);

-- sound_recorder_cloud_connections.created_by_device_id (sound_recorder_cloud_connections_created_by_device_fk)
create index if not exists sound_recorder_cloud_connections_created_by_device_id_fk_idx on sound_recorder_cloud_connections (created_by_device_id);

-- mip_solver_events.job_id (mip_solver_events_job_fk)
create index if not exists mip_solver_events_job_id_fk_idx on mip_solver_events (job_id);

-- benefactor_marketing_contracts.package_id (benefactor_marketing_contracts_package_fk)
create index if not exists benefactor_marketing_contracts_package_id_fk_idx on benefactor_marketing_contracts (package_id);

-- benefactor_marketing_invoices.contract_id (benefactor_marketing_invoices_contract_fk)
create index if not exists benefactor_marketing_invoices_contract_id_fk_idx on benefactor_marketing_invoices (contract_id);

-- benefactor_marketing_leads.source_integration_id (benefactor_marketing_leads_source_integration_fk)
create index if not exists benefactor_marketing_leads_source_integration_id_fk_idx on benefactor_marketing_leads (source_integration_id);

-- benefactor_marketing_enrichment_jobs.lead_id (benefactor_marketing_enrichment_jobs_lead_fk)
create index if not exists benefactor_marketing_enrichment_jobs_lead_id_fk_idx on benefactor_marketing_enrichment_jobs (lead_id);

-- benefactor_marketing_automation_events.workflow_id (benefactor_marketing_automation_events_workflow_fk)
create index if not exists benefactor_marketing_automation_events_workflow_id_fk_idx on benefactor_marketing_automation_events (workflow_id);

-- benefactor_marketing_automation_events.lead_id (benefactor_marketing_automation_events_lead_fk)
create index if not exists benefactor_marketing_automation_events_lead_id_fk_idx on benefactor_marketing_automation_events (lead_id);

-- benefactor_marketing_reports.campaign_id (benefactor_marketing_reports_campaign_fk)
create index if not exists benefactor_marketing_reports_campaign_id_fk_idx on benefactor_marketing_reports (campaign_id);

-- benefactor_marketing_attribution_events.campaign_id (benefactor_marketing_attribution_events_campaign_fk)
create index if not exists benefactor_marketing_attribution_events_campaign_id_fk_idx on benefactor_marketing_attribution_events (campaign_id);

-- benefactor_marketing_attribution_events.lead_id (benefactor_marketing_attribution_events_lead_fk)
create index if not exists benefactor_marketing_attribution_events_lead_id_fk_idx on benefactor_marketing_attribution_events (lead_id);

-- benefactor_marketing_opportunities.lead_id (benefactor_marketing_opportunities_lead_fk)
create index if not exists benefactor_marketing_opportunities_lead_id_fk_idx on benefactor_marketing_opportunities (lead_id);

-- benefactor_marketing_content_assets.campaign_id (benefactor_marketing_content_assets_campaign_fk)
create index if not exists benefactor_marketing_content_assets_campaign_id_fk_idx on benefactor_marketing_content_assets (campaign_id);

-- benefactor_marketing_project_tasks.campaign_id (benefactor_marketing_project_tasks_campaign_fk)
create index if not exists benefactor_marketing_project_tasks_campaign_id_fk_idx on benefactor_marketing_project_tasks (campaign_id);

-- benefactor_marketing_project_tasks.content_asset_id (benefactor_marketing_project_tasks_content_asset_fk)
create index if not exists benefactor_marketing_project_tasks_content_asset_id_fk_idx on benefactor_marketing_project_tasks (content_asset_id);

-- benefactor_marketing_client_approvals.campaign_id (benefactor_marketing_client_approvals_campaign_fk)
create index if not exists benefactor_marketing_client_approvals_campaign_id_fk_idx on benefactor_marketing_client_approvals (campaign_id);

-- benefactor_marketing_client_approvals.content_asset_id (benefactor_marketing_client_approvals_content_asset_fk)
create index if not exists benefactor_marketing_client_approvals_content_asset_id_fk_idx on benefactor_marketing_client_approvals (content_asset_id);

-- benefactor_marketing_meetings.lead_id (benefactor_marketing_meetings_lead_fk)
create index if not exists benefactor_marketing_meetings_lead_id_fk_idx on benefactor_marketing_meetings (lead_id);

-- benefactor_marketing_meetings.opportunity_id (benefactor_marketing_meetings_opportunity_fk)
create index if not exists benefactor_marketing_meetings_opportunity_id_fk_idx on benefactor_marketing_meetings (opportunity_id);

-- benefactor_marketing_team_allocations.campaign_id (benefactor_marketing_team_allocations_campaign_fk)
create index if not exists benefactor_marketing_team_allocations_campaign_id_fk_idx on benefactor_marketing_team_allocations (campaign_id);

-- benefactor_marketing_outreach_sequences.campaign_id (benefactor_marketing_outreach_sequences_campaign_fk)
create index if not exists benefactor_marketing_outreach_sequences_campaign_id_fk_idx on benefactor_marketing_outreach_sequences (campaign_id);

-- benefactor_marketing_outreach_enrollments.lead_id (benefactor_marketing_outreach_enrollments_lead_fk)
create index if not exists benefactor_marketing_outreach_enrollments_lead_id_fk_idx on benefactor_marketing_outreach_enrollments (lead_id);

-- benefactor_marketing_outreach_enrollments.contact_id (benefactor_marketing_outreach_enrollments_contact_fk)
create index if not exists benefactor_marketing_outreach_enrollments_contact_id_fk_idx on benefactor_marketing_outreach_enrollments (contact_id);

-- benefactor_marketing_outreach_touchpoints.sequence_id (benefactor_marketing_outreach_touchpoints_sequence_fk)
create index if not exists benefactor_marketing_outreach_touchpoints_sequence_id_fk_idx on benefactor_marketing_outreach_touchpoints (sequence_id);

-- benefactor_marketing_outreach_touchpoints.enrollment_id (benefactor_marketing_outreach_touchpoints_enrollment_fk)
create index if not exists benefactor_marketing_outreach_touchpoints_enrollment_id_fk_idx on benefactor_marketing_outreach_touchpoints (enrollment_id);

-- benefactor_marketing_outreach_touchpoints.campaign_id (benefactor_marketing_outreach_touchpoints_campaign_fk)
create index if not exists benefactor_marketing_outreach_touchpoints_campaign_id_fk_idx on benefactor_marketing_outreach_touchpoints (campaign_id);

-- benefactor_marketing_outreach_touchpoints.lead_id (benefactor_marketing_outreach_touchpoints_lead_fk)
create index if not exists benefactor_marketing_outreach_touchpoints_lead_id_fk_idx on benefactor_marketing_outreach_touchpoints (lead_id);

-- benefactor_marketing_outreach_touchpoints.contact_id (benefactor_marketing_outreach_touchpoints_contact_fk)
create index if not exists benefactor_marketing_outreach_touchpoints_contact_id_fk_idx on benefactor_marketing_outreach_touchpoints (contact_id);

-- benefactor_marketing_conversion_events.campaign_id (benefactor_marketing_conversion_events_campaign_fk)
create index if not exists benefactor_marketing_conversion_events_campaign_id_fk_idx on benefactor_marketing_conversion_events (campaign_id);

-- benefactor_marketing_conversion_events.lead_id (benefactor_marketing_conversion_events_lead_fk)
create index if not exists benefactor_marketing_conversion_events_lead_id_fk_idx on benefactor_marketing_conversion_events (lead_id);

-- benefactor_marketing_conversion_events.content_asset_id (benefactor_marketing_conversion_events_content_asset_fk)
create index if not exists benefactor_marketing_conversion_events_content_asset_id_fk_idx on benefactor_marketing_conversion_events (content_asset_id);

-- benefactor_marketing_portal_members.contact_id (benefactor_marketing_portal_members_contact_fk)
create index if not exists benefactor_marketing_portal_members_contact_id_fk_idx on benefactor_marketing_portal_members (contact_id);

-- benefactor_marketing_shared_documents.campaign_id (benefactor_marketing_shared_documents_campaign_fk)
create index if not exists benefactor_marketing_shared_documents_campaign_id_fk_idx on benefactor_marketing_shared_documents (campaign_id);

-- benefactor_marketing_shared_documents.content_asset_id (benefactor_marketing_shared_documents_content_asset_fk)
create index if not exists benefactor_marketing_shared_documents_content_asset_id_fk_idx on benefactor_marketing_shared_documents (content_asset_id);

-- benefactor_marketing_collaboration_comments.parent_comment_id (benefactor_marketing_collaboration_comments_parent_fk)
create index if not exists benefactor_marketing_collaboration_comments_parent_comment_id_f on benefactor_marketing_collaboration_comments (parent_comment_id);

-- benefactor_marketing_collaboration_comments.author_contact_id (benefactor_marketing_collaboration_comments_author_contact_fk)
create index if not exists benefactor_marketing_collaboration_comments_author_contact_id_f on benefactor_marketing_collaboration_comments (author_contact_id);

-- benefactor_marketing_notifications.recipient_contact_id (benefactor_marketing_notifications_contact_fk)
create index if not exists benefactor_marketing_notifications_recipient_contact_id_fk_idx on benefactor_marketing_notifications (recipient_contact_id);

-- benefactor_marketing_time_entries.campaign_id (benefactor_marketing_time_entries_campaign_fk)
create index if not exists benefactor_marketing_time_entries_campaign_id_fk_idx on benefactor_marketing_time_entries (campaign_id);

-- benefactor_marketing_time_entries.project_task_id (benefactor_marketing_time_entries_task_fk)
create index if not exists benefactor_marketing_time_entries_project_task_id_fk_idx on benefactor_marketing_time_entries (project_task_id);

-- benefactor_marketing_vendor_costs.campaign_id (benefactor_marketing_vendor_costs_campaign_fk)
create index if not exists benefactor_marketing_vendor_costs_campaign_id_fk_idx on benefactor_marketing_vendor_costs (campaign_id);

-- benefactor_marketing_commission_entries.opportunity_id (benefactor_marketing_commission_entries_opportunity_fk)
create index if not exists benefactor_marketing_commission_entries_opportunity_id_fk_idx on benefactor_marketing_commission_entries (opportunity_id);

-- benefactor_marketing_budget_forecasts.campaign_id (benefactor_marketing_budget_forecasts_campaign_fk)
create index if not exists benefactor_marketing_budget_forecasts_campaign_id_fk_idx on benefactor_marketing_budget_forecasts (campaign_id);

-- benefactor_marketing_call_insights.meeting_id (benefactor_marketing_call_insights_meeting_fk)
create index if not exists benefactor_marketing_call_insights_meeting_id_fk_idx on benefactor_marketing_call_insights (meeting_id);

-- benefactor_marketing_call_insights.lead_id (benefactor_marketing_call_insights_lead_fk)
create index if not exists benefactor_marketing_call_insights_lead_id_fk_idx on benefactor_marketing_call_insights (lead_id);

-- benefactor_marketing_call_insights.opportunity_id (benefactor_marketing_call_insights_opportunity_fk)
create index if not exists benefactor_marketing_call_insights_opportunity_id_fk_idx on benefactor_marketing_call_insights (opportunity_id);

-- usacc_case_participants.granted_by (usacc_case_participants_granted_by_fk)
create index if not exists usacc_case_participants_granted_by_fk_idx on usacc_case_participants (granted_by);

-- usacc_case_stages.assigned_user_id (usacc_case_stages_assigned_user_fk)
create index if not exists usacc_case_stages_assigned_user_id_fk_idx on usacc_case_stages (assigned_user_id);

-- usacc_ledger_entries.escrow_account_id (usacc_ledger_entries_escrow_fk)
create index if not exists usacc_ledger_entries_escrow_account_id_fk_idx on usacc_ledger_entries (escrow_account_id);

-- usacc_contract_operations.election_id (usacc_contract_operations_election_fk)
create index if not exists usacc_contract_operations_election_id_fk_idx on usacc_contract_operations (election_id);

-- usacc_contract_operations.vote_id (usacc_contract_operations_vote_fk)
create index if not exists usacc_contract_operations_vote_id_fk_idx on usacc_contract_operations (vote_id);

-- usacc_audit_events.actor_user_id (usacc_audit_events_actor_fk)
create index if not exists usacc_audit_events_actor_user_id_fk_idx on usacc_audit_events (actor_user_id);

COMMIT;

-- Change items emitted: 148
