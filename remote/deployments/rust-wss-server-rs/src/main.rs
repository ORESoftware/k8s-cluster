//! dd-rust-wss-server
//!
//! A minimal high-throughput Rust WebSocket server purpose-built as a
//! benchmark peer for `dd-dart-server` and `dd-gleamlang-server`. The
//! goal is parity with how the Dart pod is structured so the head-to-head
//! comparison is meaningful:
//!
//!   * Single pod, single-process. Concurrency comes from a multi-thread
//!     tokio runtime inside the process.
//!   * The WS port (default 8097) is bound by N independent acceptor
//!     tasks, each owning its own `TcpListener` with `SO_REUSEPORT`. The
//!     kernel hashes incoming SYNs across the listeners — same model as
//!     Dart's gateway-shard isolates that all bind 8089 with
//!     `shared: true`.
//!   * The admin port (default 8098) hosts `/metrics`, `/healthz`,
//!     `/readyz` on a separate axum router so probe + Prometheus traffic
//!     can never queue behind WS work.
//!
//! Wire protocol (kept deliberately small so the loader's correlation
//! map works without server-side message-id allocation):
//!
//! Inbound JSON:
//!   {"type":"ping","id":"<id>","ts":<u64>}      → pong-style reply
//!   {"id":"<id>","payload":"..."}                → akka-style ok-result
//!     (matches `LOAD_MODE=pipeline` on `ws-loadtest-rs`, which already
//!      emits this shape; lets us reuse the existing pipeline loader
//!      verbatim against this server)
//!
//! Inbound text "ping" → "{\"type\":\"pong\",\"ts\":<ms>}"
//!
//! Anything else is dropped silently (kept off the hot path; we don't
//! want to log per-frame).

use std::env;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::{http::StatusCode, response::IntoResponse, routing::get, Router};
use futures_util::{SinkExt, StreamExt};
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounter, IntCounterVec, IntGauge, Opts, TextEncoder};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::signal::unix::{signal, SignalKind};
use tokio_tungstenite::{accept_async, tungstenite::Message};

// ---- metrics ---------------------------------------------------------

static STARTED_AT: Lazy<Instant> = Lazy::new(Instant::now);

static WS_CONNECTIONS_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    let counter = IntCounter::new(
        "dd_rust_ws_connections_total",
        "Accepted WebSocket connections.",
    )
    .expect("dd_rust_ws_connections_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("register dd_rust_ws_connections_total");
    counter
});

static WS_DISCONNECTIONS_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    let counter = IntCounter::new(
        "dd_rust_ws_disconnections_total",
        "Closed WebSocket connections.",
    )
    .expect("dd_rust_ws_disconnections_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("register dd_rust_ws_disconnections_total");
    counter
});

static WS_HANDSHAKE_FAILED_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    let counter = IntCounter::new(
        "dd_rust_ws_handshake_failed_total",
        "Failed WebSocket handshakes (HTTP upgrade rejected).",
    )
    .expect("dd_rust_ws_handshake_failed_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("register dd_rust_ws_handshake_failed_total");
    counter
});

static WS_ACTIVE: Lazy<IntGauge> = Lazy::new(|| {
    let gauge = IntGauge::new(
        "dd_rust_ws_active",
        "Currently connected WebSocket clients.",
    )
    .expect("dd_rust_ws_active");
    prometheus::default_registry()
        .register(Box::new(gauge.clone()))
        .expect("register dd_rust_ws_active");
    gauge
});

static WS_SHARDS_LIVE: Lazy<IntGauge> = Lazy::new(|| {
    let gauge = IntGauge::new(
        "dd_rust_ws_shards_live",
        "Currently running gateway-shard accept loops.",
    )
    .expect("dd_rust_ws_shards_live");
    prometheus::default_registry()
        .register(Box::new(gauge.clone()))
        .expect("register dd_rust_ws_shards_live");
    gauge
});

static WS_MESSAGES: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "dd_rust_ws_messages_total",
            "WebSocket frames observed by direction and type.",
        ),
        &["direction", "kind"],
    )
    .expect("dd_rust_ws_messages_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("register dd_rust_ws_messages_total");
    counter
});

