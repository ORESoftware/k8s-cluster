# push-notification-server.rs

Dedicated Rust push-notification delivery service for:

- Firebase Cloud Messaging HTTP v1
- Apple Push Notification service
- Expo Push
- Web Push/VAPID

Supabase/Postgres may store device registrations and transactional outbox jobs, but it is not a push-delivery provider.

## Status

The repository contains:

- Axum/Tokio health and readiness endpoints
- graceful shutdown and environment-driven binding
- versioned provider-neutral `PushJob` and `PushOutcome` contracts
- provider traits for FCM, APNs, Expo, and Web Push adapters
- an FCM HTTP v1 adapter with server-side service-account OAuth, single-flight access-token caching, data-string coercion, mockable endpoints, and normalized result classes
- an APNs HTTP/2 adapter with `.p8` token authentication, single-flight ES256 provider-token caching, strict sandbox/production isolation, alert/background payloads, mockable endpoints, and normalized Apple error reasons
- an Expo adapter with optional project bearer authentication, batched push tickets, HTTP-200 ticket-error parsing, receipt follow-up, mockable endpoints, and normalized receipt outcomes
- a Web Push adapter with direct RFC 8291 ECE encryption, ES256 VAPID signing, strict SSRF controls, redirect blocking, endpoint redaction, and normalized push-service outcomes
- centralized validation, target fingerprinting/redaction, bounded UTF-8 errors, and retry classification
- CI, dependency auditing, secret scanning, and a non-root container build

Authenticated HTTP and NATS ingestion is implemented in DEN-329.

## Run

```bash
cargo run
```

Defaults:

```text
HOST=0.0.0.0
PORT=8121
RUST_LOG=push_notification_server=info,tower_http=info
```

Endpoints:

- `GET /healthz`
- `GET /readyz`

## FCM configuration

Create an `FcmConfig` from a Google service-account JSON document and construct `FcmProvider`. The JSON must contain `client_email`, `private_key`, and `project_id`; `FCM_PROJECT_ID` may provide an explicit project override. The token endpoint must be HTTPS and may not contain embedded credentials.

Credentials belong in Kubernetes External Secrets or workload identity configuration. Never commit the service-account JSON, private key, OAuth access token, or complete device token.

The adapter:

- mints RS256 OAuth assertions for the Firebase Messaging scope
- caches access tokens until 60 seconds before provider expiry
- holds one refresh lock so concurrent callers do not mint duplicate tokens
- maps title, body, image, TTL, priority, collapse key, dry-run, and application data
- coerces every FCM data value to a string
- classifies invalid tokens, invalid payloads, throttling, transient failures, and permanent provider/authentication failures
- returns only the target fingerprint in result events

## APNs configuration

Create a distinct `ApnsConfig` for production or sandbox using the Apple key ID, Team ID, bundle/topic, `.p8` private key, and `ProviderEnvironment`. A provider instance rejects targets from the other environment instead of silently routing them to the wrong Apple host.

Credentials belong in Kubernetes External Secrets. Never commit the `.p8` key, provider token, or complete device token.

The adapter:

- signs ES256 provider tokens containing the Apple Team ID and issue time
- reuses each provider token for 50 minutes behind a single refresh lock
- selects `api.push.apple.com` or `api.sandbox.push.apple.com` from the configured environment
- maps alert and data-only jobs to the corresponding APNs push type and priority
- sends topic, expiration, collapse ID, and canonical UUID request IDs
- keeps custom data beside the reserved `aps` dictionary and rejects producer attempts to replace `aps`
- enforces the 4 KiB APNs payload ceiling
- classifies invalid tokens, invalid payloads, throttling, transient failures, and provider/authentication failures from Apple response reasons
- returns only the target fingerprint and Apple request ID/reason in result events

Detailed APNs protocol, safety, and test behavior is documented in [`docs/apns.md`](docs/apns.md).

## Expo configuration

Create `ExpoConfig` with an optional project access token and construct `ExpoProvider`. The token is required only when enhanced Expo push security is enabled and must come from an external server-side secret.

The adapter:

- sends between 1 and 100 messages per push-ticket request
- validates Expo capability-token shape without logging the complete token
- maps title, body, data, image, TTL, priority, and collapse ID
- parses every per-message ticket, including errors returned with HTTP 200
- retains accepted ticket IDs for receipt processing
- looks up between 1 and 1,000 receipts per request
- treats missing receipts as retryable
- classifies invalid devices, invalid payloads, throttling, sender mismatch, and invalid credentials
- returns target fingerprints rather than complete Expo device tokens

Detailed ticket, receipt, retry, and test behavior is documented in [`docs/expo.md`](docs/expo.md).

## Web Push configuration

Create `WebPushConfig` from a P-256 VAPID private key, VAPID subject, and host policy, then construct `WebPushProvider`.

The adapter:

- encrypts payloads directly with Mozilla ECE using `aes128gcm`
- signs VAPID tokens with ES256 and derives the public key from the configured P-256 private key
- avoids an unused RSA dependency path with an unresolved security advisory
- defaults to known browser push-service hosts
- requires HTTPS port 443 without embedded credentials or fragments
- disables redirects and rejects internal, private, loopback, link-local, CGNAT, documentation, benchmarking, reserved, multicast, unique-local, site-local, and mapped internal addresses
- offers a weaker opt-in any-public-host mode with DNS vetting and documented rebinding limitations
- validates decoded subscription key lengths
- maps TTL, urgency, and hashed collapse topics
- redacts endpoint paths and subscription key material

Detailed cryptographic, SSRF, retry, and test behavior is documented in [`docs/web-push.md`](docs/web-push.md).

## Contracts

The shared v1 contract is documented in [`docs/contracts-v1.md`](docs/contracts-v1.md). Example payloads live under [`examples/`](examples/).

Provider target values are secrets or capabilities. Use `target_fingerprint`; never log complete FCM/APNs/Expo tokens or Web Push subscription endpoints and keys.

## Validation

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
```

## Tracking

Linear project: `github.com/ORESoftware/push-notification-server.rs`

DEN-324 established the contracts and safety boundary. DEN-325 through DEN-328 implement all four provider adapters. DEN-329 adds authenticated HTTP and NATS ingestion.
