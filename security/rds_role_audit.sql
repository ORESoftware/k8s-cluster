\set ON_ERROR_STOP on

-- Returns critical/warning rows for the exact Postgres role used by the Sonus
-- Auris backend. A production connection must use TLS and a non-privileged role.
with current_role_attributes as (
    select *
    from pg_catalog.pg_roles
    where rolname = current_user
),
violations(severity, code, object_name, detail) as (
    select
        'critical',
        'tls_disabled',
        current_database(),
        'current backend connection is not encrypted with TLS'
    where not coalesce((
        select s.ssl
        from pg_catalog.pg_stat_ssl s
        where s.pid = pg_backend_pid()
    ), false)

    union all

    select
        'critical',
        'privileged_application_role',
        current_user,
        concat_ws(', ',
            case when rolsuper then 'superuser' end,
            case when rolcreaterole then 'createrole' end,
            case when rolcreatedb then 'createdb' end,
            case when rolreplication then 'replication' end,
            case when rolbypassrls then 'bypassrls' end
        )
    from current_role_attributes
    where rolsuper or rolcreaterole or rolcreatedb or rolreplication or rolbypassrls

    union all

    select
        'critical',
        'public_schema_create',
        'public',
        'PUBLIC can CREATE objects in schema public; revoke CREATE to prevent search_path object injection'
    where has_schema_privilege('public', 'public', 'CREATE')

    union all

    select
        'warning',
        'statement_timeout_disabled',
        current_user,
        'statement_timeout is 0; configure a bounded database/role default in addition to HTTP timeouts'
    where current_setting('statement_timeout') in ('0', '0ms')

    union all

    select
        'warning',
        'lock_timeout_disabled',
        current_user,
        'lock_timeout is 0; configure a bounded value for application sessions'
    where current_setting('lock_timeout') in ('0', '0ms')

    union all

    select
        'warning',
        'idle_transaction_timeout_disabled',
        current_user,
        'idle_in_transaction_session_timeout is 0; configure a bounded value for application sessions'
    where current_setting('idle_in_transaction_session_timeout') in ('0', '0ms')
)
select severity, code, object_name, detail
from violations
order by severity, code, object_name;
