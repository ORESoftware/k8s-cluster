# Account security and recovery backend boundary

Tracking: DEN-150.

## Device lifecycle

Devices are revisioned and move through `pending`, `active`, `suspended`, and terminal `revoked` states. A new device publishes public Signal Protocol prekeys and remains pending until a trusted device approves it through QR/safety-number verification or an explicitly approved recovery flow. Revocation invalidates the service-local token, blocks prekey/mailbox operations, removes the device from future fan-out, and causes remaining devices to rotate affected wrapping keys.

## Backup email and phone OTP

Recovery destinations are envelope-encrypted with a service/KMS key. A keyed blind digest enforces per-account uniqueness without storing plaintext. Responses expose only a masked destination. OTP challenges store only a keyed digest, expire within fifteen minutes, have bounded attempts, and become consumed or invalidated exactly once.

Handlers must enforce account/channel/device/network risk rate limits, challenge issuance cooldowns, single active challenge policy where appropriate, replay protection, and constant-time digest comparison. Destinations, codes, digests, account/device IDs, and request bodies are prohibited from logs, traces, metric labels, crash reports, and user-visible error bodies.

## Biometrics, passkeys, and six-digit PIN

Biometric authentication is performed by platform authenticators or shared-auth/passkeys. This service receives only a verified assertion or short-lived step-up token; it never receives fingerprint, face, voice, or other biometric templates.

The six-digit PIN is local-only. The backend stores neither the PIN nor a verifier. Clients may expose a bounded Argon2id/scrypt policy describing how a random device-wrapping key is protected locally, but the PIN is never the account master key, vault key, Signal key, recovery key, or service credential.

## Recovery package

Trusted-device transfer is the default recovery path. Users may explicitly opt in to an encrypted recovery package. The backend stores only opaque ciphertext and associated metadata. Email/SMS/passkey/biometric success may authorize retrieval, but only the user's recovery key can decrypt it. Recovery rotates device/session material and records a redacted lifecycle event.

## Route rollout gates

No new recovery route should be enabled until:

1. shared interfaces and generated clients land;
2. destination envelope encryption is backed by reviewed KMS/key rotation;
3. OTP delivery providers and templates are configured without logging destinations/codes;
4. account/channel/device/network rate-limit and abuse tests pass;
5. PostgreSQL transaction tests cover duplicate issuance, expiry, attempts, consumption, invalidation, and channel removal races;
6. Flutter device/recovery UI and platform secure storage are complete;
7. E2E tests prove revoked devices cannot fetch mailbox data or recover keys.
