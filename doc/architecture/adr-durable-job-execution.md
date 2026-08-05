# ADR: Durable and resumable fabrication jobs

- Status: Proposed for implementation
- Date: 2026-08-05
- Owners: Daedalus Fabrication
- Scope: planning, CAD/mesh conversion, slicing/CAM, simulation, machine execution review, learning, and release-readiness jobs

## Decision

Use **PostgreSQL on AWS RDS as the canonical job state machine and recovery ledger**. Use the existing **NATS JetStream `DD_REMOTE_FABRICATION` stream as durable delivery, wakeup, and fan-out**, not as the sole record of job ownership or progress.

Every long-running worker claim uses two complementary lock layers:

1. A Fiducia Cloud lease provides a distributed coarse-grained lease and a monotonically increasing fencing token.
2. A short PostgreSQL transaction takes `pg_try_advisory_xact_lock(hashtext(tenant_id), hashtext(job_id))`, validates the persisted fencing token and job version, mutates state, and commits.

No PostgreSQL transaction or advisory lock remains open while a model, CAD tool, Meshy call, slicer, CAM tool, machine, or other external system performs slow work.

## Why this hybrid is required

JetStream is already configured for the `dd.remote.fabrication.>` subject family with file-backed storage and explicit acknowledgements. It is the right transport for load balancing, redelivery, replay, and decoupled consumers. It is not sufficient by itself for resumable multi-stage work because retention limits, redelivery counts, consumer state, or operator stream maintenance must not determine the last safe recovery point.

RDS provides atomic state transitions, idempotency constraints, checkpoint compare-and-swap, auditability, and the transactional outbox. Keeping those facts in one database transaction prevents the two common loss windows:

- a database commit succeeds but the NATS publish is lost;
- a NATS acknowledgement succeeds before the durable checkpoint commits.

The outbox closes the first window. A worker acknowledges a JetStream delivery only after its PostgreSQL claim or checkpoint commits, closing the second.

## State model

A job is in one of:

- `queued`
- `running`
- `retry_wait`
- `succeeded`
- `failed`
- `cancelled`

The durable row stores:

- tenant, request, and idempotency identities;
- current stage;
- monotonically increasing checkpoint version;
- last fully committed checkpoint JSON;
- attempt and retry state;
- logical lease owner and expiry;
- highest accepted Fiducia fencing token;
- request/result payloads and bounded error details;
- lifecycle timestamps.

A checkpoint describes completed, replay-safe work. It must never claim that an external side effect completed until that side effect is independently idempotent or has a durable provider identifier that can be reconciled.

## Enqueue and delivery protocol

1. In one RDS transaction, insert or retrieve the idempotent job row.
2. Insert a deterministic outbox event in the same transaction.
3. The outbox dispatcher claims rows with `FOR UPDATE SKIP LOCKED`.
4. It publishes to JetStream using the outbox `message_id` as `Nats-Msg-Id`.
5. It waits for the JetStream publish acknowledgement.
6. It marks the outbox row published. A crash before step 6 causes a safe duplicate publish, bounded by NATS deduplication and consumer idempotency.

The dispatcher may scale horizontally because claims expire and use `SKIP LOCKED`.

## Worker claim protocol

1. Receive a JetStream wakeup. Treat the message as a hint to inspect RDS, not as authoritative state.
2. Acquire `daedalus-fab/fabrication-job/{tenant_id}/{job_id}` in Fiducia Cloud.
3. Receive a fencing token from Fiducia.
4. Open a short RDS transaction.
5. Take the transaction-scoped advisory lock for tenant/job.
6. Claim only when:
   - state is claimable;
   - retry time has arrived;
   - attempts remain;
   - no unexpired logical lease exists;
   - the new fencing token is not lower than the persisted token.
7. Commit, then begin slow work.
8. Heartbeat Fiducia and renew the RDS logical lease. Loss of either lease makes the worker ineligible to checkpoint or complete.

