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
- centralized validation, target fingerprinting/redaction, bounded UTF-8 errors, and retry classification
- CI, dependency auditing, secret scanning, and a non-root container build

APNs, Expo, Web Push, and transport ingestion are implemented in subsequent DEN-261 child issues.

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

DEN-324 established the contracts and safety boundary. DEN-325 implements FCM; DEN-326 through DEN-328 cover APNs, Expo, and Web Push.
