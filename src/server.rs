//! # dd-soccer-rs — root soccer game server
//!
//! A multi-game wrapper around the soccer engine's single-session live bridge.
//! Each game is identified by a sanitized string id (`[a-z0-9-]`, max 64 — the same
//! shape the live UI mints client-side) and backed by its own
//! [`SoccerLiveHttpBridge`]; the registry maps `String -> Arc<GameSession>`.
//!
//! The game id is accepted as `?game=<id>` (what the engine UI sends) or `?id=<id>`
//! (API/doc clients); games are created **lazily** on first contact, seeded from
//! their id so distinct ids play out differently.
//!
//! Routes:
//!
//! - `GET /soccer`, `/soccer/`, `/soccer/live` — the full 2D canvas live UI.
//!   The page self-mints a game id and drives it via `/soccer/api/*`.
//! - `POST /soccer/game` — mint an id, start a game, return `{id}`.
//! - `GET /soccer/game?game=<id>` — game metadata / liveness.
//! - `GET /soccer/sim?game=<id>` — static/replay view of the game.
//! - `GET /soccer/inspect?game=<id>` — read-only engine internals for an external
//!   debugger (`&weights=1` embeds raw NN weights and is token-gated when set).
//! - `* /soccer/api/*?game=<id>` — the scoped live bridge API.
//! - `GET /healthz` — liveness for Kubernetes probes.
//!
//! Back-compat: the existing `dd-des-rs` server keeps its single-session
//! `/des-rs/soccer/live` etc.; this is the new *root* server for multi games.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header, HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::{
    config::apply_live_learning_env,
    database::{DatabaseReadiness, DatabaseState},
    docs, telemetry,
};

// `SoccerRealtimeSession` is the agnostic game core (also driven directly by the
// desktop client). `SoccerLiveHttpBridge` is HTTP glue that lives in the
// `soccer_engine` crate today; when that crate is made fully agnostic (its bridge
// moves behind the `web-bridge` feature / into the server layer) this server
// drives `SoccerRealtimeSession` directly so the engine carries no web deps.
use soccer_engine::soccer::{SoccerLiveHttpBridge, SoccerLiveServerConfig};

/// How long an idle game lingers before the reaper drops it.
const GAME_TTL: Duration = Duration::from_secs(60 * 30);

/// Upper bound on concurrently live games, so a flood of distinct ids (the live UI
/// mints one per first visit) cannot grow the registry without limit.
const MAX_GAMES: usize = 512;

/// One running game: its own live bridge plus bookkeeping for TTL eviction.
struct GameSession {
    bridge: Arc<SoccerLiveHttpBridge>,
    created: Instant,
}

#[derive(Clone)]
struct AppState {
    database: DatabaseState,
    games: Arc<Mutex<HashMap<String, Arc<GameSession>>>>,
    /// WebSocket fan-out for `/soccer/api/ws`: per-game broadcast + single-driver
    /// election, so spectators of one `?game=` receive frames pushed by the elected
    /// driver instead of each one independently POST-stepping over HTTP. The HTTP
    /// `/soccer/api/*` path is unchanged and remains the fallback when WS is
    /// unavailable. (Ported from dd-des-rs, adapted to this per-game registry.)
    soccer_live_ws: Arc<SoccerLiveWsHub>,
}

impl AppState {
    fn new(database: DatabaseState) -> Self {
        Self {
            database,
            games: Arc::new(Mutex::new(HashMap::new())),
            soccer_live_ws: Arc::new(SoccerLiveWsHub::default()),
        }
    }

    /// Look up an existing game without creating one (used by the inspect debugger,
    /// which attaches to a game that must already exist).
    fn lookup(&self, id: &str) -> Option<Arc<GameSession>> {
        self.games.lock().expect("games lock").get(id).cloned()
    }

