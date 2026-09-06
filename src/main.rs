//! Thin entrypoint — logic lives in the library modules.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(error) = shared_auth_nats_bridge::run().await {
        eprintln!("fatal: {error:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
