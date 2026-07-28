# Authenticated HTTP push-job ingestion v1

The HTTP API accepts the same provider-neutral `PushJob` v1 contract used by provider adapters and the future NATS/JetStream consumer.

## Routes

- `GET /healthz`
- `GET /readyz`
- `POST /v1/push/jobs`
- `POST /v1/push/jobs/batch`

Request bodies are bounded to 512 KiB. Batch requests contain `{"jobs": [...]}` and are limited to 100 jobs.

## Authentication

Ingestion fails closed by default. When no authenticator is configured, every push-job submission returns HTTP 401.

The first migration mode is explicitly enabled with:

```text
ENABLE_SHARED_SECRET_AUTH=true
SERVER_AUTH_SECRET=<at least 32 characters>
```

Clients send:

```text
Authorization: Bearer <shared secret>
```

Comparison is constant-time for equal-length inputs. This mode exists only to migrate existing internal producers. Production should move to service JWT or workload identity without changing the `PushJob` contract or provider registry.

Provider credentials never appear in producer requests.

## Validation and dispatch

Ingress validates the complete `PushJob` before provider dispatch. The registry routes:

- FCM targets to the FCM adapter
- APNs production and sandbox targets to separate provider slots
- Expo targets to the Expo adapter
- Web Push targets to the Web Push adapter

Expo is available without credentials unless project enhanced security requires an access token. Other providers are registered only when their server-side environment configuration is present and valid. Partial APNs configuration fails startup rather than silently disabling one field.

## Responses

Single submissions return a redacted `PushOutcome`.

- accepted → 202
- invalid payload → 400
- invalid/expired target → 422
- throttled → 429
- transient/internal failure → 503
- permanent provider/configuration failure → 502

Batch responses return accepted/rejected counts and one redacted outcome per job. A completely accepted batch returns 202; a mixed or rejected batch returns 207.

Outcomes contain target fingerprints, never complete provider tokens, Web Push endpoint paths, or subscription key material.

## Readiness

`/readyz` returns 200 only when:

- an ingestion authenticator is configured; and
- at least one provider reports ready.

The readiness body reports authentication mode and separate FCM, APNs production, APNs sandbox, Expo, and Web Push readiness without exposing credentials.

## Tests

The HTTP integration suite verifies:

- all submissions fail closed when no authenticator is configured
- the explicit migration bearer secret authorizes valid requests
- authenticated jobs dispatch through the provider registry
- validation failures never echo capability tokens
- batches above the 100-job limit are rejected before provider dispatch
- readiness remains unavailable without authentication or providers

Test-only HTTP body and provider-kind imports remain gated under `cfg(test)` so production builds stay warning-free with Clippy warnings denied.

The permanent merge gate additionally requires formatting, locked Clippy with warnings denied, all unit/integration tests, the Rust 1.88 container build, cargo-deny, RustSec, and full-history Gitleaks.

## NATS follow-up

The next DEN-329 PR adds dedicated versioned JetStream job/result subjects, explicit ack/nak/term behavior, retry/dead-letter policy, and signed/enveloped job authentication while reusing this registry, validation, and outcome mapping.
