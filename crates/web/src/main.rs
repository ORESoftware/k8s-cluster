//! t2v-web — the MASH (maud + axum + seaorm + htmx) dashboard for the
//! t2v-v2t platform.
//!
//! Renders server-side HTML with maud, drives interactivity with htmx, and
//! streams a live stats ticker over a websocket (htmx `ws` extension). Reads
//! the shared `t2v` Postgres namespace directly via SeaORM; interactive
//! translate/TTS actions are proxied to the t2v-api server.
//!
//! The router and modules live in `lib.rs` so integration tests and the browser
//! e2e harness can drive them. Deploys separately from t2v-api.

use std::net::SocketAddr;
use t2v_web::{app, db, state::AppState};

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
    // Keep the provider alive through graceful shutdown so queued traces,
    // metrics, and structured warnings are flushed before the pod exits.
    let _telemetry = fiducia_telemetry::init("dd-t2v-web");

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
