//! Hand-written SeaORM entities mirroring the pg-defs adapter style.
//!
//! SOURCE OF TRUTH: remote/libs/pg-defs/schema/databases/dd_build_server/schema.sql
//! (the build server's OWN database — its own namespace, separate from the
//! shared pg-defs contract database). Do not infer migrations from these
//! structs; schema changes go through the contract file + scripts/dpm.sh.

pub mod build_jobs;
pub mod gh_secret_sync_runs;
pub mod webhook_deliveries;
