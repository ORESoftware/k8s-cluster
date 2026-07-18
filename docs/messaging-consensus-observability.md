# Messaging, consensus, and observability contract

This is the cross-repository reliability contract for Fiducia. It separates
three concerns that must remain independently operable:

1. **Raft decides durable authority.** `fiducia-node` owns customer coordination
   state and fencing tokens; `fiducia-brain` owns placement and scale intent.
2. **NATS delivers events and optional work.** JetStream carries durable event
   streams and Core NATS carries disposable progress or request/reply dispatch.
3. **Telemetry explains both planes.** Structured logs, traces, and Prometheus
   counters report degradation without becoming part of either correctness path.

## Dependency boundary

`fiducia-node`, `fiducia-brain`, `fiducia-routing`, and
`fiducia-load-balance` must not depend on NATS. Their consensus peer traffic uses
the dedicated authenticated HTTP peer planes (`:9090` for node Raft and `:9095`
for brain Raft), with durable local stores and quorum membership configured from
the deployment topology. The monorepo contract tests reject a future NATS crate
dependency or `async_nats` source import in those repositories.

This is more than layering: if NATS is unavailable, leaders must still be
elected, fencing tokens must still advance, linearizable reads/writes must still
commit with a Raft majority, and the brain must still reconcile placement. NATS
must never carry votes, heartbeats, log replication, leader hints, membership,
fencing-token validation, or the only copy of a state transition.

## What NATS may do

| Use | Transport | Required behavior during outage |
|---|---|---|
| Workflow and agent lifecycle events | JetStream with `Nats-Msg-Id` dedup | State transition remains authoritative in its service store/node; publish failure is logged and counted for retry/operator action. |
| Agent live progress | Core NATS | May be dropped; task history and local streams remain available. |
| Lambda warm-pool request/reply | Core NATS | May fall back to a governed local worker when policy allows; node leases/idempotency remain authoritative. |
| Transactional domain events | Postgres outbox to JetStream | Domain transaction commits with an outbox row; relay retries until acknowledged. |

All new producers use `fiducia-messaging::MessageEnvelope`, set a stable source,
and provide an idempotency key. JetStream publication uses the tenant-scoped
digest as `Nats-Msg-Id`. A Core fallback is a live-delivery degradation, not a
substitute for an outbox where a durable event is required. Consumers that cause
an external effect must call the envelope consume gate, verify the fencing token
with `fiducia-node`, and persist an inbox/idempotency decision before acting.

Initial broker connection failure is not permanent: clients retry on a bounded
cadence without logging credential-bearing URLs. Publish, fallback,
serialization, and reconnect failures must be structured logs and counters; a
discarded `Result` is not acceptable on a delivery boundary.

## Telemetry contract

Rust services initialize `fiducia-telemetry` once, before constructing network
clients. It always emits structured stdout logs and optionally exports OTLP
traces to the local collector. Services with Prometheus endpoints retain them;
message clients add counters for connection attempts/failures, acknowledged
JetStream events, Core fallbacks, and final failures.

Telemetry is fail-soft: collector or exporter failure cannot stop Raft, request
handling, or recovery. Fail-soft does not mean silent. Errors are logged without
credentials, counters continue locally, and the service flushes the tracer
provider during graceful shutdown.

Every log on a message or consensus path should include the stable dimensions
available at that layer: service, cluster, instance, subject or shard, message
type, correlation id, and outcome. Never include bearer tokens, NATS URLs with
userinfo, customer payloads, prompts, cookies, or raw secret-bearing errors.

## Verification ladder

- Unit tests cover Raft election, quorum loss, snapshot restore, envelope
  validation/dedup, and failure counters.
- `fiducia-e2e` process composition boots three real node binaries behind the
  real load balancer and tests routing, races, leader loss, compaction, snapshot
  rejoin, and fencing continuity.
- The Tier-2 Kind suite boots three independent control planes with real node
  and brain Raft members. It checks one leader per group, replica agreement,
  cross-LB reads, NATS-free consensus pod specs, minority refusal, and heal.
- Production chaos is separately gated and must never be inferred from a local
  single-cluster success.

## Review checklist

- Is the durable decision persisted before its event is published?
- Can the operation remain correct when NATS and telemetry are both down?
- Does every external mutation carry a current fencing token and idempotency key?
- Does a broker failure reconnect, log, and increment a counter?
- Does a follower fail closed or return a bounded leader hint instead of acting?
- Do README runbooks name the real acceptance test and its destructive gates?
