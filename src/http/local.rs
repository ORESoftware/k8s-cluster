//! Postgres-primary registration, login, refresh, and logout endpoints.

use axum::{extract::State, http::StatusCode, Json};
use chrono::TimeDelta;
use serde::{Deserialize, Serialize};

use crate::error::AuthError;
use crate::session::{hash_token, hashed_identifier, RefreshToken};
use crate::state::AppState;

use super::session_tokens;

/// Password byte bounds, shared by registration validation and the login
/// pre-hash guard so the two can never drift apart.
const MIN_PASSWORD_BYTES: usize = 12;
const MAX_PASSWORD_BYTES: usize = 1024;

#[derive(Deserialize)]
pub struct RegisterRequest {
    email: String,
    password: String,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Deserialize)]
pub struct RefreshRequest {
    refresh_token: String,
}

#[derive(Deserialize)]
pub struct LogoutRequest {
    refresh_token: String,
}

#[derive(Serialize)]
pub struct SessionResponse {
    access_token: String,
    token_type: &'static str,
    expires_at: u64,
    refresh_token: String,
    refresh_expires_at: u64,
    shared_user_id: String,
    provider: String,
    roles: Vec<String>,
    amr: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    acr: Option<String>,
}

pub async fn register(
    State(state): State<AppState>,
    Json(request): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<SessionResponse>), AuthError> {
    if !state.config.sessions.allow_registration {
        return Err(AuthError::Forbidden);
    }
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    let email = normalize_email(&request.email)?;
    validate_password(&request.password)?;
    let display_name = request
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    if display_name.as_ref().is_some_and(|name| name.len() > 160) {
        return Err(AuthError::BadRequest("display name is too long"));
    }
    enforce_limit(&state, "register", &email, 5, 3600).await?;

    let password_hash = crate::password::hash(request.password).await?;
    let identity = db
        .create_local_user(&email, display_name.as_deref(), &password_hash)
        .await?;
    let response = response_from_issued(&state, identity).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

pub async fn login(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<SessionResponse>, AuthError> {
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    let email = normalize_email(&request.email)?;

    // Registration never accepts more than MAX_PASSWORD_BYTES. Reject a value
    // that can never match before entering Argon2, while making the decision
    // solely from caller-supplied length so account existence is not disclosed.
    if request.password.len() > MAX_PASSWORD_BYTES {
        return Err(AuthError::BadRequest("password exceeds maximum length"));
    }

    enforce_limit(&state, "login", &email, 10, 900).await?;

    let credential = db.local_credential(&email).await?;
    let encoded_hash = credential
        .as_ref()
        .map(|credential| credential.password_hash.clone());
    let valid = crate::password::verify(request.password, encoded_hash).await?;
    let Some(credential) = credential else {
        return Err(AuthError::Unauthorized);
    };
    if credential.locked || !valid {
        if !credential.locked {
            db.record_login_result(credential.identity.shared_user_id, false)
                .await?;
        }
        return Err(AuthError::Unauthorized);
    }
    db.record_login_result(credential.identity.shared_user_id, true)
        .await?;
    Ok(Json(
        response_from_issued(&state, credential.identity).await?,
    ))
}

pub async fn refresh(
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> Result<Json<SessionResponse>, AuthError> {
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    validate_refresh_token(&request.refresh_token)?;
    let replacement = RefreshToken::generate();
    let expires_at = chrono::Utc::now().fixed_offset()
        + TimeDelta::seconds(state.config.sessions.refresh_ttl_secs as i64);
    let session = db
        .rotate_session(
            &hash_token(&request.refresh_token),
            &replacement.hash,
            expires_at,
        )
        .await?;
    let access = session_tokens::mint_for_session(&state, &session)?;
    Ok(Json(SessionResponse {
        access_token: access.token,
        token_type: "Bearer",
        expires_at: access.expires_at,
        refresh_token: replacement.plaintext,
        refresh_expires_at: expires_at.timestamp() as u64,
        shared_user_id: session.identity.shared_user_id.to_string(),
        provider: session.identity.provider,
        roles: session.identity.roles,
        amr: access.amr,
        acr: access.acr,
    }))
}

pub async fn logout(
    State(state): State<AppState>,
    Json(request): Json<LogoutRequest>,
) -> Result<StatusCode, AuthError> {
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    validate_refresh_token(&request.refresh_token)?;
    if let Some(session_id) = db
        .revoke_by_refresh_hash(&hash_token(&request.refresh_token))
        .await?
    {
        if let Some(cache) = &state.cache {
            if let Err(error) = cache
                .mark_revoked(session_id, state.config.signing.ttl_secs)
                .await
            {
                tracing::warn!(%error, %session_id, "failed to cache session revocation");
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn response_from_issued(
    state: &AppState,
    identity: crate::db::AuthenticatedIdentity,
) -> Result<SessionResponse, AuthError> {
    let method = match identity.provider.as_str() {
        "local" => "password",
        "magic_link" => "email",
        "supabase" => "federated",
        _ => "external",
    };
    response_from_issued_with_assurance(state, identity, 1, vec![method.to_owned()]).await
}

pub(crate) async fn response_from_issued_with_assurance(
    state: &AppState,
    identity: crate::db::AuthenticatedIdentity,
    auth_level: u8,
    auth_methods: Vec<String>,
) -> Result<SessionResponse, AuthError> {
    let shared_user_id = identity.shared_user_id.to_string();
    let provider = identity.provider.clone();
    let roles = identity.roles.clone();
    let issued = session_tokens::issue_with_assurance(
        state,
        identity,
        crate::token::AuthenticationAssurance::from_level_and_methods(auth_level, &auth_methods),
    )
    .await?;
    Ok(SessionResponse {
        access_token: issued.access.token,
        token_type: "Bearer",
        expires_at: issued.access.expires_at,
        refresh_token: issued.refresh_token.ok_or(AuthError::Internal)?,
        refresh_expires_at: issued.refresh_expires_at.ok_or(AuthError::Internal)?,
        shared_user_id,
        provider,
        roles,
        amr: issued.access.amr,
        acr: issued.access.acr,
    })
}

pub(crate) async fn enforce_limit(
    state: &AppState,
    bucket: &str,
    identifier: &str,
    limit: u64,
    window_secs: u64,
) -> Result<(), AuthError> {
    let Some(cache) = &state.cache else {
        return Ok(());
    };
    match cache
        .allow(bucket, &hashed_identifier(identifier), limit, window_secs)
        .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(AuthError::RateLimited),
        Err(error) => {
            tracing::warn!(%error, "Redis rate limit unavailable; falling back to DB lockout");
            Ok(())
        }
    }
}

pub(crate) fn normalize_email(input: &str) -> Result<String, AuthError> {
    let email = input.trim().to_ascii_lowercase();
    let mut parts = email.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    if email.len() > 320
        || local.is_empty()
        || domain.is_empty()
        || parts.next().is_some()
        || email.chars().any(char::is_whitespace)
        || !domain.contains('.')
    {
        return Err(AuthError::BadRequest("invalid email"));
    }
    Ok(email)
}

fn validate_password(password: &str) -> Result<(), AuthError> {
    if !(MIN_PASSWORD_BYTES..=MAX_PASSWORD_BYTES).contains(&password.len()) {
        return Err(AuthError::BadRequest(
            "password must be between 12 and 1024 bytes",
        ));
    }
    Ok(())
}

fn validate_refresh_token(token: &str) -> Result<(), AuthError> {
    if token.starts_with(crate::session::REFRESH_TOKEN_PREFIX) && token.len() <= 128 {
        Ok(())
    } else {
        Err(AuthError::Unauthorized)
    }
}