// ---- helpers ---------------------------------------------------------

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Pull the first `"<key>":"..."` value from a JSON-ish string without
/// allocating a serde_json::Value — this is the per-frame hot path so we
/// keep the parser as a constant-time substring scan.
fn extract_str_field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    // Look for `"<key>":` then skip whitespace/colon.
    let needle_quoted = format!("\"{}\"", key);
    let key_pos = text.find(&needle_quoted)?;
    let after_key = &text[key_pos + needle_quoted.len()..];
    // Skip optional whitespace then a colon then optional whitespace.
    let colon_rel = after_key.find(':')?;
    let after_colon = &after_key[colon_rel + 1..];
    let value_start = after_colon.find('"')? + 1;
    let value_region = &after_colon[value_start..];
    let value_end = value_region.find('"')?;
    Some(&value_region[..value_end])
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

// ---- WS connection handler -------------------------------------------

#[tracing::instrument(name = "ws_connection", skip_all, fields(peer = %peer))]
async fn handle_ws_connection(stream: TcpStream, peer: SocketAddr) {
    // Disable Nagle on every connection — these are bursty small JSON
    // frames where a 40 ms delay would dominate p50.
    let _ = stream.set_nodelay(true);

    let ws = match accept_async(stream).await {
        Ok(ws) => ws,
        Err(_) => {
            WS_HANDSHAKE_FAILED_TOTAL.inc();
            return;
        }
    };

    WS_CONNECTIONS_TOTAL.inc();
    WS_ACTIVE.inc();
    let _ = peer;

    let (mut tx, mut rx) = ws.split();

    while let Some(message) = rx.next().await {
        match message {
            Ok(Message::Text(text)) => {
                WS_MESSAGES.with_label_values(&["in", "text"]).inc();
                let id_opt = extract_str_field(&text, "id");
                let kind_opt = extract_str_field(&text, "type");

                let reply = match kind_opt {
                    Some("ping") => {
                        let id = id_opt.unwrap_or("");
                        format!(
                            r#"{{"type":"pong","id":"{}","ts":{}}}"#,
                            json_escape(id),
                            now_ms()
                        )
                    }
                    _ => match id_opt {
                        // akka-style envelope: echo the original id back
                        // inside `{ok:true, result:{id:...}}`. Lets the
                        // existing `LOAD_MODE=pipeline` loader correlate
                        // without any client-side changes.
                        Some(id) => format!(
                            r#"{{"ok":true,"result":{{"id":"{}"}},"ts":{}}}"#,
                            json_escape(id),
                            now_ms()
                        ),
                        // Plain text "ping" → pong-style reply with no id.
                        None if text == "ping" => {
                            format!(r#"{{"type":"pong","ts":{}}}"#, now_ms())
                        }
                        // Otherwise drop the frame silently — keeps the
                        // hot path branch-free for frames we don't care
                        // about (heartbeats, control, malformed JSON).
                        None => continue,
                    },
                };

                if tx.send(Message::Text(reply)).await.is_err() {
                    break;
                }
                WS_MESSAGES.with_label_values(&["out", "text"]).inc();
            }
            Ok(Message::Ping(payload)) => {
                WS_MESSAGES.with_label_values(&["in", "ws-ping"]).inc();
                if tx.send(Message::Pong(payload)).await.is_err() {
                    break;
                }
                WS_MESSAGES.with_label_values(&["out", "ws-pong"]).inc();
            }
            Ok(Message::Pong(_)) | Ok(Message::Binary(_)) | Ok(Message::Frame(_)) => {}
            Ok(Message::Close(_)) => break,
            Err(_) => break,
        }
    }

    WS_ACTIVE.dec();
    WS_DISCONNECTIONS_TOTAL.inc();
}

// ---- accept-side -----------------------------------------------------

fn make_listener(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let socket = match addr {
        SocketAddr::V4(_) => TcpSocket::new_v4()?,
        SocketAddr::V6(_) => TcpSocket::new_v6()?,
    };
    socket.set_reuseaddr(true)?;
    // SO_REUSEPORT is what enables the Dart-equivalent multi-acceptor
    // model: multiple TcpListeners can all bind 0.0.0.0:8097 and the
    // kernel hashes accepted SYNs across them. Without this, only the
    // first bind would succeed.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    socket.set_reuseport(true)?;
    socket.bind(addr)?;
    socket.listen(2048)
}

async fn accept_loop(listener: TcpListener) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                tokio::spawn(handle_ws_connection(stream, peer));
            }
            Err(_) => {
                // EMFILE / ENFILE / transient kernel errors — back off
                // briefly so we don't spin a CPU on the failing accept.
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

// ---- admin HTTP -------------------------------------------------------

async fn metrics_handler() -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::default_registry().gather();
    let mut buffer: Vec<u8> = Vec::with_capacity(4096);
    if encoder.encode(&metric_families, &mut buffer).is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, Vec::new()).into_response();
    }
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            encoder.format_type().to_string(),
        )],
        buffer,
    )
        .into_response()
}

