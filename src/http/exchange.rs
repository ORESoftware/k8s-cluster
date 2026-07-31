//! Exchange a verified external-provider token for a shared-auth session.

use axum::{extract::State, http::HeaderMap, Json};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::AuthenticatedIdentity;
use crate::error::AuthError;
use crate::state::AppState;
use crate::token::AuthenticationAssurance;

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
    pub amr: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acr: Option<String>,
    /// Compatibility alias for existing Supabase consumers.
    pub project: String,
}

#[derive(Debug, Deserialize)]
struct VerifiedSupabaseAssuranceClaims {
    #[serde(default)]
    aal: Option<String>,
    #[serde(default)]
    amr: Vec<SupabaseAmrEntry>,
}

#[derive(Debug, Deserialize)]
struct SupabaseAmrEntry {
    method: String,
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

    // Parse assurance only after the exact token has passed signature, issuer,
    // audience, and expiry verification. Any malformed or unknown metadata is
    // normalized to a federated method with no ACR, which fails closed for LOA2.
    let assurance = verified_supabase_assurance(token);

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
    let issued = session_tokens::issue_with_assurance(state, identity, assurance).await?;

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
        amr: issued.access.amr,
        acr: issued.access.acr,
        project,
    })
}

/// Extract `aal` and `amr` from a token that the caller has already verified.
/// This helper intentionally performs no authentication and must never be used
/// before `ProjectRegistry::verify` succeeds for the same token.
fn verified_supabase_assurance(token: &str) -> AuthenticationAssurance {
    let Some(payload) = token.split('.').nth(1) else {
        return AuthenticationAssurance::from_supabase(None, &[]);
    };
    let Ok(decoded) = URL_SAFE_NO_PAD.decode(payload) else {
        return AuthenticationAssurance::from_supabase(None, &[]);
    };
    let Ok(claims) = serde_json::from_slice::<VerifiedSupabaseAssuranceClaims>(&decoded) else {
        return AuthenticationAssurance::from_supabase(None, &[]);
    };
    let methods = claims
        .amr
        .into_iter()
        .map(|entry| entry.method)
        .collect::<Vec<_>>();
    AuthenticationAssurance::from_supabase(claims.aal.as_deref(), &methods)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::{ACR_LOA1, ACR_LOA2};
    use serde_json::json;

    fn token_with_payload(payload: serde_json::Value) -> String {
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        format!("e30.{encoded}.signature")
    }

    #[test]
    fn signed_aal2_and_amr_normalize_into_shared_assurance() {
        let token = token_with_payload(json!({
            "aal": "aal2",
            "amr": [
                { "method": "password", "timestamp": 1 },
                { "method": "totp", "timestamp": 2 },
                { "method": "totp", "timestamp": 3 }
            ]
        }));
        let assurance = verified_supabase_assurance(&token);
        assert_eq!(assurance.amr, ["federated", "pwd", "totp"]);
        assert_eq!(assurance.acr.as_deref(), Some(ACR_LOA2));
    }

    #[test]
    fn signed_aal1_normalizes_without_inventing_step_up() {
        let token = token_with_payload(json!({
            "aal": "aal1",
            "amr": [{ "method": "oauth", "timestamp": 1 }]
        }));
        let assurance = verified_supabase_assurance(&token);
        assert_eq!(assurance.amr, ["federated"]);
        assert_eq!(assurance.acr.as_deref(), Some(ACR_LOA1));
    }

    #[test]
    fn malformed_or_unknown_assurance_fails_closed() {
        let malformed = verified_supabase_assurance("not-a-jwt");
        assert_eq!(malformed.amr, ["federated"]);
        assert_eq!(malformed.acr, None);

        let token = token_with_payload(json!({
            "aal": "aal3",
            "amr": [{ "method": "<untrusted>", "timestamp": 1 }]
        }));
        let unknown = verified_supabase_assurance(&token);
        assert_eq!(unknown.amr, ["federated"]);
        assert_eq!(unknown.acr, None);
    }
}
