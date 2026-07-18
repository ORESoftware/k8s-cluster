-- Run this as the migration/table owner after `canonical-web-server migrate`.
-- The no-ingress revoker gets its own credential and only the table operations
-- needed to reconcile encrypted Supabase logout state.

DO $bootstrap$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'canonical_session_revoker') THEN
    CREATE ROLE canonical_session_revoker
      LOGIN
      NOSUPERUSER
      NOCREATEDB
      NOCREATEROLE
      NOINHERIT
      NOREPLICATION
      NOBYPASSRLS;
  END IF;
END
$bootstrap$;

ALTER ROLE canonical_session_revoker
  LOGIN
  NOSUPERUSER
  NOCREATEDB
  NOCREATEROLE
  NOINHERIT
  NOREPLICATION
  NOBYPASSRLS;

DO $bootstrap$
DECLARE
  revoker_oid oid := (SELECT oid FROM pg_roles WHERE rolname = 'canonical_session_revoker');
BEGIN
  IF EXISTS (
    SELECT 1 FROM pg_auth_members
    WHERE member = revoker_oid OR roleid = revoker_oid
  ) THEN
    RAISE EXCEPTION 'canonical_session_revoker must have no role memberships or members';
  END IF;
  IF EXISTS (
    SELECT 1 FROM pg_database
    WHERE datname = current_database() AND datdba = revoker_oid
  ) OR EXISTS (
    SELECT 1 FROM pg_namespace
    WHERE nspname IN ('public', 'auth') AND nspowner = revoker_oid
  ) OR EXISTS (
    SELECT 1
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname IN ('public', 'auth')
      AND c.relowner = revoker_oid
  ) OR EXISTS (
    SELECT 1
    FROM pg_proc p
    JOIN pg_namespace n ON n.oid = p.pronamespace
    WHERE n.nspname IN ('public', 'auth')
      AND p.proowner = revoker_oid
  ) THEN
    RAISE EXCEPTION 'canonical_session_revoker must not own database objects';
  END IF;
END
$bootstrap$;

DO $bootstrap$
BEGIN
  EXECUTE format(
    'GRANT CONNECT ON DATABASE %I TO canonical_session_revoker',
    current_database()
  );
END
$bootstrap$;

REVOKE CREATE ON SCHEMA public, auth FROM canonical_session_revoker;
REVOKE USAGE ON SCHEMA auth FROM canonical_session_revoker;
GRANT USAGE ON SCHEMA public TO canonical_session_revoker;

DO $bootstrap$
BEGIN
  IF has_schema_privilege('canonical_session_revoker', 'public', 'CREATE')
     OR has_schema_privilege('canonical_session_revoker', 'auth', 'CREATE')
     OR has_schema_privilege('canonical_session_revoker', 'auth', 'USAGE') THEN
    RAISE EXCEPTION
      'canonical_session_revoker inherits CREATE or auth USAGE; revoke the PUBLIC/parent schema grant first';
  END IF;
END
$bootstrap$;

REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA public FROM canonical_session_revoker;
REVOKE ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public FROM canonical_session_revoker;
REVOKE ALL PRIVILEGES ON ALL FUNCTIONS IN SCHEMA public FROM canonical_session_revoker;
REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA auth FROM canonical_session_revoker;
REVOKE ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA auth FROM canonical_session_revoker;
REVOKE ALL PRIVILEGES ON ALL FUNCTIONS IN SCHEMA auth FROM canonical_session_revoker;

-- The revoker needs no tables beyond the explicit web_session grant, no
-- sequences, and no functions. Reject effective PUBLIC/parent-role access
-- after clearing direct grants so the allow-list cannot silently widen.
DO $bootstrap$
BEGIN
  IF EXISTS (
    SELECT 1
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname IN ('public', 'auth')
      AND has_schema_privilege('canonical_session_revoker', n.oid, 'USAGE')
      AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
      AND (
        has_table_privilege('canonical_session_revoker', c.oid, 'SELECT')
        OR has_table_privilege('canonical_session_revoker', c.oid, 'INSERT')
        OR has_table_privilege('canonical_session_revoker', c.oid, 'UPDATE')
        OR has_table_privilege('canonical_session_revoker', c.oid, 'DELETE')
        OR has_table_privilege('canonical_session_revoker', c.oid, 'TRUNCATE')
        OR has_table_privilege('canonical_session_revoker', c.oid, 'REFERENCES')
        OR has_table_privilege('canonical_session_revoker', c.oid, 'TRIGGER')
      )
  ) THEN
    RAISE EXCEPTION
      'canonical_session_revoker inherits a public/auth table privilege; revoke PUBLIC/parent grants before bootstrapping';
  END IF;
  IF EXISTS (
    SELECT 1
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname IN ('public', 'auth')
      AND has_schema_privilege('canonical_session_revoker', n.oid, 'USAGE')
      AND c.relkind = 'S'
      AND (
        has_sequence_privilege('canonical_session_revoker', c.oid, 'USAGE')
        OR has_sequence_privilege('canonical_session_revoker', c.oid, 'SELECT')
        OR has_sequence_privilege('canonical_session_revoker', c.oid, 'UPDATE')
      )
  ) THEN
    RAISE EXCEPTION
      'canonical_session_revoker inherits a public/auth sequence privilege; revoke PUBLIC/parent grants before bootstrapping';
  END IF;
  IF EXISTS (
    SELECT 1
    FROM pg_proc p
    JOIN pg_namespace n ON n.oid = p.pronamespace
    WHERE n.nspname IN ('public', 'auth')
      AND has_schema_privilege('canonical_session_revoker', n.oid, 'USAGE')
      AND has_function_privilege('canonical_session_revoker', p.oid, 'EXECUTE')
  ) THEN
    RAISE EXCEPTION
      'canonical_session_revoker inherits EXECUTE on a public/auth function; revoke PUBLIC/parent EXECUTE before bootstrapping';
  END IF;
END
$bootstrap$;

GRANT SELECT, UPDATE, DELETE ON TABLE public.web_session
TO canonical_session_revoker;
