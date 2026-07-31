use chrono::TimeDelta;

use crate::db::AuthenticatedIdentity;
use crate::error::AuthError;
use crate::session::RefreshToken;
use crate::state::AppState;
use crate::token::{AuthenticationAssurance, MintContext, MintedToken};

pub struct IssuedSession {
    pub access: MintedToken,
    pub refresh_token: Option<String>,
    pub refresh_expires_at: Option<u64>,
}

/// Issue a session when the authentication method is already implied by the
/// server-owned flow. Local register/login is password-authenticated; any
/// other provider remains fail-closed unless its verified assurance is passed
/// through [`issue_with_assurance`].
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
