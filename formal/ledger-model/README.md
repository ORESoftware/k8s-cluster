# Ledger posting model

This crate is the executable specification for
`LedgerService::post_transaction`. It uses Stateright 0.31.0 to exhaustively
explore a finite abstraction of same-key concurrency and database failure
boundaries without compiling the server's superproject-only path dependencies.

Run it with:

```sh
cargo test --manifest-path formal/ledger-model/Cargo.toml --locked -- --nocapture
```

The checked model has:

- two callers using one tenant and one idempotency key;
- all four balanced/unbalanced draft combinations;
- one currency and exactly two postings for a valid draft;
- explicit validation, advisory-lock acquisition, idempotency inspection,
  transaction-row staging, individual posting staging, commit, rollback/crash,
  and post-commit event publication steps;
- an optional crash after commit but before the best-effort event.

The current graph contains 188 explored states (107 unique states). A
single-threaded depth-first checker makes the CI result deterministic.

## Safety properties

The checker proves within those bounds that:

1. every committed transaction contains at least two postings and nets to zero;
2. staged transaction rows and postings remain invisible until atomic commit;
3. unbalanced drafts never enter the database transaction;
4. the modeled Postgres advisory lock has at most one owner;
5. a replay returns the original transaction identity;
6. an application-level event publication occurs only after commit and only
   from the winning caller.

The model intentionally treats event publication as best-effort. A crash after
commit may lose the event, matching the production implementation, but cannot
create another transaction or a second application publish.

## Refinement assumptions

- Postgres transaction commit is atomic: staged rows either all become visible
  or none do.
- `pg_advisory_xact_lock` serializes all callers for the same derived lock key
  until commit or rollback.
- An idempotency-key replay denotes the same logical business intent. The
  production schema does not currently persist a request fingerprint, so
  rejecting the same key with different balanced content is outside this
  model. If callers cannot guarantee that contract, add a durable fingerprint
  and model its conflict result.
- Hash collisions may serialize unrelated keys but do not violate safety.
- The model proves safety without scheduler fairness. Its `sometimes` property
  establishes that commit is reachable, not that every balanced request must
  eventually commit under an unfair scheduler.
- “One event” counts calls made by this service. Broker redelivery semantics are
  a separate transport model.

Increasing callers, currencies, posting counts, retries per caller, or modeling
NATS delivery requires an explicit bound change and a review of the resulting
state-space growth.
