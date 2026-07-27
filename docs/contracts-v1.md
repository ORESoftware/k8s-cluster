# PushJob v1 contract

`PushJob` is the provider-neutral boundary shared by HTTP, NATS, database outbox publishers, and provider adapters. Producers select a provider and target but never construct provider-specific request bodies or handle provider credentials.

## Required invariants

- `version` defaults to `v1` and must be preserved in result events.
- `tenant_id`, `application_id`, and `idempotency_key` are mandatory.
- `provider` must match the tagged `target.type`.
- Target values are secrets or capabilities and must never be logged or emitted in results.
- `target_fingerprint` is the only target identifier allowed in routine logs, metrics, results, and audit records.
- Provider adapters return a normalized `PushOutcome` class.
- Provider-specific codes may refine a normalized outcome but cannot bypass validation, redaction, or retry ceilings.

## Outcome classes

| Class | Retry by default | Token lifecycle |
| --- | --- | --- |
| `accepted` | No | Record provider acceptance |
| `invalid_token` | No | Disable or replace the registration |
| `invalid_payload` | No | Fix producer data |
| `throttled` | Yes, bounded | Preserve target |
| `transient_provider_failure` | Yes, bounded | Preserve target |
| `permanent_provider_failure` | No | Escalate provider/configuration issue |
| `internal_failure` | Yes, bounded | Investigate service failure |

## HTTP and NATS examples

See `examples/push-job-v1.json` and `examples/push-outcome-v1.json`. Future routes and subjects must use these same structures rather than inventing transport-specific payloads.

## Security boundary

This contract intentionally performs only baseline Web Push URL validation. DNS resolution, public-address enforcement, provider-host allowlisting, and rebinding controls are implemented by the Web Push adapter under DEN-328.
