//! Access-token introspection and gateway verification.

use std::sync::Once;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use hmac::{Hmac, KeyInit, Mac};
use rand::{rngs::SysRng, TryRng};
use serde::Deserialize;
use serde_json::json;
use sha2::Sha256;
use uuid::Uuid;

use crate::error::AuthError;
use crate::state::AppState;
use crate::token::OreClaims;

use super::bearer;

#[derive(Debug, Deserialize)]
pub struct IntrospectRequest {
    token: String,
}

/// Enforce caller authentication before introspection reveals full token claims.
///
/// Introspection is disabled unless `AUTH_INTROSPECT_SECRET` is configured. This
/// is intentionally fail-closed: possessing an end-user token must not also grant
/// permission to recover its complete identity, provider, role, session, email,
/// and assurance claim set. Authorized service callers present the independent
/// credential as `Authorization: Bearer <secret>`.
fn authorize_caller(state: &AppState, headers: &HeaderMap) -> Result<(), AuthError> {
    let Some(expected) = state.config.introspect_secret.as_deref() else {
        static WARN_ONCE: Once = Once::new();
        WARN_ONCE.call_once(|| {
            tracing::warn!(
                "/auth/introspect is disabled because AUTH_INTROSPECT_SECRET is unset"
            );
        });
        return Err(AuthError::Unauthorized);
    };
    authorize_service_credential(Some(expected), bearer(headers))
}

fn authorize_service_credential(
    expected: Option<&str>,
    presented: Option<&str>,
) -> Result<(), AuthError> {
    let expected = expected.ok_or(AuthError::Unauthorized)?;
    let presented = presented.ok_or(AuthError::Unauthorized)?;
    if credentials_match(expected, presented) {
        Ok(())
    } else {
        Err(AuthError::Unauthorized)
    }
}

/// Constant-time credential comparison via double-HMAC under a per-call random
/// key, so the final byte comparison leaks nothing about the secret in timing.
/// Reuses the `hmac`/`sha2`/`rand` deps already used by the webhook signer.
fn credentials_match(expected: &str, presented: &str) -> bool {
    let mut key = [0u8; 32];
    if SysRng.try_fill_bytes(&mut key).is_err() {
        return false;
    }
    let tag = |data: &[u8]| {
        let mut mac =
            <Hmac<Sha256> as KeyInit>::new_from_slice(&key).expect("HMAC accepts any key length");
        mac.update(data);
        mac.finalize().into_bytes()
    };
    tag(expected.as_bytes()) == tag(presented.as_bytes())
}

pub async fn introspect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<IntrospectRequest>,
) -> Response {
    if let Err(error) = authorize_caller(&state, &headers) {
        return error.into_response();
    }
    let verified = if request.token.len() <= 16 * 1024 {
        active_claims(&state, &request.token).await
    } else {
        Err(AuthError::Unauthorized)
    };
    state
        .metrics
        .introspections
        .with_label_values(&[if verified.is_ok() {
            "active"
        } else {
            "inactive"
        }])
        .inc();
    match verified {
        Ok(claims) => Json(json!({
            "active": true,
            "sub": claims.sub,
            "iss": claims.iss,
            "aud": claims.aud,
            "exp": claims.exp,
            "iat": claims.iat,
            "sid": claims.sid,
            "provider": claims.provider,
            "provider_tenant": claims.provider_tenant,
            "provider_subject": claims.provider_subject,
            "project": claims.project,
            "supabase_user_id": claims.supabase_user_id,
            "email": claims.email,
            "email_verified": claims.email_verified,
            "roles": claims.roles,
            "aal": claims.aal,
            "amr": claims.amr,
            "acr": claims.acr,
        }))
        .into_response(),
        Err(_) => Json(json!({ "active": false })).into_response(),
    }
}

pub async fn verify(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(token) = bearer(&headers) else {
        return AuthError::Unauthorized.into_response();
    };
    match active_claims(&state, token).await {
        Ok(claims) => {
            let mut output = HeaderMap::new();
            insert_header(&mut output, "x-auth-user-id", &claims.sub);
            insert_header(&mut output, "x-auth-provider", &claims.provider);
            insert_header(
                &mut output,
                "x-auth-provider-tenant",
                &claims.provider_tenant,
            );
            insert_header(&mut output, "x-auth-roles", &claims.roles.join(","));
            insert_header(&mut output, "x-auth-aal", &claims.aal.to_string());
            if !claims.amr.is_empty() {
                insert_header(&mut output, "x-auth-amr", &claims.amr.join(","));
            }
            if let Some(acr) = &claims.acr {
                insert_header(&mut output, "x-auth-acr", acr);
            }
            if let Some(project) = &claims.project {
                insert_header(&mut output, "x-auth-project", project);
            }
            if let Some(email) = &claims.email {
                insert_header(&mut output, "x-auth-email", email);
            }
            (StatusCode::OK, output).into_response()
        }
        Err(error) => error.into_response(),
    }
}

pub(crate) async fn active_claims(state: &AppState, token: &str) -> Result<OreClaims, AuthError> {
    let claims = state.minter.verify(token)?;
    match (&state.db, claims.sid.as_deref()) {
        (Some(db), Some(raw_session_id)) => {
            let session_id =
                Uuid::parse_str(raw_session_id).map_err(|_| AuthError::Unauthorized)?;
            if let Some(cache) = &state.cache {
                match cache.is_revoked(session_id).await {
                    Ok(true) => return Err(AuthError::Unauthorized),
                    Ok(false) => {}
                    Err(error) => {
                        tracing::warn!(%error, %session_id, "Redis revocation check failed")
                    }
                }
            }
            if !db.session_is_active(session_id).await? {
                return Err(AuthError::Unauthorized);
            }
        }
        // A production token without a session id bypasses revocation, so reject
        // it whenever the authoritative session store is configured.
        (Some(_), None) => return Err(AuthError::Unauthorized),
        (None, _) => {}
    }
    Ok(claims)
}

fn insert_header(map: &mut HeaderMap, name: &'static str, value: &str) {
    if let Ok(value) = axum::http::HeaderValue::from_str(value) {
        map.insert(name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::authorize_service_credential;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn introspection_fails_closed_without_a_configured_service_secret() {
        assert!(authorize_service_credential(None, None).is_err());
        assert!(authorize_service_credential(None, Some(SECRET)).is_err());
    }

    #[test]
    fn introspection_rejects_missing_or_incorrect_service_credentials() {
        assert!(authorize_service_credential(Some(SECRET), None).is_err());
        assert!(
            authorize_service_credential(
                Some(SECRET),
                Some("fedcba9876543210fedcba9876543210")
            )
            .is_err()
        );
    }

    #[test]
    fn introspection_accepts_only_the_independent_service_credential() {
        assert!(authorize_service_credential(Some(SECRET), Some(SECRET)).is_ok());
    }
}
