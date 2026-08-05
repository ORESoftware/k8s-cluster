# Worker SDKs

This tree contains hand-authored clients and long-lived worker loops for the independent durable-worker runtime.

It is intentionally separate from `remote/api-sdks`, which is deterministic generated OpenAPI output governed by `remote/tools/generate-api-sdks.mjs`. Generated API clients must remain reproducible; worker SDKs additionally own execution lifecycle behavior such as renewable leases, monotonic fencing, heartbeat loops, cancellation, bounded concurrency, and stale-result suppression.

All worker SDKs must preserve the runtime's at-least-once delivery contract. External side effects require an idempotency key or a downstream write guarded by the assignment fencing token.
