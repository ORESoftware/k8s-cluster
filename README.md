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
- centralized validation, target fingerprinting/redaction, bounded UTF-8 errors, and retry classification
- CI, dependency auditing, secret scanning, and a non-root container build

Provider adapters and transport ingestion are implemented in subsequent DEN-261 child issues.

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

## Contracts

The shared v1 contract is documented in [`docs/contracts-v1.md`](docs/contracts-v1.md). Example payloads live under [`examples/`](examples/).

Provider target values are secrets or capabilities. Use `target_fingerprint`; never log complete FCM/APNs/Expo tokens or Web Push subscription endpoints and keys.

## Validation

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

## Tracking

Linear project: `github.com/ORESoftware/push-notification-server.rs`

Current implementation issues include DEN-324 for contracts and safety boundaries, followed by provider adapters under DEN-325 through DEN-328.
