# Formal-methods change procedure

The fabrication service plans and records jobs for physical machines, consumes retrying NATS work, holds distributed leases, persists artifacts, and feeds learned outcomes back into planning. Duplicate execution, stale authority, divergent replicas, or unreadable persisted state can produce unsafe or irreproducible instructions. This procedure defines how those transition systems are modeled and reviewed; it does **not** claim the planned models already exist.

The checked machine inventory is [`procedure.toml`](procedure.toml).

## Change procedure

1. Identify the affected machine before changing request/job identity, NATS handling, lease acquisition/renewal, artifact persistence, learning aggregation, retention, planner policy, or replica configuration.
2. Model delivery, authority, durable state, artifact creation, and physical side effects separately. A duplicate NATS delivery may replay a durable job identity, but a stale or unfenced worker must not perform a second physical effect.
3. Distinguish single-replica `NoopCoordination` from distributed Fiducia authority. No formal result may treat local monotonically increasing tokens as globally comparable.
4. State safety properties before liveness. Liveness requires assumptions about recurring delivery/reconciliation, shared Postgres availability, advancing lease time, healthy Fiducia, and an eventually responsive machine/worker.
5. Use finite Quint/TLC or Apalache models for delivery, lease, persistence, and retention schedules. Replay ITF traces against production Rust with deterministic IDs/time, mock Postgres statements, injected coordination results, and explicit crash points.
6. Compare canonical observations: request/job ID, current lease holder/token/expiry, job/artifact state, displaced-upsert outcome, persisted payload validity, learning-policy snapshot, and retention result—not log order or in-memory collection layout.
7. Record model/source/schema revisions, coordination and persistence modes, bounds, assumptions, tool versions, and result class.

## Claim language

Use only **typechecked specification**, **randomized exploration**, **bounded exhaustive verification**, **implementation replay**, **differential replay**, or **unbounded proof**. Results must state whether coordination is `Noop` or Fiducia and persistence is disabled/in-memory or PostgreSQL. A single-process test cannot establish cross-pod exclusion; a mock database test cannot prove every PostgreSQL schedule; a planner model cannot prove real machine physics.

## Counterexamples

Retain original and minimized delivery/lease/store traces, request and job identities, expected/actual canonical state, coordination/persistence mode, schema/source/model revisions, and tool provenance. Classify model, coordination, store, planner, transport, schema, or assumption defect and add a deterministic Rust regression. Keep minimized artifacts under `formal/regressions/`. Never make a divergence disappear by ignoring a fencing token, artifact, displaced upsert, unreadable payload, or learning outcome.

## Required review triggers

Formal review is mandatory for deterministic job-ID derivation, NATS acknowledgment/redelivery, lease holder identity, acquire/renew/release or fencing behavior, replica count assumptions, JobStore/LearningStore semantics, Postgres upsert, payload sanitization, retention sweep ordering/bounds, artifact release state, learning-policy aggregation, or planner decisions affected by persisted outcomes.

## Initial modeling order

1. **Job deduplication and redelivery.** Same request ID, different request content, upsert displacement, crash, and NATS retry.
2. **Distributed work authority.** Acquire, renew, lease loss, stale worker, release failure, and Noop-versus-Fiducia deployment assumptions.
3. **Shared learning state.** Concurrent outcomes, finite-value sanitization, replica reads, aggregation, and planner-policy update.
4. **Retention convergence.** Concurrent bounded sweeps, new writes during sweep, stable newest-N ordering, and eventual convergence rather than a false hard-cap claim.
