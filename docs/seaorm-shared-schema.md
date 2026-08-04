# SQLx to SeaORM migration

Status: **migration in progress**

## Authorities

The service does not own its PostgreSQL schema or migration engine.

- SQL schema and generated Rust database adapters:
  `ORESoftware/k8s-libs-and-shared-defs` at immutable commit
  `3c84cab532b27d328378f09fba5841f02644ae3b`.
- Canonical schema:
  `pg-defs/schema/schema.sql`.
- Generated SeaORM adapter:
  `pg-defs/rust/sea-orm`.
- Declarative schema diff/review/apply:
  `declarative-migrations/declarative-migrations` version `1.4.2`.

Application startup must not run DDL. A deployment or CI workflow applies the
canonical schema with the pinned declarative-migrations binary before the
service starts.

## Target architecture

The completed service uses:

- `sea_orm::DatabaseConnection` and `ConnectOptions` for connection lifecycle;
- generated entities for ordinary table reads and writes;
- SeaORM transactions for durable job/event transitions;
- parameterized `Statement` only for PostgreSQL behavior that generated entity
  APIs cannot express cleanly;
- zero direct `sqlx`, `PgPool`, or `tokio-postgres` application dependencies;
- restart tests proving queued jobs and emitted events survive process loss;
- one repository boundary reused by server-first Maud/HTMX pages and any
  Leptos or Dioxus features.

Leptos and Dioxus are view/application features, not alternate storage
implementations. Analytics/derived-state pages may use Leptos; activity or
cross-target components may use Dioxus. Both must call the same service and
SeaORM repositories as Maud/HTMX routes.

## Migration sequence

1. Inventory every direct SQLx and tokio-postgres occurrence with
   `scripts/audit-seaorm-migration.mjs`.
2. Pin the shared schema and generated SeaORM adapter by full commit SHA.
3. Replace pool types and constructor paths with SeaORM connection lifecycle.
4. Convert durable job/event reads and mutations to generated entities.
5. Preserve atomic claim, retry, completion, and event append semantics inside
   explicit SeaORM transactions.
6. Keep advisory locks or PostgreSQL-only clauses parameterized through
   `Statement`; do not reintroduce a direct SQLx dependency for escape hatches.
7. Apply the canonical schema to PostgreSQL 17 with declarative-migrations.
8. Run restart persistence, duplicate-claim, retry, and event-ordering tests.
9. Set `seaorm-migration.json.status` to `seaorm-only`; the audit then makes any
   remaining direct SQLx or tokio-postgres occurrence a blocking failure.
10. Merge only an exact head that is current with `main`, has a generated
    lockfile, and passes the full policy and Rust suite.

## Current inventory contract

During `migration-in-progress`, the audit publishes exact file/line findings but
does not pretend the conversion is complete. Startup migration calls are always
blocking. Once the service reaches `seaorm-only`, direct driver findings become
blocking as well.

The audit excludes generated/vendor trees so generated SeaORM internals are not
misclassified as application SQLx. It also rejects symbolic links and files over
its bounded scan size.

## Validation

```sh
node --test scripts/audit-seaorm-migration.test.mjs
mkdir -p test-results
node scripts/audit-seaorm-migration.mjs \
  --report test-results/seaorm-migration-inventory.json
cargo metadata --format-version 1 --locked
```

The focused GitHub workflow additionally:

- materializes the exact shared authority over HTTPS;
- installs the pinned declarative-migrations release after SHA-256 validation;
- applies the schema to PostgreSQL 17;
- runs the repository's current Rust tests as a baseline;
- reapplies the schema to demonstrate declarative convergence;
- uploads only non-secret inventory, metadata, authority, and tool-version
  evidence.

## Completion criteria

- Direct SQLx findings: `0`.
- Direct tokio-postgres findings: `0`.
- Service startup migration calls: `0`.
- Generated SeaORM entities are pinned to the shared authority.
- Durable job and event semantics have restart and concurrency coverage.
- DPM applies and converges against PostgreSQL 17.
- No renderer-specific database access exists.
