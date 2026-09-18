//! `formal-methods-service` library entry point.
//!
//! The crate exposes the building blocks of the webhook service so that they
//! can be unit-tested and reused from `main.rs` and from integration tests.
//!
//! High-level wiring:
//!
//! * [`config`]   parses environment variables into a typed [`config::Config`].
//! * [`signature`] verifies GitHub HMAC-SHA256 webhook signatures.
//! * [`github`]   has webhook payload types and an outbound API client used to
//!   post check-run results back to the PR.
//! * [`analysis`] runs analyzer pipelines (currently just `cargo check` and
//!   `cargo test`) against a freshly checked-out PR head commit. Formal-
//!   methods steps (Kani, Verus, raw Z3, ...) plug in as additional
//!   [`analysis::Analyzer`] implementations.
//! * [`routes`]   builds the axum router and HTTP handlers.
//! * [`state`]    holds the shared application state.

pub mod analysis;
pub mod config;
pub mod dedupe;
pub mod error;
pub mod github;
pub mod path_filter;
pub mod repo_allowlist;
pub mod routes;
pub mod signature;
pub mod state;
