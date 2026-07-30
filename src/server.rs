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
    let signal_sync_enabled =
        signal_sync_api_enabled(std::env::var("ENABLE_SIGNAL_SYNC_API").ok().as_deref());
    let state = AppState::new(database)?.with_shared_auth(shared_auth);
    let router = app::router_with_signal(state.clone(), signal_sync_enabled);
    let metrics_router = app::metrics_router(state);

    tracing::info!(
        server.address = %config.bind_addr,
        server.metrics_address = %config.metrics_bind_addr,
        auth.shared.enabled = shared_auth_enabled,
        sync.signal_api.enabled = signal_sync_enabled,
        protocol.version = crate::protocol::PROTOCOL_VERSION,
        "3FA sync server listening"
    );

    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    // Telemetry gets its own socket so the public port has no /metrics route at
    // all; see `app::metrics_router`. Bound before serving either, so a port
    // clash is a startup failure rather than a silently unscrapeable pod.
    let metrics_listener = tokio::net::TcpListener::bind(config.metrics_bind_addr).await?;

    // One signal, both listeners: the pod must drain as a unit, or the kubelet
    // sees a process that is still "up" long after it stopped serving traffic.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(true);
    });

    // Socket peer information is the rate limiter's fallback when a trusted
    // ingress forwarding header is absent.
    let public = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_requested(shutdown_rx.clone()));
    let metrics = axum::serve(metrics_listener, metrics_router.into_make_service())
        .with_graceful_shutdown(shutdown_requested(shutdown_rx));

    // `try_join!` drops (and therefore cancels) the other future if either one
    // fails, so a dead telemetry listener cannot leave a half-serving process.
    tokio::try_join!(public, metrics)?;
    tracing::info!("3FA sync server stopped");
    Ok(())
}

fn signal_sync_api_enabled(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

async fn shutdown_requested(mut shutdown: tokio::sync::watch::Receiver<bool>) {
    while !*shutdown.borrow_and_update() {
        if shutdown.changed().await.is_err() {
            break;
        }
    }
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

#[cfg(test)]
mod signal_flag_tests {
    use super::signal_sync_api_enabled;

    #[test]
    fn signal_routes_are_fail_closed_by_default() {
        for value in [None, Some(""), Some("false"), Some("0"), Some("unexpected")] {
            assert!(!signal_sync_api_enabled(value));
        }
        for value in [Some("1"), Some("true"), Some("YES"), Some(" on ")] {
            assert!(signal_sync_api_enabled(value));
        }
    }
}
