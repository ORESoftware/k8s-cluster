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
-- bootstrap_runtime_role.sql and bootstrap_admin_role.sql (dpm deliberately
-- does not diff grants).
--
-- The SeaORM migration in crates/canonical-store/src/migration.rs remains the executable runtime
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

CREATE TABLE public.admin_audit_event (
    id uuid NOT NULL,
    actor_id uuid NOT NULL,
    capability text NOT NULL,
    target_type character varying NOT NULL,
    target_id character varying,
    request_id uuid NOT NULL,
    occurred_at timestamp with time zone NOT NULL,
    outcome text NOT NULL,
    metadata jsonb NOT NULL,
    CONSTRAINT admin_audit_event_capability_check CHECK ((capability IN ('user.read', 'user.invite', 'user.disable', 'engagement.read', 'engagement.write', 'role.manage', 'audit.read', 'audit.write'))),
    CONSTRAINT admin_audit_event_outcome_check CHECK ((outcome IN ('succeeded', 'denied', 'failed')))
);

ALTER TABLE ONLY public.admin_audit_event FORCE ROW LEVEL SECURITY;

CREATE TABLE public.admin_role_assignment (
    user_id uuid NOT NULL,
    role text NOT NULL,
    granted_by uuid NOT NULL,
    granted_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    CONSTRAINT admin_role_assignment_role_check CHECK ((role IN ('support', 'user_admin', 'compliance_admin', 'security_admin')))
);

ALTER TABLE ONLY public.admin_role_assignment FORCE ROW LEVEL SECURITY;

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
    operation text NOT NULL,
    payload jsonb NOT NULL,
    changed_at timestamp with time zone NOT NULL,
    CONSTRAINT sync_change_cursor_check CHECK ((cursor > 0)),
    CONSTRAINT sync_change_operation_check CHECK ((operation IN ('put', 'delete'))),
    CONSTRAINT sync_change_version_check CHECK ((version > 0))
);

ALTER TABLE ONLY public.sync_change FORCE ROW LEVEL SECURITY;

CREATE TABLE public.sync_clock (
    owner_id uuid NOT NULL,
    cursor bigint DEFAULT 0 NOT NULL,
    CONSTRAINT sync_clock_cursor_check CHECK ((cursor >= 0))
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
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT sync_record_version_check CHECK ((version > 0))
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
    refresh_lease_id uuid,
    refresh_lease_expires_at timestamp with time zone,
    csrf_token character varying NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    revocation_pending_at timestamp with time zone,
    revocation_next_attempt_at timestamp with time zone,
    revocation_attempts integer DEFAULT 0 NOT NULL,
    upstream_revoked_at timestamp with time zone,
    revocation_abandoned_at timestamp with time zone,
    revocation_failure_kind character varying
);

ALTER TABLE ONLY public.web_session FORCE ROW LEVEL SECURITY;

ALTER TABLE ONLY public.audit_engagement
    ADD CONSTRAINT audit_engagement_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.admin_audit_event
    ADD CONSTRAINT admin_audit_event_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.admin_role_assignment
    ADD CONSTRAINT admin_role_assignment_pkey PRIMARY KEY (user_id, role);

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

CREATE INDEX admin_audit_event_actor_occurred_idx ON public.admin_audit_event USING btree (actor_id, occurred_at);

CREATE INDEX admin_audit_event_request_idx ON public.admin_audit_event USING btree (request_id);

CREATE INDEX admin_role_assignment_active_idx ON public.admin_role_assignment USING btree (user_id, revoked_at);

CREATE INDEX audit_engagement_owner_status_idx ON public.audit_engagement USING btree (owner_id, status);

CREATE INDEX engagement_note_engagement_created_idx ON public.engagement_note USING btree (engagement_id, created_at);

CREATE INDEX engagement_note_owner_idx ON public.engagement_note USING btree (owner_id);

CREATE INDEX sync_change_owner_cursor_idx ON public.sync_change USING btree (owner_id, cursor);

CREATE INDEX web_session_revocation_retry_idx ON public.web_session USING btree (revocation_next_attempt_at);

CREATE INDEX web_session_supabase_revocation_idx ON public.web_session USING btree (supabase_session_id, revoked_at);

CREATE INDEX web_session_user_id_idx ON public.web_session USING btree (user_id);

