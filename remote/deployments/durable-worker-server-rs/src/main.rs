use std::{
    env,
    net::SocketAddr,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

use anyhow::{Context, Result};
use async_nats::ConnectOptions;
use axum::extract::DefaultBodyLimit;
use dd_durable_worker_server::{
    api,
    config::ServerConfig,
    docs,
    engine::{Engine, SystemClock},
    store::{NatsEventSink, NatsStateStore, SharedEventSink, SharedStore},
    AppState,
};
use tokio::time::MissedTickBehavior;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

const EXPORT_INTERNAL_FLAG: &str = "--export-openapi";
const EXPORT_PUBLIC_FLAG: &str = "--export-public-openapi";

#[tokio::main]
async fn main() -> Result<()> {
    if env::args().any(|argument| argument == EXPORT_INTERNAL_FLAG) {
        print!("{}", docs::canonical_json(&api::openapi_document())?);
        return Ok(());
    }
    if env::args().any(|argument| argument == EXPORT_PUBLIC_FLAG) {
        let public = docs::project_public(&api::openapi_document())?;
        print!("{}", docs::canonical_json(&public)?);
        return Ok(());
    }

    init_tracing();
    let config = Arc::new(ServerConfig::from_env().map_err(anyhow::Error::msg)?);
    if !config.event_subject.contains('*') {
        anyhow::bail!("DURABLE_WORKER_EVENT_SUBJECT must contain one '*' run-id wildcard");
    }

    let nats = connect_nats(&config)
        .await
        .with_context(|| format!("connect to NATS at {}", config.nats_url))?;
    let jetstream = async_nats::jetstream::new(nats.clone());
    let state_store = NatsStateStore::ensure(&jetstream, &config.state_bucket, config.replicas)
        .await
        .context("create or update durable worker state bucket")?;
    NatsEventSink::ensure_stream(
        &jetstream,
        &config.event_stream,
        &config.event_subject,
        config.replicas,
    )
    .await
    .context("create or update durable worker event stream")?;

    let store: SharedStore = Arc::new(state_store);
    let events: SharedEventSink =
        Arc::new(NatsEventSink::new(jetstream, config.event_subject.clone()));
    let engine = Arc::new(Engine::new(store, events, Arc::new(SystemClock)));
    spawn_scheduler(engine.clone(), config.scheduler_interval);

    let state = AppState {
        engine,
        nats,
        config: config.clone(),
    };
    let app = api::app_router(state)?
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(TraceLayer::new_for_http());

    let address: SocketAddr = format!("0.0.0.0:{}", config.port).parse()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(
        service = "dd-durable-worker-server",
        %address,
        shadow_mode = config.shadow_mode,
        state_bucket = %config.state_bucket,
        event_stream = %config.event_stream,
        "durable worker control plane listening"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("dd_durable_worker_server=info,tower_http=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .with_current_span(true)
        .with_span_list(true)
        .init();
}

async fn connect_nats(config: &ServerConfig) -> Result<async_nats::Client> {
    let mut options = ConnectOptions::new()
        .name("dd-durable-worker-server")
        .retry_on_initial_connect()
        .max_reconnects(None)
        .connection_timeout(Duration::from_secs(10))
        .ping_interval(Duration::from_secs(20))
        .require_tls(config.nats_require_tls);
    if let Some(token) = &config.nats_token {
        options = options.token(token.clone());
    }
    if let Some(path) = &config.nats_credentials_file {
        options = options.credentials_file(path).await?;
    }
    Ok(options.connect(config.nats_url.clone()).await?)
}

fn spawn_scheduler(engine: Arc<Engine>, interval: Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if let Err(error) = engine.tick().await {
                engine
                    .metrics()
                    .scheduler_failures_total
                    .fetch_add(1, Ordering::Relaxed);
                tracing::error!(%error, "durable scheduler tick failed");
            }
        }
    });
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to install Ctrl-C handler");
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
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
