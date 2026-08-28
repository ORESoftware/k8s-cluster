use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
};

use tokio::{
    fs,
    sync::{RwLock, Semaphore},
};

mod config;
mod db;
mod ecr;
mod entity;
mod events;
mod exec;
mod fiducia;
mod gh_secrets;
mod gha_workflow;
mod http;
mod jobs;
mod lambda_exec;
mod nats_submit;
mod profiles;
mod state;
mod types;
mod util;
mod validation;
mod webhooks;

use config::{config_from_env, env_u64, env_usize, env_value, Config};
use exec::append_log;
use http::build_router;
use jobs::enqueue_build;
use nats_submit::submit_from_nats;
use state::{AppState, Counters, DEFAULT_PORT, SERVICE_NAME};
use types::{BuildJobRecord, BuildRequest, BuildStatus, DeployRequest, NatsSubmitError};
use util::now_ms;

#[tokio::main]
async fn main() {
    let _otel = dd_telemetry::init("dd-build-server");

    let config = Arc::new(config_from_env());
    let host = env_value("HOST", "0.0.0.0");
    let port = env_u64("PORT", DEFAULT_PORT as u64) as u16;
    let max_concurrent = env_usize("BUILD_SERVER_MAX_CONCURRENT_BUILDS", 1);

    if let Err(error) = fs::create_dir_all(&config.work_root).await {
        panic!("failed to create build server work root: {error}");
    }

    // Optional Postgres persistence (own database dd_build_server on RDS). A
    // connection failure is fatal only when a URL was configured — it signals
    // misconfiguration; with no URL the server runs in-memory as before.
    let db = match config.database_url.as_deref() {
        Some(url) => match db::connect(url).await {
            Ok(connection) => {
                db::fail_interrupted_jobs(&connection).await;
                Some(connection)
            }
            Err(error) => {
                // Never interpolate the error (`{error}` / `{error:?}`):
                // sea-orm/sqlx inline the full connection string, including the
                // password, on a parse failure — which would land the DSN in
                // pod logs. Discard it and emit only a fixed message. Bind to
                // `_` so the value is explicitly dropped unprinted.
                let _ = error;
                panic!(
                    "BUILD_SERVER_DATABASE_URL was set but connect failed (message suppressed to avoid leaking the DSN)"
                );
            }
        },
        None => {
            tracing::info!(
                "no BUILD_SERVER_DATABASE_URL configured; running with in-memory jobs only"
            );
            None
        }
    };

    // Optional NATS (on by default; failure is non-fatal — the server still
    // serves HTTP, it just won't publish/consume events).
    let nats = if config.nats_enabled {
        match events::connect(&config.nats_url).await {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::warn!("NATS disabled: {error}");
                None
            }
        }
    } else {
        None
    };

    let holder = format!("dd-build-server/{}", uuid::Uuid::new_v4());

    let state = AppState {
        config: config.clone(),
        http: reqwest::Client::new(),
        jobs: Arc::new(RwLock::new(HashMap::new())),
        semaphore: Arc::new(Semaphore::new(max_concurrent)),
        counters: Arc::new(Counters::default()),
        db,
        nats,
        holder,
        recent_request_ids: Arc::new(RwLock::new(HashSet::new())),
    };

    // Durable JetStream build-request intake (opt-in).
    if config.nats_intake_enabled && state.nats.is_some() {
        tokio::spawn(events::run_request_intake(state.clone()));
    }
    // Periodic GitHub Actions secret sync (opt-in; 0 interval = manual only).
    if config.gh_sync_enabled && !config.gh_sync_interval.is_zero() {
        tokio::spawn(gh_secrets::run_periodic_sync(state.clone()));
    }

    // The production route table lives with the handlers it composes
    // (`http::build_router`), so the e2e suite drives the exact same router
    // in-process via `tower::ServiceExt::oneshot`. The GHA indie worker mounts
    // a second authenticated route set that compiles bounded workflow YAML to
    // the same fixed-profile queue instead of accepting caller-supplied shell.
    let app = build_router(state.clone()).merge(gha_workflow::router(state));

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("failed to parse bind address");
    tracing::info!("{SERVICE_NAME} listening on http://{address}");

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("failed to bind tcp listener");
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("axum server crashed");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
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
}
