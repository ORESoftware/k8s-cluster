# Formal-methods change procedure

Quaestor Ledger records financial facts. A posting must stay balanced, tenant-scoped, idempotent, fenced, and append-only even when requests race, responses disappear, schedulers retry, or event publication fails. This procedure defines how those claims are modeled and reviewed. It does **not** claim that the planned executable models already exist.

The checked machine inventory is [`procedure.toml`](procedure.toml).

## Change procedure

1. Identify the affected machine before changing posting, replay fingerprinting, distributed customer locks, scheduler jobs, sync commands, shard routing, or notary behavior.
2. Model the database transaction, advisory lock, Fiducia fencing authority, HTTP/NATS delivery, and best-effort events as distinct facts. An event publication is not part of ledger commit correctness unless a future outbox explicitly makes it so.
3. State safety before liveness. Safety includes zero-sum, one business intent per idempotency key, no partial visible posting set, and stale-token rejection. Liveness requires assumptions about database availability, recurring retries, lock service availability, and scheduler fairness.
4. Use finite Quint/TLC or Apalache models for concurrency and failure schedules. Replay ITF traces against production Rust with deterministic UUIDs, logical time, lock outcomes, database commits, and transport-loss injection.
5. Compare canonical ledger observations: transaction identity, intent fingerprint, complete posting multiset, per-currency net, fencing generation, scheduler state, sync checkpoint, and published-event eligibility—not row order or tracing text.
6. Run bounded deterministic checks on pull requests and wider same-key races, lock expiry, scheduler failover, and replay schedules periodically.
7. Record model hash, implementation revision, schema revision, tool versions, database isolation assumptions, bounds, and result class.

## Claim language

Use only **typechecked specification**, **randomized exploration**, **bounded exhaustive verification**, **implementation replay**, **differential replay**, or **unbounded proof**. A bounded posting model is not proof of PostgreSQL, NATS, every provider, Solana, or the entire service. Reports must identify whether deferred database constraints and transaction/advisory-lock behavior are modeled, replayed against a real database, or assumed.

## Counterexamples

Retain original and minimized traces, SQL/lock fault schedule, canonical expected and actual ledger state, model/schema/source revisions, and tool provenance. Classify model, Rust, schema, database-assumption, lock-service, scheduler, or adapter defect. Add a deterministic Rust/database regression for implementation failures and retain minimized traces under `formal/regressions/`. Never “fix” a trace by ignoring a posting, fingerprint, tenant, token, or commit boundary.

## Required review triggers

Formal review is mandatory for changes to debit/credit arithmetic, currency grouping, posting transaction boundaries, deferred constraints, idempotency keys or fingerprints, advisory lock derivation, customer-lock targets or fencing, shard derivation, event eligibility, scheduler claim/retry state, inbound sync idempotency, or notary anchoring state.

## Initial modeling order

1. **Balanced idempotent posting.** Two same-key callers, matching versus conflicting intent, transaction commit/rollback, and lost responses.
2. **Distributed customer fencing.** Multi-customer union locks, renewal/loss, stale worker, and database fencing inside the transaction.
3. **Scheduler execution.** Claim, run, retry, crash, lease expiry, and one-shot job deduplication.
4. **Sync command ingestion.** HTTP and NATS duplicate delivery leading to one logical synchronization job.
