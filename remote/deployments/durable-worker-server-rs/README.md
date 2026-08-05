# dd-durable-worker-server

`dd-durable-worker-server` is the independent durable-execution control plane for
`ORESoftware/k8s-cluster`. It is deliberately additive: the existing
`agent-worker-broker-rs`, Node.js agent worker, Gleam Lambda runner, and
`queue-consumer-rs` remain valid execution paths while this service supplies the
missing general-purpose run/step state machine.

The first production slice is designed around the primitives that matter for AI
agents and ordinary background work without importing Temporal, Hatchet,
Inngest, Trigger.dev, or Restate as a runtime dependency:

- one-off tasks are first-class (`POST /api/v1/tasks`);
- optional DAG runs with validated dependencies (`POST /api/v1/runs`);
- durable run, step, worker, idempotency, signal, and concurrency-lane state in
  NATS JetStream KV;
- compare-and-swap lease acquisition across replicas;
- lease generations and fencing tokens that reject stale workers;
- worker registration, capabilities, queue routing, slots, draining, heartbeat,
  and bounded long-polling;
- priorities, delayed steps, durable signals, retries with bounded exponential
  backoff, hard execution timeouts, and lease-expiration recovery;
- keyed concurrency lanes that apply across workers and runs;
- pause, resume, and cancellation;
- append-only lifecycle and streamed-output events in JetStream, plus live SSE;
- native `utoipa-axum` OpenAPI, fail-closed public docs, authenticated internal
  docs, health/readiness, and Prometheus metrics.

## Relationship to the current worker path

The control plane does **not** replace the live thread-affinity path in its first
rollout.

```text
Current path (unchanged)
client -> agent-worker-broker-rs -> dd.remote.thread.<thread>.tasks
       -> queue-consumer-rs / Node worker

New additive path
client -> dd-durable-worker-server -> JetStream KV + event journal
worker (Node/Gleam/Rust/Go/etc.) -> HTTP register/poll/lease/output/complete
```

A compatibility adapter can subsequently translate an eligible durable step to
the existing `dd.remote.thread.*.tasks` envelope. Keeping that adapter outside
the state machine prevents the durable protocol from inheriting Node-specific
or thread-specific assumptions.

## Guarantees

- **Durability:** accepted state transitions are persisted in JetStream KV.
- **Delivery:** task execution is at-least-once. Workers must make external side
  effects idempotent.
- **Single active lease per step:** the step record is changed with KV revision
  CAS; only one replica can commit the winning lease.
- **Stale-worker fencing:** every lease carries a monotonically increasing
  generation/fencing token. Completion and failure commands must match it.
- **Concurrency:** worker slots and user-defined concurrency keys use separate
  CAS-backed lane records.
- **Recovery:** the scheduler converts expired leases or hard execution
  timeouts to retry or terminal failure, releases lane holders, and promotes due
  timers/retries. Heartbeats extend the step lease and both capacity lanes, but
  never extend the execution timeout.
- **Failure policy:** protocol v1 is fail-fast. A terminal step failure cancels
  every nonterminal sibling and descendant and fences their outstanding leases.
- **Command idempotency:** repeated terminal acknowledgements using the same
  lease return the already-committed result; streamed chunks use `chunkId`.
- **Event history:** state is authoritative; lifecycle events are a best-effort
  operational journal after state commit. Output chunks are stricter: the
  journal acknowledgement is required before HTTP success, and retries reuse a
  stable event ID with `Nats-Msg-Id` deduplication.

This is not deterministic stack-frame replay. Durable boundaries are explicit
steps, waits, signals, and effects. That keeps workers language-neutral and lets
Gleam, Node.js, Rust, Go, Dart, or Python participate through the same protocol.

## HTTP protocol

All internal routes require either `X-Worker-Auth` or the compatibility header
`X-Server-Auth`.

