//! Standalone REST/WebSocket listener for `canonical-api-server`.

use std::net::SocketAddr;

use sea_orm::DatabaseBackend;

use crate::{app, config::Config, error::AppError, server, ws};

pub const SERVICE: &str = "canonical-api-server";

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
    let app = app::build_api_app(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, service.name = SERVICE, "API server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(server::shutdown_signal())
        .await?;
    Ok(())
}
