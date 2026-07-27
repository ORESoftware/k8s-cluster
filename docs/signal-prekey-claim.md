# Signal Protocol prekey claim and device revision

Tracking: DEN-150, DEN-280

This slice adds the two authenticated read/claim operations deliberately excluded from the first guarded HTTP adapter PR. They remain absent from the public router unless `ENABLE_SIGNAL_SYNC_API` is explicitly enabled.

## Routes

### `POST /v1/signal/devices/{device_id}/prekey-bundle`

Returns the target active sibling device's current public identity key, signed prekey, post-quantum signed prekey, and at most one public one-time prekey.

The PostgreSQL statement:

- proves the requester and target are distinct, active, non-revoked devices on the same account;
- refuses an expired public bundle;
- locks one unclaimed prekey with `FOR UPDATE ... SKIP LOCKED`;
- records the authenticated requester as the claimant in the same statement;
- never returns or stores any corresponding private key;
- reports the current account device-list revision;
- reports `low_prekeys` when the remaining public pool is below the replenishment threshold.

A claimed one-time public prekey is never returned to a second requester. Retrying the HTTP request therefore claims a different available prekey or returns the signed bundle without one-time material when the pool is exhausted; callers must treat the returned bundle as fresh session-setup material rather than expecting HTTP replay identity.

A missing or expired bundle, self-target, cross-account target, suspended device, or revoked device fails closed without revealing whether another account owns the identifier.

### `GET /v1/signal/device-revision`

Returns the current monotonic account device-list revision only for an active, non-revoked authenticated device. Clients use this value to detect stale fan-out before encrypting recipient-specific envelopes.

## Security boundary

The service handles public prekeys and opaque ciphertext only. Signal private keys, Double Ratchet state, vault keys, OTP seeds, PINs, biometric templates, recovery keys, and plaintext mutations remain device-local.

The route flag still defaults off. Production enablement remains blocked on PostgreSQL concurrency tests, signed revisioned revocation, generated HTTP clients, Flutter provider integration, adversarial E2E tests, telemetry-redaction evidence, and the production pg-defs rollout.
