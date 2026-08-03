#!/usr/bin/env bash
set -euo pipefail

for command in createdb dropdb psql; do
  command -v "$command" >/dev/null 2>&1 || {
    printf 'required command not found: %s\n' "$command" >&2
    exit 2
  }
done

test_db="quaestor_memberships_${RANDOM}_$$"
first_log="$(mktemp)"
second_log="$(mktemp)"
delete_log="$(mktemp)"
demote_log="$(mktemp)"
key_log="$(mktemp)"
canonical_log="$(mktemp)"
scopes_log="$(mktemp)"

cleanup() {
  dropdb --if-exists "$test_db" >/dev/null 2>&1 || true
  rm -f \
    "$first_log" \
    "$second_log" \
    "$delete_log" \
    "$demote_log" \
    "$key_log" \
    "$canonical_log" \
    "$scopes_log"
}
trap cleanup EXIT

createdb "$test_db"
export PGDATABASE="$test_db"

# The membership fragment only depends on the tenant primary key. Insert
# fixtures before installing the new-tenant trigger so this isolated test can
# exercise the fragment without importing the full billing schema.
psql -v ON_ERROR_STOP=1 <<'SQL'
create table tenants (
  id uuid primary key,
  slug text not null,
  display_name text not null
);

insert into tenants (id, slug, display_name) values
  ('11111111-1111-4111-8111-111111111111', 'tenant-a', 'Tenant A'),
  ('22222222-2222-4222-8222-222222222222', 'tenant-b', 'Tenant B');
SQL

psql -v ON_ERROR_STOP=1 --file=schema/fragments/020_tenant_memberships.sql

psql -v ON_ERROR_STOP=1 <<'SQL'
insert into tenant_memberships (
  tenant_id, shared_user_id, role, scopes, granted_by_shared_user_id
) values
  (
    '11111111-1111-4111-8111-111111111111',
    'owner-a',
    'owner',
    array['billing:read', 'billing:write', 'billing:admin']::text[],
    'owner-a'
  ),
  (
    '11111111-1111-4111-8111-111111111111',
    'owner-b',
    'owner',
    array['billing:read', 'billing:write', 'billing:admin']::text[],
    'owner-a'
  ),
  (
    '22222222-2222-4222-8222-222222222222',
    'owner-c',
    'owner',
    array['billing:read', 'billing:write', 'billing:admin']::text[],
    'owner-c'
  ),
  (
    '22222222-2222-4222-8222-222222222222',
    'owner-d',
    'owner',
    array['billing:read', 'billing:write', 'billing:admin']::text[],
    'owner-c'
  );
SQL

# Transaction one holds the tenant row while demoting owner A. Transaction two
# must wait, then fail its deferred constraint after owner B would become the
# final demotion. Without serialization both commits can succeed.
psql -v ON_ERROR_STOP=1 >"$first_log" 2>&1 <<'SQL' &
begin;
update tenant_memberships
set role = 'admin',
    scopes = array['billing:read', 'billing:write', 'billing:admin']::text[],
    granted_by_shared_user_id = 'owner-a',
    updated_at = now()
where tenant_id = '11111111-1111-4111-8111-111111111111'
  and shared_user_id = 'owner-a';
select pg_sleep(2);
commit;
SQL
first_pid=$!

sleep 0.25
set +e
psql -v ON_ERROR_STOP=1 >"$second_log" 2>&1 <<'SQL'
begin;
update tenant_memberships
set role = 'admin',
    scopes = array['billing:read', 'billing:write', 'billing:admin']::text[],
    granted_by_shared_user_id = 'owner-b',
    updated_at = now()
where tenant_id = '11111111-1111-4111-8111-111111111111'
  and shared_user_id = 'owner-b';
commit;
SQL
second_status=$?
set -e
wait "$first_pid"

if [[ "$second_status" -eq 0 ]]; then
  cat "$first_log" "$second_log" >&2
  echo 'concurrent final-owner demotions both committed' >&2
  exit 1
fi
grep -F 'must retain at least one active owner' "$second_log" >/dev/null

