# Durable NATS JetStream push ingestion v1

The JetStream consumer accepts the same validated `PushJob` v1 contract used by HTTP and provider adapters. It uses dedicated subjects and streams, separate from email and SMS lanes.

## Default subjects and streams

| Purpose | Stream | Subject |
|---|---|---|
| Work queue | `PUSH_JOBS_V1` | `push.jobs.v1` |
| Results | `PUSH_RESULTS_V1` | `push.results.v1` |
| Dead letters | `PUSH_DEAD_V1` | `push.dead.v1` |

The durable pull consumer defaults to `push-notification-server-v1`.

The job stream uses WorkQueue retention, a seven-day maximum age, explicit acknowledgements, a 120-second ack wait, and five delivery attempts by default. Result events are retained for seven days and dead-letter audit events for 30 days.

All duration, delivery, payload, and concurrency environment overrides must be positive integers. Invalid values fail startup instead of silently falling back to unsafe limits.

## Envelope

Producers publish:

```json
{
  "schema": "push.job.envelope.v1",
  "job": {
    "version": "v1"
  }
}
```

The omitted `job` fields are the complete `PushJob` v1 contract documented in `contracts-v1.md`.

NATS account and subject ACLs are the primary authorization boundary. During migration, `ENABLE_NATS_ENVELOPE_AUTH=true` requires an `auth` field matching `NATS_SHARED_SECRET`. The secret is never copied into result or dead-letter events.

Provider credentials never appear in the envelope.

## Processing order

1. Enforce the configured payload-size ceiling.
2. Parse the versioned envelope.
3. Validate its schema and optional migration authentication.
4. Validate the `PushJob` contract.
5. Dispatch through the same provider registry used by HTTP.
6. Durably publish a redacted `push.result.v1` event.
7. Ack, delayed-Nak, or dead-letter/Term the job.

A result must be durably published before the job is acknowledged. If result publishing fails, the consumer NAKs the job for another attempt.

Long provider calls send `AckKind::Progress` heartbeats at one-third of the ack-wait interval so healthy in-flight work is not redelivered. JetStream's signed delivery counter is normalized once into a nonnegative `u64` before it enters result or dead-letter schemas.

## Dispositions

- accepted, invalid target, invalid payload, or permanent provider failure: publish result, then Ack
- throttled, transient provider failure, or internal failure with attempts remaining: publish result, then delayed Nak
- retryable outcome on the final permitted delivery: publish result, publish a redacted dead-letter audit event, then Term
- malformed, unsupported, unauthenticated, or oversized envelope: publish a redacted dead-letter audit event, then Term

## Dead-letter safety

Dead-letter events never copy the raw message payload because it can contain device tokens or Web Push capabilities. They store:

- SHA-256 of the original payload
- payload byte length
- safe reason code
- job, tenant, and application identifiers when parsing succeeded
- redacted normalized outcome when available
- delivery count and configured maximum

This preserves forensic correlation without turning the DLQ into a capability-secret archive.

## Backpressure and recovery

A semaphore bounds concurrent handlers. Consumer fetch streams are recreated after errors, while `async-nats` handles connection recovery. Job processing is at-least-once; deterministic `idempotency_key` values and the future DEN-342 deduplication store prevent uncontrolled duplicate user-visible notifications.

## Tests and merge gate

Unit tests cover envelope authentication, outcome-to-disposition mapping, provider dispatch, result redaction, and payload-hash-only dead letters. CI additionally requires formatting, locked Clippy, all tests, the Rust 1.88 container build, cargo-deny, RustSec, and full-history Gitleaks.
