//! Security middleware for the admin nest.
//!
//! Three layered defenses, designed to be cheap and to fail safely:
//!
//!   1. **`require_admin_auth`** — if `BILLING_ADMIN_AUTH_BEARER` is set,
//!      every admin request must present `Authorization: Bearer <token>`
//!      (constant-time compared). Without the env var, the middleware is
//!      a no-op so local dev is friction-free.
//!   2. **`csrf_guard`** — every unsafe method (POST/PUT/PATCH/DELETE)
//!      must carry `HX-Request: true` (a custom header → cross-origin
//!      requests cannot set it without a CORS preflight that we don't
//!      grant) AND, when an `Origin` header is present, that origin must
//!      either match the request `Host` or be on the allow-list. The
//!      two checks together rule out classic form-CSRF and a wide class
//!      of XHR-based attacks even if a future change accidentally widens
//!      CORS.
//!   3. **`security_headers`** — adds CSP, anti-clickjacking,
//!      anti-MIME-sniff, referrer-policy, and permissions-policy headers
//!      to every admin response. The CSP intentionally forbids
//!      `'unsafe-eval'` and `'unsafe-inline'` on scripts; HTMX's vendored
//!      copy is self-hosted and we use no inline event handlers.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

/// Per-request security knobs. Built once from [`Config`](crate::config::Config)
/// in [`AdminSecurity::from_state`] and shared via `Arc` so middleware
/// closures don't need to hold the whole `AppState`. Tests construct it
/// directly without touching Postgres.
#[derive(Clone, Debug)]
pub struct AdminSecurity {
    pub bearer: Option<String>,
    pub allowed_origins: Vec<String>,
}

impl AdminSecurity {
    pub fn from_state(state: &AppState) -> Arc<Self> {
        Arc::new(Self {
            bearer: state.cfg.admin_auth_bearer.clone(),
            allowed_origins: state.cfg.admin_allowed_origins.clone(),
        })
    }
}

/// Constant-time byte-slice comparison. Length mismatch returns early
/// (which technically leaks the *length* of the expected secret — for a
/// fixed-shape bearer token that's a non-issue).
fn const_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Reject any admin request that doesn't authenticate when a bearer is
/// configured. Disabled (always-pass) when the bearer is unset.
pub async fn require_admin_auth(
    State(sec): State<Arc<AdminSecurity>>,
    req: Request,
    next: Next,
) -> Response {
    let Some(expected) = sec.bearer.as_deref() else {
        return next.run(req).await;
    };
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
        "admin authentication required\n",
    )
        .into_response();
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"billing-admin\""),
    );
    resp
}

/// CSRF defense-in-depth for unsafe methods on the admin nest.
///
/// Requires that **every** mutating request either:
///   - presents `HX-Request: true` (custom header → cross-origin clients
///     cannot send it without a CORS preflight we don't grant), OR
///   - the `Sec-Fetch-Site` header reports `same-origin`/`same-site`.
///
/// Additionally, when an `Origin` header is present, that origin must
/// match the request `Host` or appear in the allow-list. GET/HEAD are
/// always passed through (browsers send Origin on cross-origin POST, but
/// not on top-level navigations, so we only enforce on writes).
pub async fn csrf_guard(
    State(sec): State<Arc<AdminSecurity>>,
    req: Request,
    next: Next,
) -> Response {
    if !is_unsafe_method(req.method()) {
        return next.run(req).await;
    }

    let headers = req.headers();
    let hx_request = headers
        .get("HX-Request")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let sec_fetch_site = headers
        .get("Sec-Fetch-Site")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let sec_fetch_same_site =
        matches!(sec_fetch_site, "same-origin" | "same-site" | "none");

    if !hx_request && !sec_fetch_same_site {
        tracing::warn!(
            method = %req.method(),
            uri = %req.uri(),
            "admin csrf: rejected request with neither HX-Request nor same-site Sec-Fetch-Site"
        );
        return forbidden("csrf: missing HX-Request or same-site Sec-Fetch-Site");
    }

    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !origin_is_allowed(origin, host, &sec.allowed_origins) {
            tracing::warn!(
                method = %req.method(),
                uri = %req.uri(),
                origin,
                host,
                "admin csrf: rejected cross-origin write"
            );
            return forbidden("csrf: origin not allowed");
        }
    }

    next.run(req).await
}

