-- shared_auth — declarative schema for the OreSoftware shared auth server.
--
-- Owned by pg-defs and applied with dpm (declarative; no migration files). The
-- server connects with search_path=shared_auth and runs NO DDL. One namespace
-- per app, per the org convention (see the pg-defs + dpm memory).
--
-- This is a mirror of *identity*, not credentials. Supabase remains the identity
-- provider and password store; we only index the users we have verified so
-- downstream services can resolve a stable OreSoftware user id.

create schema if not exists shared_auth;

create table if not exists shared_auth.users (
    -- Stable OreSoftware identity. This is the `sub` of the JWTs we mint, so it
    -- must never change for a given (project, supabase user).
    shared_user_id    uuid        primary key default gen_random_uuid(),

    -- Which Supabase project/org vouched for this identity (config slug, e.g.
    -- "fiducia-cloud"). Part of the natural key: the same person in two projects
    -- is two identities here.
    supabase_project  text        not null,
    -- text (not uuid): Supabase sub is a UUID today, but keep the column tolerant
    -- of any opaque provider subject rather than rejecting at the DB layer.
    supabase_user_id  text        not null,

    email             text,
    email_verified    boolean     not null default false,
    phone             text,

    -- Verbatim Supabase metadata blobs, for downstream authorization decisions.
    user_metadata     jsonb       not null default '{}'::jsonb,
    app_metadata      jsonb       not null default '{}'::jsonb,

    created_at        timestamptz not null default now(),
    updated_at        timestamptz not null default now(),
    last_seen_at      timestamptz not null default now(),

    unique (supabase_project, supabase_user_id)
);

create index if not exists users_email_idx
    on shared_auth.users (lower(email));

create index if not exists users_project_idx
    on shared_auth.users (supabase_project);
