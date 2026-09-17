//! t2v-web library surface: the axum `Router` builder + shared modules, exposed
//! so integration tests (and the browser e2e harness) can drive the dashboard
//! without contacting the API server. `main.rs` is a thin wrapper over this.

pub mod assets;
pub mod db;
pub mod routes;
pub mod state;
pub mod views;

use axum::middleware::from_fn;
use axum::routing::get;
use axum::Router;
use state::AppState;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Backstop request timeout. The action proxy to t2v-api has its own 190s
/// client timeout; this bounds everything else (including slow request bodies).
pub const REQUEST_TIMEOUT_SECS: u64 = 200;

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(routes::dashboard))
        .route(
            "/translate",
            get(routes::translate_page).post(routes::translate_action),
        )
        .route("/speak", get(routes::speak_page).post(routes::speak_action))
        .route("/history", get(routes::history_page))
        .route("/ws/stats", get(routes::stats_ws))
        .route("/assets/htmx.min.js", get(assets::htmx_js))
        .route("/assets/htmx-ws.js", get(assets::htmx_ws_js))
        .route("/assets/app.css", get(assets::app_css))
        .route("/healthz", get(routes::healthz))
        .route("/readyz", get(routes::readyz))
        // Security headers on every response; a backstop timeout on every request.
        .layer(from_fn(routes::security_headers))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Test-only helper to build the app over an isolated in-memory SQLite DB.
#[doc(hidden)]
pub mod testkit {
    use super::state::AppState;
    use axum::Router;
    use sea_orm::{ConnectOptions, Database};
    use sea_orm_migration::MigratorTrait;

    /// Build the web app backed by a fresh in-memory SQLite database (schema
    /// migrated). A single kept-alive connection preserves the DB for the pool's
    /// lifetime.
    pub async fn build_test_app() -> Router {
        let mut opts = ConnectOptions::new("sqlite::memory:".to_string());
        opts.max_connections(1)
            .min_connections(1)
            .sqlx_logging(false);
        let db = Database::connect(opts)
            .await
            .expect("connect in-memory sqlite");
        t2v_migration::Migrator::up(&db, None)
            .await
            .expect("run migrations on in-memory sqlite");
        super::app(AppState::new(db))
    }
}
