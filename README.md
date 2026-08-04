# push-notification-server.rs

Dedicated Rust notification delivery service for Firebase Cloud Messaging HTTP v1, Apple Push Notification service, Expo Push, browser Web Push/VAPID, and optional SendGrid email and Twilio SMS fallback lanes.

Canonical GitHub home: [`fanwaave/push-notification-server.rs`](https://github.com/fanwaave/push-notification-server.rs). New remotes, CI references, deployment manifests, and documentation should use the Fanwaave location. The former ORESoftware location is only the transfer source and should not be recreated after ownership moves.

Push remains the primary, isolated contract. The service uses a versioned provider-neutral `PushJob`/`PushOutcome` contract, target fingerprinting, bounded errors, strict validation, permanent CI/security checks, and a non-root container. Email and SMS use a separate `ContactJob`/`ContactOutcome` contract so adding fallback channels cannot weaken push-target validation or allow producer-controlled provider credentials and sender identities. Supabase/Postgres may store installation registrations and transactional outbox jobs, but it is not a delivery provider.

Push provider adapters:

- FCM HTTP v1 with service-account OAuth and token caching
- APNs with ES256 provider tokens and strict production/sandbox isolation
- Expo Push with batched tickets and receipt follow-up
- Web Push with direct RFC 8291 ECE encryption, ES256 VAPID, redirect blocking, strict host/address policy, and endpoint redaction

Optional contact adapters:

- SendGrid Mail Send with verified server-side sender identity, global/EU API selection, explicit-content and dynamic-template modes, sandbox support, bounded error classification, and recipient redaction
- Twilio Messages with Auth Token or API Key authentication, Messaging Service or E.164 sender selection, optional status callback and validity period, bounded error-code classification, and phone-number redaction

Ingestion interfaces:

- fail-closed authenticated HTTP v1 single and batch routes
- optional durable NATS JetStream WorkQueue ingestion for push jobs with dedicated result/dead-letter subjects
- shared validation, redacted outcomes, trace context, and retry classification

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
- `GET /v1/contact/readyz`
- `POST /v1/contact/jobs`
- `POST /v1/contact/jobs/batch`

JetStream remains disabled unless `NATS_URL` is configured. The initial contact lanes are HTTP-only; durable email/SMS subjects and signed delivery-status webhook ingestion are tracked separately so their semantics cannot be confused with push acceptance.

## Configuration

Provider credentials and sender identities are server-side configuration. Use Kubernetes External Secrets, workload identity, or another managed secret boundary. Never commit service-account JSON, private keys, API keys, Auth Tokens, device tokens, recipient addresses, phone numbers, Web Push endpoints, or subscription key material.

A successful SendGrid or Twilio API response means the provider accepted the request; it does not prove final delivery. Final email/SMS delivery state must come from signature-verified provider event/status callbacks and be persisted separately from the immediate `ContactOutcome`.

Examples are documented in `.env.example`. Detailed protocol and operations documents:

- [`docs/contracts-v1.md`](docs/contracts-v1.md)
- [`docs/http-ingestion-v1.md`](docs/http-ingestion-v1.md)
- [`docs/nats-ingestion-v1.md`](docs/nats-ingestion-v1.md)
- [`docs/contact-delivery-v1.md`](docs/contact-delivery-v1.md)
- [`docs/sendgrid-twilio-audit.md`](docs/sendgrid-twilio-audit.md)
- [`docs/apns.md`](docs/apns.md)
- [`docs/expo.md`](docs/expo.md)
- [`docs/web-push.md`](docs/web-push.md)

## JetStream reliability

The durable push consumer:

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

GitHub Actions additionally validates process-level HTTP, live NATS compatibility, the Rust 1.88 container, cargo-deny policy, RustSec advisories, and full Git history with Gitleaks.

## Tracking

GitHub repository: `github.com/fanwaave/push-notification-server.rs`

Linear project: `github.com/ORESoftware/push-notification-server.rs` (existing tracker; rename after the GitHub transfer)

DEN-324 established the push contracts and safety boundary. DEN-325 through DEN-328 implement the four push adapters. DEN-329 adds authenticated HTTP and durable push NATS ingestion. DEN-331 established the first fully green integration-tested source SHA. SendGrid/Twilio hardening is tracked under the production conformance and migration program without reopening the old mixed-service push implementation.