remaining_owners="$(psql -Atqc "
  select count(*)
  from tenant_memberships
  where tenant_id = '11111111-1111-4111-8111-111111111111'
    and role = 'owner'
    and revoked_at is null
")"
[[ "$remaining_owners" == "1" ]] || {
  printf 'expected one active owner, found %s\n' "$remaining_owners" >&2
  exit 1
}

# Exercise the trigger's DELETE branch as well. Deleting owner C may commit
# because owner D still exists. A concurrent transaction that then demotes owner
# D must wait on the same tenant row and fail rather than leaving tenant B
# ownerless.
psql -v ON_ERROR_STOP=1 >"$delete_log" 2>&1 <<'SQL' &
begin;
delete from tenant_memberships
where tenant_id = '22222222-2222-4222-8222-222222222222'
  and shared_user_id = 'owner-c';
select pg_sleep(2);
commit;
SQL
delete_pid=$!

sleep 0.25
set +e
psql -v ON_ERROR_STOP=1 >"$demote_log" 2>&1 <<'SQL'
begin;
update tenant_memberships
set role = 'admin',
    scopes = array['billing:read', 'billing:write', 'billing:admin']::text[],
    granted_by_shared_user_id = 'owner-d',
    updated_at = now()
where tenant_id = '22222222-2222-4222-8222-222222222222'
  and shared_user_id = 'owner-d';
commit;
SQL
demote_status=$?
set -e
wait "$delete_pid"

if [[ "$demote_status" -eq 0 ]]; then
  cat "$delete_log" "$demote_log" >&2
  echo 'concurrent owner deletion and final-owner demotion both committed' >&2
  exit 1
fi
grep -F 'must retain at least one active owner' "$demote_log" >/dev/null

remaining_mixed_owners="$(psql -Atqc "
  select count(*)
  from tenant_memberships
  where tenant_id = '22222222-2222-4222-8222-222222222222'
    and role = 'owner'
    and revoked_at is null
")"
[[ "$remaining_mixed_owners" == "1" ]] || {
  printf 'expected one active owner after mixed mutation race, found %s\n' \
    "$remaining_mixed_owners" >&2
  exit 1
}

# Identity keys are immutable even for direct SQL clients.
set +e
psql -v ON_ERROR_STOP=1 >"$key_log" 2>&1 <<'SQL'
update tenant_memberships
set tenant_id = '22222222-2222-4222-8222-222222222222'
where tenant_id = '11111111-1111-4111-8111-111111111111'
  and shared_user_id = 'owner-b';
SQL
key_status=$?
set -e
[[ "$key_status" -ne 0 ]]
grep -F 'tenant membership identity is immutable' "$key_log" >/dev/null

# Canonical subjects and duplicate scopes are rejected at the database boundary.
set +e
psql -v ON_ERROR_STOP=1 >"$canonical_log" 2>&1 <<'SQL'
insert into tenant_memberships (
  tenant_id, shared_user_id, role, scopes, granted_by_shared_user_id
) values (
  '11111111-1111-4111-8111-111111111111',
  ' padded-subject ',
  'reader',
  array['billing:read']::text[],
  'owner-b'
);
SQL
canonical_status=$?
set -e
[[ "$canonical_status" -ne 0 ]]
grep -F 'tenant_memberships_subject_canonical' "$canonical_log" >/dev/null

set +e
psql -v ON_ERROR_STOP=1 >"$scopes_log" 2>&1 <<'SQL'
insert into tenant_memberships (
  tenant_id, shared_user_id, role, scopes, granted_by_shared_user_id
) values (
  '11111111-1111-4111-8111-111111111111',
  'duplicate-scope-user',
  'billing',
  array['billing:read', 'billing:read']::text[],
  'owner-b'
);
SQL
scopes_status=$?
set -e
[[ "$scopes_status" -ne 0 ]]
grep -F 'tenant_memberships_scopes' "$scopes_log" >/dev/null

printf 'tenant membership concurrency and integrity invariants passed\n'