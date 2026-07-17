//! Optional HTMX admin UI mounted at `/admin`.
//!
//! Security posture (see `security.rs` for the layered defenses):
//!   - **Read-mostly.** Writes are limited to small, idempotent actions
//!     that also exist as JSON endpoints (run-now, enable/disable, sync).
//!   - **No regressions.** The JSON API in [`crate::api`] is untouched.
//!     The admin surface is a parallel router gated by
//!     `BILLING_ADMIN_UI_ENABLED` (default on for dev).
//!   - **Self-hosted assets.** HTMX is bundled into the binary and served
//!     from `/admin/static/htmx-<hash>.js`; no CDN at runtime.
//!   - **Optional bearer auth** via `BILLING_ADMIN_AUTH_BEARER`. When
//!     unset, the UI is unauthenticated and intended for trusted
//!     networks only — front it with `dd-remote-auth` (per `AGENTS.md`)
//!     before any public exposure.
//!   - **CSRF defense in depth.** All unsafe methods must carry
//!     `HX-Request: true` (HTMX always sends it; cross-origin requests
//!     cannot set it without a CORS preflight we do not grant) AND
//!     `Origin` must match `Host` if present.
//!   - **Strict CSP** with no `'unsafe-eval'` or inline scripts. We
//!     deliberately avoid `hx-on:*` attributes in templates because they
//!     would require `'unsafe-eval'`.
//!
//! The route surface mirrors the JSON API:
//!   - `GET  /admin`                                         dashboard
//!   - `GET  /admin/status`                                  status pill fragment
//!   - `GET  /admin/tenants`                                 list + create form
//!   - `POST /admin/tenants`                                 create (HTMX form, returns row)
//!   - `GET  /admin/tenants/{id}`                            tenant detail (tabs land here)
//!   - `GET  /admin/tenants/{id}/connections`                connections table fragment
//!   - `GET  /admin/tenants/{id}/jobs`                       scheduled-jobs table fragment
//!   - `GET  /admin/tenants/{id}/locks`                      leases table fragment
//!   - `GET  /admin/tenants/{id}/notifications`              rules + dispatches fragment
//!   - `POST /admin/tenants/{tid}/jobs/{id}/run-now`         HTMX action, returns row
//!   - `POST /admin/tenants/{tid}/jobs/{id}/toggle`          HTMX action, returns row
//!   - `POST /admin/tenants/{tid}/connections/{id}/sync`     HTMX action, returns row
//!   - `GET  /admin/static/htmx-<hash>.js`                   vendored htmx asset
//!   - `GET  /admin/robots.txt`                              `noindex, nofollow`

mod assets;
mod connections;
mod dashboard;
mod errors;
mod jobs;
mod layout;
mod locks;
mod notifications;
mod security;
mod tenants;
mod time;
mod validation;

use axum::Router;
use axum::http::{StatusCode, header};
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{get, post};

use crate::state::AppState;

/// Build the admin router. Mount under `/admin` (handled by the caller).
///
/// Must be called only after [`assets::verify_integrity`] has run (the
/// startup hook in `main.rs` does that).
pub fn build_router(state: AppState) -> Router<AppState> {
    let sec = security::AdminSecurity::from_state(&state);
    build_router_with_security(sec)
}

/// Same as [`build_router`] but takes the security state directly. Used by
/// the integration tests so they don't need a Postgres pool to verify the
/// middleware wiring (auth, CSRF, headers, static asset routing).
fn build_router_with_security(sec: std::sync::Arc<security::AdminSecurity>) -> Router<AppState> {
    Router::new()
        .route("/", get(dashboard::page))
        .route("/status", get(dashboard::status_fragment))
        .route("/tenants", get(tenants::list_page).post(tenants::create))
        .route("/tenants/{id}", get(tenants::detail_page))
        .route(
            "/tenants/{id}/connections",
            get(connections::table_fragment),
        )
        .route("/tenants/{id}/jobs", get(jobs::table_fragment))
        .route("/tenants/{id}/locks", get(locks::table_fragment))
        .route(
            "/tenants/{id}/notifications",
            get(notifications::page_fragment),
        )
        .route(
            "/tenants/{tid}/jobs/{job_id}/run-now",
            post(jobs::run_now),
        )
        .route(
            "/tenants/{tid}/jobs/{job_id}/toggle",
            post(jobs::toggle),
        )
        .route(
            "/tenants/{tid}/connections/{conn_id}/sync",
            post(connections::sync_now),
        )
        .route("/static/{file}", get(assets::serve))
        .route("/robots.txt", get(robots_txt))
        // Order of middleware execution (outermost runs first on the
        // request path): auth -> csrf -> headers. So security_headers is
        // attached last (innermost) but observes every response.
        .layer(middleware::from_fn(security::security_headers))
        .layer(middleware::from_fn_with_state(sec.clone(), security::csrf_guard))
        .layer(middleware::from_fn_with_state(sec, security::require_admin_auth))
}

