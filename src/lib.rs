//! Centralized OreSoftware auth server.
//!
//! Verifies Supabase access tokens issued by any of several configured
//! Supabase projects (one per org: 3fa-app, athlet-o-store, fiducia-cloud,
//! sonus-auris, …), mirrors the verified identity into a local `shared_auth`
//! Postgres schema on AWS RDS, and mints a single unified OreSoftware JWT that
//! every downstream service trusts via this server's published JWKS.
//!
//! It runs *alongside* Supabase's built-in auth — Supabase stays the identity
//! provider; this server is the verifier + re-issuer, never a mirror of
//! Supabase's password store. The account-level Supabase credential is used
//! only by the offline `discover` subcommand, never on the request path.
//!
//! ## Module map
//! - [`config`]  — environment-driven configuration
//! - [`supabase`] — per-project JWKS verification, issuer routing, Management API
//! - [`db`]      — the RDS identity store (`shared_auth.principals`)
//! - [`token`]   — minting unified OreSoftware JWTs and publishing our JWKS
//! - [`recovery`] — government-ID, face, and Voxletra-assisted account recovery
//! - [`http`]    — the axum surface
//! - [`state`]   — shared application state
//! - [`error`], [`telemetry`]

pub mod cache;
pub mod config;
pub mod db;
pub mod email;
pub mod error;
pub mod factors;
pub mod flags;
pub mod http;
pub mod metrics;
pub mod password;
pub mod recovery;
pub mod session;
pub mod state;
pub mod supabase;
pub mod telemetry;
pub mod token;
pub mod twilio;
pub mod views;

use anyhow::Context;

/// OTel service.name / metrics namespace for this process.
pub const SERVICE_NAME: &str = "dd-shared-auth";

/// Process entrypoint: apply CLI flags, initialise telemetry, dispatch subcommand.
///
/// - `serve` (default) — run the HTTP auth server.
/// - `discover` — enumerate the account's Supabase orgs/projects via the
///   Management API and print a ready-to-paste `AUTH_SUPABASE_PROJECTS` value.
pub async fn run() -> anyhow::Result<()> {
    // Ensure a rustls crypto provider is installed for reqwest + sqlx (both use
    // rustls). Best-effort: a later duplicate install is a harmless no-op.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // CLI flags → env BEFORE telemetry/config read, so `--flag` overrides win.
    flags::apply_cli_flags();

    let _otel = telemetry::init(SERVICE_NAME);

    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "serve".to_string());
    match mode.as_str() {
        "serve" => serve().await,
        "discover" => supabase::management::discover().await,
        other => anyhow::bail!("unknown subcommand {other:?} (expected: serve | discover)"),
    }
}

async fn serve() -> anyhow::Result<()> {
    let config = config::AppConfig::from_env().context("loading configuration")?;
    let bind_addr = config.bind_addr;
    let state = state::AppState::build(config)
        .await
        .context("building application state")?;

    let router = http::router(state.clone());
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("binding {bind_addr}"))?;

    tracing::info!(
        %bind_addr,
        projects = state.supabase.len(),
        db = state.db.is_some(),
        recovery = state.recovery.is_some(),
        "shared-auth-server listening"
    );

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("http server error")
}

/// Resolve when SIGTERM (k8s pod termination) or Ctrl-C arrives.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
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
