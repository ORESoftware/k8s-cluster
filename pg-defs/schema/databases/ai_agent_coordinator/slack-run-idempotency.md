# Slack run database idempotency

Tracking issue: `DEN-1231`

## Invariant

Every durable coordinator job with:

```text
task_type = slack_agent_run
```

must use its deterministic payload run ID as the PostgreSQL idempotency key:

```text
idempotency_key = payload.run_id = ores-<24 lowercase hex>
payload.schema_version = 1
```

The existing unique constraint on `jobs.idempotency_key` then makes the run ID the database-wide duplicate boundary. Retries, Slack redelivery, process restarts, and concurrent inserts cannot create two durable jobs for the same accepted Slack run.

The constraint is intentionally conditional. Existing non-Slack queue types keep their current optional-idempotency behavior.

## Why this belongs in the schema

The command ingress and coordinator application should validate the same rule and return a bounded client error for missing or mismatched headers. The database remains the final authority because jobs can also enter through internal tools, tests, maintenance scripts, or future adapters.

The schema rejects a Slack job when any of these are true:

- `idempotency_key` is null;
- the key is not `ores-<24 lowercase hex>`;
- the payload is not a JSON object;
- `payload.schema_version` is not `1`;
- `payload.run_id` does not exactly equal the idempotency key.

The constraint does not validate the complete Slack envelope. Provider, action, context, routing, write policy, and broadcast-target validation remain application-owned in `ai-agent-coordinator.rs/src/slack_run.rs`.

## Preflight before applying to an existing database

Run the following read-only query against the target:

```sql
select
  id,
  status,
  idempotency_key,
  payload ->> 'schema_version' as schema_version,
  payload ->> 'run_id' as payload_run_id
from ai_agent_coordinator.jobs
where task_type = 'slack_agent_run'
  and (
    idempotency_key is not null
    and idempotency_key ~ '^ores-[0-9a-f]{24}$'
    and jsonb_typeof(payload) = 'object'
    and payload ->> 'schema_version' = '1'
    and payload ->> 'run_id' = idempotency_key
  ) is not true
order by created_at;
```

Also check for duplicate payload run IDs before any backfill:

```sql
select
  payload ->> 'run_id' as run_id,
  count(*) as jobs,
  array_agg(id order by created_at) as job_ids
from ai_agent_coordinator.jobs
where task_type = 'slack_agent_run'
group by payload ->> 'run_id'
having count(*) > 1;
```

Both result sets must be empty before the constraint is applied. Do not automatically rewrite ambiguous or duplicate historical jobs. Preserve evidence, identify the canonical coordinator job from Slack/bridge/run-ledger correlation, and quarantine or reconcile the others through a reviewed data-change plan.

## Declarative migration workflow

This repository is schema authority; application startup must not run DDL.

From `ai-agent-coordinator.rs`, with the shared-definitions checkout in the expected adjacent location:

```bash
export AI_AGENT_COORDINATOR_DATABASE_URL=postgres://...
export SHADOW_DATABASE_URL=postgres://.../postgres

scripts/dpm.sh diff
scripts/dpm.sh verify
scripts/dpm.sh review
scripts/dpm.sh apply
```

Review the generated SQL and lock behavior before applying. Adding the check constraint validates existing rows, so schedule the migration according to the size and write rate of `ai_agent_coordinator.jobs`.

## Focused contract test

The companion SQL test runs inside a transaction on PostgreSQL 17 and proves:

- one canonical Slack run is accepted;
- a non-Slack job may still omit an idempotency key;
- missing, mismatched, uppercase/noncanonical, wrong-version, and non-object Slack rows are rejected;
- a second job with the same canonical run ID is rejected by the unique idempotency constraint;
- rejected inserts leave no rows behind.

The focused GitHub Actions workflow applies the declarative schema to an isolated database, runs the SQL contract, inspects the installed constraint definition, and confirms no invalid Slack row survives.

## Application boundary still required

This schema guard is defense in depth. The coordinator HTTP admission path should separately require the `Idempotency-Key` header for `slack_agent_run`, compare it with `payload.run_id`, and return a `400`-class response before database access on mismatch. Normal command-ingress requests already send the deterministic run ID as the header value; direct internal callers must do the same.

## Rollback

If the constraint blocks legitimate traffic:

1. keep the Slack command deployment in dry-run mode;
2. stop new live Slack dispatch rather than weakening the constraint in place;
3. inspect the rejected payload metadata and idempotency key without logging the prompt or channel context;
4. revert the schema change through the declarative migration workflow only after identifying the contract mismatch;
5. preserve the unique idempotency key constraint and existing run IDs;
6. correct the ingress/coordinator contract, re-run the focused test, and reapply through a reviewed pull request.
