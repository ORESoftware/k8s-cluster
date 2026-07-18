pub mod app;
pub mod auth;
pub mod command;
pub mod config;
pub mod database;
pub mod db;
pub mod error;
pub mod routes;
pub mod server;
pub mod sync;
pub mod telemetry;
pub mod views;
pub mod ws;

pub use app::{build_app, build_state, AppState};
pub use database::run_migrations;
pub use server::run;

pub const SERVICE: &str = "canonical-web-server";
