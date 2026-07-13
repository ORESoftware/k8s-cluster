pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod routes;
pub mod sync;
pub mod views;
pub mod ws;

use axum::{
    http::{header, HeaderName, HeaderValue},
    Router,
};
use config::Config;
use db::migration::Migrator;
use error::AppError;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseBackend};
use sea_orm_migration::MigratorTrait;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tower_http::{
    compression::CompressionLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    sensitive_headers::SetSensitiveRequestHeadersLayer,
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};

pub const SERVICE: &str = "canonical-web-server";

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: sea_orm::DatabaseConnection,
    pub auth: Arc<dyn auth::AuthProvider>,
    pub sessions: auth::SessionService,
    pub hub: ws::Hub,
}

impl AppState {
    pub fn new(
        config: Config,
        db: sea_orm::DatabaseConnection,
        auth: Arc<dyn auth::AuthProvider>,
    ) -> Result<Self, AppError> {
        let config = Arc::new(config);
        let sessions = auth::SessionService::new(
            db.clone(),
            auth.clone(),
            &config.session_encryption_key,
            config.session_ttl,
        )?;

        Ok(Self {
            config,
            db,
            auth,
            sessions,
            hub: ws::Hub::new(256),
        })
    }
}

pub async fn build_state(config: Config) -> Result<AppState, AppError> {
    let db = connect_database(&config.database_url, config.database_max_connections).await?;
    if config.auto_migrate {
        Migrator::up(&db, None).await?;
    }

    let auth = Arc::new(auth::SupabaseAuth::new(
        config.supabase_url.clone(),
        config.supabase_publishable_key.clone(),
    )?);
    AppState::new(config, db, auth)
}

async fn connect_database(
    database_url: &str,
    database_max_connections: u32,
) -> Result<sea_orm::DatabaseConnection, sea_orm::DbErr> {
    let mut options = ConnectOptions::new(database_url);
    options
        .max_connections(database_max_connections)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(10))
        .acquire_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(300))
        .sqlx_logging(false);

    Database::connect(options).await
}

/// Applies all database migrations and exits without constructing the HTTP or
/// Supabase clients. Deploy this with a privileged URL that is never supplied
/// to the long-lived web process.
pub async fn run_migrations(
    database_url: &str,
    database_max_connections: u32,
) -> Result<(), AppError> {
    let db = connect_database(database_url, database_max_connections).await?;
    Migrator::up(&db, None).await?;
    db.close().await?;
    Ok(())
}

pub fn build_app(state: AppState) -> Router {
    let request_id_header = HeaderName::from_static("x-request-id");

    routes::router(state).layer((
        SetSensitiveRequestHeadersLayer::new([header::AUTHORIZATION, header::COOKIE]),
        SetRequestIdLayer::new(request_id_header.clone(), MakeRequestUuid),
        PropagateRequestIdLayer::new(request_id_header),
        SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ),
        SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ),
        CompressionLayer::new(),
        TraceLayer::new_for_http(),
    ))
}

pub async fn run(config: Config) -> Result<(), AppError> {
    let port = config.port;
    let state = build_state(config).await?;
    let _backplane = if state.db.get_database_backend() == DatabaseBackend::Postgres {
        Some(ws::spawn_postgres_backplane(
            state.config.database_url.clone(),
            state.hub.clone(),
        ))
    } else {
        None
    };
    let app = build_app(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, service = SERVICE, "web server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
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
