-- Declarative table definition for warm container pool configs.
-- Do not apply this file directly. Generate/review the true DB diff for the
-- target environment before running any database write.

create table if not exists container_pool_configs (
  id uuid primary key default gen_random_uuid(),
  slug varchar(120) not null,
  display_name varchar(200) not null,
  image text not null,
  command jsonb not null default '[]'::jsonb,
  env jsonb not null default '{}'::jsonb,
  request_path varchar(256) not null default '/invoke',
  container_port integer not null default 8080,
  min_warm integer not null default 1,
  max_warm integer not null default 2,
  max_concurrency_per_container integer not null default 1,
  request_timeout_ms integer not null default 30000,
  idle_ttl_seconds integer not null default 900,
  nats_subject text,
  status varchar(32) not null default 'active',
  labels jsonb not null default '[]'::jsonb,
  meta_data jsonb not null default '{}'::jsonb,
  is_soft_deleted boolean not null default false,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
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
