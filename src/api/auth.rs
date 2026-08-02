//! API authentication, per-tenant authorization, and outbound URL safety.
//!
//! ### Auth model (read this first)
//!
//! There are two kinds of caller, and they are not interchangeable.
//!
//! **1. Service callers** present the shared
//! [`BILLING_API_AUTH_BEARER`](Config::api_auth_bearer) token. That token is a
//! single process-wide secret: it proves *something authorized is calling* and
//! nothing else. It carries no user, and no tenant. Treat it as a
//! service-to-service credential only — it must never reach an end user or a
//! client application.
//!
//! **2. User callers** present a Supabase access token, verified per-request by
//! [`crate::supabase_auth`]: real signature checking against the project's
//! JWKS, with `iss`/`aud`/`exp`/`nbf` pinned.
//!
//! ### The IDOR this closes
//!
//! Historically only kind (1) existed. Every tenant-scoped route takes the
//! tenant from the URL — `/v1/tenants/{tenant_id}/...` — and the service simply
//! trusted it, delegating ownership entirely to an upstream gateway
//! (`dd-remote-auth`). So any holder of the one shared token could read or
//! mutate *any* tenant's ledger, connections, and scheduled jobs by editing a
//! path segment, and nothing in this process would object. Anything that
//! bypassed the gateway — a port-forward, cluster-internal access, a
//! misconfigured ingress — inherited the whole estate.
//!
//! [`authorize_tenant`] is the fix: after authentication, the caller must be
//! provably entitled to *the tenant named in the path*, in-process, or the
//! request gets a 403. See [`TenantScope`] for how the tenant is extracted and
//! [`Principal`] for what each caller kind is allowed.
//!
//! ### Exemptions
//!
//! Webhooks (`/v1/webhooks/*`) and public verification (`/v1/verify/*`) have
//! their own auth models — provider signatures, and "the data is deliberately
//! public" respectively. Health endpoints are unauthenticated for probes, and
//! `/admin` has its own bearer in [`crate::admin::security`].

use std::net::IpAddr;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::config::Config;
use crate::supabase_auth::{AuthError, SupabaseIdentity, SupabaseVerifier, bearer_token};

/// Financial scope a human caller must hold to mutate a tenant's ledger state.
/// A canonical Shared Auth scope name; it must match what the issuer grants.
pub const SCOPE_FINANCIAL_WRITE: &str = "billing:write";

/// Maximum age of a step-up (fresh AAL2) accepted for a financial mutation.
/// Beyond this, a human mutation must re-assert the second factor.
pub const MAX_STEP_UP_AGE_SECS: u64 = 15 * 60;

/// Who is making this request, once authenticated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Principal {
    /// Presented the shared `BILLING_API_AUTH_BEARER`. Authentic, but
    /// anonymous and tenant-less.
    Service,
    /// Presented a Supabase access token that verified.
    User(Box<SupabaseIdentity>),
}

/// What tenant, if any, a request path is scoped to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TenantScope {
    /// Not a tenant-scoped route (`POST /v1/tenants`, `/v1/oauth/...`, …).
    None,
    /// `/v1/tenants/{tenant_id}/...` with a well-formed id.
    Tenant(Uuid),
    /// `/v1/tenants/<something>/...` where `<something>` is not a UUID.
    ///
    /// Kept distinct from [`TenantScope::None`] on purpose. Collapsing the two
    /// would mean a caller could skip the entitlement check simply by sending a
    /// tenant id we cannot parse, which is exactly the bypass this whole module
    /// exists to prevent.
    Unparseable,
}

/// Extract the tenant a request is scoped to from its path.
///
/// This reads the raw path rather than axum's `Path` extractor because the
/// decision has to be made in middleware, before any handler runs, and must
/// hold for every current *and future* `/v1/tenants/{tenant_id}/...` route
/// without anyone remembering to opt in.
pub fn tenant_scope_of(uri: &Uri) -> TenantScope {
    let Some(rest) = uri.path().strip_prefix("/v1/tenants/") else {
        // Includes bare `/v1/tenants` (tenant *creation*, which has no tenant
        // to be scoped to).
        return TenantScope::None;
    };
    let segment = rest.split('/').next().unwrap_or("");
    if segment.is_empty() {
        return TenantScope::None;
    }
    match Uuid::parse_str(segment) {
        Ok(id) => TenantScope::Tenant(id),
        Err(_) => TenantScope::Unparseable,
    }
}

