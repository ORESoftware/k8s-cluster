mod auth;
mod health;
mod pages;
mod websocket;

pub mod api;

use crate::AppState;
use axum::{
    http::{header, HeaderValue},
    routing::get,
    Router,
};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

const APP_CONTENT_SECURITY_POLICY_PREFIX: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'";

pub fn router(state: AppState) -> Router {
    let marketing = ServeDir::new(&state.config.static_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(state.config.static_dir.join("index.html")));
    let app_assets = ServeDir::new(&state.config.app_asset_dir);
    let mut websocket_sources = state
        .config
        .allowed_origins
        .iter()
        .map(|origin| {
            origin
                .strip_prefix("https://")
                .map(|rest| format!("wss://{rest}"))
                .or_else(|| {
                    origin
                        .strip_prefix("http://")
                        .map(|rest| format!("ws://{rest}"))
                })
                .expect("validated application origin")
        })
        .collect::<Vec<_>>();
    websocket_sources.sort();
    let content_security_policy = HeaderValue::from_str(&format!(
        "{APP_CONTENT_SECURITY_POLICY_PREFIX} {}; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
        websocket_sources.join(" ")
    ))
    .expect("validated origins produce a valid CSP header");

    let application = Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/login", get(auth::login_page))
        .route("/ws", axum::routing::any(websocket::upgrade))
        .nest("/api", api::router())
        .nest("/auth", auth::router())
        .nest("/app", pages::router())
        .nest_service("/app-assets", app_assets)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            content_security_policy,
        ));

    Router::new()
        .merge(application)
        .fallback_service(marketing)
        .with_state(state)
}
