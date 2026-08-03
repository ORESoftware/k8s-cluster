#!/usr/bin/env bash
set -euo pipefail

for command in createdb dropdb psql; do
  command -v "$command" >/dev/null 2>&1 || {
    printf 'required command not found: %s\n' "$command" >&2
    exit 2
  }
done

test_db="quaestor_membership_audit_${RANDOM}_$$"
update_log="$(mktemp)"
delete_log="$(mktemp)"
payload_log="$(mktemp)"
owner_log="$(mktemp)"
scopes_log="$(mktemp)"

cleanup() {
  dropdb --if-exists "$test_db" >/dev/null 2>&1 || true
  rm -f "$update_log" "$delete_log" "$payload_log" "$owner_log" "$scopes_log"
}
trap cleanup EXIT

createdb "$test_db"
export PGDATABASE="$test_db"

# Insert the fixture tenant before installing the deferred new-tenant-owner
# trigger. This keeps the test isolated from the much larger billing schema.
psql -v ON_ERROR_STOP=1 <<'SQL'
create table tenants (
  id uuid primary key,
  slug text not null,
  display_name text not null
);

insert into tenants (id, slug, display_name) values (
  '33333333-3333-4333-8333-333333333333',
  'tenant-audit',
  'Tenant Audit'
);
SQL

psql -v ON_ERROR_STOP=1 --file=schema/fragments/020_tenant_memberships.sql

psql -v ON_ERROR_STOP=1 <<'SQL'
insert into tenant_memberships (
  tenant_id, shared_user_id, role, scopes, granted_by_shared_user_id
) values (
  '33333333-3333-4333-8333-333333333333',
  'audit-owner',
  'owner',
  array['billing:read', 'billing:write', 'billing:admin']::text[],
  'audit-owner'
);

insert into tenant_membership_events (
  tenant_id,
  shared_user_id,
  actor_shared_user_id,
  event_type,
  role,
  scopes
) values (
  '33333333-3333-4333-8333-333333333333',
  'audit-reader',
  'audit-owner',
  'grant_or_update',
  'reader',
  array['billing:read']::text[]
);
SQL

event_id="$(psql -Atqc "select id from tenant_membership_events order by id limit 1")"
[[ -n "$event_id" ]] || {
  echo 'expected a membership audit event fixture' >&2
  exit 1
}

set +e
psql -v ON_ERROR_STOP=1 >"$update_log" 2>&1 <<SQL
update tenant_membership_events
set role = 'billing', scopes = array['billing:read', 'billing:write']::text[]
where id = $event_id;
SQL
update_status=$?
set -e
[[ "$update_status" -ne 0 ]] || {
  echo 'membership audit event update unexpectedly succeeded' >&2
  exit 1
}
grep -F 'tenant membership audit events are append-only' "$update_log" >/dev/null

set +e
psql -v ON_ERROR_STOP=1 >"$delete_log" 2>&1 <<SQL
delete from tenant_membership_events where id = $event_id;
SQL
delete_status=$?
set -e
[[ "$delete_status" -ne 0 ]] || {
  echo 'membership audit event delete unexpectedly succeeded' >&2
  exit 1
}
grep -F 'tenant membership audit events are append-only' "$delete_log" >/dev/null

remaining_events="$(psql -Atqc 'select count(*) from tenant_membership_events')"
[[ "$remaining_events" == "1" ]] || {
  printf 'expected one immutable audit event, found %s\n' "$remaining_events" >&2
  exit 1
}

# A revoke event cannot retain a role or scopes.
set +e
psql -v ON_ERROR_STOP=1 >"$payload_log" 2>&1 <<'SQL'
insert into tenant_membership_events (
  tenant_id, shared_user_id, actor_shared_user_id, event_type, role, scopes
) values (
  '33333333-3333-4333-8333-333333333333',
  'audit-reader',
  'audit-owner',
  'revoke',
  'reader',
  array['billing:read']::text[]
);
SQL
payload_status=$?
set -e
[[ "$payload_status" -ne 0 ]]
grep -F 'tenant_membership_events_payload' "$payload_log" >/dev/null

# The first-owner event is self-acting and cannot attribute creation to another
# principal.
set +e
psql -v ON_ERROR_STOP=1 >"$owner_log" 2>&1 <<'SQL'
insert into tenant_membership_events (
  tenant_id, shared_user_id, actor_shared_user_id, event_type, role, scopes
) values (
  '33333333-3333-4333-8333-333333333333',
  'audit-owner',
  'different-actor',
  'create_owner',
  'owner',
  array['billing:read', 'billing:write', 'billing:admin']::text[]
);
SQL
owner_status=$?
set -e
[[ "$owner_status" -ne 0 ]]
grep -F 'tenant_membership_events_payload' "$owner_log" >/dev/null

# Audit payloads reject duplicate scopes just like live grants.
set +e
psql -v ON_ERROR_STOP=1 >"$scopes_log" 2>&1 <<'SQL'
insert into tenant_membership_events (
  tenant_id, shared_user_id, actor_shared_user_id, event_type, role, scopes
) values (
  '33333333-3333-4333-8333-333333333333',
  'audit-reader-2',
  'audit-owner',
  'grant_or_update',
  'reader',
  array['billing:read', 'billing:read']::text[]
);
SQL
scopes_status=$?
set -e
[[ "$scopes_status" -ne 0 ]]
grep -F 'tenant_membership_events_scopes' "$scopes_log" >/dev/null

printf 'tenant membership audit immutability and payload invariants passed\n'
