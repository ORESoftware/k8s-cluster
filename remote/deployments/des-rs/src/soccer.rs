use std::{
    collections::HashMap,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex as StdMutex,
    },
};

use axum::{
    body::Bytes,
    extract::{
        rejection::JsonRejection,
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{header, HeaderValue, Method, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use tokio::sync::broadcast;

use soccer_engine::soccer::SoccerLiveHttpReply;
use soccer_engine::soccer_planner::{planner_response_to_json, solve_planner, PlannerRequest};

use crate::models::run_streaming_model;
use crate::state::{json_error, AppState};

const SOCCER_PLANNER_HTTP_SOLVE_BUDGET_MS: f64 = 90_000.0;

/// `GET /soccer/planner` — interactive 11-a-side rotation planner UI.
pub(crate) async fn soccer_planner_page(State(state): State<AppState>) -> Html<String> {
    Html(state.soccer_planner_html.to_string())
}

/// `POST /soccer/planner/solve` — re-solve with roster/constraints from the UI.
pub(crate) async fn soccer_planner_solve(
    State(state): State<AppState>,
    request: Result<Json<PlannerRequest>, JsonRejection>,
) -> Response {
    let Json(mut req) = match request {
        Ok(req) => req,
        Err(err) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                format!("invalid soccer planner request JSON: {err}"),
            );
        }
    };
    let requested_solver_time_limit_ms = req.solver_time_limit_ms;
    let solver_time_was_capped = requested_solver_time_limit_ms.is_finite()
        && requested_solver_time_limit_ms > SOCCER_PLANNER_HTTP_SOLVE_BUDGET_MS;
    if solver_time_was_capped {
        req.solver_time_limit_ms = SOCCER_PLANNER_HTTP_SOLVE_BUDGET_MS;
    }

    let _guard = state.sim_lock.lock().await;
    let result =
        tokio::task::spawn_blocking(move || catch_unwind(AssertUnwindSafe(|| solve_planner(&req))))
            .await;
    let mut resp = match result {
        Ok(Ok(r)) => r,
        Ok(Err(panic_payload)) => {
            let error = panic_payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic_payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "soccer planner solve panicked".to_string());
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("soccer planner solve panicked: {error}"),
            );
        }
        Err(e) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("soccer planner solve task failed: {e}"),
            );
        }
    };
    if solver_time_was_capped {
        resp.solver_notes.push(format!(
            "Server capped solverTimeLimitMs from {:.0}ms to {:.0}ms so the HTTP endpoint returns JSON before the gateway timeout.",
            requested_solver_time_limit_ms, SOCCER_PLANNER_HTTP_SOLVE_BUDGET_MS
        ));
    }
    let status = if resp.ok {
        StatusCode::OK
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    };
    (status, Json(planner_response_to_json(&resp))).into_response()
}

/// `POST /soccer/planner/stream` — planner-specific alias for the generic
/// `streaming/soccer-planner` JSONL endpoint.
pub(crate) async fn soccer_planner_stream(State(state): State<AppState>, body: String) -> Response {
    run_streaming_model(state, "soccer-planner".to_string(), body).await
}

/// This web server's own release identity, in the same `BuildInfo` shape the
/// engines use (`build.rs` bakes in the `DD_DES_RS_*` values at compile time).
fn web_server_build_info() -> des_engine::BuildInfo {
    des_engine::BuildInfo::new(
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        env!("DD_DES_RS_GIT_COMMIT"),
        env!("DD_DES_RS_GIT_COMMIT_DATE"),
        env!("DD_DES_RS_BUILD_TIMESTAMP"),
    )
}

/// Full running-stack release identity: this web server plus the soccer + des
/// engines it embeds. Same payload `GET /api/build` returns (web layer merged
/// in), reused by `GET /info`.
pub(crate) fn full_stack_build_json() -> Value {
    // soccer_engine no longer exposes `live_build_info_json()` on `main` — the
    // build_info module + build.rs were dropped in a parallel-history sync. Report
    // this web server's release identity here; the soccer + des engine layers are
    // still surfaced by the soccer bridge's own `GET /api/build`
    // (merge_web_server_build_layer folds the web layer into that reply).
    serde_json::json!({ "web_server": web_server_build_info() })
}