/// The per-tenant authorization decision.
///
/// Separated from the middleware so it can be tested exhaustively without
/// standing up a router, and so the rule lives in exactly one readable place.
pub fn authorize_tenant(
    principal: &Principal,
    scope: TenantScope,
    require_user_jwt: bool,
) -> Result<(), StatusCode> {
    match scope {
        // Not tenant-scoped: authentication alone is the whole check.
        TenantScope::None => Ok(()),
        // A tenant-scoped route whose tenant we cannot even name. Refuse
        // rather than fall through to the unscoped branch.
        TenantScope::Unparseable => Err(StatusCode::FORBIDDEN),
        TenantScope::Tenant(tenant_id) => match principal {
            Principal::User(identity) => {
                if identity.is_entitled_to(tenant_id) {
                    Ok(())
                } else {
                    // The IDOR, closed: a real, fully-verified user asking for
                    // a tenant that is not theirs.
                    tracing::warn!(
                        auth.subject = %identity.subject,
                        tenant.id = %tenant_id,
                        "rejected cross-tenant request: caller is not entitled to this tenant"
                    );
                    Err(StatusCode::FORBIDDEN)
                }
            }
            Principal::Service => {
                if require_user_jwt {
                    tracing::warn!(
                        tenant.id = %tenant_id,
                        "rejected service-bearer request to a tenant-scoped route: \
                         BILLING_TENANT_ROUTES_REQUIRE_USER_JWT is on, so this route \
                         needs a per-user Supabase token"
                    );
                    Err(StatusCode::FORBIDDEN)
                } else {
                    // Migration window only. The shared token names no tenant,
                    // so this branch is the pre-fix behaviour and is exactly
                    // what BILLING_TENANT_ROUTES_REQUIRE_USER_JWT=true removes.
                    Ok(())
                }
            }
        },
    }
}

/// Whether a request reads or mutates, derived from the HTTP method. Unknown or
/// non-safe methods are treated as mutations so a new verb can never slip past
/// the financial step-up gate by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Read,
    Mutate,
}

impl Action {
    pub fn of(method: &Method) -> Self {
        match *method {
            Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE => Action::Read,
            _ => Action::Mutate,
        }
    }
}

/// The two migration-gated authorization policies, bundled so the decision
/// function and the middleware agree on exactly one shape.
#[derive(Clone, Copy, Debug)]
pub struct AuthzPolicy {
    /// See [`Config::tenant_routes_require_user_jwt`].
    pub require_user_jwt: bool,
    /// See [`Config::step_up_required_for_mutations`]. When on, a human mutation
    /// of a tenant's financial state requires fresh AAL2 and an explicit scope.
    pub require_step_up_for_mutations: bool,
}

/// Full per-request authorization.
///
/// Layered deliberately, most-fundamental first, each in one readable place:
///   1. **Tenant entitlement** — the IDOR fix ([`authorize_tenant`]).
///   2. **Financial step-up** — a *human mutation* of a named tenant additionally
///      requires a genuine, *fresh* AAL2 session and an explicit financial scope.
///
/// Reads and unscoped provisioning calls are not subject to step-up. The legacy
/// shared service credential carries no tenant, role, assurance, or scope, so
/// mutation hardening denies it on every named tenant write until tenant-bound
/// service identities exist. `now` is the current Unix time, used only to measure
/// step-up freshness. Pure, so it is exhaustively unit-tested without a router.
pub fn authorize_request(
    principal: &Principal,
    scope: TenantScope,
    action: Action,
    policy: AuthzPolicy,
    now: u64,
) -> Result<(), StatusCode> {
    authorize_tenant(principal, scope, policy.require_user_jwt)?;

    if !policy.require_step_up_for_mutations {
        return Ok(());
    }
    // Only human mutations of a *named* tenant are financial-mutation-gated.
    let (Action::Mutate, TenantScope::Tenant(tenant_id)) = (action, scope) else {
        return Ok(());
    };
    let Principal::User(identity) = principal else {
        tracing::warn!(
            tenant.id = %tenant_id,
            "rejected tenant mutation: shared service credential has no tenant, assurance, or write scope"
        );
        return Err(StatusCode::FORBIDDEN);
    };

    if !identity.assurance.is_aal2() {
        tracing::warn!(
            auth.subject = %identity.subject, tenant.id = %tenant_id,
            "rejected financial mutation: session is not AAL2 (step-up required)"
        );
        return Err(StatusCode::FORBIDDEN);
    }
    match identity.step_up_age_secs(now) {
        Some(age) if age <= MAX_STEP_UP_AGE_SECS => {}
        _ => {
            tracing::warn!(
                auth.subject = %identity.subject, tenant.id = %tenant_id,
                "rejected financial mutation: AAL2 step-up is stale or absent"
            );
            return Err(StatusCode::FORBIDDEN);
        }
    }
    if !identity.has_scope(SCOPE_FINANCIAL_WRITE) {
        tracing::warn!(
            auth.subject = %identity.subject, tenant.id = %tenant_id,
            auth.scope = SCOPE_FINANCIAL_WRITE,
            "rejected financial mutation: caller is missing the required financial scope"
        );
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// Per-request auth state. Built once at boot and shared via `Arc` so the
/// middleware closure doesn't carry the full `AppState`.
#[derive(Clone)]
pub struct ApiAuth {
    /// Shared service-to-service bearer. See [`Config::api_auth_bearer`].
    pub bearer: Option<String>,
    /// Per-user Supabase verifier. `None` when Supabase is not configured.
    pub supabase: Option<Arc<SupabaseVerifier>>,
    /// See [`Config::tenant_routes_require_user_jwt`].
    pub require_user_jwt: bool,
    /// See [`Config::step_up_required_for_mutations`].
    pub require_step_up: bool,
}

// `bearer` is a credential; keep it off the Debug surface, matching the
// redaction discipline `Config` follows.
impl std::fmt::Debug for ApiAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiAuth")
            .field("bearer", &self.bearer.as_ref().map(|_| "<redacted>"))
            .field("supabase_enabled", &self.supabase.is_some())
            .field("require_user_jwt", &self.require_user_jwt)
            .field("require_step_up", &self.require_step_up)
            .finish()
    }
}

