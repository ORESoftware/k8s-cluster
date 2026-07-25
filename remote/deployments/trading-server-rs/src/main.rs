use std::{
    error::Error,
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::Duration,
};

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};

mod app_config;
mod decision;
mod http;
mod nats;
mod platforms;
mod state;
mod types;
mod util;
mod validation;

use crate::app_config::{
    record_config_error, refresh_platform_config, run_cdc_refresh_subscription,
    run_config_refresh_loop,
};
use crate::http::{
    api_docs_html, api_docs_json, decide_http, example, healthz, metrics, readyz, root, schema,
};
use crate::nats::{connect_nats, run_nats_loop};
use crate::platforms::default_platform_config;
use crate::state::{config_from_env, AppState, Metrics, MAX_HTTP_BODY_BYTES, SERVICE_NAME};
use crate::util::env_value;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = dd_telemetry::init("dd-trading-server");

    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8103").parse::<u16>()?;
    let nats = connect_nats().await?;
    let config = config_from_env();
    let inflight = Arc::new(tokio::sync::Semaphore::new(config.max_inflight));
    let state = AppState {
        config: Arc::new(config),
        platform_config: Arc::new(RwLock::new(default_platform_config())),
        nats,
        metrics: Arc::new(Metrics::default()),
        inflight,
    };
    if let Err(error) = refresh_platform_config(&state).await {
        tracing::error!("trading platform initial config refresh failed: {error}");
        record_config_error(&state, error).await;
    }
    tokio::spawn(run_config_refresh_loop(state.clone()));
    tokio::spawn(run_nats_loop(state.clone()));
    tokio::spawn(run_cdc_refresh_subscription(state.clone()));

    let app = Router::new()
        .route("/", get(root))
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/schema", get(schema))
        .route("/example", get(example))
        .route("/decide", post(decide_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    tracing::info!("{SERVICE_NAME} listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    Ok(())
}
