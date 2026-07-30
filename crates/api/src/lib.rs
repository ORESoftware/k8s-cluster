//! t2v-api library surface: the axum `Router` builder and shared modules,
//! exposed so integration tests can drive the app without binding a socket or
//! contacting external providers. `main.rs` is a thin wrapper over this.

pub mod audio_io;
pub mod auth;
pub mod db;
pub mod error;
pub mod handlers_speech;
pub mod handlers_vapi;
pub mod history;
pub mod metrics;
pub mod state;
pub mod vapi_client;

use axum::extract::DefaultBodyLimit;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::Router;
use state::AppState;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

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

pub fn app(state: AppState) -> Router {
    // Audio uploads: larger body limit; run through the custom-FFT DSP core.
    let audio = Router::new()
        .route("/v1/stt", post(handlers_speech::stt))
        .route("/v1/analyze", post(handlers_speech::analyze_audio))
        .route(
            "/v1/speech-to-speech",
            post(handlers_speech::speech_to_speech),
        )
        .layer(DefaultBodyLimit::max(MAX_AUDIO_BODY));

    // JSON action endpoints.
    let json_actions = Router::new()
        .route("/v1/tts", post(handlers_speech::tts))
        .route("/v1/translate", post(handlers_speech::translate))
        .layer(DefaultBodyLimit::max(MAX_JSON_BODY));

    // Vapi webhook: authenticated inside the handler via x-vapi-secret.
    let webhook = Router::new()
        .route("/vapi/webhook", post(handlers_vapi::webhook))
        .layer(DefaultBodyLimit::max(MAX_JSON_BODY));

    // Operator + history endpoints: guarded by the server-auth bearer secret,
    // and they expose stored transcripts/translations, so they fail closed.
    let protected = Router::new()
        .route("/v1/history/transcriptions", get(history::transcriptions))
        .route("/v1/history/translations", get(history::translations))
        .route("/v1/history/syntheses", get(history::syntheses))
        .route("/v1/history/vapi-calls", get(history::vapi_calls))
        .route("/vapi/call", post(handlers_vapi::create_call))
        .route("/vapi/call/{id}", get(handlers_vapi::get_call))
        .layer(from_fn_with_state(state.clone(), auth::require_server_auth))
        .layer(DefaultBodyLimit::max(MAX_JSON_BODY));

    let public = Router::new()
        .route("/", get(banner))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics_handler));

    public
        .merge(audio)
        .merge(json_actions)
        .merge(webhook)
        .merge(protected)
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            request_timeout(),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn banner() -> &'static str {
    "t2v-api — voice-to-text / text-to-voice / translation. See GET /healthz, POST /v1/stt|tts|translate|speech-to-speech|analyze.\n"
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
