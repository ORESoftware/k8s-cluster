------------------------------------------------------------------------------
-- Shared Auth principals authorized to operate a Quaestor billing tenant
--
-- Authentication lives in Shared Auth. Tenant ownership and financial scopes
-- are deliberately local to Quaestor so grants are auditable, immediately
-- revocable, and cannot be forged through user-writable identity metadata.
------------------------------------------------------------------------------

create table if not exists tenant_memberships (
  tenant_id                    uuid not null references tenants(id) on delete cascade,
  shared_user_id               text not null,
  role                         text not null,
  scopes                       text[] not null,
  granted_by_shared_user_id    text not null,
  revoked_at                   timestamptz,
  created_at                   timestamptz not null default now(),
  updated_at                   timestamptz not null default now(),
  primary key (tenant_id, shared_user_id),
  constraint tenant_memberships_subject_length check (
    length(shared_user_id) between 1 and 200
    and length(granted_by_shared_user_id) between 1 and 200
  ),
  constraint tenant_memberships_role check (
    role in ('owner', 'admin', 'billing', 'reader')
  ),
  constraint tenant_memberships_scopes check (
    cardinality(scopes) between 1 and 3
    and scopes <@ array['billing:read', 'billing:write', 'billing:admin']::text[]
    and scopes @> array['billing:read']::text[]
  ),
  constraint tenant_memberships_role_scope check (
    (
      role in ('owner', 'admin')
      and scopes @> array['billing:read', 'billing:write', 'billing:admin']::text[]
    )
    or (
      role = 'billing'
      and scopes <@ array['billing:read', 'billing:write']::text[]
    )
    or (role = 'reader' and scopes = array['billing:read']::text[])
  )
);

create index if not exists tenant_memberships_principal_idx
  on tenant_memberships (shared_user_id, tenant_id)
  where revoked_at is null;

create index if not exists tenant_memberships_active_tenant_idx
  on tenant_memberships (tenant_id, role)
  where revoked_at is null;

create table if not exists tenant_membership_events (
  id                           bigserial primary key,
  tenant_id                    uuid not null references tenants(id) on delete restrict,
  shared_user_id               text not null,
  actor_shared_user_id         text not null,
  event_type                   text not null
                               check (event_type in ('create_owner', 'grant_or_update', 'revoke')),
  role                         text,
  scopes                       text[] not null default array[]::text[],
  occurred_at                  timestamptz not null default now(),
  constraint tenant_membership_events_subject_length check (
    length(shared_user_id) between 1 and 200
    and length(actor_shared_user_id) between 1 and 200
  )
);

create index if not exists tenant_membership_events_tenant_time_idx
  on tenant_membership_events (tenant_id, occurred_at desc, id desc);

create index if not exists tenant_membership_events_subject_time_idx
  on tenant_membership_events (shared_user_id, occurred_at desc, id desc);

create or replace function tenant_membership_events_immutable()
returns trigger
language plpgsql
as $$
begin
  raise exception 'tenant membership audit events are append-only';
end;
$$;

drop trigger if exists tenant_membership_events_no_update on tenant_membership_events;
create trigger tenant_membership_events_no_update
  before update on tenant_membership_events
  for each row execute function tenant_membership_events_immutable();

drop trigger if exists tenant_membership_events_no_delete on tenant_membership_events;
create trigger tenant_membership_events_no_delete
  before delete on tenant_membership_events
  for each row execute function tenant_membership_events_immutable();

-- A tenant may never lose its final active owner. Deferred evaluation allows
-- tenant creation and its first owner grant to commit atomically.
create or replace function tenant_membership_change_must_retain_owner()
returns trigger
language plpgsql
as $$
declare
  affected_tenant uuid;
begin
  if tg_op = 'DELETE' then
    affected_tenant := old.tenant_id;
  else
    affected_tenant := new.tenant_id;
  end if;

  if exists (select 1 from tenants where id = affected_tenant)
     and not exists (
       select 1
       from tenant_memberships
       where tenant_id = affected_tenant
         and role = 'owner'
         and revoked_at is null
     ) then
    raise exception 'tenant % must retain at least one active owner', affected_tenant;
  end if;
  return null;
end;
$$;

drop trigger if exists tenant_memberships_require_owner on tenant_memberships;
create constraint trigger tenant_memberships_require_owner
  after insert or update or delete on tenant_memberships
  deferrable initially deferred
  for each row execute function tenant_membership_change_must_retain_owner();

-- Close the other side of the invariant: every newly-created tenant must gain
-- its first owner in the same transaction. Existing tenants are intentionally
-- not rejected at migration time; operators backfill them before enabling
-- user-only routing (see scripts/bootstrap-tenant-owner.sql).
create or replace function new_tenant_must_have_active_owner()
returns trigger
language plpgsql
as $$
begin
  if not exists (
    select 1
    from tenant_memberships
    where tenant_id = new.id
      and role = 'owner'
      and revoked_at is null
  ) then
    raise exception 'new tenant % must be created with an active owner', new.id;
  end if;
  return null;
end;
$$;

drop trigger if exists tenants_require_owner_on_insert on tenants;
create constraint trigger tenants_require_owner_on_insert
  after insert on tenants
  deferrable initially deferred
  for each row execute function new_tenant_must_have_active_owner();
