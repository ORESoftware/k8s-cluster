use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use tokio::sync::Mutex;

mod background;
mod config;
mod dispatch;
mod engine;
mod http_api;
mod lifecycle;
mod pool_config;
mod redis_lock;
mod types;
mod util;

use crate::{
    background::{
        run_cdc_refresh_subscription, run_config_refresh_loop, run_nats_loop,
        run_reconcile_loop,
    },
    config::service_config_from_env,
    engine::cleanup_managed_containers_on_start,
    http_api::{
        api_docs_html, api_docs_json, dispatch_pool, get_pool, healthz, list_pools, metrics,
        readyz, warm_pool,
    },
    lifecycle::reconcile_all,
    pool_config::{record_config_error, refresh_pool_configs},
    redis_lock::RedisLockManager,
    types::{AppState, Metrics, PoolRegistry, DEFAULT_PORT, MAX_HTTP_BODY_BYTES, SERVICE_NAME},
    util::{env_u16, env_value},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _otel = dd_telemetry::init("dd-container-pool");

    let config = Arc::new(service_config_from_env());
    let registry = Arc::new(Mutex::new(PoolRegistry {
        next_port: config.port_start,
        ..PoolRegistry::default()
    }));
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(900))
        .build()?;
    let nats = match config.nats_url.as_deref() {
        Some(url) => match async_nats::connect(url).await {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::error!("container pool nats connect failed: {error}");
                None
            }
        },
        None => None,
    };
    let redis_locks = match config.redis_url.as_deref() {
        Some(url) => Some(RedisLockManager::new(
            url,
            config.redis_lock_prefix.clone(),
            config.redis_lock_ttl,
            config.redis_lock_wait_timeout,
            config.redis_lock_retry_delay,
            config.redis_lock_request_timeout,
        )?),
        None => None,
    };
    let state = AppState {
        config,
        registry,
        http,
        nats,
        redis_locks,
        metrics: Arc::new(Metrics::default()),
    };

    tracing::info!(
        "{SERVICE_NAME} starting: engine={} bin={} namespace={} oci_runtime={} network={} db_configured={} nats_subject={} redis_locks={}",
        state.config.engine.label(),
        state.config.engine_bin,
        state.config.containerd_namespace,
        state.config.oci_runtime.as_deref().unwrap_or("(default)"),
        state.config.network,
        state.config.database_url.is_some(),
        state.config.nats_subject,
        state.redis_locks.is_some()
    );

    if let Err(error) = cleanup_managed_containers_on_start(&state).await {
        tracing::error!("container pool startup cleanup failed: {error}");
    }
    if let Err(error) = refresh_pool_configs(&state).await {
        tracing::error!("container pool initial config refresh failed: {error}");
        record_config_error(&state, error).await;
    }
    let initial_reconcile_state = state.clone();
    tokio::spawn(async move {
        reconcile_all(&initial_reconcile_state).await;
    });

    tokio::spawn(run_config_refresh_loop(state.clone()));
    tokio::spawn(run_reconcile_loop(state.clone()));
    tokio::spawn(run_nats_loop(state.clone()));
    tokio::spawn(run_cdc_refresh_subscription(state.clone()));

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/pools", get(list_pools))
        .route("/pools/:pool", get(get_pool))
        .route("/pools/:pool/warm", post(warm_pool))
        .route("/pools/:pool/dispatch", post(dispatch_pool))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state.clone())
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let host = env_value("HOST", "0.0.0.0");
    let port = env_u16("PORT", DEFAULT_PORT);
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("{SERVICE_NAME} listening on {addr}");
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::error!("shutdown signal error: {error}");
            }
        })
        .await?;
    Ok(())
}