A stale worker cannot write because every mutation requires the exact persisted owner and fencing token. A newly elected worker with a larger token can supersede only an expired logical lease.

## Checkpoint protocol

Each stage must define an idempotency boundary. To checkpoint:

1. Complete or reconcile the external action.
2. Open a short RDS transaction.
3. Take the transaction-scoped advisory lock.
4. Require the exact owner, fencing token, and expected checkpoint version.
5. Write the new checkpoint and increment the version.
6. Commit.
7. Acknowledge or advance the JetStream work only after the commit.

A version mismatch means another valid execution advanced the job. The caller must stop rather than overwrite newer state.

## Recovery protocol

A reaper periodically finds `running` rows whose logical lease expired. For each candidate it takes the same advisory transaction lock and conditionally:

- transitions to `retry_wait`, preserves the checkpoint, clears the lease, records `lease_expired`, and inserts a deterministic retry outbox event; or
- transitions to `failed` after the attempt budget is exhausted and inserts a terminal result outbox event.

Multiple reapers are safe: the advisory lock and conditional `UPDATE` make all but one observer skip.

## PostgreSQL advisory-lock rules

Only transaction-scoped functions are allowed:

```sql
pg_try_advisory_xact_lock(hashtext(tenant_id), hashtext(job_id))
```

Session-scoped `pg_advisory_lock` and `pg_try_advisory_lock` are prohibited. They can survive transaction boundaries on pooled connections and accidentally serialize unrelated later work.

Hash collisions are acceptable because they cause conservative extra serialization, not concurrent corruption. The tenant and job are still revalidated in every SQL `WHERE` clause.

## NATS contract

Use generated subject constants from `dd-nats-subject-defs`; do not create service-local subject literals outside the shared contract.

Initial and retry wakeups use `FABRICATION_REQUESTS_SUBJECT`. Terminal outcomes use `FABRICATION_RESULTS_SUBJECT`. Specialized workers may use the existing design, instruction, assembly, execution, learning, and release-readiness subjects, while preserving the same RDS job ID and checkpoint protocol.

The application does not create or mutate streams. `ORESoftware/k8s-libs-and-shared-defs` owns the subject/stream declaration, and `ORESoftware/k8s-cluster` owns the NATS deployment.

## Failure semantics

| Failure point | Result |
|---|---|
| Process dies before RDS enqueue commits | No job exists; caller retries with the same idempotency key |
| Process dies after enqueue commit but before publish | Unpublished outbox row is reclaimed |
| Dispatcher publishes but dies before marking published | Deterministic duplicate publish; consumers re-read RDS |
| Worker dies before claim commit | No claim; JetStream redelivery or outbox retry wakes another worker |
| Worker dies during slow work | Logical lease expires; reaper resumes from last committed checkpoint |
| Fiducia lease is lost | Heartbeat fails; stale token cannot checkpoint |
| RDS connection is lost during checkpoint | Transaction rolls back; previous checkpoint remains canonical |
| NATS is unavailable | Jobs and outbox remain durable in RDS; dispatcher retries |
| RDS is unavailable | Workers must not perform new irreversible work because ownership/checkpoints cannot be committed |

## Deployment units

Run independently:

- `fabrication-job-control dispatch-loop`
- `fabrication-job-control reap-loop`
- fabrication workers that use `ClaimedJobLease`
- the existing HTTP/server process

Recommended initial replica counts:

- dispatcher: 2 replicas;
- reaper: 2 replicas;
- worker counts by specialized subject and machine/tool capacity.

Set pod disruption budgets and anti-affinity for control-plane replicas. NATS and RDS availability remain separate concerns.

## Schema ownership

`doc/database/durable-job-control.sql` is review input only. Before enabling the workloads:

1. add the tables to `pg-defs/schema/schema.sql`;
2. regenerate all shared adapters;
3. review generated diffs;
4. deploy the additive RDS change;
5. run `fabrication-job-control schema-check`;
6. enable dispatcher and reaper;
7. run the restart drill in the recovery runbook.

The server never runs DDL on startup.
