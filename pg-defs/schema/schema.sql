-- Canonical remote Postgres schema source for pg-defs.
-- This file is the desired-state contract used by the remote migration diff generator.
-- Do not apply it directly to a shared database; generate and review a diff instead.

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
  user_id uuid,
  known_git_repo_id uuid,
  title text not null,
  repo text not null,
  base_branch varchar(120) default 'dev' not null,
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
    check (base_branch ~ '^[A-Za-z0-9._/-]{1,120}$')
);

create index if not exists agent_remote_dev_threads_known_git_repo_id_idx
  on agent_remote_dev_threads (known_git_repo_id)
  where is_soft_deleted = false;

create index if not exists agent_remote_dev_threads_repo_idx
  on agent_remote_dev_threads (repo)
  where is_soft_deleted = false;

create index if not exists agent_remote_dev_threads_updated_at_idx
  on agent_remote_dev_threads (updated_at desc)
  where is_soft_deleted = false;

create table if not exists agent_remote_dev_tasks (
  id uuid primary key,
  thread_id uuid not null,
  user_id uuid,
  docker_task_id uuid,
  prompt text not null,
  status varchar(32) default 'queued' not null,
  branch text,
  pr_url text,
  pr_state varchar(32),
  exit_reason varchar(32),
  error_message text,
  last_event_seq integer default -1 not null,
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
    check (status in ('queued', 'running', 'streaming', 'done', 'failed', 'cancelled', 'pr_open')),
  constraint agent_remote_dev_tasks_pr_state_chk
    check (pr_state is null or pr_state in ('draft', 'open', 'closed', 'merged')),
  constraint agent_remote_dev_tasks_exit_reason_chk
    check (exit_reason is null or exit_reason in ('completed', 'cancelled', 'failed'))
);

create index if not exists agent_remote_dev_tasks_thread_id_created_at_idx
  on agent_remote_dev_tasks (thread_id, created_at desc)
  where is_soft_deleted = false;

create index if not exists agent_remote_dev_tasks_status_idx
  on agent_remote_dev_tasks (status)
  where is_soft_deleted = false;

create index if not exists agent_remote_dev_tasks_updated_at_idx
  on agent_remote_dev_tasks (updated_at desc)
  where is_soft_deleted = false;

create table if not exists agent_remote_dev_events (
  id uuid primary key default gen_random_uuid(),
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

create table if not exists agent_remote_dev_artifacts (
  id uuid primary key default gen_random_uuid(),
  task_id uuid not null,
  storage_provider varchar(40) default 'local' not null,
  artifact_kind varchar(40) default 'file' not null,
  file_name text not null,
  content_type text,
  url text not null,
  size_bytes integer,
  meta_data jsonb default '{}'::jsonb not null,
  created_at timestamptz default now() not null,
  constraint agent_remote_dev_artifacts_file_name_size_chk
    check (octet_length(file_name) <= 1024),
  constraint agent_remote_dev_artifacts_url_size_chk
    check (octet_length(url) <= 4096),
  constraint agent_remote_dev_artifacts_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object'),
  constraint agent_remote_dev_artifacts_storage_provider_chk
    check (storage_provider in ('local', 's3-r2', 'gcs', 'drive')),
  constraint agent_remote_dev_artifacts_artifact_kind_chk
    check (artifact_kind in ('file', 'log', 'patch', 'report'))
);

create index if not exists agent_remote_dev_artifacts_task_id_created_at_idx
  on agent_remote_dev_artifacts (task_id, created_at desc);

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
  runtime varchar(40) default 'javascript' not null,
  entry_command text default 'env -i PATH="$PATH" NODE_ENV=production node --permission --allow-net child-runtimes/js-function-runner.mjs' not null,
  function_body text not null,
  reuse_key varchar(200),
  idle_timeout_seconds integer default 300 not null,
  max_run_ms integer default 30000 not null,
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
    check (entry_command = 'env -i PATH="$PATH" NODE_ENV=production node --permission --allow-net child-runtimes/js-function-runner.mjs'),
  constraint lambda_functions_labels_array_chk
    check (jsonb_typeof(labels) = 'array'),
  constraint lambda_functions_meta_object_chk
    check (jsonb_typeof(meta_data) = 'object'),
  constraint lambda_functions_env_object_chk
    check (jsonb_typeof(env) = 'object'),
  constraint lambda_functions_status_chk
    check (status in ('draft', 'active', 'paused', 'archived')),
  constraint lambda_functions_runtime_chk
    check (runtime in ('javascript', 'typescript', 'python', 'shell', 'gleam'))
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

alter table if exists agent_remote_dev_runtime_locks
  add constraint agent_remote_dev_runtime_locks_thread_fk
  foreign key (thread_id) references agent_remote_dev_threads(id);
