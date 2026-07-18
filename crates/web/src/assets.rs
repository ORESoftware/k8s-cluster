//! Self-hosted static assets. htmx is vendored into the binary (no CDN), so
//! the dashboard runs under a strict `script-src 'self'` CSP and works with no
//! outbound network at request time.

use axum::http::header;
use axum::response::IntoResponse;

const JS: &str = "application/javascript; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
// Content is versioned by the binary; safe to cache aggressively.
const CACHE: &str = "public, max-age=31536000, immutable";

pub async fn htmx_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, JS), (header::CACHE_CONTROL, CACHE)],
        include_bytes!("../assets/htmx.min.js").as_slice(),
    )
}

pub async fn htmx_ws_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, JS), (header::CACHE_CONTROL, CACHE)],
        include_bytes!("../assets/htmx-ws.js").as_slice(),
    )
}

pub async fn app_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, CSS), (header::CACHE_CONTROL, CACHE)],
        crate::views::STYLE,
    )
}
