use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dd_nats_subject_defs::RUNTIME_EVENTS_SUBJECT;
use futures_util::StreamExt;
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounter, IntCounterVec, IntGauge, Opts, TextEncoder};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, RwLock};

static STARTED_AT: Lazy<Instant> = Lazy::new(Instant::now);
static PEER_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static HTTP_REQUESTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "dd_webrtc_http_requests_total",
            "HTTP requests observed by the WebRTC signaling service.",
        ),
        &["method", "path", "status"],
    )
    .expect("failed to create dd_webrtc_http_requests_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("failed to register dd_webrtc_http_requests_total");
    counter
});
static WS_CONNECTIONS_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    let counter = IntCounter::new(
        "dd_webrtc_ws_connections_total",
        "Accepted WebRTC signaling websocket connections.",
    )
    .expect("failed to create dd_webrtc_ws_connections_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("failed to register dd_webrtc_ws_connections_total");
    counter
});
static ACTIVE_CONNECTIONS: Lazy<IntGauge> = Lazy::new(|| {
    let gauge = IntGauge::new(
        "dd_webrtc_active_connections",
        "Currently connected WebRTC signaling peers.",
    )
    .expect("failed to create dd_webrtc_active_connections");
    prometheus::default_registry()
        .register(Box::new(gauge.clone()))
        .expect("failed to register dd_webrtc_active_connections");
    gauge
});
static ACTIVE_ROOMS: Lazy<IntGauge> = Lazy::new(|| {
    let gauge = IntGauge::new(
        "dd_webrtc_active_rooms",
        "Currently active signaling rooms.",
    )
    .expect("failed to create dd_webrtc_active_rooms");
    prometheus::default_registry()
        .register(Box::new(gauge.clone()))
        .expect("failed to register dd_webrtc_active_rooms");
    gauge
});
static SIGNAL_MESSAGES: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "dd_webrtc_signal_messages_total",
            "WebRTC signaling frames handled by the signaling service.",
        ),
        &["message_type", "delivery"],
    )
    .expect("failed to create dd_webrtc_signal_messages_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("failed to register dd_webrtc_signal_messages_total");
    counter
});
static SIGNAL_ERRORS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "dd_webrtc_signal_errors_total",
            "WebRTC signaling frame errors observed by the signaling service.",
        ),
        &["reason"],
    )
    .expect("failed to create dd_webrtc_signal_errors_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("failed to register dd_webrtc_signal_errors_total");
    counter
});

#[derive(Clone)]
struct AppState {
    rooms: Arc<RwLock<HashMap<String, RoomState>>>,
    admin_runtime_clients: Arc<RwLock<HashMap<u64, mpsc::UnboundedSender<Message>>>>,
}

#[derive(Clone, Default)]
struct RoomState {
    peers: HashMap<String, PeerConnection>,
}

#[derive(Clone)]
struct PeerConnection {
    info: PeerSummary,
    tx: mpsc::UnboundedSender<Message>,
}

#[derive(Clone, Serialize)]
struct PeerSummary {
    #[serde(rename = "peerId")]
    peer_id: String,
    #[serde(rename = "connectedAtMs")]
    connected_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none", rename = "userAgent")]
    user_agent: Option<String>,
    metadata: Value,
}

#[derive(Deserialize)]
struct SignalQuery {
    room: Option<String>,
    peer: Option<String>,
}

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
    service: String,
    mode: String,
    uptime_seconds: u64,
    active_rooms: i64,
    active_connections: i64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis() as u64
}

fn record_http(method: &str, path: &str, status: StatusCode) {
    HTTP_REQUESTS
        .with_label_values(&["GET", path, status.as_str()])
        .inc();
    if method != "GET" {
        HTTP_REQUESTS
            .with_label_values(&[method, path, status.as_str()])
            .inc();
    }
}

