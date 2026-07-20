//! Validate the OreSoftware JWTs this server mints.
//!
//! - `POST /auth/introspect` — RFC-7662-shaped: body `{ "token": "…" }` →
//!   `{ "active": bool, … claims }`.
//! - `GET /auth/verify` — lightweight bearer check for the NGINX gateway's
//!   `auth_request`: 200 + identity headers, or 401. No body.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::error::AuthError;
use crate::state::AppState;

use super::bearer;

#[derive(Debug, Deserialize)]
pub struct IntrospectRequest {
    token: String,
}

pub async fn introspect(
    State(state): State<AppState>,
    Json(req): Json<IntrospectRequest>,
) -> Json<serde_json::Value> {
    match state.minter.verify(&req.token) {
        Ok(claims) => Json(json!({
            "active": true,
            "sub": claims.sub,
            "iss": claims.iss,
            "aud": claims.aud,
            "exp": claims.exp,
            "iat": claims.iat,
            "project": claims.project,
            "supabase_user_id": claims.supabase_user_id,
            "email": claims.email,
            "email_verified": claims.email_verified,
        })),
        // Per RFC 7662 an invalid token is a normal `{ "active": false }`, not
        // an error response.
        Err(_) => Json(json!({ "active": false })),
    }
}

/// Gateway `auth_request` target. Returns 200 with `X-Auth-*` headers the
/// gateway can forward to upstreams, or 401.
pub async fn verify(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(token) = bearer(&headers) else {
        return AuthError::Unauthorized.into_response();
    };
    match state.minter.verify(token) {
        Ok(claims) => {
            let mut out = HeaderMap::new();
            insert_header(&mut out, "x-auth-user-id", &claims.sub);
            insert_header(&mut out, "x-auth-project", &claims.project);
            if let Some(email) = &claims.email {
                insert_header(&mut out, "x-auth-email", email);
            }
            (StatusCode::OK, out).into_response()
        }
        Err(err) => err.into_response(),
    }
}

fn insert_header(map: &mut HeaderMap, name: &'static str, value: &str) {
    if let Ok(v) = axum::http::HeaderValue::from_str(value) {
        map.insert(name, v);
    }
}
