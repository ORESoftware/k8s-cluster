//! 3FA zero-knowledge sync server library.

mod account_security;
mod accounts;
mod app;
mod auth;
mod config;
mod db;
mod device_sync_protocol;
mod devices;
mod entity;
mod error;
mod health;
mod json;
mod metrics;
mod protocol;
mod server;
mod shared_auth;
mod signal_store;
mod state;
mod supabase_auth;
mod telemetry;
mod vault_blob;

pub use server::run;

#[cfg(test)]
mod architecture_tests {
    const MAIN: &str = include_str!("main.rs");

    #[test]
    fn binary_entrypoint_stays_a_thin_library_adapter() {
        assert!(MAIN.lines().count() <= 12, "main.rs grew past its boundary");
        assert!(MAIN.contains("threefa_backend::run().await"));
        for misplaced in [
            "Router::new",
            "TcpListener::bind",
            "Database::connect",
            "tracing_subscriber",
        ] {
            assert!(!MAIN.contains(misplaced), "main.rs contains {misplaced}");
        }
    }
}
