# Rust server SQLx → SeaORM migration contract

Status: **fleet policy and implementation checklist**

The canonical machine-readable policy is
[`../rust-server-contract.json`](../rust-server-contract.json). This document
explains how to apply it without moving schema ownership into individual
services.

## Ownership model

| Concern | Owner |
| --- | --- |
| Shared PostgreSQL schema | `ORESoftware/k8s-libs-and-shared-defs/pg-defs/schema/schema.sql` |
| Service-specific database schema | `pg-defs/schema/databases/<service>/schema.sql` in this repository |
| Generated SeaORM entities for shared tables | `pg-defs/generated/rust/sea-orm` |
| Schema diff, convergence proof, and reviewed apply | `declarative-migrations/declarative-postgres-migrate.rs` (`dpm`) |
| Application queries and transactions | The consuming Rust service, through SeaORM |
| Kubernetes/submodule pin | `ORESoftware/k8s-cluster` after the canonical repositories merge |

Generated entities are adapters only. They do not own migrations. A service
must not infer a migration from an entity diff or run an ORM migrator at boot.

## Consumer layout

A standalone server should pin this repository as a Git submodule:

```text
vendor/k8s-libs-and-shared-defs
```

and use the generated crate through a local path dependency:

```toml
dd-pg-defs-sea-orm = {
  path = "vendor/k8s-libs-and-shared-defs/pg-defs/generated/rust/sea-orm"
}

sea-orm = {
  version = "1",
  default-features = false,
  features = [
    "macros",
    "runtime-tokio-rustls",
    "sqlx-postgres",
    "with-chrono",
    "with-json",
    "with-uuid",
  ]
}
```

The gitlink must point to a reviewed immutable commit. Do not use a floating
branch dependency in a production server and do not copy generated entities by
hand.

## Migration sequence

1. **Freeze a baseline.** Record the exact server commit and run its current
   check/test suite before changing persistence.
2. **Confirm schema authority.** Shared tables must exist in
   `pg-defs/schema/schema.sql`. A service-owned database belongs under
   `pg-defs/schema/databases/<service>/schema.sql`.
3. **Pin the shared definitions.** Add or refresh the submodule and verify
   `node pg-defs/src/generate.mjs --check` in the authority repository.
4. **Replace the connection.** Use `sea_orm::Database::connect` with
   `ConnectOptions`, preserving max/min connections, connect/acquire timeouts,
   and SQL logging policy.
5. **Replace ordinary queries.** Prefer generated entities, `Entity::find`,
   `ActiveModel`, `QueryFilter`, `QueryOrder`, and transactions.
6. **Preserve exact PostgreSQL semantics.** Use parameterized
   `sea_orm::Statement` plus `FromQueryResult` when the entity API would alter
   semantics, including data-modifying CTEs, `FOR UPDATE SKIP LOCKED`, advisory
   locks, `EXCLUDED` expressions, Postgres casts/operators, and server-clock
   writes. Never interpolate values into SQL.
7. **Remove direct SQLx.** Delete the direct dependency and every application
   `sqlx::query`, `sqlx::query_as`, `PgPool`, and `sqlx::migrate!` call. The
   `sqlx-postgres` SeaORM feature string is expected and is not a direct SQLx
   dependency.
8. **Remove boot migrations.** Delete startup migration flags and code. Preserve
   old imperative migration files only as historical evidence with a README
   pointing to the declarative source of truth.
9. **Prove schema convergence.** Use `dpm diff` for review and `dpm verify` on a
   shadow replica. `dpm apply` remains an explicit human action.
10. **Run the full verification bar.** Format, Clippy with warnings denied,
    all-target checks/tests, database integration/restart tests, and a grep-clean
    direct-SQLx policy check.
11. **Merge in dependency order.** Shared schema/adapter first, server second,
    then update the `k8s-cluster` submodule and deployment pin. Resolve conflicts
    semantically rather than choosing an entire side.

## DPM safety boundary

The reviewed wrapper is [`../scripts/dpm.sh`](../scripts/dpm.sh):

```sh
scripts/dpm.sh diff
scripts/dpm.sh verify
scripts/dpm.sh review
scripts/dpm.sh apply
```

`diff` never executes SQL. `verify` rehearses against a shadow replica and
requires an empty re-diff. `apply` requires human confirmation. Destructive SQL
requires two separate consents:

```text
--allow-destructive-sql
--allow-destructive-ops
```

Do not add either flag to unattended application startup or ordinary deployment
manifests.

## UI allocation for Rust servers

Persistence and rendering are separate decisions:

- **Maud + Axum + SeaORM + HTMX** remains the default for server-first
  operations, administration, settings, and CRUD.
- **Leptos** is a candidate for analytics, filters, drill-downs, and coordinated
  reactive state.
- **Dioxus** is a candidate for activity/live surfaces and components intended
  to align with desktop or mobile targets.

Adding a Leptos or Dioxus page must reuse the same SeaORM repositories,
authentication, owner scope, and declarative schema. It must not create a second
SQLx persistence layer.

## Required evidence in each server PR

A migration PR should state:

- exact shared-definitions gitlink commit;
- exact schema and generated-crate paths;
- every removed direct SQLx dependency/call site;
- every retained `Statement` escape hatch and why it preserves PostgreSQL
  semantics;
- confirmation that no migration runs at service startup;
- DPM diff/verify result or an honest explanation that database-bound evidence
  is unavailable;
- format, Clippy, check, unit, integration, and restart-test results;
- Kubernetes pin/update that remains after the server PR merges.

A workflow that fails before its first step is neither passing nor failing
application code and must not be used to override the merge gate.
