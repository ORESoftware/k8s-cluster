\set ON_ERROR_STOP on

-- Returns one row per security violation. A clean production schema returns no
-- rows. Run as a role that can inspect pg_catalog; no service-role API key is
-- needed or accepted.
with expected_tables(table_schema, table_name) as (
    values
        ('public'::name, 'acoustic_events'::name),
        ('public'::name, 'client_telemetry'::name),
        ('public'::name, 'user_consents'::name),
        ('public'::name, 'user_settings'::name),
        ('public'::name, 'devices'::name),
        ('public'::name, 'entitlements'::name)
),
expected_commands(table_schema, table_name, command) as (
    values
        ('public'::name, 'acoustic_events'::name, 'SELECT'::text),
        ('public'::name, 'acoustic_events'::name, 'INSERT'::text),
        ('public'::name, 'acoustic_events'::name, 'UPDATE'::text),
        ('public'::name, 'acoustic_events'::name, 'DELETE'::text),
        ('public'::name, 'client_telemetry'::name, 'SELECT'::text),
        ('public'::name, 'client_telemetry'::name, 'INSERT'::text),
        ('public'::name, 'user_consents'::name, 'SELECT'::text),
        ('public'::name, 'user_consents'::name, 'INSERT'::text),
        ('public'::name, 'user_settings'::name, 'SELECT'::text),
        ('public'::name, 'user_settings'::name, 'INSERT'::text),
        ('public'::name, 'user_settings'::name, 'UPDATE'::text),
        ('public'::name, 'devices'::name, 'SELECT'::text),
        ('public'::name, 'devices'::name, 'INSERT'::text),
        ('public'::name, 'devices'::name, 'UPDATE'::text),
        ('public'::name, 'devices'::name, 'DELETE'::text),
        ('public'::name, 'entitlements'::name, 'SELECT'::text)
),
relations as (
    select
        n.nspname::name as table_schema,
        c.relname::name as table_name,
        c.relrowsecurity,
        c.relkind
    from pg_catalog.pg_class c
    join pg_catalog.pg_namespace n on n.oid = c.relnamespace
),
policy_commands as (
    select
        schemaname::name as table_schema,
        tablename::name as table_name,
        upper(cmd) as command,
        coalesce(qual, '') as using_expression,
        coalesce(with_check, '') as check_expression
    from pg_catalog.pg_policies
),
violations(severity, code, object_name, detail) as (
    select
        'critical',
        'missing_table',
        format('%I.%I', e.table_schema, e.table_name),
        'expected exposed table does not exist'
    from expected_tables e
    left join relations r using (table_schema, table_name)
    where r.table_name is null

    union all

    select
        'critical',
        'rls_disabled',
        format('%I.%I', e.table_schema, e.table_name),
        'row-level security is not enabled'
    from expected_tables e
    join relations r using (table_schema, table_name)
    where not r.relrowsecurity

    union all

    select
        'critical',
        'missing_user_id',
        format('%I.%I', e.table_schema, e.table_name),
        'owner column user_id is missing'
    from expected_tables e
    where not exists (
        select 1
        from information_schema.columns c
        where c.table_schema = e.table_schema
          and c.table_name = e.table_name
          and c.column_name = 'user_id'
    )

    union all

    select
        'critical',
        'unsafe_user_id_default',
        format('%I.%I', e.table_schema, e.table_name),
        format('user_id default must be auth.uid(); found %s', coalesce(c.column_default, '<none>'))
    from expected_tables e
    join information_schema.columns c
      on c.table_schema = e.table_schema
     and c.table_name = e.table_name
     and c.column_name = 'user_id'
    where coalesce(c.column_default, '') !~* 'auth[.]uid\s*[(]\s*[)]'

    union all

    select
        'critical',
        'missing_owner_policy',
        format('%I.%I:%s', e.table_schema, e.table_name, lower(e.command)),
        'no policy for this command (or ALL) binds the row to auth.uid() and requires passwordless AAL2'
    from expected_commands e
    where not exists (
        select 1
        from policy_commands p
        where p.table_schema = e.table_schema
          and p.table_name = e.table_name
          and p.command in (e.command, 'ALL')
          and case e.command
                when 'INSERT' then
                    p.check_expression ~* 'auth[.]uid\s*[(]\s*[)]'
                    and p.check_expression ~* 'sonus_passwordless_aal2\s*[(]\s*[)]'
                when 'UPDATE' then
                    p.using_expression ~* 'auth[.]uid\s*[(]\s*[)]'
                    and p.check_expression ~* 'auth[.]uid\s*[(]\s*[)]'
                    and p.using_expression ~* 'sonus_passwordless_aal2\s*[(]\s*[)]'
                    and p.check_expression ~* 'sonus_passwordless_aal2\s*[(]\s*[)]'
                else
                    p.using_expression ~* 'auth[.]uid\s*[(]\s*[)]'
                    and p.using_expression ~* 'sonus_passwordless_aal2\s*[(]\s*[)]'
              end
    )

    union all

    select
        'critical',
        'missing_passwordless_aal2_guard',
        'public.sonus_passwordless_aal2()',
        'the shared RLS authentication guard is missing'
    where not exists (
        select 1
        from pg_catalog.pg_proc p
        join pg_catalog.pg_namespace n on n.oid = p.pronamespace
        where n.nspname = 'public'
          and p.proname = 'sonus_passwordless_aal2'
          and pg_get_function_identity_arguments(p.oid) = ''
    )

    union all

    select
        'critical',
        'unsafe_passwordless_aal2_guard',
        'public.sonus_passwordless_aal2()',
        'guard must be SECURITY INVOKER with a fixed empty search_path and enforce AAL2, password rejection, and OTP/magic-link AMR'
    from pg_catalog.pg_proc p
    join pg_catalog.pg_namespace n on n.oid = p.pronamespace
    where n.nspname = 'public'
      and p.proname = 'sonus_passwordless_aal2'
      and pg_get_function_identity_arguments(p.oid) = ''
      and (
          p.prosecdef
          or not exists (
              select 1
              from unnest(coalesce(p.proconfig, array[]::text[])) setting
              where setting in ('search_path=', 'search_path=""')
          )
          or pg_get_functiondef(p.oid) !~* 'aal.*aal2'
          or pg_get_functiondef(p.oid) !~* 'method.*password'
          or pg_get_functiondef(p.oid) !~* 'method.*otp.*magiclink'
      )

    union all

    select
        'critical',
        'anon_table_privilege',
        format('%I.%I', e.table_schema, e.table_name),
        'anon has direct SELECT/INSERT/UPDATE/DELETE privilege on an owner-scoped table'
    from expected_tables e
    where exists (
        select 1
        from information_schema.role_table_grants g
        where g.grantee = 'anon'
          and g.table_schema = e.table_schema
          and g.table_name = e.table_name
          and g.privilege_type in ('SELECT', 'INSERT', 'UPDATE', 'DELETE')
    )

    union all

    select
        'critical',
        'unsafe_security_definer_search_path',
        format('%I.%I(%s)', n.nspname, p.proname, pg_get_function_identity_arguments(p.oid)),
        'SECURITY DEFINER function in an exposed schema has no fixed search_path'
    from pg_catalog.pg_proc p
    join pg_catalog.pg_namespace n on n.oid = p.pronamespace
    where n.nspname in ('public', 'graphql_public')
      and p.prosecdef
      and not exists (
          select 1
          from unnest(coalesce(p.proconfig, array[]::text[])) setting
          where setting ~ '^search_path='
      )
)
select severity, code, object_name, detail
from violations
order by severity, code, object_name;
