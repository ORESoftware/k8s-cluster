-- Canonical Supabase Postgres reference schema for canonical.cloud.
--
-- Apply this DDL with a privileged migration role. The canonical-web-server
-- runtime must use a separate least-privilege role without BYPASSRLS and must
-- install the verified owner in transaction-local request.jwt.claims before
-- touching owner-scoped rows. The SeaORM migration remains the executable
-- runtime migration; keep these shared shapes and its constraints in sync.
--
-- Browser clients never connect to these tables with database credentials.
-- web_session documents the server-only encrypted-at-rest session shape, but it
-- is not part of any generated wire adapter and receives no browser/customer
-- database grant.

-- Compliance-domain storage. Owner-scoped since 2026-07: audit_engagement
-- gained the owner contract (owner_id + forced owner RLS) that its earlier
-- legacy shape lacked, and per-engagement notes live in engagement_note.
-- The executable migration is canonical-web-server.rs (SeaORM migration +
-- deploy/postgres/schema.sql, the dpm declarative source); keep these shapes
-- in sync with it.

create table if not exists audit_engagement (
    id                  uuid primary key,
    owner_id            uuid        not null references auth.users(id) on delete cascade,
    company             varchar     not null,
    framework           text        not null
        check (framework in ('soc2', 'fedramp', 'hipaa', 'iso_27001', 'pci_dss', 'gdpr')),
    status              text        not null
        check (status in ('scoping', 'remediation', 'in_audit', 'complete')),
    opened_at           timestamptz not null,
    target_report_date  date,
    updated_at          timestamptz not null
);

create index if not exists audit_engagement_owner_idx        on audit_engagement (owner_id);
create index if not exists audit_engagement_owner_status_idx on audit_engagement (owner_id, status);

alter table audit_engagement enable row level security;
alter table audit_engagement force row level security;
create policy audit_engagement_owner on audit_engagement
    using (owner_id = auth.uid()) with check (owner_id = auth.uid());

-- Free-form notes attached to an engagement, same owner contract.
create table if not exists engagement_note (
    id             uuid        primary key,
    engagement_id  uuid        not null references audit_engagement(id) on delete cascade,
    owner_id       uuid        not null references auth.users(id) on delete cascade,
    body           varchar     not null,
    created_at     timestamptz not null
);

create index if not exists engagement_note_engagement_created_idx
    on engagement_note (engagement_id, created_at);
create index if not exists engagement_note_owner_idx on engagement_note (owner_id);

alter table engagement_note enable row level security;
alter table engagement_note force row level security;
create policy engagement_note_owner on engagement_note
    using (owner_id = auth.uid()) with check (owner_id = auth.uid());

-- Capability-oriented administrative authorization. This data is not part of
-- the customer API and receives no owner policy: customer roles are denied by
-- forced RLS and by the absence of grants. Assignments are managed out of band
-- by the migration owner until a separately deployed admin service gains a
-- reviewed role-management function.
create table if not exists admin_role_assignment (
    user_id     uuid        not null references auth.users(id) on delete cascade,
    role        text        not null
        check (role in ('support', 'user_admin', 'compliance_admin', 'security_admin')),
    granted_by  uuid        not null references auth.users(id) on delete restrict,
    granted_at  timestamptz not null,
    revoked_at  timestamptz,
    primary key (user_id, role)
);

create index if not exists admin_role_assignment_active_idx
    on admin_role_assignment (user_id, revoked_at);

alter table admin_role_assignment enable row level security;
alter table admin_role_assignment force row level security;

-- Append-only evidence for privileged actions. The admin runtime gets no
-- direct table privileges; it can append only through the capability-checking
-- SECURITY DEFINER function below. No update/delete function is exposed.
create table if not exists admin_audit_event (
    id          uuid        primary key,
    actor_id    uuid        not null references auth.users(id) on delete restrict,
    capability  text        not null
        check (capability in (
            'user.read', 'user.invite', 'user.disable',
            'engagement.read', 'engagement.write', 'role.manage',
            'audit.read', 'audit.write'
        )),
    target_type varchar     not null,
    target_id   varchar,
    request_id  uuid        not null,
    occurred_at timestamptz not null,
    outcome     text        not null
        check (outcome in ('succeeded', 'denied', 'failed')),
    metadata    jsonb       not null
);

create index if not exists admin_audit_event_actor_occurred_idx
    on admin_audit_event (actor_id, occurred_at);
create index if not exists admin_audit_event_request_idx
    on admin_audit_event (request_id);

alter table admin_audit_event enable row level security;
alter table admin_audit_event force row level security;

