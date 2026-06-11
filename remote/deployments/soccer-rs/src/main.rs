//! # dd-soccer-rs — root soccer game server
//!
//! A multi-game wrapper around the soccer engine's single-session live bridge.
//! Each game is identified by a **UUID** and backed by its own
//! [`SoccerLiveHttpBridge`]; the registry maps `Uuid -> Arc<GameSession>`.
//!
//! Routes (uuid passed as `?id=<uuid>`):
//!   * `POST /soccer/game`            — mint a uuid, start a game, return `{id}`.
//!   * `GET  /soccer/game?id=<uuid>`  — game metadata / liveness.
//!   * `GET  /soccer/live?id=<uuid>`  — live 2D UI bound to the game.
//!   * `GET  /soccer/sim?id=<uuid>`   — static/replay view of the game.
//!   * `*    /api/*?id=<uuid>`        — the live bridge API, scoped to the game
//!                                      (state/step/reset/input/team-policy).
//!   * `GET  /healthz`                — liveness for k8s probes.
//!
//! Back-compat: the existing `dd-des-rs` server keeps its single-session
//! `/des-rs/soccer/live` etc.; this is the new *root* server for uuid games.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{header, Method, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// `SoccerRealtimeSession` is the agnostic game core (also driven directly by the
// desktop client). `SoccerLiveHttpBridge` is HTTP glue that currently still
// lives in the engine — TRANSITIONAL: when the engine is made fully agnostic the
// bridge's method/path/body→session translation moves *into this server crate*
// and drives `SoccerRealtimeSession` directly, so the engine carries no web deps.
use soccer_sim_game_engine::soccer::{SoccerLiveHttpBridge, SoccerLiveServerConfig};

/// How long an idle game lingers before the reaper drops it.
const GAME_TTL: Duration = Duration::from_secs(60 * 30);

/// One running game: its own live bridge plus bookkeeping for TTL eviction.
struct GameSession {
    bridge: Arc<SoccerLiveHttpBridge>,
    created: Instant,
}

#[derive(Clone)]
struct AppState {
    games: Arc<Mutex<HashMap<Uuid, Arc<GameSession>>>>,
}

impl AppState {
    fn new() -> Self {
        Self {
            games: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn lookup(&self, id: Uuid) -> Option<Arc<GameSession>> {
        self.games.lock().expect("games lock").get(&id).cloned()
    }

    fn insert(&self, id: Uuid, session: GameSession) {
        self.games.lock().expect("games lock").insert(id, Arc::new(session));
    }

    /// Drop games idle longer than [`GAME_TTL`]; returns the count evicted.
    fn reap(&self) -> usize {
        let mut games = self.games.lock().expect("games lock");
        let before = games.len();
        games.retain(|_, session| session.created.elapsed() < GAME_TTL);
        before - games.len()
    }
}

#[derive(Deserialize)]
struct GameQuery {
    id: Option<Uuid>,
}

#[derive(Serialize)]
struct CreatedGame {
    id: Uuid,
}

/// `POST /soccer/game` — create a fresh game and return its uuid.
async fn create_game(State(state): State<AppState>) -> impl IntoResponse {
    let id = Uuid::new_v4();
    // A default live config per game; the match itself starts at kickoff.
    let bridge = Arc::new(SoccerLiveHttpBridge::new(SoccerLiveServerConfig::default()));
    state.insert(
        id,
        GameSession {
            bridge,
            created: Instant::now(),
        },
    );
    (StatusCode::CREATED, Json(CreatedGame { id }))
}

/// `GET /soccer/game?id=<uuid>` — game liveness metadata (delegates to the
/// bridge's state endpoint).
async fn game_meta(State(state): State<AppState>, Query(q): Query<GameQuery>) -> Response {
    match resolve(&state, q.id) {
        Ok(session) => bridge_reply(session.bridge.handle_request("GET", "/api/state", "")),
        Err(resp) => resp,
    }
}

/// `* /api/*?id=<uuid>` — proxy any live-bridge request to the game's bridge.
async fn game_api(
    State(state): State<AppState>,
    Query(q): Query<GameQuery>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Response {
    let session = match resolve(&state, q.id) {
        Ok(session) => session,
        Err(resp) => return resp,
    };
    let body = String::from_utf8_lossy(&body).into_owned();
    // Routes are nested under /soccer so a single gateway prefix covers the whole
    // server; the engine bridge speaks the un-prefixed /api/* paths, so strip it.
    let bridge_path = uri.path().strip_prefix("/soccer").unwrap_or(uri.path());
    bridge_reply(
        session
            .bridge
            .handle_request(method.as_str(), bridge_path, &body),
    )
}

/// `GET /soccer/live?id=<uuid>` — live UI bound to the game (its JS calls
/// `/api/*?id=<uuid>`). Minimal placeholder; the full canvas UI is ported next.
async fn live_ui(Query(q): Query<GameQuery>) -> Response {
    match q.id {
        Some(id) => Html(live_html(id)).into_response(),
        None => (StatusCode::BAD_REQUEST, "missing ?id=<uuid>").into_response(),
    }
}

/// `GET /soccer/sim?id=<uuid>` — static/replay view of a game.
async fn sim_view(State(state): State<AppState>, Query(q): Query<GameQuery>) -> Response {
    match resolve(&state, q.id) {
        // The bridge renders the current frame; a fuller replay reads the game's
        // persisted playback artifacts (follow-up).
        Ok(session) => bridge_reply(session.bridge.handle_request("GET", "/api/state", "")),
        Err(resp) => resp,
    }
}

async fn healthz() -> &'static str {
    "ok"
}

/// Resolve a `?id=` to a live session, or the appropriate error response.
fn resolve(state: &AppState, id: Option<Uuid>) -> Result<Arc<GameSession>, Response> {
    let id = id.ok_or_else(|| (StatusCode::BAD_REQUEST, "missing ?id=<uuid>").into_response())?;
    state
        .lookup(id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "no such game (expired or never created)").into_response())
}

/// Map the engine's `SoccerLiveHttpReply` onto an axum response.
fn bridge_reply(reply: soccer_sim_game_engine::soccer::SoccerLiveHttpReply) -> Response {
    let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        [(header::CONTENT_TYPE, reply.content_type)],
        reply.body,
    )
        .into_response()
}

fn live_html(id: Uuid) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>soccer/live {id}</title></head>\
         <body><h1>soccer/live</h1><p>game id: <code>{id}</code></p>\
         <pre id=\"state\">loading…</pre>\
         <script>\
           const id={id:?};\
           async function tick(){{\
             const r=await fetch('/soccer/api/state?id='+id);\
             document.getElementById('state').textContent=await r.text();\
             await fetch('/soccer/api/step?id='+id,{{method:'POST',headers:{{'content-type':'application/json'}},body:'{{}}'}});\
             setTimeout(tick,100);\
           }}\
           tick();\
         </script></body></html>"
    )
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
        .route("/healthz", get(healthz))
        .route("/soccer/game", post(create_game).get(game_meta))
        .route("/soccer/live", get(live_ui))
        .route("/soccer/sim", get(sim_view))
        .route("/soccer/api/*rest", any(game_api))
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
