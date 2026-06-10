//! JSON REST backend for the US Anti-Corruption Court project.

mod auth;
mod config;
mod contract;
mod db;
mod docs;
mod error;
mod metrics;
mod models;
mod routes;
mod simulation;
mod state;

use std::net::SocketAddr;

use axum::extract::DefaultBodyLimit;
use tracing_subscriber::EnvFilter;

use crate::{config::Config, state::AppState};

const MAX_HTTP_BODY_BYTES: usize = 2 * 1024 * 1024;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    init_tracing();

    let config = Config::from_env();
    tracing::info!(
        host = %config.host,
        port = config.port,
        database_configured = config.database_url.is_some(),
        "usacc-rest-api-backend-rs starting"
    );

    let pool = db::connect(&config).await?;
    let http = reqwest::Client::builder()
        .timeout(config.request_timeout)
        .build()?;
    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let state = AppState::new(config, pool, http);
    let app = routes::router(state).layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES));

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "usacc-rest-api-backend-rs listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,hyper=warn,tower_http=info"));
    let json = std::env::var("USACC_LOG_FORMAT")
        .map(|value| value.eq_ignore_ascii_case("json"))
        .unwrap_or(false);

    if json {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install ctrl_c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("ctrl_c received, shutting down"),
        _ = terminate => tracing::info!("SIGTERM received, shutting down"),
    }
}
