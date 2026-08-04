# Financial operation audit runbook

Quaestor records accepted ledger postings in `financial_operation_events`. The event is inserted in the same PostgreSQL transaction as the ledger transaction header and postings. If actor attribution fails, the financial mutation must roll back.

## Stored evidence

The table stores only bounded identifiers and authorization evidence:

- tenant and ledger transaction IDs;
- Shared Auth subject and active session ID, or explicit `legacy_service` during migration;
- canonical request correlation UUID;
- operation, resource type, accepted outcome, scope, AAL/ACR, and AAL2 ceremony time;
- a domain-separated SHA-256 idempotency fingerprint;
- event timestamp and schema version.

Do not add bearer tokens, refresh tokens, introspection credentials, provider credentials, OTPs, passkey challenges, payment details, raw request bodies, raw idempotency keys, email addresses, or metric labels containing actor/session identifiers.

## Common queries

Find the accepted event for a ledger transaction:

```sql
select id, tenant_id, ledger_transaction_id, actor_kind,
       shared_user_id, shared_session_id, request_correlation_id,
       authorization_scope, aal, acr, auth_time_unix,
       occurred_at, schema_version
from financial_operation_events
where tenant_id = $1
  and operation = 'ledger.post_transaction'
  and ledger_transaction_id = $2;
```

Trace a request correlation ID:

```sql
select id, tenant_id, operation, resource_type, resource_id,
       actor_kind, authorization_scope, outcome, occurred_at
from financial_operation_events
where request_correlation_id = $1
order by occurred_at, id;
```

Review recent tenant activity without exporting request bodies or credentials:

```sql
select id, operation, resource_type, resource_id, actor_kind,
       authorization_scope, aal, occurred_at
from financial_operation_events
where tenant_id = $1
order by occurred_at desc, id desc
limit 200;
```

## Replay semantics

An idempotent replay returns the original ledger transaction and original audit event/correlation identity. It must not append another accepted event.

`legacy_unattributed` means the transaction predates this audit migration. Never attribute that historical transaction to the caller performing the replay.

A uniqueness conflict involving the fingerprint or ledger transaction requires investigation. Do not bypass the constraints or rewrite the event.

## Incident response

1. Preserve the affected tenant, request correlation ID, ledger transaction ID, event ID, and deployment revision.
2. Verify the event and transaction exist together. A transaction without an event is permitted only when the API explicitly returns `legacy_unattributed` for pre-migration data.
3. Check Shared Auth session revocation and ceremony time using the canonical identity service; do not infer identity from email or request headers.
4. Check idempotency replay behavior and confirm only one accepted event exists.
5. Export only the bounded columns required by the incident. Keep raw actor/session identifiers out of metrics and broad log searches.
6. Never update or delete audit rows. Corrections use a later versioned event contract, not mutation of historical evidence.

## Retention and export

Audit retention must be at least as long as the associated financial ledger and applicable contractual/regulatory evidence period. Archival must preserve append-only semantics, tenant/resource linkage, timestamps, schema version, and cryptographic integrity.

Exports must be access-controlled, encrypted, tenant-scoped, and logged. Redact or tokenize actor/session identifiers unless the investigation explicitly requires them.

## Rollout and rollback

Before deployment, apply `schema/fragments/040_financial_operation_events.sql` and run `scripts/ci/test-financial-operation-audit.sh` against PostgreSQL 17.

Rollback of application code must not drop or mutate the table. Older transactions remain valid and newer audited transactions retain their evidence. Any compatibility release must continue returning original replay identity when an event exists.

## Current scope

This implementation covers `ledger.post_transaction`. DEN-1572 remains open until every tenant-scoped financial mutation has an explicit attribution policy and equivalent atomicity/replay tests.