fn env_string(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn nats_url() -> String {
    env_string("NATS_URL")
        .unwrap_or_else(|| "nats://dd-nats.messaging.svc.cluster.local:4222".to_string())
}

fn runtime_admin_subject() -> String {
    env_string("RUNTIME_ADMIN_EVENT_SUBJECT").unwrap_or_else(|| RUNTIME_EVENTS_SUBJECT.to_string())
}

fn runtime_broadcast_secret() -> Option<String> {
    env_string("RUNTIME_BROADCAST_SECRET")
        .or_else(|| env_string("REMOTE_DEV_SERVER_SECRET"))
        .or_else(|| env_string("SERVER_AUTH_SECRET"))
}

fn authorized_admin_ws(headers: &HeaderMap) -> bool {
    headers
        .get("x-dd-admin")
        .and_then(|value| value.to_str().ok())
        == Some("1")
}

fn authorized_runtime_broadcast(headers: &HeaderMap) -> bool {
    let Some(secret) = runtime_broadcast_secret() else {
        return false;
    };
    ["x-server-auth", "x-dd-internal-auth", "x-agent-auth"]
        .iter()
        .any(|header_name| {
            headers
                .get(*header_name)
                .and_then(|value| value.to_str().ok())
                == Some(secret.as_str())
        })
}

fn normalize_id(value: Option<String>, fallback_prefix: &str) -> Result<String, String> {
    let value = value.unwrap_or_else(|| {
        let sequence = PEER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        format!("{fallback_prefix}-{sequence}")
    });
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{fallback_prefix} id is empty"));
    }
    if trimmed.len() > 96 {
        return Err(format!("{fallback_prefix} id is too long"));
    }
    if !trimmed
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        return Err(format!(
            "{fallback_prefix} id can only contain letters, numbers, dash, underscore, and dot"
        ));
    }
    Ok(trimmed.to_string())
}

fn json_text(value: Value) -> Message {
    Message::Text(value.to_string())
}

fn send_json(tx: &mpsc::UnboundedSender<Message>, value: Value) {
    let _ = tx.send(json_text(value));
}

fn should_forward_admin_runtime_message(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .is_some_and(|message_type| {
            matches!(message_type.as_str(), "k8s-runtime-event" | "task-event")
        })
}

async fn broadcast_to_admin_runtime(state: &AppState, payload: Value) -> usize {
    let text = payload.to_string();
    let targets = {
        let clients = state.admin_runtime_clients.read().await;
        clients
            .iter()
            .map(|(client_id, tx)| (*client_id, tx.clone()))
            .collect::<Vec<_>>()
    };
    let mut sent = 0;
    let mut closed = Vec::new();
    for (client_id, tx) in targets {
        match tx.send(Message::Text(text.clone())) {
            Ok(()) => sent += 1,
            Err(_) => closed.push(client_id),
        }
    }
    if !closed.is_empty() {
        let mut clients = state.admin_runtime_clients.write().await;
        for client_id in closed {
            clients.remove(&client_id);
        }
    }
    sent
}

async fn set_room_gauge(state: &AppState) {
    let rooms = state.rooms.read().await;
    ACTIVE_ROOMS.set(rooms.len() as i64);
}

