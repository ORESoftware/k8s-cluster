# Signal HTTP wire parity

Tracking: DEN-150, DEN-280, DEN-286, DEN-536

All Signal routes remain absent unless the disabled-by-default `ENABLE_SIGNAL_SYNC_API` gate is explicitly enabled.

## Aligned in this slice

### Queue one recipient envelope

`POST /v1/signal/envelopes` now accepts the shared interface shape:

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

`GET /v1/signal/mailbox` now returns the shared interface shape:

```json
{
  "items": [],
  "next_cursor": 0
}
```

`items` remain ordered by `mailbox_seq`. `next_cursor` is the final returned cursor, or the caller's existing cursor when no rows are returned, so it never moves backwards.

Pulls wait for an earlier locked mailbox row instead of skipping to a later cursor. Concurrent pulls may repeat an unacknowledged envelope, but cannot advance a client past unseen ciphertext.

### Batch acknowledgement

`POST /v1/signal/mailbox/ack` already matches the shared bounded `{items:[{envelope_id}]}` request and `{acknowledged}` response. Acknowledgement remains recipient-scoped and means decrypt, payload validation, and atomic local persistence already succeeded.

## Canonical fixture provenance

`fixtures/signal-http-wire.json` is a byte-identical snapshot of the canonical fixture merged in `3FA-app/3fa-interfaces` at `f8114237994112647453321b4e1bc4287b0bf3c9` (SHA-256 `8c6a2a72b52cb6d5c9e3ff32b4348d426dec81c32746d83540af67e497559ef3`).

The backend test hashes that snapshot, round-trips its queue, pull, empty-pull, and acknowledgement values through the actual HTTP DTOs, and drives the duplicate/status and monotonic-cursor mapping with those exact values. The publish request is required to remain rejected until the store semantics below are reconciled.

## Remaining blocker: publish-prekey revision semantics

The shared `SignalPublishPreKeysRequest/Response` uses a device-supplied strictly increasing `bundle_revision` and reports the current unclaimed prekey count. The current backend store allocates bundle revisions server-side and reports inserted prekey count instead.

That difference is not papered over in an HTTP adapter. A follow-up DEN-536 store/schema PR must define idempotent same-revision retries, reject stale or conflicting revisions, preserve atomic replenishment, and return the shared response fields before publish-prekey client generation is enabled.
