use std::{
    collections::{BTreeMap, HashSet},
    env,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, DbErr, QueryResult,
    Statement, TryGetable,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use sha2::{Digest, Sha256};
use tower_http::{
    limit::RequestBodyLimitLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

const MAX_BODY_BYTES: usize = 64 * 1024;
const MIN_AMOUNT_MINOR: i64 = 50;
const MAX_AMOUNT_MINOR: i64 = 100_000_000;
const DEFAULT_STRIPE_API_VERSION: &str = "2026-04-22.dahlia";

type SharedCheckoutState = Arc<CheckoutApiState>;

#[derive(Clone)]
struct CheckoutApiState {
    db: DatabaseConnection,
    http: reqwest::Client,
    cfg: Arc<CheckoutConfig>,
}

#[derive(Clone, Debug)]
struct CheckoutConfig {
    host: IpAddr,
    port: u16,
    database_url: String,
    api_bearer: String,
    allowed_tenants: HashSet<Uuid>,
    stripe_api_key: String,
    stripe_api_version: String,
    stripe_api_base: String,
    return_url_prefixes: Vec<AllowedReturnPrefix>,
    checkout_url_hosts: HashSet<String>,
}

impl CheckoutConfig {
    fn from_env() -> anyhow::Result<Self> {
        let _ = dotenvy::dotenv();

        let host = env::var("BILLING_CHECKOUT_HOST")
            .unwrap_or_else(|_| "0.0.0.0".to_string())
            .parse::<IpAddr>()
            .map_err(|error| anyhow::anyhow!("invalid BILLING_CHECKOUT_HOST: {error}"))?;
        let port = env::var("BILLING_CHECKOUT_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(8088);
        let database_url = env::var("BILLING_DATABASE_URL")
            .or_else(|_| env::var("DATABASE_URL"))
            .map_err(|_| anyhow::anyhow!("BILLING_DATABASE_URL or DATABASE_URL must be set"))?;

        let api_bearer = required_trimmed_env("BILLING_CHECKOUT_API_BEARER")?;
        if !(32..=4_096).contains(&api_bearer.len()) {
            anyhow::bail!("BILLING_CHECKOUT_API_BEARER must contain 32..=4096 bytes");
        }

        let allowed_tenants = parse_uuid_set_env("BILLING_CHECKOUT_ALLOWED_TENANT_IDS")?;
        if allowed_tenants.is_empty() {
            anyhow::bail!("BILLING_CHECKOUT_ALLOWED_TENANT_IDS must contain at least one UUID");
        }

        let stripe_api_key = optional_trimmed_env("STRIPE_API_KEY")
            .or_else(|| optional_trimmed_env("STRIPE_CLIENT_SECRET"))
            .ok_or_else(|| anyhow::anyhow!("STRIPE_API_KEY or STRIPE_CLIENT_SECRET must be set"))?;
        if stripe_api_key.len() < 16 {
            anyhow::bail!("Stripe API credentials are unexpectedly short");
        }
        let stripe_api_version = env::var("STRIPE_API_VERSION")
            .unwrap_or_else(|_| DEFAULT_STRIPE_API_VERSION.to_string())
            .trim()
            .to_string();
        if stripe_api_version.is_empty() || stripe_api_version.len() > 64 {
            anyhow::bail!("STRIPE_API_VERSION must contain 1..=64 bytes");
        }
        let stripe_api_base = env::var("BILLING_STRIPE_API_BASE")
            .unwrap_or_else(|_| "https://api.stripe.com".to_string());
        validate_service_base_url(&stripe_api_base, "BILLING_STRIPE_API_BASE")?;
        let stripe_api_base = stripe_api_base.trim_end_matches('/').to_string();

        let return_url_prefixes = parse_csv_env("BILLING_CHECKOUT_RETURN_URL_PREFIXES")
            .into_iter()
            .map(|value| AllowedReturnPrefix::parse(&value))
            .collect::<Result<Vec<_>, _>>()?;
        if return_url_prefixes.is_empty() {
            anyhow::bail!(
                "BILLING_CHECKOUT_RETURN_URL_PREFIXES must contain at least one HTTPS URL prefix"
            );
        }

        let checkout_url_hosts = {
            let configured = parse_csv_env("BILLING_CHECKOUT_ALLOWED_HOSTS");
            let values = if configured.is_empty() {
                vec!["checkout.stripe.com".to_string()]
            } else {
                configured
            };
            values
                .into_iter()
                .map(|host| normalize_allowed_host(&host))
                .collect::<Result<HashSet<_>, _>>()?
        };

        Ok(Self {
            host,
            port,
            database_url,
            api_bearer,
            allowed_tenants,
            stripe_api_key,
            stripe_api_version,
            stripe_api_base,
            return_url_prefixes,
            checkout_url_hosts,
        })
    }
}

#[derive(Debug)]
enum CheckoutError {
    BadRequest(String),
    Unauthorized,
    NotFound,
    Conflict(String),
    ProviderUnavailable(String),
    Database(DbErr),
    Internal(String),
}

impl From<DbErr> for CheckoutError {
    fn from(value: DbErr) -> Self {
        Self::Database(value)
    }
}

impl From<serde_json::Error> for CheckoutError {
    fn from(value: serde_json::Error) -> Self {
        Self::Internal(value.to_string())
    }
}

impl IntoResponse for CheckoutError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, "bad_request", message.as_str()),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized", "unauthorized"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found", "not found"),
            Self::Conflict(message) => (StatusCode::CONFLICT, "conflict", message.as_str()),
            Self::ProviderUnavailable(_) => (
                StatusCode::BAD_GATEWAY,
                "payment_provider_unavailable",
                "payment provider unavailable",
            ),
            Self::Database(_) | Self::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "internal server error",
            ),
        };

        match &self {
            Self::ProviderUnavailable(detail) => {
                tracing::warn!(%detail, "hosted checkout provider request failed");
            }
            Self::Database(error) => tracing::error!(%error, "checkout database operation failed"),
            Self::Internal(error) => tracing::error!(%error, "checkout internal operation failed"),
            _ => {}
        }

        let mut response = (status, Json(json!({ "error": code, "message": message }))).into_response();
        if status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer realm=\"quaestor-checkout\""),
            );
        }
        response
    }
}

