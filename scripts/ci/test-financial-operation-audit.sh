#!/usr/bin/env bash
set -euo pipefail

for command in createdb dropdb psql; do
  command -v "$command" >/dev/null 2>&1 || {
    printf 'required command not found: %s\n' "$command" >&2
    exit 2
  }
done

test_db="quaestor_financial_audit_${RANDOM}_$$"
update_log="$(mktemp)"
delete_log="$(mktemp)"
actor_log="$(mktemp)"
assurance_log="$(mktemp)"
tenant_log="$(mktemp)"
duplicate_log="$(mktemp)"

cleanup() {
  dropdb --if-exists "$test_db" >/dev/null 2>&1 || true
  rm -f \
    "$update_log" \
    "$delete_log" \
    "$actor_log" \
    "$assurance_log" \
    "$tenant_log" \
    "$duplicate_log"
}
trap cleanup EXIT

createdb "$test_db"
export PGDATABASE="$test_db"

psql -v ON_ERROR_STOP=1 <<'SQL'
create table tenants (
  id uuid primary key,
  slug text not null,
  display_name text not null
);

create table transactions (
  id uuid primary key,
  tenant_id uuid not null references tenants(id) on delete restrict,
  idempotency_key text not null,
  unique (tenant_id, idempotency_key)
);

insert into tenants (id, slug, display_name) values
  ('11111111-1111-4111-8111-111111111111', 'tenant-a', 'Tenant A'),
  ('22222222-2222-4222-8222-222222222222', 'tenant-b', 'Tenant B');
SQL

psql -v ON_ERROR_STOP=1 --file=schema/fragments/040_financial_operation_events.sql

# Accepted transaction and event commit together.
psql -v ON_ERROR_STOP=1 <<'SQL'
begin;
insert into transactions (id, tenant_id, idempotency_key) values (
  'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
  '11111111-1111-4111-8111-111111111111',
  'key-a'
);
insert into financial_operation_events (
  id,
  tenant_id,
  operation,
  outcome,
  actor_kind,
  shared_user_id,
  shared_session_id,
  request_correlation_id,
  authorization_scope,
  aal,
  acr,
  auth_time_unix,
  idempotency_key_fingerprint,
  resource_type,
  resource_id,
  ledger_transaction_id
) values (
  'eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee',
  '11111111-1111-4111-8111-111111111111',
  'ledger.post_transaction',
  'accepted',
  'shared_auth_user',
  'shared-user-1',
  'session-1',
  'cccccccc-cccc-4ccc-8ccc-cccccccccccc',
  'billing:write',
  2,
  'urn:oresoftware:loa:2',
  1700000000,
  'sha256:v1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
  'ledger_transaction',
  'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
  'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
);
commit;
SQL

[[ "$(psql -Atqc "select count(*) from transactions")" == "1" ]]
[[ "$(psql -Atqc "select count(*) from financial_operation_events")" == "1" ]]

# A rolled-back mutation leaves neither the transaction nor its audit row.
psql -v ON_ERROR_STOP=1 <<'SQL'
begin;
insert into transactions (id, tenant_id, idempotency_key) values (
  'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
  '11111111-1111-4111-8111-111111111111',
  'key-b'
);
insert into financial_operation_events (
  id,
  tenant_id,
  operation,
  outcome,
  actor_kind,
  shared_user_id,
  shared_session_id,
  request_correlation_id,
  authorization_scope,
  aal,
  acr,
  auth_time_unix,
  idempotency_key_fingerprint,
  resource_type,
  resource_id,
  ledger_transaction_id
) values (
  'ffffffff-ffff-4fff-8fff-ffffffffffff',
  '11111111-1111-4111-8111-111111111111',
  'ledger.post_transaction',
  'accepted',
  'shared_auth_user',
  'shared-user-2',
  'session-2',
  'dddddddd-dddd-4ddd-8ddd-dddddddddddd',
  'billing:write',
  1,
  'urn:oresoftware:loa:1',
  null,
  'sha256:v1:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
  'ledger_transaction',
  'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
  'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'
);
rollback;
SQL