async fn healthz_handler() -> impl IntoResponse {
    StatusCode::OK
}

async fn readyz_handler() -> impl IntoResponse {
    StatusCode::OK
}

// ---- main ------------------------------------------------------------

fn env_u16(name: &str, default_value: u16) -> u16 {
    env::var(name)
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default_value)
}

fn env_usize(name: &str, default_value: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default_value)
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let _otel = dd_telemetry::init("dd-rust-wss-server");

    Lazy::force(&STARTED_AT);
    Lazy::force(&WS_CONNECTIONS_TOTAL);
    Lazy::force(&WS_DISCONNECTIONS_TOTAL);
    Lazy::force(&WS_HANDSHAKE_FAILED_TOTAL);
    Lazy::force(&WS_ACTIVE);
    Lazy::force(&WS_SHARDS_LIVE);
    Lazy::force(&WS_MESSAGES);

    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let ws_port = env_u16("WS_PORT", 8097);
    let admin_port = env_u16("ADMIN_PORT", 8098);
    let shards = env_usize("WS_GATEWAY_SHARDS", 8);

    let ws_addr: SocketAddr = format!("{host}:{ws_port}")
        .parse()
        .expect("invalid WS_PORT/HOST");
    let admin_addr: SocketAddr = format!("{host}:{admin_port}")
        .parse()
        .expect("invalid ADMIN_PORT/HOST");

    tracing::error!(
        "dd-rust-wss-server starting host={host} ws_port={ws_port} admin_port={admin_port} shards={shards}"
    );

    let mut spawned = 0i64;
    for shard_id in 0..shards {
        match make_listener(ws_addr) {
            Ok(listener) => {
                tracing::error!("dd-rust-wss-server shard={shard_id} bound on {ws_addr}");
                tokio::spawn(accept_loop(listener));
                spawned += 1;
            }
            Err(error) => {
                tracing::error!("dd-rust-wss-server shard={shard_id} bind failed on {ws_addr}: {error}");
            }
        }
    }
    WS_SHARDS_LIVE.set(spawned);

    let admin_router = Router::new()
        .route("/healthz", get(healthz_handler))
        .route("/readyz", get(readyz_handler))
        .route("/metrics", get(metrics_handler));

    let admin_listener = TcpListener::bind(admin_addr)
        .await
        .expect("bind admin port");
    tracing::error!("dd-rust-wss-server admin listening on {admin_addr}");

    let admin = tokio::spawn(async move {
        if let Err(error) = axum::serve(
            admin_listener,
            admin_router.layer(dd_telemetry::http_trace_layer()),
        )
        .await
        {
            tracing::error!("admin serve error: {error}");
        }
    });

    let mut sigterm =
        signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut sigint =
        signal(SignalKind::interrupt()).expect("install SIGINT handler");

    tokio::select! {
        _ = sigterm.recv() => tracing::error!("received SIGTERM, exiting"),
        _ = sigint.recv() => tracing::error!("received SIGINT, exiting"),
        result = admin => {
            match result {
                Ok(()) => tracing::error!("admin server exited"),
                Err(error) => tracing::error!("admin server task panicked: {error}"),
            }
        }
    }

    let uptime_s = STARTED_AT.elapsed().as_secs_f64();
    tracing::error!(
        "dd-rust-wss-server stopping uptime_s={uptime_s:.1} active={} total_connected={} total_disconnected={}",
        WS_ACTIVE.get(),
        WS_CONNECTIONS_TOTAL.get(),
        WS_DISCONNECTIONS_TOTAL.get()
    );
    let _ = Ordering::SeqCst;
}
