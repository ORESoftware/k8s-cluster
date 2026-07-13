//! Server-rendered HTMX operator console, mounted under `/app`.
//!
//! A parallel surface to the JSON API in [`crate::routes`]: it reads and
//! writes the same Postgres tables through the same pool, but renders HTML
//! fragments driven by a vendored, self-hosted HTMX (no CDN, strict CSP).
//!
//! Posture (see [`security`]):
//!   - **Optional bearer** via `USACC_APP_UI_BEARER`; unset → open, for
//!     trusted networks behind the gateway auth the JSON API already uses.
//!   - **CSRF defense in depth**: unsafe methods must carry `HX-Request`
//!     or a same-site `Sec-Fetch-Site`, and any `Origin` must match.
//!   - **Strict security headers** (CSP without `'unsafe-eval'`/inline JS)
//!     on every response.
//!
//! Every URL is built through [`layout::Ui`] with the configured base path
//! (`USACC_APP_BASE_PATH`) so the same binary works both directly and
//! behind the path-stripping gateway (`/usacc/app`).
//!
//! Routes:
//!   - `GET  /app`                         dashboard
//!   - `GET  /app/status`                  status pill fragment
//!   - `GET  /app/cases`                   list + create form
//!   - `POST /app/cases`                   create (returns refreshed list)
//!   - `GET  /app/cases/:id`               case detail
//!   - `GET  /app/users`                   list + create form
//!   - `POST /app/users`                   create
//!   - `GET  /app/elections`               list + create form
//!   - `POST /app/elections`               create
//!   - `GET  /app/elections/:id`           election detail + votes
//!   - `POST /app/elections/:id/tally`     compute + certify (HTMX action)
//!   - `GET  /app/simulations`             run form
//!   - `POST /app/simulations`             run DES simulation
//!   - `GET  /app/static/:file`            vendored htmx asset
//!   - `GET  /app/robots.txt`              `noindex, nofollow`

mod assets;
mod cases;
mod dashboard;
mod elections;
mod layout;
mod security;
mod simulations;
mod users;

use axum::http::{header, StatusCode};
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use maud::{html, Markup};
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub use assets::verify_integrity as verify_asset_integrity;

/// Build the console sub-router (state is applied by the caller). The
/// security middleware is attached here so it scopes to `/app` routes only
/// when this is merged into the main router.
pub fn router(state: &AppState) -> Router<AppState> {
    let sec = security::UiSecurity::from_state(state);
    Router::new()
        .route("/app", get(dashboard::page))
        .route("/app/status", get(dashboard::status_fragment))
        .route("/app/cases", get(cases::list_page).post(cases::create))
        .route("/app/cases/:id", get(cases::detail_page))
        .route("/app/users", get(users::list_page).post(users::create))
        .route(
            "/app/elections",
            get(elections::list_page).post(elections::create),
        )
        .route("/app/elections/:id", get(elections::detail_page))
        .route("/app/elections/:id/tally", post(elections::tally))
        .route(
            "/app/simulations",
            get(simulations::page).post(simulations::run),
        )
        .route("/app/static/:file", get(assets::serve))
        .route("/app/robots.txt", get(robots_txt))
        // Outermost runs first on the request path: trace -> auth -> csrf
        // -> headers. The console is deliberately NOT wrapped by the JSON
        // API's permissive CORS layer (that exists for cross-origin API
        // consumers); the console is same-origin only and relies on the
        // CSRF guard + same-origin security headers.
        .layer(middleware::from_fn(security::security_headers))
        .layer(middleware::from_fn_with_state(
            sec.clone(),
            security::csrf_guard,
        ))
        .layer(middleware::from_fn_with_state(
            sec,
            security::require_ui_auth,
        ))
        .layer(TraceLayer::new_for_http())
}

/// Log a database error in full server-side and return a generic flash, so
/// raw `sqlx::Error` text (constraint names, column types, SQL fragments)
/// is never reflected to the client. `action` completes "Could not …".
pub(super) fn report_db_error(action: &str, err: impl std::fmt::Display) -> Markup {
    tracing::error!(action, error = %err, "console database error");
    layout::flash_error(format!("Could not {action}. Check the server logs for details."))
}

/// Body shown by every page when no database is configured.
fn no_db_body() -> Markup {
    html! {
        h1 { "Console" }
        (layout::flash_error(
            "No database is configured. Set USACC_DATABASE_URL (or DATABASE_URL) \
             to enable the console's data views."
        ))
    }
}

