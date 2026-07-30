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
- primary and alternate business intents for each caller, including identical
  and conflicting same-key submissions;
- pre-fingerprint legacy transactions whose intent cannot be reconstructed;
- one currency and exactly two postings for a valid draft;
- explicit validation, advisory-lock acquisition, idempotency inspection,
  transaction-row staging, individual posting staging, commit, rollback/crash,
  and post-commit event publication steps;
- an optional crash after commit but before the best-effort event.

The current graph contains 892 explored states (524 unique states). A
single-threaded depth-first checker makes the CI result deterministic.

The standalone crate also compiles the production
`src/ledger/fingerprint.rs` module and executes its four canonicalization and
replay-classification vectors.

## Safety properties

The checker proves within those bounds that:

1. every committed transaction contains at least two postings and nets to zero;
2. staged transaction rows and postings remain invisible until atomic commit;
3. unbalanced drafts never enter the database transaction;
4. the modeled Postgres advisory lock has at most one owner;
5. an identical-intent replay returns the original transaction identity;
6. a different-intent replay fails closed without staging rows, claiming an
   identity, altering the committed fingerprint, or publishing an event;
7. a legacy replay fails closed because its original intent is unverifiable;
8. only the intent bound to the committed fingerprint can succeed;
9. an application-level event publication occurs only after commit and only
   from the winning caller.

The model intentionally treats event publication as best-effort. A crash after
commit may lose the event, matching the production implementation, but cannot
create another transaction or a second application publish.

## Refinement assumptions

- Postgres transaction commit is atomic: staged rows either all become visible
  or none do.
- `pg_advisory_xact_lock` serializes all callers for the same derived lock key
  until commit or rollback.
- The canonical SHA-256 fingerprint includes tenant, kind, description,
  recursively key-sorted metadata, and every posting field. Posting order and
  JSON object-key order are transport noise; duplicate postings, JSON array
  order, and exact scalar encodings remain significant.
- SHA-256 collision resistance is treated as a cryptographic assumption.
- Rows created before fingerprint deployment carry `legacy:v0`. Replaying
  those keys fails closed and requires a new key; accepting the first
  post-upgrade request would let arbitrary content claim an unverifiable
  historical intent.
- Deploy the reviewed dpm schema change before the application version that
  queries `transactions.intent_fingerprint`.
- Hash collisions may serialize unrelated keys but do not violate safety.
- The model proves safety without scheduler fairness. Its `sometimes` property
  establishes that commit is reachable, not that every balanced request must
  eventually commit under an unfair scheduler.
- “One event” counts calls made by this service. Broker redelivery semantics are
  a separate transport model.

Increasing callers, currencies, posting counts, retries per caller, or modeling
NATS delivery requires an explicit bound change and a review of the resulting
state-space growth.