async fn broadcast_to_room(
    state: &AppState,
    room_id: &str,
    except_peer_id: Option<&str>,
    value: Value,
) -> usize {
    let targets = {
        let rooms = state.rooms.read().await;
        rooms
            .get(room_id)
            .map(|room| {
                room.peers
                    .iter()
                    .filter_map(|(peer_id, peer)| {
                        if except_peer_id == Some(peer_id.as_str()) {
                            None
                        } else {
                            Some(peer.tx.clone())
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let mut sent = 0;
    for tx in targets {
        if tx.send(json_text(value.clone())).is_ok() {
            sent += 1;
        }
    }
    sent
}

async fn send_to_peer(state: &AppState, room_id: &str, peer_id: &str, value: Value) -> bool {
    let tx = {
        let rooms = state.rooms.read().await;
        rooms
            .get(room_id)
            .and_then(|room| room.peers.get(peer_id))
            .map(|peer| peer.tx.clone())
    };
    match tx {
        Some(tx) => tx.send(json_text(value)).is_ok(),
        None => false,
    }
}

async fn update_peer_metadata(state: &AppState, room_id: &str, peer_id: &str, metadata: Value) {
    let mut rooms = state.rooms.write().await;
    if let Some(room) = rooms.get_mut(room_id) {
        if let Some(peer) = room.peers.get_mut(peer_id) {
            peer.info.metadata = metadata;
        }
    }
}

async fn remove_peer(state: &AppState, room_id: &str, peer_id: &str) {
    let mut room_is_empty = false;
    {
        let mut rooms = state.rooms.write().await;
        if let Some(room) = rooms.get_mut(room_id) {
            room.peers.remove(peer_id);
            room_is_empty = room.peers.is_empty();
        }
        if room_is_empty {
            rooms.remove(room_id);
        }
        ACTIVE_ROOMS.set(rooms.len() as i64);
    }
    ACTIVE_CONNECTIONS.dec();
    let left_frame = json!({
        "type": "peer-left",
        "room": room_id,
        "peerId": peer_id,
        "atMs": now_ms(),
    });
    let _ = broadcast_to_room(state, room_id, Some(peer_id), left_frame).await;
}

async fn handle_signal_text(
    state: &AppState,
    room_id: &str,
    peer_id: &str,
    own_tx: &mpsc::UnboundedSender<Message>,
    text: String,
) -> bool {
    let parsed = match serde_json::from_str::<Value>(&text) {
        Ok(parsed) => parsed,
        Err(error) => {
            SIGNAL_ERRORS.with_label_values(&["invalid_json"]).inc();
            send_json(
                own_tx,
                json!({
                    "type": "error",
                    "code": "invalid_json",
                    "message": error.to_string(),
                    "atMs": now_ms(),
                }),
            );
            return true;
        }
    };

    let message_type = parsed
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("message")
        .to_string();
    let metadata = parsed.get("metadata").cloned().unwrap_or(Value::Null);

    match message_type.as_str() {
        "ping" => {
            SIGNAL_MESSAGES.with_label_values(&["ping", "local"]).inc();
            send_json(
                own_tx,
                json!({
                    "type": "pong",
                    "room": room_id,
                    "peerId": peer_id,
                    "atMs": now_ms(),
                }),
            );
            true
        }
        "hello" => {
            SIGNAL_MESSAGES.with_label_values(&["hello", "local"]).inc();
            if !metadata.is_null() {
                update_peer_metadata(state, room_id, peer_id, metadata).await;
            }
            send_json(
                own_tx,
                json!({
                    "type": "hello-ack",
                    "room": room_id,
                    "peerId": peer_id,
                    "atMs": now_ms(),
                }),
            );
            true
        }
        "bye" => {
            SIGNAL_MESSAGES
                .with_label_values(&["bye", "broadcast"])
                .inc();
            let frame = json!({
                "type": "signal",
                "signalType": "bye",
                "room": room_id,
                "from": peer_id,
                "payload": parsed.get("payload").cloned().unwrap_or(Value::Null),
                "sentAtMs": now_ms(),
            });
            let _ = broadcast_to_room(state, room_id, Some(peer_id), frame).await;
            false
        }
        "offer" | "answer" | "ice" | "candidate" | "renegotiate" | "message" => {
            let to_peer = parsed.get("to").and_then(Value::as_str).map(str::to_string);
            let payload = parsed.get("payload").cloned().unwrap_or(Value::Null);
            let frame = json!({
                "type": "signal",
                "signalType": message_type,
                "room": room_id,
                "from": peer_id,
                "to": to_peer,
                "payload": payload,
                "sentAtMs": now_ms(),
            });

            if let Some(to_peer) = to_peer {
                if send_to_peer(state, room_id, &to_peer, frame).await {
                    SIGNAL_MESSAGES
                        .with_label_values(&[message_type.as_str(), "targeted"])
                        .inc();
                } else {
                    SIGNAL_ERRORS.with_label_values(&["target_missing"]).inc();
                    send_json(
                        own_tx,
                        json!({
                            "type": "error",
                            "code": "target_missing",
                            "room": room_id,
                            "peerId": peer_id,
                            "targetPeerId": to_peer,
                            "atMs": now_ms(),
                        }),
                    );
                }
            } else {
                let sent = broadcast_to_room(state, room_id, Some(peer_id), frame).await;
                SIGNAL_MESSAGES
                    .with_label_values(&[message_type.as_str(), "broadcast"])
                    .inc_by(sent as u64);
            }
            true
        }
        _ => {
            SIGNAL_ERRORS.with_label_values(&["unsupported_type"]).inc();
            send_json(
                own_tx,
                json!({
                    "type": "error",
                    "code": "unsupported_type",
                    "message": "supported types: hello, ping, offer, answer, ice, candidate, renegotiate, message, bye",
                    "receivedType": message_type,
                    "atMs": now_ms(),
                }),
            );
            true
        }
    }
}

async fn signal_socket(
    mut socket: WebSocket,
    state: AppState,
    room_id: String,
    peer_id: String,
    user_agent: Option<String>,
) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let connected_at_ms = now_ms();
    let peer_info = PeerSummary {
        peer_id: peer_id.clone(),
        connected_at_ms,
        user_agent,
        metadata: Value::Null,
    };

    let peers = {
        let mut rooms = state.rooms.write().await;
        let room = rooms.entry(room_id.clone()).or_default();
        let peers = room
            .peers
            .values()
            .map(|peer| peer.info.clone())
            .collect::<Vec<_>>();
        room.peers.insert(
            peer_id.clone(),
            PeerConnection {
                info: peer_info.clone(),
                tx: tx.clone(),
            },
        );
        ACTIVE_ROOMS.set(rooms.len() as i64);
        peers
    };

    WS_CONNECTIONS_TOTAL.inc();
    ACTIVE_CONNECTIONS.inc();
    send_json(
        &tx,
        json!({
            "type": "welcome",
            "room": room_id,
            "peerId": peer_id,
            "peers": peers,
            "supportedSignalTypes": ["hello", "ping", "offer", "answer", "ice", "candidate", "renegotiate", "message", "bye"],
            "atMs": now_ms(),
        }),
    );
    let joined_frame = json!({
        "type": "peer-joined",
        "room": room_id,
        "peer": peer_info,
        "atMs": now_ms(),
    });
    let _ = broadcast_to_room(&state, &room_id, Some(&peer_id), joined_frame).await;

    let mut heartbeat = tokio::time::interval(Duration::from_secs(25));
    loop {
        tokio::select! {
            Some(outbound) = rx.recv() => {
                if socket.send(outbound).await.is_err() {
                    break;
                }
            }
            inbound = socket.recv() => {
                match inbound {
                    Some(Ok(Message::Text(text))) => {
                        if !handle_signal_text(&state, &room_id, &peer_id, &tx, text).await {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Binary(_))) => {
                        SIGNAL_ERRORS.with_label_values(&["binary_unsupported"]).inc();
                        send_json(
                            &tx,
                            json!({
                                "type": "error",
                                "code": "binary_unsupported",
                                "message": "send JSON text frames",
                                "atMs": now_ms(),
                            }),
                        );
                    }
                    Some(Err(_)) => break,
                }
            }
            _ = heartbeat.tick() => {
                send_json(
                    &tx,
                    json!({
                        "type": "heartbeat",
                        "room": room_id,
                        "peerId": peer_id,
                        "atMs": now_ms(),
                    }),
                );
            }
        }
    }

    remove_peer(&state, &room_id, &peer_id).await;
}

async fn admin_runtime_nats_bridge(tx: mpsc::UnboundedSender<Message>) {
    let subject = runtime_admin_subject();
    let nats = match async_nats::connect(nats_url()).await {
        Ok(client) => client,
        Err(error) => {
            send_json(
                &tx,
                json!({
                    "type": "error",
                    "code": "nats_connect_failed",
                    "message": error.to_string(),
                    "atMs": now_ms(),
                }),
            );
            return;
        }
    };
    let mut subscription = match nats.subscribe(subject.clone()).await {
        Ok(subscription) => subscription,
        Err(error) => {
            send_json(
                &tx,
                json!({
                    "type": "error",
                    "code": "nats_subscribe_failed",
                    "subject": subject,
                    "message": error.to_string(),
                    "atMs": now_ms(),
                }),
            );
            return;
        }
    };
    send_json(
        &tx,
        json!({
            "type": "nats-subscribed",
            "mode": "admin-runtime-nats",
            "subject": subject,
            "forwardedTypes": ["k8s-runtime-event", "task-event"],
            "atMs": now_ms(),
        }),
    );

    while let Some(message) = subscription.next().await {
        let text = String::from_utf8_lossy(&message.payload);
        if should_forward_admin_runtime_message(&text)
            && tx.send(Message::Text(text.to_string())).is_err()
        {
            break;
        }
    }
    send_json(
        &tx,
        json!({
            "type": "error",
            "code": "nats_subscription_closed",
            "subject": subject,
            "atMs": now_ms(),
        }),
    );
}

async fn admin_runtime_socket(mut socket: WebSocket, state: AppState) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let client_id = PEER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    {
        let mut clients = state.admin_runtime_clients.write().await;
        clients.insert(client_id, tx.clone());
    }
    WS_CONNECTIONS_TOTAL.inc();
    ACTIVE_CONNECTIONS.inc();
    send_json(
        &tx,
        json!({
            "type": "welcome",
            "mode": "admin-runtime-fanout",
            "clientId": client_id,
            "directBroadcast": true,
            "natsSubject": runtime_admin_subject(),
            "atMs": now_ms(),
        }),
    );
    tokio::spawn(admin_runtime_nats_bridge(tx.clone()));
    let mut heartbeat = tokio::time::interval(Duration::from_secs(25));

    loop {
        tokio::select! {
            Some(outbound) = rx.recv() => {
                if socket.send(outbound).await.is_err() {
                    break;
                }
            }
            inbound = socket.recv() => {
                match inbound {
                    Some(Ok(Message::Text(text))) if text == "ping" => {
                        if socket.send(json_text(json!({"type": "pong", "atMs": now_ms()}))).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        let parsed = serde_json::from_str::<Value>(&text).ok();
                        if parsed
                            .as_ref()
                            .and_then(|value| value.get("type"))
                            .and_then(Value::as_str)
                            == Some("ping")
                            && socket.send(json_text(json!({"type": "pong", "atMs": now_ms()}))).await.is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Pong(_))) | Some(Ok(Message::Binary(_))) => {}
                    Some(Err(_)) => break,
                }
            }
            _ = heartbeat.tick() => {
                if tx
                    .send(json_text(json!({
                        "type": "heartbeat",
                        "mode": "admin-runtime-fanout",
                        "clientId": client_id,
                        "atMs": now_ms(),
                    })))
                    .is_err()
                {
                    break;
                }
            }
        }
    }
    {
        let mut clients = state.admin_runtime_clients.write().await;
        clients.remove(&client_id);
    }
    ACTIVE_CONNECTIONS.dec();
}

async fn admin_runtime_ws(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
) -> Response {
    if !authorized_admin_ws(&headers) {
        record_http("GET", "/webrtc/runtime/ws", StatusCode::UNAUTHORIZED);
        return StatusCode::UNAUTHORIZED.into_response();
    }
    record_http("GET", "/webrtc/runtime/ws", StatusCode::SWITCHING_PROTOCOLS);
    ws.on_upgrade(move |socket| admin_runtime_socket(socket, state))
}

async fn admin_runtime_broadcast(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    if !authorized_runtime_broadcast(&headers) {
        record_http(
            "POST",
            "/webrtc/runtime/broadcast",
            StatusCode::UNAUTHORIZED,
        );
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let delivered = broadcast_to_admin_runtime(&state, payload).await;
    record_http("POST", "/webrtc/runtime/broadcast", StatusCode::ACCEPTED);
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "ok": true,
            "delivered": delivered,
        })),
    )
        .into_response()
}