    /// Get the game for `id`, creating (and seeding) it on first contact. This is
    /// how the live UI's client-minted ids become real games. Creation is bounded
    /// by [`MAX_GAMES`]: at capacity we first drop expired games, then the oldest.
    async fn get_or_create(&self, id: &str) -> Result<Arc<GameSession>, String> {
        if let Some(session) = self.lookup(id) {
            return Ok(session);
        }
        let id_for_task = id.to_string();
        let session = tokio::task::spawn_blocking(move || Arc::new(new_session(&id_for_task)))
            .await
            .map_err(|err| format!("create game session: {err}"))?;
        Ok(self.insert_created(id.to_string(), session))
    }

    fn insert_created(&self, id: String, session: Arc<GameSession>) -> Arc<GameSession> {
        let mut games = self.games.lock().expect("games lock");
        if let Some(session) = games.get(&id) {
            return session.clone();
        }
        if games.len() >= MAX_GAMES {
            games.retain(|_, session| session.created.elapsed() < GAME_TTL);
            if games.len() >= MAX_GAMES {
                if let Some(oldest) = games
                    .iter()
                    .min_by_key(|(_, session)| session.created)
                    .map(|(key, _)| key.clone())
                {
                    games.remove(&oldest);
                }
            }
        }
        games.insert(id, session.clone());
        session
    }

    /// Drop games idle longer than [`GAME_TTL`]; returns the count evicted.
    fn reap(&self) -> usize {
        let mut games = self.games.lock().expect("games lock");
        let before = games.len();
        games.retain(|_, session| session.created.elapsed() < GAME_TTL);
        before - games.len()
    }
}

/// Build a fresh game session for `id`, seeding the match from the id so two
/// distinct ids play out differently (mirrors the engine UI's per-id seeding).
fn new_session(id: &str) -> GameSession {
    let mut config = SoccerLiveServerConfig::default();
    apply_live_learning_env(&mut config);
    config.match_config.seed = seed_from_id(id);
    GameSession {
        bridge: Arc::new(SoccerLiveHttpBridge::new(config)),
        created: Instant::now(),
    }
}

