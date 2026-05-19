-- Minimal bootstrap for services that read dynamic JSON config from app_config.
-- Keep this idempotent so maintenance jobs can safely run it before app_config seeds.

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
  updated_by uuid
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
