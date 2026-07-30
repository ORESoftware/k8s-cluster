# Transactional Signal Protocol store

The `signal_store` module is the persistence boundary for public Signal Protocol prekeys and recipient-specific opaque ciphertext. It does not implement cryptographic primitives and cannot represent private identity keys, ratchet state, vault plaintext, account master keys, PINs, OTP codes, biometric templates, or recovery secrets.

## Transactional invariants

- Only active, non-revoked devices can publish or claim public prekeys, enqueue or pull mailbox envelopes, acknowledge delivery, or revoke another device.
- One-time prekeys are selected with `FOR UPDATE ... SKIP LOCKED` and marked claimed in the same PostgreSQL statement.
- An envelope ID is an immutable idempotency key. Reusing it with different routing metadata or ciphertext fails closed.
- Pulling marks an envelope delivered but never acknowledged. Acknowledgement happens only after the recipient decrypts, validates, and atomically applies the payload locally.
- Device revocation uses an expected device-list revision, updates the legacy token state and terminal lifecycle state together, removes public prekeys and pending mailbox access, and records a security event.
- Mailbox cursors are server-issued, ascending, and bounded per pull.

## Rollout boundary

This first slice intentionally exposes no new HTTP routes. Authenticated Axum handlers, typed errors, PostgreSQL concurrency integration tests, cleanup jobs, and the disabled-by-default rollout flag are the next DEN-280 slice. Production enablement remains blocked on adversarial multi-device end-to-end tests and schema/deployment review.
