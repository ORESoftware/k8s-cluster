//! 3FA server-rendered web application.

mod app;
mod config;
mod cookies;
mod enrollment;
mod login;
mod metrics;
mod server;
mod shared_auth;
mod state;
mod telemetry;
mod totp;
mod views;

pub use server::run;

#[cfg(test)]
mod totp_edge_tests;

#[cfg(test)]
mod architecture_tests {
    const MAIN: &str = include_str!("main.rs");

    #[test]
    fn binary_entrypoint_stays_a_thin_library_adapter() {
        assert!(MAIN.lines().count() <= 15, "main.rs grew past its boundary");
        assert!(MAIN.contains("threefa_web_server::run().await"));
        for misplaced in [
            "Router::new",
            "TcpListener::bind",
            "reqwest::Client",
            "tracing_subscriber",
        ] {
            assert!(!MAIN.contains(misplaced), "main.rs contains {misplaced}");
        }
    }
}
