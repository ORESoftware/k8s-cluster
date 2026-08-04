# Durable worker protocol

This document defines the language-neutral contract between
`dd-durable-worker-server` and heterogeneous workers. The first adapters are
expected to be the existing Node.js agent runtime, the Gleam/polyglot Lambda
runner, and future Rust workers.

The service is deliberately independent from those runtimes. It owns durable
orchestration state; workers continue to own task implementation, model SDKs,
browser automation, and provider-specific credentials.

## Execution model

A **run** contains one or more **steps**. A single task is represented as a run
with one step, so clients do not need workflow boilerplate for ordinary jobs.
A multi-step run is a validated directed acyclic graph.

Durability occurs only at explicit protocol boundaries:

- run or task submission;
- worker registration and heartbeat;
- assignment lease issuance, start, and heartbeat;
- streamed output chunks;
- completion or failure;
- retry timers and hard timeouts;
- signals, pause, resume, and cancellation.

The server does not replay a worker language stack or require deterministic
workflow code. A worker that needs resumability should emit durable output or
checkpoint state at explicit boundaries and resume from the committed input and
events supplied by its adapter.

## Authentication

All internal API routes require the canonical header:

```text
X-Worker-Auth: <shared internal credential>
```

`X-Server-Auth` is accepted as a compatibility alias. The credential is loaded
from `DURABLE_WORKER_AUTH_SECRET`, with `SERVER_AUTH_SECRET` and
`REMOTE_DEV_SERVER_SECRET` supported as migration aliases. No credential is
returned by the API or written to an event.

Public liveness, readiness, metrics, and public-documentation routes are
unauthenticated and must be protected by cluster network policy when deployed.

## Client submission

### One-off task

```http
POST /api/v1/tasks
```

A task is converted to a one-step run. Use an idempotency key whenever a caller
may retry after an ambiguous network failure.

### DAG run

```http
POST /api/v1/runs
```

Every step has a stable caller-supplied `key`, a `taskType`, a `queue`, input,
and zero or more `dependsOn` keys. The server rejects missing dependencies,
self-dependencies, duplicate keys or dependencies, cycles, invalid identifiers,
and requests above configured size limits before any durable state is created.

A submission idempotency key is bound to the canonical request digest. Reusing
the key with the same request returns the original run. Reusing it with a
different request returns an idempotency conflict.

## Worker lifecycle

### Register

```http
POST /api/v1/workers/register
```

A worker declares:

- a stable worker instance identifier;
- queues it serves;
- task types and capabilities it supports;
- maximum concurrent assignments;
- optional labels or adapter metadata.

The control plane stores a durable worker record and uses its expiry time when
matching work.

### Worker heartbeat

```http
POST /api/v1/workers/{workerId}/heartbeat
```

A heartbeat refreshes the worker record and the lane reservations associated
with its active leases. Workers should send heartbeats before their advertised
expiry and stop polling when the control plane reports the worker unavailable.

### Long poll

```http
POST /api/v1/workers/{workerId}/poll?waitMs=<duration>
```

The server chooses at most one eligible step using queue, task type,
capabilities, priority, schedule time, affinity, worker capacity, DAG
prerequisites, and keyed-concurrency constraints. An empty response includes a
retry delay. The server caps `waitMs` at `DURABLE_WORKER_POLL_MAX_WAIT_MS`.

An assignment includes a unique lease token and lease expiry. Possession of the
step identifier alone is never authority to mutate a step.

## Fenced lease lifecycle

Every lease-scoped command carries:

- `workerId`;
- `leaseToken`;
- a stable command identifier where the command may be retried.

The server verifies the worker, current lease token, lease epoch, status, and
expiry through compare-and-swap state transitions. A stale worker cannot start,
heartbeat, stream output, complete, or fail a step after its lease has expired
or been replaced.

### Start

```http
POST /api/v1/steps/{stepId}/start
```

This acknowledges the assignment and marks the step running. Retrying the same
valid command is safe.

### Lease heartbeat

```http
POST /api/v1/steps/{stepId}/heartbeat
```

This extends the assignment lease and its keyed-concurrency reservation. Lease
heartbeats do not extend the step's absolute hard timeout.

### Stream output

```http
POST /api/v1/steps/{stepId}/output
```

