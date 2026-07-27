# Signal sync HTTP rollout

The Signal sync HTTP surface is fail-closed. `app::router` omits every `/v1/signal/*` route. Startup must explicitly set `ENABLE_SIGNAL_SYNC_API` to `1`, `true`, `yes`, or `on` to compose the guarded routes through `router_with_signal`; missing, blank, false, zero, or unknown values leave the surface absent and return the ordinary 404 fallback.

## Guarded routes

- `PUT /v1/signal/prekeys` publishes public identity, signed, post-quantum signed, and bounded one-time prekey material for the authenticated device.
- `POST /v1/signal/envelopes` enqueues recipient-specific opaque ciphertext. The account and sender IDs must match the authenticated device.
- `GET /v1/signal/mailbox` pulls a bounded batch after a monotonic server cursor for the authenticated recipient device.
- `POST /v1/signal/mailbox/{envelope_id}/ack` acknowledges an envelope only after client-side decrypt, validation, and atomic local apply.

Authentication is extracted from the request head before JSON bodies are read. The handlers cannot represent private Signal keys, ratchet state, vault plaintext, PIN values, OTP codes, biometric material, recovery keys, or plaintext recovery destinations.

## Production blockers

Keep `ENABLE_SIGNAL_SYNC_API` disabled until all of the following are complete:

1. PostgreSQL concurrency tests for one-time prekey claims, envelope idempotency, mailbox cursors, acknowledgement replay, cross-account rejection, and revocation races.
2. Authenticated public bundle retrieval and atomic one-time prekey claim transport.
3. Signed, revisioned device lifecycle commands and immediate fan-out/revocation behavior.
4. Mailbox and prekey expiry cleanup with capacity alarms.
5. Adversarial multi-device end-to-end tests and log/trace/metric redaction assertions.
6. Staged canary rollout with rollback procedures and no compatibility fallback to plaintext or server-side decryption.
