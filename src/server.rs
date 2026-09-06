//! Process lifecycle and HTTP listener.

use crate::config::Config;
use crate::state::AppState;
use crate::{app, telemetry};

pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _telemetry = telemetry::init("threefa-web-server");
    let config = Config::from_env()?;
    let bind_addr = config.bind_addr;
    let supabase_enabled = config.supabase.is_some();
    let shared_auth_enabled = config.shared_auth.is_some();
    let state = AppState::from_config(config)?;
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(
        server.address = %bind_addr,
        auth.provider.supabase.enabled = supabase_enabled,
        auth.shared.enabled = shared_auth_enabled,
        "3FA web server listening"
    );
    axum::serve(listener, app::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("3FA web server stopped");
    Ok(())
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let ctrl_c = tokio::signal::ctrl_c();
    let terminate = async {
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => {
                tracing::error!(error = %error, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown signal received");
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}
