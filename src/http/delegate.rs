//! Fail-closed OAuth-style delegation for product-to-product user actions.
//!
//! The endpoint exchanges a normal shared-auth user token for a short-lived
//! token whose audience and scopes are selected from an operator-configured
//! allow-list. It never contacts a factor application. A 3FA-originated TOTP or
//! passkey ceremony is represented only by the verified shared-auth `acr`/`amr`.

use std::{
    collections::HashSet,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::{
    error::AuthError,
    state::AppState,
    token::{OreClaims, ACR_LOA2},
};

use super::{bearer, introspect::active_claims};

const MAX_POLICIES: usize = 128;
const MAX_SCOPES: usize = 32;
const MAX_IDENTIFIER_BYTES: usize = 128;
const DEFAULT_DELEGATED_TTL_SECS: u64 = 300;
const DEFAULT_MAX_AUTH_AGE_SECS: u64 = 600;
const MAX_DELEGATED_TTL_SECS: u64 = 900;
const MAX_CLOCK_SKEW_SECS: u64 = 60;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelegateRequest {
    client_id: String,
    audience: String,
    scopes: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DelegateResponse {
    access_token: String,
    token_type: &'static str,
    expires_at: u64,
    audience: String,
    scope: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct DelegationPolicy {
    client_id: String,
    audience: String,
    allowed_scopes: Vec<String>,
    #[serde(default)]
    require_loa2_scopes: Vec<String>,
    #[serde(default)]
    required_roles: Vec<String>,
    #[serde(default = "default_delegated_ttl_secs")]
    ttl_secs: u64,
    #[serde(default = "default_max_auth_age_secs")]
    max_auth_age_secs: u64,
}

#[derive(Debug, PartialEq, Eq)]
struct AuthorizedDelegation {
    audience: String,
    client_id: String,
    scopes: Vec<String>,
    ttl_secs: u64,
}

pub async fn delegate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DelegateRequest>,
) -> Response {
    let Some(subject_token) = bearer(&headers) else {
        return AuthError::Unauthorized.into_response();
    };
    if subject_token.len() > 16 * 1024 {
        return AuthError::Unauthorized.into_response();
    }

    let claims = match active_claims(&state, subject_token).await {
        Ok(claims) => claims,
        Err(error) => return error.into_response(),
    };
    let policies = match delegation_policies() {
        Ok(policies) => policies,
        Err(error) => return error.into_response(),
    };
    let now = now_secs();
    let authorized = match authorize_delegation(now, &claims, &request, policies) {
        Ok(authorized) => authorized,
        Err(error) => return error.into_response(),
    };
    let minted = match state.minter.mint_delegated(
        &claims,
        &authorized.audience,
        &authorized.client_id,
        &authorized.scopes,
        authorized.ttl_secs,
    ) {
        Ok(minted) => minted,
        Err(error) => return error.into_response(),
    };

    Json(DelegateResponse {
        access_token: minted.token,
        token_type: "Bearer",
        expires_at: minted.expires_at,
        audience: authorized.audience,
        scope: authorized.scopes.join(" "),
    })
    .into_response()
}

fn authorize_delegation(
    now: u64,
    claims: &OreClaims,
    request: &DelegateRequest,
    policies: &[DelegationPolicy],
) -> Result<AuthorizedDelegation, AuthError> {
    if now == 0
        || claims.is_delegated()
        || claims.sid.as_deref().is_none_or(str::is_empty)
        || !valid_identifier(&request.client_id)
        || !valid_identifier(&request.audience)
        || request.scopes.is_empty()
        || request.scopes.len() > MAX_SCOPES
    {
        return Err(AuthError::Forbidden);
    }

    let scopes = normalized_unique(&request.scopes).ok_or(AuthError::BadRequest("invalid scopes"))?;
    let policy = policies
        .iter()
        .find(|policy| {
            policy.client_id == request.client_id && policy.audience == request.audience
        })
        .ok_or(AuthError::Forbidden)?;

    if !policy
        .required_roles
        .iter()
        .all(|required| claims.roles.iter().any(|role| role == required))
        || !scopes
            .iter()
            .all(|scope| policy.allowed_scopes.iter().any(|allowed| allowed == scope))
    {
        return Err(AuthError::Forbidden);
    }

    let requires_loa2 = scopes.iter().any(|scope| {
        policy
            .require_loa2_scopes
            .iter()
            .any(|sensitive| sensitive == scope)
    });
    if requires_loa2 {
        let Some(auth_time) = claims.auth_time else {
            return Err(AuthError::Forbidden);
        };
        if !claims.has_acr(ACR_LOA2)
            || auth_time > now.saturating_add(MAX_CLOCK_SKEW_SECS)
            || now.saturating_sub(auth_time) > policy.max_auth_age_secs
        {
            return Err(AuthError::Forbidden);
        }
    }

    Ok(AuthorizedDelegation {
        audience: policy.audience.clone(),
        client_id: policy.client_id.clone(),
        scopes,
        ttl_secs: policy.ttl_secs,
    })
}

fn delegation_policies() -> Result<&'static [DelegationPolicy], AuthError> {
    static POLICIES: OnceLock<Result<Vec<DelegationPolicy>, String>> = OnceLock::new();
    match POLICIES.get_or_init(load_delegation_policies) {
        Ok(policies) => Ok(policies.as_slice()),
        Err(error) => {
            tracing::error!(%error, "invalid AUTH_DELEGATION_POLICIES");
            Err(AuthError::Internal)
        }
    }
}

fn load_delegation_policies() -> Result<Vec<DelegationPolicy>, String> {
    let raw = std::env::var("AUTH_DELEGATION_POLICIES").unwrap_or_else(|_| "[]".to_owned());
    let policies: Vec<DelegationPolicy> =
        serde_json::from_str(&raw).map_err(|error| format!("invalid JSON: {error}"))?;
    validate_policies(&policies)?;
    if policies.is_empty() {
        tracing::warn!(
            "AUTH_DELEGATION_POLICIES is empty; /auth/delegate will deny every request"
        );
    }
    Ok(policies)
}

fn validate_policies(policies: &[DelegationPolicy]) -> Result<(), String> {
    if policies.len() > MAX_POLICIES {
        return Err("too many delegation policies".to_owned());
    }
    let mut tuples = HashSet::with_capacity(policies.len());
    for policy in policies {
        if !valid_identifier(&policy.client_id)
            || !valid_identifier(&policy.audience)
            || policy.allowed_scopes.is_empty()
            || policy.allowed_scopes.len() > MAX_SCOPES
            || policy.required_roles.len() > MAX_SCOPES
            || !(60..=MAX_DELEGATED_TTL_SECS).contains(&policy.ttl_secs)
            || !(60..=3_600).contains(&policy.max_auth_age_secs)
        {
            return Err("invalid delegation policy bounds".to_owned());
        }
        if !tuples.insert((policy.client_id.as_str(), policy.audience.as_str())) {
            return Err("duplicate client/audience delegation policy".to_owned());
        }
        let allowed = normalized_unique(&policy.allowed_scopes)
            .ok_or_else(|| "invalid or duplicate allowed scope".to_owned())?;
        let loa2 = normalized_unique(&policy.require_loa2_scopes)
            .ok_or_else(|| "invalid or duplicate LOA2 scope".to_owned())?;
        if !loa2
            .iter()
            .all(|scope| allowed.iter().any(|candidate| candidate == scope))
            || normalized_unique(&policy.required_roles).is_none()
        {
            return Err("LOA2 scopes or roles are inconsistent".to_owned());
        }
    }
    Ok(())
}

fn normalized_unique(values: &[String]) -> Option<Vec<String>> {
    let mut normalized = Vec::with_capacity(values.len());
    let mut seen = HashSet::with_capacity(values.len());
    for value in values {
        if !valid_identifier(value) || !seen.insert(value.as_str()) {
            return None;
        }
        normalized.push(value.clone());
    }
    normalized.sort_unstable();
    Some(normalized)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-')
        })
}