/// FNV-1a hash of the game id → a stable per-game match seed.
fn seed_from_id(id: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in id.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Normalize a caller-supplied id to the live UI's rules: lowercase, keep only
/// `[a-z0-9-]`, cap at 64 chars. Returns `None` when nothing usable remains.
fn sanitize_game_id(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .take(64)
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

// ---------------------------------------------------------------------------
// Live soccer WebSocket: an OPTIONAL push channel layered over the SAME per-game
// bridges. The HTTP `/soccer/api/*` routes are untouched and remain the fallback
// (the UI auto-falls-back when the socket is unavailable). Goal: stop every viewer
// of one `?game=` from independently POST-stepping the shared session — which over
// a high-latency link means a fresh TLS handshake per frame. Exactly one connection
// per game is elected `driver` (it may "pull": send step RPCs that advance the sim);
// all others are `follower`s that only receive the driver's pushed frames. Messages
// are a thin RPC envelope so the socket reuses `bridge.handle_request` verbatim.
// Ported from dd-des-rs; adapted to resolve THIS server's per-game bridge.
// ---------------------------------------------------------------------------

/// One broadcast item delivered to every subscriber of a game. `src` is the
/// connection that produced it, so a connection can skip echoes of its own frames
/// (`u64::MAX` is a sentinel meaning "deliver to everyone").
#[derive(Clone)]
struct WsBroadcast {
    src: u64,
    text: String,
}

/// Per-game fan-out plus whether the server-side autonomous stepper is already
/// running for this game.
struct WsGameRoom {
    tx: broadcast::Sender<WsBroadcast>,
    /// True while a single autonomous stepper task is advancing this game and pushing
    /// frames. Server-authoritative playback: viewers receive pushed frames at the
    /// server's step rate instead of each pulling+awaiting a step (~1 RTT/frame), so a
    /// remote viewer stays smooth regardless of link latency.
    ticking: AtomicBool,
}

/// Registry of per-game rooms plus a monotonic connection-id source.
#[derive(Default)]
struct SoccerLiveWsHub {
    rooms: Mutex<HashMap<String, Arc<WsGameRoom>>>,
    next_conn_id: AtomicU64,
}

impl SoccerLiveWsHub {
    fn room(&self, game_id: &str) -> Arc<WsGameRoom> {
        let mut rooms = self.rooms.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(rooms.entry(game_id.to_string()).or_insert_with(|| {
            Arc::new(WsGameRoom {
                tx: broadcast::channel(64).0,
                ticking: AtomicBool::new(false),
            })
        }))
    }

    fn next_id(&self) -> u64 {
        self.next_conn_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Drop a room once nobody is subscribed, so a churn of distinct `?game=` ids
    /// cannot grow the map without bound.
    fn retire_if_idle(&self, game_id: &str, room: &Arc<WsGameRoom>) {
        if room.tx.receiver_count() == 0 {
            let mut rooms = self.rooms.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = rooms.get(game_id) {
                if Arc::ptr_eq(existing, room) && existing.tx.receiver_count() == 0 {
                    rooms.remove(game_id);
                }
            }
        }
    }
}

/// Mirror the engine's `?game=` sanitizer: lowercase, `[a-z0-9-]`, max 64.
fn sanitize_ws_game_id(raw: &str) -> String {
    sanitize_game_id(raw).unwrap_or_else(|| "default".to_string())
}

fn ws_game_id_from_uri(uri: &Uri) -> String {
    let raw = uri.query().and_then(|q| {
        q.split('&').find_map(|kv| {
            let mut it = kv.splitn(2, '=');
            match it.next() {
                Some("game") => it.next(),
                _ => None,
            }
        })
    });
    sanitize_ws_game_id(raw.unwrap_or(""))
}

fn ws_hello(driver: bool) -> String {
    json!({ "t": "hello", "driver": driver, "protocol": 1 }).to_string()
}

fn ws_reply(id: &Value, status: u16, body: &str) -> String {
    json!({ "t": "reply", "id": id, "status": status, "body": body }).to_string()
}

/// `GET /soccer/api/ws?game=<id>` — upgrade to the live soccer push socket. Coexists
/// with the `/soccer/api/*rest` HTTP catch-all (axum matches this exact path first).
async fn soccer_live_ws(State(state): State<AppState>, uri: Uri, ws: WebSocketUpgrade) -> Response {
    let game_id = ws_game_id_from_uri(&uri);
    ws.on_upgrade(move |socket| soccer_live_ws_session(socket, state, game_id))
}

/// Drive one upgraded connection: pump client RPCs into this game's bridge and pushed
/// frames out to the socket, until either side closes.
async fn soccer_live_ws_session(mut socket: WebSocket, state: AppState, game_id: String) {
    let hub = Arc::clone(&state.soccer_live_ws);
    let room = hub.room(&game_id);
    let conn_id = hub.next_id();
    let mut rx = room.tx.subscribe();
    // Server-authoritative playback: every viewer is a follower that renders pushed
    // frames; the server advances the sim. So we never hand out the driver role (which
    // would make a client pull+await each step over the link — the latency-bound path).
    let is_driver = false;

    if socket
        .send(Message::Text(ws_hello(is_driver)))
        .await
        .is_err()
    {
        drop(rx);
        hub.retire_if_idle(&game_id, &room);
        return;
    }

    // Ensure exactly one autonomous stepper is running for this game, now that it has a
    // viewer. It steps + pushes frames until the last viewer leaves.
    soccer_live_ws_spawn_ticker(state.clone(), game_id.clone(), Arc::clone(&room));

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let msg = match incoming {
                    Some(Ok(msg)) => msg,
                    _ => break,
                };
                match msg {
                    Message::Text(text) => {
                        if let Some(out) =
                            soccer_live_ws_handle_text(&state, &game_id, &room, conn_id, &text).await
                        {
                            if socket.send(Message::Text(out)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Message::Ping(payload)
                        if socket.send(Message::Pong(payload.clone())).await.is_err() => break,
                    Message::Ping(_) => {}
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            pushed = rx.recv() => {
                match pushed {
                    Ok(item) if item.src == conn_id => {}
                    Ok(item) => {
                        if socket.send(Message::Text(item.text)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    drop(rx);
    hub.retire_if_idle(&game_id, &room);
}

/// Handle one inbound text frame; returns an optional direct reply for the sender.
/// Step RPCs additionally broadcast their resulting frame to followers. The bridge is
/// resolved per-RPC from THIS game's session (mirroring the HTTP `game_api` path), so
/// the socket drives exactly the same per-game state the HTTP fallback does.
async fn soccer_live_ws_handle_text(
    state: &AppState,
    game_id: &str,
    room: &Arc<WsGameRoom>,
    conn_id: u64,
    text: &str,
) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    match value.get("t").and_then(Value::as_str)? {
        "rpc" => {
            let id = value.get("id").cloned().unwrap_or(Value::Null);
            let method = value
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("GET")
                .to_string();
            let path = value
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("/")
                .to_string();
            let body = value
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            // A "step" advances the shared session, so only the driver may issue it;
            // followers stand down and follow the pushed frames.
            let is_step = path.contains("/api/step");
            if is_step {
                return Some(ws_reply(&id, 409, "{\"error\":\"not driver\"}"));
            }
            // Resolve this game's bridge (per-game registry), exactly as the HTTP path
            // does — the WS RPC sends the bare `/api/*` path the bridge speaks.
            let session = match state.get_or_create(game_id).await {
                Ok(session) => session,
                Err(error) => {
                    let body = json!({ "error": error }).to_string();
                    return Some(ws_reply(&id, 500, &body));
                }
            };
            let bridge = Arc::clone(&session.bridge);
            let reply =
                tokio::task::spawn_blocking(move || bridge.handle_request(&method, &path, &body))
                    .await;
            let reply = match reply {
                Ok(reply) => reply,
                Err(_) => return Some(ws_reply(&id, 500, "{\"error\":\"bridge panicked\"}")),
            };
            if is_step && reply.status < 400 {
                let push = json!({ "t": "push", "body": reply.body }).to_string();
                let _ = room.tx.send(WsBroadcast {
                    src: conn_id,
                    text: push,
                });
            }
            Some(ws_reply(&id, reply.status, &reply.body))
        }
        // Server-authoritative: there is no client driver to claim — the server steps
        // the game itself. Always answer "follower" so the client renders pushed frames.
        "claim" => Some(ws_hello(false)),
        _ => None,
    }
}

/// Start the single autonomous stepper for `game_id` if one is not already running.
/// While at least one viewer is subscribed to the room, it advances that game's sim and
/// pushes each frame to every viewer at (best-effort) the 1/15s cadence — so frame
/// delivery is paced by the server, never by a client round-trip. Exits (and clears the
/// flag) once the last viewer disconnects, so idle games stop consuming CPU.
fn soccer_live_ws_spawn_ticker(state: AppState, game_id: String, room: Arc<WsGameRoom>) {
    // `swap(true)` returning true means a ticker is already running for this game.
    if room.ticking.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        let frame_budget = Duration::from_millis(66);
        loop {
            if room.tx.receiver_count() == 0 {
                break;
            }
            let started = Instant::now();
            let Ok(session) = state.get_or_create(&game_id).await else {
                tokio::task::yield_now().await;
                continue;
            };
            let bridge = Arc::clone(&session.bridge);
            let stepped = tokio::task::spawn_blocking(move || {
                bridge.handle_request("POST", "/api/step", "{}")
            })
            .await;
            if let Ok(reply) = stepped {
                if reply.status < 400 {
                    // src = u64::MAX → delivered to every viewer (no self-echo skip).
                    let push = json!({ "t": "push", "body": reply.body }).to_string();
                    let _ = room.tx.send(WsBroadcast {
                        src: u64::MAX,
                        text: push,
                    });
                }
            }
            let elapsed = started.elapsed();
            if elapsed < frame_budget {
                tokio::time::sleep(frame_budget - elapsed).await;
            } else {
                // Stepping is slower than real-time for this game; yield so other tasks
                // (new connections, other games) still make progress.
                tokio::task::yield_now().await;
            }
        }
        room.ticking.store(false, Ordering::SeqCst);
    });
}

#[derive(Deserialize)]
struct GameQuery {
    // The engine UI carries the game id as `?game=`; API/doc clients may use `?id=`.
    // Either is accepted; `game` wins when both are present.
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    game: Option<String>,
}

impl GameQuery {
    fn game_id(&self) -> Option<String> {
        self.game
            .as_deref()
            .or(self.id.as_deref())
            .and_then(sanitize_game_id)
    }
}

#[derive(Serialize)]
struct CreatedGame {
    id: String,
}

/// `POST /soccer/game` — create a fresh game and return its id.
async fn create_game(State(state): State<AppState>) -> Response {
    let id = Uuid::new_v4().to_string();
    // Eagerly create (and seed) it so a subsequent `?game=<id>` finds it ready.
    match state.get_or_create(&id).await {
        Ok(_) => (StatusCode::CREATED, Json(CreatedGame { id })).into_response(),
        Err(err) => session_create_error(err),
    }
}

/// `GET /soccer/game?game=<id>` — game liveness metadata (delegates to the
/// bridge's state endpoint). Creates the game lazily if it does not yet exist.
async fn game_meta(State(state): State<AppState>, Query(q): Query<GameQuery>) -> Response {
    match resolve_or_create(&state, &q).await {
        Ok(session) => bridge_reply(session.bridge.handle_request("GET", "/api/state", "")),
        Err(resp) => resp,
    }
}

/// `* /soccer/api/*?game=<id>` — proxy any live-bridge request to the game's bridge.
async fn game_api(
    State(state): State<AppState>,
    Query(q): Query<GameQuery>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Response {
    let session = match resolve_or_create(&state, &q).await {
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

/// `GET /soccer/sim?game=<id>` — static/replay view of a game.
async fn sim_view(State(state): State<AppState>, Query(q): Query<GameQuery>) -> Response {
    match resolve_or_create(&state, &q).await {
        // The bridge renders the current frame; a fuller replay reads the game's
        // persisted playback artifacts (follow-up).
        Ok(session) => bridge_reply(session.bridge.handle_request("GET", "/api/state", "")),
        Err(resp) => resp,
    }
}

#[derive(Deserialize)]
struct InspectQuery {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    game: Option<String>,
    /// `weights=1` additionally embeds the raw neural-network snapshot (large).
    #[serde(default)]
    weights: Option<u8>,
    /// Token may be supplied here instead of an `Authorization: Bearer` header.
    #[serde(default)]
    token: Option<String>,
}

impl InspectQuery {
    fn game_id(&self) -> Option<String> {
        self.game
            .as_deref()
            .or(self.id.as_deref())
            .and_then(sanitize_game_id)
    }
}

/// `GET /soccer/inspect?game=<id>[&weights=1]` — read-only dump of the game's FULL
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
    // Inspect attaches to a game that must already exist; it does not create one.
    let session = match q.game_id() {
        Some(id) => match state.lookup(&id) {
            Some(session) => session,
            None => return (StatusCode::NOT_FOUND, "no such game").into_response(),
        },
        None => return (StatusCode::BAD_REQUEST, "missing ?game=<id>").into_response(),
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

async fn readyz(State(state): State<AppState>) -> (StatusCode, &'static str) {
    match state.database.readiness().await {
        DatabaseReadiness::Disabled => (StatusCode::OK, "database disabled"),
        DatabaseReadiness::Connected => (StatusCode::OK, "ready"),
        DatabaseReadiness::Unreachable => (StatusCode::SERVICE_UNAVAILABLE, "database unreachable"),
    }
}

/// Resolve a `?game=`/`?id=` to a live session, creating it on first contact, or
/// return a 400 when no usable id was supplied.
async fn resolve_or_create(state: &AppState, q: &GameQuery) -> Result<Arc<GameSession>, Response> {
    let id = q
        .game_id()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing ?game=<id>").into_response())?;
    state.get_or_create(&id).await.map_err(session_create_error)
}

fn session_create_error(err: String) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, err).into_response()
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

fn env_string(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) async fn run() {
    let _telemetry = telemetry::init();
    tracing::info!(
        event = "akrion_process_start",
        service_name = "dd-soccer-rs",
        cluster = env_string("AKRION_CLUSTER_NAME")
            .or_else(|| env_string("SOCCER_CLUSTER_NAME"))
            .unwrap_or_else(|| "local".to_string()),
        pod = env_string("HOSTNAME").unwrap_or_else(|| "local".to_string())
    );

    let database = DatabaseState::from_env().await;
    let state = AppState::new(database);

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
        .route("/readyz", get(readyz))
        // Docs live under /soccer/* so the existing gateway prefix (which proxies
        // /soccer/ to this server) reaches them with no gateway change. A bare
        // /docs 302s to the canonical path for convenience.
        .route(
            "/docs",
            get(|| async { axum::response::Redirect::permanent("/soccer/docs") }),
        )
        .route("/soccer/docs", get(docs::index))
        .route("/soccer/docs/:slug", get(docs::page))
        // The full live UI. `/`, `/soccer`, and `/soccer/` are the landing pages;
        // `/soccer/live` is the canonical route the engine page's mount logic keys
        // off. All four serve the same UI.
        .route("/", get(docs::live_ui))
        .route("/soccer", get(docs::live_ui))
        .route("/soccer/", get(docs::live_ui))
        .route("/soccer/live", get(docs::live_ui))
        .route("/soccer/game", post(create_game).get(game_meta))
        .route("/soccer/sim", get(sim_view))
        .route("/soccer/inspect", get(inspect_game))
        // WebSocket push transport — must precede the `/soccer/api/*rest` catch-all so
        // the upgrade reaches the WS handler instead of the HTTP bridge (which can't
        // upgrade, so the client would fall back to per-frame HTTP polling).
        .route("/soccer/api/ws", get(soccer_live_ws))
        .route("/soccer/api/*rest", any(game_api))
        .layer(axum::middleware::from_fn(telemetry::record_http_metrics))
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
    tracing::info!(event = "akrion_listening", addr = %addr);
    axum::serve(listener, app)
        .await
        .expect("dd-soccer-rs serve");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_matches_ui_rules() {
        assert_eq!(sanitize_game_id("AB_c-12!").as_deref(), Some("abc-12"));
        assert_eq!(sanitize_game_id("***"), None);
        assert_eq!(sanitize_game_id(""), None);
        assert_eq!(sanitize_game_id(&"a".repeat(100)).unwrap().len(), 64);
        // A v4 uuid (what the UI and POST /soccer/game mint) survives unchanged.
        let uuid = uuid::Uuid::new_v4().to_string();
        assert_eq!(sanitize_game_id(&uuid).as_deref(), Some(uuid.as_str()));
    }

    #[test]
    fn game_query_prefers_game_then_id() {
        let both = GameQuery {
            id: Some("aaa".into()),
            game: Some("bbb".into()),
        };
        assert_eq!(both.game_id().as_deref(), Some("bbb"));
        let id_only = GameQuery {
            id: Some("ccc".into()),
            game: None,
        };
        assert_eq!(id_only.game_id().as_deref(), Some("ccc"));
        let neither = GameQuery {
            id: None,
            game: None,
        };
        assert_eq!(neither.game_id(), None);
    }

    #[test]
    fn distinct_ids_get_distinct_seeds() {
        assert_ne!(seed_from_id("game-a"), seed_from_id("game-b"));
        assert_eq!(seed_from_id("stable"), seed_from_id("stable"));
    }

    #[tokio::test]
    async fn get_or_create_is_idempotent_and_lazy() {
        let state = AppState::new(DatabaseState::default());
        assert!(state.lookup("abc").is_none());
        let first = state.get_or_create("abc").await.unwrap();
        let second = state.get_or_create("abc").await.unwrap();
        assert!(Arc::ptr_eq(&first, &second), "same id reuses the session");
        assert!(state.lookup("abc").is_some());
    }

    #[tokio::test]
    async fn readiness_is_healthy_when_database_is_disabled() {
        let state = AppState::new(DatabaseState::default());
        assert_eq!(
            readyz(State(state)).await,
            (StatusCode::OK, "database disabled")
        );
    }
}
