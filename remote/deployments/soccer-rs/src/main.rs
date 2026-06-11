//! # dd-soccer-rs — root soccer game server
//!
//! Serves soccer_engine's full live match UI, with each match addressed by a
//! game uuid in the query string — exactly what the UI expects:
//!
//!   GET  /?game=<uuid>      — the live 2D match UI. The HTML is game-agnostic;
//!                             its JS reads `?game=` (minting + address-bar-
//!                             rewriting one if absent) and scopes every
//!                             `/api/*` call to that game.
//!   *    /api/*?game=<uuid> — the live bridge API for that game. The game (its
//!                             own match + bridge) is created on first contact.
//!   GET  /healthz           — k8s liveness.
//!
//! The same UI also works behind a `/soccer` gateway prefix (the UI detects the
//! mount and prefixes its calls), so `/soccer/live` + `/soccer/api/*` are served
//! too; the `/soccer` prefix is stripped before the engine bridge sees the path.
//!
//! Each game uuid maps to its own [`SoccerLiveHttpBridge`] (its own match);
//! idle games are reaped after [`GAME_TTL`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{header, Method, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::{any, get},
    Router,
};
use serde::Deserialize;

use soccer_engine::soccer::{soccer_live_page_html, SoccerLiveHttpBridge, SoccerLiveServerConfig};

/// How long an idle game lingers before the reaper drops it.
const GAME_TTL: Duration = Duration::from_secs(60 * 30);

/// One running game: its own live bridge plus bookkeeping for TTL eviction.
struct GameSession {
    bridge: Arc<SoccerLiveHttpBridge>,
    created: Instant,
}

#[derive(Clone)]
struct AppState {
    games: Arc<Mutex<HashMap<String, Arc<GameSession>>>>,
}

impl AppState {
    fn new() -> Self {
        Self {
            games: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get the game for `id`, creating its match + bridge on first contact
    /// (the UI's `/api/*?game=<uuid>` calls are what trigger creation).
    fn get_or_create(&self, id: &str) -> Arc<GameSession> {
        self.games
            .lock()
            .expect("games lock")
            .entry(id.to_string())
            .or_insert_with(|| {
                Arc::new(GameSession {
                    bridge: Arc::new(SoccerLiveHttpBridge::new(SoccerLiveServerConfig::default())),
                    created: Instant::now(),
                })
            })
            .clone()
    }

    /// Drop games idle longer than [`GAME_TTL`]; returns the count evicted.
    fn reap(&self) -> usize {
        let mut games = self.games.lock().expect("games lock");
        let before = games.len();
        games.retain(|_, s| s.created.elapsed() < GAME_TTL);
        before - games.len()
    }
}

#[derive(Deserialize)]
struct GameParam {
    game: Option<String>,
    /// `?id=` accepted as an alias for `?game=`.
    id: Option<String>,
}

/// Sanitize to the same shape the live UI uses for ids: `[a-z0-9-]`, max 64.
fn sanitize_game_id(raw: &str) -> String {
    raw.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(64)
        .collect()
}

/// `GET /` and `GET /soccer/live` — the live match UI (game-agnostic HTML).
async fn live_ui() -> Response {
    Html(soccer_live_page_html()).into_response()
}

/// `* /api/*?game=<uuid>` (and `/soccer/api/*`) — proxy to the game's bridge,
/// creating the game on first hit. The optional `/soccer` mount prefix is
/// stripped so the engine bridge sees its native `/api/*` paths.
async fn game_api(
    State(state): State<AppState>,
    Query(q): Query<GameParam>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Response {
    let id = sanitize_game_id(q.game.or(q.id).as_deref().unwrap_or(""));
    if id.is_empty() {
        return (StatusCode::BAD_REQUEST, "missing ?game=<uuid>").into_response();
    }
    let session = state.get_or_create(&id);
    let bridge_path = uri.path().strip_prefix("/soccer").unwrap_or(uri.path());
    let body = String::from_utf8_lossy(&body).into_owned();
    bridge_reply(
        session
            .bridge
            .handle_request(method.as_str(), bridge_path, &body),
    )
}

/// Map the engine's `SoccerLiveHttpReply` onto an axum response.
fn bridge_reply(reply: soccer_engine::soccer::SoccerLiveHttpReply) -> Response {
    let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        [(header::CONTENT_TYPE, reply.content_type)],
        reply.body,
    )
        .into_response()
}

async fn healthz() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() {
    let state = AppState::new();

    // Background TTL reaper so abandoned games don't leak.
    {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let evicted = state.reap();
                if evicted > 0 {
                    eprintln!("dd-soccer-rs: reaped {evicted} idle game(s)");
                }
            }
        });
    }

    let app = Router::new()
        .route("/", get(live_ui))
        .route("/api/*rest", any(game_api))
        .route("/soccer/live", get(live_ui))
        .route("/soccer/api/*rest", any(game_api))
        .route("/healthz", get(healthz))
        .with_state(state);

    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8113);
    let addr = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("dd-soccer-rs bind {addr}: {e}"));
    eprintln!("dd-soccer-rs listening on {addr}");
    axum::serve(listener, app)
        .await
        .expect("dd-soccer-rs serve");
}
