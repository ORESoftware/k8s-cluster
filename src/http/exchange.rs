//! Exchange a verified external-provider token for a shared-auth session.

use axum::{extract::State, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::AuthenticatedIdentity;
use crate::error::AuthError;
use crate::state::AppState;

use super::bearer;
use super::session_tokens;

#[derive(Debug, Deserialize)]
pub struct ExchangeRequest {
    access_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ExchangeResponse {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_expires_at: Option<u64>,
    pub shared_user_id: String,
    pub provider: &'static str,
    pub provider_tenant: String,
    pub provider_subject: String,
    pub roles: Vec<String>,
    /// Compatibility alias for existing Supabase consumers.
    pub project: String,
}

pub(crate) async fn perform_exchange(
    state: &AppState,
    token: &str,
) -> Result<ExchangeResponse, AuthError> {
    let verified = match state.supabase.verify(&state.http, token).await {
        Ok(identity) => identity,
        Err(error) => {
            state.metrics.verify_failures.inc();
            return Err(error);
        }
    };

    let identity = match &state.db {
        Some(db) => db.upsert_supabase_identity(&verified).await?,
        None => AuthenticatedIdentity {
            shared_user_id: Uuid::new_v5(
                &Uuid::NAMESPACE_URL,
                format!(
                    "supabase:{}:{}",
                    verified.project, verified.supabase_user_id
                )
                .as_bytes(),
            ),
            provider: "supabase".into(),
            provider_tenant: verified.project.clone(),
            provider_subject: verified.supabase_user_id.clone(),
            email: verified.email.clone(),
            email_verified: verified.email_verified,
            roles: verified.role.clone().into_iter().collect(),
        },
    };
    let shared_user_id = identity.shared_user_id.to_string();
    let project = identity.provider_tenant.clone();
    let provider_subject = identity.provider_subject.clone();
    let roles = identity.roles.clone();
    let issued = session_tokens::issue(state, identity).await?;

    state
        .metrics
        .exchanges
        .with_label_values(&[&project, "ok"])
        .inc();
    tracing::info!(provider = "supabase", provider_tenant = %project, %shared_user_id, "issued shared-auth session");

    Ok(ExchangeResponse {
        access_token: issued.access.token,
        token_type: "Bearer",
        expires_at: issued.access.expires_at,
        refresh_token: issued.refresh_token,
        refresh_expires_at: issued.refresh_expires_at,
        shared_user_id,
        provider: "supabase",
        provider_tenant: project.clone(),
        provider_subject,
        roles,
        project,
    })
}

pub async fn exchange(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<ExchangeRequest>>,
) -> Result<Json<ExchangeResponse>, AuthError> {
    let token = bearer(&headers)
        .map(str::to_owned)
        .or_else(|| body.and_then(|body| body.0.access_token))
        .filter(|token| token.len() <= 16 * 1024)
        .ok_or(AuthError::BadRequest(
            "missing or oversized provider access token",
        ))?;
    Ok(Json(perform_exchange(&state, &token).await?))
}
