# Daily portfolio briefing delivery schema

This contract is the PostgreSQL authority for the durable execution boundary tracked by `DEN-2334` in `ORESoftware/ai-agent-coordinator.rs`.

The Rust state machine defines allowed transitions in application code. These database objects preserve the same immutable identity, fencing, compare-and-set, receipt, and scheduled-baseline facts across process restarts and concurrent coordinator replicas.

## Objects

### `daily_portfolio_delivery_fence_seq`

A schema-qualified monotonic `bigint` sequence. Every successful lease acquisition consumes a new fence. A stale worker may retain an old owner name, but it can never reproduce a newer fence.

### `daily_portfolio_delivery_runs`

One row per logical scheduled, recovery, or manual delivery run.

Immutable planning fields:

- `run_key`
- `scheduled_run_key`
- `mode`
- `source_digest`
- `plan_digest`
- `delivery_digest`
- `destination`
- `idempotency_key`

Mutable transaction fields:

- `status`
- `generation`
- `attempts`
- bounded `last_error`
- lease owner, fence, and expiry
- destination receipt identity, destination, body digest, and delivery time
- creation/update timestamps

The database enforces:

- lowercase SHA-256 digests;
- exact scheduled/recovery/manual key relationships;
- destination idempotency key equal to the logical run key;
- unique idempotency keys;
- nonnegative generations and attempts;
- all-or-nothing lease triples;
- a live lease for `delivering` rows;
- bounded errors only for `failed` and `ambiguous` rows;
- all-or-nothing confirmed receipts only for `delivered` rows;
- receipt destination/body digest equal to the immutable plan;
- no lease on a delivered row;
- nonregressing row timestamps.

The schema does not perform transitions automatically. The coordinator repository must use transactions and predicates over the expected status, generation, owner, fence, and lease expiry.

### `daily_portfolio_delivery_baseline`

A single row keyed by `scheduled`. It records the last confirmed scheduled or recovery delivery used for unchanged-item suppression:

- source logical run
- canonical scheduled key
- plan and delivery digests
- confirmed destination receipt
- delivery time

Manual runs must never update this row. The repository transaction must compare scheduled dates and reject same-date receipt/digest conflicts before updating either the run or the baseline.

## Transaction contract

A production repository should use the following pattern.

### Plan

Insert the immutable run row with `on conflict (run_key) do nothing`, then read and compare every immutable field. Identical retries are idempotent; drift is a conflict.

### Claim

In one transaction:

1. lock the run row;
2. prove no unexpired lease exists and the run is not delivered;
3. obtain `nextval('ai_agent_coordinator.daily_portfolio_delivery_fence_seq')`;
4. write owner, fence, expiry, and a new `updated_at`;
5. return the exact token.

### Begin or retry delivery

Update only where run key, owner, fence, expected generation, and an unexpired lease match. Transition `planned` or `failed` to `delivering`, increment generation and attempts exactly once, and clear the bounded error.

### Failure or ambiguity

Update only from `delivering` with the expected generation and live fence. Increment generation, record a bounded error, clear the lease, and choose `failed` or `ambiguous`.

An expired `delivering` row must become `ambiguous`; it is not directly resendable.

### Confirmed receipt

In one transaction:

1. lock and verify the run row and live fence;
2. compare receipt destination and body digest to the immutable plan;
3. reconcile the scheduled baseline when mode is scheduled or recovery;
4. transition the run to `delivered`, increment generation, store the receipt, clear the lease/error, and update timestamps;
5. commit both run and baseline together.

A retry after an uncertain response must query the destination by the same idempotency key before deciding whether to store a receipt or retry sending.

## Privacy boundary

These tables do not store prompt bodies, inbox or Slack message bodies, source URLs, model output, credentials, authorization headers, or arbitrary destination responses.

Identifiers and errors are bounded. Digests and provider receipt IDs are retained only to prove exact logical and destination outcomes.

## Validation

```bash
createdb daily_portfolio_delivery_contract
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 \
  -f pg-defs/schema/databases/ai_agent_coordinator/schema.sql
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 \
  -f pg-defs/schema/databases/ai_agent_coordinator/daily-portfolio-delivery.test.sql
```

The dedicated GitHub Actions workflow runs the schema and accepted/rejected contract suite on PostgreSQL 17, inspects exact constraints and indexes, proves the fence sequence is monotonic, and retains a bounded summary.

Application-level concurrent-connection and restart tests remain in the coordinator repository because they must exercise the actual repository adapter rather than SQL constraints alone.
