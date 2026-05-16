-- Declarative table definition for user-defined remote lambda functions.
-- Do not apply this file directly. Generate/review the true DB diff for the
-- target environment before running any database write.

create table if not exists lambda_functions (
  id uuid primary key default gen_random_uuid(),
  slug varchar(120) not null,
  display_name varchar(200) not null,
  description text not null default '',
  runtime varchar(40) not null default 'nodejs',
  entry_command text not null default 'env -i PATH="$PATH" NODE_ENV=production node --permission --allow-net child-runtimes/js-function-runner.mjs',
  function_body text not null,
  reuse_key varchar(200),
  idle_timeout_seconds integer not null default 300,
  max_run_ms integer not null default 30000,
  containerized boolean not null default false,
  container_image text,
  container_build_status varchar(32) not null default 'not_requested',
  container_build_error text,
  container_built_at timestamptz,
  status varchar(32) not null default 'draft',
  env jsonb not null default '{}'::jsonb,
  labels jsonb not null default '[]'::jsonb,
  meta_data jsonb not null default '{}'::jsonb,
  last_invoked_at timestamptz,
  is_soft_deleted boolean not null default false,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  created_by uuid,
  updated_by uuid,
  constraint lambda_functions_slug_format_chk
    check (slug ~ '^[a-z0-9][a-z0-9-]{1,118}[a-z0-9]$'),
  constraint lambda_functions_body_size_chk
    check (octet_length(function_body) <= 262144),
  constraint lambda_functions_entry_command_chk
    check (entry_command in (
      'env -i PATH="$PATH" NODE_ENV=production node --permission --allow-net child-runtimes/js-function-runner.mjs',
      'env -i PATH="$PATH" PYTHONUNBUFFERED=1 python3 child-runtimes/python-function-runner.py',
      'env -i PATH="$PATH" ruby child-runtimes/ruby-function-runner.rb',
      'env -i PATH="$PATH" node --permission --allow-net --allow-child-process child-runtimes/bash-function-runner.mjs'
    )),
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