fn default_delegated_ttl_secs() -> u64 {
    DEFAULT_DELEGATED_TTL_SECS
}

fn default_max_auth_age_secs() -> u64 {
    DEFAULT_MAX_AUTH_AGE_SECS
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(acr: &str, auth_time: u64) -> OreClaims {
        OreClaims {
            sub: "shared-user-1".into(),
            iss: "https://auth.test".into(),
            aud: "oresoftware".into(),
            iat: 1_000,
            exp: 2_000,
            nbf: 999,
            jti: "base-token-id".into(),
            sid: Some("session-1".into()),
            provider: "supabase".into(),
            provider_tenant: "memebank".into(),
            provider_subject: "provider-user-1".into(),
            project: Some("memebank".into()),
            supabase_user_id: Some("provider-user-1".into()),
            email: None,
            email_verified: false,
            roles: vec!["user".into()],
            aal: if acr == ACR_LOA2 { 2 } else { 1 },
            amr: vec!["federated".into(), "totp".into()],
            acr: Some(acr.into()),
            auth_time: Some(auth_time),
            scope: String::new(),
            azp: None,
            parent_jti: None,
        }
    }

    fn policy() -> DelegationPolicy {
        DelegationPolicy {
            client_id: "memebank-api".into(),
            audience: "cliptown-api".into(),
            allowed_scopes: vec![
                "cliptown:memebank:read".into(),
                "cliptown:memebank:write".into(),
                "cliptown:memebank:delete".into(),
            ],
            require_loa2_scopes: vec![
                "cliptown:memebank:write".into(),
                "cliptown:memebank:delete".into(),
            ],
            required_roles: vec!["user".into()],
            ttl_secs: 300,
            max_auth_age_secs: 600,
        }
    }

    fn request(scopes: &[&str]) -> DelegateRequest {
        DelegateRequest {
            client_id: "memebank-api".into(),
            audience: "cliptown-api".into(),
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
        }
    }

    #[test]
    fn base_assurance_can_receive_read_only_scope() {
        let result = authorize_delegation(
            1_100,
            &claims("urn:oresoftware:loa:1", 1_000),
            &request(&["cliptown:memebank:read"]),
            &[policy()],
        )
        .unwrap();
        assert_eq!(result.audience, "cliptown-api");
        assert_eq!(result.client_id, "memebank-api");
        assert_eq!(result.scopes, ["cliptown:memebank:read"]);
    }

    #[test]
    fn sensitive_scope_consumes_shared_auth_loa2_not_a_direct_3fa_proof() {
        let result = authorize_delegation(
            1_100,
            &claims(ACR_LOA2, 1_000),
            &request(&["cliptown:memebank:write"]),
            &[policy()],
        )
        .unwrap();
        assert_eq!(result.scopes, ["cliptown:memebank:write"]);
    }

    #[test]
    fn sensitive_scope_rejects_missing_or_stale_step_up() {
        assert!(authorize_delegation(
            1_100,
            &claims("urn:oresoftware:loa:1", 1_000),
            &request(&["cliptown:memebank:write"]),
            &[policy()],
        )
        .is_err());
        assert!(authorize_delegation(
            2_000,
            &claims(ACR_LOA2, 1_000),
            &request(&["cliptown:memebank:write"]),
            &[policy()],
        )
        .is_err());
    }

    #[test]
    fn wrong_client_audience_scope_and_recursive_exchange_fail_closed() {
        let policies = [policy()];
        let valid_claims = claims(ACR_LOA2, 1_000);

        let mut wrong_client = request(&["cliptown:memebank:read"]);
        wrong_client.client_id = "other-client".into();
        assert!(authorize_delegation(1_100, &valid_claims, &wrong_client, &policies).is_err());

        let mut wrong_audience = request(&["cliptown:memebank:read"]);
        wrong_audience.audience = "memebank-api".into();
        assert!(authorize_delegation(1_100, &valid_claims, &wrong_audience, &policies).is_err());

        assert!(authorize_delegation(
            1_100,
            &valid_claims,
            &request(&["cliptown:admin"]),
            &policies,
        )
        .is_err());

        let mut delegated_claims = valid_claims;
        delegated_claims.azp = Some("memebank-api".into());
        delegated_claims.parent_jti = Some("parent".into());
        assert!(authorize_delegation(
            1_100,
            &delegated_claims,
            &request(&["cliptown:memebank:read"]),
            &policies,
        )
        .is_err());
    }

    #[test]
    fn policy_parser_rejects_duplicate_or_unbounded_grants() {
        let duplicate = vec![policy(), policy()];
        assert!(validate_policies(&duplicate).is_err());

        let mut inconsistent = policy();
        inconsistent.require_loa2_scopes = vec!["cliptown:admin".into()];
        assert!(validate_policies(&[inconsistent]).is_err());

        let mut excessive_ttl = policy();
        excessive_ttl.ttl_secs = MAX_DELEGATED_TTL_SECS + 1;
        assert!(validate_policies(&[excessive_ttl]).is_err());
    }
}
