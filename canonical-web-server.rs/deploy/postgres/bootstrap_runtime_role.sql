-- Run this as the migration/table owner after `canonical-web-server migrate`.
-- The role is intentionally created without a password: set its password (or
-- another authentication mechanism) outside source control before using it.
-- Re-run this file after migrations add tables so the explicit allow-list
-- remains the complete set of objects available to the application.

DO $bootstrap$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'canonical_web_server') THEN
    CREATE ROLE canonical_web_server
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

-- Reassert the security properties on every run without changing an existing
-- password or other externally managed authentication material.
ALTER ROLE canonical_web_server
  LOGIN
  NOSUPERUSER
  NOCREATEDB
  NOCREATEROLE
  NOINHERIT
  NOREPLICATION
  NOBYPASSRLS;

-- NOINHERIT does not prevent an explicit SET ROLE into a granted parent, so a
-- runtime login with memberships is not accepted as least privilege.
DO $bootstrap$
DECLARE
  runtime_oid oid := (SELECT oid FROM pg_roles WHERE rolname = 'canonical_web_server');
BEGIN
  IF EXISTS (
    SELECT 1 FROM pg_auth_members
    WHERE member = runtime_oid OR roleid = runtime_oid
  ) THEN
    RAISE EXCEPTION 'canonical_web_server must have no role memberships or members';
  END IF;
  IF EXISTS (
    SELECT 1 FROM pg_database
    WHERE datname = current_database() AND datdba = runtime_oid
  ) OR EXISTS (
    SELECT 1 FROM pg_namespace
    WHERE nspname IN ('public', 'auth') AND nspowner = runtime_oid
  ) OR EXISTS (
    SELECT 1
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname IN ('public', 'auth')
      AND c.relowner = runtime_oid
  ) OR EXISTS (
    SELECT 1
    FROM pg_proc p
    JOIN pg_namespace n ON n.oid = p.pronamespace
    WHERE n.nspname IN ('public', 'auth')
      AND p.proowner = runtime_oid
  ) THEN
    RAISE EXCEPTION 'canonical_web_server must not own the database, schemas, or application tables';
  END IF;
END
$bootstrap$;

DO $bootstrap$
BEGIN
  EXECUTE format(
    'GRANT CONNECT ON DATABASE %I TO canonical_web_server',
    current_database()
  );
END
$bootstrap$;

REVOKE CREATE ON SCHEMA public, auth FROM canonical_web_server;
GRANT USAGE ON SCHEMA public, auth TO canonical_web_server;

-- A grant inherited from PUBLIC is additive and cannot be negated for one
-- role. Fail closed if this database has a permissive schema default rather
-- than silently giving the runtime process DDL rights.
DO $bootstrap$
BEGIN
  IF has_schema_privilege('canonical_web_server', 'public', 'CREATE')
     OR has_schema_privilege('canonical_web_server', 'auth', 'CREATE') THEN
    RAISE EXCEPTION
      'canonical_web_server inherits CREATE on public or auth; revoke that CREATE grant before bootstrapping';
  END IF;
END
$bootstrap$;

-- Clear direct grants before rebuilding the least-privilege allow-list. The
-- role does not own these objects and cannot bypass their forced RLS policies.
REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA public FROM canonical_web_server;
REVOKE ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public FROM canonical_web_server;
REVOKE ALL PRIVILEGES ON ALL FUNCTIONS IN SCHEMA public FROM canonical_web_server;
REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA auth FROM canonical_web_server;
REVOKE ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA auth FROM canonical_web_server;
REVOKE ALL PRIVILEGES ON ALL FUNCTIONS IN SCHEMA auth FROM canonical_web_server;

-- PUBLIC and parent-role grants are additive and cannot be negated for only
-- this login. Validate effective privileges after direct grants are cleared,
-- before rebuilding the exact table/function allow-list below. auth.uid() is
-- the sole inherited helper accepted because every RLS policy depends on it.
DO $bootstrap$
BEGIN
  IF EXISTS (
    SELECT 1
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname IN ('public', 'auth')
      AND has_schema_privilege('canonical_web_server', n.oid, 'USAGE')
      AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
      AND (
        has_table_privilege('canonical_web_server', c.oid, 'SELECT')
        OR has_table_privilege('canonical_web_server', c.oid, 'INSERT')
        OR has_table_privilege('canonical_web_server', c.oid, 'UPDATE')
        OR has_table_privilege('canonical_web_server', c.oid, 'DELETE')
        OR has_table_privilege('canonical_web_server', c.oid, 'TRUNCATE')
        OR has_table_privilege('canonical_web_server', c.oid, 'REFERENCES')
        OR has_table_privilege('canonical_web_server', c.oid, 'TRIGGER')
      )
  ) THEN
    RAISE EXCEPTION
      'canonical_web_server inherits a public/auth table privilege; revoke PUBLIC/parent grants before bootstrapping';
  END IF;
  IF EXISTS (
    SELECT 1
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname IN ('public', 'auth')
      AND has_schema_privilege('canonical_web_server', n.oid, 'USAGE')
      AND c.relkind = 'S'
      AND (
        has_sequence_privilege('canonical_web_server', c.oid, 'USAGE')
        OR has_sequence_privilege('canonical_web_server', c.oid, 'SELECT')
        OR has_sequence_privilege('canonical_web_server', c.oid, 'UPDATE')
      )
  ) THEN
    RAISE EXCEPTION
      'canonical_web_server inherits a public/auth sequence privilege; revoke PUBLIC/parent grants before bootstrapping';
  END IF;
  IF EXISTS (
    SELECT 1
    FROM pg_proc p
    JOIN pg_namespace n ON n.oid = p.pronamespace
    WHERE n.nspname IN ('public', 'auth')
      AND has_schema_privilege('canonical_web_server', n.oid, 'USAGE')
      AND NOT (n.nspname = 'auth' AND p.oid = 'auth.uid()'::regprocedure)
      AND has_function_privilege('canonical_web_server', p.oid, 'EXECUTE')
  ) THEN
    RAISE EXCEPTION
      'canonical_web_server inherits EXECUTE on a non-allow-listed public/auth function; revoke PUBLIC/parent EXECUTE before bootstrapping';
  END IF;
END
$bootstrap$;

GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE
  public.user_profile,
  public.web_session,
  public.sync_record,
  public.sync_clock,
  public.sync_change,
  public.sync_receipt,
  public.audit_engagement,
  public.engagement_note
TO canonical_web_server;


GRANT SELECT ON TABLE
  public.canonical_context
TO canonical_web_server;

GRANT SELECT, INSERT, UPDATE ON TABLE
  public.compliance_quote
TO canonical_web_server;

-- No current customer table uses a sequence. A future sequence must be
-- reviewed and granted by exact name; never grant every current/future public
-- sequence to the customer process.

GRANT EXECUTE ON FUNCTION auth.uid() TO canonical_web_server;

-- Deliberately absent from the allow-list above: admin_role_assignment,
-- admin_audit_event, and both canonical_admin_* SECURITY DEFINER functions.
-- The customer application can never convert an authenticated user into an
-- administrative database actor.