async fn signal_ws(
    ws: WebSocketUpgrade,
    Query(query): Query<SignalQuery>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let room_id = match normalize_id(query.room, "room") {
        Ok(room_id) => room_id,
        Err(message) => {
            record_http("GET", "/signal", StatusCode::BAD_REQUEST);
            return (StatusCode::BAD_REQUEST, message).into_response();
        }
    };
    let peer_id = match normalize_id(query.peer, "peer") {
        Ok(peer_id) => peer_id,
        Err(message) => {
            record_http("GET", "/signal", StatusCode::BAD_REQUEST);
            return (StatusCode::BAD_REQUEST, message).into_response();
        }
    };
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    record_http("GET", "/signal", StatusCode::SWITCHING_PROTOCOLS);
    ws.on_upgrade(move |socket| signal_socket(socket, state, room_id, peer_id, user_agent))
}

async fn root() -> impl IntoResponse {
    record_http("GET", "/", StatusCode::OK);
    Html(HOME_HTML)
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    set_room_gauge(&state).await;
    record_http("GET", "/healthz", StatusCode::OK);
    Json(HealthResponse {
        ok: true,
        service: "dd-webrtc-signaling".to_string(),
        mode: "room-websocket-signaling-only".to_string(),
        uptime_seconds: STARTED_AT.elapsed().as_secs(),
        active_rooms: ACTIVE_ROOMS.get(),
        active_connections: ACTIVE_CONNECTIONS.get(),
    })
}

