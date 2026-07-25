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

mod analysis;
mod auth;
mod briefs;
mod catalog;
mod grants;
mod http;
mod ingest;
mod nats;
mod pipeline;
mod state;
mod store;
#[cfg(test)]
mod tests;
mod types;
mod ui;
mod util;

use crate::http::{
    api_docs_html, api_docs_json, correlations_http, datasets, descriptor, example,
    grant_match_http, healthz, ingest_http, jobs, metrics, pipeline_jobs_http, readyz, schema,
    scrape_http, sources, trends_http, ui_dashboard, ui_recent_records_fragment, ui_scrape_action,
    ui_sources_fragment, ui_summary_fragment, webhook_ingest_http, white_paper_http,
};
use crate::nats::run_nats_loop;
use crate::state::{
    config_from_env, env_value, optional_env, AppState, Metrics, PublicDataStore,
    MAX_HTTP_BODY_BYTES, SERVICE_NAME,
};
use crate::ui::root;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = dd_telemetry::init("dd-public-data-server");

    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8115").parse::<u16>()?;
    let nats = match optional_env("NATS_URL") {
        // Degrade gracefully if the broker is down at boot: the HTTP/ingest API
        // must come up even when messaging is unavailable. async-nats serves a
        // reconnecting client, so a later recovery is picked up.
        Some(url) => match async_nats::connect(&url).await {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::error!("dd-public-data-server NATS connect failed ({url}): {error}");
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
            .timeout(Duration::from_secs(75))
            .build()?,
        store: Arc::new(RwLock::new(PublicDataStore::default())),
    };
    tokio::spawn(run_nats_loop(state.clone()));

    let app = Router::new()
        .route("/", get(root))
        .route("/ui", get(ui_dashboard))
        .route("/ui/fragments/summary", get(ui_summary_fragment))
        .route("/ui/fragments/sources", get(ui_sources_fragment))
        .route(
            "/ui/fragments/recent-records",
            get(ui_recent_records_fragment),
        )
        .route("/ui/actions/scrape", post(ui_scrape_action))
        .route("/descriptor", get(descriptor))
        .route("/sources", get(sources))
        .route("/schema", get(schema))
        .route("/example", get(example))
        .route("/datasets", get(datasets))
        .route("/jobs", get(jobs))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/webhooks/ingest", post(webhook_ingest_http))
        .route("/ingest", post(ingest_http))
        .route("/scrape", post(scrape_http))
        .route("/grants/match", post(grant_match_http))
        .route("/analysis/trends", post(trends_http))
        .route("/analysis/correlations", post(correlations_http))
        .route("/briefs/white-paper", post(white_paper_http))
        .route("/pipeline/jobs", post(pipeline_jobs_http))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    tracing::info!("{SERVICE_NAME} listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