/// Add a strict baseline of security headers to every admin response.
///
/// CSP rationale:
///   - `script-src 'self'` — htmx is vendored under `/admin/static/`; no
///     CDN, no inline scripts. We deliberately avoid `'unsafe-inline'`
///     and `'unsafe-eval'`, so no `hx-on:*` attributes (which call
///     `new Function`) may be used in templates.
///   - `style-src 'self' 'unsafe-inline'` — the admin layout uses a
///     single inline `<style>` block. `'unsafe-inline'` for styles is a
///     standard trade-off and does not enable code execution.
///   - `frame-ancestors 'none'` — clickjacking defense; paired with
///     `X-Frame-Options: DENY` for browsers that ignore CSP.
///   - `form-action 'self'`, `base-uri 'self'` — narrow the attack
///     surface of a successful injection.
pub async fn security_headers(req: Request<Body>, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();

    static_insert(h, header::CONTENT_SECURITY_POLICY, CSP);
    static_insert(h, header::X_FRAME_OPTIONS, "DENY");
    static_insert(h, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    static_insert(h, header::REFERRER_POLICY, "same-origin");
    static_insert(
        h,
        header::HeaderName::from_static("permissions-policy"),
        "accelerometer=(), camera=(), geolocation=(), gyroscope=(), \
         magnetometer=(), microphone=(), payment=(), usb=(), \
         interest-cohort=()",
    );
    static_insert(
        h,
        header::HeaderName::from_static("cross-origin-opener-policy"),
        "same-origin",
    );
    static_insert(
        h,
        header::HeaderName::from_static("cross-origin-resource-policy"),
        "same-origin",
    );
    // Don't allow search engines to index any admin response that
    // somehow escapes its auth boundary.
    static_insert(
        h,
        header::HeaderName::from_static("x-robots-tag"),
        "noindex, nofollow, noarchive",
    );

    resp
}

const CSP: &str = "default-src 'self'; \
script-src 'self'; \
style-src 'self' 'unsafe-inline'; \
img-src 'self' data:; \
font-src 'self'; \
connect-src 'self'; \
frame-ancestors 'none'; \
form-action 'self'; \
base-uri 'self'; \
object-src 'none'";

fn static_insert(h: &mut axum::http::HeaderMap, name: header::HeaderName, value: &'static str) {
    h.insert(name, HeaderValue::from_static(value));
}

fn is_unsafe_method(m: &Method) -> bool {
    matches!(
        m,
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
    )
}

/// Compare a browser-supplied `Origin` value (`scheme://host[:port]`)
/// against the request `Host` header. Same host (ignoring scheme) is
/// considered same-origin for the admin's purposes. Explicit
/// allow-list entries match the entire origin string.
fn origin_is_allowed(origin: &str, host: &str, allow: &[String]) -> bool {
    if allow.iter().any(|o| o == origin) {
        return true;
    }
    // Strip scheme to get just the host[:port]; do the prefix match
    // case-insensitively (browsers normally lowercase the scheme, but a
    // non-browser client could send `HTTPS://...`).
    let lowered = origin.to_ascii_lowercase();
    let origin_host = lowered
        .strip_prefix("https://")
        .or_else(|| lowered.strip_prefix("http://"))
        .unwrap_or(&lowered);
    origin_host.eq_ignore_ascii_case(host)
}

fn forbidden(msg: &'static str) -> Response {
    (StatusCode::FORBIDDEN, msg).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn const_time_eq_basic() {
        assert!(const_time_eq(b"abc", b"abc"));
        assert!(!const_time_eq(b"abc", b"abd"));
        assert!(!const_time_eq(b"abc", b"abcd"));
        assert!(!const_time_eq(b"abc", b""));
        assert!(const_time_eq(b"", b""));
    }

    #[test]
    fn origin_check_accepts_same_host() {
        let allow: Vec<String> = vec![];
        assert!(origin_is_allowed("http://localhost:18087", "localhost:18087", &allow));
        assert!(origin_is_allowed("https://billing.example.com", "billing.example.com", &allow));
        assert!(origin_is_allowed("HTTPS://Billing.Example.com", "billing.example.com", &allow));
    }

    #[test]
    fn origin_check_rejects_different_host_without_allow_list() {
        let allow: Vec<String> = vec![];
        assert!(!origin_is_allowed("https://evil.example", "billing.example.com", &allow));
        assert!(!origin_is_allowed("https://billing.example.com.evil", "billing.example.com", &allow));
        // Subdomain confusion guard:
        assert!(!origin_is_allowed("https://api.billing.example.com", "billing.example.com", &allow));
    }

    #[test]
    fn origin_check_allows_explicit_allow_list_entries() {
        let allow = vec!["https://ops.example.com".to_string()];
        assert!(origin_is_allowed("https://ops.example.com", "billing.example.com", &allow));
        assert!(!origin_is_allowed("https://other.example.com", "billing.example.com", &allow));
    }

    #[test]
    fn unsafe_methods_classified_correctly() {
        assert!(is_unsafe_method(&Method::POST));
        assert!(is_unsafe_method(&Method::PUT));
        assert!(is_unsafe_method(&Method::PATCH));
        assert!(is_unsafe_method(&Method::DELETE));
        assert!(!is_unsafe_method(&Method::GET));
        assert!(!is_unsafe_method(&Method::HEAD));
        assert!(!is_unsafe_method(&Method::OPTIONS));
    }
}
