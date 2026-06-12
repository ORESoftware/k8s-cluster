//! 3FA zero-knowledge sync server entrypoint.

mod app;
mod auth;
mod db;
mod devices;
mod error;
mod protocol;
mod ratelimit;
mod vault_blob;

#[tokio::main]
async fn main() {
    if let Err(e) = app::run().await {
        eprintln!("fatal: {e}");
        std::process::exit(1);
    }
}
