//! Shared Auth authentication, Quaestor-owned tenant authorization, and
//! outbound URL safety.
//!
//! Shared Auth establishes an active, revocation-aware identity and its
//! authentication assurance. Quaestor then resolves the tenant grant from its
//! own database. These are deliberately separate decisions: a valid identity
//! never implies access to a tenant, and a write scope on one tenant never
//! applies to another tenant.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::memberships::{
    MembershipService, SCOPE_BILLING_ADMIN, SCOPE_BILLING_READ, SCOPE_BILLING_WRITE, TenantGrant,
};
use crate::shared_auth::{AuthError, SharedAuthIdentity, SharedAuthVerifier, bearer_token};
use crate::state::AppState;

/// Compatibility name retained for API/docs that already refer to this scope.
pub const SCOPE_FINANCIAL_WRITE: &str = SCOPE_BILLING_WRITE;
/// Maximum accepted age of the Shared Auth token minted by a completed LOA2
/// ceremony for a human financial mutation.
pub const MAX_STEP_UP_AGE_SECS: u64 = 15 * 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedUser {
    pub identity: SharedAuthIdentity,
    /// Grant for the tenant named in the current request path. `None` for
    /// unscoped routes; embedded-tenant handlers perform their own lookup.
    pub grant: Option<TenantGrant>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Principal {
    /// Legacy process-wide service credential. It is never accepted for a
    /// tenant mutation once mutation hardening is enabled and is rejected from
    /// every tenant route when user-only mode is enabled.
    Service,
    User(Box<AuthorizedUser>),
}

impl Principal {
    pub fn shared_user_id(&self) -> Option<&str> {
        match self {
            Principal::Service => None,
            Principal::User(user) => Some(&user.identity.subject),
        }
    }

    pub fn user(&self) -> AppResult<&AuthorizedUser> {
        match self {
            Principal::User(user) => Ok(user),
            Principal::Service => Err(AppError::Forbidden),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TenantScope {
    None,
    Tenant(Uuid),
    Unparseable,
}

pub fn tenant_scope_of(uri: &Uri) -> TenantScope {
    let Some(rest) = uri.path().strip_prefix("/v1/tenants/") else {
        return TenantScope::None;
    };
    let segment = rest.split('/').next().unwrap_or("");
    if segment.is_empty() {
        return TenantScope::None;
    }
    Uuid::parse_str(segment)
        .map(TenantScope::Tenant)
        .unwrap_or(TenantScope::Unparseable)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Read,
    Mutate,
}

impl Action {
    pub fn of(method: &Method) -> Self {
        match *method {
            Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE => Self::Read,
            _ => Self::Mutate,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AuthzPolicy {
    pub require_user_jwt: bool,
    pub require_step_up_for_mutations: bool,
}

/// Pure authorization decision after authentication and membership lookup.
pub fn authorize_request(
    principal: &Principal,
    scope: TenantScope,
    action: Action,
    policy: AuthzPolicy,
    now: u64,
) -> Result<(), StatusCode> {
    match scope {
        TenantScope::None => {}
        TenantScope::Unparseable => return Err(StatusCode::FORBIDDEN),
        TenantScope::Tenant(tenant_id) => match principal {
            Principal::Service if policy.require_user_jwt => {
                tracing::warn!(
                    tenant.id = %tenant_id,
                    "legacy service credential rejected from tenant route"
                );
                return Err(StatusCode::FORBIDDEN);
            }
            Principal::Service => {}
            Principal::User(user) => {
                let Some(grant) = &user.grant else {
                    tracing::warn!(
                        auth.subject = %user.identity.subject,
                        tenant.id = %tenant_id,
                        "rejected cross-tenant request: no active Quaestor membership"
                    );
                    return Err(StatusCode::FORBIDDEN);
                };
                if grant.tenant_id != tenant_id
                    || grant.shared_user_id != user.identity.subject
                    || !grant.has_scope(SCOPE_BILLING_READ)
                {
                    tracing::warn!(
                        auth.subject = %user.identity.subject,
                        tenant.id = %tenant_id,
                        "rejected request: membership does not match tenant or principal"
                    );
                    return Err(StatusCode::FORBIDDEN);
                }
            }
        },
    }

    if !policy.require_step_up_for_mutations || action == Action::Read {
        return Ok(());
    }
    let TenantScope::Tenant(tenant_id) = scope else {
        // Tenant creation and other unscoped control-plane handlers enforce
        // their resource-specific policy in the handler.
        return Ok(());
    };
    let Principal::User(user) = principal else {
        tracing::warn!(
            tenant.id = %tenant_id,
            "legacy service credential cannot perform a hardened tenant mutation"
        );
        return Err(StatusCode::FORBIDDEN);
    };
    let Some(grant) = &user.grant else {
        return Err(StatusCode::FORBIDDEN);
    };
    if !grant.has_scope(SCOPE_FINANCIAL_WRITE) {
        tracing::warn!(
            auth.subject = %user.identity.subject,
            tenant.id = %tenant_id,
            "financial mutation rejected: membership lacks billing:write"
        );
        return Err(StatusCode::FORBIDDEN);
    }
    if !user.identity.assurance.is_aal2() {
        tracing::warn!(
            auth.subject = %user.identity.subject,
            tenant.id = %tenant_id,
            "financial mutation rejected: Shared Auth session is not LOA2"
        );
        return Err(StatusCode::FORBIDDEN);
    }
    match user.identity.step_up_age_secs(now) {
        Some(age) if age <= MAX_STEP_UP_AGE_SECS => Ok(()),
        _ => {
            tracing::warn!(
                auth.subject = %user.identity.subject,
                tenant.id = %tenant_id,
                "financial mutation rejected: Shared Auth step-up is stale"
            );
            Err(StatusCode::FORBIDDEN)
        }
    }
}

#[derive(Clone)]
pub struct ApiAuth {
    /// Migration-only service credential. Kept redacted on Debug.
    pub bearer: Option<String>,
    pub shared_auth: Option<Arc<SharedAuthVerifier>>,
    pub memberships: Option<MembershipService>,
    pub require_user_jwt: bool,
    pub require_step_up: bool,
}

impl std::fmt::Debug for ApiAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ApiAuth")
            .field("bearer", &self.bearer.as_ref().map(|_| "<redacted>"))
            .field("shared_auth_enabled", &self.shared_auth.is_some())
            .field("memberships_enabled", &self.memberships.is_some())
            .field("require_user_jwt", &self.require_user_jwt)
            .field("require_step_up", &self.require_step_up)
            .finish()
    }
}

impl ApiAuth {
    pub fn from_state(state: &AppState) -> anyhow::Result<Arc<Self>> {
        let shared_auth = SharedAuthVerifier::from_env()?.map(Arc::new);
        if state.cfg.tenant_routes_require_user_jwt && shared_auth.is_none() {
            anyhow::bail!(
                "BILLING_TENANT_ROUTES_REQUIRE_USER_JWT=true requires Shared Auth; configure \
                 BILLING_SHARED_AUTH_BASE_URL and BILLING_SHARED_AUTH_INTROSPECT_SECRET"
            );
        }
        if state.cfg.step_up_required_for_mutations && shared_auth.is_none() {
            anyhow::bail!("BILLING_TENANT_MUTATIONS_REQUIRE_STEP_UP=true requires Shared Auth");
        }
        Ok(Arc::new(Self {
            bearer: state.cfg.api_auth_bearer.clone(),
            shared_auth,
            memberships: Some(state.memberships.clone()),
            require_user_jwt: state.cfg.tenant_routes_require_user_jwt,
            require_step_up: state.cfg.step_up_required_for_mutations,
        }))
    }
}

fn const_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right.iter())
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

pub fn is_exempt_path(uri: &Uri) -> bool {
    let path = uri.path();
    matches!(path, "/healthz" | "/readyz" | "/metrics")
        || path.starts_with("/admin")
        || path.starts_with("/v1/webhooks/")
        || path.starts_with("/v1/verify/")
        || (path.starts_with("/v1/oauth/") && path.ends_with("/callback"))
}

fn unauthorized() -> Response {
    let mut response = (StatusCode::UNAUTHORIZED, "api authentication required\n").into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"quaestor-ledger\""),
    );
    response
}

fn auth_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "authentication temporarily unavailable\n",
    )
        .into_response()
}

