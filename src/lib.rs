//! 3FA server-rendered web application.

mod app;
mod config;
mod cookies;
mod enrollment;
mod login;
mod metrics;
mod server;
mod state;
mod telemetry;
mod totp;
mod views;

pub use server::run;
