-- Supabase project: mckxblyvfzyoxpwvrnjm
-- Organization: ngixgitdtrbwnaimwsbb (sonus-auris)
--
-- Safe, owner-readable projection of the authoritative encrypted records in
-- remote Postgres sound_recorder_cloud_connections. OAuth tokens, object-store
-- credentials, provider subject identifiers, scopes, and opaque metadata must
-- never be copied into this table.

create table if not exists public.cloud_connections (
  id uuid primary key,
  user_id uuid not null references auth.users(id) on delete cascade,
  provider text not null,
  link_mode text not null,
  status text not null,
  display_name text,
  folder_path text,
  token_expires_at timestamptz,
  last_sync_at timestamptz,
  created_at timestamptz not null,
  updated_at timestamptz not null,
  constraint cloud_connections_provider_check
    check (
      provider in (
        'google_drive',
        'microsoft_onedrive',
        'apple_icloud',
        'dropbox',
        'amazon_s3',
        'cloudflare_r2'
      )
    ),
  constraint cloud_connections_link_mode_check
    check (link_mode in ('server_oauth', 'client_managed')),
  constraint cloud_connections_status_check
    check (status in ('active', 'paused', 'revoked', 'failed')),
  constraint cloud_connections_display_name_size_check
    check (display_name is null or octet_length(display_name) between 1 and 160),
  constraint cloud_connections_folder_path_size_check
    check (folder_path is null or octet_length(folder_path) between 1 and 512)
);

create index if not exists cloud_connections_user_status_idx
  on public.cloud_connections (user_id, status, updated_at desc);

create index if not exists cloud_connections_user_provider_idx
  on public.cloud_connections (user_id, provider, updated_at desc);

alter table public.cloud_connections enable row level security;

revoke all on table public.cloud_connections from anon;
revoke insert, update, delete, truncate, references, trigger
  on table public.cloud_connections from authenticated;
grant select on table public.cloud_connections to authenticated;
grant all on table public.cloud_connections to service_role;

drop policy if exists cloud_connections_owner_select on public.cloud_connections;

create policy cloud_connections_owner_select
  on public.cloud_connections
  for select
  to authenticated
  using ((select auth.uid()) = user_id);
