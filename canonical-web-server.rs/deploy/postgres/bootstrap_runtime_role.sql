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
  IF EXISTS (SELECT 1 FROM pg_auth_members WHERE member = runtime_oid) THEN
    RAISE EXCEPTION 'canonical_web_server must not be a member of another role';
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
    WHERE n.nspname = 'public'
      AND c.relname IN (
        'user_profile', 'web_session', 'sync_record', 'sync_clock',
        'sync_change', 'sync_receipt'
      )
      AND c.relowner = runtime_oid
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
REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA auth FROM canonical_web_server;

GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE
  public.user_profile,
  public.web_session,
  public.sync_record,
  public.sync_clock,
  public.sync_change,
  public.sync_receipt
TO canonical_web_server;

-- No current table uses a sequence, but granting only sequence value access
-- keeps this bootstrap compatible with an explicitly approved future serial
-- key without granting ownership or DDL privileges.
GRANT USAGE, SELECT, UPDATE ON ALL SEQUENCES IN SCHEMA public
TO canonical_web_server;

GRANT EXECUTE ON FUNCTION auth.uid() TO canonical_web_server;
