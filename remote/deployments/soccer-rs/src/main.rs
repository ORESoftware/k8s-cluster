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
//!   * `GET  /soccer/inspect?id=<uuid>` — read-only dump of the game's full engine
//!                                      internals for an external debugger/inspector
//!                                      (`&weights=1` embeds raw NN weights; gated by
//!                                      `SOCCER_INSPECT_TOKEN` when set).
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
    http::{header, HeaderMap, Method, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// `SoccerRealtimeSession` is the agnostic game core (also driven directly by the
// desktop client). `SoccerLiveHttpBridge` is HTTP glue that lives in the
// `soccer_engine` crate today; when that crate is made fully agnostic (its bridge
// moves behind the `web-bridge` feature / into the server layer) this server
// drives `SoccerRealtimeSession` directly so the engine carries no web deps.
use soccer_engine::soccer::{SoccerLiveHttpBridge, SoccerLiveServerConfig};

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

#[derive(Deserialize)]
struct InspectQuery {
    id: Option<Uuid>,
    /// `weights=1` additionally embeds the raw neural-network snapshot (large).
    #[serde(default)]
    weights: Option<u8>,
    /// Token may be supplied here instead of an `Authorization: Bearer` header.
    #[serde(default)]
    token: Option<String>,
}

/// `GET /soccer/inspect?id=<uuid>[&weights=1]` — read-only dump of the game's FULL
/// engine internals for an external debugger/inspector: the physical frame plus the
/// neural critic state, Q-policy aggregates, every agent's MDP/POMDP decision (with
/// its observation vector, masked+scored options and chosen target), the
/// central-brain/formation-LP decision, and the reward plumbing.
///
/// This is the **deep** inspection tier: a one-shot dump of all the decision/learning
/// internals. It complements the engine's own `GET /soccer/api/inspect` (proxied
/// through `game_api`), which is the **fast** tier — a zero-copy mmap ring serving
/// curated kinematics + time-series history (`?player=&fields=&history=&section=`).
/// Use the proxied ring path for high-frequency field/history polling; use this for
/// the full why-did-it-do-that snapshot (per-agent MDP/POMDP decisions, neural
/// critic, Q aggregates, reward plumbing, central-brain/LP).
///
/// It is the "attach and read the engine's memory" seam done as structured data:
/// **pull-based** (nothing is computed until this is hit, under one brief session
/// lock) so it costs nothing when idle — far cheaper than continuously streaming
/// everything to I/O — and the engine stays transport-agnostic (it just hands back
/// JSON via the bridge; this server owns the HTTP). Gated by `SOCCER_INSPECT_TOKEN`
/// when that env var is set; default-open otherwise, for the in-cluster workflow.
async fn inspect_game(
    State(state): State<AppState>,
    Query(q): Query<InspectQuery>,
    headers: HeaderMap,
) -> Response {
    if let Some(expected) = inspect_token() {
        let provided = bearer_token(&headers).or(q.token.as_deref());
        let authorized = provided
            .map(|token| constant_time_eq(token, &expected))
            .unwrap_or(false);
        if !authorized {
            return (StatusCode::FORBIDDEN, "invalid or missing inspect token").into_response();
        }
    }
    let session = match resolve(&state, q.id) {
        Ok(session) => session,
        Err(resp) => return resp,
    };
    let include_weights = matches!(q.weights, Some(n) if n != 0);
    Json(session.bridge.inspector_snapshot(include_weights)).into_response()
}

/// The inspect-endpoint admin token, if one is configured. When unset the endpoint
/// is open (the in-cluster default); when set, callers must present it.
fn inspect_token() -> Option<String> {
    std::env::var("SOCCER_INSPECT_TOKEN")
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

/// Constant-time string comparison so a wrong token cannot be recovered by timing
/// the response. Mirrors the engine's own admin-token check.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Extract a bearer token from the `Authorization` header, if present.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
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
fn bridge_reply(reply: soccer_engine::soccer::SoccerLiveHttpReply) -> Response {
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let state = AppState::new();

    // Background TTL reaper so abandoned games don't leak.
    {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let evicted = state.reap();
                if evicted > 0 {
                    tracing::error!("dd-soccer-rs: reaped {evicted} idle game(s)");
                }
            }
        });
    }

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/soccer/game", post(create_game).get(game_meta))
        .route("/soccer/live", get(live_ui))
        .route("/soccer/sim", get(sim_view))
        .route("/soccer/inspect", get(inspect_game))
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
    tracing::error!("dd-soccer-rs listening on {addr}");
    axum::serve(listener, app)
        .await
        .expect("dd-soccer-rs serve");
}
