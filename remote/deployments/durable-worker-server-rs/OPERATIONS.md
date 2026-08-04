# Durable worker operations

This runbook covers the shadow rollout and recovery procedures for
`dd-durable-worker-server`. The service is an orchestration control plane; it
does not replace the Node.js thread worker, Gleam Lambda runner, or existing
JetStream consumers.

## Rollout gates

The checked-in Kubernetes Deployment intentionally starts with `replicas: 0`.
The initial `:dev` image reference is also a rollout placeholder: replace it
with the `sha256` digest produced by a successful `dev` or `main` image publish
before scaling the Deployment above zero.

Before scaling it above zero, verify all of the following:

1. The image reference is immutable (`@sha256:...`) and its build completed the
   Rust, protocol, manifest, end-to-end JetStream, SBOM, and provenance gates.
2. The `DD_DURABLE_WORKER_STATE` KV bucket and
   `DD_DURABLE_WORKER_EVENTS` stream can be created with the configured NATS
   account.
3. `DURABLE_WORKER_AUTH_SECRET` is present through `dd-agent-secrets` and is
   different from public API credentials.
4. `/readyz` returns success and `/metrics` is scraped under the
   `dd-durable-worker-server` application label.
5. Only synthetic queues are enabled; no existing Node.js or Gleam submitter is
   routed to the service yet.
6. At least two synthetic workers have completed lease-expiry, retry,
   cancellation, signal, concurrency-lane, and stale-fencing tests.

Scale to one replica first. Scale to multiple replicas only after the same tests
pass while requests are distributed across replicas.

## Health and observability

- `GET /healthz` confirms the HTTP process is alive.
- `GET /readyz` confirms the state store is reachable.
- `GET /metrics` exposes submission, lease, timeout, retry, completion,
  journaling, and scheduler counters.
- Structured JSON logs include request IDs through the HTTP middleware.
- The cluster resource exporter watches the
  `dd-durable-worker-server` application label even while the Deployment is
  scaled to zero.

Alert on sustained growth in:

- `dd_durable_scheduler_failures_total`;
- `dd_durable_journal_failures_total`;
- `dd_durable_lease_expirations_total` without corresponding retry or terminal
  events; and
- worker registrations whose heartbeats expire repeatedly.

A lifecycle event is an operational journal after authoritative state is
committed. Do not repair state by replaying lifecycle events blindly. Output
chunk acknowledgements are stricter and must be deduplicated by `eventId` or
`chunkId`.

## Safe rollback

1. Stop new submissions at the caller or compatibility adapter.
2. Put workers into draining mode so they stop accepting new assignments.
3. Allow active leases to complete until the operational timeout expires.
4. Scale the Deployment to zero.
5. Leave JetStream KV and stream data intact for inspection and resumption.

Scaling the control plane to zero does not change the pre-existing Node.js,
Gleam, or queue-consumer paths.

## Lease and timeout recovery

Workers must heartbeat before `leaseExpiresAtMs`. A worker that loses its lease
must stop side effects and must not submit completion with an old fencing token.
The scheduler will either queue a retry or mark the step terminal according to
the retry policy.

A heartbeat extends the lease and capacity/concurrency holders but never the
hard execution timeout. For long AI-agent tasks, workers should divide work into
explicit durable steps rather than relying on unlimited heartbeats.

When investigating a stuck run:

1. Read the run snapshot and identify nonterminal steps.
2. Compare each active step's worker, lease generation, fencing token, lease
   expiry, and hard timeout.
3. Confirm the worker registration is online and its heartbeat is current.
4. Inspect the corresponding `dd.durable.run.<run-id>.events` subject.
5. Run one scheduler reconciliation cycle or restart a control-plane replica;
   reconciliation is designed to be repeatable.

Do not edit KV values manually unless the exact revision and invariant changes
are understood. CAS revisions are part of the concurrency guarantee.

## NATS protection

Production NATS should use TLS and account-scoped credentials. Raise
`DURABLE_WORKER_NATS_REPLICAS` only after the JetStream cluster has enough
members to satisfy that replication factor. Monitor storage usage and configure
retention separately for authoritative KV state and append-only events.

The service currently discovers scheduler candidates by scanning KV keys. Keep
the shadow workload bounded until indexed ready queues and tenant sharding are
implemented.

## Existing-runtime adapter

A future compatibility adapter may translate a durable assignment into the
existing `dd.remote.thread.<thread>.tasks` envelope. Keep that adapter outside
this state machine. It must preserve:

- durable `runId`, `stepId`, attempt, lease generation, and fencing token;
- completion and output idempotency;
- cancellation and draining semantics; and
- the original thread-affinity key when one is required.

Do not switch a production queue until adapter failure and redelivery tests show
that duplicate delivery cannot duplicate external side effects.
