# DEN-830 email-attention persistence contract

The `ai_agent_coordinator` email-attention tables in `pg-defs/schema/schema.sql` are an externally managed PostgreSQL desired-state contract. Application processes must not create, alter, or repair these tables during startup.

## Privacy boundary

Durable item and cursor records store opaque provider identifiers, fingerprints, scheduling metadata, aggregate counts, and bounded diagnostics. They intentionally exclude sender addresses, mailbox addresses, subjects, snippets, message bodies, and attachment contents.

A user-visible digest may exist only while a notification is pending in the transactional outbox. After confirmed delivery, the digest is replaced by a redacted tombstone while the exact delivery fingerprint remains available for idempotency and audit.

## Coordination boundary

The scheduler lease uses compare-and-swap semantics. Notification production and item state changes must be committed transactionally with the outbox row so retries cannot silently lose or duplicate user-visible attention items.

## Validation

The repository CI proves that the complete declarative schema applies to a fresh PostgreSQL 17 database, converges without drift under `dpm verify`, and remains synchronized with every generated language binding. The NATS Rust binding regenerated in the same pull request is an independent schema-owned drift correction discovered by that full validation gate.
