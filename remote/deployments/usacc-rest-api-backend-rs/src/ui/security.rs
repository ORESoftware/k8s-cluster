//! Security middleware for the `/app` console nest.
//!
//! Three cheap, fail-safe layers, mirroring the JSON API's posture:
//!   1. **`require_ui_auth`** — when `USACC_APP_UI_BEARER` is set, every
//!      console request must present a matching `Authorization: Bearer`
//!      (constant-time compared). Unset → no-op for friction-free dev.
//!   2. **`csrf_guard`** — unsafe methods must carry `HX-Request: true`
//!      (a custom header cross-origin clients can't set without a CORS
//!      preflight we never grant) or report a same-site `Sec-Fetch-Site`;
//!      and any `Origin` present must match `Host` or the allow-list.
//!   3. **`security_headers`** — CSP (no `'unsafe-eval'`/inline scripts),
//!      anti-clickjacking, nosniff, referrer + permissions policy, and a
//!      `noindex` robots tag on every console response.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

/// Per-request security knobs, built once from [`Config`](crate::config::Config).
#[derive(Clone, Debug)]
pub struct UiSecurity {
    pub bearer: Option<String>,
    pub allowed_origins: Vec<String>,
}

impl UiSecurity {
    pub fn from_state(state: &AppState) -> Arc<Self> {
        Arc::new(Self {
            bearer: state.config.app_ui_bearer.clone(),
            allowed_origins: state.config.app_ui_allowed_origins.clone(),
        })
    }
}

/// Constant-time byte-slice comparison.
fn const_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Reject console requests that don't authenticate when a bearer is set.
pub async fn require_ui_auth(
    State(sec): State<Arc<UiSecurity>>,
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
    let mut resp = (StatusCode::UNAUTHORIZED, "console authentication required\n").into_response();
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"usacc-console\""),
    );
    resp
}

/// CSRF defense-in-depth for unsafe methods on the console nest.
pub async fn csrf_guard(
    State(sec): State<Arc<UiSecurity>>,
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
    let same_site = matches!(sec_fetch_site, "same-origin" | "same-site" | "none");

    if !hx_request && !same_site {
        tracing::warn!(
            method = %req.method(),
            uri = %req.uri(),
            "console csrf: rejected request with neither HX-Request nor same-site Sec-Fetch-Site"
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
                "console csrf: rejected cross-origin write"
            );
            return forbidden("csrf: origin not allowed");
        }
    }

    next.run(req).await
}

/// Add a strict baseline of security headers to every console response.
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
         magnetometer=(), microphone=(), payment=(), usb=(), interest-cohort=()",
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
    static_insert(
        h,
        header::HeaderName::from_static("x-robots-tag"),
        "noindex, nofollow, noarchive",
    );

    // Console pages can carry participant PII (names, email hashes, case
    // detail), so keep them out of shared/browser caches. The hash-pinned
    // static htmx asset sets its own immutable Cache-Control in the
    // handler; don't clobber it.
    if !h.contains_key(header::CACHE_CONTROL) {
        h.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        );
    }

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

/// Same host (ignoring scheme) as the request `Host` is same-origin;
/// explicit allow-list entries match the full origin string.
fn origin_is_allowed(origin: &str, host: &str, allow: &[String]) -> bool {
    if allow.iter().any(|o| o == origin) {
        return true;
    }
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
        assert!(const_time_eq(b"", b""));
    }

    #[test]
    fn origin_check_accepts_same_host_rejects_others() {
        let allow: Vec<String> = vec![];
        assert!(origin_is_allowed(
            "http://localhost:8121",
            "localhost:8121",
            &allow
        ));
        assert!(!origin_is_allowed(
            "https://evil.example",
            "usacc.example.com",
            &allow
        ));
        assert!(!origin_is_allowed(
            "https://api.usacc.example.com",
            "usacc.example.com",
            &allow
        ));
    }

    #[test]
    fn origin_allow_list_entries_match() {
        let allow = vec!["https://ops.example.com".to_string()];
        assert!(origin_is_allowed(
            "https://ops.example.com",
            "usacc.example.com",
            &allow
        ));
    }

    #[test]
    fn unsafe_methods_classified() {
        assert!(is_unsafe_method(&Method::POST));
        assert!(is_unsafe_method(&Method::PATCH));
        assert!(!is_unsafe_method(&Method::GET));
    }
}
