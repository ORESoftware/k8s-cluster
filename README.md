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
- a Web Push/VAPID adapter with encrypted payloads, redirect blocking, strict push-service allowlisting, private-address rejection, optional DNS-vetted open mode, endpoint redaction, and normalized outcomes
- centralized validation, target fingerprinting/redaction, bounded UTF-8 errors, and retry classification
- CI, dependency auditing, secret scanning, and a non-root container build

Authenticated HTTP/NATS transport ingestion is implemented in the next DEN-261 child issue.

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

Create `WebPushConfig` using a secret-backed VAPID EC private key, a `mailto:` or HTTPS subject, and a `WebPushHostPolicy`. The default policy permits only known browser push-service hosts and real subdomains.

The adapter:

- encrypts JSON payloads using `aes128gcm`
- signs requests using VAPID
- requires HTTPS on port 443 without embedded credentials or fragments
- disables HTTP redirects
- rejects localhost, private, link-local, CGNAT, documentation, benchmarking, reserved, multicast, and mapped internal addresses
- offers an explicitly weaker any-public-host mode with preflight DNS vetting
- maps TTL, urgency, and a hashed 32-character collapse topic
- classifies expired subscriptions, invalid payloads, throttling, transient failures, and permanent VAPID/provider rejection
- redacts endpoint paths and query strings

Detailed SSRF controls, residual DNS-rebinding risk, payload behavior, and tests are documented in [`docs/web-push.md`](docs/web-push.md).

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

DEN-324 established the contracts and safety boundary. DEN-325 through DEN-328 implement FCM, APNs, Expo, and Web Push. DEN-329 adds versioned authenticated HTTP/NATS ingestion.
