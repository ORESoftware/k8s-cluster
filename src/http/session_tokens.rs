use chrono::TimeDelta;
use uuid::Uuid;

use crate::db::AuthenticatedIdentity;
use crate::error::AuthError;
use crate::session::RefreshToken;
use crate::state::AppState;
use crate::token::{MintContext, MintedToken, OreClaims, ACR_BASE, ACR_STEP_UP};

pub struct IssuedSession {
    pub access: MintedToken,
    pub refresh_token: Option<String>,
    pub refresh_expires_at: Option<u64>,
}

pub async fn issue(
    state: &AppState,
    identity: AuthenticatedIdentity,
) -> Result<IssuedSession, AuthError> {
    let (identity, session_id, refresh_token, refresh_expires_at) = if let Some(db) = &state.db {
        let refresh = RefreshToken::generate();
        let expires_at = chrono::Utc::now().fixed_offset()
            + TimeDelta::seconds(state.config.sessions.refresh_ttl_secs as i64);
        let session = db
            .create_session(identity, &refresh.hash, expires_at, None)
            .await?;
        (
            session.identity,
            Some(session.session_id),
            Some(refresh.plaintext),
            Some(expires_at.timestamp() as u64),
        )
    } else {
        (identity, None, None, None)
    };

    let access = state.minter.mint(base_context(&identity, session_id))?;
    Ok(IssuedSession {
        access,
        refresh_token,
        refresh_expires_at,
    })
}

/// Refresh deliberately returns base assurance. A high-assurance operation must
/// perform a fresh step-up rather than extending an old MFA proof indefinitely.
pub fn mint_for_session(
    state: &AppState,
    session: &crate::db::SessionRecord,
) -> Result<MintedToken, AuthError> {
    state
        .minter
        .mint(base_context(&session.identity, Some(session.session_id)))
}

/// Mint an access-only token for the same active session after an MFA/passkey
/// challenge succeeds. No refresh token is issued and refresh drops back to base
/// assurance.
pub fn mint_step_up(
    state: &AppState,
    claims: &OreClaims,
    method: &str,
) -> Result<MintedToken, AuthError> {
    let session_id = claims
        .sid
        .as_deref()
        .ok_or(AuthError::Unauthorized)
        .and_then(|raw| Uuid::parse_str(raw).map_err(|_| AuthError::Unauthorized))?;
    let mut amr = claims.amr.clone();
    if amr.is_empty() {
        amr.push(base_method(&claims.provider).to_string());
    }
    amr.push(method.to_string());
    state.minter.mint(MintContext {
        shared_user_id: claims.sub.clone(),
        session_id: Some(session_id),
        provider: claims.provider.clone(),
        provider_tenant: claims.provider_tenant.clone(),
        provider_subject: claims.provider_subject.clone(),
        email: claims.email.clone(),
        email_verified: claims.email_verified,
        roles: claims.roles.clone(),
        amr,
        acr: Some(ACR_STEP_UP.to_string()),
    })
}

fn base_context(identity: &AuthenticatedIdentity, session_id: Option<Uuid>) -> MintContext {
    MintContext {
        shared_user_id: identity.shared_user_id.to_string(),
        session_id,
        provider: identity.provider.clone(),
        provider_tenant: identity.provider_tenant.clone(),
        provider_subject: identity.provider_subject.clone(),
        email: identity.email.clone(),
        email_verified: identity.email_verified,
        roles: identity.roles.clone(),
        amr: vec![base_method(&identity.provider).to_string()],
        acr: Some(ACR_BASE.to_string()),
    }
}

fn base_method(provider: &str) -> &'static str {
    if provider == "local" {
        "pwd"
    } else {
        "federated"
    }
}