create or replace function canonical_admin_has_capability(
    requested_capability text
) returns boolean
language sql
stable
security definer
set search_path = pg_catalog, public
as $function$
    select coalesce(
        (
            coalesce(
                nullif(current_setting('request.jwt.claims', true), ''),
                '{}'
            )::jsonb ->> 'aal'
        ) = 'aal2',
        false
    )
    and auth.uid() is not null
    and exists (
            select 1
            from public.admin_role_assignment assignment
            where assignment.user_id = auth.uid()
              and assignment.revoked_at is null
              and case assignment.role
                when 'support' then requested_capability in (
                    'user.read', 'engagement.read', 'audit.write'
                )
                when 'user_admin' then requested_capability in (
                    'user.read', 'user.invite', 'user.disable', 'audit.write'
                )
                when 'compliance_admin' then requested_capability in (
                    'engagement.read', 'engagement.write', 'audit.write'
                )
                when 'security_admin' then requested_capability in (
                    'user.read', 'user.invite', 'user.disable',
                    'engagement.read', 'engagement.write', 'role.manage',
                    'audit.read', 'audit.write'
                )
                else false
              end
        )
$function$;

create or replace function canonical_admin_append_audit(
    event_id uuid,
    requested_capability text,
    target_type varchar,
    target_id varchar,
    request_id uuid,
    outcome text,
    metadata jsonb
) returns uuid
language plpgsql
volatile
security definer
set search_path = pg_catalog, public
as $function$
begin
    if not public.canonical_admin_has_capability('audit.write') then
        raise exception 'actor is not an active administrator'
            using errcode = '42501';
    end if;
    if outcome = 'succeeded'
       and not public.canonical_admin_has_capability(requested_capability) then
        raise exception 'actor lacks the recorded capability'
            using errcode = '42501';
    end if;
    insert into public.admin_audit_event (
        id, actor_id, capability, target_type, target_id,
        request_id, occurred_at, outcome, metadata
    ) values (
        event_id, auth.uid(), requested_capability, target_type, target_id,
        request_id, now(), outcome, metadata
    );
    return event_id;
end
$function$;

revoke all on table admin_role_assignment, admin_audit_event
    from public, anon, authenticated;
revoke all on function canonical_admin_has_capability(text)
    from public, anon, authenticated;
revoke all on function canonical_admin_append_audit(
    uuid, text, varchar, varchar, uuid, text, jsonb
) from public, anon, authenticated;

-- Server-owned Supabase session state. Access and refresh credentials are
-- encrypted before persistence; their plaintext forms must never become SQL
-- columns, generated API fields, logs, or browser-visible data. Local revocation
-- takes effect immediately, while the terminal/retry fields make upstream
-- Supabase sign-out durable and auditable across process restarts. Nullable
-- refresh leases fence token rotation across replicas without holding a
-- database transaction open during the upstream Supabase request.
create table if not exists web_session (
    id_hash                    varchar     primary key,
    user_id                    uuid        not null,
    email                      varchar     not null,
    supabase_session_id        uuid,
    encrypted_access_token     text        not null,
    encrypted_refresh_token    text        not null,
    access_expires_at          timestamptz not null,
    refresh_lease_id           uuid,
    refresh_lease_expires_at   timestamptz,
    csrf_token                 varchar     not null,
    created_at                 timestamptz not null,
    updated_at                 timestamptz not null,
    expires_at                 timestamptz not null,
    revoked_at                 timestamptz,
    revocation_pending_at      timestamptz,
    revocation_next_attempt_at timestamptz,
    revocation_attempts        integer     not null default 0,
    upstream_revoked_at        timestamptz,
    revocation_abandoned_at    timestamptz,
    revocation_failure_kind    varchar
);

create index if not exists web_session_user_id_idx
    on web_session (user_id);
create index if not exists web_session_revocation_retry_idx
    on web_session (revocation_next_attempt_at);
create index if not exists web_session_supabase_revocation_idx
    on web_session (supabase_session_id, revoked_at);

alter table web_session enable row level security;
alter table web_session force row level security;

-- This is a process boundary rather than customer ownership RLS. The web
-- server may manage its session rows; the no-ingress revoker may touch them
-- through code paths tagged by the transaction helper. The marker catches
-- accidental untagged queries but is caller-settable and is not authorization;
-- the isolated exact database login and narrow table grant are the boundary.
do $$
begin
    if not exists (
        select 1 from pg_policies
        where schemaname = current_schema()
          and tablename = 'web_session'
          and policyname = 'web_session_process_boundary'
    ) then
        create policy web_session_process_boundary on web_session
            using (
                current_user = 'canonical_web_server'
                or (
                    current_user = 'canonical_session_revoker'
                    and current_setting('canonical.system_task', true)
                        = 'session_revocation'
                )
            )
            with check (
                current_user = 'canonical_web_server'
                or (
                    current_user = 'canonical_session_revoker'
                    and current_setting('canonical.system_task', true)
                        = 'session_revocation'
                )
            );
    end if;
end
$$;

revoke all on table web_session from public, anon, authenticated;

-- Supabase-auth identity projection. auth.users remains the identity source of
-- truth; this table contains only application profile data.
create table if not exists user_profile (
    user_id       uuid primary key references auth.users(id) on delete cascade,
    email         varchar     not null,
    display_name  varchar,
    created_at    timestamptz not null,
    updated_at    timestamptz not null
);