/// Discourage indexing if the admin somehow leaks through a misconfigured
/// gateway. The `X-Robots-Tag` header in `security_headers` provides the
/// same protection on every response; this is a polite second copy at
/// the well-known path.
async fn robots_txt() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "User-agent: *\nDisallow: /\n",
    )
}

pub use assets::verify_integrity as verify_asset_integrity;

#[cfg(test)]
mod tests {
    //! Wire-level tests for the security middleware stack. These build
    //! the same router shape the binary serves (with a tiny dummy `/`
    //! handler that bypasses the dashboard's DB queries) and exercise it
    //! via `tower::ServiceExt::oneshot` — no Postgres required.
    //!
    //! What we cover:
    //!   - GET /admin/static/htmx-<hash>.js succeeds; other names 404.
    //!   - GET /admin/robots.txt returns `Disallow: /`.
    //!   - Every response carries CSP, X-Frame-Options, and friends.
    //!   - POST without `HX-Request: true` is rejected (CSRF).
    //!   - POST with `HX-Request: true` is allowed.
    //!   - POST with a foreign `Origin` is rejected even with HX-Request.
    //!   - Same-origin POST (Origin matches Host) is allowed.
    //!   - Bearer-auth: GET is rejected when token missing/wrong, allowed
    //!     when correct. Constant-time compare (regression: any-prefix bug
    //!     would let a partial bearer through).

    use std::sync::Arc;

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::{get, post};
    use tower::ServiceExt;

    use super::security::AdminSecurity;
    use super::{assets, robots_txt, security};

    /// Build a router that mirrors the production layer stack but plugs
    /// in tiny synthetic handlers so we don't need any application state.
    fn router(sec: AdminSecurity) -> Router {
        Router::new()
            .route("/", get(|| async { "ok" }))
            .route("/write", post(|| async { "wrote" }))
            .route("/static/{file}", get(assets::serve))
            .route("/robots.txt", get(robots_txt))
            .layer(axum::middleware::from_fn(security::security_headers))
            .layer(axum::middleware::from_fn_with_state(
                Arc::new(sec.clone()),
                security::csrf_guard,
            ))
            .layer(axum::middleware::from_fn_with_state(
                Arc::new(sec),
                security::require_admin_auth,
            ))
    }

    fn open() -> AdminSecurity {
        AdminSecurity {
            bearer: None,
            allowed_origins: vec![],
        }
    }

