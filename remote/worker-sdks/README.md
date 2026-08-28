# Worker SDKs

This tree contains hand-authored clients and long-lived worker loops for the independent durable-worker runtime.

It is intentionally separate from `remote/api-sdks`, which is deterministic generated OpenAPI output governed by `remote/tools/generate-api-sdks.mjs`. Generated API clients must remain reproducible; worker SDKs additionally own execution lifecycle behavior such as renewable leases, monotonic fencing, heartbeat loops, cancellation, bounded concurrency, and stale-result suppression.

Available hand-authored worker SDKs:

- `typescript/durable-worker` — dependency-free native ESM for Node.js 22+;
- `python/durable-worker` — dependency-free Python 3.11+ client and threaded worker loop;
- `go/durable-worker` — dependency-free Go 1.23+ client and goroutine-based worker loop;
- `rust/durable-worker` — async Rust 1.85+ client and Tokio worker loop with a replaceable transport boundary.

Shared lifecycle semantics are ratcheted in `fixtures/durable-worker-protocol-v1.json`. The fixture defines ambiguous operations that must not be retried without a protocol identity, lease-loss statuses, progress identity, and the common assignment envelope. Language-specific runtime tests remain authoritative for concurrency and cancellation behavior.

All worker SDKs must preserve the runtime's at-least-once delivery contract. External side effects require an idempotency key or a downstream write guarded by the assignment fencing token.
