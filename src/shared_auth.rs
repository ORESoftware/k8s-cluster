//! Daedalus adapter for the organization-wide `shared-auth` guard.
//!
//! The shared library owns authority racing, provider verification, policy
//! enforcement, and auth spans. This module only translates service config and
//! the shared identity contract into the small operator type used by handlers.

use std::{sync::Arc, time::Duration};

#[cfg(test)]
use axum::http::{header::AUTHORIZATION, HeaderValue};
use axum::{
    extract::{FromRequestParts, Request, State},
    http::{request::Parts, HeaderMap},
    middleware::Next,
    response::Response,
};
use shared_auth_lib::{
    AccessPolicy, AuthGuard, AuthGuardConfig, AuthOutcome, Authority, AuthorityConfig, GuardConfig,
    Identity,
};

use crate::{config::AuthConfig, error::ServiceError, AppState};

#[derive(Clone, Debug)]
pub(crate) struct Operator {
    pub(crate) subject: String,
    pub(crate) email: Option<String>,
    pub(crate) roles: Vec<String>,
    pub(crate) authority: Authority,
}

#[derive(Clone)]
enum Backend {
    Guard(Arc<AuthGuard>),
    #[cfg(test)]
    Fixed(Operator),
}

#[derive(Clone)]
pub(crate) struct SharedAuthVerifier {
    backend: Backend,
}

impl SharedAuthVerifier {
    pub(crate) fn from_config(config: &AuthConfig) -> Option<Self> {
        let auth_guard_config = AuthGuardConfig {
            guard: GuardConfig {
                authority: AuthorityConfig {
                    shared_auth_base: config.shared_auth_base.clone(),
                    issuer: config.issuer.clone(),
                    audience: config.audience.clone(),
                    supabase_url: config.supabase_url.clone(),
                    supabase_api_key: config.supabase_api_key.clone(),
                    introspect_secret: config.introspect_secret.clone(),
                    arm_timeout: Duration::from_millis(config.arm_timeout_ms),
                },
                supabase_project: Some(config.provider_tenant.clone()),
                race_deadline: Duration::from_millis(config.deadline_ms),
                ..GuardConfig::default()
            },
            policy: AccessPolicy {
                allowed_emails: config.allowed_emails.clone(),
                allowed_roles: config.allowed_roles.clone(),
            },
        };
        AuthGuard::from_config(&auth_guard_config).map(|guard| Self {
            backend: Backend::Guard(Arc::new(guard)),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(operator: Operator) -> Self {
        Self {
            backend: Backend::Fixed(operator),
        }
    }

    #[tracing::instrument(
        name = "daedalus.auth.authorize",
        skip(self, headers),
        fields(auth.header_count = headers.len())
    )]
    pub(crate) async fn authorize(&self, headers: &HeaderMap) -> Result<Operator, ServiceError> {
        match &self.backend {
            Backend::Guard(guard) => operator_from_outcome(guard.authorize(headers).await),
            #[cfg(test)]
            Backend::Fixed(operator)
                if headers
                    .get(AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| bearer_token(Some(value)))
                    .is_none() =>
            {
                let _ = operator;
                Err(ServiceError::Unauthorized)
            }
            #[cfg(test)]
            Backend::Fixed(operator) => Ok(operator.clone()),
        }
    }
}

#[cfg(test)]
pub(crate) async fn authorize_bearer(
    verifier: Option<&SharedAuthVerifier>,
    header: Option<&str>,
) -> Result<Operator, ServiceError> {
    let verifier = verifier.ok_or_else(|| {
        ServiceError::Unavailable(
            "shared-auth is not configured; refusing to serve authenticated routes".to_string(),
        )
    })?;
    let token = bearer_token(header).ok_or(ServiceError::Unauthorized)?;
    let mut headers = HeaderMap::new();
    let value = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| ServiceError::Unauthorized)?;
    headers.insert(AUTHORIZATION, value);
    verifier.authorize(&headers).await
}

#[axum::async_trait]
impl FromRequestParts<AppState> for Operator {
    type Rejection = ServiceError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // The router-level gate already authorized this request and stashed the
        // identity. Re-authorizing here would race both authorities twice.
        if let Some(operator) = parts.extensions.get::<Operator>() {
            return Ok(operator.clone());
        }
        Self::authorize(parts, state).await
    }
}

impl Operator {
    async fn authorize(parts: &mut Parts, state: &AppState) -> Result<Self, ServiceError> {
        let verifier = state.verifier.as_deref().ok_or_else(|| {
            ServiceError::Unavailable(
                "shared-auth is not configured; refusing to serve authenticated routes".to_string(),
            )
        })?;
        verifier.authorize(&parts.headers).await
    }
}