    fn req(method: &str, uri: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("Host", "billing.example.com")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn get_root_carries_security_headers() {
        let resp = router(open()).oneshot(req("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let h = resp.headers();
        assert!(h.contains_key("content-security-policy"));
        let csp = h.get("content-security-policy").unwrap().to_str().unwrap();
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("script-src 'self'"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(!csp.contains("'unsafe-eval'"));
        assert_eq!(h.get("x-frame-options").unwrap(), "DENY");
        assert_eq!(h.get("x-content-type-options").unwrap(), "nosniff");
        assert_eq!(h.get("referrer-policy").unwrap(), "same-origin");
        assert!(h.contains_key("permissions-policy"));
        assert_eq!(h.get("cross-origin-opener-policy").unwrap(), "same-origin");
        assert_eq!(h.get("cross-origin-resource-policy").unwrap(), "same-origin");
        assert_eq!(h.get("x-robots-tag").unwrap(), "noindex, nofollow, noarchive");
    }

    #[tokio::test]
    async fn static_asset_serves_vendored_htmx() {
        // assets::htmx_asset_path() returns the URL under the production
        // `/admin` nest. The test router isn't nested, so strip the
        // `/admin` prefix to hit the local `/static/{file}` route.
        let path = assets::htmx_asset_path()
            .strip_prefix("/admin")
            .unwrap()
            .to_string();
        let r = router(open())
            .oneshot(req("GET", &path))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers().get("content-type").unwrap(),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(
            r.headers().get("cache-control").unwrap(),
            "public, max-age=31536000, immutable"
        );
        let body = to_bytes(r.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.as_ref(), assets::HTMX_BYTES);
    }

    #[tokio::test]
    async fn static_asset_rejects_unknown_filenames() {
        for bad in [
            "/static/foo.js",
            "/static/htmx.min.js",
            "/static/htmx-.js",
            "/static/htmx-A.css",
            "/static/htmx-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA.js",
        ] {
            let r = router(open())
                .oneshot(req("GET", bad))
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{bad}");
        }
    }

    #[tokio::test]
    async fn robots_txt_disallows_all() {
        let r = router(open()).oneshot(req("GET", "/robots.txt")).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let body = to_bytes(r.into_body(), usize::MAX).await.unwrap();
        assert!(std::str::from_utf8(&body).unwrap().contains("Disallow: /"));
    }

    #[tokio::test]
    async fn csrf_rejects_post_without_hx_request_header() {
        let r = router(open()).oneshot(req("POST", "/write")).await.unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn csrf_accepts_post_with_hx_request_header() {
        let mut r = req("POST", "/write");
        r.headers_mut()
            .insert("HX-Request", axum::http::HeaderValue::from_static("true"));
        let resp = router(open()).oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn csrf_accepts_same_origin_post_via_sec_fetch_site() {
        // Some clients (e.g. JSON API tests, server-to-server) won't set
        // HX-Request but DO send Sec-Fetch-Site: same-origin.
        let mut r = req("POST", "/write");
        r.headers_mut().insert(
            "Sec-Fetch-Site",
            axum::http::HeaderValue::from_static("same-origin"),
        );
        let resp = router(open()).oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn csrf_rejects_cross_origin_post_even_with_hx_request() {
        let mut r = req("POST", "/write");
        r.headers_mut()
            .insert("HX-Request", axum::http::HeaderValue::from_static("true"));
        r.headers_mut().insert(
            "Origin",
            axum::http::HeaderValue::from_static("https://evil.example"),
        );
        let resp = router(open()).oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn csrf_accepts_same_origin_post_with_matching_origin() {
        let mut r = req("POST", "/write");
        r.headers_mut()
            .insert("HX-Request", axum::http::HeaderValue::from_static("true"));
        r.headers_mut().insert(
            "Origin",
            axum::http::HeaderValue::from_static("https://billing.example.com"),
        );
        let resp = router(open()).oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn csrf_accepts_post_from_explicit_allowed_origin() {
        let mut sec = open();
        sec.allowed_origins.push("https://ops.example.com".into());
        let mut r = req("POST", "/write");
        r.headers_mut()
            .insert("HX-Request", axum::http::HeaderValue::from_static("true"));
        r.headers_mut().insert(
            "Origin",
            axum::http::HeaderValue::from_static("https://ops.example.com"),
        );
        let resp = router(sec).oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn bearer_disabled_when_unset() {
        // Default state has bearer=None — GET should pass with no auth header.
        let resp = router(open()).oneshot(req("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn bearer_required_when_set_and_missing() {
        let sec = AdminSecurity {
            bearer: Some("super-secret-token".into()),
            allowed_origins: vec![],
        };
        let resp = router(sec).oneshot(req("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers().get("www-authenticate").unwrap(),
            "Bearer realm=\"billing-admin\""
        );
    }

    #[tokio::test]
    async fn bearer_required_when_set_and_wrong() {
        let sec = AdminSecurity {
            bearer: Some("super-secret-token".into()),
            allowed_origins: vec![],
        };
        let mut r = req("GET", "/");
        r.headers_mut().insert(
            "Authorization",
            axum::http::HeaderValue::from_static("Bearer nope"),
        );
        let resp = router(sec).oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bearer_partial_prefix_is_not_accepted() {
        // Regression guard against a `starts_with` / non-constant-time bug.
        let sec = AdminSecurity {
            bearer: Some("super-secret-token".into()),
            allowed_origins: vec![],
        };
        let mut r = req("GET", "/");
        r.headers_mut().insert(
            "Authorization",
            axum::http::HeaderValue::from_static("Bearer super-secret"),
        );
        let resp = router(sec).oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bearer_accepts_correct_token() {
        let sec = AdminSecurity {
            bearer: Some("super-secret-token".into()),
            allowed_origins: vec![],
        };
        let mut r = req("GET", "/");
        r.headers_mut().insert(
            "Authorization",
            axum::http::HeaderValue::from_static("Bearer super-secret-token"),
        );
        let resp = router(sec).oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn bearer_blocks_static_asset_when_set() {
        // Auth middleware runs outermost — even the static htmx asset
        // requires the bearer when one is configured. This matters
        // because the script tag loads with `crossorigin=anonymous` and
        // the browser will send cookies/headers from the original
        // navigation, so an authed page also gets an authed script.
        let sec = AdminSecurity {
            bearer: Some("super-secret-token".into()),
            allowed_origins: vec![],
        };
        let r = router(sec).oneshot(req("GET", &assets::htmx_asset_path())).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }
}
