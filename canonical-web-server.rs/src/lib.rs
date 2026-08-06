#[cfg(all(feature = "test-auth", not(debug_assertions)))]
compile_error!("the test-auth feature is forbidden in release builds");

pub mod app;
pub mod auth;
pub mod command;
pub mod database;
pub mod error;
pub mod metrics;
pub mod routes;
pub mod server;
pub mod sync;
pub mod telemetry;
pub mod views;
pub mod ws;

pub use app::{build_app, build_state, AppState};
pub use canonical_config as config;
pub use canonical_store as db;
pub use database::run_migrations;
pub use server::run;

pub const SERVICE: &str = "canonical-web-server";
