//! API bearer auth + outbound URL safety helpers.
//!
//! ### Auth model (read this first)
//!
//! The billing API has historically trusted the path `tenant_id` and
//! relied on an upstream gateway (`dd-remote-auth`) for proof of
//! ownership. This module adds an **in-process** floor so the service
//! remains safe even when the gateway is bypassed (port-forward,
//! cluster-internal access, misconfigured ingress, …).
//!
//! When [`Config::api_auth_bearer`] is set:
//!   * Every `/v1/...` request — including the OAuth `/start`,
//!     `/callback`, and Plaid `link-token` / `exchange` flows —
//!     must present `Authorization: Bearer <token>`.
//!   * `Authorization: Bearer <token>` is compared in constant time.
//!   * Webhooks (`/v1/webhooks/*`) and the public verification
//!     endpoint (`/v1/verify/...`) are **exempt** — they have their
//!     own auth model (provider signatures and "the data is public",
//!     respectively).
//!   * Health endpoints and `/admin` are also exempt; admin has its
//!     own bearer in [`super::super::admin::security`].
//!
//! When [`Config::api_auth_bearer`] is **unset**, this middleware is a
//! no-op for dev friction. We log a single WARN at boot so operators
//! notice. Production manifests inject the bearer via SealedSecrets.
//!
//! Per-tenant scoping (giving each tenant its own short-lived token) is
//! intentionally **out of scope** here: the in-process floor only
//! enforces caller authenticity. Tenant-ownership checks belong to the
//! gateway, and to the per-handler `state.tenants.by_id(...)` calls
//! that already prove the tenant exists.

use std::net::IpAddr;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::config::Config;

/// Per-request auth knobs. Built once at boot from
/// [`Config::api_auth_bearer`] and shared via `Arc` so the middleware
/// closure doesn't carry the full `AppState`.
#[derive(Clone, Debug)]
pub struct ApiAuth {
    pub bearer: Option<String>,
}

impl ApiAuth {
    pub fn from_config(cfg: &Config) -> Arc<Self> {
        Arc::new(Self {
            bearer: cfg.api_auth_bearer.clone(),
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

/// Axum middleware: enforce the bearer when one is configured.
pub async fn require_api_auth(
    State(auth): State<Arc<ApiAuth>>,
    req: Request,
    next: Next,
) -> Response {
    let Some(expected) = auth.bearer.as_deref() else {
        return next.run(req).await;
    };
    if is_exempt_path(req.uri()) {
        return next.run(req).await;
    }
    let provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or("");
    if !provided.is_empty() && const_time_eq(provided.as_bytes(), expected.as_bytes()) {
        return next.run(req).await;
    }
    let mut resp = (
        StatusCode::UNAUTHORIZED,
        "api authentication required\n",
    )
        .into_response();
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"billing-api\""),
    );
    resp
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
            .route(
                "/v1/oauth/stripe/start",
                get(|| async { "ok-oauth-start" }),
            )
            .route(
                "/v1/oauth/stripe/callback",
                get(|| async { "ok-oauth-cb" }),
            )
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
            status_of(
                app,
                "POST",
                "/v1/tenants",
                Some("Bearer not-the-token")
            )
            .await,
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
}
