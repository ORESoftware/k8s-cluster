//! The axum surface.
//!
//! JSON API + a small Maud/htmx HTML UI. No websockets.
//! - `GET  /`                           status landing (HTML)
//! - `GET  /ui`                          token-exchange helper (HTML)
//! - `POST /ui/exchange`                 htmx fragment (HTML)
//! - `GET  /healthz`                     liveness
//! - `GET  /readyz`                      readiness (DB ping if configured)
//! - `GET  /.well-known/jwks.json`       our public JWKS (downstream verifiers)
//! - `POST /auth/exchange`               Supabase access token → OreSoftware JWT
//! - `POST /auth/introspect`             validate an OreSoftware JWT → claims
//! - `GET  /auth/verify`                 bearer check (gateway auth_request)
//! - `GET  /metrics`                     Prometheus

mod exchange;
mod health;
mod introspect;
mod jwks;
mod metrics;
mod ui;

use std::time::Duration;

use axum::{
    routing::{get, post},
    Router,
};
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

use crate::state::AppState;

// TimeoutLayer::new is deprecated in tower-http 0.6 in favour of
// with_status_code; new() still produces the same 408-on-timeout behaviour we
// want, so allow it rather than pin to the newer signature.
#[allow(deprecated)]
pub fn router(state: AppState) -> Router {
    let cors = build_cors(&state);

    Router::new()
        // HTML UI
        .route("/", get(ui::landing))
        .route("/ui", get(ui::sign_in))
        .route("/ui/exchange", post(ui::ui_exchange))
        // JSON API
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/.well-known/jwks.json", get(jwks::jwks))
        .route("/auth/exchange", post(exchange::exchange))
        .route("/auth/introspect", post(introspect::introspect))
        .route("/auth/verify", get(introspect::verify))
        .route("/metrics", get(metrics::metrics))
        // One tracing span per request (W3C traceparent → OTLP), then limits.
        .layer(crate::telemetry::http_trace_layer())
        .layer(TimeoutLayer::new(Duration::from_secs(10)))
        .layer(RequestBodyLimitLayer::new(64 * 1024))
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
