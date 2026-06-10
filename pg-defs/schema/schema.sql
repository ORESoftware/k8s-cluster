-- Canonical remote Postgres schema source for pg-defs.
-- This file is the desired-state contract used by the remote migration diff generator.
-- Do not apply it directly to a shared database; generate and review a diff instead.

create table if not exists app_config (
  id uuid primary key default gen_random_uuid(),
  scope varchar(120) default 'default' not null,
  key varchar(200) not null,
  value jsonb not null,
  version integer default 1 not null,
  status varchar(32) default 'active' not null,
  labels jsonb default '[]'::jsonb not null,
  meta_data jsonb default '{}'::jsonb not null,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint app_config_scope_format_chk
    check (scope ~ '^[A-Za-z0-9._/-]{1,120}$'),
  constraint app_config_key_format_chk
    check (key ~ '^[A-Za-z0-9._:/-]{1,200}$'),
  constraint app_config_value_object_chk
    check (jsonb_typeof(value) = 'object'),
  constraint app_config_labels_array_chk
    check (jsonb_typeof(labels) = 'array'),
  constraint app_config_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object'),
  constraint app_config_version_chk
    check (version > 0),
  constraint app_config_status_chk
    check (status in ('active', 'paused', 'archived'))
);

create unique index if not exists app_config_scope_key_uq
  on app_config (scope, key);

create index if not exists app_config_status_idx
  on app_config (status)
  where is_soft_deleted = false;

create index if not exists app_config_updated_at_idx
  on app_config (updated_at desc)
  where is_soft_deleted = false;

create index if not exists app_config_labels_gin_idx
  on app_config using gin (labels);

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

alter table if exists music_song_votes
  add constraint music_song_votes_song_fk
  foreign key (song_id) references music_songs(id);

create table if not exists sound_recorder_accounts (
  id uuid primary key default gen_random_uuid(),
  status varchar(32) default 'active' not null,
  external_subject varchar(240),
  display_name varchar(160),
  legal_region varchar(64),
  retention_hours integer default 500 not null,
  retention_policy_version varchar(80) default 'sound-recorder-retention-v1' not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint sound_recorder_accounts_status_chk
    check (status in ('active', 'paused', 'locked', 'deleted')),
  constraint sound_recorder_accounts_external_subject_size_chk
    check (external_subject is null or octet_length(external_subject) between 1 and 240),
  constraint sound_recorder_accounts_display_name_size_chk
    check (display_name is null or octet_length(display_name) between 1 and 160),
  constraint sound_recorder_accounts_legal_region_format_chk
    check (legal_region is null or legal_region ~ '^[A-Za-z0-9._:/-]{1,64}$'),
  constraint sound_recorder_accounts_retention_hours_chk
    check (retention_hours between 1 and 500),
  constraint sound_recorder_accounts_retention_policy_version_chk
    check (retention_policy_version ~ '^[A-Za-z0-9._:/-]{1,80}$')
);

create unique index if not exists sound_recorder_accounts_external_subject_uq
  on sound_recorder_accounts (external_subject)
  where external_subject is not null;

create index if not exists sound_recorder_accounts_status_updated_at_idx
  on sound_recorder_accounts (status, updated_at desc);

create table if not exists sound_recorder_devices (
  id uuid primary key default gen_random_uuid(),
  account_id uuid not null,
  platform varchar(24) not null,
  status varchar(32) default 'active' not null,
  install_id varchar(160) not null,
  device_label varchar(160),
  app_version varchar(80),
  os_version varchar(80),
  token_hash varchar(64) not null,
  token_last4 varchar(4) not null,
  consent_version varchar(80) not null,
  consent_accepted_at timestamptz not null,
  recording_indicator_acknowledged boolean default false not null,
  last_seen_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint sound_recorder_devices_platform_chk
    check (platform in ('ios', 'android')),
  constraint sound_recorder_devices_status_chk
    check (status in ('active', 'revoked', 'lost', 'replaced', 'deleted')),
  constraint sound_recorder_devices_install_id_size_chk
    check (octet_length(install_id) between 1 and 160),
  constraint sound_recorder_devices_device_label_size_chk
    check (device_label is null or octet_length(device_label) between 1 and 160),
  constraint sound_recorder_devices_app_version_size_chk
    check (app_version is null or octet_length(app_version) between 1 and 80),
  constraint sound_recorder_devices_os_version_size_chk
    check (os_version is null or octet_length(os_version) between 1 and 80),
  constraint sound_recorder_devices_token_hash_chk
    check (token_hash ~ '^[a-f0-9]{64}$'),
  constraint sound_recorder_devices_token_last4_chk
    check (token_last4 ~ '^[A-Za-z0-9_-]{4}$'),
  constraint sound_recorder_devices_consent_version_chk
    check (consent_version ~ '^[A-Za-z0-9._:/-]{1,80}$')
);

create unique index if not exists sound_recorder_devices_token_hash_uq
  on sound_recorder_devices (token_hash);

create unique index if not exists sound_recorder_devices_account_install_uq
  on sound_recorder_devices (account_id, install_id);

create index if not exists sound_recorder_devices_account_status_idx
  on sound_recorder_devices (account_id, status, updated_at desc);

alter table if exists sound_recorder_devices
  add constraint sound_recorder_devices_account_fk
  foreign key (account_id) references sound_recorder_accounts(id);