pub async fn require_api_auth(
    State(auth): State<Arc<ApiAuth>>,
    mut request: Request,
    next: Next,
) -> Response {
    if is_exempt_path(request.uri()) {
        return next.run(request).await;
    }
    // Config refuses this state outside explicit insecure development.
    if auth.bearer.is_none() && auth.shared_auth.is_none() {
        return next.run(request).await;
    }

    let raw_authorization = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let service_match = match (auth.bearer.as_deref(), raw_authorization) {
        (Some(expected), Some(raw)) => raw.strip_prefix("Bearer ").is_some_and(|presented| {
            !presented.is_empty() && const_time_eq(presented.as_bytes(), expected.as_bytes())
        }),
        _ => false,
    };

    let scope = tenant_scope_of(request.uri());
    let principal = if service_match {
        Principal::Service
    } else {
        let Some(token) = bearer_token(raw_authorization) else {
            return unauthorized();
        };
        let Some(verifier) = &auth.shared_auth else {
            return unauthorized();
        };
        let identity = match verifier.verify(token).await {
            Ok(identity) => identity,
            Err(AuthError::Unauthorized) => return unauthorized(),
            Err(AuthError::Unavailable(error)) => {
                tracing::error!(%error, "Shared Auth introspection unavailable");
                return auth_unavailable();
            }
        };
        let grant = match scope {
            TenantScope::Tenant(tenant_id) => {
                let Some(memberships) = &auth.memberships else {
                    tracing::error!("membership service unavailable");
                    return auth_unavailable();
                };
                match memberships.grant_for(tenant_id, &identity.subject).await {
                    Ok(grant) => grant,
                    Err(error) => {
                        tracing::error!(%error, "tenant membership lookup failed");
                        return auth_unavailable();
                    }
                }
            }
            TenantScope::None | TenantScope::Unparseable => None,
        };
        Principal::User(Box::new(AuthorizedUser { identity, grant }))
    };

    let policy = AuthzPolicy {
        require_user_jwt: auth.require_user_jwt,
        require_step_up_for_mutations: auth.require_step_up,
    };
    if let Err(status) = authorize_request(
        &principal,
        scope,
        Action::of(request.method()),
        policy,
        now_seconds(),
    ) {
        return (status, "forbidden\n").into_response();
    }

    request.extensions_mut().insert(principal);
    next.run(request).await
}

