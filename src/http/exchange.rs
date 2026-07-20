//! `POST /auth/exchange` — the core of the parallel/shortcut auth system.
//!
//! In: a Supabase access token (from any configured project), via the
//! `Authorization: Bearer …` header or a `{ "access_token": "…" }` body.
//! Steps: verify against the issuing project's JWKS → mirror the identity into
//! `shared_auth.users` (if the DB is configured) → mint a unified OreSoftware
//! JWT whose `sub` is the stable `shared_user_id`.
//! Out: the minted token.

use axum::{extract::State, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};

use crate::error::AuthError;
use crate::state::AppState;

use super::bearer;

#[derive(Debug, Deserialize)]
pub struct ExchangeRequest {
    access_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ExchangeResponse {
    access_token: String,
    token_type: &'static str,
    expires_at: u64,
    shared_user_id: String,
    project: String,
}

pub async fn exchange(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<ExchangeRequest>>,
) -> Result<Json<ExchangeResponse>, AuthError> {
    let token = bearer(&headers)
        .map(str::to_string)
        .or_else(|| body.and_then(|b| b.0.access_token))
        .ok_or(AuthError::BadRequest("missing Supabase access token"))?;

    // 1. Verify against the project that issued it.
    let identity = state.supabase.verify(&state.http, &token).await?;

    // 2. Mirror into RDS if configured; otherwise fall back to a deterministic
    //    stable id so the server is still usable DB-less.
    let shared_user_id = match &state.db {
        Some(db) => db
            .upsert_identity(&identity)
            .await?
            .shared_user_id
            .to_string(),
        None => format!("{}:{}", identity.project, identity.supabase_user_id),
    };

    // 3. Mint the unified OreSoftware token.
    let minted = state.minter.mint(
        &shared_user_id,
        &identity.project,
        &identity.supabase_user_id,
        identity.email.clone(),
        identity.email_verified,
    )?;

    tracing::info!(
        project = %identity.project,
        shared_user_id = %shared_user_id,
        "issued unified token"
    );

    Ok(Json(ExchangeResponse {
        access_token: minted.token,
        token_type: "Bearer",
        expires_at: minted.expires_at,
        shared_user_id,
        project: identity.project,
    }))
}
