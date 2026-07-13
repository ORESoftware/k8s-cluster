-- Generated contract source: apps/sonus-auris-interfaces/schema/tables.json.
-- Keep this reviewed migration aligned with @sonus-auris/interfaces.

create extension if not exists pgcrypto;

create table if not exists public.acoustic_events (
  id uuid not null default gen_random_uuid(),
  user_id uuid not null default auth.uid() references auth.users(id) on delete cascade,
  device_id text not null,
  kind text not null,
  started_at timestamptz not null,
  ended_at timestamptz not null,
  confidence double precision not null default 0,
  details jsonb not null default '{}'::jsonb,
  created_at timestamptz not null default now(),
  constraint acoustic_events_pkey primary key (id)
);

alter table public.acoustic_events enable row level security;

do $$
begin
  if not exists (
    select 1
    from pg_policies
    where schemaname = 'public'
      and tablename = 'acoustic_events'
      and policyname = 'acoustic_events_owner'
  ) then
    create policy "acoustic_events_owner"
      on public.acoustic_events
      for all
      to authenticated
      using (auth.uid() = user_id)
      with check (auth.uid() = user_id);
  end if;
end
$$;

create index if not exists acoustic_events_user_started_idx
  on public.acoustic_events (user_id, started_at desc);

revoke all on table public.acoustic_events from anon;
grant select, insert, update, delete on table public.acoustic_events to authenticated;

create table if not exists public.user_consents (
  id uuid not null default gen_random_uuid(),
  user_id uuid not null default auth.uid() references auth.users(id) on delete cascade,
  device_id text not null,
  consent_version text not null,
  platform text,
  granted jsonb not null,
  accepted_at timestamptz not null,
  created_at timestamptz not null default now(),
  constraint user_consents_pkey primary key (id)
);

alter table public.user_consents enable row level security;

do $$
begin
  if not exists (
    select 1
    from pg_policies
    where schemaname = 'public'
      and tablename = 'user_consents'
      and policyname = 'user_consents_insert'
  ) then
    create policy "user_consents_insert"
      on public.user_consents
      for insert
      to authenticated
      with check (auth.uid() = user_id);
  end if;
end
$$;

do $$
begin
  if not exists (
    select 1
    from pg_policies
    where schemaname = 'public'
      and tablename = 'user_consents'
      and policyname = 'user_consents_select'
  ) then
    create policy "user_consents_select"
      on public.user_consents
      for select
      to authenticated
      using (auth.uid() = user_id);
  end if;
end
$$;

create index if not exists user_consents_user_accepted_idx
  on public.user_consents (user_id, accepted_at desc);

revoke all on table public.user_consents from anon;
grant select, insert on table public.user_consents to authenticated;
