# Government-ID, face, and voice account recovery

Shared-auth supports a high-assurance recovery ceremony that combines:

1. government-ID authenticity;
2. selfie-to-ID face match and active liveness;
3. a random spoken challenge plus voice liveness/anti-replay through the Voxletra adapter; any speaker-comparison result is advisory only.

This is an account-recovery option, not a replacement for passkeys, TOTP, or ordinary passwordless recovery. Biometric capture is optional and requires the current, explicit consent version in every enrollment or recovery request.

## Security boundary

Shared-auth never receives or stores government-ID images, camera frames, face templates, voice audio, voiceprints, or speaker embeddings. The browser/mobile client captures media only at short-lived HTTPS provider URLs. Shared-auth persists one-way ceremony/identifier hashes, opaque provider references, normalized verdicts/confidences, timestamps, and reviewer audit identifiers.

A spoken challenge proves liveness, not identity. Speaker comparison is never an authorization factor and cannot grant or deny recovery. Automatic recovery requires prior identity proofing and a repeated government-ID/facial verification; without that prior identity binding, successful capture can only enter `pending_review` and can never auto-approve. Unknown email addresses receive the same launch response shape and non-retaining provider decoy sessions. Public status remains generic `pending_review`; an unknown account has no principal to approve and therefore can only be rejected by review or expire.

## Enrollment

Enrollment requires an active shared-auth access token at AAL2. This prevents a stolen password-only session from binding an attacker's face or voice.

```http
POST /auth/recovery/enrollment
Authorization: Bearer <aal2-access-token>
Content-Type: application/json
```

```json
{
  "accepted_biometric_processing": true,
  "consent_version": "2026-08-04"
}
```

The response contains a one-time `ceremony_token`, two short-lived capture URLs, and the Voxletra challenge phrase. The token is returned once and stored only as a SHA-256 hash.

After both captures finish:

```http
POST /auth/recovery/enrollment/{ceremonyId}/complete
Authorization: Bearer <aal2-access-token>
Content-Type: application/json
```

```json
{ "ceremony_token": "sat_recovery_..." }
```

Document authenticity, face match/liveness, and voice challenge/liveness checks must pass their configured thresholds. A successful enrollment stores only the providers' opaque identity and voice references. An AAL2 user can revoke them with:

```http
DELETE /auth/recovery/enrollment
Authorization: Bearer <aal2-access-token>
```

Revocation also rejects active recovery ceremonies for that user.

## Recovery

Start a recovery ceremony:

```http
POST /auth/recovery/ceremonies
Content-Type: application/json
```

```json
{
  "email": "person@example.com",
  "accepted_biometric_processing": true,
  "consent_version": "2026-08-04"
}
```

The API returns `202` with the same response shape for enrolled, unenrolled, and unknown accounts. The provider contract must likewise create a short-lived session without revealing whether a subject reference exists. `mode: "decoy"` means the provider performs liveness/challenge work but must not create or retain a reusable biometric reference.

After capture, ask shared-auth to poll and normalize both provider decisions:

```http
POST /auth/recovery/ceremonies/{ceremonyId}/complete
Content-Type: application/json
```

```json
{ "ceremony_token": "sat_recovery_..." }
```

Possible public states are deliberately coarse and do not expose provider evidence or account existence:

- `pending`: one or both providers have not finished;
- `pending_review`: no prior binding, provider review, or an inconclusive signal;
- `cooldown`: approved but the configured delay has not elapsed;
- `ready`: approved and redeemable;
- `rejected`, `expired`, or `consumed`;
- `enrolled` for the enrollment endpoint.

Status checks use a POST body rather than a query string so the ceremony token never appears in access logs:

```http
POST /auth/recovery/ceremonies/{ceremonyId}/status
```

Redeem only after `ready`:

```http
POST /auth/recovery/ceremonies/{ceremonyId}/redeem
Content-Type: application/json
```

```json
{
  "ceremony_token": "sat_recovery_...",
  "new_password": "a new long passphrase"
}
```

Redemption updates the Argon2id password, clears lockout state, consumes the ceremony, rejects competing recovery ceremonies, and revokes every active session in one Postgres transaction. It deliberately does not issue a new session; the user must authenticate normally with the new credential.