impl ApiAuth {
    pub fn from_config(cfg: &Config) -> Arc<Self> {
        Arc::new(Self {
            bearer: cfg.api_auth_bearer.clone(),
            supabase: SupabaseVerifier::from_config(&cfg.supabase).map(Arc::new),
            require_user_jwt: cfg.tenant_routes_require_user_jwt,
            require_step_up: cfg.step_up_required_for_mutations,
        })
    }
}

/// Constant-time byte-slice equality. Length mismatch returns early
/// (length leak is fine for a fixed-shape opaque token).
fn const_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// True when this URI is exempt from bearer auth.
///
/// Exempted paths:
///   - `/healthz`, `/readyz`, `/metrics` — orchestrator probes
///   - `/v1/webhooks/*` — provider-signature gated
///   - `/v1/verify/*` — explicitly public by design
///   - `/v1/oauth/*/callback` — auth happens via the single-use CSRF
///     state token in the URL
///   - `/admin/*` — admin has its own bearer middleware
///
/// Everything else (including OAuth `/start`, Plaid endpoints,
/// connection, ledger, scheduler, etc.) requires the bearer.
pub fn is_exempt_path(uri: &Uri) -> bool {
    let path = uri.path();
    if matches!(path, "/healthz" | "/readyz" | "/metrics") {
        return true;
    }
    if path.starts_with("/admin") {
        return true;
    }
    if path.starts_with("/v1/webhooks/") {
        return true;
    }
    if path.starts_with("/v1/verify/") {
        return true;
    }
    // OAuth callback uses the single-use `state` parameter as its own
    // CSRF token; requiring a bearer here would break the redirect
    // flow from the provider.
    if path.starts_with("/v1/oauth/") && path.ends_with("/callback") {
        return true;
    }
    false
}

fn unauthorized() -> Response {
    let mut resp = (StatusCode::UNAUTHORIZED, "api authentication required\n").into_response();
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"billing-api\""),
    );
    resp
}

/// Authenticate the caller, then authorize them for the tenant in the path.
///
/// The two steps are deliberately distinct: step one establishes *who*, step
/// two establishes *what they may touch*. Conflating them is how the original
/// IDOR happened — the service knew the caller was authentic and inferred,
/// wrongly, that this made the request legitimate.
pub async fn require_api_auth(
    State(auth): State<Arc<ApiAuth>>,
    mut req: Request,
    next: Next,
) -> Response {
    if is_exempt_path(req.uri()) {
        return next.run(req).await;
    }

    // Fully-open dev mode: no service bearer and no Supabase. `Config::from_env`
    // refuses to boot into this state unless BILLING_ALLOW_INSECURE_DEV=1.
    if auth.bearer.is_none() && auth.supabase.is_none() {
        return next.run(req).await;
    }

    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    // The same header carries both credential kinds, so try the cheap
    // constant-time comparison first and fall back to JWT verification. A
    // Supabase token can never be mistaken for the service bearer: the compare
    // is over the entire string.
    //
    // The service-bearer match stays byte-exact on the `Bearer ` prefix,
    // unchanged from before per-user auth existed. The JWT path uses the
    // RFC 7235 case-insensitive parser instead, because real Supabase client
    // SDKs differ in how they spell the scheme — and unlike the shared secret,
    // a JWT's authenticity does not rest on the header being byte-identical.
    let service_match = match (auth.bearer.as_deref(), presented) {
        (Some(expected), Some(raw)) => raw
            .strip_prefix("Bearer ")
            .is_some_and(|t| !t.is_empty() && const_time_eq(t.as_bytes(), expected.as_bytes())),
        _ => false,
    };

    let principal = if service_match {
        Principal::Service
    } else if let (Some(verifier), Some(token)) = (auth.supabase.as_ref(), bearer_token(presented))
    {
        match verifier.verify(token).await {
            Ok(identity) => Principal::User(Box::new(identity)),
            Err(AuthError::Unauthorized) => return unauthorized(),
            Err(AuthError::Unavailable(message)) => {
                // We could not reach Supabase. That is our failure, not the
                // caller's — a 401 here would tell a legitimate user their
                // credentials are bad and invite them to re-login pointlessly.
                tracing::error!(error = %message, "Supabase verification unavailable");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "authentication temporarily unavailable\n",
                )
                    .into_response();
            }
        }
    } else {
        return unauthorized();
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let policy = AuthzPolicy {
        require_user_jwt: auth.require_user_jwt,
        require_step_up_for_mutations: auth.require_step_up,
    };
    if let Err(status) = authorize_request(
        &principal,
        tenant_scope_of(req.uri()),
        Action::of(req.method()),
        policy,
        now,
    ) {
        return (status, "forbidden\n").into_response();
    }

    // Hand the verified principal to the handlers. Anything needing the acting
    // user (audit trails, narrowing a query) reads it from here rather than
    // re-parsing the token.
    req.extensions_mut().insert(principal);
    next.run(req).await
}

