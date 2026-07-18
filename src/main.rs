//! 3FA zero-knowledge sync server entrypoint.

mod app;
mod auth;
mod db;
mod devices;
mod error;
mod metrics;
mod protocol;
mod telemetry;
mod vault_blob;

#[tokio::main]
async fn main() {
    if let Err(e) = app::run().await {
        tracing::error!(error = %e, "3FA sync server stopped with a fatal error");
        std::process::exit(1);
    }
}
