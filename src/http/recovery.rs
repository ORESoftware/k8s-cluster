//! Account-recovery HTTP surface. All camera and microphone capture happens at
//! short-lived provider URLs; these JSON endpoints never accept biometric media.

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::AuthError;
use crate::recovery::{RecoveryCapabilities, RecoveryService, ReviewDecision};
use crate::state::AppState;

use super::bearer;
use super::introspect::active_claims;
use super::local::normalize_email;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentRequest {
    accepted_biometric_processing: bool,
    consent_version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryStartRequest {
    email: String,
    accepted_biometric_processing: bool,
    consent_version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CeremonyTokenRequest {
    ceremony_token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedeemRequest {
    ceremony_token: String,
    new_password: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAction {
    Approve,
    Reject,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRequest {
    decision: ReviewAction,
    reviewer: String,
}

pub async fn capabilities(State(state): State<AppState>) -> Json<RecoveryCapabilities> {
    Json(match state.recovery.as_ref() {
        Some(service) => service.capabilities(),
        None => RecoveryCapabilities {
            enabled: false,
            consent_version: String::new(),
            government_id_required: true,
            face_match_required: true,
            face_liveness_required: true,
            voice_liveness_required: true,
            voice_phrase_required: true,
            voice_speaker_match_advisory_only: true,
            automatic_recovery_requires_prior_identity_proofing: true,
            raw_biometrics_stored_by_shared_auth: false,
            cooldown_seconds: 0,
            manual_review_available: false,
        },
    })
}

pub async fn begin_enrollment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ConsentRequest>,
) -> Result<Response, AuthError> {
    let shared_user_id = current_aal2_user(&state, &headers).await?;
    let service = service(&state)?;
    service.validate_consent(
        request.accepted_biometric_processing,
        &request.consent_version,
    )?;
    let launch = service
        .begin_enrollment(shared_user_id, &request.consent_version)
        .await?;
    Ok(no_store((StatusCode::CREATED, Json(launch))))
}

pub async fn complete_enrollment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(ceremony_id): Path<String>,
    Json(request): Json<CeremonyTokenRequest>,
) -> Result<Response, AuthError> {
    let shared_user_id = current_aal2_user(&state, &headers).await?;
    let ceremony_id = parse_ceremony_id(&ceremony_id)?;
    Ok(no_store(Json(
        service(&state)?
            .evaluate(
                ceremony_id,
                &request.ceremony_token,
                "enrollment",
                Some(shared_user_id),
            )
            .await?,
    )))
}

pub async fn revoke_enrollment(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AuthError> {
    let shared_user_id = current_aal2_user(&state, &headers).await?;
    service(&state)?
        .revoke_enrollment(shared_user_id)
        .await?;
    Ok(no_store(StatusCode::NO_CONTENT))
}

pub async fn begin_recovery(
    State(state): State<AppState>,
    Json(request): Json<RecoveryStartRequest>,
) -> Result<Response, AuthError> {
    let service = service(&state)?;
    service.validate_consent(
        request.accepted_biometric_processing,
        &request.consent_version,
    )?;
    let email = normalize_email(&request.email)?;
    // A syntactically valid request receives the same response shape whether
    // the account exists, is unbound, or is already enrolled.
    let launch = service
        .begin_recovery(&email, &request.consent_version)
        .await?;
    Ok(no_store((StatusCode::ACCEPTED, Json(launch))))
}

pub async fn recovery_status(
    State(state): State<AppState>,
    Path(ceremony_id): Path<String>,
    Json(request): Json<CeremonyTokenRequest>,
) -> Result<Response, AuthError> {
    let ceremony_id = parse_ceremony_id(&ceremony_id)?;
    Ok(no_store(Json(
        service(&state)?
            .ceremony_status(ceremony_id, &request.ceremony_token, "recovery", None)
            .await?,
    )))
}

pub async fn complete_recovery(
    State(state): State<AppState>,
    Path(ceremony_id): Path<String>,
    Json(request): Json<CeremonyTokenRequest>,
) -> Result<Response, AuthError> {
    let ceremony_id = parse_ceremony_id(&ceremony_id)?;
    Ok(no_store(Json(
        service(&state)?
            .evaluate(ceremony_id, &request.ceremony_token, "recovery", None)
            .await?,
    )))
}

pub async fn redeem_recovery(
    State(state): State<AppState>,
    Path(ceremony_id): Path<String>,
    Json(request): Json<RedeemRequest>,
) -> Result<Response, AuthError> {
    let ceremony_id = parse_ceremony_id(&ceremony_id)?;
    service(&state)?
        .redeem(
            ceremony_id,
            &request.ceremony_token,
            request.new_password,
        )
        .await?;
    Ok(no_store(StatusCode::NO_CONTENT))
}

pub async fn review_recovery(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(ceremony_id): Path<String>,
    Json(request): Json<ReviewRequest>,
) -> Result<Response, AuthError> {
    let service = service(&state)?;
    service.authorize_reviewer(bearer(&headers))?;
    let ceremony_id = parse_ceremony_id(&ceremony_id)?;
    service
        .review(
            ceremony_id,
            match request.decision {
                ReviewAction::Approve => ReviewDecision::Approve,
                ReviewAction::Reject => ReviewDecision::Reject,
            },
            &request.reviewer,
        )
        .await?;
    Ok(no_store(StatusCode::NO_CONTENT))
}

fn no_store(response: impl IntoResponse) -> Response {
    let mut response = response.into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

fn service(state: &AppState) -> Result<&RecoveryService, AuthError> {
    state.recovery.as_ref().ok_or(AuthError::Unavailable)
}

async fn current_aal2_user(state: &AppState, headers: &HeaderMap) -> Result<Uuid, AuthError> {
    let claims = active_claims(state, bearer(headers).ok_or(AuthError::Unauthorized)?).await?;
    if claims.aal < 2 {
        return Err(AuthError::Forbidden);
    }
    Uuid::parse_str(&claims.sub).map_err(|_| AuthError::Unauthorized)
}

fn parse_ceremony_id(value: &str) -> Result<Uuid, AuthError> {
    Uuid::parse_str(value).map_err(|_| AuthError::BadRequest("invalid ceremony id"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceremony_ids_are_strict_uuids() {
        assert!(parse_ceremony_id("00000000-0000-0000-0000-000000000000").is_ok());
        assert!(parse_ceremony_id("not-a-uuid").is_err());
    }

    #[test]
    fn recovery_responses_disable_caching() {
        let response = no_store(StatusCode::NO_CONTENT);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store, max-age=0"
        );
    }
}
