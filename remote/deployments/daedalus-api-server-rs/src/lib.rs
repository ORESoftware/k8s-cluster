//! daedalus-api-server — JSON API over the `daedalus` Postgres schema.
//!
//! Surface:
//!   GET  /health              liveness, unauthenticated
//!   GET  /ready               readiness (database reachable), unauthenticated
//!   GET  /metrics             Prometheus text exposition, unauthenticated
//!   GET  /v1/plans            list the caller's fabrication plans
//!   POST /v1/plans            create a plan
//!   GET  /v1/plans/:id        fetch one plan
//!   GET  /v1/plans/:id/events websocket: live run/plan events for one plan
//!
//! Everything under /v1 requires a Supabase bearer token whose `email` claim is
//! on the allow-list (see supabase_auth). Ownership is enforced by filtering on
//! the verified email rather than trusting any client-supplied identifier —
//! this database has no RLS, so the server is the only authorization boundary.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};

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
mod ws;

pub const SERVICE_NAME: &str = "daedalus-api-server";

pub(crate) struct AppState {
    pub(crate) persistence: Persistence,
    pub(crate) verifier: Option<SupabaseVerifier>,
    pub(crate) http: reqwest::Client,
    pub(crate) metrics: metrics::Metrics,
    /// Broadcast bus for websocket fan-out. Bounded: a slow client lags and is
    /// disconnected rather than growing the buffer without limit.
    pub(crate) events: tokio::sync::broadcast::Sender<ws::PlanEvent>,
}

pub(crate) type SharedState = Arc<AppState>;

/// Axum extractor that yields a verified, allow-listed operator.
///
/// Placing the gate in an extractor means a route cannot accidentally skip it:
/// a handler either takes `Operator` and is authorized, or it does not and is
/// deliberately public.
#[axum::async_trait]
impl FromRequestParts<SharedState> for Operator {
    type Rejection = ServiceError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &SharedState,
    ) -> Result<Self, Self::Rejection> {
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
        let operator = verifier.authorize(&state.http, token).await?;
        state.metrics.record_authorized();
        Ok(operator)
    }
}

pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _guard = observability::init();
    let config = ServiceConfig::from_env()?;
    let persistence = Persistence::from_env().await?;
    let verifier = SupabaseVerifier::from_config(&config.supabase);

    if verifier.is_none() {
        // Loud, because the /v1 surface will refuse every request in this state.
        tracing::warn!(
            "Supabase auth is NOT configured (need a JWT secret or JWKS URL *and* \
             DAEDALUS_API_ALLOWED_EMAILS); authenticated routes will return 503"
        );
    }

    let (events, _) = tokio::sync::broadcast::channel(256);
    let state: SharedState = Arc::new(AppState {
        persistence,
        verifier,
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?,
        metrics: metrics::Metrics::new(),
        events,
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
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(routes::ready))
        .route("/metrics", get(routes::metrics))
        .route("/v1/plans", get(routes::list_plans).post(routes::create_plan))
        .route("/v1/plans/:id", get(routes::get_plan))
        .route("/v1/plans/:id/events", get(ws::plan_events))
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
            telemetry.logs = "stdout/loki",
            telemetry.traces = "otlp",
            telemetry.metrics = "prometheus",
            "daedalus api server listening"
        );
    }
}
