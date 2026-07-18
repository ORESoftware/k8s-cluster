-- Declarative desired-state schema for the canonical-web-server public
-- schema, consumed by dpm (declarative-postgres-migrate,
-- https://github.com/declarative-migrations/declarative-postgres-migrate.rs).
--
-- NEVER apply this file directly to a live database. dpm materializes it on
-- a throwaway shadow database, introspects the result, and emits reviewable
-- migration SQL:
--
--   dpm diff   --source deploy/postgres/schema.sql --target "$DATABASE_URL" \
--              --shadow "$SHADOW_DATABASE_URL"
--   dpm verify --source deploy/postgres/schema.sql --target "$DATABASE_URL" \
--              --shadow "$SHADOW_DATABASE_URL"
--
-- Supabase: connect through the direct connection or session pooler (5432),
-- never the transaction pooler (6543). Grants and role bootstrap live in
-- bootstrap_runtime_role.sql (dpm deliberately does not diff grants).
--
-- The SeaORM migration in src/db/migration.rs remains the executable runtime
-- migration; CI proves this file and the migrated schema converge, so edit
-- both together.

-- Shadow-materialization fixture. Real Supabase databases already provide
-- the auth schema (dpm excludes managed schemas from diffs); this block
-- exists only so the file can materialize on a bare shadow database.
CREATE SCHEMA IF NOT EXISTS auth;
CREATE TABLE IF NOT EXISTS auth.users (id uuid PRIMARY KEY);
CREATE OR REPLACE FUNCTION auth.uid() RETURNS uuid
    LANGUAGE sql STABLE
    AS $$ SELECT nullif(current_setting('request.jwt.claim.sub', true), '')::uuid $$;

-- Everything below is the pg_dump --schema-only canonical form of the
-- migrated public schema (including SeaORM's seaql_migrations bookkeeping
-- table, which is part of the deployed state).




COMMENT ON SCHEMA public IS 'standard public schema';

CREATE TABLE public.audit_engagement (
    id uuid NOT NULL,
    owner_id uuid NOT NULL,
    company character varying NOT NULL,
    framework text NOT NULL,
    status text NOT NULL,
    opened_at timestamp with time zone NOT NULL,
    target_report_date date,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT audit_engagement_framework_check CHECK ((framework IN ('soc2', 'fedramp', 'hipaa', 'iso_27001', 'pci_dss', 'gdpr'))),
    CONSTRAINT audit_engagement_status_check CHECK ((status IN ('scoping', 'remediation', 'in_audit', 'complete')))
);

ALTER TABLE ONLY public.audit_engagement FORCE ROW LEVEL SECURITY;

CREATE TABLE public.engagement_note (
    id uuid NOT NULL,
    engagement_id uuid NOT NULL,
    owner_id uuid NOT NULL,
    body character varying NOT NULL,
    created_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.engagement_note FORCE ROW LEVEL SECURITY;

CREATE TABLE public.seaql_migrations (
    version character varying NOT NULL,
    applied_at bigint NOT NULL
);

CREATE TABLE public.sync_change (
    owner_id uuid NOT NULL,
    cursor bigint NOT NULL,
    collection character varying NOT NULL,
    record_id uuid NOT NULL,
    version bigint NOT NULL,
    operation character varying NOT NULL,
    payload jsonb NOT NULL,
    changed_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.sync_change FORCE ROW LEVEL SECURITY;

CREATE TABLE public.sync_clock (
    owner_id uuid NOT NULL,
    cursor bigint NOT NULL
);

ALTER TABLE ONLY public.sync_clock FORCE ROW LEVEL SECURITY;

CREATE TABLE public.sync_receipt (
    owner_id uuid NOT NULL,
    client_id uuid NOT NULL,
    mutation_id uuid NOT NULL,
    request_hash character varying NOT NULL,
    result jsonb NOT NULL,
    created_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.sync_receipt FORCE ROW LEVEL SECURITY;

CREATE TABLE public.sync_record (
    owner_id uuid NOT NULL,
    collection character varying NOT NULL,
    record_id uuid NOT NULL,
    version bigint NOT NULL,
    payload jsonb NOT NULL,
    deleted_at timestamp with time zone,
    updated_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.sync_record FORCE ROW LEVEL SECURITY;

CREATE TABLE public.user_profile (
    user_id uuid NOT NULL,
    email character varying NOT NULL,
    display_name character varying,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.user_profile FORCE ROW LEVEL SECURITY;

CREATE TABLE public.web_session (
    id_hash character varying NOT NULL,
    user_id uuid NOT NULL,
    email character varying NOT NULL,
    supabase_session_id uuid,
    encrypted_access_token text NOT NULL,
    encrypted_refresh_token text NOT NULL,
    access_expires_at timestamp with time zone NOT NULL,
    csrf_token character varying NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    revocation_pending_at timestamp with time zone,
    revocation_next_attempt_at timestamp with time zone,
    revocation_attempts integer DEFAULT 0 NOT NULL,
    upstream_revoked_at timestamp with time zone
);

ALTER TABLE ONLY public.audit_engagement
    ADD CONSTRAINT audit_engagement_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.engagement_note
    ADD CONSTRAINT engagement_note_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.seaql_migrations
    ADD CONSTRAINT seaql_migrations_pkey PRIMARY KEY (version);

ALTER TABLE ONLY public.sync_change
    ADD CONSTRAINT sync_change_pkey PRIMARY KEY (owner_id, cursor);

ALTER TABLE ONLY public.sync_clock
    ADD CONSTRAINT sync_clock_pkey PRIMARY KEY (owner_id);

ALTER TABLE ONLY public.sync_receipt
    ADD CONSTRAINT sync_receipt_pkey PRIMARY KEY (owner_id, client_id, mutation_id);

ALTER TABLE ONLY public.sync_record
    ADD CONSTRAINT sync_record_pkey PRIMARY KEY (owner_id, collection, record_id);

ALTER TABLE ONLY public.user_profile
    ADD CONSTRAINT user_profile_pkey PRIMARY KEY (user_id);

ALTER TABLE ONLY public.web_session
    ADD CONSTRAINT web_session_pkey PRIMARY KEY (id_hash);

CREATE INDEX audit_engagement_owner_idx ON public.audit_engagement USING btree (owner_id);

CREATE INDEX audit_engagement_owner_status_idx ON public.audit_engagement USING btree (owner_id, status);

CREATE INDEX engagement_note_engagement_created_idx ON public.engagement_note USING btree (engagement_id, created_at);

CREATE INDEX engagement_note_owner_idx ON public.engagement_note USING btree (owner_id);

CREATE INDEX sync_change_owner_cursor_idx ON public.sync_change USING btree (owner_id, cursor);

CREATE INDEX web_session_revocation_retry_idx ON public.web_session USING btree (revocation_next_attempt_at);

CREATE INDEX web_session_user_id_idx ON public.web_session USING btree (user_id);

ALTER TABLE ONLY public.audit_engagement
    ADD CONSTRAINT audit_engagement_auth_user_fk FOREIGN KEY (owner_id) REFERENCES auth.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.engagement_note
    ADD CONSTRAINT engagement_note_auth_user_fk FOREIGN KEY (owner_id) REFERENCES auth.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.engagement_note
    ADD CONSTRAINT engagement_note_engagement_fk FOREIGN KEY (engagement_id) REFERENCES public.audit_engagement(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_profile
    ADD CONSTRAINT user_profile_auth_user_fk FOREIGN KEY (user_id) REFERENCES auth.users(id) ON DELETE CASCADE;

ALTER TABLE public.audit_engagement ENABLE ROW LEVEL SECURITY;

CREATE POLICY audit_engagement_owner ON public.audit_engagement USING ((owner_id = auth.uid())) WITH CHECK ((owner_id = auth.uid()));

ALTER TABLE public.engagement_note ENABLE ROW LEVEL SECURITY;

CREATE POLICY engagement_note_owner ON public.engagement_note USING ((owner_id = auth.uid())) WITH CHECK ((owner_id = auth.uid()));

ALTER TABLE public.sync_change ENABLE ROW LEVEL SECURITY;

CREATE POLICY sync_change_owner ON public.sync_change USING ((owner_id = auth.uid())) WITH CHECK ((owner_id = auth.uid()));

ALTER TABLE public.sync_clock ENABLE ROW LEVEL SECURITY;

CREATE POLICY sync_clock_owner ON public.sync_clock USING ((owner_id = auth.uid())) WITH CHECK ((owner_id = auth.uid()));

ALTER TABLE public.sync_receipt ENABLE ROW LEVEL SECURITY;

CREATE POLICY sync_receipt_owner ON public.sync_receipt USING ((owner_id = auth.uid())) WITH CHECK ((owner_id = auth.uid()));

ALTER TABLE public.sync_record ENABLE ROW LEVEL SECURITY;

CREATE POLICY sync_record_owner ON public.sync_record USING ((owner_id = auth.uid())) WITH CHECK ((owner_id = auth.uid()));

ALTER TABLE public.user_profile ENABLE ROW LEVEL SECURITY;

CREATE POLICY user_profile_owner ON public.user_profile USING ((user_id = auth.uid())) WITH CHECK ((user_id = auth.uid()));
