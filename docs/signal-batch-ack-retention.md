# Signal Protocol batch acknowledgement and retention

Tracking: DEN-150, DEN-280

## Batch acknowledgement

`POST /v1/signal/mailbox/ack` accepts the shared-interface wire shape:

```json
{
  "items": [
    { "envelope_id": "00000000-0000-0000-0000-000000000000" }
  ]
}
```

The authenticated recipient may acknowledge 1–250 envelope IDs per request. The backend deduplicates identifiers, verifies the recipient is active and non-revoked, and updates the batch in one database transaction. Unknown IDs, IDs owned by another recipient/account, and already acknowledged rows do not increase the returned `acknowledged` count.

Acknowledgement means the client already decrypted the ciphertext, validated the application payload, and atomically persisted the local mutation. Download or decryption attempt alone must never trigger acknowledgement.

The original single-envelope compatibility route remains available while generated clients migrate to the shared batch contract.

## Retention cleanup

The store exposes deterministic cleanup for:

- expired opaque mailbox ciphertext;
- acknowledged ciphertext older than the bounded retry/idempotency window;
- expired public prekey bundles;
- claimed public one-time prekeys older than their audit/retry window.

Cleanup never receives or deletes Signal private keys, Double Ratchet state, vault keys, OTP seeds, PINs, biometric material, recovery keys, or plaintext mutations. Those remain device-local.

Production scheduling remains disabled until the canonical pg-defs schema, operator runbook, metrics, and rollback policy are reviewed. Cleanup must be idempotent and may be invoked by a bounded authenticated administrative job, never by public clients.

## PostgreSQL evidence

The dedicated CI job runs the ignored transaction suite serially against an isolated PostgreSQL 17 service database. Each test recreates the minimal service-owned schema it needs so timing and lock behavior are deterministic and do not depend on developer-local state.

The concurrent-claim test shares one SeaORM connection pool through `Arc<DatabaseConnection>` and starts two tasks behind a three-party barrier. The pool remains responsible for independent database connections; the test does not clone or serialize one connection object. This exercises the production `FOR UPDATE ... SKIP LOCKED` behavior under genuinely overlapping requests.

The suite proves:

- concurrent sibling requests claim different one-time public prekeys;
- claimed rows identify two distinct requesters;
- batch acknowledgement is atomic and idempotent;
- duplicate and unknown IDs do not inflate acknowledgement counts;
- retention cleanup removes only rows that meet explicit expiry/age conditions.
