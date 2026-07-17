//! SeaORM entities for the billing database. Hand-written against the
//! declarative schema in `schema/schema.sql` (the consolidated final state of
//! the frozen files under `migrations/`); operators converge the live
//! database onto that schema with the dpm workflow in `scripts/dpm.sh`.
//!
//! Style notes, mirroring `remote/libs/pg-defs/generated/rust/sea-orm`:
//!   * Postgres enum columns are modeled as `String` with
//!     `select_as = "text"` / `save_as = "<enum>"` so reads arrive as `text`
//!     (sqlx cannot decode a custom enum OID into `String` without the cast)
//!     and writes are cast back to the enum type — byte-for-byte the same
//!     casts the previous hand-written SQL used.
//!   * `citext` columns use the same select_as/save_as trick. This matters
//!     for WHERE clauses too: comparing a citext column against a bare text
//!     parameter degrades to a case-SENSITIVE text comparison, so lookup
//!     queries keep their explicit `$n::citext` casts (see the services).
//!   * `numeric(38, 0)` money columns are `Decimal` (rust_decimal) for
//!     schema accuracy; the ledger runtime paths continue to transport
//!     amounts as text-cast i128 exactly as before (rust_decimal's 96-bit
//!     mantissa cannot represent the full 38-digit column domain).
//!
//! Some tables (e.g. `tenant_api_keys`) have no runtime call sites yet; the
//! entity layer intentionally models the full declarative schema so it stays
//! a complete typed mirror, hence the module-wide dead_code allowance.
#![allow(dead_code)]

pub mod accounts;
pub mod anchors;
pub mod dead_letter_jobs;
pub mod job_runs;
pub mod notification_dispatches;
pub mod notification_rules;
pub mod oauth_states;
pub mod postings;
pub mod provider_balance_snapshots;
pub mod provider_connections;
pub mod provider_rate_limit_buckets;
pub mod reconciliation_breaks;
pub mod scheduled_jobs;
pub mod tenant_api_keys;
pub mod tenant_lock_events;
pub mod tenant_locks;
pub mod tenants;
pub mod transactions;
pub mod users;
pub mod webhook_events;
