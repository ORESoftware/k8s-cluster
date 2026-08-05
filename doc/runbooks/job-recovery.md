# Durable fabrication job recovery runbook

This runbook covers stalled jobs, worker crashes, NATS outages, Fiducia lease loss, and safe replay from the last committed PostgreSQL checkpoint.

## Safety invariants

Before intervening, preserve these invariants:

1. RDS is the canonical job and checkpoint store.
2. NATS messages are wakeups; never reconstruct authoritative progress only from a message body.
3. A worker must own both a healthy Fiducia lease and the matching persisted fencing token.
4. All job mutations use a short PostgreSQL transaction and `pg_try_advisory_xact_lock`.
5. Never keep a database transaction open while waiting on a machine, model, Meshy, CAD/CAM/slicer process, object store, or human review.
6. Never manually lower or reuse a fencing token.
7. Never mark a stage complete unless its external side effect is idempotent or reconciled by a durable provider ID.

## Required environment

The control binary reads:

```text
FABRICATION_DATABASE_URL
NATS_URL
NATS_REQUIRE_TLS
NATS_CREDENTIALS_FILE or NATS_TOKEN or NATS_NKEY
FIDUCIA_BASE_URL
FIDUCIA_AUTH_TOKEN
FIDUCIA_TLS_CA_PEM or FIDUCIA_TLS_CA_PATH
```

Use workload identity or Kubernetes secrets. Do not place credentials in repository files, command history, issue bodies, Linear, or logs.

## Pre-deployment checks

Run against the target RDS database:

```bash
cargo run --bin fabrication-job-control -- schema-check
```

Verify the generated NATS definitions still map fabrication work under the shared stream:

```bash
nats stream info DD_REMOTE_FABRICATION
```

Expected properties include file storage, the `dd.remote.fabrication.>` subject family, and explicit acknowledgement consumers.

Start one-shot dry operational checks before enabling loops:

```bash
cargo run --bin fabrication-job-control -- dispatch-once --limit 10
cargo run --bin fabrication-job-control -- reap-once --limit 10
```

Both commands are safe to run concurrently across replicas. Outbox claims expire, and per-job recovery uses transaction-scoped advisory locks.

## Enqueue an idempotent test job

```bash
cargo run --bin fabrication-job-control -- enqueue \
  --tenant recovery-drill \
  --request-id drill-2026-08-05 \
  --idempotency-key drill-2026-08-05 \
  --kind design_conversion \
  --payload '{"source":"recovery-drill","dryRun":true}' \
  --max-attempts 3
```

Record the returned `jobId`. Re-running the command with the same tenant and idempotency key returns the existing job rather than creating duplicate work.

Inspect it:

```bash
cargo run --bin fabrication-job-control -- show \
  --tenant recovery-drill \
  --job-id <job-id>
```

## Worker restart drill

Use a non-production tenant and a dry-run worker/tool adapter.

1. Start the dispatcher loop.
2. Start a worker that claims the test job through `ClaimedJobLease`.
3. Confirm `state=running`, a non-null lease owner/expiry, and a persisted fencing token.
4. Let the worker commit at least one checkpoint.
5. Terminate the worker process without releasing its lease.
6. Do not alter the row manually.
7. Wait until both the Fiducia lease and RDS logical lease expire.
8. Run the reaper once.
9. Confirm:
   - state moved to `retry_wait`;
   - the checkpoint payload was preserved;
   - checkpoint version increased;
   - lease owner/expiry were cleared;
   - `last_error_code=lease_expired`;
   - a deterministic unpublished retry outbox row exists.
10. Run the dispatcher.
11. Start a replacement worker.
12. Confirm the new fencing token is greater than the old token.
13. Confirm execution resumes from the committed checkpoint, not from an in-memory step.

The helper command can exercise lease acquisition, heartbeat, checkpoint compare-and-swap, and optional completion:

```bash
cargo run --bin fabrication-job-control -- lease-drill \
  --tenant recovery-drill \
  --job-id <job-id> \
  --seconds 45
```

Add `--complete` only when the test should end in `succeeded`.

## Diagnose a stalled job

Inspect the canonical row:

```sql
SELECT
    job_id,
    tenant_id,
    request_id,
    kind,
    state,
    current_stage,
    checkpoint_version,
    attempt_count,
    max_attempts,
    lease_owner,
    lease_expires_at,
    fiducia_fencing_token,
    next_attempt_at,
    last_error_code,
    last_error_message,
    updated_at
FROM daedalus.fabrication_job_executions
WHERE tenant_id = $1
  AND job_id = $2::uuid;
```

Then inspect pending/published wakeups:

```sql
SELECT
    event_id,
    event_type,
    subject,
    message_id,
    available_at,
    publish_attempts,
    claim_owner,
    claim_expires_at,
    published_at,
    last_error,
    created_at
FROM daedalus.fabrication_job_outbox
WHERE job_id = $1::uuid
ORDER BY created_at, event_id;
```

Interpretation:

