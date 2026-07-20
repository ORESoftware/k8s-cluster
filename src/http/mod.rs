//! The axum surface.
//!
//! Routes, mounted under the cluster gateway at `/shared-auth/`:
//! - `GET  /healthz`                    liveness
//! - `GET  /readyz`                     readiness (DB ping if configured)
//! - `GET  /.well-known/jwks.json`      our public JWKS (downstream verifiers)
//! - `POST /auth/exchange`              Supabase access token → OreSoftware JWT
//! - `POST /auth/introspect`            validate an OreSoftware JWT → claims
//! - `GET  /auth/verify`                lightweight bearer check (gateway auth_request)
//! - `GET  /metrics`                    Prometheus

mod exchange;
mod health;
mod introspect;
mod jwks;
mod metrics;

use std::time::Duration;

use axum::{
    routing::{get, post},
    Router,
};
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    let cors = build_cors(&state);

    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/.well-known/jwks.json", get(jwks::jwks))
        .route("/auth/exchange", post(exchange::exchange))
        .route("/auth/introspect", post(introspect::introspect))
        .route("/auth/verify", get(introspect::verify))
        .route("/metrics", get(metrics::metrics))
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::new(Duration::from_secs(10)))
        .layer(RequestBodyLimitLayer::new(16 * 1024))
        .layer(cors)
        .with_state(state)
}

fn build_cors(state: &AppState) -> CorsLayer {
    if state.config.cors_allow_origins.is_empty() {
        return CorsLayer::new();
    }
    let origins = state
        .config
        .cors_allow_origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect::<Vec<_>>();
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods(Any)
        .allow_headers(Any)
}

/// Extract a bearer token from the `Authorization` header.
pub(crate) fn bearer(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|t| !t.is_empty())
}