[[ "$(psql -Atqc "select count(*) from transactions where id = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'")" == "0" ]]
[[ "$(psql -Atqc "select count(*) from financial_operation_events where id = 'ffffffff-ffff-4fff-8fff-ffffffffffff'")" == "0" ]]

set +e
psql -v ON_ERROR_STOP=1 >"$update_log" 2>&1 <<'SQL'
update financial_operation_events
set authorization_scope = 'legacy:service'
where id = 'eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee';
SQL
update_status=$?
set -e
[[ "$update_status" -ne 0 ]]
grep -F 'financial operation events are append-only' "$update_log" >/dev/null

set +e
psql -v ON_ERROR_STOP=1 >"$delete_log" 2>&1 <<'SQL'
delete from financial_operation_events
where id = 'eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee';
SQL
delete_status=$?
set -e
[[ "$delete_status" -ne 0 ]]
grep -F 'financial operation events are append-only' "$delete_log" >/dev/null

# Shared Auth actors require both canonical subject and live session identity.
set +e
psql -v ON_ERROR_STOP=1 >"$actor_log" 2>&1 <<'SQL'
insert into financial_operation_events (
  id, tenant_id, operation, outcome, actor_kind,
  shared_user_id, shared_session_id, request_correlation_id,
  authorization_scope, aal, acr, auth_time_unix,
  idempotency_key_fingerprint, resource_type, resource_id,
  ledger_transaction_id
) values (
  gen_random_uuid(),
  '11111111-1111-4111-8111-111111111111',
  'ledger.post_transaction',
  'accepted',
  'shared_auth_user',
  ' shared-user-1',
  null,
  gen_random_uuid(),
  'billing:write',
  2,
  'urn:oresoftware:loa:2',
  1700000000,
  'sha256:v1:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
  'ledger_transaction',
  'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
  'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
);
SQL
actor_status=$?
set -e
[[ "$actor_status" -ne 0 ]]
grep -F 'financial_operation_events_actor' "$actor_log" >/dev/null

# AAL2 cannot exist without the LOA2 ACR and authoritative ceremony time.
set +e
psql -v ON_ERROR_STOP=1 >"$assurance_log" 2>&1 <<'SQL'
insert into financial_operation_events (
  id, tenant_id, operation, outcome, actor_kind,
  shared_user_id, shared_session_id, request_correlation_id,
  authorization_scope, aal, acr, auth_time_unix,
  idempotency_key_fingerprint, resource_type, resource_id,
  ledger_transaction_id
) values (
  gen_random_uuid(),
  '11111111-1111-4111-8111-111111111111',
  'ledger.post_transaction',
  'accepted',
  'shared_auth_user',
  'shared-user-1',
  'session-1',
  gen_random_uuid(),
  'billing:write',
  2,
  'urn:oresoftware:loa:2',
  null,
  'sha256:v1:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd',
  'ledger_transaction',
  'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
  'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
);
SQL
assurance_status=$?
set -e
[[ "$assurance_status" -ne 0 ]]
grep -F 'financial_operation_events_assurance' "$assurance_log" >/dev/null

# An event cannot claim a ledger transaction owned by another tenant.
set +e
psql -v ON_ERROR_STOP=1 >"$tenant_log" 2>&1 <<'SQL'
insert into financial_operation_events (
  id, tenant_id, operation, outcome, actor_kind,
  shared_user_id, shared_session_id, request_correlation_id,
  authorization_scope, aal, acr, auth_time_unix,
  idempotency_key_fingerprint, resource_type, resource_id,
  ledger_transaction_id
) values (
  gen_random_uuid(),
  '22222222-2222-4222-8222-222222222222',
  'ledger.post_transaction',
  'accepted',
  'legacy_service',
  null,
  null,
  gen_random_uuid(),
  'legacy:service',
  0,
  null,
  null,
  'sha256:v1:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
  'ledger_transaction',
  'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
  'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
);
SQL
tenant_status=$?
set -e
[[ "$tenant_status" -ne 0 ]]
grep -F 'financial operation event tenant does not match ledger transaction' "$tenant_log" >/dev/null

# Replay identity is unique for both the transaction resource and the
# fingerprinted idempotency intent.
set +e
psql -v ON_ERROR_STOP=1 >"$duplicate_log" 2>&1 <<'SQL'
insert into financial_operation_events (
  id, tenant_id, operation, outcome, actor_kind,
  shared_user_id, shared_session_id, request_correlation_id,
  authorization_scope, aal, acr, auth_time_unix,
  idempotency_key_fingerprint, resource_type, resource_id,
  ledger_transaction_id
) values (
  gen_random_uuid(),
  '11111111-1111-4111-8111-111111111111',
  'ledger.post_transaction',
  'accepted',
  'shared_auth_user',
  'shared-user-1',
  'session-1',
  gen_random_uuid(),
  'billing:write',
  2,
  'urn:oresoftware:loa:2',
  1700000000,
  'sha256:v1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
  'ledger_transaction',
  'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
  'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
);
SQL
duplicate_status=$?
set -e
[[ "$duplicate_status" -ne 0 ]]
grep -E 'financial_operation_events_tenant_id_operation_(ledger_transaction_id|idempotency_key_fingerprint)_key' "$duplicate_log" >/dev/null

printf 'financial operation audit atomicity and append-only invariants passed\n'
