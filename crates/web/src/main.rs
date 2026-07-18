//! t2v-web — the MASH (maud + axum + seaorm + htmx) dashboard for the
//! t2v-v2t platform.
//!
//! It renders server-side HTML with maud, drives interactivity with htmx, and
//! streams a live stats ticker over a websocket (htmx `ws` extension). The
//! dashboard reads the shared `t2v` Postgres namespace directly via SeaORM;
//! interactive translate/TTS actions are proxied to the t2v-api server.
//!
//! Deploys separately from t2v-api.

mod assets;
mod db;
mod routes;
mod state;
mod views;

use axum::middleware::from_fn;
use axum::routing::get;
use axum::Router;
use state::AppState;
use std::net::SocketAddr;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;

/// Backstop request timeout. The action proxy to t2v-api has its own 190s
/// client timeout; this bounds everything else (including slow request bodies).
const REQUEST_TIMEOUT_SECS: u64 = 200;

fn init_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .init();
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(routes::dashboard))
        .route(
            "/translate",
            get(routes::translate_page).post(routes::translate_action),
        )
        .route("/speak", get(routes::speak_page).post(routes::speak_action))
        .route("/history", get(routes::history_page))
        .route("/ws/stats", get(routes::stats_ws))
        .route("/healthz", get(routes::healthz))
        .route("/readyz", get(routes::readyz))
        .with_state(state)
}

fn port() -> u16 {
    std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8131)
}

async fn shutdown_signal() {
    use tokio::signal;
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("t2v-web shutdown signal received");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

    let db = db::connect_and_prepare().await?;
    let state = AppState::new(db);
    tracing::info!("t2v-web: proxying actions to API at {}", state.api_base);

    let addr = SocketAddr::from(([0, 0, 0, 0], port()));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("t2v-web listening on http://{addr}");

    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}
