//! Process lifecycle and HTTP listener.

use crate::config::Config;
use crate::shared_auth::SharedAuthClient;
use crate::state::AppState;
use crate::{app, db, telemetry};
use std::net::SocketAddr;

pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _telemetry = telemetry::init("threefa-sync-server");
    let config = Config::from_env()?;
    let database = db::connect(&config.database_url).await?;
    let shared_auth = config.shared_auth.clone().map(SharedAuthClient::new);
    let shared_auth_enabled = shared_auth.is_some();
    let state = AppState::new(database)?.with_shared_auth(shared_auth);
    let router = app::router(state);

    tracing::info!(
        server.address = %config.bind_addr,
        auth.shared.enabled = shared_auth_enabled,
        protocol.version = crate::protocol::PROTOCOL_VERSION,
        "3FA sync server listening"
    );

    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    // Socket peer information is the rate limiter's fallback when a trusted
    // ingress forwarding header is absent.
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    tracing::info!("3FA sync server stopped");
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
