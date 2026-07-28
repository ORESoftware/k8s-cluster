# push-notification-server.rs

Dedicated Rust push-notification delivery service for Firebase Cloud Messaging HTTP v1, Apple Push Notification service, Expo Push, and browser Web Push/VAPID.

The service uses a versioned provider-neutral `PushJob`/`PushOutcome` contract, target fingerprinting, bounded errors, strict validation, permanent CI/security checks, and a non-root container. Supabase/Postgres may store installation registrations and transactional outbox jobs, but it is not a push provider.

Provider adapters:

- FCM HTTP v1 with service-account OAuth and token caching
- APNs with ES256 provider tokens and strict production/sandbox isolation
- Expo Push with batched tickets and receipt follow-up
- Web Push with direct RFC 8291 ECE encryption, ES256 VAPID, redirect blocking, strict host/address policy, and endpoint redaction

Ingestion interfaces:

- fail-closed authenticated HTTP v1 single and batch routes
- optional durable NATS JetStream WorkQueue ingestion with dedicated result/dead-letter subjects
- shared provider registry, validation, redacted outcomes, trace context, and retry classification

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

Current HTTP endpoints:

- `GET /healthz`
- `GET /readyz`
- `POST /v1/push/jobs`
- `POST /v1/push/jobs/batch`

JetStream remains disabled unless `NATS_URL` is configured.

## Configuration

Provider credentials are server-side secrets. Use Kubernetes External Secrets, workload identity, or another managed secret boundary. Never commit service-account JSON, private keys, access tokens, device tokens, Web Push endpoints, or subscription key material.

Examples are documented in `.env.example`. Detailed protocol and operations documents:

- [`docs/contracts-v1.md`](docs/contracts-v1.md)
- [`docs/http-ingestion-v1.md`](docs/http-ingestion-v1.md)
- [`docs/nats-ingestion-v1.md`](docs/nats-ingestion-v1.md)
- [`docs/apns.md`](docs/apns.md)
- [`docs/expo.md`](docs/expo.md)
- [`docs/web-push.md`](docs/web-push.md)

## JetStream reliability

The durable consumer:

- uses dedicated versioned job, result, and dead-letter streams/subjects
- publishes a redacted result before Ack
- sends ack-progress heartbeats during long provider calls
- delayed-NAKs retryable outcomes while attempts remain
- dead-letters and Terms final retryable or poison messages
- hashes raw payloads instead of copying capability-bearing targets into DLQ records
- bounds concurrency and message size
- relies on NATS account/subject ACLs, with optional migration envelope authentication

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

DEN-324 established the contracts and safety boundary. DEN-325 through DEN-328 implement the four provider adapters. DEN-329 adds authenticated HTTP and durable NATS ingestion.