/// Authorization for endpoints whose tenant id is carried in a query/body
/// rather than the canonical `/v1/tenants/{tenant_id}` path (OAuth and Plaid).
/// Service credentials are deliberately refused: provider credentials are a
/// human-controlled, high-impact tenant mutation.
pub async fn require_embedded_tenant_scope(
    state: &AppState,
    principal: &Principal,
    tenant_id: Uuid,
    required_scope: &str,
) -> AppResult<TenantGrant> {
    let user = principal.user()?;
    state
        .memberships
        .require_scope(tenant_id, &user.identity.subject, required_scope)
        .await
}

pub async fn require_embedded_tenant_write(
    state: &AppState,
    principal: &Principal,
    tenant_id: Uuid,
) -> AppResult<TenantGrant> {
    let grant =
        require_embedded_tenant_scope(state, principal, tenant_id, SCOPE_BILLING_WRITE).await?;
    require_fresh_user_step_up(state, principal)?;
    Ok(grant)
}

pub async fn require_embedded_tenant_admin(
    state: &AppState,
    principal: &Principal,
    tenant_id: Uuid,
) -> AppResult<TenantGrant> {
    let grant =
        require_embedded_tenant_scope(state, principal, tenant_id, SCOPE_BILLING_ADMIN).await?;
    require_fresh_user_step_up(state, principal)?;
    Ok(grant)
}

