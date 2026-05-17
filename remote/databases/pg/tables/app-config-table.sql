-- Generic JSON app configuration table for RDS/Postgres-backed runtime services.
-- Do not apply this file directly. Generate/review the true DB diff for the
-- target environment before running any database write.

create table if not exists app_config (
  id uuid primary key default gen_random_uuid(),
  scope varchar(120) not null default 'default',
  key varchar(200) not null,
  value jsonb not null,
  version integer not null default 1,
  status varchar(32) not null default 'active',
  labels jsonb not null default '[]'::jsonb,
  meta_data jsonb not null default '{}'::jsonb,
  is_soft_deleted boolean not null default false,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
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
