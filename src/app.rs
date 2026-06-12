//! HTTP surface: router, shared state, and request handlers.
//!
//! Routes:
//!   POST /v1/register        -> create account + first device, returns token
//!   POST /v1/login           -> verify account, register a device, returns token
//!   POST /v1/devices/revoke  -> revoke a device   (auth)
//!   GET  /v1/vault           -> pull sealed blob   (auth)
//!   POST /v1/vault           -> push sealed blob   (auth)
//!   GET  /healthz            -> liveness

use crate::error::ApiError;
use crate::ratelimit::{self, RateLimiter};
use crate::{auth, db, devices, vault_blob};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use crate::protocol::{PullResponse, PushRequest, PushResponse};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

/// Hard caps on attacker-controlled string inputs (bytes). The body limit is
/// 1 MiB; these stop a single field from being absurdly large before it ever
/// reaches Argon2 or the database.
const MAX_USERNAME_LEN: usize = 256;
const MAX_DEVICE_NAME_LEN: usize = 256;
const MAX_PASSWORD_LEN: usize = 1024;
/// Per-request wall-clock budget. Bounds slow/stuck handlers.
const REQUEST_TIMEOUT_SECS: u64 = 15;
/// Default per-IP, per-minute budget for the auth endpoints (login/register).
/// Override at runtime with `RATE_LIMIT_AUTH_PER_MIN`.
const DEFAULT_AUTH_RATE_PER_MIN: u32 = 10;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL must be set (Postgres connection string)")?;
    let bind = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());

    let pool = db::connect(&database_url).await?;
    let state = AppState { pool };

    let app = router(state);
    let addr: SocketAddr = bind.parse()?;
    tracing::info!(%addr, "3FA sync server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    // `into_make_service_with_connect_info` exposes the TCP peer address to the
    // rate-limit middleware (used as the client-IP fallback behind the ingress).
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

pub fn router(state: AppState) -> Router {
    let auth_rate = ratelimit::limit_from_env("RATE_LIMIT_AUTH_PER_MIN", DEFAULT_AUTH_RATE_PER_MIN);
    let auth_limiter = Arc::new(RateLimiter::new(auth_rate, Duration::from_secs(60)));

    // The unauthenticated, password-handling endpoints. Rate-limited per client
    // IP so online brute force / registration flooding is bounded on top of
    // Argon2's per-attempt cost. Authenticated routes are gated by a 256-bit
    // bearer token (infeasible to guess) so they don't need the same throttle.
    let auth_routes = Router::new()
        .route("/v1/register", post(register))
        .route("/v1/login", post(login))
        .route_layer(axum::middleware::from_fn(move |
            ConnectInfo(peer): ConnectInfo<SocketAddr>,
            req: axum::http::Request<axum::body::Body>,
            next: axum::middleware::Next,
        | {
            let limiter = auth_limiter.clone();
            async move {
                let ip = ratelimit::client_ip(req.headers(), peer);
                if limiter.check(ip) {
                    Ok(next.run(req).await)
                } else {
                    Err(ApiError::TooManyRequests)
                }
            }
        }));

    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(auth_routes)
        .route("/v1/devices/revoke", post(revoke_device))
        .route("/v1/vault", get(pull_vault).post(push_vault))
        // Outermost-to-innermost: request log, body cap, then a hard timeout.
        .layer(TraceLayer::new_for_http())
        // Sealed blobs are small; cap bodies to 1 MiB to bound abuse.
        .layer(RequestBodyLimitLayer::new(1024 * 1024))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        ))
        .with_state(state)
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}

// ---- DTOs ----

#[derive(Deserialize)]
struct CredsRequest {
    username: String,
    /// The account password. Used only to derive the Argon2id verifier (and, in
    /// the OPAQUE upgrade, never sent at all). Never stored in plaintext.
    password: String,
    device_name: String,
}

#[derive(Serialize)]
struct TokenResponse {
    account_id: Uuid,
    device_id: Uuid,
    /// Bearer token — shown once. Lost tokens require re-login.
    sync_token: String,
}

#[derive(Deserialize)]
struct RevokeRequest {
    device_id: Uuid,
}

// ---- handlers ----

async fn register(
    State(st): State<AppState>,
    Json(req): Json<CredsRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    if req.username.trim().is_empty()
        || req.username.len() > MAX_USERNAME_LEN
        || req.device_name.len() > MAX_DEVICE_NAME_LEN
        || !(8..=MAX_PASSWORD_LEN).contains(&req.password.len())
    {
        return Err(ApiError::BadRequest);
    }
    let secret = auth::hash_password(req.password.as_bytes())?;

    // A duplicate username trips the UNIQUE constraint, which sqlx surfaces as a
    // database error (NOT an empty row set). Map that to a coarse 409 instead of
    // letting it bubble up as a 500 with an error-log entry on every retry.
    let account_id: Uuid = match sqlx::query_scalar(
        "INSERT INTO accounts (username, auth_secret) VALUES ($1, $2) RETURNING id",
    )
    .bind(&req.username)
    .bind(&secret)
    .fetch_one(&st.pool)
    .await
    {
        Ok(id) => id,
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            return Err(ApiError::Conflict);
        }
        Err(e) => return Err(e.into()),
    };

    let (device_id, token) = devices::register(&st.pool, account_id, &req.device_name).await?;
    Ok(Json(TokenResponse {
        account_id,
        device_id,
        sync_token: token,
    }))
}

async fn login(
    State(st): State<AppState>,
    Json(req): Json<CredsRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    // Bound the inputs before any DB or Argon2 work. These checks are
    // account-independent, so they add no username-enumeration signal.
    if req.username.len() > MAX_USERNAME_LEN
        || req.device_name.len() > MAX_DEVICE_NAME_LEN
        || req.password.len() > MAX_PASSWORD_LEN
    {
        return Err(ApiError::BadRequest);
    }
    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, auth_secret FROM accounts WHERE username = $1")
            .bind(&req.username)
            .fetch_optional(&st.pool)
            .await?;

    // Always run exactly one Argon2 verify to avoid a username-enumeration
    // timing oracle, whether or not the account exists.
    let (account_id, secret) = row.unwrap_or_else(|| (Uuid::nil(), dummy_phc().to_string()));
    let ok = auth::verify_password(req.password.as_bytes(), &secret);
    if !ok || account_id.is_nil() {
        return Err(ApiError::Unauthorized);
    }

    let (device_id, token) = devices::register(&st.pool, account_id, &req.device_name).await?;
    Ok(Json(TokenResponse {
        account_id,
        device_id,
        sync_token: token,
    }))
}

async fn revoke_device(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RevokeRequest>,
) -> Result<(), ApiError> {
    let who = auth::authenticate(&st.pool, &headers).await?;
    devices::revoke(&st.pool, who.account_id, req.device_id).await
}

async fn pull_vault(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<PullResponse>, ApiError> {
    let who = auth::authenticate(&st.pool, &headers).await?;
    Ok(Json(vault_blob::load(&st.pool, who.account_id).await?))
}

async fn push_vault(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PushRequest>,
) -> Result<Json<PushResponse>, ApiError> {
    let who = auth::authenticate(&st.pool, &headers).await?;
    Ok(Json(vault_blob::store(&st.pool, who, &req).await?))
}

/// A valid Argon2id PHC string, computed once, to verify against when the
/// username is unknown — so login timing doesn't reveal account existence.
fn dummy_phc() -> &'static str {
    use std::sync::OnceLock;
    static D: OnceLock<String> = OnceLock::new();
    D.get_or_init(|| {
        auth::hash_password(b"3fa-dummy-account-not-real").expect("dummy hash")
    })
}
