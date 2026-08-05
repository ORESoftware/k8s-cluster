//! SMS second-factor enrollment and verification through Twilio Verify.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::TimeDelta;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::AuthenticatedIdentity;
use crate::error::AuthError;
use crate::state::AppState;

use super::bearer;
use super::introspect::active_claims;
use super::local::{enforce_limit, response_from_issued_with_assurance, SessionResponse};

#[derive(Deserialize)]
pub struct SmsChallengeRequest {
    #[serde(default)]
    phone: Option<String>,
}

#[derive(Serialize)]
pub struct SmsChallengeResponse {
    challenge_id: String,
    expires_at: u64,
    phone_hint: String,
}

#[derive(Deserialize)]
pub struct SmsVerifyRequest {
    challenge_id: String,
    code: String,
}

pub async fn request_sms(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SmsChallengeRequest>,
) -> Result<(StatusCode, Json<SmsChallengeResponse>), AuthError> {
    if !state.config.twilio_verify.is_enabled() {
        return Err(AuthError::Unavailable);
    }
    let claims = active_claims(&state, bearer(&headers).ok_or(AuthError::Unauthorized)?).await?;
    let shared_user_id = Uuid::parse_str(&claims.sub).map_err(|_| AuthError::Unauthorized)?;
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    let phone = match request.phone {
        Some(phone) => normalize_e164(&phone)?,
        None => db
            .phone_for_user(shared_user_id)
            .await?
            .ok_or(AuthError::BadRequest("phone is required for enrollment"))?,
    };
    // Toll-fraud guard: cap SMS enrollment sends per user and per destination
    // number before any paid Twilio call. Both identifiers are hashed inside
    // `enforce_limit`; when Redis is unconfigured this is a no-op, exactly like
    // the login/register limits. Without it an authenticated caller can pump
    // unbounded SMS to arbitrary numbers (SMS-pumping / cost abuse).
    enforce_limit(&state, "mfa_sms_user", &shared_user_id.to_string(), 3, 3600).await?;
    enforce_limit(&state, "mfa_sms_phone", &phone, 3, 3600).await?;
    let expires_at = chrono::Utc::now().fixed_offset() + TimeDelta::minutes(10);
    let challenge_id = db
        .create_sms_challenge(shared_user_id, &phone, expires_at)
        .await?;
    crate::twilio::start_sms_verification(&state.http, &state.config.twilio_verify, &phone).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(SmsChallengeResponse {
            challenge_id: challenge_id.to_string(),
            expires_at: expires_at.timestamp() as u64,
            phone_hint: mask_phone(&phone),
        }),
    ))
}

pub async fn verify_sms(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SmsVerifyRequest>,
) -> Result<Json<SessionResponse>, AuthError> {
    if !state.config.twilio_verify.is_enabled() {
        return Err(AuthError::Unavailable);
    }
    if !(4..=10).contains(&request.code.len())
        || !request.code.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(AuthError::BadRequest("invalid verification code"));
    }
    let claims = active_claims(&state, bearer(&headers).ok_or(AuthError::Unauthorized)?).await?;
    let shared_user_id = Uuid::parse_str(&claims.sub).map_err(|_| AuthError::Unauthorized)?;
    let challenge_id = Uuid::parse_str(&request.challenge_id)
        .map_err(|_| AuthError::BadRequest("invalid challenge id"))?;
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    let phone = db.sms_challenge_phone(shared_user_id, challenge_id).await?;
    if !crate::twilio::check_sms_verification(
        &state.http,
        &state.config.twilio_verify,
        &phone,
        &request.code,
    )
    .await?
    {
        return Err(AuthError::Unauthorized);
    }
    db.complete_sms_challenge(shared_user_id, challenge_id)
        .await?;

    let mut auth_methods = claims.amr;
    if !auth_methods.iter().any(|method| method == "sms") {
        auth_methods.push("sms".to_owned());
    }
    let identity = AuthenticatedIdentity {
        shared_user_id,
        provider: claims.provider,
        provider_tenant: claims.provider_tenant,
        provider_subject: claims.provider_subject,
        email: claims.email,
        email_verified: claims.email_verified,
        roles: claims.roles,
    };
    Ok(Json(
        response_from_issued_with_assurance(&state, identity, 2, auth_methods).await?,
    ))
}

fn normalize_e164(value: &str) -> Result<String, AuthError> {
    let phone = value.trim();
    if !(9..=16).contains(&phone.len())
        || !phone.starts_with('+')
        || phone.as_bytes().get(1).is_none_or(|byte| *byte == b'0')
        || !phone[1..].bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(AuthError::BadRequest("phone must use E.164 format"));
    }
    Ok(phone.to_owned())
}

fn mask_phone(phone: &str) -> String {
    let suffix = phone.chars().rev().take(4).collect::<Vec<_>>();
    format!("••••{}", suffix.into_iter().rev().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_validation_requires_e164() {
        assert_eq!(normalize_e164("+14155550100").unwrap(), "+14155550100");
        for invalid in ["4155550100", "+012345678", "+1 415 555 0100", "+123"] {
            assert!(normalize_e164(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn phone_hint_discloses_only_the_suffix() {
        assert_eq!(mask_phone("+14155550100"), "••••0100");
    }
}