Each output chunk has a stable `chunkId`, stream name, sequence, and payload.
The control plane stores a receipt before publishing an event with a stable
message identifier. Retrying an identical chunk is idempotent; reusing a chunk
identifier with different content is rejected. Workers must retry an ambiguous
503 response with the same `chunkId`.

This endpoint is intended for LLM tokens, progress, tool-call records,
checkpoint metadata, and other bounded incremental output. Large binary data
belongs in object storage; the durable event should contain a content-addressed
reference.

### Complete

```http
POST /api/v1/steps/{stepId}/complete
```

Completion durably records the result, releases worker and concurrency slots,
and makes newly satisfied DAG descendants eligible. Repeating the same command
identifier returns the committed terminal mutation rather than applying it
again.

### Fail

```http
POST /api/v1/steps/{stepId}/fail
```

A retryable failure uses the step retry policy to calculate a bounded,
deterministically jittered backoff. The scheduler makes the step eligible after
the durable timer. Exhausted or non-retryable failures become terminal and
cancel non-terminal descendants according to fail-fast DAG semantics.

## Timers and recovery

The scheduler periodically scans durable state and performs idempotent recovery:

- expired leases are fenced and retried or failed;
- scheduled retry times release eligible steps;
- hard timeouts win even when worker and lease heartbeats continue;
- run status and counts are reconciled from terminal step state;
- stale worker and keyed-concurrency reservations are released.

Multiple control-plane replicas may run the scheduler. All state transitions
use compare-and-swap revisions, so a losing replica retries or observes the
winner's committed state.

## Run control

```http
POST /api/v1/runs/{runId}/signals/{signalName}
POST /api/v1/runs/{runId}/pause
POST /api/v1/runs/{runId}/resume
POST /api/v1/runs/{runId}/cancel
```

Signals are durable values that release steps waiting for the named signal.
Pause prevents new assignments while allowing already leased work to reach a
durable boundary. Resume makes eligible work assignable again. Cancel marks all
non-terminal steps cancelled and fences further worker mutation.

## Inspection and events

```http
GET /api/v1/runs/{runId}
GET /api/v1/runs/{runId}/events
```

The run endpoint returns the durable run and step snapshot. The event endpoint
is a live server-sent-events projection of the NATS subject for that run. The
JetStream event stream is the durable journal; consumers that need historical
replay should use a durable JetStream consumer rather than treating the HTTP
SSE connection as the source of truth.

Internal executable OpenAPI is available at `/internal/openapi.json` and
`/internal/docs/api` with worker authentication. `/openapi.json`,
`/api/docs.json`, `/api/docs`, and `/docs/api` expose a fail-closed public
projection containing only explicitly allowlisted public and operational routes.

## Adapter rules

An adapter for Node.js, Gleam, Rust, Go, TypeScript, Python, or another runtime
should:

1. Register a stable worker identity and declared capabilities.
2. Heartbeat the worker independently from individual step heartbeats.
3. Long-poll only after successful registration.
4. Treat lease tokens as opaque fencing credentials.
5. Use stable command and chunk identifiers across retries.
6. Stop work promptly after a stale-lease or cancellation response.
7. Put large artifacts in object storage and emit references.
8. Never place provider secrets, model credentials, or user tokens in task
   output or durable events.
9. Preserve the original task idempotency key when bridging from another queue.
10. Report retryability explicitly when failing a task.

## Rollout contract

The Kubernetes manifest is committed with `replicas: 0`. `shadowMode: true` is
telemetry, not an execution fence; therefore scaling is a deliberate operator
action after the secret, NATS durability, image/source revision, network policy,
and adapter behavior are verified. The first scale-up should remain isolated
from production submitters and workers until end-to-end lease fencing and
recovery drills pass.

### Absolute run deadline

A task or DAG submission may set `deadlineMs` to an absolute Unix epoch time in
milliseconds. A new submission must use a value later than the server time.
The deadline is part of the idempotency binding; an exact replay still returns
the original run after expiry.

Once reached, the scheduler cancels every non-terminal step, releases worker
and keyed-concurrency lanes, records the run as irreversibly failed, emits
`run.deadline_exceeded`, and increments
`dd_durable_run_deadlines_exceeded_total`. New leases and lease-scoped
mutations are rejected at or after the deadline, including late completions.
