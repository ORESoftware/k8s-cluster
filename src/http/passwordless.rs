//! RDS-backed passwordless email login.
//!
//! Requests are enumeration-resistant: a syntactically valid address always
//! receives `202 Accepted`, whether or not signup is enabled or a matching
//! account exists. The link token and OTP are single-use and expire together.

use axum::{extract::State, http::StatusCode, Json};
use chrono::TimeDelta;
use serde::{Deserialize, Serialize};

use crate::db::AuthenticatedIdentity;
use crate::error::AuthError;
use crate::session::{
    hash_otp, hash_token, hashed_identifier, MagicLinkToken, MAGIC_LINK_TOKEN_PREFIX,
};
use crate::state::AppState;

use super::local::{enforce_limit, normalize_email, response_from_issued, SessionResponse};

#[derive(Deserialize)]
pub struct PasswordlessRequest {
    email: String,
}

#[derive(Serialize)]
pub struct PasswordlessAccepted {
    accepted: bool,
}

#[derive(Deserialize)]
pub struct PasswordlessConsumeRequest {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    otp: Option<String>,
}

pub async fn request(
    State(state): State<AppState>,
    Json(request): Json<PasswordlessRequest>,
) -> Result<(StatusCode, Json<PasswordlessAccepted>), AuthError> {
    request_magic_link(&state, &request.email, None).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(PasswordlessAccepted { accepted: true }),
    ))
}

pub(crate) async fn request_magic_link(
    state: &AppState,
    raw_email: &str,
    link_state: Option<&str>,
) -> Result<String, AuthError> {
    if !state.config.magic_links.is_enabled() {
        return Err(AuthError::Unavailable);
    }
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    let email = normalize_email(raw_email)?;
    // The identifier is hashed inside the limiter. This throttles repeated mail
    // sends without disclosing account existence or placing raw email addresses
    // in Redis keys/logs.
    enforce_limit(state, "passwordless", &email, 5, 900).await?;
    let token = MagicLinkToken::generate();
    let identifier_hash = hashed_identifier(&email);
    let pepper = state
        .config
        .magic_links
        .otp_pepper
        .as_deref()
        .ok_or(AuthError::Unavailable)?;
    let otp_hash = hash_otp(pepper, &email, &token.otp);
    let expires_at = chrono::Utc::now().fixed_offset()
        + TimeDelta::seconds(state.config.magic_links.ttl_secs as i64);
    let should_send = db
        .prepare_magic_link(
            &email,
            state.config.magic_links.allow_signup,
            &token.hash,
            &otp_hash,
            &identifier_hash,
            expires_at,
        )
        .await?;
    if should_send {
        crate::email::send_magic_link(
            &state.http,
            &state.config.magic_links,
            &email,
            &token.plaintext,
            &token.otp,
            link_state,
        )
        .await?;
    }
    Ok(email)
}

pub async fn consume(
    State(state): State<AppState>,
    Json(request): Json<PasswordlessConsumeRequest>,
) -> Result<Json<SessionResponse>, AuthError> {
    let identity = match (
        request.token.as_deref(),
        request.email.as_deref(),
        request.otp.as_deref(),
    ) {
        (Some(token), None, None) => consume_magic_link_identity(&state, token).await?,
        (None, Some(email), Some(otp)) => consume_email_otp_identity(&state, email, otp).await?,
        _ => {
            return Err(AuthError::BadRequest(
                "provide either token or email with six-digit otp",
            ))
        }
    };
    Ok(Json(response_from_issued(&state, identity).await?))
}

pub(crate) async fn consume_magic_link_identity(
    state: &AppState,
    token: &str,
) -> Result<AuthenticatedIdentity, AuthError> {
    if !token.starts_with(MAGIC_LINK_TOKEN_PREFIX) || token.len() > 128 {
        return Err(AuthError::Unauthorized);
    }
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    db.consume_magic_link(&hash_token(token)).await
}

pub(crate) async fn consume_email_otp_identity(
    state: &AppState,
    raw_email: &str,
    otp: &str,
) -> Result<AuthenticatedIdentity, AuthError> {
    if otp.len() != 6 || !otp.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AuthError::BadRequest("email otp must contain six digits"));
    }
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    let email = normalize_email(raw_email)?;
    let pepper = state
        .config
        .magic_links
        .otp_pepper
        .as_deref()
        .ok_or(AuthError::Unavailable)?;
    db.consume_email_otp(
        &hashed_identifier(&email),
        &hash_otp(pepper, &email, otp),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consume_body_supports_link_or_otp_without_ambiguity() {
        let link: PasswordlessConsumeRequest =
            serde_json::from_str(r#"{"token":"sat_magic_example"}"#).unwrap();
        assert!(link.token.is_some());
        assert!(link.email.is_none());

        let otp: PasswordlessConsumeRequest =
            serde_json::from_str(r#"{"email":"user@example.com","otp":"123456"}"#).unwrap();
        assert_eq!(otp.otp.as_deref(), Some("123456"));
        assert!(otp.token.is_none());
    }
}