/// Router-level fail-closed shared-auth gate for every non-public route.
#[tracing::instrument(
    name = "daedalus.auth.require_operator",
    skip(state, request, next),
    fields(http.request.method = %request.method(), http.route = %request.uri().path())
)]
pub(crate) async fn require_operator(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ServiceError> {
    let (mut parts, body) = request.into_parts();
    let operator = Operator::authorize(&mut parts, &state).await?;
    tracing::debug!(
        auth.subject = %operator.subject,
        auth.email = ?operator.email,
        auth.authority = ?operator.authority,
        auth.roles = ?operator.roles,
        event.name = "auth.authorization.succeeded",
        "request authorized"
    );
    let mut request = Request::from_parts(parts, body);
    request.extensions_mut().insert(operator);
    Ok(next.run(request).await)
}

fn operator_from_outcome(outcome: AuthOutcome) -> Result<Operator, ServiceError> {
    match outcome {
        AuthOutcome::Authenticated {
            identity,
            authority,
            ..
        } => Ok(operator_from_identity(*identity, authority)),
        AuthOutcome::Anonymous | AuthOutcome::Unauthenticated => Err(ServiceError::Unauthorized),
        AuthOutcome::Degraded { .. } => Err(ServiceError::Unavailable(
            "shared-auth authorities are temporarily unavailable".to_string(),
        )),
    }
}

fn operator_from_identity(identity: Identity, authority: Authority) -> Operator {
    Operator {
        subject: identity.shared_user_id,
        email: identity.email,
        roles: identity.roles,
        authority,
    }
}

/// Extract a bearer token from an `Authorization` header value.
#[cfg(test)]
pub(crate) fn bearer_token(header: Option<&str>) -> Option<&str> {
    let raw = header?.trim();
    let (scheme, token) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty() && token.len() <= 16 * 1024).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Identity {
        Identity {
            shared_user_id: "shared-operator-1".into(),
            provider: "supabase".into(),
            provider_tenant: "daedalus".into(),
            provider_subject: "supabase-user-1".into(),
            project: Some("daedalus".into()),
            supabase_user_id: Some("supabase-user-1".into()),
            session_id: Some("session-1".into()),
            email: Some("operator@example.com".into()),
            email_verified: true,
            roles: vec!["daedalus-operator".into()],
            authority: Authority::SharedAuth,
        }
    }

    #[test]
    fn shared_identity_maps_to_the_small_service_operator() {
        let operator = operator_from_outcome(AuthOutcome::Authenticated {
            identity: Box::new(identity()),
            authority: Authority::SharedAuth,
            elapsed_ms: 12,
        })
        .expect("authorized operator");

        assert_eq!(operator.subject, "shared-operator-1");
        assert_eq!(operator.email.as_deref(), Some("operator@example.com"));
        assert_eq!(operator.roles, ["daedalus-operator"]);
        assert_eq!(operator.authority, Authority::SharedAuth);
    }

    #[test]
    fn unavailable_authorities_are_not_misreported_as_bad_credentials() {
        let error = operator_from_outcome(AuthOutcome::Degraded {
            reason: "deadline".into(),
        })
        .expect_err("degraded auth must fail closed");
        assert!(matches!(error, ServiceError::Unavailable(_)));
        assert!(matches!(
            operator_from_outcome(AuthOutcome::Unauthenticated),
            Err(ServiceError::Unauthorized)
        ));
    }

    #[test]
    fn bearer_parser_is_bounded_and_scheme_insensitive() {
        assert_eq!(bearer_token(Some("Bearer abc123")), Some("abc123"));
        assert_eq!(bearer_token(Some(" bearer   abc123 ")), Some("abc123"));
        assert_eq!(bearer_token(Some("Basic abc123")), None);
        assert_eq!(bearer_token(Some("Bearer")), None);
        assert_eq!(bearer_token(Some("Bearer   ")), None);
        assert_eq!(bearer_token(None), None);
        let oversized = format!("Bearer {}", "x".repeat(16 * 1024 + 1));
        assert_eq!(bearer_token(Some(&oversized)), None);
    }

    #[tokio::test]
    async fn fixed_test_verifier_exercises_the_same_bearer_boundary() {
        let operator = operator_from_identity(identity(), Authority::SharedAuth);
        let verifier = SharedAuthVerifier::for_test(operator.clone());
        let verified = authorize_bearer(Some(&verifier), Some("Bearer test-token"))
            .await
            .expect("fixed authorization");
        assert_eq!(verified.subject, operator.subject);
        assert!(matches!(
            authorize_bearer(Some(&verifier), None).await,
            Err(ServiceError::Unauthorized)
        ));
        assert!(matches!(
            authorize_bearer(None, Some("Bearer test-token")).await,
            Err(ServiceError::Unavailable(_))
        ));
    }
}