ALTER TABLE ONLY public.audit_engagement
    ADD CONSTRAINT audit_engagement_auth_user_fk FOREIGN KEY (owner_id) REFERENCES auth.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.admin_audit_event
    ADD CONSTRAINT admin_audit_event_actor_fk FOREIGN KEY (actor_id) REFERENCES auth.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.admin_role_assignment
    ADD CONSTRAINT admin_role_assignment_granted_by_fk FOREIGN KEY (granted_by) REFERENCES auth.users(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.admin_role_assignment
    ADD CONSTRAINT admin_role_assignment_user_fk FOREIGN KEY (user_id) REFERENCES auth.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.engagement_note
    ADD CONSTRAINT engagement_note_auth_user_fk FOREIGN KEY (owner_id) REFERENCES auth.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.engagement_note
    ADD CONSTRAINT engagement_note_engagement_fk FOREIGN KEY (engagement_id) REFERENCES public.audit_engagement(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_profile
    ADD CONSTRAINT user_profile_auth_user_fk FOREIGN KEY (user_id) REFERENCES auth.users(id) ON DELETE CASCADE;

CREATE FUNCTION public.canonical_admin_has_capability(requested_capability text) RETURNS boolean
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path TO 'pg_catalog', 'public'
    AS $function$
      SELECT COALESCE(
        (
          COALESCE(
            NULLIF(current_setting('request.jwt.claims', true), ''),
            '{}'
          )::jsonb ->> 'aal'
        ) = 'aal2',
        false
      )
      AND auth.uid() IS NOT NULL
      AND EXISTS (
          SELECT 1
          FROM public.admin_role_assignment assignment
          WHERE assignment.user_id = auth.uid()
            AND assignment.revoked_at IS NULL
            AND CASE assignment.role
              WHEN 'support' THEN requested_capability IN (
                'user.read', 'engagement.read', 'audit.write'
              )
              WHEN 'user_admin' THEN requested_capability IN (
                'user.read', 'user.invite', 'user.disable', 'audit.write'
              )
              WHEN 'compliance_admin' THEN requested_capability IN (
                'engagement.read', 'engagement.write', 'audit.write'
              )
              WHEN 'security_admin' THEN requested_capability IN (
                'user.read', 'user.invite', 'user.disable',
                'engagement.read', 'engagement.write', 'role.manage',
                'audit.read', 'audit.write'
              )
              ELSE false
            END
        )
    $function$;

CREATE FUNCTION public.canonical_admin_append_audit(event_id uuid, requested_capability text, target_type character varying, target_id character varying, request_id uuid, outcome text, metadata jsonb) RETURNS uuid
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'pg_catalog', 'public'
    AS $function$
BEGIN
  IF NOT public.canonical_admin_has_capability('audit.write') THEN
    RAISE EXCEPTION 'actor is not an active administrator'
      USING ERRCODE = '42501';
  END IF;
  IF outcome = 'succeeded'
     AND NOT public.canonical_admin_has_capability(requested_capability) THEN
    RAISE EXCEPTION 'actor lacks the recorded capability'
      USING ERRCODE = '42501';
  END IF;
  INSERT INTO public.admin_audit_event (
    id, actor_id, capability, target_type, target_id,
    request_id, occurred_at, outcome, metadata
  ) VALUES (
    event_id, auth.uid(), requested_capability, target_type, target_id,
    request_id, now(), outcome, metadata
  );
  RETURN event_id;
END
    $function$;

ALTER TABLE public.admin_audit_event ENABLE ROW LEVEL SECURITY;

ALTER TABLE public.admin_role_assignment ENABLE ROW LEVEL SECURITY;

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

ALTER TABLE public.web_session ENABLE ROW LEVEL SECURITY;

CREATE POLICY web_session_process_boundary ON public.web_session USING (((CURRENT_USER = 'canonical_web_server'::name) OR ((CURRENT_USER = 'canonical_session_revoker'::name) AND (current_setting('canonical.system_task'::text, true) = 'session_revocation'::text)))) WITH CHECK (((CURRENT_USER = 'canonical_web_server'::name) OR ((CURRENT_USER = 'canonical_session_revoker'::name) AND (current_setting('canonical.system_task'::text, true) = 'session_revocation'::text))));

-- Compliance quote desired state. Runtime grants are deliberately
-- maintained in bootstrap_runtime_role.sql because dpm does not diff grants.
CREATE TABLE public.canonical_context (
  id uuid PRIMARY KEY,
  context_key text NOT NULL,
  version integer NOT NULL CHECK (version > 0),
  context_markdown text NOT NULL CHECK (octet_length(context_markdown) <= 65536),
  active boolean NOT NULL DEFAULT true,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (context_key, version)
);

CREATE INDEX canonical_context_active_key_idx
  ON public.canonical_context (context_key, active, version DESC);

CREATE TABLE public.compliance_quote (
  id uuid PRIMARY KEY,
  owner_id uuid NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
  status text NOT NULL CHECK (status IN ('queued', 'analyzing', 'ready', 'failed')),
  request jsonb NOT NULL CHECK (jsonb_typeof(request) = 'object'),
  analysis jsonb CHECK (analysis IS NULL OR jsonb_typeof(analysis) = 'object'),
  model text NOT NULL CHECK (length(model) BETWEEN 1 AND 128),
  failure_code text CHECK (failure_code IS NULL OR length(failure_code) <= 100),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CHECK ((status = 'ready') = (analysis IS NOT NULL)),
  CHECK (status = 'failed' OR failure_code IS NULL)
);

CREATE INDEX compliance_quote_owner_created_idx
  ON public.compliance_quote (owner_id, created_at DESC);
CREATE INDEX compliance_quote_owner_status_idx
  ON public.compliance_quote (owner_id, status, updated_at DESC);

ALTER TABLE public.canonical_context ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.canonical_context FORCE ROW LEVEL SECURITY;
ALTER TABLE public.compliance_quote ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.compliance_quote FORCE ROW LEVEL SECURITY;

CREATE POLICY canonical_context_runtime_read ON public.canonical_context
  FOR SELECT
  USING (current_user = 'canonical_web_server' AND active = true);

CREATE POLICY compliance_quote_owner ON public.compliance_quote
  USING (owner_id = auth.uid())
  WITH CHECK (owner_id = auth.uid());
