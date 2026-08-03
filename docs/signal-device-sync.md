# Signal Protocol multi-device sync backend

Tracking issue: [DEN-150](https://linear.app/denman/issue/DEN-150/3fa-implement-signal-protocol-encrypted-multi-device-account-sync)

## Security boundary

The backend is an authenticated but cryptographically untrusted relay. It stores:

- account and device identifiers;
- public identity/signed/post-quantum/one-time prekeys;
- device-list revisions and redacted lifecycle events;
- recipient-specific opaque ciphertext and bounded routing metadata;
- delivery and acknowledgement timestamps.

It never stores or receives private key material, Double Ratchet state, vault keys, OTP seeds, decrypted vault records, recovery secrets, or plaintext device-control messages.

RSA is not part of this design. Flutter devices use the **Signal Protocol**. PQXDH/X3DH, Double Ratchet, and Sesame are the relevant key-agreement, ratcheting, and asynchronous multi-device/session-management components.

## Schema

`migrations/0005_signal_device_sync.sql` adds four service-owned tables:

- `device_prekey_bundles`: one current public bundle per device, revisioned and expiring;
- `device_one_time_prekeys`: one-time public prekeys with an atomic, auditable claim state;
- `device_mailbox`: recipient-specific opaque envelopes with a monotonic server cursor;
- `device_security_events`: redacted device lifecycle/key-rotation audit metadata.

It also adds a monotonic `signal_device_revision` to each account and composite foreign keys proving that mailbox sender and recipient both belong to the stated account.

The migration revokes direct `PUBLIC` access. Production remains behind the Rust service. A future Supabase-direct path must expose narrowly scoped authenticated RPCs with equivalent ownership checks and must never grant table-wide access to clients.

## Planned authenticated operations

The route names below are the intended contract; they remain disabled until handlers, generated interfaces, and end-to-end tests land together.

### Publish/rotate a device bundle

`PUT /v1/signal/prekeys`

- requires the caller's service-local sync token;
- verifies `device_id` equals the authenticated device;
- validates structural bounds before decoding/storing base64;
- requires a strictly increasing `bundle_revision`;
- replaces signed and post-quantum signed public prekeys transactionally;
- inserts a bounded batch of one-time prekeys with unique ids;
- never logs key bytes, signatures, bearer tokens, or request bodies.

### Fetch a sibling prekey bundle

`POST /v1/signal/devices/{device_id}/prekey-bundle`

- requires the target and caller to be active devices on the same account;
- locks one unclaimed one-time prekey with `FOR UPDATE SKIP LOCKED`;
- marks that prekey claimed by the caller in the same transaction;
- returns the current identity/signed/PQ public bundle and at most one claimed one-time prekey;
- returns an explicit low-prekey signal when no one-time key is available, without reusing a claimed key.

### Enqueue ciphertext

`POST /v1/signal/envelopes`

- requires the authenticated device to match `sender_device_id`;
- verifies account/device membership and that neither device is revoked;
- validates version, identifiers, lifetime, ciphertext length, and future-clock skew;
- treats `envelope_id` as the idempotency/replay key;
- inserts ciphertext once and returns the existing cursor on an exact duplicate;
- rejects a conflicting reuse of an envelope id;
- returns `410 Gone` when the recipient is no longer active, which tells clients
  to refresh the device list and stop retrying rather than minting envelope ids;
- does not parse or infer plaintext purpose beyond the authenticated metadata enum.

### Pull and acknowledge

`GET /v1/device-mailbox?after=<cursor>&limit=<n>` returns only the authenticated recipient's unexpired envelopes in ascending cursor order. Limits are hard-capped and empty pages cannot claim more data.

`POST /v1/device-mailbox/ack` atomically acknowledges a bounded set of envelope ids owned by the authenticated recipient. Acknowledged and expired rows are deleted after a short retention window so delayed retries remain idempotent without permitting unbounded storage.

## Device enrollment and revocation

A newly authenticated device is initially pending. It publishes public prekeys and must be approved by an existing trusted device through a QR/safety-number ceremony. The trusted device sends a short-lived `vault_key_transfer` envelope; successful acknowledgement activates normal fan-out.

Revocation increments `signal_device_revision`, invalidates the service-local sync token, prevents mailbox pull/push and prekey fetch, excludes the device from future fan-out, and records only a redacted lifecycle event. Existing sibling devices receive a signed device-control envelope and delete sessions after a bounded stale-session period.

An identity-key replacement is never silently accepted. The backend records the revision/change event but cannot authenticate the new key on behalf of users; clients pause delivery until an explicit verification ceremony succeeds.

## Required transaction and storage bounds

- maximum active devices per account: existing `MAX_DEVICES_PER_ACCOUNT` policy;
- bounded one-time-prekey upload batch and total unclaimed pool;
- one atomic claim per one-time prekey;
- maximum ciphertext: 512 KiB;
- maximum envelope lifetime: 30 days;
- bounded pull page, acknowledgement batch, retries, and mailbox rows per recipient;
- deterministic expiry cleanup and lifecycle-event retention;
- no ciphertext, public key bytes, signatures, tokens, account ids, or device ids in metric labels;
- no request/response bodies in tracing spans.

## Validation and rollout gates

Before enabling routes:

1. generate shared Rust/Dart/TypeScript DTOs from `3fa-interfaces`;
2. add SeaORM entities and transaction-focused repository methods;
3. test concurrent one-time-prekey claims against PostgreSQL;
4. test duplicate/replayed/out-of-order/expired envelopes and cursor pagination;
5. test revocation races and cross-account routing attempts;
6. add Supabase RLS/RPC parity tests if direct Supabase access is retained;
7. run redaction snapshots against logs, traces, metrics, and error bodies;
8. keep the feature flag off until a production Flutter `SignalProtocolProvider` and cross-language associated-data fixture pass.

The canonical production schema also needs the reviewed equivalent in `ORESoftware/k8s-cluster/remote/libs/pg-defs/schema/schema.sql`; this repository's migration remains the local/legacy bootstrap contract.