async fn metrics() -> impl IntoResponse {
    record_http("GET", "/metrics", StatusCode::OK);
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    let status = match encoder.encode(&metric_families, &mut buffer) {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut response = Response::new(axum::body::Body::from(buffer));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    response
}

const HOME_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>dd WebRTC signaling</title>
    <style>
      :root { color-scheme: dark; --bg: #0b1117; --panel: #111923; --line: rgba(148,163,184,.24); --text: #eef2f6; --muted: #a8b3c1; --accent: #5eead4; }
      * { box-sizing: border-box; }
      body { margin: 0; min-height: 100vh; background: var(--bg); color: var(--text); font-family: Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif; padding: 24px; }
      main { max-width: 960px; margin: 0 auto; }
      h1 { margin: 0 0 10px; font-size: 30px; }
      h2 { margin: 0 0 10px; font-size: 17px; }
      p, li { color: var(--muted); line-height: 1.55; }
      a { color: var(--accent); text-decoration: none; }
      a:hover { text-decoration: underline; }
      section { border: 1px solid var(--line); border-radius: 8px; background: var(--panel); padding: 16px; margin: 16px 0; }
      code { display: inline-block; max-width: 100%; overflow-wrap: anywhere; border: 1px solid rgba(148,163,184,.2); border-radius: 6px; padding: 2px 5px; background: #0a1017; color: #d7fbf4; font-size: 12px; }
      pre { overflow: auto; border: 1px solid var(--line); border-radius: 8px; padding: 12px; background: #0a1017; color: #d7fbf4; }
    </style>
  </head>
  <body>
    <main>
      <h1>dd WebRTC signaling</h1>
      <p>This service handles room membership and WebRTC signaling only. Media/data channels remain peer-to-peer between browser and mobile clients whenever NAT traversal allows it.</p>
      <section>
        <h2>Endpoints</h2>
        <p><a href="/webrtc/healthz"><code>/webrtc/healthz</code></a> <a href="/webrtc/metrics"><code>/webrtc/metrics</code></a> <code>wss://54.91.17.58/webrtc/signal?room=&lt;roomId&gt;&amp;peer=&lt;peerId&gt;</code></p>
      </section>
      <section>
        <h2>Protocol</h2>
        <p>Clients join a room with a websocket. Send JSON text frames with <code>type</code> values like <code>offer</code>, <code>answer</code>, <code>ice</code>, <code>candidate</code>, <code>renegotiate</code>, <code>message</code>, <code>ping</code>, and <code>bye</code>. Add <code>to</code> for targeted delivery, or omit it to broadcast to the room.</p>
        <pre>{"type":"offer","to":"mobile-peer","payload":{"sdp":"..."}}</pre>
      </section>
      <section>
        <h2>Client Notes</h2>
        <ul>
          <li>Works the same for browser to mobile, mobile to mobile, and browser to browser: all clients speak the same websocket signaling protocol.</li>
          <li>Use STUN in the client WebRTC config. Add a TURN deployment later for strict NATs and mobile carrier networks.</li>
          <li>This is not an SFU or media relay. It never sees audio, video, or WebRTC data-channel payloads after peers connect.</li>
        </ul>
      </section>
    </main>
  </body>
</html>"#;

async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

#[tokio::main]
async fn main() {
    let _otel = dd_telemetry::init("dd-webrtc-signaling");

    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT").unwrap_or_else(|_| "8095".to_string());
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("HOST/PORT must form a socket address");
    let state = AppState {
        rooms: Arc::new(RwLock::new(HashMap::new())),
        admin_runtime_clients: Arc::new(RwLock::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/", get(root))
        .route("/webrtc", get(root))
        .route("/webrtc/", get(root))
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/webrtc/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/webrtc/metrics", get(metrics))
        .route("/runtime/ws", get(admin_runtime_ws))
        .route("/webrtc/runtime/ws", get(admin_runtime_ws))
        .route("/runtime/broadcast", post(admin_runtime_broadcast))
        .route("/webrtc/runtime/broadcast", post(admin_runtime_broadcast))
        .route("/signal", get(signal_ws))
        .route("/webrtc/signal", get(signal_ws))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind WebRTC signaling listener");
    tracing::info!("dd-webrtc-signaling listening on http://{addr}");
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("dd-webrtc-signaling server failed");
}
