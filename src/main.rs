//! Thin binary entrypoint. All logic lives in the library modules
//! (`config`, `supabase`, `db`, `token`, `http`, …) so `main.rs` stays a
//! shell: initialise telemetry, dispatch the subcommand, surface the exit code.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(err) = shared_auth_server::run().await {
        eprintln!("fatal: {err:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
