//! Sonus Auris backend service library.
//!
//! The binary target is intentionally only a Tokio entry point. Service
//! composition and business behavior live in library modules so they can be
//! tested without booting a process.

mod database;
mod service;
mod telemetry;

pub use service::run;
