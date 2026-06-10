//! billing-server-rs entrypoint.
//!
//! HTTP-only billing platform. Model A (observer/recorder) — we do not move
//! money on our own license. Postgres is the ledger source of truth; Solana
//! is the tamper-evidence notary; provider data flows in via OAuth /
//! webhook ingestors (mostly stubbed in this scaffold).

mod admin;
mod api;
mod cdc;
mod config;
mod crypto;
mod customer_locks;
mod customers;
mod db;
mod error;
mod events;
mod jobs;
mod ledger;
mod locks;
mod money;
mod notifications;
mod providers;
mod scheduler;
mod shard;
mod solana;
mod state;
mod sync;
mod tenants;
mod users;
mod vendors;

use std::sync::Arc;
use tower::Layer;
use tower_http::normalize_path::NormalizePathLayer;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::crypto::Sealer;
use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    // Fail fast at startup if the vendored htmx bytes drifted from the
    // pinned SRI hash. Browsers would otherwise refuse to execute the
    // script and the admin UI would silently break.
    admin::verify_asset_integrity();

    let cfg = Arc::new(Config::from_env()?);
    tracing::info!(host = %cfg.host, port = cfg.port, "billing-server-rs starting");

    let pool = db::connect(&cfg).await?;
    let sealer = Arc::new(Sealer::from_b64_key(&cfg.master_seal_key_b64)?);

    // Optional NATS event bus (publish redacted domain events + listen for
    // inbound sync commands). Disabled unless BILLING_NATS_PUBLISH_ENABLED is
    // set and a URL resolves; a broker outage at boot degrades to a no-op
    // rather than blocking startup. See src/events.rs.
    let events = Arc::new(build_event_bus(&cfg).await);

    let state = AppState::new(cfg.clone(), pool, sealer, events);

    // Seed the built-in system jobs (idempotent) and start the scheduler.
    if let Err(e) = jobs::seed_system_jobs(&state.scheduler).await {
        tracing::warn!(error = %e, "failed to seed system jobs (continuing)");
    }
    let registry = jobs::build_registry(&state);
    let runner = std::sync::Arc::new(scheduler::SchedulerRunner::new(
        state.pool.clone(),
        registry,
    ));
    {
        let r = runner.clone();
        tokio::spawn(async move { r.run_forever().await });
    }

    // Optional WAL-gateway CDC subscription (silent no-op unless
    // BILLING_CDC_NATS_URL is set — see src/cdc.rs).
    cdc::spawn();

    // Inbound NATS sync-command subscriber (silent no-op when the event bus
    // is disabled). Turns dd.remote.billing.commands.sync messages into the
    // same one-shot sync.connection jobs the HTTP "Sync now" path enqueues.
    {
        let s = state.clone();
        tokio::spawn(async move { events::run_sync_command_loop(s).await });
    }

    let app = api::build_router(state);
    // Strip trailing slashes before routing so `/admin/` matches the same
    // handler as `/admin` (which `Router::nest` does not provide on its own).
    // Applied to the entire surface — JSON routes do not use trailing
    // slashes so this is behavior-preserving for them.
    let app = NormalizePathLayer::trim_trailing_slash().layer(app);

    let addr = format!("{}:{}", cfg.host, cfg.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "listening");

    axum::serve(
        listener,
        axum::ServiceExt::<axum::extract::Request>::into_make_service(app),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

/// Build the NATS event bus from config. Off unless
/// `BILLING_NATS_PUBLISH_ENABLED` is set and a URL resolves; a failed
/// connection degrades to a no-op bus (logged) rather than aborting boot.
async fn build_event_bus(cfg: &Config) -> events::EventBus {
    if cfg.nats_publish_enabled {
        if let Some(url) = &cfg.nats_url {
            return events::EventBus::connect(url, cfg.nats_max_payload_bytes).await;
        }
        tracing::warn!(
            "BILLING_NATS_PUBLISH_ENABLED=true but no BILLING_NATS_URL/NATS_URL set; event bus disabled"
        );
    }
    events::EventBus::disabled()
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,hyper=warn,tower_http=info"));

    let want_json = std::env::var("BILLING_LOG_FORMAT")
        .map(|s| s.eq_ignore_ascii_case("json"))
        .unwrap_or(false);

    if want_json {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
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
