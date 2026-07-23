//! Access-token introspection and gateway verification.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::error::AuthError;
use crate::state::AppState;
use crate::token::OreClaims;

use super::bearer;

#[derive(Debug, Deserialize)]
pub struct IntrospectRequest {
    token: String,
}

pub async fn introspect(
    State(state): State<AppState>,
    Json(request): Json<IntrospectRequest>,
) -> Json<serde_json::Value> {
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
        })),
        Err(_) => Json(json!({ "active": false })),
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
