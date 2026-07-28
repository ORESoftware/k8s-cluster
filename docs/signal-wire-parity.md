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

### Pull a bounded mailbox page

`GET /v1/signal/mailbox` now returns the shared interface shape:

```json
{
  "items": [],
  "next_cursor": 0
}
```

`items` remain ordered by `mailbox_seq`. `next_cursor` is the final returned cursor, or the caller's existing cursor when no rows are returned, so it never moves backwards.

### Batch acknowledgement

`POST /v1/signal/mailbox/ack` already matches the shared bounded `{items:[{envelope_id}]}` request and `{acknowledged}` response. Acknowledgement remains recipient-scoped and means decrypt, payload validation, and atomic local persistence already succeeded.

## Remaining blocker: publish-prekey revision semantics

The shared `SignalPublishPreKeysRequest/Response` uses a device-supplied strictly increasing `bundle_revision` and reports the current unclaimed prekey count. The current backend store allocates bundle revisions server-side and reports inserted prekey count instead.

That difference is not papered over in an HTTP adapter. A follow-up DEN-536 store/schema PR must define idempotent same-revision retries, reject stale or conflicting revisions, preserve atomic replenishment, and return the shared response fields before publish-prekey client generation is enabled.
