mod auth;
mod health;
mod pages;
mod quote;
mod websocket;

pub mod api;

use crate::{metrics, views, AppState};
use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Router,
};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

const APP_CONTENT_SECURITY_POLICY_PREFIX: &str =
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:";
const MARKETING_CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; base-uri 'self'; form-action 'self'; frame-ancestors 'none'; object-src 'none'";

pub fn router(state: AppState) -> Router {
    let marketing_files = ServeDir::new(&state.config.static_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(state.config.static_dir.join("index.html")));
    let app_assets = ServeDir::new(&state.config.app_asset_dir);
    let content_security_policy =
        application_content_security_policy(&state.config.allowed_origins);

    let private_application = Router::new()
        .route("/login", get(auth::login_page))
        .route("/ws", axum::routing::any(websocket::upgrade))
        .route("/u/quote", get(quote::page).post(quote::submit))
        .route("/u/quote/{id}", get(quote::detail))
        .nest("/api", api::router())
        .nest("/auth", auth::router())
        .nest("/app", pages::router())
        // These responses can contain identity, CSRF tokens, or customer
        // records. Keep them out of browser and shared intermediary caches;
        // static application assets and the marketing fallback stay outside
        // this layer so their independent cache policy remains available.
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ));
    let application = Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/metrics", get(metrics::endpoint))
        .merge(private_application)
        // Administrative UI/API lives on a separate future origin and
        // process. Reserve this namespace so it can never be answered by the
        // marketing SPA fallback on the customer server.
        .route("/admin", any(admin_not_found))
        .route("/admin/{*path}", any(admin_not_found))
        .nest_service("/app-assets", app_assets)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            content_security_policy,
        ));
    let marketing = Router::new().fallback_service(marketing_files).layer(
        SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(MARKETING_CONTENT_SECURITY_POLICY),
        ),
    );

    Router::new()
        .merge(application)
        .fallback_service(marketing)
        .with_state(state)
}

/// Router for the separately deployed `canonical-api-server` process.
///
/// It deliberately has no marketing fallback, password-login page, or account
/// UI. Session-cookie REST and WebSocket calls still pass through the same
/// origin/CSRF/session validation used by the combined web process.
pub fn api_only_router(state: AppState) -> Router {
    let content_security_policy =
        application_content_security_policy(&state.config.allowed_origins);
    let private_api = Router::new()
        .route("/ws", axum::routing::any(websocket::upgrade))
        .nest("/api", api::router())
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ));
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/metrics", get(metrics::endpoint))
        .merge(private_api)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            content_security_policy,
        ))
        .with_state(state)
}

async fn admin_not_found() -> Response {
    (StatusCode::NOT_FOUND, views::html_not_found()).into_response()
}

fn application_content_security_policy(
    allowed_origins: &std::collections::HashSet<String>,
) -> HeaderValue {
    let mut websocket_sources = allowed_origins
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
    HeaderValue::from_str(&format!(
        "{APP_CONTENT_SECURITY_POLICY_PREFIX}; connect-src 'self' {}; object-src 'none'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
        websocket_sources.join(" ")
    ))
    .expect("validated origins produce a valid CSP header")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn application_csp_places_websockets_only_in_connect_src() {
        let origins = HashSet::from([
            "https://app.example.com".to_owned(),
            "http://localhost:8081".to_owned(),
        ]);
        let header = application_content_security_policy(&origins);
        let policy = header.to_str().unwrap();
        let directives = policy
            .split(';')
            .map(str::trim)
            .filter(|directive| !directive.is_empty())
            .collect::<Vec<_>>();

        assert!(
            directives.contains(&"connect-src 'self' ws://localhost:8081 wss://app.example.com")
        );
        assert!(directives.contains(&"object-src 'none'"));
        assert_eq!(
            directives
                .iter()
                .filter(|directive| directive.starts_with("object-src "))
                .copied()
                .collect::<Vec<_>>(),
            vec!["object-src 'none'"]
        );
    }
}