| Operation | Route |
| --- | --- |
| Submit one task | `POST /api/v1/tasks` |
| Submit DAG | `POST /api/v1/runs` |
| Read run | `GET /api/v1/runs/{runId}` |
| Stream live events | `GET /api/v1/runs/{runId}/events` |
| Signal a wait | `POST /api/v1/runs/{runId}/signals/{signal}` |
| Pause/resume/cancel | `POST /api/v1/runs/{runId}/{operation}` |
| Register worker | `POST /api/v1/workers/register` |
| Worker heartbeat | `POST /api/v1/workers/{workerId}/heartbeat` |
| Long-poll assignment | `POST /api/v1/workers/{workerId}/poll?waitMs=30000` |
| Start/heartbeat/output | `POST /api/v1/steps/{stepId}/{operation}` |
| Complete/fail | `POST /api/v1/steps/{stepId}/{operation}` |

See [PROTOCOL.md](./PROTOCOL.md) for wire examples and
[`examples/node-worker.mjs`](./examples/node-worker.mjs) for a dependency-free
Node.js worker loop.

## Configuration

| Environment variable | Default | Purpose |
| --- | --- | --- |
| `PORT` | `8152` | HTTP listen port |
| `DURABLE_WORKER_AUTH_SECRET` | required | Internal worker/service secret |
| `NATS_URL` | in-cluster NATS service | NATS connection URL |
| `NATS_CREDENTIALS_FILE` | unset | Optional NATS user credentials |
| `NATS_TOKEN` | unset | Optional NATS token |
| `NATS_REQUIRE_TLS` | `false` | Require NATS TLS |
| `DURABLE_WORKER_STATE_BUCKET` | `DD_DURABLE_WORKER_STATE` | KV bucket |
| `DURABLE_WORKER_EVENT_STREAM` | `DD_DURABLE_WORKER_EVENTS` | Event stream |
| `DURABLE_WORKER_EVENT_SUBJECT` | `dd.durable.run.*.events` | Run event subject pattern |
| `DURABLE_WORKER_NATS_REPLICAS` | `1` | JetStream replicas (raise with clustered NATS) |
| `DURABLE_WORKER_POLL_MAX_WAIT_MS` | `30000` | Maximum HTTP long poll |
| `DURABLE_WORKER_SCHEDULER_INTERVAL_MS` | `1000` | Lease/timer reconciliation interval |
| `DURABLE_WORKER_SHADOW_MODE` | `true` | Signals additive rollout posture |

`DURABLE_WORKER_ALLOW_INSECURE_LOCAL=true` supplies a local-only development
secret. It must not be used in Kubernetes.

## Local run

```bash
nats-server -js
DURABLE_WORKER_ALLOW_INSECURE_LOCAL=true \
NATS_URL=nats://127.0.0.1:4222 \
cargo run
```

OpenAPI export does not parse environment variables or connect to NATS:

```bash
cargo run -- --export-openapi
cargo run -- --export-public-openapi
```

## Rollout sequence

1. Deploy in shadow mode and exercise the HTTP protocol with synthetic workers.
2. Add thin SDKs/clients for Node.js, Gleam, Rust, Go, and Dart from the internal
   OpenAPI contract.
3. Add an adapter that dispatches selected durable steps through the existing
   thread-affinity subjects.
4. Add cron/calendar schedules, durable event replay cursors, worker push over
   gRPC/WebSocket, per-tenant quotas, and a run graph UI.
5. Move selected agent workflows from ad-hoc queue code only after parity and
   recovery tests pass. The existing path remains an explicit fallback.

## Known first-slice boundaries

- The SSE endpoint is live-tail; durable historical replay is currently through
  the JetStream journal and will receive an HTTP cursor API in a follow-up. SSE
  clients must deduplicate by `eventId` across reconnects.
- Run materialization spans several KV keys. Deterministic step IDs and
  idempotency repair make retries safe, but a future transaction journal will
  make partial-creation recovery automatic without a client retry.
- Scheduler candidate discovery currently scans KV keys, which is intentionally
  simple and correct for the shadow rollout but O(total state). Ready queues,
  shards, and tenant indexes are the next scale milestone.
- Cron/calendar scheduling, child runs, compensation/continue-on-error policies,
  versioned workflow definitions, UI, and multi-region replication policy are
  roadmap items rather than hidden claims.

## Run deadlines

Set `deadlineMs` on a task or DAG submission to an absolute Unix epoch
timestamp in milliseconds. At expiry, active and queued steps are cancelled,
their leases are fenced, and the run is durably failed. A late worker
completion cannot resurrect it. Exact idempotent retries return the original
terminal run.