## Manual review

Bootstrap/inconclusive recovery requires an operator decision. The internal endpoint fails closed unless `AUTH_RECOVERY_REVIEW_SECRET` is configured:

```http
POST /internal/recovery/ceremonies/{ceremonyId}/review
Authorization: Bearer <review-service-secret>
Content-Type: application/json
```

```json
{
  "decision": "approve",
  "reviewer": "trust-ops:case-1234"
}
```

Approval of a bootstrap ceremony persists the newly issued opaque provider references, then starts the same cooldown as automatic recovery. Reviewers must inspect the provider portals and organizational account evidence; shared-auth does not expose raw evidence or provider capture URLs to the review API.

## Provider contracts

### Identity provider

Shared-auth calls the configured HTTPS origin with a bearer credential:

- `POST /v1/identity-verification/sessions`
- `GET /v1/identity-verification/sessions/{sessionId}`

Create request:

```json
{
  "mode": "enroll",
  "subject_reference": "opaque-pseudonymous-reference",
  "correlation_id": "00000000-0000-0000-0000-000000000000",
  "expires_in_seconds": 900
}
```

Create response:

```json
{
  "session_id": "identity-session-id",
  "capture_url": "https://capture.example/session/...",
  "expires_at": 1785859200
}
```

Status response fields are `status`, `result_id`, `reference_id`, `document_verified`, `document_confidence`, `face_match`, `face_liveness`, `face_confidence`, and `expires_at`.

### Voxletra

Shared-auth calls the Voxletra server-auth protected adapter:

- `POST /v1/voice-verification/sessions`
- `GET /v1/voice-verification/sessions/{sessionId}`

The normalized response carries `speaker_match`, `speaker_confidence`, `liveness`, `phrase_match`, and `liveness_confidence`. Speaker fields are retained only as advisory fraud signals and are not consumed as authorization evidence. The Voxletra server delegates speaker embeddings and anti-spoofing to its configured biometric authority and returns `503` when it is not configured; it has no permissive mock fallback.

## Configuration

Required together to enable the feature:

- `AUTH_RECOVERY_IDENTITY_URL`
- `AUTH_RECOVERY_IDENTITY_TOKEN`
- `AUTH_RECOVERY_VOXLETRA_URL`
- `AUTH_RECOVERY_VOXLETRA_TOKEN`
- `AUTH_RECOVERY_SUBJECT_PEPPER`

Optional policy values:

- `AUTH_RECOVERY_REVIEW_SECRET`
- `AUTH_RECOVERY_TTL_SECS` (default `900`)
- `AUTH_RECOVERY_COOLDOWN_SECS` (default `86400`)
- `AUTH_RECOVERY_REDEEM_TTL_SECS` (default `86400`)
- `AUTH_RECOVERY_DOCUMENT_THRESHOLD` (default `0.85`)
- `AUTH_RECOVERY_FACE_THRESHOLD` (default `0.90`)
- `AUTH_RECOVERY_VOICE_LIVENESS_THRESHOLD` (default `0.90`)
- `AUTH_RECOVERY_ALWAYS_MANUAL_REVIEW` (default `false`)
- `AUTH_RECOVERY_CONSENT_VERSION` (default `2026-08-04`)

URLs must use HTTPS outside loopback development. Provider and reviewer secrets must be delivered by Fiducia/runtime secret management and contain at least 32 bytes. Partial configuration fails startup.

## Database rollout

`db/account-recovery.sql` is the declarative schema fragment. Merge it into the canonical `pg-defs` shared-auth schema and apply it with dpm before enabling the runtime variables. The application never runs DDL. Deploying code first is safe while recovery remains unconfigured; enabling it before the tables exist will fail requests closed with `503`/upstream errors.

## Operational controls

- At most three enrollment or recovery launches per identifier in 24 hours.
- At most ten provider evaluations per ceremony.
- Capture URLs and tokens are never logged.
- Provider JSON responses are bounded to 64 KiB and redirects are disabled.
- Capture, review, cooldown, and redemption all have independent expiry limits.
- Manual approval is impossible for an unknown account; its public ceremony remains generic until review rejection or expiry.
- Successful redemption revokes all sessions and does not mint a replacement.
