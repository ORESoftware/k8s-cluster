//! Exchange a verified external-provider token for a shared-auth session.

use std::time::{SystemTime, UNIX_EPOCH};

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

const AUTH_TIME_FUTURE_LEEWAY_SECS: u64 = 30;

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
    #[serde(default)]
    timestamp: Option<u64>,
}

struct VerifiedAssurance {
    assurance: AuthenticationAssurance,
    auth_time: Option<u64>,
}

impl VerifiedAssurance {
    fn fail_closed() -> Self {
        Self {
            assurance: AuthenticationAssurance::from_supabase(None, &[]),
            auth_time: None,
        }
    }
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
    //
    // AAL2 additionally requires a usable timestamp from the MFA factor itself.
    // A newer password, OAuth, or refresh timestamp must not make an older MFA
    // ceremony appear fresh to a downstream financial service.
    let VerifiedAssurance {
        assurance,
        auth_time,
    } = verified_supabase_assurance(token);

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
    let issued =
        session_tokens::issue_with_assurance_at(state, identity, assurance, auth_time).await?;

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

/// Extract `aal`, `amr`, and the factor-ceremony timestamp from a token that the
/// caller has already verified.
///
/// This helper intentionally performs no authentication and must never be used
/// before `ProjectRegistry::verify` succeeds for the same token. A provider
/// token claiming AAL2 without a timestamp on a recognized second-factor method
/// fails closed to base assurance. We deliberately do not use the maximum
/// timestamp across every AMR entry: first-factor reauthentication or token
/// refresh must not renew the apparent age of an older MFA ceremony.
fn verified_supabase_assurance(token: &str) -> VerifiedAssurance {
    let Some(payload) = token.split('.').nth(1) else {
        return VerifiedAssurance::fail_closed();
    };
    let Ok(decoded) = URL_SAFE_NO_PAD.decode(payload) else {
        return VerifiedAssurance::fail_closed();
    };
    let Ok(claims) = serde_json::from_slice::<VerifiedSupabaseAssuranceClaims>(&decoded) else {
        return VerifiedAssurance::fail_closed();
    };

    let methods = claims
        .amr
        .iter()
        .map(|entry| entry.method.clone())
        .collect::<Vec<_>>();
    let latest_mfa_auth_time = latest_second_factor_timestamp(&claims.amr);

    if claims.aal.as_deref() == Some("aal2") {
        let now = now_secs();
        let Some(auth_time) = latest_mfa_auth_time
            .filter(|timestamp| *timestamp <= now.saturating_add(AUTH_TIME_FUTURE_LEEWAY_SECS))
        else {
            return VerifiedAssurance {
                assurance: AuthenticationAssurance::from_supabase(None, &methods),
                auth_time: None,
            };
        };
        return VerifiedAssurance {
            assurance: AuthenticationAssurance::from_supabase(Some("aal2"), &methods),
            auth_time: Some(auth_time),
        };
    }

    VerifiedAssurance {
        assurance: AuthenticationAssurance::from_supabase(claims.aal.as_deref(), &methods),
        auth_time: None,
    }
}

fn latest_second_factor_timestamp(entries: &[SupabaseAmrEntry]) -> Option<u64> {
    entries
        .iter()
        .filter(|entry| is_second_factor_method(&entry.method))
        .filter_map(|entry| entry.timestamp)
        .max()
}

fn is_second_factor_method(method: &str) -> bool {
    matches!(
        method.trim().to_ascii_lowercase().as_str(),
        "totp" | "otp" | "phone" | "sms" | "sms_otp" | "webauthn" | "passkey"
    )
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
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
    fn signed_aal2_and_amr_preserve_verified_factor_time() {
        let token = token_with_payload(json!({
            "aal": "aal2",
            "amr": [
                { "method": "password", "timestamp": 1_700_000_001u64 },
                { "method": "totp", "timestamp": 1_700_000_002u64 },
                { "method": "totp", "timestamp": 1_700_000_003u64 }
            ]
        }));
        let verified = verified_supabase_assurance(&token);
        assert_eq!(verified.assurance.amr, ["federated", "pwd", "totp"]);
        assert_eq!(verified.assurance.acr.as_deref(), Some(ACR_LOA2));
        assert_eq!(verified.auth_time, Some(1_700_000_003));
    }

    #[test]
    fn newer_password_timestamp_does_not_refresh_the_mfa_ceremony() {
        let token = token_with_payload(json!({
            "aal": "aal2",
            "amr": [
                { "method": "totp", "timestamp": 1_700_000_003u64 },
                { "method": "password", "timestamp": 1_700_999_999u64 }
            ]
        }));
        let verified = verified_supabase_assurance(&token);
        assert_eq!(verified.assurance.acr.as_deref(), Some(ACR_LOA2));
        assert_eq!(verified.auth_time, Some(1_700_000_003));
    }

    #[test]
    fn signed_phone_otp_aal2_preserves_the_factor_time() {
        let token = token_with_payload(json!({
            "aal": "aal2",
            "amr": [
                { "method": "password", "timestamp": 1_700_000_001u64 },
                { "method": "otp", "timestamp": 1_700_000_004u64 }
            ]
        }));
        let verified = verified_supabase_assurance(&token);
        assert_eq!(verified.assurance.acr.as_deref(), Some(ACR_LOA2));
        assert_eq!(verified.auth_time, Some(1_700_000_004));
    }

    #[test]
    fn signed_aal1_normalizes_without_inventing_step_up() {
        let token = token_with_payload(json!({
            "aal": "aal1",
            "amr": [{ "method": "oauth", "timestamp": 1_700_000_001u64 }]
        }));
        let verified = verified_supabase_assurance(&token);
        assert_eq!(verified.assurance.amr, ["federated"]);
        assert_eq!(verified.assurance.acr.as_deref(), Some(ACR_LOA1));
        assert_eq!(verified.auth_time, None);
    }

    #[test]
    fn aal2_without_a_usable_factor_timestamp_fails_closed() {
        let missing = token_with_payload(json!({
            "aal": "aal2",
            "amr": [{ "method": "totp" }]
        }));
        let verified = verified_supabase_assurance(&missing);
        assert_eq!(verified.assurance.acr, None);
        assert_eq!(verified.auth_time, None);

        let first_factor_only = token_with_payload(json!({
            "aal": "aal2",
            "amr": [{ "method": "password", "timestamp": 1_700_000_001u64 }]
        }));
        let verified = verified_supabase_assurance(&first_factor_only);
        assert_eq!(verified.assurance.acr, None);
        assert_eq!(verified.auth_time, None);

        let future = token_with_payload(json!({
            "aal": "aal2",
            "amr": [{ "method": "totp", "timestamp": u64::MAX }]
        }));
        let verified = verified_supabase_assurance(&future);
        assert_eq!(verified.assurance.acr, None);
        assert_eq!(verified.auth_time, None);
    }

    #[test]
    fn malformed_or_unknown_assurance_fails_closed() {
        let malformed = verified_supabase_assurance("not-a-jwt");
        assert_eq!(malformed.assurance.amr, ["federated"]);
        assert_eq!(malformed.assurance.acr, None);
        assert_eq!(malformed.auth_time, None);

        let token = token_with_payload(json!({
            "aal": "aal3",
            "amr": [{ "method": "<untrusted>", "timestamp": 1_700_000_001u64 }]
        }));
        let unknown = verified_supabase_assurance(&token);
        assert_eq!(unknown.assurance.amr, ["federated"]);
        assert_eq!(unknown.assurance.acr, None);
        assert_eq!(unknown.auth_time, None);
    }
}