create table if not exists sound_recorder_upload_sessions (
  id uuid primary key default gen_random_uuid(),
  account_id uuid not null,
  device_id uuid not null,
  status varchar(32) default 'active' not null,
  storage_provider varchar(32) default 's3' not null,
  storage_bucket varchar(200) not null,
  storage_prefix text not null,
  content_type varchar(120) default 'audio/mp4' not null,
  codec varchar(80),
  sample_rate integer,
  channel_count integer default 1 not null,
  segment_duration_seconds integer default 60 not null,
  max_segment_bytes integer default 10485760 not null,
  started_at timestamptz default now() not null,
  last_heartbeat_at timestamptz,
  closed_at timestamptz,
  expires_at timestamptz,
  client_timezone varchar(80),
  legal_region varchar(64),
  use_case varchar(32) default 'security' not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint sound_recorder_upload_sessions_status_chk
    check (status in ('active', 'closed', 'revoked', 'expired')),
  constraint sound_recorder_upload_sessions_use_case_chk
    check (use_case in ('security', 'music', 'meeting', 'voice_note', 'ambient')),
  constraint sound_recorder_upload_sessions_storage_provider_chk
    check (storage_provider in ('s3')),
  constraint sound_recorder_upload_sessions_storage_bucket_size_chk
    check (octet_length(storage_bucket) between 1 and 200),
  constraint sound_recorder_upload_sessions_storage_prefix_size_chk
    check (octet_length(storage_prefix) between 1 and 2048),
  constraint sound_recorder_upload_sessions_content_type_size_chk
    check (octet_length(content_type) between 1 and 120),
  constraint sound_recorder_upload_sessions_codec_size_chk
    check (codec is null or octet_length(codec) between 1 and 80),
  constraint sound_recorder_upload_sessions_sample_rate_chk
    check (sample_rate is null or sample_rate between 8000 and 192000),
  constraint sound_recorder_upload_sessions_channel_count_chk
    check (channel_count between 1 and 8),
  constraint sound_recorder_upload_sessions_segment_duration_chk
    check (segment_duration_seconds between 1 and 600),
  constraint sound_recorder_upload_sessions_max_segment_bytes_chk
    check (max_segment_bytes between 1 and 209715200),
  constraint sound_recorder_upload_sessions_client_timezone_size_chk
    check (client_timezone is null or octet_length(client_timezone) between 1 and 80),
  constraint sound_recorder_upload_sessions_legal_region_format_chk
    check (legal_region is null or legal_region ~ '^[A-Za-z0-9._:/-]{1,64}$'),
  constraint sound_recorder_upload_sessions_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists sound_recorder_upload_sessions_storage_prefix_uq
  on sound_recorder_upload_sessions (storage_prefix);

create index if not exists sound_recorder_upload_sessions_account_started_idx
  on sound_recorder_upload_sessions (account_id, started_at desc);

create index if not exists sound_recorder_upload_sessions_device_status_idx
  on sound_recorder_upload_sessions (device_id, status, started_at desc);

alter table if exists sound_recorder_upload_sessions
  add constraint sound_recorder_upload_sessions_account_fk
  foreign key (account_id) references sound_recorder_accounts(id);

alter table if exists sound_recorder_upload_sessions
  add constraint sound_recorder_upload_sessions_device_fk
  foreign key (device_id) references sound_recorder_devices(id);

create table if not exists sound_recorder_segments (
  id uuid primary key default gen_random_uuid(),
  account_id uuid not null,
  device_id uuid not null,
  session_id uuid not null,
  sequence_number integer not null,
  status varchar(32) default 'pending' not null,
  storage_provider varchar(32) default 's3' not null,
  storage_bucket varchar(200) not null,
  storage_key text not null,
  content_type varchar(120) default 'audio/mp4' not null,
  codec varchar(80),
  captured_started_at timestamptz not null,
  captured_ended_at timestamptz,
  duration_millis integer not null,
  byte_count integer,
  sha256_hex varchar(64),
  upload_url_expires_at timestamptz,
  etag varchar(160),
  uploaded_at timestamptz,
  expires_at timestamptz not null,
  pinned_at timestamptz,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint sound_recorder_segments_sequence_chk
    check (sequence_number >= 0),
  constraint sound_recorder_segments_status_chk
    check (status in ('pending', 'uploaded', 'failed', 'expired', 'deleted')),
  constraint sound_recorder_segments_storage_provider_chk
    check (storage_provider in ('s3')),
  constraint sound_recorder_segments_storage_bucket_size_chk
    check (octet_length(storage_bucket) between 1 and 200),
  constraint sound_recorder_segments_storage_key_size_chk
    check (octet_length(storage_key) between 1 and 2048),
  constraint sound_recorder_segments_content_type_size_chk
    check (octet_length(content_type) between 1 and 120),
  constraint sound_recorder_segments_codec_size_chk
    check (codec is null or octet_length(codec) between 1 and 80),
  constraint sound_recorder_segments_duration_chk
    check (duration_millis between 1 and 600000),
  constraint sound_recorder_segments_byte_count_chk
    check (byte_count is null or byte_count between 0 and 209715200),
  constraint sound_recorder_segments_sha256_chk
    check (sha256_hex is null or sha256_hex ~ '^[a-f0-9]{64}$'),
  constraint sound_recorder_segments_etag_size_chk
    check (etag is null or octet_length(etag) between 1 and 160),
  constraint sound_recorder_segments_capture_window_chk
    check (captured_ended_at is null or captured_ended_at >= captured_started_at),
  constraint sound_recorder_segments_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists sound_recorder_segments_session_sequence_uq
  on sound_recorder_segments (session_id, sequence_number);

create unique index if not exists sound_recorder_segments_storage_key_uq
  on sound_recorder_segments (storage_key);

create index if not exists sound_recorder_segments_account_capture_idx
  on sound_recorder_segments (account_id, captured_started_at desc)
  where status = 'uploaded';

create index if not exists sound_recorder_segments_expiry_idx
  on sound_recorder_segments (expires_at asc)
  where status in ('pending', 'uploaded') and pinned_at is null;

alter table if exists sound_recorder_segments
  add constraint sound_recorder_segments_account_fk
  foreign key (account_id) references sound_recorder_accounts(id);

alter table if exists sound_recorder_segments
  add constraint sound_recorder_segments_device_fk
  foreign key (device_id) references sound_recorder_devices(id);

alter table if exists sound_recorder_segments
  add constraint sound_recorder_segments_session_fk
  foreign key (session_id) references sound_recorder_upload_sessions(id);

create table if not exists sound_recorder_evidence_exports (
  id uuid primary key default gen_random_uuid(),
  account_id uuid not null,
  device_id uuid,
  created_by_device_id uuid,
  status varchar(32) default 'requested' not null,
  requested_from timestamptz not null,
  requested_to timestamptz not null,
  segment_count integer default 0 not null,
  manifest jsonb default '{}'::jsonb not null,
  download_url_expires_at timestamptz,
  requested_at timestamptz default now() not null,
  ready_at timestamptz,
  expires_at timestamptz,
  meta_data jsonb default '{}'::jsonb not null,
  constraint sound_recorder_evidence_exports_status_chk
    check (status in ('requested', 'ready', 'expired', 'revoked')),
  constraint sound_recorder_evidence_exports_window_chk
    check (requested_to > requested_from),
  constraint sound_recorder_evidence_exports_segment_count_chk
    check (segment_count >= 0),
  constraint sound_recorder_evidence_exports_manifest_object_chk
    check (jsonb_typeof(manifest) = 'object'),
  constraint sound_recorder_evidence_exports_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists sound_recorder_evidence_exports_account_requested_idx
  on sound_recorder_evidence_exports (account_id, requested_at desc);

create index if not exists sound_recorder_evidence_exports_status_idx
  on sound_recorder_evidence_exports (status, requested_at desc);

alter table if exists sound_recorder_evidence_exports
  add constraint sound_recorder_evidence_exports_account_fk
  foreign key (account_id) references sound_recorder_accounts(id);

alter table if exists sound_recorder_evidence_exports
  add constraint sound_recorder_evidence_exports_device_fk
  foreign key (device_id) references sound_recorder_devices(id);

alter table if exists sound_recorder_evidence_exports
  add constraint sound_recorder_evidence_exports_created_by_device_fk
  foreign key (created_by_device_id) references sound_recorder_devices(id);

create table if not exists sound_recorder_audit_events (
  id uuid primary key default gen_random_uuid(),
  account_id uuid,
  device_id uuid,
  event_type varchar(80) not null,
  event_hash varchar(64) not null,
  payload jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  constraint sound_recorder_audit_events_event_type_format_chk
    check (event_type ~ '^[A-Za-z0-9._:/-]{1,80}$'),
  constraint sound_recorder_audit_events_event_hash_chk
    check (event_hash ~ '^[a-f0-9]{64}$'),
  constraint sound_recorder_audit_events_payload_object_chk
    check (jsonb_typeof(payload) = 'object')
);

create unique index if not exists sound_recorder_audit_events_event_hash_uq
  on sound_recorder_audit_events (event_hash);

create index if not exists sound_recorder_audit_events_account_created_idx
  on sound_recorder_audit_events (account_id, created_at desc)
  where account_id is not null;

create index if not exists sound_recorder_audit_events_type_created_idx
  on sound_recorder_audit_events (event_type, created_at desc);

alter table if exists sound_recorder_audit_events
  add constraint sound_recorder_audit_events_account_fk
  foreign key (account_id) references sound_recorder_accounts(id);

alter table if exists sound_recorder_audit_events
  add constraint sound_recorder_audit_events_device_fk
  foreign key (device_id) references sound_recorder_devices(id);

create table if not exists sound_recorder_oauth_states (
  id uuid primary key default gen_random_uuid(),
  account_id uuid not null,
  device_id uuid not null,
  provider varchar(32) not null,
  state_hash varchar(64) not null,
  redirect_uri varchar(512) not null,
  folder_path varchar(512),
  status varchar(32) default 'pending' not null,
  expires_at timestamptz not null,
  consumed_at timestamptz,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint sound_recorder_oauth_states_provider_chk
    check (provider in ('google_drive', 'microsoft_onedrive', 'apple_icloud')),
  constraint sound_recorder_oauth_states_status_chk
    check (status in ('pending', 'consumed', 'expired', 'revoked')),
  constraint sound_recorder_oauth_states_hash_chk
    check (state_hash ~ '^[a-f0-9]{64}$'),
  constraint sound_recorder_oauth_states_redirect_uri_size_chk
    check (octet_length(redirect_uri) between 1 and 512),
  constraint sound_recorder_oauth_states_folder_path_size_chk
    check (folder_path is null or octet_length(folder_path) between 1 and 512),
  constraint sound_recorder_oauth_states_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists sound_recorder_oauth_states_hash_uq
  on sound_recorder_oauth_states (state_hash);

create index if not exists sound_recorder_oauth_states_account_provider_idx
  on sound_recorder_oauth_states (account_id, provider, created_at desc);

create index if not exists sound_recorder_oauth_states_expiry_idx
  on sound_recorder_oauth_states (expires_at asc)
  where status = 'pending';

alter table if exists sound_recorder_oauth_states
  add constraint sound_recorder_oauth_states_account_fk
  foreign key (account_id) references sound_recorder_accounts(id);

alter table if exists sound_recorder_oauth_states
  add constraint sound_recorder_oauth_states_device_fk
  foreign key (device_id) references sound_recorder_devices(id);

create table if not exists sound_recorder_cloud_connections (
  id uuid primary key default gen_random_uuid(),
  account_id uuid not null,
  created_by_device_id uuid,
  provider varchar(32) not null,
  link_mode varchar(32) default 'server_oauth' not null,
  status varchar(32) default 'active' not null,
  display_name varchar(160),
  provider_account_id varchar(240),
  provider_subject_hash varchar(64),
  root_folder_id varchar(512),
  folder_path varchar(512) default 'sound-recorder' not null,
  oauth_scope text,
  token_ciphertext text,
  token_nonce varchar(64),
  token_aad varchar(512),
  token_version integer,
  token_expires_at timestamptz,
  last_sync_at timestamptz,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint sound_recorder_cloud_connections_provider_chk
    check (provider in ('google_drive', 'microsoft_onedrive', 'apple_icloud')),
  constraint sound_recorder_cloud_connections_link_mode_chk
    check (link_mode in ('server_oauth', 'client_managed')),
  constraint sound_recorder_cloud_connections_status_chk
    check (status in ('active', 'paused', 'revoked', 'failed')),
  constraint sound_recorder_cloud_connections_display_name_size_chk
    check (display_name is null or octet_length(display_name) between 1 and 160),
  constraint sound_recorder_cloud_connections_provider_account_id_size_chk
    check (provider_account_id is null or octet_length(provider_account_id) between 1 and 240),
  constraint sound_recorder_cloud_connections_subject_hash_chk
    check (provider_subject_hash is null or provider_subject_hash ~ '^[a-f0-9]{64}$'),
  constraint sound_recorder_cloud_connections_root_folder_id_size_chk
    check (root_folder_id is null or octet_length(root_folder_id) between 1 and 512),
  constraint sound_recorder_cloud_connections_folder_path_size_chk
    check (octet_length(folder_path) between 1 and 512),
  constraint sound_recorder_cloud_connections_token_version_chk
    check (token_version is null or token_version > 0),
  constraint sound_recorder_cloud_connections_token_shape_chk
    check (
      status = 'revoked'
      or link_mode = 'client_managed'
      or (token_ciphertext is not null and token_nonce is not null and token_aad is not null and token_version is not null)
    ),
  constraint sound_recorder_cloud_connections_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists sound_recorder_cloud_connections_active_account_provider_uq
  on sound_recorder_cloud_connections (account_id, provider, provider_account_id)
  where status <> 'revoked' and provider_account_id is not null;

create index if not exists sound_recorder_cloud_connections_account_status_idx
  on sound_recorder_cloud_connections (account_id, status, updated_at desc);

alter table if exists sound_recorder_cloud_connections
  add constraint sound_recorder_cloud_connections_account_fk
  foreign key (account_id) references sound_recorder_accounts(id);

alter table if exists sound_recorder_cloud_connections
  add constraint sound_recorder_cloud_connections_created_by_device_fk
  foreign key (created_by_device_id) references sound_recorder_devices(id);

create table if not exists sound_recorder_cloud_copy_jobs (
  id uuid primary key default gen_random_uuid(),
  account_id uuid not null,
  connection_id uuid not null,
  segment_id uuid not null,
  provider varchar(32) not null,
  status varchar(32) default 'pending' not null,
  destination_key varchar(2048) not null,
  provider_file_id varchar(512),
  attempts integer default 0 not null,
  locked_until timestamptz,
  started_at timestamptz,
  completed_at timestamptz,
  last_error varchar(500),
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint sound_recorder_cloud_copy_jobs_provider_chk
    check (provider in ('google_drive', 'microsoft_onedrive', 'apple_icloud')),
  constraint sound_recorder_cloud_copy_jobs_status_chk
    check (status in ('pending', 'running', 'waiting_client', 'completed', 'failed', 'skipped')),
  constraint sound_recorder_cloud_copy_jobs_destination_key_size_chk
    check (octet_length(destination_key) between 1 and 2048),
  constraint sound_recorder_cloud_copy_jobs_provider_file_id_size_chk
    check (provider_file_id is null or octet_length(provider_file_id) between 1 and 512),
  constraint sound_recorder_cloud_copy_jobs_attempts_chk
    check (attempts >= 0 and attempts <= 50),
  constraint sound_recorder_cloud_copy_jobs_last_error_size_chk
    check (last_error is null or octet_length(last_error) between 1 and 500),
  constraint sound_recorder_cloud_copy_jobs_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists sound_recorder_cloud_copy_jobs_connection_segment_uq
  on sound_recorder_cloud_copy_jobs (connection_id, segment_id);

create index if not exists sound_recorder_cloud_copy_jobs_account_status_idx
  on sound_recorder_cloud_copy_jobs (account_id, status, updated_at asc);

create index if not exists sound_recorder_cloud_copy_jobs_connection_status_idx
  on sound_recorder_cloud_copy_jobs (connection_id, status, updated_at asc);

create index if not exists sound_recorder_cloud_copy_jobs_segment_idx
  on sound_recorder_cloud_copy_jobs (segment_id);

alter table if exists sound_recorder_cloud_copy_jobs
  add constraint sound_recorder_cloud_copy_jobs_account_fk
  foreign key (account_id) references sound_recorder_accounts(id);

alter table if exists sound_recorder_cloud_copy_jobs
  add constraint sound_recorder_cloud_copy_jobs_connection_fk
  foreign key (connection_id) references sound_recorder_cloud_connections(id);

alter table if exists sound_recorder_cloud_copy_jobs
  add constraint sound_recorder_cloud_copy_jobs_segment_fk
  foreign key (segment_id) references sound_recorder_segments(id);

create table if not exists container_pool_configs (
  id uuid primary key default gen_random_uuid(),
  slug varchar(120) not null,
  display_name varchar(200) not null,
  image text not null,
  command jsonb default '[]'::jsonb not null,
  env jsonb default '{}'::jsonb not null,
  request_path varchar(256) default '/invoke' not null,
  health_path varchar(256) default '/healthz' not null,
  container_port integer default 8080 not null,
  min_warm integer default 1 not null,
  max_warm integer default 2 not null,
  max_concurrency_per_container integer default 1 not null,
  request_timeout_ms integer default 30000 not null,
  idle_ttl_seconds integer default 900 not null,
  nats_subject text,
  status varchar(32) default 'active' not null,
  labels jsonb default '[]'::jsonb not null,
  meta_data jsonb default '{}'::jsonb not null,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint container_pool_configs_slug_format_chk
    check (slug ~ '^[a-z0-9][a-z0-9-]{0,118}[a-z0-9]$'),
  constraint container_pool_configs_image_size_chk
    check (octet_length(image) between 1 and 512),
  constraint container_pool_configs_display_name_size_chk
    check (octet_length(display_name) <= 200),
  constraint container_pool_configs_command_array_chk
    check (jsonb_typeof(command) = 'array'),
  constraint container_pool_configs_env_object_chk
    check (jsonb_typeof(env) = 'object'),
  constraint container_pool_configs_request_path_chk
    check (request_path ~ '^/[A-Za-z0-9._~!$&''()*+,;=:@%/-]{0,255}$'),
  constraint container_pool_configs_health_path_chk
    check (health_path ~ '^/[A-Za-z0-9._~!$&''()*+,;=:@%/-]{0,255}$'),
  constraint container_pool_configs_container_port_chk
    check (container_port between 1 and 65535),
  constraint container_pool_configs_min_warm_chk
    check (min_warm between 0 and 64),
  constraint container_pool_configs_max_warm_chk
    check (max_warm between 1 and 128 and max_warm >= min_warm),
  constraint container_pool_configs_concurrency_chk
    check (max_concurrency_per_container between 1 and 128),
  constraint container_pool_configs_timeout_chk
    check (request_timeout_ms between 100 and 900000),
  constraint container_pool_configs_idle_ttl_chk
    check (idle_ttl_seconds between 10 and 86400),
  constraint container_pool_configs_nats_subject_size_chk
    check (nats_subject is null or octet_length(nats_subject) <= 256),
  constraint container_pool_configs_labels_array_chk
    check (jsonb_typeof(labels) = 'array'),
  constraint container_pool_configs_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object'),
  constraint container_pool_configs_status_chk
    check (status in ('active', 'paused', 'archived'))
);

create unique index if not exists container_pool_configs_slug_active_uq
  on container_pool_configs (slug)
  where is_soft_deleted = false;

create index if not exists container_pool_configs_status_idx
  on container_pool_configs (status)
  where is_soft_deleted = false;

create index if not exists container_pool_configs_updated_at_idx
  on container_pool_configs (updated_at desc)
  where is_soft_deleted = false;

create index if not exists container_pool_configs_labels_gin_idx
  on container_pool_configs using gin (labels);

create table if not exists known_git_repos (
  id uuid primary key default gen_random_uuid(),
  repo_url text not null,
  display_name varchar(200) not null,
  provider varchar(40) default 'github' not null,
  default_branch varchar(120) default 'dev' not null,
  status varchar(32) default 'active' not null,
  last_verified_at timestamptz,
  meta_data jsonb default '{}'::jsonb not null,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint known_git_repos_repo_url_format_chk
    check (repo_url ~ '^(git@|ssh://|https://).+'),
  constraint known_git_repos_repo_url_size_chk
    check (octet_length(repo_url) <= 2048),
  constraint known_git_repos_display_name_size_chk
    check (octet_length(display_name) <= 200),
  constraint known_git_repos_default_branch_format_chk
    check (default_branch ~ '^[A-Za-z0-9._/-]{1,120}$'),
  constraint known_git_repos_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object'),
  constraint known_git_repos_provider_chk
    check (provider in ('github', 'gitlab', 'bitbucket', 'generic')),
  constraint known_git_repos_status_chk
    check (status in ('active', 'paused', 'archived'))
);

create unique index if not exists known_git_repos_repo_url_active_uq
  on known_git_repos (repo_url)
  where is_soft_deleted = false;

create index if not exists known_git_repos_status_idx
  on known_git_repos (status)
  where is_soft_deleted = false;

create index if not exists known_git_repos_updated_at_idx
  on known_git_repos (updated_at desc)
  where is_soft_deleted = false;

create table if not exists agent_context_blobs (
  id uuid primary key default gen_random_uuid(),
  project_id varchar(120) default 'default' not null,
  repo_id uuid,
  context_id varchar(200) not null,
  context_title varchar(300) not null,
  context_blob text not null,
  status varchar(32) default 'active' not null,
  labels jsonb default '[]'::jsonb not null,
  meta_data jsonb default '{}'::jsonb not null,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint agent_context_blobs_project_id_format_chk
    check (project_id ~ '^[A-Za-z0-9._:/-]{1,120}$'),
  constraint agent_context_blobs_context_id_format_chk
    check (context_id ~ '^[A-Za-z0-9._:/-]{1,200}$'),
  constraint agent_context_blobs_context_title_size_chk
    check (octet_length(context_title) between 1 and 300),
  constraint agent_context_blobs_context_blob_size_chk
    check (octet_length(context_blob) between 1 and 1048576),
  constraint agent_context_blobs_labels_array_chk
    check (jsonb_typeof(labels) = 'array'),
  constraint agent_context_blobs_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object'),
  constraint agent_context_blobs_status_chk
    check (status in ('active', 'paused', 'archived'))
);

create unique index if not exists agent_context_blobs_project_repo_context_active_uq
  on agent_context_blobs (project_id, repo_id, context_id)
  where is_soft_deleted = false;

create index if not exists agent_context_blobs_repo_id_idx
  on agent_context_blobs (repo_id)
  where is_soft_deleted = false;

create index if not exists agent_context_blobs_project_id_idx
  on agent_context_blobs (project_id)
  where is_soft_deleted = false;

create index if not exists agent_context_blobs_updated_at_idx
  on agent_context_blobs (updated_at desc)
  where is_soft_deleted = false;

create table if not exists agent_context_embeddings (
  id uuid primary key default gen_random_uuid(),
  context_blob_id uuid not null,
  embedding_model varchar(120) not null,
  embedding jsonb not null,
  embedding_dimensions integer not null,
  content_sha256 varchar(64) not null,
  created_at timestamptz default now() not null,
  constraint agent_context_embeddings_model_format_chk
    check (embedding_model ~ '^[A-Za-z0-9._:/-]{1,120}$'),
  constraint agent_context_embeddings_dimensions_chk
    check (embedding_dimensions > 0),
  constraint agent_context_embeddings_array_chk
    check (jsonb_typeof(embedding) = 'array'),
  constraint agent_context_embeddings_sha256_chk
    check (content_sha256 ~ '^[a-f0-9]{64}$')
);

create unique index if not exists agent_context_embeddings_blob_model_sha_uq
  on agent_context_embeddings (context_blob_id, embedding_model, content_sha256);

create index if not exists agent_context_embeddings_blob_id_idx
  on agent_context_embeddings (context_blob_id);

create table if not exists agent_remote_dev_threads (
  id uuid primary key,
  user_id uuid not null,
  known_git_repo_id uuid,
  title text default 'New thread' not null,
  repo text not null,
  base_branch varchar(120) default 'dev' not null,
  meta jsonb default '{}'::jsonb not null,
  archived_at timestamptz,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint agent_remote_dev_threads_repo_format_chk
    check (repo ~ '^(git@|ssh://|https://).+'),
  constraint agent_remote_dev_threads_repo_size_chk
    check (octet_length(repo) <= 2048),
  constraint agent_remote_dev_threads_title_size_chk
    check (octet_length(title) <= 500),
  constraint agent_remote_dev_threads_base_branch_format_chk
    check (base_branch ~ '^[A-Za-z0-9._/-]{1,120}$'),
  constraint agent_remote_dev_threads_meta_object_chk
    check (jsonb_typeof(meta) = 'object')
);

create index if not exists agent_remote_dev_threads_user_id_idx
  on agent_remote_dev_threads (user_id)
  where is_soft_deleted = false;

create index if not exists agent_remote_dev_threads_known_git_repo_id_idx
  on agent_remote_dev_threads (known_git_repo_id)
  where is_soft_deleted = false;

create index if not exists agent_remote_dev_threads_repo_idx
  on agent_remote_dev_threads (repo)
  where is_soft_deleted = false;

create index if not exists agent_remote_dev_threads_updated_at_idx
  on agent_remote_dev_threads (updated_at desc)
  where is_soft_deleted = false;

create index if not exists agent_remote_dev_threads_created_at_idx
  on agent_remote_dev_threads (created_at desc)
  where is_soft_deleted = false;

create table if not exists agent_remote_dev_tasks (
  id uuid primary key,
  thread_id uuid not null,
  user_id uuid not null,
  docker_task_id uuid not null,
  prompt text not null,
  status varchar(32) default 'queued' not null,
  branch varchar(200),
  pr_url text,
  pr_state varchar(32),
  exit_reason varchar(32),
  error_message text,
  last_event_seq integer default -1 not null,
  meta jsonb default '{}'::jsonb not null,
  is_soft_deleted boolean default false not null,
  started_at timestamptz,
  finished_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint agent_remote_dev_tasks_prompt_size_chk
    check (octet_length(prompt) <= 1048576),
  constraint agent_remote_dev_tasks_status_chk
    check (status in ('queued', 'running', 'streaming', 'pushed', 'pr_open', 'pr_merged', 'pr_closed', 'done', 'failed', 'cancelled')),
  constraint agent_remote_dev_tasks_pr_state_chk
    check (pr_state is null or pr_state in ('draft', 'open', 'closed', 'merged')),
  constraint agent_remote_dev_tasks_exit_reason_chk
    check (exit_reason is null or exit_reason in ('completed', 'cancelled', 'failed')),
  constraint agent_remote_dev_tasks_meta_object_chk
    check (jsonb_typeof(meta) = 'object')
);

create unique index if not exists agent_remote_dev_tasks_docker_task_id_uq
  on agent_remote_dev_tasks (docker_task_id);

create index if not exists agent_remote_dev_tasks_thread_id_created_at_idx
  on agent_remote_dev_tasks (thread_id, created_at desc)
  where is_soft_deleted = false;

create index if not exists agent_remote_dev_tasks_user_id_idx
  on agent_remote_dev_tasks (user_id)
  where is_soft_deleted = false;

create index if not exists agent_remote_dev_tasks_status_idx
  on agent_remote_dev_tasks (status)
  where is_soft_deleted = false;

create index if not exists agent_remote_dev_tasks_updated_at_idx
  on agent_remote_dev_tasks (updated_at desc)
  where is_soft_deleted = false;

create index if not exists agent_remote_dev_tasks_created_at_idx
  on agent_remote_dev_tasks (created_at desc)
  where is_soft_deleted = false;

create table if not exists agent_remote_dev_events (
  id bigserial primary key,
  task_id uuid not null,
  thread_id uuid,
  seq integer not null,
  event_kind varchar(80) not null,
  payload jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  constraint agent_remote_dev_events_event_kind_format_chk
    check (event_kind ~ '^[A-Za-z0-9._:-]{1,80}$'),
  constraint agent_remote_dev_events_payload_object_chk
    check (jsonb_typeof(payload) = 'object')
);

create unique index if not exists agent_remote_dev_events_task_seq_uq
  on agent_remote_dev_events (task_id, seq);

alter table if exists agent_remote_dev_events
  add column if not exists thread_id uuid;

create index if not exists agent_remote_dev_events_task_id_created_at_idx
  on agent_remote_dev_events (task_id, created_at desc);

create index if not exists agent_remote_dev_events_thread_id_created_at_idx
  on agent_remote_dev_events (thread_id, created_at desc)
  where thread_id is not null;

create index if not exists agent_remote_dev_events_created_at_idx
  on agent_remote_dev_events (created_at desc);

create table if not exists agent_remote_dev_breadcrumbs (
  id bigserial primary key,
  thread_id uuid not null,
  task_id uuid,
  kind varchar(80) not null,
  payload jsonb default '{}'::jsonb not null,
  emitted_at timestamptz default now() not null,
  pod_name varchar(253),
  branch varchar(120),
  provider varchar(60),
  constraint agent_remote_dev_breadcrumbs_kind_format_chk
    check (kind ~ '^[A-Za-z0-9._:-]{1,80}$'),
  constraint agent_remote_dev_breadcrumbs_payload_object_chk
    check (jsonb_typeof(payload) = 'object'),
  constraint agent_remote_dev_breadcrumbs_pod_name_size_chk
    check (pod_name is null or octet_length(pod_name) <= 253),
  constraint agent_remote_dev_breadcrumbs_branch_size_chk
    check (branch is null or octet_length(branch) <= 120),
  constraint agent_remote_dev_breadcrumbs_provider_size_chk
    check (provider is null or octet_length(provider) <= 60)
);

create index if not exists agent_remote_dev_breadcrumbs_thread_id_emitted_at_idx
  on agent_remote_dev_breadcrumbs (thread_id, emitted_at desc);

create index if not exists agent_remote_dev_breadcrumbs_task_id_emitted_at_idx
  on agent_remote_dev_breadcrumbs (task_id, emitted_at desc)
  where task_id is not null;

create index if not exists agent_remote_dev_breadcrumbs_emitted_at_idx
  on agent_remote_dev_breadcrumbs (emitted_at desc);

create table if not exists agent_remote_dev_artifacts (
  id uuid primary key default gen_random_uuid(),
  task_id uuid not null,
  thread_id uuid not null,
  filename text not null,
  content_type varchar(200),
  size_bytes bigint,
  storage_provider varchar(32) not null,
  storage_bucket varchar(200),
  storage_key text,
  url text not null,
  signed_url_expires_at timestamptz,
  sha256 varchar(64),
  meta jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  constraint agent_remote_dev_artifacts_filename_size_chk
    check (octet_length(filename) <= 1024),
  constraint agent_remote_dev_artifacts_url_size_chk
    check (octet_length(url) <= 4096),
  constraint agent_remote_dev_artifacts_meta_object_chk
    check (jsonb_typeof(meta) = 'object'),
  constraint agent_remote_dev_artifacts_storage_provider_chk
    check (storage_provider in ('s3', 'r2', 'gcs', 'drive', 'local'))
);

create index if not exists agent_remote_dev_artifacts_task_id_created_at_idx
  on agent_remote_dev_artifacts (task_id, created_at desc);

create index if not exists agent_remote_dev_artifacts_thread_id_created_at_idx
  on agent_remote_dev_artifacts (thread_id, created_at desc);

create index if not exists agent_remote_dev_artifacts_created_at_idx
  on agent_remote_dev_artifacts (created_at desc);

create table if not exists agent_remote_dev_runtime_locks (
  id uuid primary key default gen_random_uuid(),
  thread_id uuid not null,
  owner varchar(200) not null,
  status varchar(32) default 'active' not null,
  fencing_token integer default 0 not null,
  lease_expires_at timestamptz not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint agent_remote_dev_runtime_locks_owner_size_chk
    check (octet_length(owner) <= 200),
  constraint agent_remote_dev_runtime_locks_status_chk
    check (status in ('active', 'released', 'expired'))
);

create unique index if not exists agent_remote_dev_runtime_locks_thread_active_uq
  on agent_remote_dev_runtime_locks (thread_id)
  where status = 'active';

create index if not exists agent_remote_dev_runtime_locks_lease_expires_at_idx
  on agent_remote_dev_runtime_locks (lease_expires_at);

create table if not exists mip_solver_sessions (
  session_id varchar(200) primary key,
  revision bigint default 0 not null,
  problem jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint mip_solver_sessions_session_id_size_chk
    check (octet_length(session_id) between 1 and 200),
  constraint mip_solver_sessions_revision_chk
    check (revision >= 0),
  constraint mip_solver_sessions_problem_json_chk
    check (jsonb_typeof(problem) = 'object')
);

create index if not exists mip_solver_sessions_updated_at_idx
  on mip_solver_sessions (updated_at desc);

create table if not exists mip_solver_solves (
  solve_id varchar(160) primary key,
  request_id varchar(200) not null,
  revision bigint default 0 not null,
  status varchar(64) default 'running' not null,
  node_id varchar(253) not null,
  node_role varchar(32) not null,
  problem jsonb default '{}'::jsonb not null,
  options jsonb default '{}'::jsonb not null,
  response jsonb default '{}'::jsonb not null,
  jobs_expected integer default 0 not null,
  jobs_published integer default 0 not null,
  jobs_completed integer default 0 not null,
  jobs_redelegated integer default 0 not null,
  jobs_split integer default 0 not null,
  timed_out boolean default false not null,
  distributed boolean default false not null,
  warnings jsonb default '[]'::jsonb not null,
  started_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  finished_at timestamptz,
  constraint mip_solver_solves_solve_id_size_chk
    check (octet_length(solve_id) between 1 and 160),
  constraint mip_solver_solves_request_id_size_chk
    check (octet_length(request_id) between 1 and 200),
  constraint mip_solver_solves_status_size_chk
    check (octet_length(status) between 1 and 64),
  constraint mip_solver_solves_node_id_size_chk
    check (octet_length(node_id) between 1 and 253),
  constraint mip_solver_solves_node_role_chk
    check (node_role in ('master', 'slave')),
  constraint mip_solver_solves_counts_chk
    check (revision >= 0 and jobs_expected >= 0 and jobs_published >= 0 and jobs_completed >= 0 and jobs_redelegated >= 0 and jobs_split >= 0),
  constraint mip_solver_solves_problem_json_chk
    check (jsonb_typeof(problem) = 'object'),
  constraint mip_solver_solves_options_json_chk
    check (jsonb_typeof(options) = 'object'),
  constraint mip_solver_solves_response_json_chk
    check (jsonb_typeof(response) = 'object'),
  constraint mip_solver_solves_warnings_json_chk
    check (jsonb_typeof(warnings) = 'array')
);

create index if not exists mip_solver_solves_request_id_idx
  on mip_solver_solves (request_id, updated_at desc);

create index if not exists mip_solver_solves_status_idx
  on mip_solver_solves (status, updated_at desc);

create table if not exists mip_solver_jobs (
  job_id varchar(240) primary key,
  solve_id varchar(160) not null,
  root_job_id varchar(240) not null,
  retry_index integer default 0 not null,
  depth integer default 0 not null,
  status varchar(64) default 'submitted' not null,
  worker_node varchar(253),
  job_payload jsonb default '{}'::jsonb not null,
  result_payload jsonb default '{}'::jsonb not null,
  submitted_at timestamptz default now() not null,
  finished_at timestamptz,
  updated_at timestamptz default now() not null,
  constraint mip_solver_jobs_job_id_size_chk
    check (octet_length(job_id) between 1 and 240),
  constraint mip_solver_jobs_root_job_id_size_chk
    check (octet_length(root_job_id) between 1 and 240),
  constraint mip_solver_jobs_status_size_chk
    check (octet_length(status) between 1 and 64),
  constraint mip_solver_jobs_counts_chk
    check (retry_index >= 0 and depth >= 0),
  constraint mip_solver_jobs_job_payload_json_chk
    check (jsonb_typeof(job_payload) = 'object'),
  constraint mip_solver_jobs_result_payload_json_chk
    check (jsonb_typeof(result_payload) = 'object')
);

create index if not exists mip_solver_jobs_solve_status_idx
  on mip_solver_jobs (solve_id, status, updated_at desc);

create index if not exists mip_solver_jobs_root_idx
  on mip_solver_jobs (solve_id, root_job_id, retry_index);

create table if not exists mip_solver_events (
  id bigserial primary key,
  solve_id varchar(160),
  session_id varchar(200),
  job_id varchar(240),
  event_kind varchar(80) not null,
  payload jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  constraint mip_solver_events_event_kind_format_chk
    check (event_kind ~ '^[A-Za-z0-9._:-]{1,80}$'),
  constraint mip_solver_events_payload_json_chk
    check (jsonb_typeof(payload) = 'object')
);

create index if not exists mip_solver_events_solve_created_at_idx
  on mip_solver_events (solve_id, created_at desc)
  where solve_id is not null;

create index if not exists mip_solver_events_session_created_at_idx
  on mip_solver_events (session_id, created_at desc)
  where session_id is not null;

alter table if exists mip_solver_jobs
  add constraint mip_solver_jobs_solve_fk
  foreign key (solve_id) references mip_solver_solves(solve_id);

alter table if exists mip_solver_events
  add constraint mip_solver_events_solve_fk
  foreign key (solve_id) references mip_solver_solves(solve_id);

alter table if exists mip_solver_events
  add constraint mip_solver_events_session_fk
  foreign key (session_id) references mip_solver_sessions(session_id);

alter table if exists mip_solver_events
  add constraint mip_solver_events_job_fk
  foreign key (job_id) references mip_solver_jobs(job_id);

create table if not exists lambda_functions (
  id uuid primary key default gen_random_uuid(),
  slug varchar(120) not null,
  display_name varchar(200) not null,
  description text default '' not null,
  runtime varchar(40) default 'nodejs' not null,
  entry_command text default 'env -i PATH="$PATH" NODE_ENV=production NODE_NO_WARNINGS=1 node --permission --allow-net child-runtimes/js-function-runner.mjs' not null,
  function_body text not null,
  reuse_key varchar(200),
  idle_timeout_seconds integer default 300 not null,
  max_run_ms integer default 30000 not null,
  containerized boolean default false not null,
  container_image text,
  container_build_status varchar(32) default 'not_requested' not null,
  container_build_error text,
  container_built_at timestamptz,
  status varchar(32) default 'draft' not null,
  env jsonb default '{}'::jsonb not null,
  labels jsonb default '[]'::jsonb not null,
  meta_data jsonb default '{}'::jsonb not null,
  last_invoked_at timestamptz,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint lambda_functions_slug_format_chk
    check (slug ~ '^[a-z0-9][a-z0-9-]{1,118}[a-z0-9]$'),
  constraint lambda_functions_body_size_chk
    check (octet_length(function_body) <= 262144),
  constraint lambda_functions_entry_command_chk
    check (octet_length(entry_command) between 1 and 512),
  constraint lambda_functions_container_image_size_chk
    check (container_image is null or octet_length(container_image) <= 512),
  constraint lambda_functions_container_build_error_size_chk
    check (container_build_error is null or octet_length(container_build_error) <= 8192),
  constraint lambda_functions_labels_array_chk
    check (jsonb_typeof(labels) = 'array'),
  constraint lambda_functions_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object'),
  constraint lambda_functions_env_object_chk
    check (jsonb_typeof(env) = 'object'),
  constraint lambda_functions_status_chk
    check (status in ('draft', 'active', 'paused', 'archived')),
  constraint lambda_functions_runtime_chk
    check (runtime in ('nodejs', 'javascript', 'typescript', 'python3', 'python', 'ruby', 'bash', 'shell', 'golang', 'go', 'dart', 'erlang', 'erl', 'elixir', 'ex', 'java', 'jvm')),
  constraint lambda_functions_container_build_status_chk
    check (container_build_status in ('not_requested', 'pending', 'building', 'built', 'failed', 'skipped'))
);

create unique index if not exists lambda_functions_slug_active_uq
  on lambda_functions (slug)
  where is_soft_deleted = false;

create index if not exists lambda_functions_status_idx
  on lambda_functions (status)
  where is_soft_deleted = false;

create index if not exists lambda_functions_updated_at_idx
  on lambda_functions (updated_at desc)
  where is_soft_deleted = false;

create index if not exists lambda_functions_labels_gin_idx
  on lambda_functions using gin (labels);

-- ─────────────────────────────────────────────────────────────────────────────
-- Container-pool image config:
--
-- One row per saved Dockerfile revision for each container-pool image (e.g.
-- the per-runtime warm runtime images and the dd-dev-server worker image).
-- Operators iterate on Dockerfiles via /container-pool/config in the web UI;
-- the on-disk Dockerfile in git is the "sane default" (loaded as a synthetic
-- revision with source='disk-default'), and saves become new rows here.
-- Each revision is content-addressed by `dockerfile_sha256` so duplicate
-- saves coalesce into a single row.
-- ─────────────────────────────────────────────────────────────────────────────

create table if not exists container_pool_image_revisions (
  id uuid primary key default gen_random_uuid(),
  image_slug varchar(120) not null,
  image_ref text not null,
  dockerfile_path text not null,
  build_context text not null,
  dockerfile_text text not null,
  dockerfile_sha256 varchar(64) not null,
  source varchar(32) default 'user' not null,
  notes text default '' not null,
  status varchar(32) default 'candidate' not null,
  meta_data jsonb default '{}'::jsonb not null,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint container_pool_image_revisions_slug_format_chk
    check (image_slug ~ '^[a-z0-9][a-z0-9-]{0,118}[a-z0-9]$'),
  constraint container_pool_image_revisions_dockerfile_size_chk
    check (octet_length(dockerfile_text) between 1 and 65536),
  constraint container_pool_image_revisions_image_ref_size_chk
    check (octet_length(image_ref) between 1 and 512),
  constraint container_pool_image_revisions_path_size_chk
    check (octet_length(dockerfile_path) between 1 and 512),
  constraint container_pool_image_revisions_context_size_chk
    check (octet_length(build_context) between 1 and 512),
  constraint container_pool_image_revisions_notes_size_chk
    check (octet_length(notes) <= 8192),
  constraint container_pool_image_revisions_sha_format_chk
    check (dockerfile_sha256 ~ '^[0-9a-f]{64}$'),
  constraint container_pool_image_revisions_status_chk
    check (status in ('candidate', 'active', 'archived')),
  constraint container_pool_image_revisions_source_chk
    check (source in ('disk-default', 'user', 'system')),
  constraint container_pool_image_revisions_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists container_pool_image_revisions_slug_idx
  on container_pool_image_revisions (image_slug, created_at desc)
  where is_soft_deleted = false;

create unique index if not exists container_pool_image_revisions_slug_sha_uq
  on container_pool_image_revisions (image_slug, dockerfile_sha256)
  where is_soft_deleted = false;

-- Per-image build + smoke-test runs. `build_status` covers the nerdctl build
-- step; `test_status` covers the post-build smoke run; `overall_status` is
-- the rolled-up state surfaced in the UI.
create table if not exists container_pool_build_runs (
  id uuid primary key default gen_random_uuid(),
  image_slug varchar(120) not null,
  revision_id uuid not null references container_pool_image_revisions(id),
  image_ref text not null,
  candidate_tag text not null,
  build_status varchar(32) default 'queued' not null,
  test_status varchar(32) default 'not_started' not null,
  overall_status varchar(32) default 'queued' not null,
  test_command text default '' not null,
  build_started_at timestamptz,
  build_finished_at timestamptz,
  test_started_at timestamptz,
  test_finished_at timestamptz,
  build_log_excerpt text default '' not null,
  test_log_excerpt text default '' not null,
  error_message text,
  triggered_by uuid,
  meta_data jsonb default '{}'::jsonb not null,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint container_pool_build_runs_slug_format_chk
    check (image_slug ~ '^[a-z0-9][a-z0-9-]{0,118}[a-z0-9]$'),
  constraint container_pool_build_runs_image_ref_size_chk
    check (octet_length(image_ref) between 1 and 512),
  constraint container_pool_build_runs_candidate_tag_size_chk
    check (octet_length(candidate_tag) between 1 and 512),
  constraint container_pool_build_runs_test_command_size_chk
    check (octet_length(test_command) <= 4096),
  constraint container_pool_build_runs_log_size_chk
    check (octet_length(build_log_excerpt) <= 65536
       and octet_length(test_log_excerpt) <= 65536),
  constraint container_pool_build_runs_error_size_chk
    check (error_message is null or octet_length(error_message) <= 8192),
  constraint container_pool_build_runs_build_status_chk
    check (build_status in ('queued', 'building', 'built', 'failed', 'skipped', 'cancelled')),
  constraint container_pool_build_runs_test_status_chk
    check (test_status in ('not_started', 'pending', 'testing', 'passed', 'failed', 'skipped', 'cancelled')),
  constraint container_pool_build_runs_overall_status_chk
    check (overall_status in ('queued', 'running', 'passed', 'failed', 'cancelled', 'errored')),
  constraint container_pool_build_runs_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create index if not exists container_pool_build_runs_slug_idx
  on container_pool_build_runs (image_slug, created_at desc)
  where is_soft_deleted = false;

create index if not exists container_pool_build_runs_overall_idx
  on container_pool_build_runs (overall_status)
  where is_soft_deleted = false;

alter table if exists agent_remote_dev_threads
  add constraint agent_remote_dev_threads_known_git_repo_fk
  foreign key (known_git_repo_id) references known_git_repos(id);

alter table if exists agent_context_blobs
  add constraint agent_context_blobs_repo_fk
  foreign key (repo_id) references known_git_repos(id);

alter table if exists agent_context_embeddings
  add constraint agent_context_embeddings_blob_fk
  foreign key (context_blob_id) references agent_context_blobs(id);

alter table if exists agent_remote_dev_tasks
  add constraint agent_remote_dev_tasks_thread_fk
  foreign key (thread_id) references agent_remote_dev_threads(id);

alter table if exists agent_remote_dev_events
  add constraint agent_remote_dev_events_task_fk
  foreign key (task_id) references agent_remote_dev_tasks(id);

alter table if exists agent_remote_dev_events
  add constraint agent_remote_dev_events_thread_fk
  foreign key (thread_id) references agent_remote_dev_threads(id);

alter table if exists agent_remote_dev_artifacts
  add constraint agent_remote_dev_artifacts_task_fk
  foreign key (task_id) references agent_remote_dev_tasks(id);

alter table if exists agent_remote_dev_artifacts
  add constraint agent_remote_dev_artifacts_thread_fk
  foreign key (thread_id) references agent_remote_dev_threads(id);

alter table if exists agent_remote_dev_runtime_locks
  add constraint agent_remote_dev_runtime_locks_thread_fk
  foreign key (thread_id) references agent_remote_dev_threads(id);

-- ─────────────────────────────────────────────────────────────────────────────
-- Conversation presence: durable membership for the websocket presence
-- service. The BEAM-side in-memory registry caches the subset of these rows
-- relevant to connections currently on the local node and rebuilds itself
-- from these tables after a process or node restart.
-- ─────────────────────────────────────────────────────────────────────────────

create table if not exists presence_convs (
  id uuid primary key default gen_random_uuid(),
  slug varchar(120) not null,
  display_name varchar(200) default '' not null,
  status varchar(32) default 'active' not null,
  meta_data jsonb default '{}'::jsonb not null,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint presence_convs_slug_format_chk
    check (slug ~ '^[A-Za-z0-9._:/-]{1,120}$'),
  constraint presence_convs_display_name_size_chk
    check (octet_length(display_name) <= 200),
  constraint presence_convs_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object'),
  constraint presence_convs_status_chk
    check (status in ('active', 'paused', 'archived'))
);

create unique index if not exists presence_convs_slug_active_uq
  on presence_convs (slug)
  where is_soft_deleted = false;

create index if not exists presence_convs_status_idx
  on presence_convs (status)
  where is_soft_deleted = false;

create index if not exists presence_convs_updated_at_idx
  on presence_convs (updated_at desc)
  where is_soft_deleted = false;

create table if not exists presence_conv_members (
  id uuid primary key default gen_random_uuid(),
  conv_id uuid not null,
  user_id uuid not null,
  role varchar(32) default 'member' not null,
  status varchar(32) default 'active' not null,
  meta_data jsonb default '{}'::jsonb not null,
  is_soft_deleted boolean default false not null,
  joined_at timestamptz default now() not null,
  left_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint presence_conv_members_role_chk
    check (role in ('owner', 'admin', 'member', 'guest', 'bot')),
  constraint presence_conv_members_status_chk
    check (status in ('active', 'muted', 'banned', 'archived')),
  constraint presence_conv_members_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists presence_conv_members_conv_user_active_uq
  on presence_conv_members (conv_id, user_id)
  where is_soft_deleted = false;

create index if not exists presence_conv_members_user_id_idx
  on presence_conv_members (user_id)
  where is_soft_deleted = false;

create index if not exists presence_conv_members_conv_id_idx
  on presence_conv_members (conv_id)
  where is_soft_deleted = false;

create index if not exists presence_conv_members_updated_at_idx
  on presence_conv_members (updated_at desc)
  where is_soft_deleted = false;

alter table if exists presence_conv_members
  add constraint presence_conv_members_conv_fk
  foreign key (conv_id) references presence_convs(id);

-- ────────────────────────────────────────────────────────────────────────
-- Presence membership change pipeline — fast LISTEN/NOTIFY + durable
-- outbox.
--
-- We push every membership change down TWO parallel paths so that no
-- single failure mode (LISTEN session drop, pod restart, NOTIFY queue
-- saturation, transient partition) can lose a delivery:
--
--   1. FAST PATH — sharded LISTEN/NOTIFY on TWO axes per change:
--        * presence_change_conv_<shard(conv_id)>
--        * presence_change_user_<shard(user_id)>
--      Sub-millisecond push, payload-bearing, fire-and-forget: PG drops
--      the notification if nobody is LISTENing at the instant of commit.
--      Each pod LISTENs on (a) shards for convs it has local conv-ws's
--      in and (b) shards for users it has local user-ws's for. With
--      both axes, a pod that has only Alice's user-ws still gets
--      notifications when Alice is added to a brand new conv on a
--      shard the pod otherwise wouldn't be listening to.
--
--   2. DURABLE PATH — outbox table `presence_events` with monotonic
--      bigserial `seq`. The trigger appends a row inside the same
--      transaction as the membership write, so the row is committed iff
--      the membership change is committed. Consumers (one per pod,
--      `pg_outbox.gleam`) remember the last `seq` they processed and
--      poll for `seq > last AND (conv_shard ∈ S_c OR user_shard ∈ S_u)`.
--      That gives us replay across pod restarts, LISTEN reconnects,
--      and NOTIFY-queue overflows without needing logical replication
--      slots, `wal_level = logical`, or superuser. Same guarantees as a
--      WAL CDC consumer; pure SQL.
--
-- Why both:
--   The fast path keeps p50 latency at sub-ms for the common case (pod
--   is connected, LISTEN is open). The durable path catches the gaps
--   (LISTEN was reconnecting, pod was rotating, NOTIFY queue overflowed
--   before consumer drained it). The conversations actor dedupes on
--   `(op, conv_id, user_id, kind)` within a 500ms window so anything
--   arriving via both paths collapses to a single dispatch.
--
-- Shard arithmetic (deliberately portable, NOT hashtext):
--   shard = ('x' || first_4_hex_chars(uuid))::bit(16)::int % N
--   N defaults to 256; override per-database with
--     ALTER DATABASE mydb SET presence.notify_shards = 64;
--   The Erlang side (`pg_listen.shard_of`) computes the same value so
--   subscribers find the right channel without a round-trip.
--
-- Retention:
--   `presence_events` rows live as long as needed for replay. In
--   production, a daily job should delete rows older than 24-72 hours
--   (pick based on max plausible consumer downtime). Out of scope for
--   the schema itself.
-- ────────────────────────────────────────────────────────────────────────

create or replace function presence_notify_shards()
returns int
language plpgsql
stable
as $$
declare
  v_raw text;
  v_n int;
begin
  v_raw := current_setting('presence.notify_shards', true);
  if v_raw is null or v_raw = '' then
    return 256;
  end if;
  begin
    v_n := v_raw::int;
  exception when others then
    return 256;
  end;
  if v_n is null or v_n < 1 then
    return 256;
  end if;
  return v_n;
end;
$$;

-- Convert any text identifier into a stable UUID. Real UUIDs (matching
-- the canonical 8-4-4-4-12 hex pattern) pass through unchanged; any
-- other string is hashed via md5 so demo data ("conv-1", "alice") and
-- production data (real uuid()s) both flow through the same column type.
-- The mapping is deterministic so repeated calls with the same input
-- always produce the same UUID — a "user upsert" stays idempotent.
create or replace function presence_to_uuid(p text)
returns uuid
language plpgsql
immutable
as $$
begin
  -- Cheap regex check first so the common case (real UUID input) doesn't
  -- pay the md5 cost.
  if p ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$' then
    return p::uuid;
  end if;
  return md5(p)::uuid;
end;
$$;

-- ── Slug lookup for round-tripping ──────────────────────────────────────
-- presence_convs already stores `slug` so conv-id round-trips. Users
-- don't have a dedicated table elsewhere, so we keep a thin one here
-- purely for slug ↔ UUID lookup. Production code that already uses real
-- UUIDs everywhere can ignore this — the slug column will simply equal
-- the UUID's text form.

create table if not exists presence_users (
  id uuid primary key,
  slug text not null,
  updated_at timestamptz not null default now()
);

create unique index if not exists presence_users_slug_uq on presence_users (slug);

create or replace function presence_user_upsert(p_slug text)
returns uuid
language sql
as $$
  insert into presence_users (id, slug, updated_at)
  values (presence_to_uuid(p_slug), p_slug, now())
  on conflict (id) do update set updated_at = excluded.updated_at
  returning id;
$$;

-- ── Outbox table ────────────────────────────────────────────────────────
-- One row per membership change. `seq` is the monotonic stream cursor;
-- consumers persist their position so they can resume after restart.
-- `conv_shard` / `user_shard` are precomputed so `pg_outbox` can filter
-- by index without recomputing the hash for every row.

create table if not exists presence_events (
  seq bigserial primary key,
  event_at timestamptz not null default now(),
  op text not null,
  conv_id uuid not null,
  user_id uuid not null,
  -- Mirror of the UUIDs into the consumer-facing identifier the rest of
  -- the system uses. Production code that always uses real UUIDs will
  -- see conv_slug = conv_id::text; demo / human-readable IDs round-trip
  -- through presence_convs.slug / presence_users.slug.
  conv_slug text not null,
  user_slug text not null,
  conv_shard integer not null,
  user_shard integer not null,
  soft_deleted boolean not null default false,
  constraint presence_events_op_chk
    check (op in ('INSERT', 'UPDATE', 'DELETE'))
);

create index if not exists presence_events_conv_shard_seq_idx
  on presence_events (conv_shard, seq);

create index if not exists presence_events_user_shard_seq_idx
  on presence_events (user_shard, seq);

create index if not exists presence_events_event_at_idx
  on presence_events (event_at);

-- ── Trigger ─────────────────────────────────────────────────────────────
-- 1. Insert into outbox (gets `seq`)
-- 2. NOTIFY conv-shard channel with full payload including `seq`
-- 3. NOTIFY user-shard channel with same payload
-- All three happen in the same transaction.

create or replace function notify_presence_member_change()
returns trigger
language plpgsql
as $$
declare
  v_op text := tg_op;
  v_conv_uuid uuid := coalesce(new.conv_id, old.conv_id);
  v_user_uuid uuid := coalesce(new.user_id, old.user_id);
  v_soft boolean := coalesce(new.is_soft_deleted, old.is_soft_deleted, false);
  v_shards int := presence_notify_shards();
  v_conv_shard int;
  v_user_shard int;
  v_seq bigint;
  v_payload text;
  -- Resolve UUID → slug so downstream consumers see whichever
  -- identifier the application uses everywhere else (the slug is the
  -- ws routing key, cache key, etc.). Falls back to the UUID text if
  -- no slug row exists.
  v_conv_slug text;
  v_user_slug text;
begin
  v_conv_shard := (('x' || substring(replace(v_conv_uuid::text, '-', ''), 1, 4))
                   ::bit(16)::int % v_shards);
  v_user_shard := (('x' || substring(replace(v_user_uuid::text, '-', ''), 1, 4))
                   ::bit(16)::int % v_shards);

  select slug into v_conv_slug from presence_convs where id = v_conv_uuid;
  select slug into v_user_slug from presence_users where id = v_user_uuid;
  v_conv_slug := coalesce(v_conv_slug, v_conv_uuid::text);
  v_user_slug := coalesce(v_user_slug, v_user_uuid::text);

  -- Outbox stores BOTH the UUID (for FK / strict typing) and the slug
  -- (for downstream consumers that key off the human-readable IDs).
  insert into presence_events
    (op, conv_id, user_id, conv_slug, user_slug,
     conv_shard, user_shard, soft_deleted)
    values (v_op, v_conv_uuid, v_user_uuid, v_conv_slug, v_user_slug,
            v_conv_shard, v_user_shard, v_soft)
    returning seq into v_seq;

  v_payload := json_build_object(
    'op',           v_op,
    'conv_id',      v_conv_slug,
    'user_id',      v_user_slug,
    'soft_deleted', v_soft,
    'conv_shard',   v_conv_shard,
    'user_shard',   v_user_shard,
    'seq',          v_seq,
    'emitted_at',   extract(epoch from clock_timestamp())
  )::text;

  perform pg_notify('presence_change_conv_' || v_conv_shard::text, v_payload);
  perform pg_notify('presence_change_user_' || v_user_shard::text, v_payload);

  return coalesce(new, old);
end;
$$;

drop trigger if exists presence_conv_members_notify on presence_conv_members;

create trigger presence_conv_members_notify
  after insert or update or delete on presence_conv_members
  for each row
  execute function notify_presence_member_change();

-- ── Shard helper exposed to clients ─────────────────────────────────────
-- Canonical reference for the shard algorithm. The Erlang side re-
-- implements the same UUID-prefix math locally to avoid a roundtrip on
-- every subscribe; this function is the cross-check escape hatch.

create or replace function presence_shard_of(p_id uuid)
returns int
language sql
stable
as $$
  select (('x' || substring(replace(p_id::text, '-', ''), 1, 4))
          ::bit(16)::int % presence_notify_shards());
$$;

-- ── Per-consumer checkpoint table ───────────────────────────────────────
-- Each pod (`consumer_id` = erlang node name) records the highest `seq`
-- it has processed. On startup, `pg_outbox.gleam` reads this to know
-- where to resume from. Survives pod restart; reset to NULL to force a
-- full replay of whatever is still in `presence_events`.

create table if not exists presence_consumer_checkpoints (
  consumer_id text primary key,
  last_seq bigint not null default 0,
  updated_at timestamptz not null default now()
);

-- ── WAL / logical replication helpers ───────────────────────────────────
--
-- The opt-in pg_wal.gleam consumer wants TWO Postgres-side artefacts:
--
--   1. A PUBLICATION listing the tables we care about. Logical
--      replication only ships row changes for tables included in some
--      publication, so the publication doubles as our allow-list.
--
--   2. A LOGICAL REPLICATION SLOT per consumer. Each slot is a named
--      cursor over WAL — Postgres retains WAL until the slot's
--      `confirmed_flush_lsn` advances past it. We use the `wal2json`
--      output plugin so the consumer reads JSON directly from
--      `pg_logical_slot_get_changes(...)` via the regular SQL pool,
--      instead of speaking the streaming replication protocol.
--
-- Prereqs (NOT created here):
--   * `wal_level = logical` (RDS: rds.logical_replication = 1)
--   * `wal2json` extension installed (RDS supports `CREATE EXTENSION
--     wal2json;` directly)
--   * `max_replication_slots` ≥ #pods that will run pg_wal
--   * `max_slot_wal_keep_size` set, OR an alert on slot lag, to avoid
--     the classic "dead slot fills the disk" outage.
--
-- The helpers below are intentionally simple wrappers. Enable the per-pod
-- WAL consumer only for deployments that explicitly need the direct PG WAL
-- path, and pair it with `max_slot_wal_keep_size` plus slot-lag alerts; the
-- SQL outbox above remains the lower-risk durable replay path because it
-- does not retain Postgres WAL per pod.

-- `CREATE PUBLICATION IF NOT EXISTS` doesn't exist in any PG version, so
-- wrap in a DO block so re-running the schema file is idempotent.
do $$
begin
  if not exists (select 1 from pg_publication where pubname = 'presence_pub') then
    create publication presence_pub for table presence_conv_members;
  end if;
end;
$$;

-- Idempotent slot bootstrap. Returns true when the slot exists at end of
-- call (either pre-existing or freshly created). Returns false if the
-- creation failed — most commonly because the requested output plugin
-- isn't installed on the server. Designed to be called by the consumer
-- on startup as a one-shot `SELECT`. It deliberately raises any other
-- error (e.g. transaction restrictions) so the Gleam side surfaces them.
--
-- Note: `pg_create_logical_replication_slot` cannot run inside a wrapped
-- PL/pgSQL block in older PG versions; in PG16 it's allowed at the top
-- level of a function provided the surrounding txn hasn't done writes.
-- The consumer calls this as the FIRST statement on a fresh pooled
-- connection so that constraint is satisfied.
create or replace function presence_ensure_wal_slot(p_slot_name text)
returns boolean
language plpgsql
as $$
declare
  v_exists boolean;
begin
  select exists(
    select 1 from pg_replication_slots where slot_name = p_slot_name
  ) into v_exists;
  if v_exists then
    return true;
  end if;
  begin
    perform pg_create_logical_replication_slot(p_slot_name, 'wal2json');
    return true;
  exception when others then
    return false;
  end;
end;
$$;

-- Coarse-grained precondition check: `wal_level` is the only thing we
-- can verify cheaply from inside a PL/pgSQL function (slot creation
-- can't happen inside a transaction). The Gleam side calls
-- `presence_ensure_wal_slot()` directly and treats any error as "WAL
-- decoding not actually available on this server" — typically because
-- `wal2json` (or whichever output plugin we asked for) isn't installed.
create or replace function presence_wal_available()
returns boolean
language sql
stable
as $$
  select current_setting('wal_level') = 'logical';
$$;

-- ─────────────────────────────────────────────────────────────────────────────
-- DES soccer self-play learning.
--
-- Immutable policy versions plus per-simulation deltas are the durable learning
-- authority. JSONB stores high-dimensional state/config payloads, while merge
-- math uses fixed-point integer columns so generated adapters stay portable.
-- Values and weights use a 1e6 scale.
-- ─────────────────────────────────────────────────────────────────────────────

create table if not exists des_soccer_learning_experiments (
  id uuid primary key default gen_random_uuid(),
  slug varchar(160) not null,
  display_name varchar(240) not null,
  description text default '' not null,
  status varchar(32) default 'active' not null,
  config jsonb default '{}'::jsonb not null,
  labels jsonb default '[]'::jsonb not null,
  meta_data jsonb default '{}'::jsonb not null,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint des_soccer_learning_experiments_slug_format_chk
    check (slug ~ '^[a-z0-9][a-z0-9._/-]{1,158}[a-z0-9]$'),
  constraint des_soccer_learning_experiments_display_name_size_chk
    check (octet_length(display_name) between 1 and 240),
  constraint des_soccer_learning_experiments_description_size_chk
    check (octet_length(description) <= 8192),
  constraint des_soccer_learning_experiments_config_object_chk
    check (jsonb_typeof(config) = 'object'),
  constraint des_soccer_learning_experiments_labels_array_chk
    check (jsonb_typeof(labels) = 'array'),
  constraint des_soccer_learning_experiments_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object'),
  constraint des_soccer_learning_experiments_status_chk
    check (status in ('active', 'paused', 'archived'))
);

create unique index if not exists des_soccer_learning_experiments_slug_active_uq
  on des_soccer_learning_experiments (slug)
  where is_soft_deleted = false;

create index if not exists des_soccer_learning_experiments_status_idx
  on des_soccer_learning_experiments (status)
  where is_soft_deleted = false;

create index if not exists des_soccer_learning_experiments_updated_at_idx
  on des_soccer_learning_experiments (updated_at desc)
  where is_soft_deleted = false;

create table if not exists des_soccer_learning_policy_versions (
  id uuid primary key default gen_random_uuid(),
  experiment_id uuid not null references des_soccer_learning_experiments(id),
  parent_policy_version_id uuid references des_soccer_learning_policy_versions(id),
  generation integer default 0 not null,
  version_label varchar(160) not null,
  source_kind varchar(40) default 'seed' not null,
  status varchar(32) default 'candidate' not null,
  options jsonb default '{}'::jsonb not null,
  config jsonb default '{}'::jsonb not null,
  lineage jsonb default '[]'::jsonb not null,
  metrics jsonb default '{}'::jsonb not null,
  entry_count integer default 0 not null,
  target_entry_count integer default 0 not null,
  visit_count bigint default 0 not null,
  fitness_micros bigint default 0 not null,
  branch_key uuid not null,
  retention_kind varchar(32) default 'branch_tip' not null,
  full_entries_retained boolean default true not null,
  full_entries_pruned_at timestamptz,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint des_soccer_learning_policy_versions_generation_chk
    check (generation >= 0),
  constraint des_soccer_learning_policy_versions_label_format_chk
    check (version_label ~ '^[A-Za-z0-9._:/-]{1,160}$'),
  constraint des_soccer_learning_policy_versions_source_chk
    check (source_kind in ('seed', 'merge', 'mutation', 'crossover', 'import', 'replay')),
  constraint des_soccer_learning_policy_versions_status_chk
    check (status in ('candidate', 'active', 'archived', 'rejected')),
  constraint des_soccer_learning_policy_versions_options_object_chk
    check (jsonb_typeof(options) = 'object'),
  constraint des_soccer_learning_policy_versions_config_object_chk
    check (jsonb_typeof(config) = 'object'),
  constraint des_soccer_learning_policy_versions_lineage_array_chk
    check (jsonb_typeof(lineage) = 'array'),
  constraint des_soccer_learning_policy_versions_metrics_object_chk
    check (jsonb_typeof(metrics) = 'object'),
  constraint des_soccer_learning_policy_versions_entry_count_chk
    check (entry_count >= 0),
  constraint des_soccer_learning_policy_versions_target_entry_count_chk
    check (target_entry_count >= 0),
  constraint des_soccer_learning_policy_versions_visit_count_chk
    check (visit_count >= 0),
  constraint des_soccer_learning_policy_versions_retention_kind_chk
    check (retention_kind in ('branch_tip', 'retain_all', 'metadata_only'))
);

create unique index if not exists des_soccer_learning_policy_versions_label_uq
  on des_soccer_learning_policy_versions (experiment_id, version_label);

create index if not exists des_soccer_learning_policy_versions_active_idx
  on des_soccer_learning_policy_versions (experiment_id, generation desc, updated_at desc)
  where status = 'active';

create index if not exists des_soccer_learning_policy_versions_fitness_idx
  on des_soccer_learning_policy_versions (experiment_id, fitness_micros desc, updated_at desc)
  where status in ('active', 'candidate');

create index if not exists des_soccer_learning_policy_versions_branch_tip_idx
  on des_soccer_learning_policy_versions (
    experiment_id,
    branch_key,
    generation desc,
    updated_at desc
  )
  where full_entries_retained = true;

-- At most one active policy version per experiment. The writer archives the
-- prior active version and inserts the new one in a single transaction; this
-- partial unique index makes that invariant durable under concurrent runners
-- (a racing second activation fails to commit instead of producing two actives).
-- Note: if a live database already holds duplicate active rows, the reviewed
-- migration must first archive all but the newest active version per experiment
-- before this index can be created.
create unique index if not exists des_soccer_learning_policy_versions_single_active_uq
  on des_soccer_learning_policy_versions (experiment_id)
  where status = 'active';

create table if not exists des_soccer_learning_policy_entries (
  id uuid primary key default gen_random_uuid(),
  policy_version_id uuid not null references des_soccer_learning_policy_versions(id),
  team varchar(8) not null,
  entry_kind varchar(16) not null,
  state_hash varchar(32) not null,
  state_key jsonb not null,
  action varchar(80) not null,
  target_fine_cell_id integer default -1 not null,
  target_tactical_cell_id integer default -1 not null,
  target_macro_cell_id integer default -1 not null,
  target_root_cell_id integer default -1 not null,
  value_micros bigint not null,
  visits integer default 0 not null,
  source_run_id uuid,
  created_at timestamptz default now() not null,
  constraint des_soccer_learning_policy_entries_team_chk
    check (team in ('home', 'away')),
  constraint des_soccer_learning_policy_entries_kind_chk
    check (entry_kind in ('action', 'target')),
  constraint des_soccer_learning_policy_entries_state_hash_chk
    check (state_hash ~ '^[a-f0-9]{16,32}$'),
  constraint des_soccer_learning_policy_entries_state_object_chk
    check (jsonb_typeof(state_key) = 'object'),
  constraint des_soccer_learning_policy_entries_action_size_chk
    check (octet_length(action) between 1 and 80),
  constraint des_soccer_learning_policy_entries_target_fine_chk
    check (target_fine_cell_id >= -1),
  constraint des_soccer_learning_policy_entries_target_tactical_chk
    check (target_tactical_cell_id >= -1),
  constraint des_soccer_learning_policy_entries_target_macro_chk
    check (target_macro_cell_id >= -1),
  constraint des_soccer_learning_policy_entries_target_root_chk
    check (target_root_cell_id >= -1),
  constraint des_soccer_learning_policy_entries_visits_chk
    check (visits >= 0)
);

create unique index if not exists des_soccer_learning_policy_entries_key_uq
  on des_soccer_learning_policy_entries (
    policy_version_id,
    team,
    entry_kind,
    state_hash,
    action,
    target_fine_cell_id,
    target_tactical_cell_id,
    target_macro_cell_id,
    target_root_cell_id
  );

create index if not exists des_soccer_learning_policy_entries_lookup_idx
  on des_soccer_learning_policy_entries (policy_version_id, team, entry_kind, state_hash);

create table if not exists des_soccer_learning_jobs (
  id uuid primary key default gen_random_uuid(),
  experiment_id uuid not null references des_soccer_learning_experiments(id),
  base_policy_version_id uuid references des_soccer_learning_policy_versions(id),
  spawn_strategy varchar(32) default 'latest' not null,
  status varchar(32) default 'queued' not null,
  priority integer default 0 not null,
  seed bigint not null,
  attempt integer default 0 not null,
  max_attempts integer default 1 not null,
  lease_owner varchar(200),
  lease_expires_at timestamptz,
  started_at timestamptz,
  finished_at timestamptz,
  config jsonb default '{}'::jsonb not null,
  runner_config jsonb default '{}'::jsonb not null,
  result_run_id uuid,
  error text,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint des_soccer_learning_jobs_spawn_strategy_chk
    check (spawn_strategy in ('latest', 'elite', 'mutation', 'crossover', 'random', 'replay')),
  constraint des_soccer_learning_jobs_status_chk
    check (status in ('queued', 'running', 'completed', 'failed', 'canceled')),
  constraint des_soccer_learning_jobs_seed_chk
    check (seed >= 0),
  constraint des_soccer_learning_jobs_attempt_chk
    check (attempt >= 0),
  constraint des_soccer_learning_jobs_max_attempts_chk
    check (max_attempts between 1 and 100),
  constraint des_soccer_learning_jobs_lease_owner_size_chk
    check (lease_owner is null or octet_length(lease_owner) <= 200),
  constraint des_soccer_learning_jobs_config_object_chk
    check (jsonb_typeof(config) = 'object'),
  constraint des_soccer_learning_jobs_runner_config_object_chk
    check (jsonb_typeof(runner_config) = 'object'),
  constraint des_soccer_learning_jobs_error_size_chk
    check (error is null or octet_length(error) <= 16384)
);

create index if not exists des_soccer_learning_jobs_claim_idx
  on des_soccer_learning_jobs (experiment_id, status, priority desc, created_at)
  where status in ('queued', 'running');

create index if not exists des_soccer_learning_jobs_base_policy_idx
  on des_soccer_learning_jobs (base_policy_version_id, created_at desc);

create table if not exists des_soccer_learning_runs (
  id uuid primary key default gen_random_uuid(),
  job_id uuid references des_soccer_learning_jobs(id),
  experiment_id uuid not null references des_soccer_learning_experiments(id),
  base_policy_version_id uuid references des_soccer_learning_policy_versions(id),
  output_policy_version_id uuid references des_soccer_learning_policy_versions(id),
  runner_id varchar(200) not null,
  seed bigint not null,
  episode_index integer default 0 not null,
  status varchar(32) default 'completed' not null,
  score_home integer default 0 not null,
  score_away integer default 0 not null,
  home_goal_diff integer default 0 not null,
  away_goal_diff integer default 0 not null,
  home_outcome varchar(16) default 'draw' not null,
  away_outcome varchar(16) default 'draw' not null,
  home_merge_weight_micros bigint default 0 not null,
  away_merge_weight_micros bigint default 0 not null,
  fitness_micros bigint default 0 not null,
  duration_ticks bigint default 0 not null,
  simulated_seconds_micros bigint default 0 not null,
  elapsed_millis bigint default 0 not null,
  transitions integer default 0 not null,
  summary jsonb default '{}'::jsonb not null,
  stats jsonb default '{}'::jsonb not null,
  error text,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint des_soccer_learning_runs_runner_id_size_chk
    check (octet_length(runner_id) between 1 and 200),
  constraint des_soccer_learning_runs_seed_chk
    check (seed >= 0),
  constraint des_soccer_learning_runs_episode_index_chk
    check (episode_index >= 0),
  constraint des_soccer_learning_runs_status_chk
    check (status in ('completed', 'failed')),
  constraint des_soccer_learning_runs_scores_chk
    check (score_home >= 0 and score_away >= 0),
  constraint des_soccer_learning_runs_home_outcome_chk
    check (home_outcome in ('win', 'draw', 'loss')),
  constraint des_soccer_learning_runs_away_outcome_chk
    check (away_outcome in ('win', 'draw', 'loss')),
  constraint des_soccer_learning_runs_duration_ticks_chk
    check (duration_ticks >= 0),
  constraint des_soccer_learning_runs_simulated_seconds_chk
    check (simulated_seconds_micros >= 0),
  constraint des_soccer_learning_runs_elapsed_millis_chk
    check (elapsed_millis >= 0),
  constraint des_soccer_learning_runs_transitions_chk
    check (transitions >= 0),
  constraint des_soccer_learning_runs_summary_object_chk
    check (jsonb_typeof(summary) = 'object'),
  constraint des_soccer_learning_runs_stats_object_chk
    check (jsonb_typeof(stats) = 'object'),
  constraint des_soccer_learning_runs_error_size_chk
    check (error is null or octet_length(error) <= 16384)
);

create index if not exists des_soccer_learning_runs_experiment_idx
  on des_soccer_learning_runs (experiment_id, created_at desc);

create index if not exists des_soccer_learning_runs_policy_fitness_idx
  on des_soccer_learning_runs (base_policy_version_id, fitness_micros desc, created_at desc);

create table if not exists des_soccer_learning_run_deltas (
  id uuid primary key default gen_random_uuid(),
  run_id uuid not null references des_soccer_learning_runs(id),
  team varchar(8) not null,
  entry_kind varchar(16) not null,
  state_hash varchar(32) not null,
  state_key jsonb not null,
  action varchar(80) not null,
  target_fine_cell_id integer default -1 not null,
  target_tactical_cell_id integer default -1 not null,
  target_macro_cell_id integer default -1 not null,
  target_root_cell_id integer default -1 not null,
  before_value_micros bigint default 0 not null,
  after_value_micros bigint default 0 not null,
  value_delta_micros bigint default 0 not null,
  visit_delta integer default 0 not null,
  merge_weight_micros bigint default 0 not null,
  effective_visit_micros bigint default 0 not null,
  created_at timestamptz default now() not null,
  constraint des_soccer_learning_run_deltas_team_chk
    check (team in ('home', 'away')),
  constraint des_soccer_learning_run_deltas_kind_chk
    check (entry_kind in ('action', 'target')),
  constraint des_soccer_learning_run_deltas_state_hash_chk
    check (state_hash ~ '^[a-f0-9]{16,32}$'),
  constraint des_soccer_learning_run_deltas_state_object_chk
    check (jsonb_typeof(state_key) = 'object'),
  constraint des_soccer_learning_run_deltas_action_size_chk
    check (octet_length(action) between 1 and 80),
  constraint des_soccer_learning_run_deltas_target_fine_chk
    check (target_fine_cell_id >= -1),
  constraint des_soccer_learning_run_deltas_target_tactical_chk
    check (target_tactical_cell_id >= -1),
  constraint des_soccer_learning_run_deltas_target_macro_chk
    check (target_macro_cell_id >= -1),
  constraint des_soccer_learning_run_deltas_target_root_chk
    check (target_root_cell_id >= -1),
  constraint des_soccer_learning_run_deltas_visit_delta_chk
    check (visit_delta > 0),
  constraint des_soccer_learning_run_deltas_merge_weight_chk
    check (merge_weight_micros >= 0),
  constraint des_soccer_learning_run_deltas_effective_visit_chk
    check (effective_visit_micros >= 0)
);

create unique index if not exists des_soccer_learning_run_deltas_key_uq
  on des_soccer_learning_run_deltas (
    run_id,
    team,
    entry_kind,
    state_hash,
    action,
    target_fine_cell_id,
    target_tactical_cell_id,
    target_macro_cell_id,
    target_root_cell_id
  );

create index if not exists des_soccer_learning_run_deltas_merge_idx
  on des_soccer_learning_run_deltas (team, entry_kind, state_hash, action);

create table if not exists des_soccer_learning_merge_events (
  id uuid primary key default gen_random_uuid(),
  experiment_id uuid not null references des_soccer_learning_experiments(id),
  base_policy_version_id uuid references des_soccer_learning_policy_versions(id),
  output_policy_version_id uuid not null references des_soccer_learning_policy_versions(id),
  strategy varchar(40) default 'outcome_weighted_average' not null,
  input_run_count integer default 0 not null,
  input_delta_count integer default 0 not null,
  decay_micros bigint default 1000000 not null,
  metrics jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  constraint des_soccer_learning_merge_events_strategy_chk
    check (strategy in ('outcome_weighted_average', 'elite', 'mutation', 'crossover')),
  constraint des_soccer_learning_merge_events_input_run_count_chk
    check (input_run_count >= 0),
  constraint des_soccer_learning_merge_events_input_delta_count_chk
    check (input_delta_count >= 0),
  constraint des_soccer_learning_merge_events_decay_chk
    check (decay_micros between 0 and 1000000),
  constraint des_soccer_learning_merge_events_metrics_object_chk
    check (jsonb_typeof(metrics) = 'object')
);

create index if not exists des_soccer_learning_merge_events_experiment_idx
  on des_soccer_learning_merge_events (experiment_id, created_at desc);

-- ─────────────────────────────────────────────────────────────────────────────
-- DES FEL elevator dispatch learning.
--
-- Durable storage for next-event elevator learning runs. Full run/policy
-- payloads stay in JSONB so the animation and non-HTML renderers can replay
-- exact artifacts, while fixed-point summary columns make RDS querying cheap
-- and generated adapters portable.
-- Values that represent seconds, rates, probabilities, or losses use a 1e6
-- scale.
-- ─────────────────────────────────────────────────────────────────────────────

create table if not exists des_fel_elevator_learning_runs (
  id uuid primary key default gen_random_uuid(),
  run_label varchar(200) not null,
  scenario_slug varchar(160) not null,
  status varchar(32) default 'completed' not null,
  dispatch_policy varchar(40) not null,
  seed bigint not null,
  floors integer not null,
  shafts integer not null,
  capacity integer not null,
  travel_seconds_micros bigint default 0 not null,
  dwell_seconds_micros bigint default 0 not null,
  arrival_rate_micros bigint default 0 not null,
  horizon_seconds_micros bigint default 0 not null,
  events bigint default 0 not null,
  arrivals bigint default 0 not null,
  boarded bigint default 0 not null,
  served bigint default 0 not null,
  mean_wait_micros bigint default 0 not null,
  dispatch_decisions integer default 0 not null,
  pomdp_belief_updates integer default 0 not null,
  online_learning_updates bigint default 0 not null,
  online_learning_loss_last_micros bigint,
  config jsonb default '{}'::jsonb not null,
  metrics jsonb default '{}'::jsonb not null,
  artifact jsonb not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  constraint des_fel_elevator_learning_runs_label_size_chk
    check (octet_length(run_label) between 1 and 200),
  constraint des_fel_elevator_learning_runs_scenario_format_chk
    check (scenario_slug ~ '^[a-z0-9][a-z0-9._/-]{1,158}[a-z0-9]$'),
  constraint des_fel_elevator_learning_runs_status_chk
    check (status in ('completed', 'failed', 'imported')),
  constraint des_fel_elevator_learning_runs_policy_chk
    check (dispatch_policy in ('look', 'mdp-table', 'neural-scorer', 'pomdp-belief', 'neural-td')),
  constraint des_fel_elevator_learning_runs_seed_chk
    check (seed >= 0),
  constraint des_fel_elevator_learning_runs_dimensions_chk
    check (floors between 2 and 256 and shafts between 1 and 128 and capacity between 1 and 10000),
  constraint des_fel_elevator_learning_runs_time_chk
    check (
      travel_seconds_micros >= 0
      and dwell_seconds_micros >= 0
      and arrival_rate_micros >= 0
      and horizon_seconds_micros >= 0
    ),
  constraint des_fel_elevator_learning_runs_counts_chk
    check (
      events >= 0
      and arrivals >= 0
      and boarded >= 0
      and served >= 0
      and mean_wait_micros >= 0
      and dispatch_decisions >= 0
      and pomdp_belief_updates >= 0
      and online_learning_updates >= 0
    ),
  constraint des_fel_elevator_learning_runs_loss_chk
    check (online_learning_loss_last_micros is null or online_learning_loss_last_micros >= 0),
  constraint des_fel_elevator_learning_runs_config_object_chk
    check (jsonb_typeof(config) = 'object'),
  constraint des_fel_elevator_learning_runs_metrics_object_chk
    check (jsonb_typeof(metrics) = 'object'),
  constraint des_fel_elevator_learning_runs_artifact_object_chk
    check (jsonb_typeof(artifact) = 'object')
);

create index if not exists des_fel_elevator_learning_runs_scenario_idx
  on des_fel_elevator_learning_runs (scenario_slug, created_at desc);

create index if not exists des_fel_elevator_learning_runs_policy_idx
  on des_fel_elevator_learning_runs (dispatch_policy, created_at desc);

create index if not exists des_fel_elevator_learning_runs_mean_wait_idx
  on des_fel_elevator_learning_runs (scenario_slug, mean_wait_micros asc, created_at desc);

create table if not exists des_fel_elevator_policy_states (
  id uuid primary key default gen_random_uuid(),
  run_id uuid not null references des_fel_elevator_learning_runs(id),
  policy_kind varchar(40) not null,
  source_kind varchar(40) default 'run-final' not null,
  feature_dim integer default 0 not null,
  output_dim integer default 0 not null,
  parameter_count integer default 0 not null,
  online_learning_updates bigint default 0 not null,
  loss_history jsonb default '[]'::jsonb not null,
  state jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  constraint des_fel_elevator_policy_states_policy_chk
    check (policy_kind in ('look', 'mdp-table', 'neural-scorer', 'pomdp-belief', 'neural-td')),
  constraint des_fel_elevator_policy_states_source_chk
    check (source_kind in ('run-final', 'offline-training', 'import', 'checkpoint')),
  constraint des_fel_elevator_policy_states_dims_chk
    check (feature_dim >= 0 and output_dim >= 0 and parameter_count >= 0 and online_learning_updates >= 0),
  constraint des_fel_elevator_policy_states_loss_array_chk
    check (jsonb_typeof(loss_history) = 'array'),
  constraint des_fel_elevator_policy_states_state_object_chk
    check (jsonb_typeof(state) = 'object')
);

create unique index if not exists des_fel_elevator_policy_states_run_source_uq
  on des_fel_elevator_policy_states (run_id, source_kind, policy_kind);

create index if not exists des_fel_elevator_policy_states_policy_idx
  on des_fel_elevator_policy_states (policy_kind, created_at desc);

create table if not exists des_fel_elevator_dispatch_decisions (
  id uuid primary key default gen_random_uuid(),
  run_id uuid not null references des_fel_elevator_learning_runs(id),
  decision_index integer not null,
  sim_time_micros bigint default 0 not null,
  call_floor integer not null,
  car_index integer not null,
  policy_kind varchar(40) not null,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  constraint des_fel_elevator_dispatch_decisions_policy_chk
    check (policy_kind in ('look', 'mdp-table', 'neural-scorer', 'pomdp-belief', 'neural-td')),
  constraint des_fel_elevator_dispatch_decisions_index_chk
    check (decision_index >= 0),
  constraint des_fel_elevator_dispatch_decisions_time_chk
    check (sim_time_micros >= 0),
  constraint des_fel_elevator_dispatch_decisions_floor_car_chk
    check (call_floor >= 0 and car_index >= 0),
  constraint des_fel_elevator_dispatch_decisions_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists des_fel_elevator_dispatch_decisions_run_index_uq
  on des_fel_elevator_dispatch_decisions (run_id, decision_index);

create index if not exists des_fel_elevator_dispatch_decisions_time_idx
  on des_fel_elevator_dispatch_decisions (run_id, sim_time_micros);

create table if not exists des_fel_elevator_pomdp_beliefs (
  id uuid primary key default gen_random_uuid(),
  run_id uuid not null references des_fel_elevator_learning_runs(id),
  belief_index integer not null,
  sim_time_micros bigint default 0 not null,
  floor integer not null,
  action varchar(32) not null,
  observation varchar(32) not null,
  empty_prob_micros integer default 0 not null,
  waiting_prob_micros integer default 0 not null,
  crowded_prob_micros integer default 0 not null,
  belief jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  constraint des_fel_elevator_pomdp_beliefs_index_chk
    check (belief_index >= 0),
  constraint des_fel_elevator_pomdp_beliefs_time_floor_chk
    check (sim_time_micros >= 0 and floor >= 0),
  constraint des_fel_elevator_pomdp_beliefs_action_chk
    check (action in ('hold', 'dispatch')),
  constraint des_fel_elevator_pomdp_beliefs_observation_chk
    check (observation in ('quiet', 'call')),
  constraint des_fel_elevator_pomdp_beliefs_prob_chk
    check (
      empty_prob_micros between 0 and 1000000
      and waiting_prob_micros between 0 and 1000000
      and crowded_prob_micros between 0 and 1000000
    ),
  constraint des_fel_elevator_pomdp_beliefs_belief_object_chk
    check (jsonb_typeof(belief) = 'object')
);

create unique index if not exists des_fel_elevator_pomdp_beliefs_run_index_uq
  on des_fel_elevator_pomdp_beliefs (run_id, belief_index);

create index if not exists des_fel_elevator_pomdp_beliefs_floor_time_idx
  on des_fel_elevator_pomdp_beliefs (run_id, floor, sim_time_micros);

-- ─────────────────────────────────────────────────────────────────────────────
-- Generic CDC gateway publication.
--
-- The `wal-gateway-rs` service runs ONE logical replication slot per cluster
-- (leader-elected via a Postgres advisory lock) and fans the resulting
-- row-change events out over NATS JetStream subjects shaped like
-- `cdc.<schema>.<table>.<op>`. Application services subscribe via the
-- `dd-wal-consumer` crate; they never see Postgres on the read path.
--
-- This publication is intentionally narrow: only tables that other services
-- read but rarely (or never) write are listed here. `presence_conv_members`
-- has its own `presence_pub` because the presence server runs a *separate*
-- per-pod consumer with sharded routing. We deliberately keep the two
-- streams apart so a busy presence pipeline can't starve the slow-moving
-- config CDC stream.
--
-- Adding a table to the gateway means adding it to the create-publication list
-- and the idempotent `alter publication ... add table` guard below; new
-- subscribers do not require schema changes.
-- ─────────────────────────────────────────────────────────────────────────────

do $$
begin
  if not exists (select 1 from pg_publication where pubname = 'cdc_pub') then
    create publication cdc_pub for table
      app_config,
      vapi_phone_call_events,
      container_pool_configs,
      lambda_functions,
      known_git_repos,
      agent_remote_dev_events;
  end if;

  if not exists (
    select 1
    from pg_publication_tables
    where pubname = 'cdc_pub'
      and schemaname = 'public'
      and tablename = 'agent_remote_dev_events'
  ) then
    alter publication cdc_pub add table agent_remote_dev_events;
  end if;

  if not exists (
    select 1
    from pg_publication_tables
    where pubname = 'cdc_pub'
      and schemaname = 'public'
      and tablename = 'vapi_phone_call_events'
  ) then
    alter publication cdc_pub add table vapi_phone_call_events;
  end if;
end;
$$;

-- Idempotent slot bootstrap for the gateway. Mirrors `presence_ensure_wal_slot`
-- but generic over the plugin so future deploys can swap to `pgoutput` or
-- `test_decoding` without a schema change. Returns true when the slot exists
-- at end of call. Returns false if creation failed (typically because the
-- requested output plugin isn't installed).
create or replace function cdc_ensure_wal_slot(p_slot_name text, p_plugin text default 'wal2json')
returns boolean
language plpgsql
as $$
declare
  v_exists boolean;
begin
  select exists(
    select 1 from pg_replication_slots where slot_name = p_slot_name
  ) into v_exists;
  if v_exists then
    return true;
  end if;
  begin
    perform pg_create_logical_replication_slot(p_slot_name, p_plugin);
    return true;
  exception when others then
    return false;
  end;
end;
$$;

-- Same coarse precondition check as `presence_wal_available`. Kept as a
-- separate symbol so the gateway code doesn't depend on presence_* names.
create or replace function cdc_wal_available()
returns boolean
language sql
stable
as $$
  select current_setting('wal_level') = 'logical';
$$;

-- Slot lag introspection. Exposed for the gateway's /healthz and for
-- ops dashboards. Returns NULL when no slot with the given name exists.
create or replace function cdc_slot_lag_bytes(p_slot_name text)
returns bigint
language sql
stable
as $$
  select case
    when slot is null then null
    else pg_wal_lsn_diff(pg_current_wal_lsn(), slot.confirmed_flush_lsn)
  end
  from (
    select * from pg_replication_slots where slot_name = p_slot_name
  ) slot;
$$;

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

alter table if exists benefactor_marketing_contacts
  add constraint benefactor_marketing_contacts_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

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

alter table if exists benefactor_marketing_contracts
  add constraint benefactor_marketing_contracts_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_contracts
  add constraint benefactor_marketing_contracts_package_fk
  foreign key (package_id) references benefactor_marketing_service_packages(id);

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

alter table if exists benefactor_marketing_invoices
  add constraint benefactor_marketing_invoices_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_invoices
  add constraint benefactor_marketing_invoices_contract_fk
  foreign key (contract_id) references benefactor_marketing_contracts(id);

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

alter table if exists benefactor_marketing_integrations
  add constraint benefactor_marketing_integrations_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

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

alter table if exists benefactor_marketing_leads
  add constraint benefactor_marketing_leads_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_leads
  add constraint benefactor_marketing_leads_source_integration_fk
  foreign key (source_integration_id) references benefactor_marketing_integrations(id);

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

alter table if exists benefactor_marketing_enrichment_jobs
  add constraint benefactor_marketing_enrichment_jobs_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_enrichment_jobs
  add constraint benefactor_marketing_enrichment_jobs_lead_fk
  foreign key (lead_id) references benefactor_marketing_leads(id);

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

alter table if exists benefactor_marketing_campaigns
  add constraint benefactor_marketing_campaigns_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

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

alter table if exists benefactor_marketing_campaign_channels
  add constraint benefactor_marketing_campaign_channels_campaign_fk
  foreign key (campaign_id) references benefactor_marketing_campaigns(id);

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

alter table if exists benefactor_marketing_campaign_experiments
  add constraint benefactor_marketing_campaign_experiments_campaign_fk
  foreign key (campaign_id) references benefactor_marketing_campaigns(id);

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

alter table if exists benefactor_marketing_automation_workflows
  add constraint benefactor_marketing_automation_workflows_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

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

alter table if exists benefactor_marketing_automation_events
  add constraint benefactor_marketing_automation_events_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_automation_events
  add constraint benefactor_marketing_automation_events_workflow_fk
  foreign key (workflow_id) references benefactor_marketing_automation_workflows(id);

alter table if exists benefactor_marketing_automation_events
  add constraint benefactor_marketing_automation_events_lead_fk
  foreign key (lead_id) references benefactor_marketing_leads(id);

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

alter table if exists benefactor_marketing_reports
  add constraint benefactor_marketing_reports_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_reports
  add constraint benefactor_marketing_reports_campaign_fk
  foreign key (campaign_id) references benefactor_marketing_campaigns(id);

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

alter table if exists benefactor_marketing_attribution_events
  add constraint benefactor_marketing_attribution_events_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_attribution_events
  add constraint benefactor_marketing_attribution_events_campaign_fk
  foreign key (campaign_id) references benefactor_marketing_campaigns(id);

alter table if exists benefactor_marketing_attribution_events
  add constraint benefactor_marketing_attribution_events_lead_fk
  foreign key (lead_id) references benefactor_marketing_leads(id);

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

alter table if exists benefactor_marketing_opportunities
  add constraint benefactor_marketing_opportunities_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_opportunities
  add constraint benefactor_marketing_opportunities_lead_fk
  foreign key (lead_id) references benefactor_marketing_leads(id);

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

alter table if exists benefactor_marketing_content_assets
  add constraint benefactor_marketing_content_assets_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_content_assets
  add constraint benefactor_marketing_content_assets_campaign_fk
  foreign key (campaign_id) references benefactor_marketing_campaigns(id);

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

alter table if exists benefactor_marketing_project_tasks
  add constraint benefactor_marketing_project_tasks_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_project_tasks
  add constraint benefactor_marketing_project_tasks_campaign_fk
  foreign key (campaign_id) references benefactor_marketing_campaigns(id);

alter table if exists benefactor_marketing_project_tasks
  add constraint benefactor_marketing_project_tasks_content_asset_fk
  foreign key (content_asset_id) references benefactor_marketing_content_assets(id);

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

alter table if exists benefactor_marketing_client_approvals
  add constraint benefactor_marketing_client_approvals_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_client_approvals
  add constraint benefactor_marketing_client_approvals_campaign_fk
  foreign key (campaign_id) references benefactor_marketing_campaigns(id);

alter table if exists benefactor_marketing_client_approvals
  add constraint benefactor_marketing_client_approvals_content_asset_fk
  foreign key (content_asset_id) references benefactor_marketing_content_assets(id);

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

alter table if exists benefactor_marketing_tickets
  add constraint benefactor_marketing_tickets_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

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

alter table if exists benefactor_marketing_meetings
  add constraint benefactor_marketing_meetings_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_meetings
  add constraint benefactor_marketing_meetings_lead_fk
  foreign key (lead_id) references benefactor_marketing_leads(id);

alter table if exists benefactor_marketing_meetings
  add constraint benefactor_marketing_meetings_opportunity_fk
  foreign key (opportunity_id) references benefactor_marketing_opportunities(id);

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

alter table if exists benefactor_marketing_team_allocations
  add constraint benefactor_marketing_team_allocations_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_team_allocations
  add constraint benefactor_marketing_team_allocations_campaign_fk
  foreign key (campaign_id) references benefactor_marketing_campaigns(id);

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

alter table if exists benefactor_marketing_integration_sync_runs
  add constraint benefactor_marketing_integration_sync_runs_integration_fk
  foreign key (integration_id) references benefactor_marketing_integrations(id);

alter table if exists benefactor_marketing_integration_sync_runs
  add constraint benefactor_marketing_integration_sync_runs_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

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

alter table if exists benefactor_marketing_outreach_sequences
  add constraint benefactor_marketing_outreach_sequences_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_outreach_sequences
  add constraint benefactor_marketing_outreach_sequences_campaign_fk
  foreign key (campaign_id) references benefactor_marketing_campaigns(id);

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

alter table if exists benefactor_marketing_outreach_steps
  add constraint benefactor_marketing_outreach_steps_sequence_fk
  foreign key (sequence_id) references benefactor_marketing_outreach_sequences(id);

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

alter table if exists benefactor_marketing_outreach_enrollments
  add constraint benefactor_marketing_outreach_enrollments_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_outreach_enrollments
  add constraint benefactor_marketing_outreach_enrollments_sequence_fk
  foreign key (sequence_id) references benefactor_marketing_outreach_sequences(id);

alter table if exists benefactor_marketing_outreach_enrollments
  add constraint benefactor_marketing_outreach_enrollments_lead_fk
  foreign key (lead_id) references benefactor_marketing_leads(id);

alter table if exists benefactor_marketing_outreach_enrollments
  add constraint benefactor_marketing_outreach_enrollments_contact_fk
  foreign key (contact_id) references benefactor_marketing_contacts(id);

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

alter table if exists benefactor_marketing_outreach_touchpoints
  add constraint benefactor_marketing_outreach_touchpoints_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_outreach_touchpoints
  add constraint benefactor_marketing_outreach_touchpoints_sequence_fk
  foreign key (sequence_id) references benefactor_marketing_outreach_sequences(id);

alter table if exists benefactor_marketing_outreach_touchpoints
  add constraint benefactor_marketing_outreach_touchpoints_enrollment_fk
  foreign key (enrollment_id) references benefactor_marketing_outreach_enrollments(id);

alter table if exists benefactor_marketing_outreach_touchpoints
  add constraint benefactor_marketing_outreach_touchpoints_campaign_fk
  foreign key (campaign_id) references benefactor_marketing_campaigns(id);

alter table if exists benefactor_marketing_outreach_touchpoints
  add constraint benefactor_marketing_outreach_touchpoints_lead_fk
  foreign key (lead_id) references benefactor_marketing_leads(id);

alter table if exists benefactor_marketing_outreach_touchpoints
  add constraint benefactor_marketing_outreach_touchpoints_contact_fk
  foreign key (contact_id) references benefactor_marketing_contacts(id);

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

alter table if exists benefactor_marketing_prospect_research_briefs
  add constraint benefactor_marketing_prospect_research_briefs_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_prospect_research_briefs
  add constraint benefactor_marketing_prospect_research_briefs_lead_fk
  foreign key (lead_id) references benefactor_marketing_leads(id);

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

alter table if exists benefactor_marketing_conversion_events
  add constraint benefactor_marketing_conversion_events_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_conversion_events
  add constraint benefactor_marketing_conversion_events_campaign_fk
  foreign key (campaign_id) references benefactor_marketing_campaigns(id);

alter table if exists benefactor_marketing_conversion_events
  add constraint benefactor_marketing_conversion_events_lead_fk
  foreign key (lead_id) references benefactor_marketing_leads(id);

alter table if exists benefactor_marketing_conversion_events
  add constraint benefactor_marketing_conversion_events_content_asset_fk
  foreign key (content_asset_id) references benefactor_marketing_content_assets(id);

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

alter table if exists benefactor_marketing_portal_members
  add constraint benefactor_marketing_portal_members_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_portal_members
  add constraint benefactor_marketing_portal_members_contact_fk
  foreign key (contact_id) references benefactor_marketing_contacts(id);

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

alter table if exists benefactor_marketing_shared_documents
  add constraint benefactor_marketing_shared_documents_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_shared_documents
  add constraint benefactor_marketing_shared_documents_campaign_fk
  foreign key (campaign_id) references benefactor_marketing_campaigns(id);

alter table if exists benefactor_marketing_shared_documents
  add constraint benefactor_marketing_shared_documents_content_asset_fk
  foreign key (content_asset_id) references benefactor_marketing_content_assets(id);

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

alter table if exists benefactor_marketing_collaboration_comments
  add constraint benefactor_marketing_collaboration_comments_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_collaboration_comments
  add constraint benefactor_marketing_collaboration_comments_parent_fk
  foreign key (parent_comment_id) references benefactor_marketing_collaboration_comments(id);

alter table if exists benefactor_marketing_collaboration_comments
  add constraint benefactor_marketing_collaboration_comments_author_contact_fk
  foreign key (author_contact_id) references benefactor_marketing_contacts(id);

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

alter table if exists benefactor_marketing_notifications
  add constraint benefactor_marketing_notifications_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_notifications
  add constraint benefactor_marketing_notifications_contact_fk
  foreign key (recipient_contact_id) references benefactor_marketing_contacts(id);

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

alter table if exists benefactor_marketing_time_entries
  add constraint benefactor_marketing_time_entries_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_time_entries
  add constraint benefactor_marketing_time_entries_campaign_fk
  foreign key (campaign_id) references benefactor_marketing_campaigns(id);

alter table if exists benefactor_marketing_time_entries
  add constraint benefactor_marketing_time_entries_task_fk
  foreign key (project_task_id) references benefactor_marketing_project_tasks(id);

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

alter table if exists benefactor_marketing_vendor_costs
  add constraint benefactor_marketing_vendor_costs_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_vendor_costs
  add constraint benefactor_marketing_vendor_costs_campaign_fk
  foreign key (campaign_id) references benefactor_marketing_campaigns(id);

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

alter table if exists benefactor_marketing_commission_entries
  add constraint benefactor_marketing_commission_entries_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_commission_entries
  add constraint benefactor_marketing_commission_entries_opportunity_fk
  foreign key (opportunity_id) references benefactor_marketing_opportunities(id);

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

alter table if exists benefactor_marketing_budget_forecasts
  add constraint benefactor_marketing_budget_forecasts_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_budget_forecasts
  add constraint benefactor_marketing_budget_forecasts_campaign_fk
  foreign key (campaign_id) references benefactor_marketing_campaigns(id);

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

alter table if exists benefactor_marketing_call_insights
  add constraint benefactor_marketing_call_insights_client_fk
  foreign key (client_id) references benefactor_marketing_clients(id);

alter table if exists benefactor_marketing_call_insights
  add constraint benefactor_marketing_call_insights_meeting_fk
  foreign key (meeting_id) references benefactor_marketing_meetings(id);

alter table if exists benefactor_marketing_call_insights
  add constraint benefactor_marketing_call_insights_lead_fk
  foreign key (lead_id) references benefactor_marketing_leads(id);

alter table if exists benefactor_marketing_call_insights
  add constraint benefactor_marketing_call_insights_opportunity_fk
  foreign key (opportunity_id) references benefactor_marketing_opportunities(id);

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

alter table if exists usacc_cases
  add constraint usacc_cases_plaintiff_fk
  foreign key (plaintiff_user_id) references usacc_users(id);

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

alter table if exists usacc_case_participants
  add constraint usacc_case_participants_case_fk
  foreign key (case_id) references usacc_cases(id);

alter table if exists usacc_case_participants
  add constraint usacc_case_participants_user_fk
  foreign key (user_id) references usacc_users(id);

alter table if exists usacc_case_participants
  add constraint usacc_case_participants_granted_by_fk
  foreign key (granted_by) references usacc_users(id);

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

alter table if exists usacc_case_stages
  add constraint usacc_case_stages_case_fk
  foreign key (case_id) references usacc_cases(id);

alter table if exists usacc_case_stages
  add constraint usacc_case_stages_assigned_user_fk
  foreign key (assigned_user_id) references usacc_users(id);

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

alter table if exists usacc_elections
  add constraint usacc_elections_case_fk
  foreign key (case_id) references usacc_cases(id);

alter table if exists usacc_elections
  add constraint usacc_elections_stage_fk
  foreign key (stage_id) references usacc_case_stages(id);

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

alter table if exists usacc_votes
  add constraint usacc_votes_election_fk
  foreign key (election_id) references usacc_elections(id);

alter table if exists usacc_votes
  add constraint usacc_votes_case_fk
  foreign key (case_id) references usacc_cases(id);

alter table if exists usacc_votes
  add constraint usacc_votes_voter_fk
  foreign key (voter_user_id) references usacc_users(id);

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

alter table if exists usacc_escrow_accounts
  add constraint usacc_escrow_accounts_case_fk
  foreign key (case_id) references usacc_cases(id);

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

alter table if exists usacc_ledger_entries
  add constraint usacc_ledger_entries_case_fk
  foreign key (case_id) references usacc_cases(id);

alter table if exists usacc_ledger_entries
  add constraint usacc_ledger_entries_escrow_fk
  foreign key (escrow_account_id) references usacc_escrow_accounts(id);

alter table if exists usacc_ledger_entries
  add constraint usacc_ledger_entries_user_fk
  foreign key (user_id) references usacc_users(id);

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

alter table if exists usacc_contract_operations
  add constraint usacc_contract_operations_case_fk
  foreign key (case_id) references usacc_cases(id);

alter table if exists usacc_contract_operations
  add constraint usacc_contract_operations_election_fk
  foreign key (election_id) references usacc_elections(id);

alter table if exists usacc_contract_operations
  add constraint usacc_contract_operations_vote_fk
  foreign key (vote_id) references usacc_votes(id);

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

alter table if exists usacc_simulation_runs
  add constraint usacc_simulation_runs_case_fk
  foreign key (case_id) references usacc_cases(id);

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

alter table if exists usacc_audit_events
  add constraint usacc_audit_events_case_fk
  foreign key (case_id) references usacc_cases(id);

alter table if exists usacc_audit_events
  add constraint usacc_audit_events_actor_fk
  foreign key (actor_user_id) references usacc_users(id);

-- ─────────────────────────────────────────────────────────────────────────────
-- benefactor.cc local-service lead scraping
-- Ported from the dd-next-1 benefactor pipeline. These tables live in a dedicated
-- `benefactor` Postgres schema so the lead-generation data model is isolated from the
-- shared public tables. Consuming services address them via `search_path = benefactor, public`
-- (or the schema-qualified constants emitted by pg-defs).
-- ─────────────────────────────────────────────────────────────────────────────

create schema if not exists benefactor;

create table if not exists benefactor.benefactor_leads (
  id uuid primary key default gen_random_uuid(),
  business_name varchar(300) default '' not null,
  owner_first_name varchar(120) default '' not null,
  owner_last_name varchar(130) default '' not null,
  primary_email varchar(255) default '' not null,
  secondary_email varchar(255),
  primary_phone varchar(100),
  website_url varchar(500),
  service_category varchar(60) default 'other' not null,
  service_subcategories jsonb default '[]'::jsonb not null,
  city varchar(120),
  state varchar(80),
  zip_code varchar(20),
  country varchar(80) default 'US' not null,
  service_area varchar(500),
  lead_status varchar(30) default 'new' not null,
  outreach_status varchar(30) default 'pending' not null,
  total_outreach_attempts integer default 0 not null,
  last_outreach_at timestamptz,
  contact_attempts jsonb default '[]'::jsonb not null,
  source_url varchar(1000),
  source_query varchar(500),
  source_tool varchar(60),
  source_engine varchar(30),
  is_verified boolean default false not null,
  tags jsonb default '[]'::jsonb not null,
  meta_data jsonb default '{}'::jsonb not null,
  notes text,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint benefactor_leads_status_chk
    check (lead_status in ('new', 'contacted', 'replied', 'booked', 'rejected', 'unsubscribed', 'unqualified', 'do_not_contact')),
  constraint benefactor_leads_outreach_status_chk
    check (outreach_status in ('pending', 'new', 'contacted', 'failed')),
  constraint benefactor_leads_subcategories_array_chk
    check (jsonb_typeof(service_subcategories) = 'array'),
  constraint benefactor_leads_contact_attempts_array_chk
    check (jsonb_typeof(contact_attempts) = 'array'),
  constraint benefactor_leads_tags_array_chk
    check (jsonb_typeof(tags) = 'array'),
  constraint benefactor_leads_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists benefactor_leads_email_uq
  on benefactor.benefactor_leads (primary_email);

create index if not exists benefactor_leads_business_name_idx
  on benefactor.benefactor_leads (business_name);

create index if not exists benefactor_leads_category_idx
  on benefactor.benefactor_leads (service_category);

create index if not exists benefactor_leads_city_idx
  on benefactor.benefactor_leads (city);

create index if not exists benefactor_leads_state_idx
  on benefactor.benefactor_leads (state);

create index if not exists benefactor_leads_zip_idx
  on benefactor.benefactor_leads (zip_code);

create index if not exists benefactor_leads_status_idx
  on benefactor.benefactor_leads (lead_status);

create index if not exists benefactor_leads_outreach_idx
  on benefactor.benefactor_leads (outreach_status);

create index if not exists benefactor_leads_last_outreach_idx
  on benefactor.benefactor_leads (last_outreach_at);

create index if not exists benefactor_leads_created_at_idx
  on benefactor.benefactor_leads (created_at);

create index if not exists benefactor_leads_soft_deleted_idx
  on benefactor.benefactor_leads (is_soft_deleted);

create index if not exists benefactor_leads_verified_idx
  on benefactor.benefactor_leads (is_verified);

create index if not exists benefactor_leads_category_city_idx
  on benefactor.benefactor_leads (service_category, city);

create index if not exists benefactor_leads_category_state_idx
  on benefactor.benefactor_leads (service_category, state);

create table if not exists benefactor.benefactor_leads_domains (
  id uuid primary key default gen_random_uuid(),
  domain varchar(255) not null,
  domain_kind varchar(32) default 'email' not null,
  status varchar(40) default 'allowed' not null,
  reason text,
  source varchar(80) default 'manual' not null,
  is_blacklisted boolean default false not null,
  is_blocked boolean default false not null,
  is_permanently_blocked boolean default false not null,
  blocked_reason text,
  blocked_until timestamptz,
  skip_until timestamptz,
  scrape_count integer default 0 not null,
  skip_count integer default 0 not null,
  skipped_count integer default 0 not null,
  email_found_count integer default 0 not null,
  lead_inserted_count integer default 0 not null,
  last_seen_at timestamptz,
  last_scraped_at timestamptz,
  last_skipped_at timestamptz,
  last_email_found_at timestamptz,
  last_lead_inserted_at timestamptz,
  last_seen_url text,
  meta_data jsonb default '{}'::jsonb not null,
  is_active boolean default true not null,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint benefactor_leads_domains_kind_chk
    check (domain_kind in ('email', 'website')),
  constraint benefactor_leads_domains_status_chk
    check (status in ('allowed', 'blocked', 'skipped', 'scraped_recently', 'recently_scraped')),
  constraint benefactor_leads_domains_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists benefactor_leads_domains_domain_kind_uq
  on benefactor.benefactor_leads_domains (domain, domain_kind);

create index if not exists benefactor_leads_domains_domain_idx
  on benefactor.benefactor_leads_domains (domain);

create index if not exists benefactor_leads_domains_kind_idx
  on benefactor.benefactor_leads_domains (domain_kind);

create index if not exists benefactor_leads_domains_status_idx
  on benefactor.benefactor_leads_domains (status);

create index if not exists benefactor_leads_domains_blacklisted_idx
  on benefactor.benefactor_leads_domains (is_blacklisted);

create index if not exists benefactor_leads_domains_kind_status_idx
  on benefactor.benefactor_leads_domains (domain_kind, status);

create index if not exists benefactor_leads_domains_blocked_idx
  on benefactor.benefactor_leads_domains (is_blocked);

create index if not exists benefactor_leads_domains_blocked_until_idx
  on benefactor.benefactor_leads_domains (blocked_until);

create index if not exists benefactor_leads_domains_skip_until_idx
  on benefactor.benefactor_leads_domains (skip_until);

create index if not exists benefactor_leads_domains_last_scraped_idx
  on benefactor.benefactor_leads_domains (last_scraped_at);

create index if not exists benefactor_leads_domains_active_idx
  on benefactor.benefactor_leads_domains (is_active);

create table if not exists benefactor.benefactor_search_locations (
  id uuid primary key default gen_random_uuid(),
  slug varchar(160) not null,
  city varchar(120) not null,
  state varchar(80) not null,
  state_code varchar(10),
  country varchar(80) default 'US' not null,
  metro_area varchar(220),
  military_area varchar(220),
  primary_installation varchar(220),
  installation_aliases jsonb default '[]'::jsonb not null,
  location_type varchar(80) default 'military_town' not null,
  priority integer default 5 not null,
  search_weight integer default 5 not null,
  total_query_runs integer default 0 not null,
  total_emails_inserted integer default 0 not null,
  success_count integer default 0 not null,
  failure_count integer default 0 not null,
  last_run_at timestamptz,
  last_success_at timestamptz,
  last_failure_at timestamptz,
  cooldown_until timestamptz,
  meta_data jsonb default '{}'::jsonb not null,
  is_active boolean default true not null,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint benefactor_search_locations_aliases_array_chk
    check (jsonb_typeof(installation_aliases) = 'array'),
  constraint benefactor_search_locations_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists benefactor_search_locations_slug_uq
  on benefactor.benefactor_search_locations (slug);

create unique index if not exists benefactor_search_locations_city_state_country_uq
  on benefactor.benefactor_search_locations (city, state, country);

create index if not exists benefactor_search_locations_state_idx
  on benefactor.benefactor_search_locations (state);

create index if not exists benefactor_search_locations_military_area_idx
  on benefactor.benefactor_search_locations (military_area);

create index if not exists benefactor_search_locations_type_idx
  on benefactor.benefactor_search_locations (location_type);

create index if not exists benefactor_search_locations_priority_idx
  on benefactor.benefactor_search_locations (priority);

create index if not exists benefactor_search_locations_cooldown_idx
  on benefactor.benefactor_search_locations (cooldown_until);

create index if not exists benefactor_search_locations_active_idx
  on benefactor.benefactor_search_locations (is_active);

create index if not exists benefactor_search_locations_soft_deleted_idx
  on benefactor.benefactor_search_locations (is_soft_deleted);

create table if not exists benefactor.benefactor_scrape_queries (
  id uuid primary key default gen_random_uuid(),
  query_text text not null,
  query_hash varchar(64) not null,
  benefactor_icp_slug varchar(160),
  benefactor_icp_name varchar(220),
  benefactor_search_location_id uuid,
  service_category varchar(60) default 'other' not null,
  target_city varchar(120),
  target_state varchar(80),
  target_country varchar(80) default 'US' not null,
  target_military_area varchar(220),
  target_installation varchar(220),
  query_variant varchar(80) default 'email_contact' not null,
  search_page_depth integer default 4 not null,
  priority integer default 5 not null,
  total_runs integer default 0 not null,
  total_urls_visited integer default 0 not null,
  total_emails_found integer default 0 not null,
  total_emails_inserted integer default 0 not null,
  total_emails_duplicate integer default 0 not null,
  total_errors integer default 0 not null,
  success_count integer default 0 not null,
  failure_count integer default 0 not null,
  last_run_at timestamptz,
  last_success_at timestamptz,
  last_failure_at timestamptz,
  last_run_emails_found integer default 0 not null,
  last_run_emails_inserted integer default 0 not null,
  last_run_success boolean default false not null,
  last_run_duration_ms integer default 0 not null,
  last_run_error text,
  cooldown_until timestamptz,
  consecutive_zero_new_runs integer default 0 not null,
  last_zero_new_run_at timestamptz,
  meta_data jsonb default '{}'::jsonb not null,
  is_active boolean default true not null,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint benefactor_scrape_queries_variant_chk
    check (query_variant in ('email_contact', 'contact_us', 'website_domain', 'fuzzy_email', 'fuzzy_city')),
  constraint benefactor_scrape_queries_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists benefactor_scrape_queries_hash_uq
  on benefactor.benefactor_scrape_queries (query_hash);

create index if not exists benefactor_scrape_queries_icp_slug_idx
  on benefactor.benefactor_scrape_queries (benefactor_icp_slug);

create index if not exists benefactor_scrape_queries_location_id_idx
  on benefactor.benefactor_scrape_queries (benefactor_search_location_id);

create index if not exists benefactor_scrape_queries_category_idx
  on benefactor.benefactor_scrape_queries (service_category);

create index if not exists benefactor_scrape_queries_city_idx
  on benefactor.benefactor_scrape_queries (target_city);

create index if not exists benefactor_scrape_queries_state_idx
  on benefactor.benefactor_scrape_queries (target_state);

create index if not exists benefactor_scrape_queries_military_area_idx
  on benefactor.benefactor_scrape_queries (target_military_area);

create index if not exists benefactor_scrape_queries_variant_idx
  on benefactor.benefactor_scrape_queries (query_variant);

create index if not exists benefactor_scrape_queries_priority_idx
  on benefactor.benefactor_scrape_queries (priority);

create index if not exists benefactor_scrape_queries_last_run_idx
  on benefactor.benefactor_scrape_queries (last_run_at);

create index if not exists benefactor_scrape_queries_success_count_idx
  on benefactor.benefactor_scrape_queries (success_count);

create index if not exists benefactor_scrape_queries_failure_count_idx
  on benefactor.benefactor_scrape_queries (failure_count);

create index if not exists benefactor_scrape_queries_active_idx
  on benefactor.benefactor_scrape_queries (is_active);

create index if not exists benefactor_scrape_queries_cooldown_idx
  on benefactor.benefactor_scrape_queries (cooldown_until);

create index if not exists benefactor_scrape_queries_zero_new_idx
  on benefactor.benefactor_scrape_queries (consecutive_zero_new_runs);

create table if not exists benefactor.benefactor_domain_search_tracking (
  id uuid primary key default gen_random_uuid(),
  domain varchar(255) not null,
  for_what varchar(80) default 'benefactor_lead_scrape' not null,
  search_result_appearances integer default 0 not null,
  queued_visit_count integer default 0 not null,
  visit_count integer default 0 not null,
  good_result_count integer default 0 not null,
  bad_result_count integer default 0 not null,
  email_found_count integer default 0 not null,
  lead_inserted_count integer default 0 not null,
  last_queued_at timestamptz,
  last_visited_at timestamptz,
  last_good_result_at timestamptz,
  last_bad_result_at timestamptz,
  last_email_found_at timestamptz,
  last_lead_inserted_at timestamptz,
  last_good_url text,
  last_bad_url text,
  blocked_until timestamptz,
  is_permanently_blocked boolean default false not null,
  blocked_reason text,
  meta_data jsonb default '{}'::jsonb not null,
  is_active boolean default true not null,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint benefactor_domain_search_tracking_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists benefactor_domain_search_tracking_domain_for_what_uq
  on benefactor.benefactor_domain_search_tracking (domain, for_what);

create index if not exists benefactor_domain_search_tracking_for_what_idx
  on benefactor.benefactor_domain_search_tracking (for_what);

create index if not exists benefactor_domain_search_tracking_visit_count_idx
  on benefactor.benefactor_domain_search_tracking (visit_count);

create index if not exists benefactor_domain_search_tracking_good_result_idx
  on benefactor.benefactor_domain_search_tracking (good_result_count);

create index if not exists benefactor_domain_search_tracking_bad_result_idx
  on benefactor.benefactor_domain_search_tracking (bad_result_count);

create index if not exists benefactor_domain_search_tracking_email_found_idx
  on benefactor.benefactor_domain_search_tracking (email_found_count);

create index if not exists benefactor_domain_search_tracking_blocked_until_idx
  on benefactor.benefactor_domain_search_tracking (blocked_until);

create index if not exists benefactor_domain_search_tracking_active_idx
  on benefactor.benefactor_domain_search_tracking (is_active);

create table if not exists benefactor.benefactor_icps (
  id uuid primary key default gen_random_uuid(),
  slug varchar(160) not null,
  name varchar(220) not null,
  category varchar(120) default 'local_services' not null,
  service_category varchar(120) default 'other' not null,
  description text default '' not null,
  outcall_fit_score integer default 5 not null,
  priority integer default 5 not null,
  search_terms jsonb default '[]'::jsonb not null,
  search_signals jsonb default '[]'::jsonb not null,
  target_home_services boolean default false not null,
  target_medical boolean default false not null,
  target_legal boolean default false not null,
  target_events boolean default false not null,
  target_corporate boolean default false not null,
  target_industrial boolean default false not null,
  meta_data jsonb default '{}'::jsonb not null,
  is_active boolean default true not null,
  is_soft_deleted boolean default false not null,
  created_at timestamptz default now() not null,
  updated_at timestamptz default now() not null,
  created_by uuid,
  updated_by uuid,
  constraint benefactor_icps_search_terms_array_chk
    check (jsonb_typeof(search_terms) = 'array'),
  constraint benefactor_icps_search_signals_array_chk
    check (jsonb_typeof(search_signals) = 'array'),
  constraint benefactor_icps_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object')
);

create unique index if not exists benefactor_icps_slug_uq
  on benefactor.benefactor_icps (slug);

create index if not exists benefactor_icps_category_idx
  on benefactor.benefactor_icps (category);

create index if not exists benefactor_icps_service_category_idx
  on benefactor.benefactor_icps (service_category);

create index if not exists benefactor_icps_priority_idx
  on benefactor.benefactor_icps (priority);

create index if not exists benefactor_icps_outcall_fit_idx
  on benefactor.benefactor_icps (outcall_fit_score);

create index if not exists benefactor_icps_active_idx
  on benefactor.benefactor_icps (is_active);

create index if not exists benefactor_icps_soft_deleted_idx
  on benefactor.benefactor_icps (is_soft_deleted);

alter table if exists benefactor.benefactor_scrape_queries
  add constraint benefactor_scrape_queries_location_fk
  foreign key (benefactor_search_location_id) references benefactor.benefactor_search_locations(id);
