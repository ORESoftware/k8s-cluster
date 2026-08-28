# SeaORM conversion brief (for upstream submodule repos)

Policy (2026-07-17): all Rust webservers in the fleet use **SeaORM** as the DB layer — no direct
`sqlx` dependency, no raw `tokio-postgres` — and **declarative dpm migrations**
([declarative-postgres-migrate](https://github.com/declarative-migrations/declarative-postgres-migrate.rs)):
a `schema/schema.sql` is the source of truth, services never migrate at boot, humans review/apply.
Completed in-tree: `usacc-rest-api-backend-rs`, `billing-server-rs`, `dd-music-rs`,
`dd-embeddings-rs` (see their diffs for worked examples).

The authority-layer contract merged to
`ORESoftware/k8s-libs-and-shared-defs@3c84cab532b27d328378f09fba5841f02644ae3b`.
It defines and mutation-tests the exact shared schema, generated SeaORM adapter, DPM review/apply
boundary, submodule consumer layout, sanctioned parameterized Statement cases, and verification
bar. Consuming PRs must pin an immutable shared-definitions commit rather than copying entities or
using a floating branch.

## Current rollout

| Service/repository | Current state | Next evidence |
| --- | --- | --- |
| `remote/deployments/contract-service-rs` | Active PR: direct SQLx pool and advisory query replaced by SeaORM `DatabaseConnection`, `DatabaseTransaction`, and parameterized `Statement`; optional coordination keeps lazy `OnceCell` initialization and transaction-lifetime fencing | Regenerated lockfile, exact shared gitlink, format/Clippy/check/tests, then merge |
| `ORESoftware/ai-agent-bridge.rs` | Direct SQLx isolated in `src/db.rs`; shared `ai_agent_bridge.*` entities already generated | Pin shared defs, replace pool/rows with SeaORM entities plus Statements for CTE/upsert/version semantics, remove `postgres = dep:sqlx` |
| `ORESoftware/mip-solver-node.rs` | Direct SQLx journal persists in the root/local crates; vendored pg-defs exists | Refresh vendored/shared definitions, isolate repository module, preserve durable job/event semantics, remove both SQLx dependencies |
| `fiducia-cloud/*` remaining servers | Mixed SQLx/SeaORM status across optional PostgreSQL features | Inventory by exact repo, update Fiducia interfaces/entities first, migrate in dependency order |
| `3fa-backend.rs`, `ai-agent-coordinator.rs` | Already SeaORM at the application layer | Keep direct-SQLx policy gates and shared-schema pins current |

Leptos and Dioxus remain page-level options over the same repositories and schema. They must not
introduce a second persistence implementation: Maud/HTMX operations pages, Leptos analytics, and
Dioxus activity/live pages all reuse the same SeaORM query and authorization boundary.

## The pattern

1. **Dependency** (drop `sqlx = ...` entirely):
   ```toml
   sea-orm = { version = "1", default-features = false, features = [
     "macros", "runtime-tokio-rustls", "sqlx-postgres",
     "with-chrono", "with-json", "with-uuid",  # + "with-rust_decimal" where Decimal columns exist
   ] }
   ```
2. **Entities** — decision tree:
   - Tables in the shared pg-defs contract (`remote/libs/pg-defs/schema/schema.sql`) → use the
     generated crate `dd-pg-defs-sea-orm` (`generated/rust/sea-orm`). Do not hand-copy entities.
   - Service-owned separate database → add the declarative schema under
     `remote/libs/pg-defs/schema/databases/<service>/schema.sql`; generate adapters from that
     authority. Do not create an application-owned migration runner.
3. **Pool** → `sea_orm::Database::connect` with `ConnectOptions`, preserving the existing
   max/min/timeout tuning and pinning SQL logging deliberately rather than inheriting a noisy
   default. Optional features may use a lazy `OnceCell<DatabaseConnection>` boundary when startup
   semantics require it; readiness must still prove connectivity.
4. **Queries** → entity ops where provably identical. Keep verbatim parameterized
   `sea_orm::Statement` (+ `FromQueryResult`) for: data-modifying CTEs, `FOR UPDATE SKIP LOCKED`
   claims, advisory locks, `EXCLUDED`-expression upserts, citext/vector/tsquery casts, server-clock
   `now()`/`current_date` writes, aggregates the entity API would approximate. Never interpolate
   values into SQL. `sea_orm::sqlx` re-export is the sanctioned escape for `PgListener` only.
5. **Migrations** → delete the boot-time `sqlx::migrate!` path and its env gate (config, tests,
   k8s manifest); freeze `migrations/` with a README (do not delete files); fold the final state
   into the declarative schema.
6. **DPM** → use the reviewed wrapper under `remote/libs/pg-defs/scripts/dpm.sh`. `diff` is
   non-executing; `verify` rehearses on a shadow replica and requires convergence; `apply` remains a
   human action. Destructive changes require both `--allow-destructive-sql` and
   `--allow-destructive-ops`.
7. **Verification bar** — build the unmodified baseline first, then prove: `cargo check
   --all-targets`, `cargo test --all-targets`, Clippy with warnings denied, grep-clean direct SQLx
   in Cargo.toml/src (feature strings like `sqlx-postgres` excepted), no boot migration path, and a
   DPM zero-drift/convergence result from the authority repository.
8. **Merge order** — authority schema/generated adapter first, service second, Kubernetes/submodule
   pin last. Merge current `main` into feature branches and reconcile behavior semantically; never
   rebase away or choose an entire conflict side.

## Per-repo checklist

- **fiducia.cloud** (`fiducia-messaging.rs` — inbox/outbox/transactional, note sqlx **0.9** behind
  an optional `postgres` feature; `fiducia-memory.rs`, `fiducia-operations-control-plane`,
  `fiducia-ai-agent-control-plane`, `fiducia-ai-agent-bridge.rs`, monorepo `fiducia-customer.rs`):
  fiducia has its own interfaces package — prefer generating/keeping entities there, mirroring how
  `fiducia-customer.rs`/`fiducia-admin.rs` already did it.
- **fiducia-customer.rs** (top-level repo): code is already SeaORM; delete any leftover direct
  `sqlx` dependency line and add a policy test preventing its return.
- **ai-agent-bridge**: its tables are in the shared contract (`ai_agent_bridge.*` schema) — use
  `dd-pg-defs-sea-orm` generated entities. Preserve data-modifying CTEs, JSONB metadata merge,
  `EXCLUDED` state rules, server timestamps, and optimistic context-version guards through
  parameterized SeaORM Statements where entity builders would change semantics.
- **3fa-backend**: `threefa.*` tables are in the shared contract; keep the existing SeaORM boundary
  and shared generated entities current.
- **mip-solver-node.rs** (+ `local/` crate): has a **vendored** `vendor/pg-defs` — re-vendor from
  the current pg-defs first, then convert against the vendored SeaORM crate. Preserve durable
  solve/job/event ordering and recovery behavior before removing SQLx.
- **contract-service-rs**: coordination uses a PostgreSQL transaction-scoped advisory lock, not an
  application table. Preserve it as a bound `Statement` held by an owned `DatabaseTransaction`;
  rollback remains the lock release on complete, abandon, contention, and error paths.

## pg-defs caveats

- **Fixed**: table-level composite primary keys are retained by the generated SeaORM adapter.
  Consumers must pin a commit containing the fixed generator/output.
- **Outstanding — bare-name collisions**: generated Rust module names are bare table names
  (`threefa.accounts` → `pub mod accounts`), so a second table named `accounts` in another pg
  schema cannot be added to the contract until the generator learns to disambiguate.
- **Outstanding — pgvector/tsvector**: strict-typed renderers reject `vector(N)`/`tsvector`;
  those service schemas stay under the declarative per-database tree until the generator supports
  the types.

A workflow that fails before its first step is neither a passing nor failing application test. Do
not override a merge gate with a no-step Actions result, and never solve the private-submodule
credential problem by committing or reusing the broad classic PAT pasted into chat.
