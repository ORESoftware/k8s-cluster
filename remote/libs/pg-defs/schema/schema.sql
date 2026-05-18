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

create index if not exists agent_remote_dev_events_task_id_created_at_idx
  on agent_remote_dev_events (task_id, created_at desc);

create index if not exists agent_remote_dev_events_created_at_idx
  on agent_remote_dev_events (created_at desc);

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

create table if not exists lambda_functions (
  id uuid primary key default gen_random_uuid(),
  slug varchar(120) not null,
  display_name varchar(200) not null,
  description text default '' not null,
  runtime varchar(40) default 'nodejs' not null,
  entry_command text default 'env -i PATH="$PATH" NODE_ENV=production node --permission --allow-net child-runtimes/js-function-runner.mjs' not null,
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
    check (runtime in ('nodejs', 'javascript', 'typescript', 'python3', 'python', 'ruby', 'bash', 'shell')),
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

alter table if exists agent_remote_dev_threads
  add constraint agent_remote_dev_threads_known_git_repo_fk
  foreign key (known_git_repo_id) references known_git_repos(id);

alter table if exists agent_remote_dev_tasks
  add constraint agent_remote_dev_tasks_thread_fk
  foreign key (thread_id) references agent_remote_dev_threads(id);

alter table if exists agent_remote_dev_events
  add constraint agent_remote_dev_events_task_fk
  foreign key (task_id) references agent_remote_dev_tasks(id);

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
-- Presence membership change notifications via SHARDED LISTEN/NOTIFY.
--
-- Why sharded:
--   A naïve pg_notify('presence_member_change', …) blasts every change to
--   every pod that's LISTENing — fine at small scale, terrible once you
--   have hundreds of pods and thousands of writes per second. Most pods
--   don't care about most conversations.
--
-- Sharding strategy:
--   For each membership change, compute
--       shard = abs(hashtext(conv_id::text)) % presence_notify_shards()
--   and pg_notify on channel `presence_change_<shard>`. The shard count
--   is read from the GUC `presence.notify_shards` (default 256) so the
--   value is part of the database's contract and pods don't have to guess.
--
--   Each pod LISTENs only on the shards corresponding to convs it has
--   live connections in. The pg_listen.gleam module ref-counts these
--   subscriptions and dynamically LISTEN/UNLISTENs as connections come
--   and go, so per-pod NOTIFY volume scales with the pod's actual
--   subscriber set, not the global write rate.
--
-- Sharding key (conv_id, not user_id):
--   - When user X is added to conv Y, every pod that has any user in
--     conv Y needs to update its members_of(Y) cache, and the pod where
--     user X has a connection needs to dispatch JoinConv(Y).
--   - The first set is exactly "pods listening on shard(Y)" by
--     construction. The second set is reached because that pod is also
--     listening on shard(Y) once it has any conn whose user is in Y.
--   - Edge case: if user X isn't a member of any conv on this pod's
--     listened shards before this NOTIFY, the pod misses the event for
--     this user. That's fine because the conn already loaded the user's
--     conv list from PG at open time, and any subsequent JoinConv flows
--     over the broadcast plane (Erlang `pg` + NATS) which is independent.
--
-- Dedup:
--   The same logical event may also arrive via the pg-mesh gossip path
--   (Erlang `pg`, used as a fallback / belt-and-braces channel). The
--   conversations actor dedupes by
--       (op, conv_id, user_id, emitted_at)
--   in a small TTL window so neither path produces duplicate client
--   frames.
--
-- Payload:
--   `{"op":...,"conv_id":...,"user_id":...,"soft_deleted":bool,
--     "emitted_at":epoch_seconds}` — well under the 8KB NOTIFY cap.
-- ────────────────────────────────────────────────────────────────────────

-- Default number of NOTIFY shards. Override per-database with:
--   ALTER DATABASE mydb SET presence.notify_shards = 64;
-- Reading is via the helper below which always returns a positive int.

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

create or replace function notify_presence_member_change()
returns trigger
language plpgsql
as $$
declare
  v_op text := tg_op;
  v_conv uuid := coalesce(new.conv_id, old.conv_id);
  v_user uuid := coalesce(new.user_id, old.user_id);
  v_soft boolean := coalesce(new.is_soft_deleted, old.is_soft_deleted, false);
  v_shards int := presence_notify_shards();
  v_shard int;
  v_channel text;
  v_payload text;
begin
  -- Use the first 16 bits of the conv_id UUID as the shard input.
  -- The Erlang side mirrors this exactly via `dd_nats` / `pg_listen`'s
  -- shard_of helper. Avoiding hashtext() keeps the algorithm portable
  -- across BEAM and Postgres without re-implementing PG's internal hash.
  v_shard := (('x' || substring(replace(v_conv::text, '-', ''), 1, 4))
              ::bit(16)::int % v_shards);
  v_channel := 'presence_change_' || v_shard::text;

  v_payload := json_build_object(
    'op',           v_op,
    'conv_id',      v_conv,
    'user_id',      v_user,
    'soft_deleted', v_soft,
    'shard',        v_shard,
    'emitted_at',   extract(epoch from clock_timestamp())
  )::text;

  perform pg_notify(v_channel, v_payload);
  return coalesce(new, old);
end;
$$;

drop trigger if exists presence_conv_members_notify on presence_conv_members;

create trigger presence_conv_members_notify
  after insert or update or delete on presence_conv_members
  for each row
  execute function notify_presence_member_change();

-- Helper exposed to clients (and to the BEAM service) so they can compute
-- the same shard locally without re-implementing the hash. Useful for
-- pg_listen.gleam — it asks Postgres which shard a conv belongs to and
-- LISTENs on the matching channel.

create or replace function presence_shard_of(p_conv_id uuid)
returns int
language sql
stable
as $$
  select (('x' || substring(replace(p_conv_id::text, '-', ''), 1, 4))
          ::bit(16)::int % presence_notify_shards());
$$;
