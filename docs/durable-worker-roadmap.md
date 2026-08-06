# Durable worker runtime roadmap

Status: active

Canonical implementation: `remote/deployments/durable-worker-server-rs`

Canonical worker SDKs: `remote/worker-sdks`

Linear baseline: [DEN-1675](https://linear.app/denman/issue/DEN-1675/add-independent-durable-worker-runtime-to-k8s-cluster)

## Goal

Build an independent durable-execution and agent-worker platform for reliable background jobs, long-running agents, CI/build adapters, and service orchestration. The design borrows useful primitives from durable worker platforms, event-driven engines, and log-first service runtimes without adopting Temporal's deterministic workflow-code replay contract or coupling this cluster to another vendor's control plane.

The platform is intentionally split into independent services:

| Service | Responsibility |
| --- | --- |
| `dd-agent-worker-broker` | live ingress, admission control, backpressure, and dispatch |
| `dd-durable-worker-server` | authoritative run/task state, scheduling, leases, fencing, signals, and event history |
| `dd-build-server` | specialized repository, build, test, and CI execution |

Do not merge these responsibilities into one process merely to reduce deployment count. The separation permits independent scaling, failure containment, and replacement.

## Delivery and effect contract

Task delivery is **at least once**. A worker may receive the same logical work after a timeout, restart, lost response, or lease expiry. Therefore:

1. external effects must carry a stable idempotency key; or
2. the downstream system must reject writes from an older assignment fencing token.

An HTTP success response is not proof of exactly-once execution. The runtime rejects stale heartbeats and terminal mutations, but it cannot make an arbitrary external API transactional with its event journal.

## Landed capabilities

### Control plane — PR #714

- one-off durable tasks and DAG runs;
- dependency gates, retries, exponential backoff, deadlines, and timeouts;
- priority, queue limits, rate windows, and keyed concurrency;
- long-poll worker claims and capability matching;
- renewable leases, monotonic generations, and fencing tokens;
- cancellation, signals, approval gates, and streamed progress;
- JetStream-backed authoritative state, OpenAPI, metrics, probes, and GitOps.

### Recovery proof — PR #783

- terminates both the Rust server and JetStream process;
- recreates JetStream from the same file-backed volume;
- proves run, step, worker, idempotency, output, lease, and fencing state survive;
- proves redelivery advances attempt, lease generation, and fencing token;
- proves a stale completion receives HTTP 409 and cannot mutate the run.

### TypeScript worker SDK — PR #791

- dependency-free native ESM for Node.js 22+;
- bounded and long-lived worker modes;
- worker and step heartbeat ownership;
- handler cancellation and stale-terminal suppression after fencing;
- safe retry boundaries tied to protocol idempotency.

### Python worker SDK — PR #971 / DEN-2218

- dependency-free Python 3.11+ client and threaded worker loop;
- Python 3.11, 3.12, and 3.13 conformance matrix;
- redirect refusal, bounded responses, progress, heartbeats, fencing, and draining;
- unbound submissions and ambiguous worker polls are not retried.

### Go worker SDK and shared protocol conformance — PR #999 / DEN-2289

- dependency-free Go 1.23+ client and long-lived worker loop;
- Go 1.23 and current-stable race tests, vet, and repeated fencing stress;
- local slot admission, worker and step heartbeats, progress, panic recovery, and draining;
- confirmed lease loss and ambiguous terminal-protocol failures remain distinct outcomes;
- a versioned TypeScript, Python, and Go protocol fixture ratchets retry, progress-ID, and fencing behavior;
- TypeScript signals and ambiguous polls are no longer retried, and redirects are rejected before credentials can be forwarded;
- a push-only workflow publishes a deterministic source archive and SHA-256 checksum.

The `dev` run for merge commit `a693040ad69a1f54f14dd65fb8b74ab11fee132b` published artifact `durable-worker-go-sdk-a693040ad69a1f54f14dd65fb8b74ab11fee132b` with GitHub artifact digest `sha256:b24060664d79c845c4b7370f4cabb5b0ac79b9a09fc5b00b45b596ad9948d78c`.

## Architectural position

The runtime's differentiated model is:

- first-class one-off tasks instead of requiring workflow wrappers;
- DAG composition and streamed outputs for agent and document pipelines;
- pull-based, long-lived language-neutral workers;
- log-first authoritative state rather than replaying arbitrary application code;
- low-friction HTTP contracts plus hand-authored lifecycle SDKs;
- Kubernetes-native deployment and OpenTelemetry integration;
- explicit leases and fencing as the effect-safety boundary.

This keeps the runtime suitable for heavy workers, serverless-style bounded workers, AI pipelines, build jobs, and low-latency service effects without forcing every workload into one programming model.

## Milestone M1 — replay, projections, and operator search

Deliver:

- stable event cursor format with documented retention behavior;
- resumable SSE/stream clients using `Last-Event-ID` or an explicit cursor;
- persisted run/task projections by state, queue, task type, label, owner, and time;
- cursor pagination with deterministic ordering;
- projection rebuild from the authoritative event stream;
- corruption and missed-event detection;
- operator-only search endpoints with credentials separate from workers.

Exit gate:

- a disconnected client resumes without gaps or duplicate application;
- a projection can be deleted and deterministically rebuilt;
- search never becomes an alternate authority for run mutations.

## Milestone M2 — composition and workflow evolution

Deliver:

- schedules and calendar-triggered submissions;
- child runs with parent/child status and cancellation propagation;
- `continue-as-new` for bounded event histories;
- versioned workflow definitions and explicit compatibility policy;
- compensation/saga helpers that preserve at-least-once semantics;
- map, fan-out, and fan-in primitives with bounded concurrency;
- durable signal buffering across version changes.

Exit gate:

- a long-running workflow crosses a definition version and rolls its history forward without losing signals, deadlines, or idempotency bindings.

## Milestone M3 — SDK and executor fleet

Delivered lifecycle-aware worker SDKs:

- TypeScript;
- Python;
- Go.

Remaining worker SDKs:

- Rust;
- Dart;
- Gleam;
- Erlang and Elixir interoperability.

Also deliver:

- adapters for `dd-agent-worker-broker` and `dd-build-server`;
- Node.js, Bun, Deno, edge, Python, Go, and container examples;
- generated API clients kept separate from lifecycle-aware worker SDKs;
- one cross-language fixture corpus for retries, polling ambiguity, progress, cancellation, restart, lease loss, and fencing.

Exit gate:

- every supported SDK passes identical protocol scenarios and never sends a stale terminal mutation after losing a lease.

## Milestone M4 — multi-tenant operations and observability

Deliver:

- tenant-scoped queue, concurrency, rate, payload, and history quotas;
- tenant-aware authorization and immutable audit records;
- traces spanning submitter, scheduler, worker, and downstream effect;
- searchable logs and metrics with payload redaction;
- dead-letter inspection and controlled retry/requeue;
- an operator UI for timelines, signals, waits, attempts, and leases.

Exit gate:

- an operator can explain why a run is delayed or failed without obtaining worker credentials or exposing an unredacted tenant payload.

## Milestone M5 — HA ownership and failure certification

Deliver:

- partitioned JetStream streams and explicit snapshot/compaction policy;
- Fiducia epochs and fencing for scheduler-shard ownership;
- controlled leader migration and scheduler failover;
- chaos tests for disk pressure, duplicate delivery, worker death, controller death, NATS restart, and network partitions;
- measured recovery point and recovery time objectives;
- multi-cluster disaster-recovery procedures.

Exit gate:

- no scheduler or worker can mutate authoritative state after losing its epoch;
- failover objectives are demonstrated by repeatable destructive tests rather than documentation alone.

## Current infrastructure blocker

GitHub issue [#886](https://github.com/ORESoftware/k8s-cluster/issues/886) and Linear issue [DEN-2332](https://linear.app/denman/issue/DEN-2332/restore-github-app-credentials-for-private-deployment-contract-ci) track the missing `K8S_SUBMODULE_APP_ID` and `K8S_SUBMODULE_APP_PRIVATE_KEY` configuration required by the broad private-backend deployment-contract job.

This is a repository infrastructure blocker, not evidence that a focused durable-worker protocol, SDK, or GitOps contract failed. The fix must use a least-privilege repository-scoped GitHub App rather than a user PAT.

## Merge gates

Every worker-runtime PR must satisfy all applicable gates:

- focused unit, state-machine, and protocol tests;
- final-head CI after current `dev` is rebased or semantically merged;
- secret scan and no-PAT-propagation checks;
- no temporary archive, source carrier, self-modifying workflow, or persistent write credential in the final diff;
- generated contracts remain deterministic;
- GitOps rendering and security-context checks for deployment changes;
- Linear issue and GitHub issue/PR cross-links;
- exact-head merge protection.

A broad repository job blocked only by unavailable unrelated private submodules must be documented, but it must not be misrepresented as a successful check or used to waive a failing focused worker contract.
