mod auth;

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

use crate::auth::{sanitize_client_metadata, PeerIdentity, SignalAuthConfig, SignalAuthError};

static STARTED_AT: Lazy<Instant> = Lazy::new(Instant::now);
static PEER_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const MAX_SIGNAL_FRAME_BYTES: usize = 64 * 1024;
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
    signal_auth: SignalAuthConfig,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    identity: Option<PeerIdentity>,
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
                match sanitize_client_metadata(metadata) {
                    Ok(metadata) => {
                        update_peer_metadata(state, room_id, peer_id, metadata).await;
                    }
                    Err(message) => {
                        SIGNAL_ERRORS.with_label_values(&["invalid_metadata"]).inc();
                        send_json(
                            own_tx,
                            json!({
                                "type": "error",
                                "code": "invalid_metadata",
                                "message": message,
                                "atMs": now_ms(),
                            }),
                        );
                        return true;
                    }
                }
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
    identity: Option<PeerIdentity>,
) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let connected_at_ms = now_ms();
    let peer_info = PeerSummary {
        peer_id: peer_id.clone(),
        connected_at_ms,
        user_agent,
        identity,
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
    let identity = match state.signal_auth.identity_from_headers(&headers) {
        Ok(identity) => identity,
        Err(
            error @ (SignalAuthError::AuthenticationRequired
            | SignalAuthError::UntrustedProxy
            | SignalAuthError::InvalidIdentity),
        ) => {
            record_http("GET", "/signal", StatusCode::UNAUTHORIZED);
            return (StatusCode::UNAUTHORIZED, error.public_message()).into_response();
        }
    };
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
    ws.max_message_size(MAX_SIGNAL_FRAME_BYTES)
        .max_frame_size(MAX_SIGNAL_FRAME_BYTES)
        .on_upgrade(move |socket| {
            signal_socket(socket, state, room_id, peer_id, user_agent, identity)
        })
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
    let signal_auth =
        SignalAuthConfig::from_env().expect("invalid WebRTC shared-auth configuration");
    tracing::info!(auth_mode = ?signal_auth.mode, "configured WebRTC upgrade authentication");
    let state = AppState {
        rooms: Arc::new(RwLock::new(HashMap::new())),
        admin_runtime_clients: Arc::new(RwLock::new(HashMap::new())),
        signal_auth,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // -------------------------------------------------------------------
    // helpers
    // -------------------------------------------------------------------

    fn test_state() -> AppState {
        AppState {
            rooms: Arc::new(RwLock::new(HashMap::new())),
            admin_runtime_clients: Arc::new(RwLock::new(HashMap::new())),
            signal_auth: SignalAuthConfig::disabled(),
        }
    }

    /// Insert a peer into `room` exactly like `signal_socket` does, and return
    /// (its outbound sender, its receiver). The sender doubles as `own_tx`.
    async fn insert_peer(
        state: &AppState,
        room: &str,
        peer: &str,
    ) -> (
        mpsc::UnboundedSender<Message>,
        mpsc::UnboundedReceiver<Message>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel::<Message>();
        let mut rooms = state.rooms.write().await;
        let room_state = rooms.entry(room.to_string()).or_default();
        room_state.peers.insert(
            peer.to_string(),
            PeerConnection {
                info: PeerSummary {
                    peer_id: peer.to_string(),
                    connected_at_ms: 0,
                    user_agent: None,
                    identity: None,
                    metadata: Value::Null,
                },
                tx: tx.clone(),
            },
        );
        (tx, rx)
    }

    /// Non-blocking drain of every queued text frame, parsed as JSON.
    fn drain(rx: &mut mpsc::UnboundedReceiver<Message>) -> Vec<Value> {
        let mut out = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(Message::Text(text)) => {
                    out.push(serde_json::from_str::<Value>(&text).expect("frame is valid json"));
                }
                Ok(other) => panic!("expected a text frame, got {other:?}"),
                Err(_) => break,
            }
        }
        out
    }

    async fn handle(
        state: &AppState,
        room: &str,
        peer: &str,
        own_tx: &mpsc::UnboundedSender<Message>,
        text: &str,
    ) -> bool {
        handle_signal_text(state, room, peer, own_tx, text.to_string()).await
    }

    async fn peer_metadata(state: &AppState, room: &str, peer: &str) -> Value {
        state
            .rooms
            .read()
            .await
            .get(room)
            .and_then(|room| room.peers.get(peer))
            .map(|peer| peer.info.metadata.clone())
            .unwrap_or(Value::Null)
    }

    async fn room_exists(state: &AppState, room: &str) -> bool {
        state.rooms.read().await.contains_key(room)
    }

    async fn peer_exists(state: &AppState, room: &str, peer: &str) -> bool {
        state
            .rooms
            .read()
            .await
            .get(room)
            .map(|room| room.peers.contains_key(peer))
            .unwrap_or(false)
    }

    // env-var tests mutate process-global state; serialize them and restore.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn save(keys: &[&str]) -> Vec<(String, Option<String>)> {
        keys.iter()
            .map(|&k| (k.to_string(), env::var(k).ok()))
            .collect()
    }

    fn restore(saved: &[(String, Option<String>)]) {
        for (key, value) in saved {
            match value {
                Some(value) => env::set_var(key, value),
                None => env::remove_var(key),
            }
        }
    }

    // -------------------------------------------------------------------
    // normalize_id: the room/peer id validation applied at join time
    // -------------------------------------------------------------------

    #[test]
    fn normalize_id_accepts_and_trims_valid() {
        assert_eq!(
            normalize_id(Some("  room-1  ".into()), "room").unwrap(),
            "room-1"
        );
        assert_eq!(normalize_id(Some("peer".into()), "peer").unwrap(), "peer");
    }

    #[test]
    fn normalize_id_accepts_full_allowed_charset() {
        let id = "Abc-123_foo.BAR";
        assert_eq!(normalize_id(Some(id.into()), "peer").unwrap(), id);
    }

    #[test]
    fn normalize_id_rejects_empty_and_whitespace() {
        assert!(normalize_id(Some(String::new()), "room").is_err());
        assert!(normalize_id(Some("    ".into()), "room").is_err());
    }

    #[test]
    fn normalize_id_length_boundary_96_ok_97_rejected() {
        let ok = "a".repeat(96);
        assert_eq!(normalize_id(Some(ok.clone()), "room").unwrap(), ok);
        // length is measured on the trimmed value
        let padded = format!("  {}  ", "a".repeat(96));
        assert_eq!(normalize_id(Some(padded), "room").unwrap(), "a".repeat(96));
        assert!(normalize_id(Some("a".repeat(97)), "room").is_err());
    }

    #[test]
    fn normalize_id_rejects_illegal_characters() {
        // path-traversal / spoofing / injection shaped inputs must be rejected
        for bad in [
            "room/1", "room 1", "a:b", "a@b", "a$b", "a\tb", "café", "a\nb", "../etc", "a*b",
            "a?b", "a=b", "a,b", "a;b", "a#b", "a%b",
        ] {
            assert!(
                normalize_id(Some(bad.to_string()), "peer").is_err(),
                "expected {bad:?} to be rejected",
            );
        }
    }

    #[test]
    fn normalize_id_none_generates_unique_fallback_ids() {
        let a = normalize_id(None, "peer").unwrap();
        let b = normalize_id(None, "peer").unwrap();
        assert!(a.starts_with("peer-"), "got {a}");
        assert!(b.starts_with("peer-"), "got {b}");
        assert_ne!(a, b, "fallback ids must be unique");
        // a generated id is itself a valid id
        assert_eq!(normalize_id(Some(a.clone()), "peer").unwrap(), a);
    }

    // -------------------------------------------------------------------
    // admin-runtime forward filter / serialization / header auth / clock
    // -------------------------------------------------------------------

    #[test]
    fn should_forward_admin_runtime_message_accepts_allowed_types() {
        assert!(should_forward_admin_runtime_message(
            r#"{"type":"k8s-runtime-event","x":1}"#
        ));
        assert!(should_forward_admin_runtime_message(
            r#"{"type":"task-event"}"#
        ));
    }

    #[test]
    fn should_forward_admin_runtime_message_rejects_others_and_malformed() {
        assert!(!should_forward_admin_runtime_message(r#"{"type":"chat"}"#));
        assert!(!should_forward_admin_runtime_message(r#"{"no_type":true}"#));
        assert!(!should_forward_admin_runtime_message(r#"{"type":5}"#));
        assert!(!should_forward_admin_runtime_message("not json"));
        assert!(!should_forward_admin_runtime_message("[1,2,3]"));
        assert!(!should_forward_admin_runtime_message(
            "\"k8s-runtime-event\""
        ));
    }

    #[test]
    fn json_text_roundtrips_value() {
        let value = json!({"type":"x","nested":{"a":[1,2,3]},"n":42});
        match json_text(value.clone()) {
            Message::Text(text) => {
                let back: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(back, value);
            }
            other => panic!("expected a text frame, got {other:?}"),
        }
    }

    #[test]
    fn authorized_admin_ws_requires_exact_flag() {
        let mut yes = HeaderMap::new();
        yes.insert("x-dd-admin", HeaderValue::from_static("1"));
        assert!(authorized_admin_ws(&yes));

        let mut zero = HeaderMap::new();
        zero.insert("x-dd-admin", HeaderValue::from_static("0"));
        assert!(!authorized_admin_ws(&zero));

        let mut two = HeaderMap::new();
        two.insert("x-dd-admin", HeaderValue::from_static("2"));
        assert!(!authorized_admin_ws(&two));

        assert!(!authorized_admin_ws(&HeaderMap::new()));
    }

    #[test]
    fn now_ms_is_a_millisecond_epoch_after_2020() {
        // 1_600_000_000_000 ms == 2020-09-13; guards against unit/precision drift
        assert!(now_ms() > 1_600_000_000_000);
    }

    // -------------------------------------------------------------------
    // config-from-env: defaults, overrides, secret precedence
    // -------------------------------------------------------------------

    #[test]
    fn env_string_trims_and_filters_empty() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let saved = save(&["DD_TEST_ENV_STRING"]);

        env::set_var("DD_TEST_ENV_STRING", "  hello  ");
        assert_eq!(env_string("DD_TEST_ENV_STRING"), Some("hello".to_string()));

        env::set_var("DD_TEST_ENV_STRING", "   ");
        assert_eq!(env_string("DD_TEST_ENV_STRING"), None);

        env::remove_var("DD_TEST_ENV_STRING");
        assert_eq!(env_string("DD_TEST_ENV_STRING"), None);

        restore(&saved);
    }

    #[test]
    fn nats_url_default_and_override() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let saved = save(&["NATS_URL"]);

        env::remove_var("NATS_URL");
        assert_eq!(
            nats_url(),
            "nats://dd-nats.messaging.svc.cluster.local:4222"
        );

        env::set_var("NATS_URL", "nats://localhost:4222");
        assert_eq!(nats_url(), "nats://localhost:4222");

        restore(&saved);
    }

    #[test]
    fn runtime_admin_subject_default_and_override() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let saved = save(&["RUNTIME_ADMIN_EVENT_SUBJECT"]);

        env::remove_var("RUNTIME_ADMIN_EVENT_SUBJECT");
        assert_eq!(runtime_admin_subject(), RUNTIME_EVENTS_SUBJECT);

        env::set_var("RUNTIME_ADMIN_EVENT_SUBJECT", "custom.subject");
        assert_eq!(runtime_admin_subject(), "custom.subject");

        restore(&saved);
    }

    #[test]
    fn runtime_broadcast_secret_precedence() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let keys = [
            "RUNTIME_BROADCAST_SECRET",
            "REMOTE_DEV_SERVER_SECRET",
            "SERVER_AUTH_SECRET",
        ];
        let saved = save(&keys);
        for key in keys {
            env::remove_var(key);
        }

        assert_eq!(runtime_broadcast_secret(), None);

        env::set_var("SERVER_AUTH_SECRET", "third");
        assert_eq!(runtime_broadcast_secret(), Some("third".to_string()));

        env::set_var("REMOTE_DEV_SERVER_SECRET", "second");
        assert_eq!(runtime_broadcast_secret(), Some("second".to_string()));

        env::set_var("RUNTIME_BROADCAST_SECRET", "first");
        assert_eq!(runtime_broadcast_secret(), Some("first".to_string()));

        restore(&saved);
    }

    #[test]
    fn authorized_runtime_broadcast_matches_any_accepted_header() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let keys = [
            "RUNTIME_BROADCAST_SECRET",
            "REMOTE_DEV_SERVER_SECRET",
            "SERVER_AUTH_SECRET",
        ];
        let saved = save(&keys);
        for key in keys {
            env::remove_var(key);
        }

        // no secret configured -> deny even when a header is present
        let mut header = HeaderMap::new();
        header.insert("x-server-auth", HeaderValue::from_static("whatever"));
        assert!(!authorized_runtime_broadcast(&header));

        env::set_var("RUNTIME_BROADCAST_SECRET", "s3cret");

        for name in ["x-server-auth", "x-dd-internal-auth", "x-agent-auth"] {
            let mut ok = HeaderMap::new();
            ok.insert(name, HeaderValue::from_str("s3cret").unwrap());
            assert!(authorized_runtime_broadcast(&ok), "{name} should authorize");
        }

        let mut wrong = HeaderMap::new();
        wrong.insert("x-server-auth", HeaderValue::from_str("nope").unwrap());
        assert!(!authorized_runtime_broadcast(&wrong));

        // right secret under an unaccepted header name is ignored
        let mut unrelated = HeaderMap::new();
        unrelated.insert("authorization", HeaderValue::from_str("s3cret").unwrap());
        assert!(!authorized_runtime_broadcast(&unrelated));

        restore(&saved);
    }

    // -------------------------------------------------------------------
    // room routing primitives: broadcast_to_room / send_to_peer
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn broadcast_to_room_excludes_sender_and_counts() {
        let state = test_state();
        let (_a, mut ra) = insert_peer(&state, "r1", "A").await;
        let (_b, mut rb) = insert_peer(&state, "r1", "B").await;
        let (_c, mut rc) = insert_peer(&state, "r1", "C").await;

        let sent = broadcast_to_room(&state, "r1", Some("A"), json!({"type":"x"})).await;
        assert_eq!(sent, 2);
        assert!(drain(&mut ra).is_empty(), "sender must be excluded");
        assert_eq!(drain(&mut rb).len(), 1);
        assert_eq!(drain(&mut rc).len(), 1);
    }

    #[tokio::test]
    async fn broadcast_to_room_none_except_hits_all() {
        let state = test_state();
        let (_a, mut ra) = insert_peer(&state, "r1", "A").await;
        let (_b, mut rb) = insert_peer(&state, "r1", "B").await;
        let sent = broadcast_to_room(&state, "r1", None, json!({"type":"x"})).await;
        assert_eq!(sent, 2);
        assert_eq!(drain(&mut ra).len(), 1);
        assert_eq!(drain(&mut rb).len(), 1);
    }

    #[tokio::test]
    async fn broadcast_to_room_unknown_room_is_zero() {
        let state = test_state();
        let sent = broadcast_to_room(&state, "ghost", None, json!({"type":"x"})).await;
        assert_eq!(sent, 0);
    }

    #[tokio::test]
    async fn broadcast_to_room_is_isolated_across_rooms() {
        let state = test_state();
        let (_a, mut ra) = insert_peer(&state, "r1", "A").await;
        let (_x, mut rx) = insert_peer(&state, "r2", "X").await;
        let sent = broadcast_to_room(&state, "r1", None, json!({"type":"x"})).await;
        assert_eq!(sent, 1);
        assert_eq!(drain(&mut ra).len(), 1);
        assert!(
            drain(&mut rx).is_empty(),
            "a peer in another room must never receive"
        );
    }

    #[tokio::test]
    async fn send_to_peer_targets_a_single_peer() {
        let state = test_state();
        let (_a, mut ra) = insert_peer(&state, "r1", "A").await;
        let (_b, mut rb) = insert_peer(&state, "r1", "B").await;
        assert!(send_to_peer(&state, "r1", "B", json!({"type":"x"})).await);
        assert!(drain(&mut ra).is_empty());
        assert_eq!(drain(&mut rb).len(), 1);
    }

    #[tokio::test]
    async fn send_to_peer_missing_target_is_false() {
        let state = test_state();
        let _a = insert_peer(&state, "r1", "A").await;
        assert!(!send_to_peer(&state, "r1", "ghost", json!({"type":"x"})).await);
    }

    #[tokio::test]
    async fn send_to_peer_is_isolated_across_rooms() {
        let state = test_state();
        let _a = insert_peer(&state, "r1", "A").await;
        let (_b, mut rb) = insert_peer(&state, "r2", "B").await;
        // B exists, but only in r2: a targeted send scoped to r1 must fail and never leak
        assert!(!send_to_peer(&state, "r1", "B", json!({"type":"x"})).await);
        assert!(drain(&mut rb).is_empty());
    }

    // -------------------------------------------------------------------
    // membership state machine: update_peer_metadata / remove_peer
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn update_peer_metadata_only_touches_target() {
        let state = test_state();
        let _a = insert_peer(&state, "r1", "A").await;
        let _b = insert_peer(&state, "r1", "B").await;
        update_peer_metadata(&state, "r1", "A", json!({"role":"host"})).await;
        assert_eq!(
            peer_metadata(&state, "r1", "A").await,
            json!({"role":"host"})
        );
        assert_eq!(peer_metadata(&state, "r1", "B").await, Value::Null);
    }

    #[tokio::test]
    async fn remove_peer_broadcasts_left_and_keeps_room_with_survivors() {
        let state = test_state();
        let (_a, mut ra) = insert_peer(&state, "r1", "A").await;
        let (_b, mut rb) = insert_peer(&state, "r1", "B").await;
        remove_peer(&state, "r1", "A").await;

        assert!(!peer_exists(&state, "r1", "A").await);
        assert!(
            room_exists(&state, "r1").await,
            "room persists while B remains"
        );

        let frames = drain(&mut rb);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["type"], json!("peer-left"));
        assert_eq!(frames[0]["peerId"], json!("A"));
        assert_eq!(frames[0]["room"], json!("r1"));
        assert!(
            drain(&mut ra).is_empty(),
            "the leaving peer is excluded from its own peer-left"
        );
    }

    #[tokio::test]
    async fn remove_peer_reclaims_empty_room() {
        let state = test_state();
        let _a = insert_peer(&state, "r1", "A").await;
        remove_peer(&state, "r1", "A").await;
        assert!(
            !room_exists(&state, "r1").await,
            "an emptied room must be reclaimed"
        );
    }

    // -------------------------------------------------------------------
    // signaling protocol: handle_signal_text (offer/answer/ICE/hello/bye)
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn handle_ping_replies_pong() {
        let state = test_state();
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
        assert!(handle(&state, "r1", "solo", &tx, r#"{"type":"ping"}"#).await);
        let frames = drain(&mut rx);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["type"], json!("pong"));
        assert_eq!(frames[0]["room"], json!("r1"));
        assert_eq!(frames[0]["peerId"], json!("solo"));
    }

    #[tokio::test]
    async fn handle_hello_acks_and_stores_metadata() {
        let state = test_state();
        let (tx, mut rx) = insert_peer(&state, "r1", "A").await;
        assert!(
            handle(
                &state,
                "r1",
                "A",
                &tx,
                r#"{"type":"hello","metadata":{"ua":"firefox"}}"#,
            )
            .await
        );
        let frames = drain(&mut rx);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["type"], json!("hello-ack"));
        assert_eq!(
            peer_metadata(&state, "r1", "A").await,
            json!({"ua":"firefox"})
        );
    }

    #[tokio::test]
    async fn handle_hello_without_metadata_keeps_null() {
        let state = test_state();
        let (tx, mut rx) = insert_peer(&state, "r1", "A").await;
        assert!(handle(&state, "r1", "A", &tx, r#"{"type":"hello"}"#).await);
        let frames = drain(&mut rx);
        assert_eq!(frames[0]["type"], json!("hello-ack"));
        assert_eq!(peer_metadata(&state, "r1", "A").await, Value::Null);
    }

    #[tokio::test]
    async fn handle_bye_broadcasts_and_closes() {
        let state = test_state();
        let (tx, mut ra) = insert_peer(&state, "r1", "A").await;
        let (_b, mut rb) = insert_peer(&state, "r1", "B").await;
        let keep = handle(
            &state,
            "r1",
            "A",
            &tx,
            r#"{"type":"bye","payload":{"reason":"done"}}"#,
        )
        .await;
        assert!(!keep, "bye closes the connection");
        assert!(
            drain(&mut ra).is_empty(),
            "sender gets no echo of its own bye"
        );
        let frames = drain(&mut rb);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["type"], json!("signal"));
        assert_eq!(frames[0]["signalType"], json!("bye"));
        assert_eq!(frames[0]["from"], json!("A"));
        assert_eq!(frames[0]["payload"], json!({"reason":"done"}));
    }

    #[tokio::test]
    async fn handle_targeted_offer_delivers_wrapped_signal() {
        let state = test_state();
        let (tx, mut ra) = insert_peer(&state, "r1", "A").await;
        let (_b, mut rb) = insert_peer(&state, "r1", "B").await;
        assert!(
            handle(
                &state,
                "r1",
                "A",
                &tx,
                r#"{"type":"offer","to":"B","payload":{"sdp":"v=0"}}"#,
            )
            .await
        );
        assert!(drain(&mut ra).is_empty(), "no echo to sender on success");
        let frames = drain(&mut rb);
        assert_eq!(frames.len(), 1);
        let frame = &frames[0];
        assert_eq!(frame["type"], json!("signal"));
        assert_eq!(frame["signalType"], json!("offer"));
        assert_eq!(frame["room"], json!("r1"));
        assert_eq!(frame["from"], json!("A"));
        assert_eq!(frame["to"], json!("B"));
        assert_eq!(frame["payload"], json!({"sdp":"v=0"}));
    }

    #[tokio::test]
    async fn handle_targeted_offer_missing_target_errors_back_to_sender() {
        let state = test_state();
        let (tx, mut ra) = insert_peer(&state, "r1", "A").await;
        assert!(
            handle(
                &state,
                "r1",
                "A",
                &tx,
                r#"{"type":"offer","to":"ghost","payload":{}}"#,
            )
            .await
        );
        let frames = drain(&mut ra);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["type"], json!("error"));
        assert_eq!(frames[0]["code"], json!("target_missing"));
        assert_eq!(frames[0]["targetPeerId"], json!("ghost"));
    }

    #[tokio::test]
    async fn handle_broadcast_answer_reaches_others_only() {
        let state = test_state();
        let (tx, mut ra) = insert_peer(&state, "r1", "A").await;
        let (_b, mut rb) = insert_peer(&state, "r1", "B").await;
        let (_c, mut rc) = insert_peer(&state, "r1", "C").await;
        assert!(
            handle(
                &state,
                "r1",
                "A",
                &tx,
                r#"{"type":"answer","payload":{"sdp":"a"}}"#,
            )
            .await
        );
        assert!(drain(&mut ra).is_empty());
        let fb = drain(&mut rb);
        let fc = drain(&mut rc);
        assert_eq!(fb.len(), 1);
        assert_eq!(fc.len(), 1);
        assert_eq!(fb[0]["signalType"], json!("answer"));
        assert_eq!(fb[0]["from"], json!("A"));
        assert!(fb[0]["to"].is_null(), "a broadcast frame has a null target");
    }

    #[tokio::test]
    async fn handle_untyped_frame_defaults_to_message_broadcast() {
        let state = test_state();
        let (tx, _ra) = insert_peer(&state, "r1", "A").await;
        let (_b, mut rb) = insert_peer(&state, "r1", "B").await;
        // absent "type" defaults to "message" and broadcasts
        assert!(handle(&state, "r1", "A", &tx, r#"{"payload":{"k":1}}"#).await);
        let frames = drain(&mut rb);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["signalType"], json!("message"));
        assert_eq!(frames[0]["payload"], json!({"k":1}));
    }

    #[tokio::test]
    async fn handle_unknown_type_errors() {
        let state = test_state();
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
        assert!(
            handle(&state, "r1", "A", &tx, r#"{"type":"frobnicate"}"#).await,
            "an unknown type keeps the connection open"
        );
        let frames = drain(&mut rx);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["type"], json!("error"));
        assert_eq!(frames[0]["code"], json!("unsupported_type"));
        assert_eq!(frames[0]["receivedType"], json!("frobnicate"));
    }

    #[tokio::test]
    async fn handle_invalid_json_errors() {
        let state = test_state();
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
        assert!(handle(&state, "r1", "A", &tx, "definitely not json {").await);
        let frames = drain(&mut rx);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["type"], json!("error"));
        assert_eq!(frames[0]["code"], json!("invalid_json"));
    }

    #[tokio::test]
    async fn handle_targeted_offer_does_not_cross_rooms() {
        // B lives in r2; a targeted offer scoped to r1 must not reach it.
        let state = test_state();
        let (tx, mut ra) = insert_peer(&state, "r1", "A").await;
        let (_b, mut rb) = insert_peer(&state, "r2", "B").await;
        assert!(
            handle(
                &state,
                "r1",
                "A",
                &tx,
                r#"{"type":"offer","to":"B","payload":{}}"#,
            )
            .await
        );
        assert!(
            drain(&mut rb).is_empty(),
            "cross-room targeted delivery must not leak"
        );
        let frames = drain(&mut ra);
        assert_eq!(frames[0]["code"], json!("target_missing"));
    }

    #[tokio::test]
    async fn handle_broadcast_does_not_cross_rooms() {
        let state = test_state();
        let (tx, _ra) = insert_peer(&state, "r1", "A").await;
        let (_x, mut rx) = insert_peer(&state, "r2", "X").await;
        handle(
            &state,
            "r1",
            "A",
            &tx,
            r#"{"type":"offer","payload":{"sdp":"o"}}"#,
        )
        .await;
        assert!(
            drain(&mut rx).is_empty(),
            "a room broadcast must not cross rooms"
        );
    }

    #[tokio::test]
    async fn from_field_is_server_authoritative_not_client_spoofable() {
        // A client supplies a forged `from`; the server must stamp the real peer id.
        let state = test_state();
        let (tx, _ra) = insert_peer(&state, "r1", "attacker").await;
        let (_b, mut rb) = insert_peer(&state, "r1", "victim").await;
        handle(
            &state,
            "r1",
            "attacker",
            &tx,
            r#"{"type":"offer","to":"victim","from":"admin","payload":{"sdp":"x"}}"#,
        )
        .await;
        let frames = drain(&mut rb);
        assert_eq!(frames.len(), 1);
        assert_eq!(
            frames[0]["from"],
            json!("attacker"),
            "server must ignore a client-supplied `from`"
        );
    }
}
