-- Run this as the migration/table owner after the full migration chain.
-- This role is for a future separately deployed admin server. It is not the
-- customer web server role and must use independent credentials, sessions,
-- origins, and secret-manager scope.

DO $bootstrap$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'canonical_admin_server') THEN
    CREATE ROLE canonical_admin_server
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

ALTER ROLE canonical_admin_server
  LOGIN
  NOSUPERUSER
  NOCREATEDB
  NOCREATEROLE
  NOINHERIT
  NOREPLICATION
  NOBYPASSRLS;

DO $bootstrap$
DECLARE
  runtime_oid oid := (SELECT oid FROM pg_roles WHERE rolname = 'canonical_admin_server');
BEGIN
  IF EXISTS (
    SELECT 1 FROM pg_auth_members
    WHERE member = runtime_oid OR roleid = runtime_oid
  ) THEN
    RAISE EXCEPTION 'canonical_admin_server must have no role memberships or members';
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
    RAISE EXCEPTION 'canonical_admin_server must not own database objects';
  END IF;
END
$bootstrap$;

DO $bootstrap$
BEGIN
  EXECUTE format(
    'GRANT CONNECT ON DATABASE %I TO canonical_admin_server',
    current_database()
  );
END
$bootstrap$;

REVOKE CREATE ON SCHEMA public, auth FROM canonical_admin_server;
REVOKE USAGE ON SCHEMA auth FROM canonical_admin_server;
GRANT USAGE ON SCHEMA public TO canonical_admin_server;

DO $bootstrap$
BEGIN
  IF has_schema_privilege('canonical_admin_server', 'public', 'CREATE')
     OR has_schema_privilege('canonical_admin_server', 'auth', 'CREATE')
     OR has_schema_privilege('canonical_admin_server', 'auth', 'USAGE') THEN
    RAISE EXCEPTION
      'canonical_admin_server inherits CREATE or auth USAGE; revoke the PUBLIC/parent schema grant first';
  END IF;
END
$bootstrap$;

-- Rebuild a function-only allow-list. The admin runtime has no direct table or
-- sequence privileges, cannot bypass RLS, and cannot alter role assignments or
-- audit rows. New administrative operations require a separately reviewed,
-- capability-checking SECURITY DEFINER function and an explicit grant here.
REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA public FROM canonical_admin_server;
REVOKE ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public FROM canonical_admin_server;
REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA auth FROM canonical_admin_server;
REVOKE ALL PRIVILEGES ON ALL FUNCTIONS IN SCHEMA public FROM canonical_admin_server;
REVOKE ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA auth FROM canonical_admin_server;
REVOKE ALL PRIVILEGES ON ALL FUNCTIONS IN SCHEMA auth FROM canonical_admin_server;

-- Refuse any effective PUBLIC/parent-role object access after direct grants
-- are cleared. The two reviewed procedures are granted only after this check.
DO $bootstrap$
BEGIN
  IF EXISTS (
    SELECT 1
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname IN ('public', 'auth')
      AND has_schema_privilege('canonical_admin_server', n.oid, 'USAGE')
      AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
      AND (
        has_table_privilege('canonical_admin_server', c.oid, 'SELECT')
        OR has_table_privilege('canonical_admin_server', c.oid, 'INSERT')
        OR has_table_privilege('canonical_admin_server', c.oid, 'UPDATE')
        OR has_table_privilege('canonical_admin_server', c.oid, 'DELETE')
        OR has_table_privilege('canonical_admin_server', c.oid, 'TRUNCATE')
        OR has_table_privilege('canonical_admin_server', c.oid, 'REFERENCES')
        OR has_table_privilege('canonical_admin_server', c.oid, 'TRIGGER')
      )
  ) THEN
    RAISE EXCEPTION
      'canonical_admin_server inherits a public/auth table privilege; revoke PUBLIC/parent grants before bootstrapping';
  END IF;
  IF EXISTS (
    SELECT 1
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname IN ('public', 'auth')
      AND has_schema_privilege('canonical_admin_server', n.oid, 'USAGE')
      AND c.relkind = 'S'
      AND (
        has_sequence_privilege('canonical_admin_server', c.oid, 'USAGE')
        OR has_sequence_privilege('canonical_admin_server', c.oid, 'SELECT')
        OR has_sequence_privilege('canonical_admin_server', c.oid, 'UPDATE')
      )
  ) THEN
    RAISE EXCEPTION
      'canonical_admin_server inherits a public/auth sequence privilege; revoke PUBLIC/parent grants before bootstrapping';
  END IF;
  IF EXISTS (
    SELECT 1
    FROM pg_proc p
    JOIN pg_namespace n ON n.oid = p.pronamespace
    WHERE n.nspname IN ('public', 'auth')
      AND has_schema_privilege('canonical_admin_server', n.oid, 'USAGE')
      AND has_function_privilege('canonical_admin_server', p.oid, 'EXECUTE')
  ) THEN
    RAISE EXCEPTION
      'canonical_admin_server inherits EXECUTE on a public/auth function; revoke PUBLIC/parent EXECUTE before bootstrapping';
  END IF;
END
$bootstrap$;

GRANT EXECUTE ON FUNCTION
  public.canonical_admin_has_capability(text),
  public.canonical_admin_append_audit(uuid, text, varchar, varchar, uuid, text, jsonb)
TO canonical_admin_server;
