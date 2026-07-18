# SeaORM conversion brief (for upstream submodule repos)

Policy (2026-07-17): all Rust webservers in the fleet use **SeaORM** as the DB layer — no direct
`sqlx` dependency, no raw `tokio-postgres` — and **declarative dpm migrations**
([declarative-postgres-migrate](https://github.com/declarative-migrations/declarative-postgres-migrate.rs)):
a `schema/schema.sql` is the source of truth, services never migrate at boot, humans review/apply.
Completed in-tree: `usacc-rest-api-backend-rs`, `billing-server-rs`, `dd-music-rs`,
`dd-embeddings-rs` (see their diffs for worked examples). This brief is the prompt/checklist for the
remaining upstream repos.

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
   - Service-owned separate database → hand-written `src/entity/` + service-local
     `schema/schema.sql` + `scripts/dpm.sh` (copy the billing-server-rs / dd-embeddings-rs setup).
3. **Pool** → `sea_orm::Database::connect` with `ConnectOptions`, preserving the existing
   max/min/timeout tuning and pinning `sqlx_logging_level` to DEBUG (sea-orm defaults to INFO).
4. **Queries** → entity ops where provably identical. Keep verbatim parameterized
   `sea_orm::Statement` (+ `FromQueryResult`) for: data-modifying CTEs, `FOR UPDATE SKIP LOCKED`
   claims, advisory locks, `EXCLUDED`-expression upserts, citext/vector/tsquery casts, server-clock
   `now()`/`current_date` writes, aggregates the entity API would approximate. Never interpolate
   values into SQL. `sea_orm::sqlx` re-export is the sanctioned escape for `PgListener`.
5. **Migrations** → delete the boot-time `sqlx::migrate!` path and its env gate (config, tests,
   k8s manifest); freeze `migrations/` with a README (do not delete files); fold the final state
   into the declarative schema.
6. **Verification bar** — build the unmodified baseline first, then prove: `cargo check
   --all-targets` with zero *new* warnings, full test suite with zero regressions (report DB-bound
   tests honestly), and grep-clean `sqlx` in Cargo.toml/src (feature strings like `sqlx-postgres`
   excepted).

## Per-repo checklist

- **fiducia.cloud** (`fiducia-messaging.rs` — inbox/outbox/transactional, note sqlx **0.9** behind
  an optional `postgres` feature; `fiducia-memory.rs`, `fiducia-operations-control-plane`,
  `fiducia-ai-agent-control-plane`, `fiducia-ai-agent-bridge.rs`, monorepo `fiducia-customer.rs`):
  fiducia has its own interfaces package — prefer generating/keeping entities there, mirroring how
  `fiducia-customer.rs`/`fiducia-admin.rs` already did it.
- **fiducia-customer.rs** (top-level repo): code is already SeaORM; just delete the leftover
  direct `sqlx` dependency line.
- **ai-agent-bridge**: its tables are in the shared contract (`ai_agent_bridge.*` schema) — use
  `dd-pg-defs-sea-orm` generated entities.
- **3fa-backend**: same — `threefa.*` tables are in the shared contract.
- **mip-solver-node.rs** (+ `local/` crate): has a **vendored** `vendor/pg-defs` — re-vendor from
  the current pg-defs first (see caveats below), then convert against the vendored sea-orm crate.

## pg-defs caveats (fixed / outstanding)

- **Fixed 2026-07-17** (commit pending in the `remote/libs` submodule): the generator previously
  dropped table-level composite primary keys, which made `dd-pg-defs-sea-orm` uncompilable
  (`DeriveEntityModel` requires a PK). Upstream repos vendoring pg-defs must pick up the fixed
  generator/output.
- **Outstanding — bare-name collisions**: generated Rust module names are bare table names
  (`threefa.accounts` → `pub mod accounts`), so a second table named `accounts` in another pg
  schema cannot be added to the contract until the generator learns to disambiguate.
- **Outstanding — pgvector/tsvector**: strict-typed renderers reject `vector(N)`/`tsvector`;
  pgvector tables stay out of the shared contract (service-local schema instead, per
  `dd-embeddings-rs`).