async fn robots_txt() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "User-agent: *\nDisallow: /\n",
    )
}

#[cfg(test)]
mod tests {
    //! Wire-level guards for the console's security posture. They build the
    //! production router shape against a pool-less `AppState` (every page
    //! degrades to the "no database" body, which is all we need to exercise
    //! the middleware) and drive it with `tower::ServiceExt::oneshot`.

    use std::time::Duration;

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::assets;
    use crate::config::Config;
    use crate::state::AppState;

    fn cfg(bearer: Option<String>) -> Config {
        Config {
            host: "0.0.0.0".into(),
            port: 8121,
            database_url: None,
            auth_secret: None,
            auth_required: false,
            contract_service_url: "http://localhost".into(),
            request_timeout: Duration::from_secs(5),
            max_page_limit: 250,
            app_ui_enabled: true,
            app_base_path: String::new(),
            app_ui_bearer: bearer,
            app_ui_allowed_origins: vec![],
        }
    }

    fn app(bearer: Option<String>) -> axum::Router {
        let state = AppState::new(cfg(bearer), None, reqwest::Client::new());
        super::router(&state).with_state(state)
    }

    fn req(method: &str, uri: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("Host", "usacc.example.com")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn get_page_carries_security_headers_no_store_and_no_cors() {
        let resp = app(None).oneshot(req("GET", "/app")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let h = resp.headers();
        let csp = h.get("content-security-policy").unwrap().to_str().unwrap();
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("script-src 'self'"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(!csp.contains("'unsafe-eval'"));
        assert_eq!(h.get("x-frame-options").unwrap(), "DENY");
        assert_eq!(h.get("x-content-type-options").unwrap(), "nosniff");
        assert_eq!(h.get("cache-control").unwrap(), "no-store");
        assert_eq!(
            h.get("x-robots-tag").unwrap(),
            "noindex, nofollow, noarchive"
        );
        // The console must NOT advertise permissive CORS — it's same-origin.
        assert!(h.get("access-control-allow-origin").is_none());
    }

    #[tokio::test]
    async fn static_asset_keeps_immutable_cache_not_no_store() {
        let path = assets::htmx_asset_suffix();
        let resp = app(None).oneshot(req("GET", &path)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("cache-control").unwrap(),
            "public, max-age=31536000, immutable"
        );
    }

    #[tokio::test]
    async fn static_asset_rejects_unknown_filenames() {
        for bad in ["/app/static/foo.js", "/app/static/htmx.min.js", "/app/static/htmx-.js"] {
            let resp = app(None).oneshot(req("GET", bad)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{bad}");
        }
    }

    #[tokio::test]
    async fn robots_disallows_all() {
        let resp = app(None).oneshot(req("GET", "/app/robots.txt")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert!(std::str::from_utf8(&body).unwrap().contains("Disallow: /"));
    }

    #[tokio::test]
    async fn csrf_rejects_write_without_hx_request() {
        let resp = app(None).oneshot(req("POST", "/app/cases")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn csrf_allows_write_with_hx_request() {
        let r = Request::builder()
            .method("POST")
            .uri("/app/cases")
            .header("Host", "usacc.example.com")
            .header("HX-Request", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(
                "case_number=x&title=t&defendant_summary=d&conduct_summary=c&status=draft&filing_tier=screen",
            ))
            .unwrap();
        // Reaches the handler (then short-circuits on the absent pool).
        let resp = app(None).oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn csrf_rejects_cross_origin_write_even_with_hx_request() {
        let r = Request::builder()
            .method("POST")
            .uri("/app/cases")
            .header("Host", "usacc.example.com")
            .header("HX-Request", "true")
            .header("Origin", "https://evil.example")
            .body(Body::empty())
            .unwrap();
        let resp = app(None).oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn bearer_required_when_set() {
        let bearer = Some("super-secret-token".to_string());
        let missing = app(bearer.clone()).oneshot(req("GET", "/app")).await.unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            missing.headers().get("www-authenticate").unwrap(),
            "Bearer realm=\"usacc-console\""
        );

        let wrong = Request::builder()
            .method("GET")
            .uri("/app")
            .header("Host", "usacc.example.com")
            .header("Authorization", "Bearer super-secret")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app(bearer.clone()).oneshot(wrong).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );

        let ok = Request::builder()
            .method("GET")
            .uri("/app")
            .header("Host", "usacc.example.com")
            .header("Authorization", "Bearer super-secret-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app(bearer).oneshot(ok).await.unwrap().status(),
            StatusCode::OK
        );
    }
}
