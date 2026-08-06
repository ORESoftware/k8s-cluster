//! Network listener, backplane lifecycle, and graceful shutdown.

use std::net::SocketAddr;

use sea_orm::DatabaseBackend;

use crate::{app, config::Config, error::AppError, ws, SERVICE};

pub async fn run(config: Config) -> Result<(), AppError> {
    let port = config.port;
    let state = app::build_state(config).await?;
    let _backplane = if state.db.get_database_backend() == DatabaseBackend::Postgres {
        Some(ws::spawn_postgres_backplane(
            state.config.database_url.clone(),
            state.hub.clone(),
        ))
    } else {
        None
    };
    let app = app::build_app(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, service.name = SERVICE, "web server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

pub(crate) async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!(%error, "failed to install SIGTERM handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
