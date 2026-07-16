-- Dedicated client observability table. Clients insert only after Supabase
-- login, using the user's JWT; RLS binds every row to auth.uid().

create extension if not exists pgcrypto;

create table if not exists public.client_telemetry (
  id uuid not null default gen_random_uuid(),
  user_id uuid not null default auth.uid() references auth.users(id) on delete cascade,
  device_id text not null,
  level text not null check (level in ('debug', 'info', 'warning', 'error', 'fatal')),
  event text not null,
  message text not null,
  stack text,
  platform text,
  app_version text,
  details jsonb not null default '{}'::jsonb,
  occurred_at timestamptz not null,
  created_at timestamptz not null default now(),
  constraint client_telemetry_pkey primary key (id)
);

alter table public.client_telemetry enable row level security;

do $$ begin
  if not exists (
    select 1 from pg_policies
    where schemaname = 'public'
      and tablename = 'client_telemetry'
      and policyname = 'client_telemetry_insert'
  ) then
    create policy "client_telemetry_insert"
      on public.client_telemetry
      for insert
      to authenticated
      with check (auth.uid() = user_id);
  end if;
end $$;

do $$ begin
  if not exists (
    select 1 from pg_policies
    where schemaname = 'public'
      and tablename = 'client_telemetry'
      and policyname = 'client_telemetry_select'
  ) then
    create policy "client_telemetry_select"
      on public.client_telemetry
      for select
      to authenticated
      using (auth.uid() = user_id);
  end if;
end $$;

create index if not exists client_telemetry_user_occurred_idx
  on public.client_telemetry (user_id, occurred_at desc);
create index if not exists client_telemetry_level_occurred_idx
  on public.client_telemetry (level, occurred_at desc);
create index if not exists client_telemetry_event_occurred_idx
  on public.client_telemetry (event, occurred_at desc);

revoke all on table public.client_telemetry from anon;
grant select, insert on table public.client_telemetry to authenticated;

do $$ begin
  if exists (select 1 from pg_publication where pubname = 'supabase_realtime')
     and not exists (
       select 1 from pg_publication_tables
       where pubname = 'supabase_realtime'
         and schemaname = 'public'
         and tablename = 'client_telemetry'
     ) then
    alter publication supabase_realtime add table public.client_telemetry;
  end if;
end $$;
