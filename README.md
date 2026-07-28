# push-notification-server.rs

Dedicated Rust push-notification delivery service for Firebase Cloud Messaging HTTP v1, Apple Push Notification service, Expo Push, and browser Web Push/VAPID.

The service uses a versioned provider-neutral `PushJob`/`PushOutcome` contract, target fingerprinting, bounded errors, strict validation, permanent CI/security checks, and a non-root container. Supabase/Postgres may store installation registrations and transactional outbox jobs, but it is not a push provider.

Provider adapters:

- FCM HTTP v1 with service-account OAuth and token caching
- APNs with ES256 provider tokens and strict production/sandbox isolation
- Expo Push with batched tickets and receipt follow-up
- Web Push with direct RFC 8291 ECE encryption, ES256 VAPID, redirect blocking, strict host/address policy, and endpoint redaction

Authenticated versioned HTTP and NATS ingestion is tracked by DEN-329.

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

Current endpoints:

- `GET /healthz`
- `GET /readyz`

## Configuration

Provider credentials are server-side secrets. Use Kubernetes External Secrets, workload identity, or another managed secret boundary. Never commit service-account JSON, private keys, access tokens, device tokens, Web Push endpoints, or subscription key material.

Examples are documented in `.env.example`. Detailed protocol and operations documents:

- [`docs/contracts-v1.md`](docs/contracts-v1.md)
- [`docs/apns.md`](docs/apns.md)
- [`docs/expo.md`](docs/expo.md)
- [`docs/web-push.md`](docs/web-push.md)

## Web Push security

The Web Push adapter:

- uses Mozilla ECE directly for `aes128gcm` encryption
- signs VAPID with ES256 using a P-256 private key
- contains no unused RSA signing dependency path
- defaults to known browser push-service hosts
- requires HTTPS port 443 without embedded credentials or fragments
- disables redirects
- blocks private, loopback, link-local, CGNAT, documentation, benchmarking, reserved, multicast, unique-local, site-local, and mapped internal addresses
- supports a weaker explicit any-public-host mode with preflight DNS validation and documented rebinding limitations
- redacts endpoint paths, query strings, and subscription key material

## Validation

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
```

GitHub Actions additionally validates the Rust 1.88 container, cargo-deny policy, RustSec advisories, and full Git history with Gitleaks.

## Tracking

Linear project: `github.com/ORESoftware/push-notification-server.rs`

DEN-324 established the contracts and safety boundary. DEN-325 through DEN-328 implement the four provider adapters. DEN-329 adds authenticated HTTP and NATS ingestion.
