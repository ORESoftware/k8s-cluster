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
use crate::{auth, db, devices, vault_blob};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::net::SocketAddr;
use crate::protocol::{PullResponse, PushRequest, PushResponse};
use tower_http::limit::RequestBodyLimitLayer;
use uuid::Uuid;

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
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/register", post(register))
        .route("/v1/login", post(login))
        .route("/v1/devices/revoke", post(revoke_device))
        .route("/v1/vault", get(pull_vault).post(push_vault))
        // Sealed blobs are small; cap bodies to 1 MiB to bound abuse.
        .layer(RequestBodyLimitLayer::new(1024 * 1024))
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
    if req.username.trim().is_empty() || req.password.len() < 8 {
        return Err(ApiError::BadRequest);
    }
    let secret = auth::hash_password(req.password.as_bytes())?;

    let account_id: Uuid = sqlx::query_scalar(
        "INSERT INTO accounts (username, auth_secret) VALUES ($1, $2) RETURNING id",
    )
    .bind(&req.username)
    .bind(&secret)
    .fetch_optional(&st.pool)
    .await?
    // A unique-violation (username taken) surfaces as no row → coarse error.
    .ok_or(ApiError::BadRequest)?;

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
