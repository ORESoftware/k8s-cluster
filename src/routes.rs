use std::time::Duration;

use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{header::CACHE_CONTROL, HeaderMap, HeaderName, HeaderValue, Request},
    middleware::{self, Next},
    response::Response,
    routing::get,
    Json, Router,
};
use maud::{html, Markup};
use tokio::time;
use tower_http::{services::ServeDir, trace::TraceLayer};

use crate::{
    app::{asset_dir, AppState, PublicConfig},
    data::DashboardStats,
    views,
};

pub(crate) fn router(state: AppState) -> Router {
    let base_path = state.base_path.clone();
    let routes = if base_path.is_empty() {
        app_routes()
    } else {
        let base_path_with_slash = format!("{base_path}/");
        Router::new()
            .merge(app_routes())
            .route(base_path_with_slash.as_str(), get(home))
            .nest(base_path.as_str(), app_routes())
    };

    let header_state = state.clone();
    routes
        .layer(middleware::from_fn_with_state(
            header_state,
            security_headers,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

fn app_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(home))
        .route("/healthz", get(healthz))
        .route("/config", get(public_config))
        .route("/portal", get(portal))
        .route("/partials/overview", get(overview_partial))
        .route("/partials/games", get(games_partial))
        .route("/partials/sessions", get(sessions_partial))
        .route("/partials/auth", get(auth_partial))
        .route("/ws/portal", get(portal_ws))
        .nest_service("/assets", ServeDir::new(asset_dir()))
}

async fn home(State(state): State<AppState>) -> Markup {
    views::render_page(&state, "home", views::home_body(&state).await)
}

async fn portal(State(state): State<AppState>) -> Markup {
    views::render_page(&state, "portal", views::portal_body(&state).await)
}

async fn overview_partial(State(state): State<AppState>) -> Markup {
    views::overview_panel(&DashboardStats::load(&state).await)
}

async fn games_partial(State(state): State<AppState>) -> Markup {
    views::games_panel(&DashboardStats::load(&state).await)
}

async fn sessions_partial(State(state): State<AppState>) -> Markup {
    views::sessions_panel(&DashboardStats::load(&state).await)
}

async fn auth_partial(State(state): State<AppState>) -> Markup {
    views::auth_panel(&state)
}

async fn public_config(State(state): State<AppState>) -> (HeaderMap, Json<PublicConfig>) {
    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    (headers, Json(state.public_config()))
}

async fn portal_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| stream_portal_stats(socket, state))
}

async fn stream_portal_stats(mut socket: WebSocket, state: AppState) {
    let mut interval = time::interval(Duration::from_secs(2));
    loop {
        interval.tick().await;
        let stats = DashboardStats::load(&state).await;
        let update = html! {
            div id="live-ticker" hx-swap-oob="true" {
                (views::live_ticker(&stats))
            }
            div id="metric-running" hx-swap-oob="true" {
                (views::metric_value(stats.games_running, "running games"))
            }
            div id="metric-sessions" hx-swap-oob="true" {
                (views::metric_value(stats.active_sessions, "active sessions"))
            }
        }
        .into_string();

        if socket.send(Message::Text(update)).await.is_err() {
            break;
        }
    }
}

async fn healthz() -> &'static str {
    "ok"
}

async fn security_headers(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_str(&state.content_security_policy())
            .unwrap_or_else(|_| HeaderValue::from_static("default-src 'self'")),
    );
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), geolocation=(), microphone=()"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    response
}
