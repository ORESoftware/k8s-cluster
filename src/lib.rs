//! shared-auth-nats-bridge — the event plane between shared-auth HTTP services
//! and cluster NATS.
//!
//! HTTP→NATS: internal callers (shared-auth-server, the shared-auth-sync outbox
//! flusher) POST events to `/publish`; the bridge lands them broker-confirmed on
//! `shared-auth.*` subjects. NATS→HTTP: configured subjects fan out to webhooks
//! (`BRIDGE_DELIVERIES`), so services without a NATS client still react.
//!
//! Module map: [`config`], [`publisher`] (background-connect NATS slot),
//! [`deliver`] (subscription→webhook loops), [`http`] (axum surface),
//! [`metrics`], [`telemetry`], [`flags`].

pub mod config;
pub mod deliver;
pub mod flags;
pub mod http;
pub mod metrics;
pub mod publisher;
pub mod telemetry;

use std::sync::Arc;

use anyhow::Context;

pub const SERVICE_NAME: &str = "dd-shared-auth-nats-bridge";

pub async fn run() -> anyhow::Result<()> {
    flags::apply_cli_flags();
    let _otel = telemetry::init(SERVICE_NAME);

    let config = config::Config::from_env().context("loading configuration")?;
    let bind_addr = config.bind_addr;

    let publisher = publisher::Publisher::connect_in_background(config.nats_url.clone());
    let outbound = reqwest::Client::builder()
        .user_agent(concat!(
            "shared-auth-nats-bridge/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .context("building http client")?;
    let metrics = metrics::Metrics::new();

    deliver::spawn_delivery_loops(
        publisher.clone(),
        config.deliveries.clone(),
        outbound,
        metrics.clone(),
    );

    let state = http::AppState {
        config: Arc::new(config),
        publisher,
        metrics,
    };
    let app = http::router(state);
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("binding {bind_addr}"))?;
    tracing::info!(%bind_addr, "shared-auth-nats-bridge listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("http server error")
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown signal received");
}
