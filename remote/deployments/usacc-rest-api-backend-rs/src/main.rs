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
mod ui;

use std::net::SocketAddr;

use axum::extract::DefaultBodyLimit;

use crate::{config::Config, state::AppState};

const MAX_HTTP_BODY_BYTES: usize = 2 * 1024 * 1024;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _otel = dd_telemetry::init("usacc-rest-api-backend-rs");

    // Fail loudly at startup if the vendored htmx bytes drift from the
    // pinned SRI hash, rather than letting browsers refuse the script.
    ui::verify_asset_integrity();

    let config = Config::from_env();
    tracing::info!(
        host = %config.host,
        port = config.port,
        database_configured = config.database_url.is_some(),
        app_ui_enabled = config.app_ui_enabled,
        app_base_path = %config.app_base_path,
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
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
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
