use super::{AuthContext, CredentialSource};
use crate::{error::AppError, AppState};
use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, HeaderMap},
};
use axum_extra::extract::cookie::CookieJar;

pub struct Authenticated(pub AuthContext);
pub struct SessionAuthenticated(pub AuthContext);
pub struct OptionalAuthenticated(pub Option<AuthContext>);

impl FromRequestParts<AppState> for Authenticated {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(authenticate(parts, state, true).await?))
    }
}

impl FromRequestParts<AppState> for SessionAuthenticated {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(authenticate(parts, state, false).await?))
    }
}

impl FromRequestParts<AppState> for OptionalAuthenticated {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match authenticate(parts, state, true).await {
            Ok(context) => Ok(Self(Some(context))),
            Err(AppError::Unauthorized) => Ok(Self(None)),
            Err(error) => Err(error),
        }
    }
}

async fn authenticate(
    parts: &Parts,
    state: &AppState,
    allow_bearer: bool,
) -> Result<AuthContext, AppError> {
    if let Some(value) = parts.headers.get(header::AUTHORIZATION) {
        if !allow_bearer {
            return Err(AppError::Unauthorized);
        }
        let value = value.to_str().map_err(|_| AppError::Unauthorized)?;
        let token = value
            .strip_prefix("Bearer ")
            .filter(|token| !token.is_empty())
            .ok_or(AppError::Unauthorized)?;
        // An invalid bearer never falls back to a cookie.
        // Fail closed before calling `/auth/v1/user` when this process has
        // exhausted its bounded verification capacity. The permit spans the
        // upstream call and local revocation check, and no token-derived data
        // is included in the stable 429 response.
        let _permit = state
            .bearer_auth_semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| AppError::AuthBusy)?;
        return state
            .sessions
            .authenticate_bearer(token)
            .await
            .map_err(AppError::from);
    }

    let jar = CookieJar::from_headers(&parts.headers);
    let raw_id = jar
        .get(&state.config.session_cookie)
        .map(|cookie| cookie.value())
        .ok_or(AppError::Unauthorized)?;
    state
        .sessions
        .authenticate(raw_id)
        .await
        .map_err(AppError::from)
}

pub fn require_origin(headers: &HeaderMap, state: &AppState) -> Result<(), AppError> {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .ok_or(AppError::Forbidden)?;
    if state.config.allowed_origins.contains(origin) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

pub fn require_csrf(
    context: &AuthContext,
    headers: &HeaderMap,
    form_token: Option<&str>,
) -> Result<(), AppError> {
    if context.source == CredentialSource::Bearer {
        return Ok(());
    }
    let supplied = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .or(form_token)
        .ok_or(AppError::Forbidden)?;
    if context.csrf_token.as_deref() == Some(supplied) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}
