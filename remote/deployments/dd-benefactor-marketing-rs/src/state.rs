use std::{
    env,
    sync::{atomic::AtomicU64, Arc},
    time::Instant,
};

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use sea_orm::{DatabaseConnection, DbErr};
use serde_json::json;
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::warn;
pub(crate) const SERVICE_NAME: &str = "dd-benefactor-marketing-rs";
pub(crate) const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
const DEFAULT_PORT: u16 = 8134;
pub(crate) const DEFAULT_LIMIT: u64 = 50;
pub(crate) const MAX_LIMIT: u64 = 200;
const DEFAULT_CACHE_TTL_SECONDS: u64 = 120;
const DEFAULT_RATE_LIMIT_PER_MINUTE: u64 = 600;
pub(crate) const DEFAULT_JOB_STREAM: &str = "benefactor:marketing:jobs";

pub(crate) type AppResult<T> = Result<T, AppError>;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) cfg: Arc<Config>,
    pub(crate) db: DatabaseConnection,
    pub(crate) redis: Option<redis::Client>,
    pub(crate) redis_connection: Arc<Mutex<Option<redis::aio::MultiplexedConnection>>>,
    pub(crate) metrics: Arc<Metrics>,
    pub(crate) started_at: Instant,
}

#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) database_url: String,
    pub(crate) api_auth_bearer: Option<String>,
    pub(crate) allow_unauthenticated: bool,
    pub(crate) scraper_base_url: Option<String>,
    pub(crate) redis_url: Option<String>,
    pub(crate) redis_required_for_ready: bool,
    pub(crate) cache_ttl_seconds: u64,
    pub(crate) rate_limit_per_minute: u64,
    pub(crate) job_stream: String,
}

#[derive(Default)]
pub(crate) struct Metrics {
    pub(crate) mutations_total: AtomicU64,
    pub(crate) enrichment_jobs_total: AtomicU64,
    pub(crate) lead_imports_total: AtomicU64,
    pub(crate) auth_failures_total: AtomicU64,
    pub(crate) db_errors_total: AtomicU64,
    pub(crate) redis_errors_total: AtomicU64,
    pub(crate) cache_hits_total: AtomicU64,
    pub(crate) cache_misses_total: AtomicU64,
    pub(crate) cache_invalidations_total: AtomicU64,
    pub(crate) rate_limit_rejections_total: AtomicU64,
    pub(crate) redis_jobs_published_total: AtomicU64,
    pub(crate) integration_sync_runs_total: AtomicU64,
    pub(crate) outreach_touchpoints_total: AtomicU64,
    pub(crate) research_briefs_total: AtomicU64,
    pub(crate) conversion_events_total: AtomicU64,
    pub(crate) client_collaboration_events_total: AtomicU64,
    pub(crate) agency_finance_records_total: AtomicU64,
    pub(crate) call_insights_total: AtomicU64,
}

#[derive(Debug, Error)]
pub(crate) enum AppError {
    #[error("authentication required")]
    Unauthorized,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("rate limit exceeded")]
    RateLimited,
    #[error("database operation failed")]
    Database(#[from] DbErr),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match self {
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            AppError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        if matches!(self, AppError::Database(_)) {
            warn!(error = %self, "request failed");
        }
        let body = json!({
            "error": status.canonical_reason().unwrap_or("error"),
            "message": self.to_string(),
        });
        (status, Json(body)).into_response()
    }
}

impl Config {
    pub(crate) fn from_env() -> anyhow::Result<Self> {
        let host = env::var("BENEFACTOR_MARKETING_HOST")
            .or_else(|_| env::var("HOST"))
            .unwrap_or_else(|_| "0.0.0.0".to_string());
        let port = env::var("BENEFACTOR_MARKETING_PORT")
            .or_else(|_| env::var("PORT"))
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);
        let database_url = env::var("BENEFACTOR_MARKETING_DATABASE_URL")
            .or_else(|_| env::var("DATABASE_URL"))
            .map_err(|_| {
                anyhow::anyhow!("BENEFACTOR_MARKETING_DATABASE_URL or DATABASE_URL must be set")
            })?;
        let api_auth_bearer = env::var("BENEFACTOR_MARKETING_API_AUTH_BEARER")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let allow_unauthenticated = env_bool("BENEFACTOR_MARKETING_ALLOW_UNAUTHENTICATED", false);
        let scraper_base_url = env::var("BENEFACTOR_MARKETING_SCRAPER_BASE_URL")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let redis_url = env::var("BENEFACTOR_MARKETING_REDIS_URL")
            .or_else(|_| env::var("REDIS_URL"))
            .ok()
            .filter(|value| !value.trim().is_empty());
        let redis_required_for_ready =
            env_bool("BENEFACTOR_MARKETING_REDIS_REQUIRED_FOR_READY", false);
        let cache_ttl_seconds = env_u64(
            "BENEFACTOR_MARKETING_CACHE_TTL_SECONDS",
            DEFAULT_CACHE_TTL_SECONDS,
        );
        let rate_limit_per_minute = env_u64(
            "BENEFACTOR_MARKETING_RATE_LIMIT_PER_MINUTE",
            DEFAULT_RATE_LIMIT_PER_MINUTE,
        );
        let job_stream = env::var("BENEFACTOR_MARKETING_JOB_STREAM")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_JOB_STREAM.to_string());

        Ok(Self {
            host,
            port,
            database_url,
            api_auth_bearer,
            allow_unauthenticated,
            scraper_base_url,
            redis_url,
            redis_required_for_ready,
            cache_ttl_seconds,
            rate_limit_per_minute,
            job_stream,
        })
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}
