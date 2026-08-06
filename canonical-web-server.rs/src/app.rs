//! Application state and HTTP router assembly.

use std::sync::Arc;

use axum::{
    http::{header, HeaderName, HeaderValue},
    Router,
};
use tokio::sync::Semaphore;
use tower_http::{
    compression::CompressionLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    sensitive_headers::SetSensitiveRequestHeadersLayer,
    set_header::SetResponseHeaderLayer,
};

use crate::{
    auth::{self, AuthProvider},
    config::Config,
    database,
    error::AppError,
    metrics, routes, telemetry, ws,
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: sea_orm::DatabaseConnection,
    pub auth: Arc<dyn AuthProvider>,
    pub login_rate_limiter: auth::LoginRateLimiter,
    pub(crate) login_auth_semaphore: Arc<Semaphore>,
    pub sessions: auth::SessionService,
    pub hub: ws::Hub,
    pub(crate) bearer_auth_semaphore: Arc<Semaphore>,
}

impl AppState {
    pub fn new(
        config: Config,
        db: sea_orm::DatabaseConnection,
        auth: Arc<dyn AuthProvider>,
    ) -> Result<Self, AppError> {
        let config = Arc::new(config);
        let sessions = auth::SessionService::new(
            db.clone(),
            auth.clone(),
            &config.session_encryption_key,
            config.session_ttl,
        )?;
        let login_rate_limiter = auth::LoginRateLimiter::new(
            config.login_rate_limit_attempts,
            config.login_rate_limit_global_attempts,
            config.login_rate_limit_window,
            config.login_rate_limit_max_keys,
        );
        let login_auth_semaphore = Arc::new(Semaphore::new(config.login_auth_max_concurrency));
        let bearer_auth_semaphore = Arc::new(Semaphore::new(config.bearer_auth_max_concurrency));

        Ok(Self {
            config,
            db,
            auth,
            login_rate_limiter,
            login_auth_semaphore,
            sessions,
            hub: ws::Hub::new(256),
            bearer_auth_semaphore,
        })
    }
}

pub async fn build_state(config: Config) -> Result<AppState, AppError> {
    let db = database::connect(&config.database_url, config.database_max_connections).await?;
    // The long-lived customer process must fail closed unless it received the
    // exact non-owner, non-BYPASSRLS runtime identity. Schema changes are an
    // explicit `migrate` command with a separately mounted credential.
    crate::db::verify_runtime_database_role(&db).await?;

    #[cfg(feature = "test-auth")]
    if auth::test_provider::BrowserTestAuth::is_enabled() {
        tracing::warn!("browser-e2e test authentication provider enabled");
        return AppState::new(config, db, Arc::new(auth::test_provider::BrowserTestAuth));
    }

    let auth = Arc::new(auth::SupabaseAuth::new(
        config.supabase_url.clone(),
        config.supabase_publishable_key.clone(),
    )?);
    AppState::new(config, db, auth)
}

pub fn build_app(state: AppState) -> Router {
    let request_id_header = HeaderName::from_static("x-request-id");
    let app = telemetry::instrument_http(routes::router(state))
        .layer(axum::middleware::from_fn(metrics::record_http));

    app.layer((
        SetSensitiveRequestHeadersLayer::new([header::AUTHORIZATION, header::COOKIE]),
        SetRequestIdLayer::new(request_id_header.clone(), MakeRequestUuid),
        PropagateRequestIdLayer::new(request_id_header),
        SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ),
        SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ),
        SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ),
        SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static(
                "camera=(), geolocation=(), microphone=(), payment=(), usb=()",
            ),
        ),
        SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        ),
        // Browsers only honor HSTS when delivered over HTTPS. The public edge
        // is responsible for redirecting cleartext traffic first.
        SetResponseHeaderLayer::if_not_present(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        ),
        CompressionLayer::new(),
    ))
}
