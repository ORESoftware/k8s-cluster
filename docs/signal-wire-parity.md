# Signal HTTP wire parity

Tracking: DEN-150, DEN-280, DEN-286, DEN-536

All Signal routes remain absent unless the disabled-by-default `ENABLE_SIGNAL_SYNC_API` gate is explicitly enabled.

## Aligned routes

### Publish public prekeys

`PUT /v1/signal/prekeys` accepts the shared flattened `SignalPublishPreKeysRequest` and returns `SignalPublishPreKeysResponse`:

```json
{
  "version": 1,
  "bundle_revision": 7,
  "registration_id": 42,
  "identity_key": [1],
  "signed_pre_key_id": 10,
  "signed_pre_key": [2],
  "signed_pre_key_signature": [3],
  "pq_signed_pre_key_id": 11,
  "pq_signed_pre_key": [4],
  "pq_signed_pre_key_signature": [5],
  "one_time_prekeys": [],
  "expires_at_ms": 4102444800000
}
```

```json
{
  "bundle_revision": 7,
  "device_revision": 12,
  "unclaimed_prekey_count": 24
}
```

The authenticated device supplies a positive, strictly increasing device-local `bundle_revision`. Publication locks the active device and current bundle in one PostgreSQL transaction:

* a lower revision fails with a revision conflict;
* the same revision succeeds only when the complete signed bundle, expiry, and every supplied one-time prekey are byte-identical to already stored public material;
* an exact same-revision retry is read-only and does not bump the account device revision or emit a duplicate audit event;
* a higher revision atomically replaces the signed bundle, inserts only fresh one-time-prekey identifiers, bumps the account device revision once, records a bounded security event, and returns the current unclaimed-key count;
* any duplicate prekey identifier in the request or any reuse across revisions fails closed and rolls back the whole transaction.

The HTTP body cannot supply a device identifier. The backend derives it from the authenticated request head, so a caller cannot publish public material for a sibling device.

### Queue one recipient envelope

`POST /v1/signal/envelopes` accepts the shared interface shape:

```json
{
  "envelope": {
    "metadata": { "...": "versioned routing metadata" },
    "ciphertext": [1, 2, 3]
  }
}
```

The response uses one unambiguous duplicate flag:

```json
{
  "mailbox_seq": 7,
  "duplicate": false
}
```

`duplicate` is the inverse of the transactional store's internal `inserted` result. A duplicate is returned only when the existing row has the exact same immutable routing metadata and ciphertext; conflicting reuse of an envelope ID still fails closed.

When two exact retries race, PostgreSQL can observe the conflicting unique key before the winning row is visible to the loser's initial `READ COMMITTED` snapshot. The store therefore performs its exact-match check in a fresh statement after `ON CONFLICT DO NOTHING`; this preserves idempotency without accepting a changed envelope.

### Pull a bounded mailbox page

`GET /v1/signal/mailbox` returns the shared interface shape:

```json
{
  "items": [],
  "next_cursor": 0
}
```

`items` remain ordered by `mailbox_seq`. `next_cursor` is the final returned cursor, or the caller's existing cursor when no rows are returned, so it never moves backwards.

Pulls wait for an earlier locked mailbox row instead of skipping to a later cursor. Concurrent pulls may repeat an unacknowledged envelope, but cannot advance a client past unseen ciphertext.

### Batch acknowledgement

`POST /v1/signal/mailbox/ack` matches the shared bounded `{items:[{envelope_id}]}` request and `{acknowledged}` response. Acknowledgement remains recipient-scoped and means decrypt, payload validation, and atomic local persistence already succeeded.

### Publish or replenish prekeys

`PUT /v1/signal/prekeys` now accepts the shared flattened `SignalPublishPreKeysRequest` and returns `bundle_revision`, `device_revision`, and the current `unclaimed_prekey_count`.

The device supplies a positive `bundle_revision`. A lower revision is rejected, a same-revision retry must match the stored bundle and any reused prekey IDs exactly, and a higher revision rotates the bundle. A same-revision request may add previously unseen one-time prekeys without rewriting the bundle. Exact retries do not increment `device_revision`; effective bundle rotations or pool replenishments do.

The response counts the current unclaimed pool after the transaction, rather than only the rows inserted by this request. Bundle comparison, one-time prekey conflict checks, revision updates, security events, and the pool count all share one transaction.

## Canonical fixture provenance

`fixtures/signal-http-wire.json` is a byte-identical snapshot of the canonical fixture merged in `3FA-app/3fa-interfaces` at `f8114237994112647453321b4e1bc4287b0bf3c9` (SHA-256 `8c6a2a72b52cb6d5c9e3ff32b4348d426dec81c32746d83540af67e497559ef3`).

Backend tests hash that snapshot and round-trip publish, queue, pull, empty-pull, and acknowledgement values through the actual HTTP DTOs, and drive the duplicate/status behavior and monotonic-cursor mapping with those exact values. Ignored real-PostgreSQL coverage exercises first publication, exact retry, stale revision, same-revision conflict, higher-revision replenishment, reused prekey-ID rollback, account revision monotonicity, and current unclaimed-key counts.

## Remaining rollout gates

Wire parity does not enable Signal sync. Production remains blocked on the disabled-by-default rollout gate, native provider/legal review, least-privilege E2E repository access, real service-container E2E, telemetry review, rollback drills, and restoration of GitHub-hosted runner allocation under DEN-539.
