//! 3FA zero-knowledge sync server library.

mod accounts;
mod app;
mod auth;
mod config;
mod db;
mod devices;
mod entity;
mod error;
mod health;
mod metrics;
mod protocol;
mod server;
mod state;
mod supabase;
mod supabase_auth;
mod telemetry;
mod vault_blob;

pub use server::run;
