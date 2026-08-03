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

-- Validate through ordinary psql-substituted SQL. psql does not substitute
-- variables inside dollar-quoted PL/pgSQL bodies, so do not hide these checks
-- in a DO $$ block.
select (
  length(:'shared_user_id') between 1 and 200
  and :'shared_user_id' !~ '[[:cntrl:]]'
  and strpos(:'shared_user_id', '/') = 0
  and strpos(:'shared_user_id', E'\\') = 0
) as subject_valid
\gset

\if :subject_valid
\else
  \echo 'error: invalid Shared Auth subject'
  \quit 2
\endif

begin;

select exists (
  select 1
  from tenants
  where id = :'tenant_id'::uuid
) as tenant_exists
\gset

\if :tenant_exists
\else
  \echo 'error: tenant_id does not exist'
  rollback;
  \quit 3
\endif

-- Lock the tenant row so concurrent provisioning/deletion cannot race this
-- bootstrap after the existence check.
select id, slug, display_name
from tenants
where id = :'tenant_id'::uuid
for update;

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
