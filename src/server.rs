use std::sync::Arc;

use tower::Layer;
use tower_http::normalize_path::NormalizePathLayer;

use crate::config::Config;
use crate::crypto::Sealer;
use crate::state::AppState;
use crate::{admin, api, cdc, checkout, db, events, jobs, scheduler, shared_auth_startup};

/// Initialize the billing platform resources, background jobs, and HTTP API.
pub(crate) async fn run() -> anyhow::Result<()> {
    let _telemetry = dd_telemetry::init("billing-server-rs");

    // Fail fast if the vendored htmx bytes drift from the pinned SRI hash.
    admin::verify_asset_integrity();

    // Validate centralized identity before opening Postgres, unsealing provider
    // credentials, connecting messaging, or starting jobs.
    shared_auth_startup::validate_before_resources()?;

    let config = Arc::new(Config::from_env()?);
    tracing::info!(host = %config.host, port = config.port, "billing-server-rs starting");

    let pool = db::connect(&config).await?;
    let sealer = Arc::new(Sealer::from_b64_key(&config.master_seal_key_b64)?);

    // Optional NATS event bus. A broker outage at boot degrades to a disabled
    // bus rather than blocking startup; committed work remains in Postgres.
    let event_bus = Arc::new(build_event_bus(&config).await);

    let state = AppState::new(config.clone(), pool, sealer, event_bus)?;

    // Seed built-in system jobs idempotently and start the scheduler.
    if let Err(error) = jobs::seed_system_jobs(&state.scheduler).await {
        tracing::warn!(error = %error, "failed to seed system jobs (continuing)");
    }
    let registry = jobs::build_registry(&state);
    let runner = Arc::new(scheduler::SchedulerRunner::new(
        state.pool.clone(),
        registry,
    ));
    {
        let runner = runner.clone();
        tokio::spawn(async move { runner.run_forever().await });
    }

    // Optional WAL-gateway CDC subscription.
    cdc::spawn();

    // Inbound NATS sync commands become the same one-shot jobs as HTTP
    // "Sync now" requests. This is a silent no-op when the bus is disabled.
    {
        let state = state.clone();
        tokio::spawn(async move { events::run_sync_command_loop(state).await });
    }

    let app = api::build_router(state.clone())?
        .merge(checkout::router(state)?)
        .layer(dd_telemetry::http_trace_layer());
    // Make `/admin/` and `/admin` equivalent without changing JSON route
    // semantics.
    let app = NormalizePathLayer::trim_trailing_slash().layer(app);

    let address = listener_address(&config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(%address, "listening");

    axum::serve(
        listener,
        axum::ServiceExt::<axum::extract::Request>::into_make_service(app),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

/// Build the NATS event bus from configuration. It is off unless publishing is
/// enabled and a URL resolves; failed connection degrades to a no-op bus.
async fn build_event_bus(config: &Config) -> events::EventBus {
    if config.nats_publish_enabled {
        if let Some(url) = &config.nats_url {
            return events::EventBus::connect(url, config.nats_max_payload_bytes).await;
        }
        tracing::warn!(
            "BILLING_NATS_PUBLISH_ENABLED=true but no BILLING_NATS_URL/NATS_URL set; event bus disabled"
        );
    }
    events::EventBus::disabled()
}

fn listener_address(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c().await.expect("install ctrl_c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("ctrl_c received, shutting down"),
        _ = terminate => tracing::info!("SIGTERM received, shutting down"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_address_combines_the_configured_host_and_port() {
        assert_eq!(listener_address("0.0.0.0", 8080), "0.0.0.0:8080");
        assert_eq!(listener_address("127.0.0.1", 9120), "127.0.0.1:9120");
    }
}