-- One row per owner. Lock this row FOR UPDATE before assigning a cursor. The
-- lock, record mutation, clock increment, sync_change insert, and sync_receipt
-- insert must commit in one transaction so cursors reflect commit order without
-- gaps that a client could accidentally advance past.
create table if not exists sync_clock (
    owner_id  uuid primary key,
    cursor    bigint not null default 0,
    constraint sync_clock_cursor_check check (cursor >= 0)
);

-- Authoritative current record state. Versions are Postgres bigint values here
-- and unsigned decimal strings on the JSON wire to avoid JavaScript precision
-- loss. Tombstoned record IDs are permanent and must not be reused.
create table if not exists sync_record (
    owner_id    uuid        not null,
    collection  varchar     not null,
    record_id   uuid        not null,
    version     bigint      not null,
    payload     jsonb       not null,
    deleted_at  timestamptz,
    updated_at  timestamptz not null,
    constraint sync_record_version_check check (version > 0),
    primary key (owner_id, collection, record_id)
);

-- Append-only, owner-local change log. (owner_id, cursor) is the stable pull
-- order and every payload is a complete wire snapshot, including tombstones.
create table if not exists sync_change (
    owner_id    uuid        not null,
    cursor      bigint      not null,
    collection  varchar     not null,
    record_id   uuid        not null,
    version     bigint      not null,
    operation   text        not null,
    payload     jsonb       not null,
    changed_at  timestamptz not null,
    constraint sync_change_cursor_check check (cursor > 0),
    constraint sync_change_operation_check check (operation in ('put', 'delete')),
    constraint sync_change_version_check check (version > 0),
    primary key (owner_id, cursor)
);

create index if not exists sync_change_owner_cursor_idx
    on sync_change (owner_id, cursor);

-- Durable idempotency receipt. request_hash is a digest of the mutation body;
-- result is the previously returned JSON result. Neither column contains an
-- authentication credential.
create table if not exists sync_receipt (
    owner_id     uuid        not null,
    client_id    uuid        not null,
    mutation_id  uuid        not null,
    request_hash varchar     not null,
    result       jsonb       not null,
    created_at   timestamptz not null,
    primary key (owner_id, client_id, mutation_id)
);

alter table user_profile enable row level security;
alter table user_profile force row level security;
alter table sync_clock enable row level security;
alter table sync_clock force row level security;
alter table sync_record enable row level security;
alter table sync_record force row level security;
alter table sync_change enable row level security;
alter table sync_change force row level security;
alter table sync_receipt enable row level security;
alter table sync_receipt force row level security;

-- Policy creation is guarded so the reference DDL can be reapplied while the
-- table definitions continue to use CREATE TABLE IF NOT EXISTS.
do $$
begin
    if not exists (
        select 1 from pg_policies
        where schemaname = current_schema()
          and tablename = 'user_profile'
          and policyname = 'user_profile_owner'
    ) then
        create policy user_profile_owner on user_profile
            using (user_id = auth.uid())
            with check (user_id = auth.uid());
    end if;

    if not exists (
        select 1 from pg_policies
        where schemaname = current_schema()
          and tablename = 'sync_clock'
          and policyname = 'sync_clock_owner'
    ) then
        create policy sync_clock_owner on sync_clock
            using (owner_id = auth.uid())
            with check (owner_id = auth.uid());
    end if;

    if not exists (
        select 1 from pg_policies
        where schemaname = current_schema()
          and tablename = 'sync_record'
          and policyname = 'sync_record_owner'
    ) then
        create policy sync_record_owner on sync_record
            using (owner_id = auth.uid())
            with check (owner_id = auth.uid());
    end if;

    if not exists (
        select 1 from pg_policies
        where schemaname = current_schema()
          and tablename = 'sync_change'
          and policyname = 'sync_change_owner'
    ) then
        create policy sync_change_owner on sync_change
            using (owner_id = auth.uid())
            with check (owner_id = auth.uid());
    end if;

    if not exists (
        select 1 from pg_policies
        where schemaname = current_schema()
          and tablename = 'sync_receipt'
          and policyname = 'sync_receipt_owner'
    ) then
        create policy sync_receipt_owner on sync_receipt
            using (owner_id = auth.uid())
            with check (owner_id = auth.uid());
    end if;
end
$$;

-- Required server transaction shape (illustrative, not a client API):
--
--   begin;
--   select set_config(
--     'request.jwt.claims', json_build_object('sub', :owner_id)::text, true
--   );
--   insert into sync_clock (owner_id, cursor) values (:owner_id, 0)
--     on conflict (owner_id) do nothing;
--   select cursor from sync_clock where owner_id = :owner_id for update;
--   -- validate/idempotency-check, mutate sync_record, advance sync_clock,
--   -- append sync_change, and persist sync_receipt here
--   commit;
--
-- Pull cursors sent to browsers are encrypted, owner-bound application tokens;
-- never expose the raw bigint clock as the resumable REST cursor.
