# Daedalus fabrication job-control rollout

`pg-defs/schema/schema.sql` is the source of truth for the durable Daedalus
execution ledger and transactional NATS outbox. Generated adapters are
application bindings, not migration authority. Services must never apply
this DDL at startup.

## Ownership model

- AWS RDS/PostgreSQL stores job state, attempts, retry schedule, logical
  lease, highest accepted Fiducia fencing token, and the last committed
  checkpoint.
- NATS JetStream carries durable wakeups and terminal events. A message is
  acknowledged only after the corresponding PostgreSQL transaction commits.
- Fiducia Cloud provides the coarse distributed lease. The worker persists
  its fencing token; a stale/lower token cannot checkpoint or complete work.
- `pg_try_advisory_xact_lock` serializes each short state transition. No
  transaction or advisory lock spans slow provider or fabrication work.

## Review and apply to RDS

From a reviewed checkout of this repository:

```bash
node pg-defs/src/generate.mjs --check
node pg-defs/src/diff.mjs --parse-only

export TARGET_DATABASE_URL='postgresql://...'
export SHADOW_DATABASE_URL='postgresql://...'
./pg-defs/scripts/dpm.sh diff --fail-on-diff
./pg-defs/scripts/dpm.sh review
# Human approval is required before:
./pg-defs/scripts/dpm.sh apply
```

The tables and indexes are additive. Review the DPM plan and RDS lock impact,
take the normal backup/snapshot, and apply through the established controlled
deployment workflow.

## Enable workloads

1. Run `fabrication-job-control schema-check` against the target RDS database.
2. Start one outbox dispatcher and one expired-lease reaper.
3. Enable a small worker cohort and verify enqueue, claim, checkpoint,
   completion, retry, and stale-token rejection.
4. Scale workers only after queue age, outbox lag, lease age, checkpoint age,
   retry counts, and reaper activity are visible.
5. Run a crash drill: stop a worker after a committed checkpoint; verify a
   replacement obtains a newer Fiducia token and resumes from that version.

## Rollback

Disable dispatchers, reapers, and workers first. Because the schema change is
additive, leave the tables in place while investigating; do not drop durable
recovery evidence during an incident. Re-enable only after the RDS state and
JetStream consumers have been reconciled.
