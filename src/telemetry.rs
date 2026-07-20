//! Tracing / logging init. JSON logs in the cluster (`AUTH_LOG_FORMAT=json`),
//! pretty logs locally. Filter via `RUST_LOG` (default `info`).

use tracing_subscriber::{prelude::*, EnvFilter};

pub fn init() {
    let filter = EnvFilter::try_from_env("RUST_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let json = std::env::var("AUTH_LOG_FORMAT").as_deref() == Ok("json");

    let registry = tracing_subscriber::registry().with(filter);
    if json {
        registry
            .with(tracing_subscriber::fmt::layer().json().flatten_event(true))
            .init();
    } else {
        registry
            .with(tracing_subscriber::fmt::layer().compact())
            .init();
    }
}