/// The soccer bridge's `GET /api/build` returns the soccer + des engine layers
/// (all it can see from inside the engine). As the host, fold in our own
/// `web_server` layer so the live UI shows the full running stack.
fn merge_web_server_build_layer(reply: SoccerLiveHttpReply) -> SoccerLiveHttpReply {
    if reply.status != 200 || !reply.content_type.starts_with("application/json") {
        return reply;
    }
    let mut value: Value = match serde_json::from_str(&reply.body) {
        Ok(value @ Value::Object(_)) => value,
        _ => return reply,
    };
    if let (Value::Object(map), Ok(ws)) =
        (&mut value, serde_json::to_value(web_server_build_info()))
    {
        map.insert("web_server".to_string(), ws);
    }
    match serde_json::to_string(&value) {
        Ok(body) => SoccerLiveHttpReply { body, ..reply },
        Err(_) => reply,
    }
}

fn soccer_live_reply_response(reply: SoccerLiveHttpReply) -> Response {
    let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = (status, reply.body).into_response();
    let content_type = HeaderValue::from_str(&reply.content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("text/plain; charset=utf-8"));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    response
}

pub(crate) async fn soccer_live_bridge_request(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Response {
    let body = String::from_utf8_lossy(&body).into_owned();
    let path = uri.path();
    let reply = state
        .soccer_live_bridge
        .handle_request(method.as_str(), path, &body);
    let reply = if method == Method::GET
        && (path.ends_with("/api/build") || path.ends_with("/api/version"))
    {
        merge_web_server_build_layer(reply)
    } else {
        reply
    };
    soccer_live_reply_response(reply)
}

// ---------------------------------------------------------------------------
// Live soccer WebSocket: an OPTIONAL push channel layered over the SAME
// `soccer_live_bridge`. The HTTP `/api/*` routes above are untouched and remain
// the fallback. Goal: stop every viewer of one `?game=` from independently
// POST-stepping the shared session. Exactly one connection per game is elected
// `driver` (it may "pull": send RPCs that advance the sim); all others are
// `follower`s that only receive the driver's frames pushed to them. Messages
// are a thin RPC envelope so the socket reuses `bridge.handle_request` verbatim.
// ---------------------------------------------------------------------------

/// One broadcast item delivered to every subscriber of a game. `src` is the
/// connection that produced it, so a connection can skip echoes of its own
/// frames (`u64::MAX` is a sentinel meaning "deliver to everyone").
#[derive(Clone)]
struct WsBroadcast {
    src: u64,
    text: String,
}

/// Per-game fan-out: a broadcast channel plus the elected driver connection id.
struct WsGameRoom {
    tx: broadcast::Sender<WsBroadcast>,
    driver: StdMutex<Option<u64>>,
}

impl WsGameRoom {
    /// Claim the driver slot if it is free (or already ours). Returns whether
    /// this connection is the driver afterwards.
    fn try_claim(&self, conn_id: u64) -> bool {
        let mut driver = self.driver.lock().unwrap_or_else(|e| e.into_inner());
        match *driver {
            None => {
                *driver = Some(conn_id);
                true
            }
            Some(current) => current == conn_id,
        }
    }

    fn is_driver(&self, conn_id: u64) -> bool {
        *self.driver.lock().unwrap_or_else(|e| e.into_inner()) == Some(conn_id)
    }

    /// Release the driver slot if held by `conn_id`. Returns whether we held it.
    fn release(&self, conn_id: u64) -> bool {
        let mut driver = self.driver.lock().unwrap_or_else(|e| e.into_inner());
        if *driver == Some(conn_id) {
            *driver = None;
            true
        } else {
            false
        }
    }
}

/// Registry of per-game rooms plus a monotonic connection-id source.
#[derive(Default)]
pub(crate) struct SoccerLiveWsHub {
    rooms: StdMutex<HashMap<String, Arc<WsGameRoom>>>,
    next_conn_id: AtomicU64,
}

impl SoccerLiveWsHub {
    fn room(&self, game_id: &str) -> Arc<WsGameRoom> {
        let mut rooms = self.rooms.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(rooms.entry(game_id.to_string()).or_insert_with(|| {
            Arc::new(WsGameRoom {
                tx: broadcast::channel(64).0,
                driver: StdMutex::new(None),
            })
        }))
    }

    fn next_id(&self) -> u64 {
        self.next_conn_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Drop a room once nobody is subscribed, so a churn of distinct `?game=`
    /// ids cannot grow the map without bound.
    fn retire_if_idle(&self, game_id: &str, room: &Arc<WsGameRoom>) {
        if room.tx.receiver_count() == 0 {
            let mut rooms = self.rooms.lock().unwrap_or_else(|e| e.into_inner());
            // Re-check under the lock and confirm it is still the same room with
            // no late subscriber, so we never evict a room a new client just took.
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
    let cleaned: String = raw
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .take(64)
        .collect();
    if cleaned.is_empty() {
        "default".to_string()
    } else {
        cleaned
    }
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

/// `GET /api/ws?game=<id>` — upgrade to the live soccer push socket. Coexists
/// with the `/api/*path` catch-all exactly as `/api/docs` already does.
pub(crate) async fn soccer_live_ws(
    State(state): State<AppState>,
    uri: Uri,
    ws: WebSocketUpgrade,
) -> Response {
    let game_id = ws_game_id_from_uri(&uri);
    ws.on_upgrade(move |socket| soccer_live_ws_session(socket, state, game_id))
}

fn ws_hello(driver: bool) -> String {
    json!({ "t": "hello", "driver": driver, "protocol": 1 }).to_string()
}

fn ws_reply(id: &Value, status: u16, body: &str) -> String {
    json!({ "t": "reply", "id": id, "status": status, "body": body }).to_string()
}

/// Drive one upgraded connection: pump client RPCs into the bridge and pushed
/// frames out to the socket, until either side closes.
async fn soccer_live_ws_session(mut socket: WebSocket, state: AppState, game_id: String) {
    let hub = Arc::clone(&state.soccer_live_ws);
    let room = hub.room(&game_id);
    let conn_id = hub.next_id();
    let mut rx = room.tx.subscribe();
    let is_driver = room.try_claim(conn_id);

    if socket
        .send(Message::Text(ws_hello(is_driver)))
        .await
        .is_err()
    {
        room.release(conn_id);
        drop(rx);
        hub.retire_if_idle(&game_id, &room);
        return;
    }

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
                            soccer_live_ws_handle_text(&state, &room, conn_id, &text).await
                        {
                            if socket.send(Message::Text(out)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Message::Ping(payload) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            pushed = rx.recv() => {
                match pushed {
                    // Skip echoes of our own frames; the driver already saw them
                    // as the direct reply to its own step RPC.
                    Ok(item) if item.src == conn_id => {}
                    Ok(item) => {
                        if socket.send(Message::Text(item.text)).await.is_err() {
                            break;
                        }
                    }
                    // We fell behind the driver; the next frame supersedes the
                    // dropped ones, so just keep going (latest-wins for frames).
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    // On the driver leaving, open the slot and tell followers so one of them can
    // promote itself (it sends `{"t":"claim"}`).
    if room.release(conn_id) {
        let _ = room.tx.send(WsBroadcast {
            src: u64::MAX,
            text: json!({ "t": "driver-open" }).to_string(),
        });
    }
    drop(rx);
    hub.retire_if_idle(&game_id, &room);
}

/// Handle one inbound text frame; returns an optional direct reply for the
/// sender. Step RPCs additionally broadcast their resulting frame to followers.
async fn soccer_live_ws_handle_text(
    state: &AppState,
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
            // A "step" advances the shared session, so only the driver may issue
            // it; followers are told to stand down and follow the pushed frames.
            let is_step = path.contains("/api/step");
            if is_step && !room.is_driver(conn_id) {
                return Some(ws_reply(&id, 409, "{\"error\":\"not driver\"}"));
            }
            let bridge = Arc::clone(&state.soccer_live_bridge);
            let reply = tokio::task::spawn_blocking(move || {
                bridge.handle_request(&method, &path, &body)
            })
            .await;
            let reply = match reply {
                Ok(reply) => reply,
                Err(_) => return Some(ws_reply(&id, 500, "{\"error\":\"bridge panicked\"}")),
            };
            if is_step && reply.status < 400 {
                // Fan the fresh frame out to every follower of this game.
                let push = json!({ "t": "push", "body": reply.body }).to_string();
                let _ = room.tx.send(WsBroadcast {
                    src: conn_id,
                    text: push,
                });
            }
            Some(ws_reply(&id, reply.status, &reply.body))
        }
        // A follower bidding for the (now empty) driver slot.
        "claim" => Some(ws_hello(room.try_claim(conn_id))),
        _ => None,
    }
}
