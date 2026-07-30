//! t2v-api — the JSON API server for the t2v-v2t platform.
//!
//! Endpoints (all JSON unless noted):
//!   GET  /                       service banner
//!   GET  /healthz                liveness
//!   GET  /readyz                 readiness (DB ping)
//!   GET  /metrics                Prometheus text
//!   POST /v1/stt                 audio body -> transcription      (custom FFT VAD trim)
//!   POST /v1/tts                 {text,voice,format} -> audio bytes
//!   POST /v1/translate           {text,target_lang,...} -> translation
//!   POST /v1/speech-to-speech    audio body -> translated audio   (STT->translate->TTS)
//!   POST /v1/analyze             audio body -> FFT spectral analysis + DTMF
//!   GET  /v1/history/...         recent rows for each table
//!   POST /vapi/webhook           Vapi server webhook (x-vapi-secret)
//!   POST /vapi/call              start a Vapi call (operator, VAPI_API_KEY)
//!   GET  /vapi/call/:id          fetch a Vapi call (operator)
//!
//! The router and modules live in `lib.rs` so integration tests can drive them
//! without a socket. Persistence is SeaORM (not sqlx); the Postgres schema is
//! owned by the shared pg-defs contract under the `t2v` namespace.

use std::net::SocketAddr;
use t2v_api::{app, db, state::AppState};

fn port() -> u16 {
    std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8130)
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
    tracing::info!("t2v-api shutdown signal received");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Keep the provider alive through graceful shutdown so queued traces,
    // metrics, and structured warnings are flushed before the pod exits.
    let _telemetry = fiducia_telemetry::init("dd-t2v-api");

    let db = db::connect_and_prepare().await?;
    let state = AppState::new(db);

    let providers: Vec<&str> = state
        .llm
        .configured_providers()
        .iter()
        .map(|p| p.as_str())
        .collect();
    tracing::info!(
        "t2v-api starting: translation providers configured = {:?}, vapi = {}",
        providers,
        state.vapi.is_configured()
    );

    let addr = SocketAddr::from(([0, 0, 0, 0], port()));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("t2v-api listening on http://{addr}");

    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}
