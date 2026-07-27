//! 3FA zero-knowledge sync server library.

#[expect(
    dead_code,
    reason = "account recovery and local-unlock contracts are validated now but their authenticated HTTP workflow is tracked separately"
)]
mod account_security;
mod accounts;
mod app;
mod auth;
mod config;
mod db;
#[expect(
    dead_code,
    reason = "associated-data and validation helpers are shared protocol fixtures; some remain client-consumed until the public Rust SDK is split out"
)]
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
mod signal_api;
#[expect(
    dead_code,
    reason = "terminal Signal revocation is transactionally implemented and tested but the live device endpoint still uses the legacy revocation path"
)]
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
