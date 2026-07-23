//! HMAC-authenticated, idempotent provider synchronization.
//!
//! Signature contract:
//! `base64url(HMAC-SHA256(secret, "<unix_timestamp>.<raw_body>"))` in
//! `x-shared-auth-signature`, with the timestamp in `x-shared-auth-timestamp`.

use axum::{body::Bytes, extract::State, http::HeaderMap, Json};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use uuid::Uuid;

use crate::error::AuthError;
use crate::session::hash_token;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct SyncEvent {
    event_id: Uuid,
    provider: String,
    #[serde(flatten)]
    action: SyncAction,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "payload")]
enum SyncAction {
    #[serde(rename = "identity.upsert")]
    IdentityUpsert {
        provider_tenant: String,
        provider_subject: String,
        #[serde(default)]
        email: Option<String>,
        #[serde(default)]
        email_verified: bool,
        #[serde(default)]
        metadata: serde_json::Value,
    },
    #[serde(rename = "roles.replace")]
    RolesReplace {
        shared_user_id: Uuid,
        roles: Vec<String>,
    },
    #[serde(rename = "session.revoke")]
    SessionRevoke { session_id: Uuid },
}

#[derive(serde::Serialize)]
pub struct SyncResponse {
    status: &'static str,
    duplicate: bool,
}

pub async fn sync_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<SyncResponse>, AuthError> {
    verify_signature(&state, &headers, &body)?;
    let event: SyncEvent =
        serde_json::from_slice(&body).map_err(|_| AuthError::BadRequest("invalid sync event"))?;
    validate_provider(&event.provider)?;
    let event_type = match &event.action {
        SyncAction::IdentityUpsert { .. } => "identity.upsert",
        SyncAction::RolesReplace { .. } => "roles.replace",
        SyncAction::SessionRevoke { .. } => "session.revoke",
    };
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;

    match &event.action {
        SyncAction::IdentityUpsert {
            provider_tenant,
            provider_subject,
            email,
            email_verified,
            metadata,
        } => {
            validate_external_key(provider_tenant, 255)?;
            validate_external_key(provider_subject, 512)?;
            db.upsert_external_identity(
                &event.provider,
                provider_tenant,
                provider_subject,
                email.clone(),
                *email_verified,
                metadata.clone(),
            )
            .await?;
        }
        SyncAction::RolesReplace {
            shared_user_id,
            roles,
        } => {
            if roles.len() > 64 || roles.iter().any(|role| !valid_role(role)) {
                return Err(AuthError::BadRequest("invalid roles"));
            }
            db.replace_roles(*shared_user_id, roles).await?;
        }
        SyncAction::SessionRevoke { session_id } => {
            db.revoke_by_session_id(*session_id).await?;
            if let Some(cache) = &state.cache {
                if let Err(error) = cache
                    .mark_revoked(*session_id, state.config.signing.ttl_secs)
                    .await
                {
                    tracing::warn!(%error, %session_id, "failed to cache webhook revocation");
                }
            }
        }
    }

    // Each action above is itself idempotent; recording after application avoids
    // a failed event being permanently skipped on retry.
    let inserted = db
        .record_webhook_event(
            event.event_id,
            &event.provider,
            event_type,
            &hash_token(std::str::from_utf8(&body).unwrap_or_default()),
        )
        .await?;
    Ok(Json(SyncResponse {
        status: "ok",
        duplicate: !inserted,
    }))
}

fn verify_signature(state: &AppState, headers: &HeaderMap, body: &[u8]) -> Result<(), AuthError> {
    let secret = state
        .config
        .webhook_secret
        .as_deref()
        .ok_or(AuthError::Unauthorized)?;
    let timestamp = headers
        .get("x-shared-auth-timestamp")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or(AuthError::Unauthorized)?;
    let now = chrono::Utc::now().timestamp();
    if (now - timestamp).abs() > 300 {
        return Err(AuthError::Unauthorized);
    }
    let signature = headers
        .get("x-shared-auth-signature")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
        .ok_or(AuthError::Unauthorized)?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| AuthError::Internal)?;
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    mac.verify_slice(&signature)
        .map_err(|_| AuthError::Unauthorized)
}

fn validate_provider(provider: &str) -> Result<(), AuthError> {
    if provider.len() <= 64
        && !provider.is_empty()
        && provider
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        Ok(())
    } else {
        Err(AuthError::BadRequest("invalid provider"))
    }
}

fn validate_external_key(value: &str, max: usize) -> Result<(), AuthError> {
    if !value.is_empty() && value.len() <= max {
        Ok(())
    } else {
        Err(AuthError::BadRequest("invalid provider identifier"))
    }
}

fn valid_role(role: &str) -> bool {
    let mut bytes = role.bytes();
    matches!(bytes.next(), Some(first) if first.is_ascii_lowercase())
        && role.len() <= 64
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b':' | b'_' | b'-')
        })
}
