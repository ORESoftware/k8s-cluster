//! Path-scoped response hardening for documentation and operator endpoints.
//!
//! A router-level middleware covers successful handlers, authorization
//! failures, `HEAD`, and method-not-allowed responses without changing the
//! established speech or webhook response semantics.

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

const DOCS_PATHS: &[&str] = &[
    "/openapi.json",
    "/api/docs.json",
    "/api/docs",
    "/docs/api",
    "/internal/openapi.json",
    "/internal/docs/api",
];
const HTML_DOCS_PATHS: &[&str] = &["/api/docs", "/docs/api", "/internal/docs/api"];

fn is_operator_path(path: &str) -> bool {
    path.starts_with("/internal/")
        || path.starts_with("/v1/history/")
        || path == "/vapi/call"
        || path.starts_with("/vapi/call/")
}

fn append_vary_authorization(headers: &mut HeaderMap) {
    let already_varies = headers
        .get_all(header::VARY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|value| value.trim().eq_ignore_ascii_case("authorization"));
    if already_varies {
        return;
    }

    match headers
        .get(header::VARY)
        .and_then(|value| value.to_str().ok())
    {
        Some(existing) if !existing.trim().is_empty() => {
            if let Ok(value) = HeaderValue::from_str(&format!("{existing}, Authorization")) {
                headers.insert(header::VARY, value);
            }
        }
        _ => {
            headers.insert(header::VARY, HeaderValue::from_static("Authorization"));
        }
    }
}

fn apply_common(headers: &mut HeaderMap) {
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
}

pub async fn apply(request: Request<Body>, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let is_docs = DOCS_PATHS.contains(&path.as_str());
    let is_operator = is_operator_path(&path);
    let is_html = HTML_DOCS_PATHS.contains(&path.as_str());

    let mut response = next.run(request).await;
    if is_docs || is_operator {
        let status = response.status();
        let headers = response.headers_mut();
        apply_common(headers);
        if is_operator {
            append_vary_authorization(headers);
            if status == StatusCode::UNAUTHORIZED {
                headers.insert(
                    header::WWW_AUTHENTICATE,
                    HeaderValue::from_static("Bearer realm=\"t2v-operator\""),
                );
            }
        }
        if is_html {
            headers.insert(
                HeaderName::from_static("content-security-policy"),
                HeaderValue::from_static(
                    "frame-ancestors 'none'; base-uri 'none'; object-src 'none'",
                ),
            );
            headers.insert(
                HeaderName::from_static("x-frame-options"),
                HeaderValue::from_static("DENY"),
            );
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_and_docs_classification_is_fail_closed_and_exact() {
        assert!(DOCS_PATHS.contains(&"/openapi.json"));
        assert!(HTML_DOCS_PATHS.contains(&"/internal/docs/api"));
        assert!(is_operator_path("/v1/history/translations"));
        assert!(is_operator_path("/vapi/call"));
        assert!(is_operator_path("/vapi/call/abc"));
        assert!(!is_operator_path("/vapi/callback"));
        assert!(!is_operator_path("/vapi/webhook"));
        assert!(!DOCS_PATHS.contains(&"/internal/openapi.json/extra"));
    }

    #[test]
    fn vary_preserves_existing_values_and_is_idempotent() {
        let mut headers = HeaderMap::new();
        headers.insert(header::VARY, HeaderValue::from_static("Origin"));
        append_vary_authorization(&mut headers);
        assert_eq!(headers[header::VARY], "Origin, Authorization");
        append_vary_authorization(&mut headers);
        assert_eq!(headers[header::VARY], "Origin, Authorization");
    }
}