// --- Outbound URL safety helpers --------------------------------------------

/// Result of validating a tenant-supplied URL we're about to POST to.
#[derive(Debug, PartialEq, Eq)]
pub enum UrlSafety {
    Allowed,
    /// URL host resolves to a private/loopback/link-local IP. Refused
    /// to prevent the billing server from being used as a probe into
    /// the cluster's internal services.
    BlockedPrivate,
    /// Scheme other than http/https (e.g. `file:`, `gopher:`).
    BlockedScheme,
    /// Host could not be parsed.
    Malformed,
}

/// Decide whether `url` is safe to POST to from a tenant-supplied
/// webhook URL (e.g. notification channel, `tenant.webhook` job).
///
/// We block:
///   * non-http(s) schemes (file://, gopher://, etc.)
///   * literal private / loopback / link-local IPs (10/8, 172.16/12,
///     192.168/16, 127/8, 169.254/16, 100.64/10, and the IPv6
///     equivalents fc00::/7, ::1, fe80::/10, ::ffff:* mapped private)
///   * the metadata IP 169.254.169.254 (covered by link-local)
///
/// DNS-only hostnames are allowed without resolving them here — we
/// trust `reqwest` to resolve them and the cluster network policy to
/// drop egress to private CIDRs at the network layer. This function
/// is the *literal-IP* defense; the network policy is the
/// *DNS-rebinding* defense.
pub fn classify_outbound_url(url: &str) -> UrlSafety {
    let parsed = match url::Url::parse(url) {
        Ok(u) => u,
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

/// True for IPv4/IPv6 addresses that should never be the target of a
/// tenant-controlled HTTP POST. Includes loopback (`127/8`, `::1`),
/// link-local (`169.254/16`, `fe80::/10`), CGNAT (`100.64/10`),
/// private (`10/8`, `172.16/12`, `192.168/16`), and the IPv6 unique
/// local block (`fc00::/7`).
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_documentation()
                // CGNAT 100.64.0.0/10 (not exposed via the stdlib helper).
                || matches!(v4.octets(), [100, b, ..] if (64..=127).contains(&b))
                // 0.0.0.0/8 — "this network", routable to loopback in
                // some misconfigs.
                || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                // Unique local fc00::/7.
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // Link-local fe80::/10.
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                // IPv4-mapped — check the embedded v4 recursively.
                || v6
                    .to_ipv4_mapped()
                    .map(|v4| is_private_ip(IpAddr::V4(v4)))
                    .unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supabase_auth::Aal;
    use std::net::Ipv4Addr;

    #[test]
    fn ct_eq_basic() {
        assert!(const_time_eq(b"a", b"a"));
        assert!(!const_time_eq(b"a", b"b"));
        assert!(!const_time_eq(b"a", b"aa"));
        assert!(const_time_eq(b"", b""));
    }

    #[test]
    fn exempt_paths_recognised() {
        for p in [
            "/healthz",
            "/readyz",
            "/metrics",
            "/admin",
            "/admin/tenants",
            "/v1/webhooks/stripe",
            "/v1/webhooks/fireblocks",
            "/v1/verify/tenants/00000000-0000-0000-0000-000000000000/postings/1",
            "/v1/oauth/stripe/callback",
        ] {
            let uri: Uri = p.parse().unwrap();
            assert!(is_exempt_path(&uri), "{p} should be exempt");
        }
    }

    #[test]
    fn non_exempt_paths_require_auth() {
        for p in [
            "/v1/tenants",
            "/v1/tenants/00000000-0000-0000-0000-000000000000",
            "/v1/oauth/stripe/start",
            "/v1/plaid/link-token",
            "/v1/plaid/exchange",
            "/v1/tenants/00000000-0000-0000-0000-000000000000/connections",
            "/v1/tenants/00000000-0000-0000-0000-000000000000/scheduled-jobs",
        ] {
            let uri: Uri = p.parse().unwrap();
            assert!(!is_exempt_path(&uri), "{p} should NOT be exempt");
        }
    }

    #[test]
    fn private_ips_v4() {
        let cases = [
            "10.0.0.1",
            "10.255.255.254",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.1.1",
            "127.0.0.1",
            "169.254.169.254", // metadata
            "100.64.0.1",      // CGNAT
            "0.0.0.0",
        ];
        for c in cases {
            let ip: Ipv4Addr = c.parse().unwrap();
            assert!(is_private_ip(IpAddr::V4(ip)), "{c} should be private");
        }
    }

    #[test]
    fn public_ips_v4() {
        let cases = ["1.1.1.1", "8.8.8.8", "13.107.6.152", "172.32.0.1"];
        for c in cases {
            let ip: Ipv4Addr = c.parse().unwrap();
            assert!(!is_private_ip(IpAddr::V4(ip)), "{c} should be public");
        }
    }

    #[test]
    fn private_ips_v6() {
        for c in ["::1", "fe80::1", "fc00::1", "fd12::1", "::ffff:127.0.0.1"] {
            let ip: IpAddr = c.parse().unwrap();
            assert!(is_private_ip(ip), "{c} should be private v6");
        }
    }

    #[test]
    fn public_ips_v6() {
        for c in ["2606:4700:4700::1111", "2001:4860:4860::8888"] {
            let ip: IpAddr = c.parse().unwrap();
            assert!(!is_private_ip(ip), "{c} should be public v6");
        }
    }

    #[test]
    fn classify_url_paths() {
        assert_eq!(
            classify_outbound_url("https://api.example.com/x"),
            UrlSafety::Allowed
        );
        assert_eq!(
            classify_outbound_url("http://127.0.0.1:9000/x"),
            UrlSafety::BlockedPrivate
        );
        assert_eq!(
            classify_outbound_url("http://169.254.169.254/latest/meta-data/"),
            UrlSafety::BlockedPrivate
        );
        assert_eq!(
            classify_outbound_url("http://[::1]/x"),
            UrlSafety::BlockedPrivate
        );
        assert_eq!(
            classify_outbound_url("file:///etc/passwd"),
            UrlSafety::BlockedScheme
        );
        assert_eq!(classify_outbound_url("not-a-url"), UrlSafety::Malformed);
    }

    #[test]
    fn domains_pass_through_classify() {
        // We deliberately don't resolve domains here — DNS-rebinding is
        // handled at the network policy layer. So `localhost` (which is
        // a domain, not a literal IP) passes classification but will be
        // dropped by the cluster egress rules.
        assert_eq!(
            classify_outbound_url("https://localhost/x"),
            UrlSafety::Allowed
        );
    }

    // --- Integration: require_api_auth middleware via a tiny router ---

    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, header};
    use axum::routing::{get, post};
    use tower::ServiceExt;

    fn auth_arc(bearer: Option<&str>) -> Arc<ApiAuth> {
        Arc::new(ApiAuth {
            bearer: bearer.map(str::to_string),
            // These tests cover the static service-bearer behaviour, which must
            // be unchanged by the addition of per-user auth. The Supabase and
            // per-tenant paths have their own tests below and in
            // `crate::supabase_auth`.
            supabase: None,
            require_user_jwt: false,
            require_step_up: false,
        })
    }

    fn build_test_router(auth: Arc<ApiAuth>) -> Router {
        Router::new()
            .route("/v1/tenants", post(|| async { "ok-tenants" }))
            .route(
                "/v1/tenants/{tenant_id}/connections",
                get(|| async { "ok-conn" }),
            )
            .route("/v1/webhooks/stripe", post(|| async { "ok-stripe" }))
            .route("/v1/verify/x/y", get(|| async { "ok-verify" }))
            .route("/v1/oauth/stripe/start", get(|| async { "ok-oauth-start" }))
            .route("/v1/oauth/stripe/callback", get(|| async { "ok-oauth-cb" }))
            .route("/healthz", get(|| async { "ok-health" }))
            .layer(axum::middleware::from_fn_with_state(
                auth.clone(),
                require_api_auth,
            ))
            .with_state(auth)
    }

    async fn status_of(
        router: Router,
        method: &str,
        uri: &str,
        auth_header: Option<&str>,
    ) -> StatusCode {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(h) = auth_header {
            req = req.header(header::AUTHORIZATION, h);
        }
        let req = req.body(Body::empty()).unwrap();
        router.oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn no_bearer_configured_lets_everything_through() {
        let app = build_test_router(auth_arc(None));
        assert_eq!(
            status_of(app.clone(), "POST", "/v1/tenants", None).await,
            StatusCode::OK
        );
        assert_eq!(
            status_of(app, "GET", "/healthz", None).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn bearer_configured_rejects_missing_header() {
        let app = build_test_router(auth_arc(Some("hunter2")));
        assert_eq!(
            status_of(app.clone(), "POST", "/v1/tenants", None).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status_of(
                app,
                "GET",
                "/v1/tenants/00000000-0000-0000-0000-000000000000/connections",
                None
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn bearer_configured_rejects_wrong_token() {
        let app = build_test_router(auth_arc(Some("hunter2")));
        assert_eq!(
            status_of(app, "POST", "/v1/tenants", Some("Bearer not-the-token")).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn bearer_configured_accepts_correct_token() {
        let app = build_test_router(auth_arc(Some("hunter2")));
        assert_eq!(
            status_of(app, "POST", "/v1/tenants", Some("Bearer hunter2")).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn exempt_paths_bypass_bearer() {
        let app = build_test_router(auth_arc(Some("hunter2")));
        for (method, uri) in [
            ("GET", "/healthz"),
            ("POST", "/v1/webhooks/stripe"),
            ("GET", "/v1/verify/x/y"),
            ("GET", "/v1/oauth/stripe/callback"),
        ] {
            assert_eq!(
                status_of(app.clone(), method, uri, None).await,
                StatusCode::OK,
                "{method} {uri} should bypass bearer"
            );
        }
    }

    #[tokio::test]
    async fn oauth_start_is_not_exempt() {
        let app = build_test_router(auth_arc(Some("hunter2")));
        // OAuth /start mints CSRF state for a tenant — must be
        // authenticated even though /callback is open.
        assert_eq!(
            status_of(app, "GET", "/v1/oauth/stripe/start", None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn rejects_bearer_prefix_typo() {
        let app = build_test_router(auth_arc(Some("hunter2")));
        // Capital B / lower-case-only "bearer" / no space.
        for h in ["bearer hunter2", "BEARER hunter2", "Bearer  hunter2"] {
            assert_eq!(
                status_of(app.clone(), "POST", "/v1/tenants", Some(h)).await,
                StatusCode::UNAUTHORIZED,
                "{h:?} must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn rejects_empty_bearer_value() {
        let app = build_test_router(auth_arc(Some("hunter2")));
        assert_eq!(
            status_of(app, "POST", "/v1/tenants", Some("Bearer ")).await,
            StatusCode::UNAUTHORIZED
        );
    }

    // --- Tenant scope extraction ---------------------------------------------

    const TENANT_A: &str = "11111111-1111-4111-8111-111111111111";
    const TENANT_B: &str = "22222222-2222-4222-8222-222222222222";

    fn uuid_a() -> Uuid {
        Uuid::parse_str(TENANT_A).unwrap()
    }

    fn uuid_b() -> Uuid {
        Uuid::parse_str(TENANT_B).unwrap()
    }

    fn scope(path: &str) -> TenantScope {
        tenant_scope_of(&path.parse::<Uri>().unwrap())
    }

    #[test]
    fn tenant_scope_is_extracted_from_every_tenant_route_shape() {
        for path in [
            "/v1/tenants/11111111-1111-4111-8111-111111111111",
            "/v1/tenants/11111111-1111-4111-8111-111111111111/users",
            "/v1/tenants/11111111-1111-4111-8111-111111111111/connections",
            "/v1/tenants/11111111-1111-4111-8111-111111111111/scheduled-jobs/7/runs",
            "/v1/tenants/11111111-1111-4111-8111-111111111111/locks/some-resource/renew",
            "/v1/tenants/11111111-1111-4111-8111-111111111111/customers/by-email/a@b.com/billing-state",
        ] {
            assert_eq!(scope(path), TenantScope::Tenant(uuid_a()), "{path}");
        }
    }

    #[test]
    fn non_tenant_routes_have_no_scope() {
        // `POST /v1/tenants` creates a tenant, so there is no tenant to be
        // scoped to; it stays a service-to-service provisioning call.
        for path in [
            "/v1/tenants",
            "/v1/tenants/",
            "/v1/oauth/stripe/start",
            "/v1/plaid/link-token",
            "/healthz",
        ] {
            assert_eq!(scope(path), TenantScope::None, "{path}");
        }
    }

    #[test]
    fn an_unparseable_tenant_id_is_not_treated_as_unscoped() {
        // Otherwise "send a tenant id we can't parse" would be a free bypass of
        // the entitlement check.
        for path in [
            "/v1/tenants/not-a-uuid/users",
            "/v1/tenants/../../etc/passwd",
            "/v1/tenants/%2e%2e/users",
            "/v1/tenants/11111111-1111-4111-8111-11111111111/users", // one digit short
        ] {
            assert_eq!(scope(path), TenantScope::Unparseable, "{path}");
        }
    }

    // --- The IDOR fix: per-tenant authorization ------------------------------

    /// A fully-privileged human: AAL2, financial scope, and a fresh step-up.
    /// Keeps the existing `authorize_tenant` tests (which only care about tenant
    /// membership) unchanged, while the step-up tests below vary the extras.
    fn user_of(tenants: &[Uuid]) -> Principal {
        user_full(tenants, Aal::Aal2, &[SCOPE_FINANCIAL_WRITE], Some(1_000))
    }

    fn user_full(
        tenants: &[Uuid],
        assurance: Aal,
        scopes: &[&str],
        step_up_at: Option<u64>,
    ) -> Principal {
        Principal::User(Box::new(SupabaseIdentity {
            subject: "user-abc".into(),
            email: Some("operator@example.com".into()),
            role: Some("authenticated".into()),
            tenant_ids: tenants.to_vec(),
            assurance,
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            step_up_at,
        }))
    }

    #[test]
    fn a_user_may_reach_their_own_tenant() {
        assert_eq!(
            authorize_tenant(&user_of(&[uuid_a()]), TenantScope::Tenant(uuid_a()), true),
            Ok(())
        );
    }

    #[test]
    fn a_user_may_not_reach_another_tenant() {
        // This is the IDOR. Before this change, a caller holding a valid
        // credential could swap the path segment and operate on any tenant.
        assert_eq!(
            authorize_tenant(&user_of(&[uuid_a()]), TenantScope::Tenant(uuid_b()), true),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn a_user_with_no_tenant_claims_reaches_nothing() {
        assert_eq!(
            authorize_tenant(&user_of(&[]), TenantScope::Tenant(uuid_a()), true),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn a_multi_tenant_user_reaches_exactly_their_tenants() {
        let principal = user_of(&[uuid_a(), uuid_b()]);
        assert_eq!(
            authorize_tenant(&principal, TenantScope::Tenant(uuid_a()), true),
            Ok(())
        );
        assert_eq!(
            authorize_tenant(&principal, TenantScope::Tenant(uuid_b()), true),
            Ok(())
        );
        let other = Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap();
        assert_eq!(
            authorize_tenant(&principal, TenantScope::Tenant(other), true),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn the_service_bearer_is_refused_on_tenant_routes_once_user_jwts_are_required() {
        assert_eq!(
            authorize_tenant(&Principal::Service, TenantScope::Tenant(uuid_a()), true),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn the_service_bearer_still_works_on_tenant_routes_during_the_migration_window() {
        // BILLING_TENANT_ROUTES_REQUIRE_USER_JWT=false — the documented, WARN-ing
        // escape hatch that keeps existing callers working while they migrate.
        assert_eq!(
            authorize_tenant(&Principal::Service, TenantScope::Tenant(uuid_a()), false),
            Ok(())
        );
    }

    #[test]
    fn the_service_bearer_always_works_on_unscoped_routes() {
        // Tenant *creation* and the OAuth/Plaid handshakes are provisioning
        // calls with no tenant in the path; requiring a user token there would
        // break them for no security gain.
        for require in [true, false] {
            assert_eq!(
                authorize_tenant(&Principal::Service, TenantScope::None, require),
                Ok(())
            );
        }
    }

    #[test]
    fn an_unparseable_tenant_is_refused_for_every_principal() {
        for require in [true, false] {
            assert_eq!(
                authorize_tenant(&Principal::Service, TenantScope::Unparseable, require),
                Err(StatusCode::FORBIDDEN)
            );
            assert_eq!(
                authorize_tenant(&user_of(&[uuid_a()]), TenantScope::Unparseable, require),
                Err(StatusCode::FORBIDDEN)
            );
        }
    }

    // --- DEN-1190: fresh-AAL2 + financial-scope step-up for human mutations ---

    const NOW: u64 = 1_000_000;

    fn step_up_policy() -> AuthzPolicy {
        AuthzPolicy {
            require_user_jwt: true,
            require_step_up_for_mutations: true,
        }
    }

    fn authz(principal: &Principal, scope: TenantScope, action: Action) -> Result<(), StatusCode> {
        authorize_request(principal, scope, action, step_up_policy(), NOW)
    }

    #[test]
    fn aal2_scoped_fresh_user_may_mutate_own_tenant() {
        let user = user_full(&[uuid_a()], Aal::Aal2, &[SCOPE_FINANCIAL_WRITE], Some(NOW));
        assert_eq!(
            authz(&user, TenantScope::Tenant(uuid_a()), Action::Mutate),
            Ok(())
        );
    }

    #[test]
    fn reads_never_require_step_up() {
        // An AAL1 user with no scope may still *read* their own tenant.
        let user = user_full(&[uuid_a()], Aal::Aal1, &[], None);
        assert_eq!(
            authz(&user, TenantScope::Tenant(uuid_a()), Action::Read),
            Ok(())
        );
    }

    #[test]
    fn aal1_user_may_not_mutate() {
        let user = user_full(&[uuid_a()], Aal::Aal1, &[SCOPE_FINANCIAL_WRITE], Some(NOW));
        assert_eq!(
            authz(&user, TenantScope::Tenant(uuid_a()), Action::Mutate),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn stale_or_absent_step_up_is_refused_but_the_boundary_is_fresh() {
        let stale = user_full(
            &[uuid_a()],
            Aal::Aal2,
            &[SCOPE_FINANCIAL_WRITE],
            Some(NOW - (MAX_STEP_UP_AGE_SECS + 1)),
        );
        assert_eq!(
            authz(&stale, TenantScope::Tenant(uuid_a()), Action::Mutate),
            Err(StatusCode::FORBIDDEN)
        );
        let absent = user_full(&[uuid_a()], Aal::Aal2, &[SCOPE_FINANCIAL_WRITE], None);
        assert_eq!(
            authz(&absent, TenantScope::Tenant(uuid_a()), Action::Mutate),
            Err(StatusCode::FORBIDDEN)
        );
        // Exactly at the max age still counts as fresh.
        let boundary = user_full(
            &[uuid_a()],
            Aal::Aal2,
            &[SCOPE_FINANCIAL_WRITE],
            Some(NOW - MAX_STEP_UP_AGE_SECS),
        );
        assert_eq!(
            authz(&boundary, TenantScope::Tenant(uuid_a()), Action::Mutate),
            Ok(())
        );
    }

    #[test]
    fn missing_financial_scope_is_refused() {
        let user = user_full(&[uuid_a()], Aal::Aal2, &["billing:read"], Some(NOW));
        assert_eq!(
            authz(&user, TenantScope::Tenant(uuid_a()), Action::Mutate),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn wrong_tenant_is_refused_before_any_step_up_check() {
        // Tenant entitlement is layered first; a fully-privileged user still
        // cannot touch a tenant that is not theirs.
        let user = user_full(&[uuid_a()], Aal::Aal2, &[SCOPE_FINANCIAL_WRITE], Some(NOW));
        assert_eq!(
            authz(&user, TenantScope::Tenant(uuid_b()), Action::Mutate),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn service_principal_cannot_mutate_a_tenant_route() {
        // Service/user confusion: the shared bearer names no tenant and carries
        // no assurance; step-up mode does not loosen the migration rule.
        assert_eq!(
            authz(
                &Principal::Service,
                TenantScope::Tenant(uuid_a()),
                Action::Mutate
            ),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn mutation_hardening_denies_shared_service_credential_during_user_jwt_migration() {
        let policy = AuthzPolicy {
            require_user_jwt: false,
            require_step_up_for_mutations: true,
        };
        assert_eq!(
            authorize_request(
                &Principal::Service,
                TenantScope::Tenant(uuid_a()),
                Action::Mutate,
                policy,
                NOW,
            ),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            authorize_request(
                &Principal::Service,
                TenantScope::Tenant(uuid_a()),
                Action::Read,
                policy,
                NOW,
            ),
            Ok(())
        );
    }

    #[test]
    fn unscoped_provisioning_mutation_is_not_financial_gated() {
        // POST /v1/tenants creates a tenant; there is no tenant to be scoped to.
        let user = user_full(&[], Aal::Aal1, &[], None);
        assert_eq!(authz(&user, TenantScope::None, Action::Mutate), Ok(()));
    }

    #[test]
    fn step_up_disabled_preserves_pre_migration_behaviour() {
        // With the flag off (default), an AAL1 user mutating their own tenant is
        // allowed exactly as before — turning Supabase on can't instantly break
        // callers who have not stepped up yet.
        let policy = AuthzPolicy {
            require_user_jwt: true,
            require_step_up_for_mutations: false,
        };
        let user = user_full(&[uuid_a()], Aal::Aal1, &[], None);
        assert_eq!(
            authorize_request(
                &user,
                TenantScope::Tenant(uuid_a()),
                Action::Mutate,
                policy,
                NOW
            ),
            Ok(())
        );
    }

    #[test]
    fn action_derivation_is_fail_closed() {
        for m in [Method::GET, Method::HEAD, Method::OPTIONS, Method::TRACE] {
            assert_eq!(Action::of(&m), Action::Read, "{m} should read");
        }
        for m in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert_eq!(Action::of(&m), Action::Mutate, "{m} should mutate");
        }
    }

    // --- End-to-end through the middleware -----------------------------------

    fn tenant_router(auth: Arc<ApiAuth>) -> Router {
        Router::new()
            .route("/v1/tenants", post(|| async { "ok-create" }))
            .route(
                "/v1/tenants/{tenant_id}/connections",
                get(|| async { "ok-conn" }),
            )
            .layer(axum::middleware::from_fn_with_state(
                auth.clone(),
                require_api_auth,
            ))
            .with_state(auth)
    }

    #[tokio::test]
    async fn service_bearer_is_blocked_from_tenant_routes_end_to_end() {
        let auth = Arc::new(ApiAuth {
            bearer: Some("service-token".into()),
            supabase: None,
            require_user_jwt: true,
            require_step_up: false,
        });
        let app = tenant_router(auth);

        // Unscoped provisioning route: still fine.
        assert_eq!(
            status_of(
                app.clone(),
                "POST",
                "/v1/tenants",
                Some("Bearer service-token")
            )
            .await,
            StatusCode::OK
        );
        // Tenant-scoped route: refused, because the shared token names no tenant.
        assert_eq!(
            status_of(
                app,
                "GET",
                &format!("/v1/tenants/{TENANT_A}/connections"),
                Some("Bearer service-token")
            )
            .await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn an_unverifiable_token_is_still_unauthorized_not_forbidden() {
        // A bad credential must read as 401 (who are you?), never 403 (I know
        // who you are and you may not) — the two say very different things to
        // a caller and to an audit log.
        let auth = Arc::new(ApiAuth {
            bearer: Some("service-token".into()),
            supabase: None,
            require_user_jwt: true,
            require_step_up: false,
        });
        let app = tenant_router(auth);
        assert_eq!(
            status_of(
                app,
                "GET",
                &format!("/v1/tenants/{TENANT_A}/connections"),
                Some("Bearer wrong-token")
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn the_debug_surface_never_prints_the_service_bearer() {
        let auth = ApiAuth {
            bearer: Some("super-secret-bearer".into()),
            supabase: None,
            require_user_jwt: true,
            require_step_up: false,
        };
        let rendered = format!("{auth:?}");
        assert!(!rendered.contains("super-secret-bearer"));
        assert!(rendered.contains("<redacted>"));
    }
}