/// Require the active Shared Auth user to have completed a recent LOA2
/// ceremony. Used for unscoped control-plane mutations such as creating a new
/// billing tenant, as well as embedded-tenant routes.
pub fn require_fresh_user_step_up(state: &AppState, principal: &Principal) -> AppResult<()> {
    let user = principal.user()?;
    if !state.cfg.step_up_required_for_mutations {
        return Ok(());
    }
    let fresh = matches!(
        user.identity.step_up_age_secs(now_seconds()),
        Some(age) if age <= MAX_STEP_UP_AGE_SECS
    );
    if user.identity.assurance.is_aal2() && fresh {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

// --- Outbound URL safety helpers --------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum UrlSafety {
    Allowed,
    BlockedPrivate,
    BlockedScheme,
    Malformed,
}

pub fn classify_outbound_url(url: &str) -> UrlSafety {
    let parsed = match url::Url::parse(url) {
        Ok(url) => url,
        Err(_) => return UrlSafety::Malformed,
    };
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return UrlSafety::BlockedScheme,
    }
    let Some(host) = parsed.host() else {
        return UrlSafety::Malformed;
    };
    let ip = match host {
        url::Host::Ipv4(ip) => Some(IpAddr::V4(ip)),
        url::Host::Ipv6(ip) => Some(IpAddr::V6(ip)),
        url::Host::Domain(_) => None,
    };
    match ip {
        Some(ip) if is_private_ip(ip) => UrlSafety::BlockedPrivate,
        _ => UrlSafety::Allowed,
    }
}

pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || ip.is_documentation()
                || matches!(ip.octets(), [100, second, ..] if (64..=127).contains(&second))
                || ip.octets()[0] == 0
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
                || ip
                    .to_ipv4_mapped()
                    .map(|mapped| is_private_ip(IpAddr::V4(mapped)))
                    .unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_auth::{ACR_LOA1, ACR_LOA2, Aal};
    use chrono::Utc;

    const TENANT_A: &str = "11111111-1111-4111-8111-111111111111";
    const TENANT_B: &str = "22222222-2222-4222-8222-222222222222";
    const NOW: u64 = 1_000_000;

    fn tenant(value: &str) -> Uuid {
        Uuid::parse_str(value).unwrap()
    }

    fn identity(aal: Aal, issued_at: u64) -> SharedAuthIdentity {
        SharedAuthIdentity {
            subject: "shared-user-1".to_owned(),
            provider: "local".to_owned(),
            provider_tenant: "default".to_owned(),
            provider_subject: "shared-user-1".to_owned(),
            session_id: Some("session-1".to_owned()),
            email: Some("operator@example.com".to_owned()),
            email_verified: true,
            roles: vec!["user".to_owned()],
            assurance: aal,
            amr: if aal.is_aal2() {
                vec!["pwd".to_owned(), "totp".to_owned()]
            } else {
                vec!["pwd".to_owned()]
            },
            acr: Some(if aal.is_aal2() { ACR_LOA2 } else { ACR_LOA1 }.to_owned()),
            issued_at,
            expires_at: NOW + 3600,
        }
    }

    fn grant(tenant_id: Uuid, scopes: &[&str]) -> TenantGrant {
        TenantGrant {
            tenant_id,
            shared_user_id: "shared-user-1".to_owned(),
            role: "billing".to_owned(),
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn user(tenant_id: Uuid, scopes: &[&str], aal: Aal, issued_at: u64) -> Principal {
        Principal::User(Box::new(AuthorizedUser {
            identity: identity(aal, issued_at),
            grant: Some(grant(tenant_id, scopes)),
        }))
    }

    fn hardened() -> AuthzPolicy {
        AuthzPolicy {
            require_user_jwt: true,
            require_step_up_for_mutations: true,
        }
    }

    #[test]
    fn path_scope_is_fail_closed() {
        assert_eq!(
            tenant_scope_of(&format!("/v1/tenants/{TENANT_A}/users").parse().unwrap()),
            TenantScope::Tenant(tenant(TENANT_A))
        );
        assert_eq!(
            tenant_scope_of(&"/v1/tenants/not-a-uuid/users".parse().unwrap()),
            TenantScope::Unparseable
        );
        assert_eq!(
            tenant_scope_of(&"/v1/tenants".parse().unwrap()),
            TenantScope::None
        );
    }

    #[test]
    fn grant_is_bound_to_exact_tenant_and_subject() {
        let principal = user(
            tenant(TENANT_A),
            &[SCOPE_BILLING_READ, SCOPE_BILLING_WRITE],
            Aal::Aal2,
            NOW,
        );
        assert_eq!(
            authorize_request(
                &principal,
                TenantScope::Tenant(tenant(TENANT_A)),
                Action::Read,
                hardened(),
                NOW
            ),
            Ok(())
        );
        assert_eq!(
            authorize_request(
                &principal,
                TenantScope::Tenant(tenant(TENANT_B)),
                Action::Read,
                hardened(),
                NOW
            ),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn write_scope_cannot_leak_between_tenants() {
        let mut principal = user(
            tenant(TENANT_A),
            &[SCOPE_BILLING_READ, SCOPE_BILLING_WRITE],
            Aal::Aal2,
            NOW,
        );
        let Principal::User(user) = &mut principal else {
            unreachable!()
        };
        // Simulate a lookup bug returning A's write grant for B. The tenant id
        // binding still denies before the scope can be considered.
        user.grant.as_mut().unwrap().tenant_id = tenant(TENANT_A);
        assert_eq!(
            authorize_request(
                &principal,
                TenantScope::Tenant(tenant(TENANT_B)),
                Action::Mutate,
                hardened(),
                NOW
            ),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn mutation_requires_tenant_write_scope_and_fresh_loa2() {
        let allowed = user(
            tenant(TENANT_A),
            &[SCOPE_BILLING_READ, SCOPE_BILLING_WRITE],
            Aal::Aal2,
            NOW,
        );
        assert_eq!(
            authorize_request(
                &allowed,
                TenantScope::Tenant(tenant(TENANT_A)),
                Action::Mutate,
                hardened(),
                NOW
            ),
            Ok(())
        );
        let read_only = user(tenant(TENANT_A), &[SCOPE_BILLING_READ], Aal::Aal2, NOW);
        let weak = user(
            tenant(TENANT_A),
            &[SCOPE_BILLING_READ, SCOPE_BILLING_WRITE],
            Aal::Aal1,
            NOW,
        );
        let stale = user(
            tenant(TENANT_A),
            &[SCOPE_BILLING_READ, SCOPE_BILLING_WRITE],
            Aal::Aal2,
            NOW - MAX_STEP_UP_AGE_SECS - 1,
        );
        for denied in [&read_only, &weak, &stale] {
            assert_eq!(
                authorize_request(
                    denied,
                    TenantScope::Tenant(tenant(TENANT_A)),
                    Action::Mutate,
                    hardened(),
                    NOW
                ),
                Err(StatusCode::FORBIDDEN)
            );
        }
    }

    #[test]
    fn service_principal_is_not_a_tenant_user() {
        assert_eq!(
            authorize_request(
                &Principal::Service,
                TenantScope::Tenant(tenant(TENANT_A)),
                Action::Read,
                hardened(),
                NOW
            ),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn exemptions_are_narrow() {
        for path in [
            "/healthz",
            "/readyz",
            "/metrics",
            "/v1/webhooks/stripe",
            "/v1/verify/x/y",
            "/v1/oauth/stripe/callback",
        ] {
            assert!(is_exempt_path(&path.parse().unwrap()), "{path}");
        }
        for path in [
            "/v1/tenants",
            "/v1/oauth/stripe/start",
            "/v1/plaid/link-token",
        ] {
            assert!(!is_exempt_path(&path.parse().unwrap()), "{path}");
        }
    }

    #[test]
    fn outbound_url_policy_still_blocks_literal_private_targets() {
        assert_eq!(
            classify_outbound_url("https://api.example.com/x"),
            UrlSafety::Allowed
        );
        assert_eq!(
            classify_outbound_url("http://169.254.169.254/latest/meta-data"),
            UrlSafety::BlockedPrivate
        );
        assert_eq!(
            classify_outbound_url("file:///etc/passwd"),
            UrlSafety::BlockedScheme
        );
    }

    #[test]
    fn debug_never_exposes_service_credential() {
        let auth = ApiAuth {
            bearer: Some("super-secret".to_owned()),
            shared_auth: None,
            memberships: None,
            require_user_jwt: true,
            require_step_up: true,
        };
        let rendered = format!("{auth:?}");
        assert!(!rendered.contains("super-secret"));
        assert!(rendered.contains("<redacted>"));
    }
}
