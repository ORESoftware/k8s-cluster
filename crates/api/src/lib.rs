//! t2v-api library surface: the Axum router, executable OpenAPI contract, and
//! shared modules. Integration and browser tests drive the same application
//! factory without contacting production providers.

pub mod audio_io;
pub mod auth;
pub mod db;
pub mod error;
pub mod handlers_speech;
pub mod handlers_vapi;
pub mod history;
pub mod http_contract;
pub mod metrics;
pub mod openapi;
pub mod state;
pub mod vapi_client;

use axum::extract::DefaultBodyLimit;
use axum::middleware::from_fn_with_state;
use axum::{Extension, Router};
use openapi::{ApiDocs, ApiDocuments};
use state::AppState;
use std::sync::Arc;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use utoipa_axum::{router::OpenApiRouter, routes};

/// Max request body for audio uploads — matches OpenAI Whisper's 25 MB cap and
/// bounds the DSP/FFT work a single request can trigger.
const MAX_AUDIO_BODY: usize = 25 * 1024 * 1024;
/// Max request body for JSON endpoints and the Vapi webhook.
const MAX_JSON_BODY: usize = 1024 * 1024;
/// Backstop request timeout (whole request, including a slow body). Generous so
/// the speech-to-speech pipeline is never cut off; tunable via env.
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 300;

fn request_timeout() -> Duration {
    let secs = std::env::var("T2V_REQUEST_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// Public routes are registered and documented together. The body-limit layer
/// remains scoped to the same audio/JSON groups as before this migration.
fn public_contract_router() -> OpenApiRouter<AppState> {
    let service_and_docs = OpenApiRouter::new()
        .routes(routes!(http_contract::banner))
        .routes(routes!(http_contract::healthz))
        .routes(routes!(http_contract::readyz))
        .routes(routes!(http_contract::public_openapi))
        .routes(routes!(http_contract::public_openapi_alias))
        .routes(routes!(http_contract::public_scalar))
        .routes(routes!(http_contract::public_scalar_alias));

    let audio = OpenApiRouter::new()
        .routes(routes!(http_contract::stt))
        .routes(routes!(http_contract::analyze_audio))
        .routes(routes!(http_contract::speech_to_speech))
        .layer(DefaultBodyLimit::max(MAX_AUDIO_BODY));

    let json_actions = OpenApiRouter::new()
        .routes(routes!(http_contract::tts))
        .routes(routes!(http_contract::translate))
        .layer(DefaultBodyLimit::max(MAX_JSON_BODY));

    service_and_docs.merge(audio).merge(json_actions)
}

/// Internal but unauthenticated-at-the-router routes. Prometheus remains
/// behavior-compatible with the existing deployment. Vapi validates its own
/// callback secret in the established constant-time handler.
fn internal_unprotected_contract_router() -> OpenApiRouter<AppState> {
    let operations = OpenApiRouter::new().routes(routes!(http_contract::metrics));
    let partner = OpenApiRouter::new()
        .routes(routes!(http_contract::vapi_webhook))
        .layer(DefaultBodyLimit::max(MAX_JSON_BODY));
    operations.merge(partner)
}

/// Operator routes are collected without middleware here so deterministic
/// exports require no state or secrets. `app` applies the live fail-closed
/// bearer middleware to this exact router before serving it.
fn operator_contract_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(http_contract::transcription_history))
        .routes(routes!(http_contract::translation_history))
        .routes(routes!(http_contract::synthesis_history))
        .routes(routes!(http_contract::vapi_call_history))
        .routes(routes!(http_contract::create_vapi_call))
        .routes(routes!(http_contract::get_vapi_call))
        .routes(routes!(http_contract::internal_openapi))
        .routes(routes!(http_contract::internal_scalar))
}

/// Build both documents without reading environment configuration, opening a
/// database, constructing provider clients, starting telemetry, or binding a
/// socket. This is the source for committed artifacts and generated SDKs.
pub fn openapi_documents() -> Result<ApiDocuments, String> {
    let (_, public) = public_contract_router().split_for_parts();
    let (_, internal_unprotected) = internal_unprotected_contract_router().split_for_parts();
    let (_, operator) = operator_contract_router().split_for_parts();
    openapi::finalize(public, internal_unprotected, operator)
}

pub fn app(state: AppState) -> Router {
    let documents = openapi_documents().expect("t2v executable OpenAPI contract must assemble");
    let docs = Arc::new(ApiDocs::new(&documents).expect("t2v API documents must serialize"));

    let (public, _) = public_contract_router().split_for_parts();
    let (internal_unprotected, _) = internal_unprotected_contract_router().split_for_parts();
    let (operator, _) = operator_contract_router().split_for_parts();
    let operator = operator
        .route_layer(from_fn_with_state(
            state.clone(),
            auth::require_server_auth,
        ))
        .layer(DefaultBodyLimit::max(MAX_JSON_BODY));

    public
        .merge(internal_unprotected)
        .merge(operator)
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            request_timeout(),
        ))
        .layer(TraceLayer::new_for_http())
        .layer(Extension(docs))
        .with_state(state)
}

async fn banner() -> &'static str {
    "t2v-api — voice-to-text / text-to-voice / translation. See GET /docs/api and GET /openapi.json for the public contract.\n"
}

async fn healthz() -> &'static str {
    "ok\n"
}

async fn readyz(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    match state.db.ping().await {
        Ok(_) => (StatusCode::OK, "ready\n").into_response(),
        Err(e) => {
            tracing::error!("readiness DB ping failed: {e}");
            (StatusCode::SERVICE_UNAVAILABLE, "not ready\n").into_response()
        }
    }
}

async fn metrics_handler(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> axum::response::Response {
    use axum::http::header;
    use axum::response::IntoResponse;
    metrics::Metrics::bump(&state.metrics.http_requests_total);
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.metrics.render(),
    )
        .into_response()
}

/// Test-only helpers to build an app over an isolated in-memory SQLite DB.
#[doc(hidden)]
pub mod testkit {
    use super::state::AppState;
    use axum::Router;
    use sea_orm::{ConnectOptions, Database};
    use sea_orm_migration::MigratorTrait;
    use std::sync::Arc;

    pub struct TestApp {
        pub state: AppState,
    }

    impl TestApp {
        pub fn app(&self) -> Router {
            super::app(self.state.clone())
        }

        /// Require this shared secret on the Vapi webhook.
        pub fn with_vapi_secret(mut self, secret: &str) -> Self {
            self.state.vapi_webhook_secret = Some(Arc::from(secret));
            self
        }

        /// Configure the server-auth bearer secret for operator/history routes.
        pub fn with_server_auth(mut self, secret: &str) -> Self {
            self.state.server_auth_secret = Some(Arc::from(secret));
            self
        }
    }

    /// Build an app backed by a fresh in-memory SQLite database with the
    /// schema migrated. A single kept-alive connection preserves the DB for
    /// the lifetime of the pool.
    pub async fn build_test_state() -> TestApp {
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
        TestApp {
            state: AppState::new(db),
        }
    }
}
