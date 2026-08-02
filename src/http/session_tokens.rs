use chrono::TimeDelta;
use uuid::Uuid;

use crate::db::AuthenticatedIdentity;
use crate::error::AuthError;
use crate::session::RefreshToken;
use crate::state::AppState;
use crate::token::{AuthenticationAssurance, MintContext, MintedToken, OreClaims};

pub struct IssuedSession {
    pub access: MintedToken,
    pub refresh_token: Option<String>,
    pub refresh_expires_at: Option<u64>,
}

/// Issue a session when the authentication method is already implied by the
/// server-owned flow. Local register/login is password-authenticated; any
/// other provider remains fail-closed unless its verified assurance is passed
/// through [`issue_with_assurance`].
///
// Not yet routed: `local.rs` still calls `issue_with_assurance` directly with
// its own (level, methods) pair. Kept as the intended entry point for flows
// whose method is implied by the server rather than supplied by the caller.
#[allow(dead_code)]
pub async fn issue(
    state: &AppState,
    identity: AuthenticatedIdentity,
) -> Result<IssuedSession, AuthError> {
    let assurance = if identity.provider == "local" {
        AuthenticationAssurance::local_password()
    } else {
        AuthenticationAssurance::from_supabase(None, &[])
    };
    issue_with_assurance(state, identity, assurance).await
}

/// Issue a session with assurance extracted only after the upstream token or
/// ceremony has been cryptographically verified. This function stays inside
/// the HTTP module so request payloads cannot supply their own AMR/ACR.
pub(super) async fn issue_with_assurance(
    state: &AppState,
    identity: AuthenticatedIdentity,
    assurance: AuthenticationAssurance,
) -> Result<IssuedSession, AuthError> {
    let (identity, session_id, refresh_token, refresh_expires_at) = if let Some(db) = &state.db {
        let refresh = RefreshToken::generate();
        let expires_at = chrono::Utc::now().fixed_offset()
            + TimeDelta::seconds(state.config.sessions.refresh_ttl_secs as i64);
        // The session row keeps the assurance that was actually proven, so it
        // stays available for audit even though refresh deliberately mints at
        // base assurance rather than replaying it.
        let session = db
            .create_session(
                identity,
                &refresh.hash,
                expires_at,
                None,
                assurance.level(),
                &assurance.amr,
            )
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

    let access = state.minter.mint(MintContext {
        shared_user_id: identity.shared_user_id.to_string(),
        session_id,
        provider: identity.provider,
        provider_tenant: identity.provider_tenant,
        provider_subject: identity.provider_subject,
        email: identity.email,
        email_verified: identity.email_verified,
        roles: identity.roles,
        assurance,
    })?;
    Ok(IssuedSession {
        access,
        refresh_token,
        refresh_expires_at,
    })
}

pub fn mint_for_session(
    state: &AppState,
    session: &crate::db::SessionRecord,
) -> Result<MintedToken, AuthError> {
    let identity = &session.identity;
    state.minter.mint(MintContext {
        shared_user_id: identity.shared_user_id.to_string(),
        session_id: Some(session.session_id),
        provider: identity.provider.clone(),
        provider_tenant: identity.provider_tenant.clone(),
        provider_subject: identity.provider_subject.clone(),
        email: identity.email.clone(),
        email_verified: identity.email_verified,
        roles: identity.roles.clone(),
        assurance: AuthenticationAssurance::refresh_token(),
    })
}

/// Mint an access-only token for the same active session after an MFA/passkey
/// challenge succeeds. No refresh token is issued, so a later refresh drops
/// back to base assurance rather than extending the step-up indefinitely.
///
/// `claims` must come from a token this server already verified — the caller
/// carries the prior `amr` forward so the new token records the whole chain.
///
// Not yet routed: the step-up challenge endpoint lands with the MFA platform
// work. `mfa.rs` currently issues a fresh session via
// `response_from_issued_with_assurance` instead of re-minting for the existing
// session, which is what this does.
#[allow(dead_code)]
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
    state.minter.mint(MintContext {
        shared_user_id: claims.sub.clone(),
        session_id: Some(session_id),
        provider: claims.provider.clone(),
        provider_tenant: claims.provider_tenant.clone(),
        provider_subject: claims.provider_subject.clone(),
        email: claims.email.clone(),
        email_verified: claims.email_verified,
        roles: claims.roles.clone(),
        assurance: AuthenticationAssurance::step_up(&claims.amr, method),
    })
}