pub async fn run() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .try_init();

    let cfg = Arc::new(CheckoutConfig::from_env()?);
    let mut options = ConnectOptions::new(cfg.database_url.clone());
    options
        .max_connections(16)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(300))
        .sqlx_logging(true)
        .sqlx_logging_level(log::LevelFilter::Debug);
    let db = Database::connect(options).await?;
    let http = reqwest::Client::builder()
        .user_agent("quaestor-ledger-checkout/1")
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let state = Arc::new(CheckoutApiState { db, http, cfg });

    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route(
            "/internal/v1/tenants/{tenant_id}/checkout-sessions",
            post(create_checkout_session),
        )
        .route(
            "/internal/v1/tenants/{tenant_id}/checkout-sessions/{session_id}",
            get(get_checkout_session),
        )
        .with_state(state.clone())
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TimeoutLayer::new(Duration::from_secs(35)))
        .layer(TraceLayer::new_for_http());

    let address = SocketAddr::new(state.cfg.host, state.cfg.port);
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, "Quaestor hosted checkout API listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn health() -> Json<JsonValue> {
    Json(json!({ "ok": true, "service": "quaestor-checkout" }))
}

async fn ready(State(state): State<SharedCheckoutState>) -> Result<Json<JsonValue>, CheckoutError> {
    state
        .db
        .query_one(Statement::from_string(DbBackend::Postgres, "SELECT 1".to_string()))
        .await?;
    Ok(Json(json!({ "ready": true })))
}

fn required_trimmed_env(name: &str) -> anyhow::Result<String> {
    optional_trimmed_env(name).ok_or_else(|| anyhow::anyhow!("{name} must be set"))
}

fn optional_trimmed_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_csv_env(name: &str) -> Vec<String> {
    env::var(name)
        .ok()
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn parse_uuid_set_env(name: &str) -> anyhow::Result<HashSet<Uuid>> {
    parse_csv_env(name)
        .into_iter()
        .map(|value| {
            value
                .parse::<Uuid>()
                .map_err(|error| anyhow::anyhow!("invalid UUID in {name}: {error}"))
        })
        .collect()
}

fn normalize_allowed_host(value: &str) -> anyhow::Result<String> {
    let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 253
        || value.starts_with('.')
        || value.ends_with('.')
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-')
        })
    {
        anyhow::bail!("invalid checkout URL host allow-list entry: {value:?}");
    }
    Ok(value)
}

fn validate_service_base_url(value: &str, name: &str) -> anyhow::Result<()> {
    let parsed = reqwest::Url::parse(value)
        .map_err(|error| anyhow::anyhow!("invalid {name}: {error}"))?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        anyhow::bail!("{name} must not contain userinfo, a query, or a fragment");
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("{name} must contain a host"))?;
    let secure = parsed.scheme() == "https";
    let local = parsed.scheme() == "http" && matches!(host, "localhost" | "127.0.0.1" | "::1");
    if !secure && !local {
        anyhow::bail!("{name} must use HTTPS outside loopback development");
    }
    Ok(())
}
