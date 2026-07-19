//! daedalus-web-server — server-rendered Maud/htmx UI over the `daedalus` schema.
//!
//! This is the M/H tier of MASH (Maud + htmx). It reads the same `daedalus`
//! Postgres data the API server writes and renders it as HTML, with a websocket
//! that pushes live HTML fragments (htmx-ws). It never writes: mutations go
//! through daedalus-api-server.
//!
//! Surface:
//!   GET /health                    liveness, unauthenticated
//!   GET /ready                     readiness (database reachable)
//!   GET /metrics                   Prometheus text exposition
//!   GET /assets/:name              pinned, self-hosted htmx bundles
//!   GET /                          the caller's plans (HTML)
//!   GET /plans/:id                 one plan + its runs (HTML)
//!   GET /plans/:id/runs            runs table fragment (htmx)
//!   GET /plans/:id/runs/ws         websocket: live runs fragment (htmx-ws)
//!
//! Everything except health/ready/metrics/assets requires a Supabase bearer
//! token whose email is on the allow-list. Ownership is enforced by filtering
//! on the verified email — this database has no RLS.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use tower_http::set_header::SetResponseHeaderLayer;

use config::ServiceConfig;
use error::ServiceError;
use persistence::Persistence;
use supabase_auth::{bearer_token, Operator, SupabaseVerifier};

mod config;
mod error;
mod metrics;
mod persistence;
mod routes;
mod supabase_auth;
mod views;
mod ws;

pub const SERVICE_NAME: &str = "daedalus-web-server";

pub(crate) struct AppState {
    pub(crate) persistence: Persistence,
    pub(crate) verifier: Option<SupabaseVerifier>,
    pub(crate) http: reqwest::Client,
    pub(crate) metrics: metrics::Metrics,
}

pub(crate) type SharedState = Arc<AppState>;

/// Same verified-and-allow-listed extractor as the API server. Placing the gate
/// in an extractor means a page handler cannot accidentally skip it.
#[axum::async_trait]
impl FromRequestParts<SharedState> for Operator {
    type Rejection = ServiceError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &SharedState,
    ) -> Result<Self, Self::Rejection> {
        let result = Self::authorize(parts, state).await;
        match &result {
            Ok(_) => state.metrics.record_authorized(),
            Err(_) => state.metrics.record_rejected(),
        }
        result
    }
}

impl Operator {
    async fn authorize(parts: &mut Parts, state: &SharedState) -> Result<Self, ServiceError> {
        let verifier = state.verifier.as_ref().ok_or_else(|| {
            // Fail closed. An unconfigured gate must never mean "allow".
            ServiceError::Unavailable(
                "Supabase auth is not configured; refusing to serve authenticated routes"
                    .to_string(),
            )
        })?;
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok());
        let token = bearer_token(header).ok_or(ServiceError::Unauthorized)?;
        verifier.authorize(&state.http, token).await
    }
}

pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _guard = observability::init();
    let config = ServiceConfig::from_env()?;
    let persistence = Persistence::from_env().await?;
    let verifier = SupabaseVerifier::from_config(&config.supabase);

    if verifier.is_none() {
        tracing::warn!(
            "Supabase auth is NOT configured (need a JWT secret or JWKS URL *and* \
             DAEDALUS_WEB_ALLOWED_EMAILS); authenticated pages will return 503"
        );
    }

    let state: SharedState = Arc::new(AppState {
        persistence,
        verifier,
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?,
        metrics: metrics::Metrics::new(),
    });

    let app = router(state.clone());
    let address: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    observability::server_listening(
        address,
        state.persistence.is_enabled(),
        state.verifier.is_some(),
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

pub(crate) fn router(state: SharedState) -> Router {
    // A strict, self-only CSP. htmx and its ws extension are served from
    // /assets on this origin, so no third-party script/connect origin is needed.
    // `connect-src 'self'` covers the same-origin websocket.
    const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
                       connect-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'";
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(routes::ready))
        .route("/metrics", get(routes::metrics))
        .route("/assets/:name", get(routes::asset))
        .route("/", get(routes::index))
        .route("/plans/:id", get(routes::plan_detail))
        .route("/plans/:id/runs", get(routes::plan_runs_fragment))
        .route("/plans/:id/runs/ws", get(ws::plan_runs))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
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
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown signal received");
}

mod observability {
    use std::net::SocketAddr;

    pub(crate) fn init() -> dd_telemetry::OtelGuard {
        dd_telemetry::init(crate::SERVICE_NAME)
    }

    pub(crate) fn server_listening(address: SocketAddr, persistence: bool, auth: bool) {
        tracing::info!(
            service.name = crate::SERVICE_NAME,
            server.address = %address,
            db.client = "seaorm",
            db.schema = "daedalus",
            persistence.enabled = persistence,
            auth.provider = "supabase",
            auth.enabled = auth,
            ui.stack = "maud+htmx",
            "daedalus web server listening"
        );
    }
}
