-- Canonical Supabase Postgres reference schema for canonical.cloud.
--
-- Apply this DDL with a privileged migration role. The canonical-web-server
-- runtime must use a separate least-privilege role without BYPASSRLS and must
-- install the verified owner in transaction-local request.jwt.claims before
-- touching owner-scoped rows. The SeaORM migration remains the executable
-- runtime migration; keep these shared shapes and its constraints in sync.
--
-- Browser clients never connect to these tables with database credentials.
-- web_session is intentionally not duplicated here: its server-owned migration
-- stores Supabase access and refresh tokens encrypted, never as plaintext.

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

-- Supabase-auth identity projection. auth.users remains the identity source of
-- truth; this table contains only application profile data.
create table if not exists user_profile (
    user_id       uuid primary key references auth.users(id) on delete cascade,
    email         text        not null,
    display_name  text,
    created_at    timestamptz not null,
    updated_at    timestamptz not null
);

-- One row per owner. Lock this row FOR UPDATE before assigning a cursor. The
-- lock, record mutation, clock increment, sync_change insert, and sync_receipt
-- insert must commit in one transaction so cursors reflect commit order without
-- gaps that a client could accidentally advance past.
create table if not exists sync_clock (
    owner_id  uuid primary key,
    cursor    bigint not null default 0 check (cursor >= 0)
);

-- Authoritative current record state. Versions are Postgres bigint values here
-- and unsigned decimal strings on the JSON wire to avoid JavaScript precision
-- loss. Tombstoned record IDs are permanent and must not be reused.
create table if not exists sync_record (
    owner_id    uuid        not null,
    collection  text        not null,
    record_id   uuid        not null,
    version     bigint      not null check (version > 0),
    payload     jsonb       not null,
    deleted_at  timestamptz,
    updated_at  timestamptz not null,
    primary key (owner_id, collection, record_id)
);

-- Append-only, owner-local change log. (owner_id, cursor) is the stable pull
-- order and every payload is a complete wire snapshot, including tombstones.
create table if not exists sync_change (
    owner_id    uuid        not null,
    cursor      bigint      not null check (cursor > 0),
    collection  text        not null,
    record_id   uuid        not null,
    version     bigint      not null check (version > 0),
    operation   text        not null check (operation in ('put', 'delete')),
    payload     jsonb       not null,
    changed_at  timestamptz not null,
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
    request_hash text        not null,
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
