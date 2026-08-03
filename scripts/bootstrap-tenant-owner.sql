-- One-time, explicit owner bootstrap for a tenant that predates database-backed
-- Shared Auth memberships.
--
-- Usage (review the exact tenant and Shared Auth subject before execution):
--   psql "$BILLING_DATABASE_URL" \
--     --set=tenant_id='11111111-1111-4111-8111-111111111111' \
--     --set=shared_user_id='shared-auth-subject' \
--     --file=scripts/bootstrap-tenant-owner.sql
--
-- This script intentionally handles one tenant at a time. A global default
-- owner across every GitHub organization/app would create an unnecessarily
-- broad blast radius and is not an acceptable production shortcut.

\set ON_ERROR_STOP on

\if :{?tenant_id}
\else
  \echo 'error: --set=tenant_id=<uuid> is required'
  \quit 2
\endif

\if :{?shared_user_id}
\else
  \echo 'error: --set=shared_user_id=<Shared Auth subject> is required'
  \quit 2
\endif

begin;

-- Lock the tenant row so concurrent provisioning cannot race this bootstrap.
select id, slug, display_name
from tenants
where id = :'tenant_id'::uuid
for update;

\if :ROW_COUNT
\else
  \echo 'error: tenant_id does not exist'
  rollback;
  \quit 3
\endif

do $$
begin
  if length(:'shared_user_id') not between 1 and 200
     or :'shared_user_id' ~ '[[:cntrl:]/\\]' then
    raise exception 'invalid Shared Auth subject';
  end if;
end;
$$;

insert into tenant_memberships (
  tenant_id,
  shared_user_id,
  role,
  scopes,
  granted_by_shared_user_id,
  revoked_at,
  updated_at
)
values (
  :'tenant_id'::uuid,
  :'shared_user_id',
  'owner',
  array['billing:read', 'billing:write', 'billing:admin']::text[],
  :'shared_user_id',
  null,
  now()
)
on conflict (tenant_id, shared_user_id) do update set
  role = excluded.role,
  scopes = excluded.scopes,
  granted_by_shared_user_id = excluded.granted_by_shared_user_id,
  revoked_at = null,
  updated_at = now();

insert into tenant_membership_events (
  tenant_id,
  shared_user_id,
  actor_shared_user_id,
  event_type,
  role,
  scopes
)
values (
  :'tenant_id'::uuid,
  :'shared_user_id',
  :'shared_user_id',
  'create_owner',
  'owner',
  array['billing:read', 'billing:write', 'billing:admin']::text[]
);

commit;

select tenant_id, shared_user_id, role, scopes, revoked_at, created_at, updated_at
from tenant_memberships
where tenant_id = :'tenant_id'::uuid
  and shared_user_id = :'shared_user_id';
