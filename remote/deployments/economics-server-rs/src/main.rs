use std::{
    collections::BTreeMap,
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
use serde_json::json;

mod catalog;
mod dashboard;
mod forecast;
mod handlers;
mod metrics;
mod nats;
mod pipeline;
mod recommendations;
mod sentiment;
mod shared;
mod sources;
mod state;
#[cfg(test)]
mod tests;
mod types;

use crate::handlers::*;
use crate::metrics::{metrics, Metrics};
use crate::nats::run_nats_loop;
use crate::shared::emit_log;
use crate::state::{config_from_env, env_value, optional_env, AppState, MAX_HTTP_BODY_BYTES};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = dd_telemetry::init("dd-economics-server");

    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8114").parse::<u16>()?;
    // Degrade gracefully if the broker is unreachable at boot: the HTTP API
    // (forecasts, readiness, ingestion) must come up even when messaging is down.
    // async-nats serves a reconnecting client, so a later recovery is picked up.
    let nats = match optional_env("NATS_URL") {
        Some(url) => match async_nats::connect(&url).await {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::error!("dd-economics-server NATS connect failed ({url}): {error}");
                None
            }
        },
        None => None,
    };
    let state = AppState {
        config: Arc::new(config_from_env()),
        metrics: Arc::new(Metrics::default()),
        nats,
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("dd-economics-server/0.1 source-pull")
            .build()?,
        series_store: Arc::new(RwLock::new(BTreeMap::new())),
    };
    tokio::spawn(run_nats_loop(state.clone()));

    let app = Router::new()
        .route("/", get(root))
        .route("/descriptor", get(descriptor))
        .route("/dashboard.json", get(dashboard_json))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/schema", get(schema))
        .route("/example", get(example))
        .route("/sources", get(sources))
        .route("/sources/public", get(public_sources))
        .route("/sources/pull", post(pull_source_http))
        .route("/sentiment/sources", get(sentiment_sources))
        .route("/sentiment/analyze", post(sentiment_analyze_http))
        .route("/macro/indicators", get(macro_indicators))
        .route("/vc/investment", get(vc_investment))
        .route("/recommendations", post(recommendations_http))
        .route("/audit/hardening", get(hardening_audit))
        .route("/observability", get(observability))
        .route("/integrations/health", get(integrations_health))
        .route("/pipelines/catalog", get(pipeline_catalog))
        .route("/pipelines/plan", post(pipeline_plan_http))
        .route("/pipelines/submit", post(pipeline_submit_http))
        .route("/model/equations", get(equations))
        .route("/engine/des", get(des_engine_descriptor))
        .route("/forecast", post(forecast_http))
        .route("/ingest", post(ingest_http))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    emit_log(
        "INFO",
        "economics.server.start",
        "dd-economics-server listening",
        json!({
            "address": addr.to_string(),
            "metricsRoute": "GET /metrics",
            "observabilityRoute": "GET /observability",
            "otelMode": "explicit-only"
        }),
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    Ok(())
}
