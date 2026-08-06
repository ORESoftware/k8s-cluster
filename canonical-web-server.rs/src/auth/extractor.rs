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
/// Accepts a Shared Auth bearer token, an existing Canonical session, or a
/// browser cookie independently verified by Shared Auth.
pub struct QuoteAuthenticated(pub AuthContext);
/// Browser-only quote authentication. Bearer headers are rejected so HTML
/// routes cannot accidentally change credential semantics.
pub struct QuoteSessionAuthenticated(pub AuthContext);

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

impl FromRequestParts<AppState> for QuoteAuthenticated {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(authenticate_quote(parts, state, true).await?))
    }
}

impl FromRequestParts<AppState> for QuoteSessionAuthenticated {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(authenticate_quote(parts, state, false).await?))
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
    if let Some(token) = bearer_token(&parts.headers)? {
        if !allow_bearer {
            return Err(AppError::Unauthorized);
        }
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

async fn authenticate_quote(
    parts: &Parts,
    state: &AppState,
    allow_bearer: bool,
) -> Result<AuthContext, AppError> {
    // Explicit Authorization has fail-closed precedence. Quote API bearer
    // tokens are always verified by the configured Shared Auth realm so
    // api.canonical.plus has the same tenant, issuer, audience, and revocation
    // boundary as the browser flow.
    if let Some(token) = bearer_token(&parts.headers)? {
        if !allow_bearer {
            return Err(AppError::Unauthorized);
        }
        let _permit = state
            .bearer_auth_semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| AppError::AuthBusy)?;
        return state.shared_auth.authenticate_bearer(token).await;
    }

    let jar = CookieJar::from_headers(&parts.headers);
    if jar.get(&state.config.session_cookie).is_some() {
        return authenticate(parts, state, false).await;
    }

    let token = jar
        .get(state.shared_auth.cookie_name())
        .map(|cookie| cookie.value())
        .ok_or(AppError::Unauthorized)?;
    let _permit = state
        .bearer_auth_semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::AuthBusy)?;
    state.shared_auth.authenticate_session(token).await
}

fn bearer_token(headers: &HeaderMap) -> Result<Option<&str>, AppError> {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| AppError::Unauthorized)?;
    let token = value
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
        .ok_or(AppError::Unauthorized)?;
    Ok(Some(token))
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

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn bearer_header_parsing_is_fail_closed() {
        let mut headers = HeaderMap::new();
        assert_eq!(bearer_token(&headers).unwrap(), None);

        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer token"),
        );
        assert_eq!(bearer_token(&headers).unwrap(), Some("token"));

        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Basic token"),
        );
        assert!(bearer_token(&headers).is_err());

        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer "));
        assert!(bearer_token(&headers).is_err());
    }
}