- `queued` with no outbox row: enqueue transaction/procedure is incomplete or a nonconforming producer bypassed it.
- unpublished outbox with future `available_at`: retry backoff is active.
- unpublished outbox with expired claim: dispatcher can reclaim it.
- published outbox and `queued`: verify the JetStream stream/consumer and worker subscription.
- `running` with future lease expiry: worker may still own the job; do not force recovery.
- `running` with expired lease: run the reaper.
- `retry_wait` with future `next_attempt_at`: normal retry backoff.
- terminal state: never requeue by editing state; create a new idempotency key or an explicit audited replay workflow.

## NATS outage

Symptoms include dispatcher publish errors and increasing unpublished outbox rows.

1. Keep accepting work only if RDS is healthy and outbox growth is within capacity.
2. Do not mark outbox rows published manually.
3. Inspect NATS cluster quorum, JetStream storage, account limits, and authorization.
4. Restore NATS.
5. Run or scale the dispatcher. It will reclaim expired outbox claims.
6. Watch publish error rate and outbox age until drained.

Useful database query:

```sql
SELECT
    count(*) AS pending,
    min(available_at) AS oldest_available,
    max(publish_attempts) AS max_attempts
FROM daedalus.fabrication_job_outbox
WHERE published_at IS NULL;
```

## RDS outage

Workers must stop before beginning new irreversible external actions because they cannot atomically claim or checkpoint.

1. Keep Fiducia leases short; do not treat a healthy Fiducia lease as sufficient by itself.
2. Stop or pause workers whose database health check fails.
3. Restore RDS connectivity.
4. Allow logical leases to expire.
5. Run the reaper.
6. Resume workers from the last committed checkpoint.
7. Reconcile provider/machine IDs for any side effect that may have completed while its checkpoint transaction failed.

## Fiducia outage or lease loss

A worker that cannot heartbeat Fiducia is stale even if its local process and RDS connection remain healthy.

1. Stop initiating new external work.
2. Attempt no checkpoint after the lease is known unhealthy.
3. Let the RDS logical lease expire.
4. Restore Fiducia.
5. Run the reaper and allow a newly fenced worker to resume.
6. Confirm the replacement token is greater than the stored token.

Never overwrite `fiducia_fencing_token` by hand.

## Poison or repeatedly failing jobs

When `attempt_count` reaches `max_attempts`, the job transitions to `failed` and emits a terminal outbox event.

Review:

- bounded error code/message;
- current stage and checkpoint;
- input validity;
- external provider/machine evidence;
- whether the stage is actually idempotent;
- whether retry classification is correct.

Fix the underlying issue, then create an explicit audited replay with a new idempotency key. Do not reset attempts or state in place.

## Outbox claim held by a dead dispatcher

An outbox row is reclaimable after `claim_expires_at`. No manual action is normally required.

Emergency inspection:

```sql
SELECT event_id, claim_owner, claim_expires_at, publish_attempts, last_error
FROM daedalus.fabrication_job_outbox
WHERE published_at IS NULL
  AND claim_owner IS NOT NULL
ORDER BY claim_expires_at;
```

Do not clear a non-expired claim unless the dispatcher identity is conclusively dead and the incident is documented.

## Advisory-lock inspection

The implementation uses transaction-scoped advisory locks, so locks disappear when the short transaction commits or rolls back.

During a live claim/checkpoint transaction:

```sql
SELECT
    pid,
    locktype,
    classid,
    objid,
    granted,
    application_name,
    state,
    query
FROM pg_locks
JOIN pg_stat_activity USING (pid)
WHERE locktype = 'advisory';
```

A long-lived advisory lock indicates a defect. The application must never use session-scoped `pg_advisory_lock` or hold a transaction while performing fabrication work.

## Metrics and alerts

Alert on:

- oldest unpublished outbox age;
- outbox publish failure rate;
- expired running leases;
- jobs in `retry_wait` beyond `next_attempt_at`;
- attempt exhaustion rate by kind/stage;
- Fiducia heartbeat failures;
- stale-fencing rejection count;
- checkpoint compare-and-swap rejection count;
- RDS transaction latency and pool saturation;
- JetStream consumer pending/redelivery counts.

Every log/trace should carry `job_id`, `tenant_id`, `request_id`, `kind`, `stage`, `attempt_count`, `checkpoint_version`, `lease_owner`, and fencing token. Never log raw credentials or unredacted sensitive job payloads.

## Rollback

The feature is additive.

1. Scale dispatcher, reaper, and new workers to zero.
2. Leave the RDS tables in place for audit and later restart.
3. Existing server behavior continues independently.
4. Do not drop tables during an incident.
5. Re-enable only after schema, NATS, Fiducia, and worker health checks pass.

## Incident evidence

Attach to the Linear incident and GitHub issue/PR:

- job ID and tenant (redacted if required);
- timestamps in UTC;
- checkpoint version before and after recovery;
- old/new fencing tokens;
- attempt counts;
- outbox message IDs;
- JetStream stream/consumer observations;
- RDS query evidence;
- remediation and regression test.

Do not attach credentials, full customer payloads, private model inputs, machine secrets, or raw authentication headers.
