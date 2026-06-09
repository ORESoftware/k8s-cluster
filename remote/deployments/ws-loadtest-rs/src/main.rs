use futures_util::{SinkExt, StreamExt};
use hdrhistogram::Histogram;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::process;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::{sleep, timeout, Duration, Instant};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

const DEFAULT_WS_URL: &str = "ws://dd-gleamlang-server.default.svc.cluster.local:8081/ws";
const DEFAULT_MESSAGE_ENCODINGS: &str = "json";
const DEFAULT_LOADTEST_TRANSPORTS: &str = "http,tcp,websocket";

/// LOAD_MODE values.
const LOAD_MODE_HOLD: &str = "hold";
const LOAD_MODE_PIPELINE: &str = "pipeline";
/// gcs mode drives the chat.vibe Go websocket server (via gcs-router) using its
/// real chat protocol rather than the akka-style echo frames. See the
/// `run_gcs_client` block below for the wire format.
const LOAD_MODE_GCS: &str = "gcs";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MessageEncoding {
    Json,
    MessagePack,
    Protobuf,
    FlatBuffers,
}

impl MessageEncoding {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "msgpack" | "messagepack" | "message-pack" => Some(Self::MessagePack),
            "protobuf" | "proto" | "protocol-buffers" | "protocol_buffers" => Some(Self::Protobuf),
            "flatbuffers" | "flatbuffer" | "flat-buffers" | "flat_buffers" => {
                Some(Self::FlatBuffers)
            }
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::MessagePack => "msgpack",
            Self::Protobuf => "protobuf",
            Self::FlatBuffers => "flatbuffers",
        }
    }
}

#[derive(Debug, Default)]
struct Stats {
    attempted: AtomicUsize,
    connected: AtomicUsize,
    failed: AtomicUsize,
    open: AtomicUsize,
    messages: AtomicUsize,
    // Pipeline-mode only. Populated by per-client send/recv loops and reset every report.
    sent: AtomicUsize,
    received: AtomicUsize,
    receive_errors: AtomicUsize,
    correlation_misses: AtomicUsize,
    in_flight: AtomicUsize,
    // Total micros to track simple mean without scanning the histogram on every report.
    total_latency_us: AtomicU64,
    connect_error_logs: AtomicUsize,
}

impl Stats {
    fn record_latency_us(&self, latency_us: u64, latencies: &Mutex<Histogram<u64>>) {
        self.received.fetch_add(1, Ordering::Relaxed);
        self.total_latency_us
            .fetch_add(latency_us, Ordering::Relaxed);
        if let Ok(mut hist) = latencies.lock() {
            let _ = hist.record(latency_us);
        }
    }

    fn should_log_connect_error(&self) -> bool {
        let n = self.connect_error_logs.fetch_add(1, Ordering::Relaxed) + 1;
        n <= 20 || n % 10_000 == 0
    }
}

#[derive(Debug)]
struct Config {
    target_ws_url: String,
    client_count: usize,
    hold_seconds: u64,
    connect_timeout_seconds: u64,
    receive_timeout_seconds: u64,
    reconnect_delay_ms: u64,
    ramp_delay_ms: u64,
    report_interval_seconds: u64,
    load_mode: String,
    /// Pipeline-mode: messages per second per client. Translated into a per-client send interval.
    messages_per_second_per_client: f64,
    /// Pipeline-mode: per-message payload string. Defaults to a short sample.
    message_payload: String,
    /// Pipeline-mode: one or more encodings to round-robin over for the
    /// `{id, payload}` message model.
    message_encodings: Vec<MessageEncoding>,
    /// Advertised protocol coverage for deployment/runbook automation.
    loadtest_transports: String,
    /// Pipeline-mode: drop unmatched-request entries older than this (memory bound on
    /// pending-request map).
    correlation_timeout_seconds: u64,
    /// gcs-mode: how many websocket clients share a single conversation (drives
    /// conv-hash routing in gcs-router and the fan-out factor).
    gcs_clients_per_conv: usize,
    /// gcs-mode: how many clients per conversation actually send (0 => all send).
    gcs_senders_per_conv: usize,
    /// gcs-mode: hot-path wire format for MongoChatMessage frames.
    gcs_message_encoding: MessageEncoding,
}

fn env_usize(name: &str, default_value: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default_value)
}

fn env_u64(name: &str, default_value: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default_value)
}

fn env_f64(name: &str, default_value: f64) -> f64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| *value > 0.0)
        .unwrap_or(default_value)
}

fn env_csv(name: &str, default_value: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default_value.to_string())
}

fn parse_message_encodings() -> Vec<MessageEncoding> {
    let raw = env::var("MESSAGE_ENCODINGS")
        .ok()
        .or_else(|| env::var("MESSAGE_ENCODING").ok())
        .unwrap_or_else(|| DEFAULT_MESSAGE_ENCODINGS.to_string());
    let mut encodings = Vec::new();
    for part in raw.split(',') {
        if let Some(encoding) = MessageEncoding::parse(part) {
            if !encodings.contains(&encoding) {
                encodings.push(encoding);
            }
        }
    }
    if encodings.is_empty() {
        encodings.push(MessageEncoding::Json);
    }
    encodings
}

fn parse_gcs_message_encoding() -> MessageEncoding {
    env::var("GCS_MESSAGE_ENCODING")
        .ok()
        .or_else(|| env::var("MESSAGE_ENCODING").ok())
        .and_then(|value| MessageEncoding::parse(&value))
        .filter(|encoding| matches!(encoding, MessageEncoding::Json | MessageEncoding::Protobuf))
        .unwrap_or(MessageEncoding::Json)
}

fn format_message_encodings(encodings: &[MessageEncoding]) -> String {
    encodings
        .iter()
        .map(|encoding| encoding.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn load_config() -> Config {
    Config {
        target_ws_url: env::var("TARGET_WS_URL").unwrap_or_else(|_| DEFAULT_WS_URL.to_string()),
        client_count: env_usize("CLIENT_COUNT", 5_000),
        hold_seconds: env_u64("HOLD_SECONDS", 300),
        connect_timeout_seconds: env_u64("CONNECT_TIMEOUT_SECONDS", 20),
        receive_timeout_seconds: env_u64("RECEIVE_TIMEOUT_SECONDS", 5),
        reconnect_delay_ms: env_u64("RECONNECT_DELAY_MS", 1_000),
        ramp_delay_ms: env_u64("RAMP_DELAY_MS", 1),
        report_interval_seconds: env_u64("REPORT_INTERVAL_SECONDS", 10),
        load_mode: env::var("LOAD_MODE").unwrap_or_else(|_| LOAD_MODE_HOLD.to_string()),
        messages_per_second_per_client: env_f64("MESSAGES_PER_SECOND_PER_CLIENT", 10.0),
        message_payload: env::var("MESSAGE_PAYLOAD")
            .unwrap_or_else(|_| "a benchmark message body".to_string()),
        message_encodings: parse_message_encodings(),
        loadtest_transports: env_csv("LOADTEST_TRANSPORTS", DEFAULT_LOADTEST_TRANSPORTS),
        correlation_timeout_seconds: env_u64("CORRELATION_TIMEOUT_SECONDS", 10),
        gcs_clients_per_conv: env_usize("GCS_CLIENTS_PER_CONV", 5),
        // env_usize filters out 0, so an unset/zero value falls through to this
        // 0 sentinel meaning "every client in the conversation sends".
        gcs_senders_per_conv: env::var("GCS_SENDERS_PER_CONV")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0),
        gcs_message_encoding: parse_gcs_message_encoding(),
    }
}

fn container_pool_dispatch_url() -> Option<String> {
    let base_url = env::var("CONTAINER_POOL_URL").ok()?;
    let route_prefix =
        env::var("CONTAINER_POOL_ROUTE_PREFIX").unwrap_or_else(|_| "/pools".to_string());
    let pool = env::var("CONTAINER_POOL_POOL").unwrap_or_else(|_| "rust".to_string());
    Some(format!(
        "{}{}/{}/dispatch",
        base_url.trim_end_matches('/'),
        route_prefix,
        pool
    ))
}

fn smoke_key() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = process::id() as u128;
    let value = nanos ^ (pid << 64);
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (value >> 96) as u32,
        (value >> 80) as u16,
        (value >> 64) as u16,
        (value >> 48) as u16,
        value & 0x0000_ffff_ffff_ffff_ffff
    )
}

async fn run_container_pool_smoke() -> Result<(), String> {
    let Some(url) = container_pool_dispatch_url() else {
        return Ok(());
    };
    let echo_key = env::var("CONTAINER_POOL_ECHO_KEY").unwrap_or_else(|_| smoke_key());
    let timeout_seconds = env_u64("CONTAINER_POOL_TIMEOUT_SECONDS", 30);
    println!(
        "ws-loadtest-rs container-pool-smoke starting url={} echo_key={} timeout_seconds={}",
        url, echo_key, timeout_seconds
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_seconds))
        .build()
        .map_err(|error| error.to_string())?;
    let mut request = client.post(url).json(&json!({
        "requestId": echo_key,
        "payload": {
            "echoKey": echo_key,
            "client": "ws-loadtest-rs"
        }
    }));
    if let Ok(secret) = env::var("CONTAINER_POOL_AUTH_SECRET") {
        request = request.header("x-server-auth", secret);
    }

    let response = request.send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response
        .json::<Value>()
        .await
        .map_err(|error| error.to_string())?;
    let returned_key = body
        .pointer("/body/echoKey")
        .or_else(|| body.pointer("/body/request/echoKey"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !status.is_success() || returned_key != echo_key {
        return Err(format!(
            "unexpected container-pool response status={} returned_key={} body={}",
            status, returned_key, body
        ));
    }
    println!(
        "ws-loadtest-rs container-pool-smoke ok pool={} container={} echo_key={}",
        body.pointer("/poolSlug")
            .and_then(Value::as_str)
            .unwrap_or(""),
        body.pointer("/containerName")
            .and_then(Value::as_str)
            .unwrap_or(""),
        returned_key
    );
    Ok(())
}

/// Pipeline-mode client: sends shaped JSON messages at a fixed per-client rate and correlates
/// responses by id so we can measure end-to-end round-trip latency.
///
/// The id format is `c<client>-<seq>` so each request is globally unique across all clients,
/// which lets the response correlator match the response's `id` field back to the per-id
/// send timestamp.
async fn run_pipeline_client(
    client_id: usize,
    config: Arc<Config>,
    stats: Arc<Stats>,
    latencies: Arc<Mutex<Histogram<u64>>>,
) -> ! {
    let connect_timeout = Duration::from_secs(config.connect_timeout_seconds);
    let reconnect_delay = Duration::from_millis(config.reconnect_delay_ms);
    let correlation_timeout = Duration::from_secs(config.correlation_timeout_seconds);
    // 1 / rate => seconds between sends; convert to nanoseconds for tokio's interval.
    let send_interval =
        Duration::from_nanos((1_000_000_000.0 / config.messages_per_second_per_client) as u64);

    let payload = config.message_payload.clone();
    let encodings = config.message_encodings.clone();

    loop {
        stats.attempted.fetch_add(1, Ordering::Relaxed);
        let connect_result = timeout(connect_timeout, connect_async(&config.target_ws_url)).await;

        match connect_result {
            Ok(Ok((socket, _response))) => {
                stats.connected.fetch_add(1, Ordering::Relaxed);
                stats.open.fetch_add(1, Ordering::Relaxed);

                // Split for concurrent send + receive on the same connection.
                let (mut writer, mut reader) = socket.split();
                let pending: Arc<Mutex<HashMap<String, Instant>>> =
                    Arc::new(Mutex::new(HashMap::new()));

                // Sender task: paces messages at the configured rate.
                let send_pending = Arc::clone(&pending);
                let send_stats = Arc::clone(&stats);
                let send_payload = payload.clone();
                let send_encodings = encodings.clone();
                let sender = tokio::spawn(async move {
                    let mut seq: u64 = 0;
                    let mut ticker = tokio::time::interval(send_interval);
                    // Skip the initial tick that fires immediately so the rate is honoured.
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    loop {
                        ticker.tick().await;
                        seq = seq.wrapping_add(1);
                        let id = format!("c{client_id}-{seq}");
                        let encoding = send_encodings[(seq as usize - 1) % send_encodings.len()];
                        let frame = encode_pipeline_message(&id, &send_payload, encoding);
                        if let Ok(mut map) = send_pending.lock() {
                            map.insert(id.clone(), Instant::now());
                            send_stats.in_flight.store(map.len(), Ordering::Relaxed);
                        }
                        if writer.send(frame).await.is_err() {
                            break;
                        }
                        send_stats.sent.fetch_add(1, Ordering::Relaxed);
                    }
                });

                // Receiver task: parses each frame, extracts `id`, matches against pending map.
                while let Some(message) = reader.next().await {
                    match message {
                        Ok(message) => {
                            stats.messages.fetch_add(1, Ordering::Relaxed);
                            if let Some(id) = extract_id_from_message(&message) {
                                let sent_at_opt = pending.lock().ok().and_then(|mut map| {
                                    let v = map.remove(&id);
                                    stats.in_flight.store(map.len(), Ordering::Relaxed);
                                    v
                                });
                                if let Some(sent_at) = sent_at_opt {
                                    let latency_us = sent_at.elapsed().as_micros() as u64;
                                    stats.record_latency_us(latency_us, &latencies);
                                } else {
                                    stats.correlation_misses.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                        Err(_) => {
                            stats.receive_errors.fetch_add(1, Ordering::Relaxed);
                            break;
                        }
                    }
                    // Drop very-old pending entries to bound memory if the server is slow.
                    if let Ok(mut map) = pending.lock() {
                        let cutoff = Instant::now() - correlation_timeout;
                        map.retain(|_, sent_at| *sent_at > cutoff);
                    }
                }

                sender.abort();
                stats.open.fetch_sub(1, Ordering::Relaxed);
            }
            Ok(Err(error)) => {
                stats.failed.fetch_add(1, Ordering::Relaxed);
                if stats.should_log_connect_error() {
                    eprintln!("connect failed client={} error={}", client_id, error);
                }
            }
            Err(_elapsed) => {
                stats.failed.fetch_add(1, Ordering::Relaxed);
                if stats.should_log_connect_error() {
                    eprintln!("connect timeout client={}", client_id);
                }
            }
        }

        sleep(reconnect_delay).await;
    }
}

/// Best-effort extraction of `id` from the response JSON. The akka-ws-server returns
/// `{"ok":true,"result":{"id":"<original>", ...}}`; we only need the original id.
/// Avoids a full serde_json::from_str on every frame.
fn extract_id(frame: &str) -> Option<String> {
    let needle = "\"id\":\"";
    let start = frame.find(needle)? + needle.len();
    let rest = &frame[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn encode_pipeline_message(id: &str, payload: &str, encoding: MessageEncoding) -> Message {
    match encoding {
        MessageEncoding::Json => Message::Text(format!(
            r#"{{"id":"{}","payload":"{}"}}"#,
            json_escape(id),
            json_escape(payload)
        )),
        MessageEncoding::MessagePack => {
            Message::Binary(encode_msgpack_pipeline_message(id, payload))
        }
        MessageEncoding::Protobuf => Message::Binary(encode_protobuf_pipeline_message(id, payload)),
        MessageEncoding::FlatBuffers => {
            Message::Binary(encode_flatbuffers_pipeline_message(id, payload))
        }
    }
}

fn extract_id_from_message(message: &Message) -> Option<String> {
    match message {
        Message::Text(text) => extract_id(text),
        Message::Binary(bytes) => extract_id_from_binary(bytes)
            .or_else(|| std::str::from_utf8(bytes).ok().and_then(extract_id)),
        _ => None,
    }
}

fn extract_id_from_binary(bytes: &[u8]) -> Option<String> {
    extract_id_msgpack(bytes)
        .or_else(|| extract_id_protobuf(bytes))
        .or_else(|| extract_id_flatbuffers(bytes))
}

fn encode_msgpack_pipeline_message(id: &str, payload: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(id.len() + payload.len() + 16);
    out.push(0x82);
    push_msgpack_str(&mut out, "id");
    push_msgpack_str(&mut out, id);
    push_msgpack_str(&mut out, "payload");
    push_msgpack_str(&mut out, payload);
    out
}

fn push_msgpack_str(out: &mut Vec<u8>, value: &str) {
    let len = value.len();
    if len <= 31 {
        out.push(0xa0 | len as u8);
    } else if len <= u8::MAX as usize {
        out.extend_from_slice(&[0xd9, len as u8]);
    } else if len <= u16::MAX as usize {
        out.push(0xda);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0xdb);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
    out.extend_from_slice(value.as_bytes());
}

fn read_msgpack_str(bytes: &[u8], index: &mut usize) -> Option<String> {
    let tag = *bytes.get(*index)?;
    *index += 1;
    let len = match tag {
        0xa0..=0xbf => (tag & 0x1f) as usize,
        0xd9 => {
            let len = *bytes.get(*index)? as usize;
            *index += 1;
            len
        }
        0xda => {
            let len = u16::from_be_bytes([*bytes.get(*index)?, *bytes.get(*index + 1)?]) as usize;
            *index += 2;
            len
        }
        0xdb => {
            let len = u32::from_be_bytes([
                *bytes.get(*index)?,
                *bytes.get(*index + 1)?,
                *bytes.get(*index + 2)?,
                *bytes.get(*index + 3)?,
            ]) as usize;
            *index += 4;
            len
        }
        _ => return None,
    };
    let end = index.checked_add(len)?;
    let value = std::str::from_utf8(bytes.get(*index..end)?)
        .ok()?
        .to_string();
    *index = end;
    Some(value)
}

fn extract_id_msgpack(bytes: &[u8]) -> Option<String> {
    let mut index = 0usize;
    let tag = *bytes.get(index)?;
    index += 1;
    let pairs = match tag {
        0x80..=0x8f => (tag & 0x0f) as usize,
        0xde => {
            let len = u16::from_be_bytes([*bytes.get(index)?, *bytes.get(index + 1)?]) as usize;
            index += 2;
            len
        }
        0xdf => {
            let len = u32::from_be_bytes([
                *bytes.get(index)?,
                *bytes.get(index + 1)?,
                *bytes.get(index + 2)?,
                *bytes.get(index + 3)?,
            ]) as usize;
            index += 4;
            len
        }
        _ => return None,
    };

    for _ in 0..pairs {
        let key = read_msgpack_str(bytes, &mut index)?;
        let value = read_msgpack_str(bytes, &mut index)?;
        if key == "id" {
            return Some(value);
        }
    }
    None
}

fn encode_protobuf_pipeline_message(id: &str, payload: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(id.len() + payload.len() + 8);
    push_protobuf_string_field(&mut out, 1, id);
    push_protobuf_string_field(&mut out, 2, payload);
    out
}

fn push_protobuf_string_field(out: &mut Vec<u8>, field_number: u64, value: &str) {
    push_varint(out, (field_number << 3) | 2);
    push_varint(out, value.len() as u64);
    out.extend_from_slice(value.as_bytes());
}

fn push_protobuf_bytes_field(out: &mut Vec<u8>, field_number: u64, value: &[u8]) {
    push_varint(out, (field_number << 3) | 2);
    push_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn push_protobuf_varint_field(out: &mut Vec<u8>, field_number: u64, value: u64) {
    if value == 0 {
        return;
    }
    push_varint(out, field_number << 3);
    push_varint(out, value);
}

fn push_protobuf_bool_field(out: &mut Vec<u8>, field_number: u64, value: bool) {
    if !value {
        return;
    }
    push_protobuf_varint_field(out, field_number, 1);
}

fn push_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_varint(bytes: &[u8], index: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        let byte = *bytes.get(*index)?;
        *index += 1;
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

fn extract_id_protobuf(bytes: &[u8]) -> Option<String> {
    let mut index = 0usize;
    while index < bytes.len() {
        let key = read_varint(bytes, &mut index)?;
        let field_number = key >> 3;
        let wire_type = key & 0x07;
        match wire_type {
            0 => {
                let _ = read_varint(bytes, &mut index)?;
            }
            1 => {
                index = index.checked_add(8)?;
            }
            2 => {
                let len = read_varint(bytes, &mut index)? as usize;
                let end = index.checked_add(len)?;
                let value = bytes.get(index..end)?;
                if field_number == 1 {
                    return Some(std::str::from_utf8(value).ok()?.to_string());
                }
                index = end;
            }
            5 => {
                index = index.checked_add(4)?;
            }
            _ => return None,
        }
    }
    None
}

fn encode_flatbuffers_pipeline_message(id: &str, payload: &str) -> Vec<u8> {
    let mut out = vec![0u8; 24];
    write_u32_le(&mut out, 0, 12);
    write_u16_le(&mut out, 4, 8);
    write_u16_le(&mut out, 6, 12);
    write_u16_le(&mut out, 8, 4);
    write_u16_le(&mut out, 10, 8);
    write_i32_le(&mut out, 12, 8);

    let id_start = append_flatbuffers_string(&mut out, id);
    write_u32_le(&mut out, 16, (id_start - 16) as u32);
    let payload_start = append_flatbuffers_string(&mut out, payload);
    write_u32_le(&mut out, 20, (payload_start - 20) as u32);
    out
}

fn append_flatbuffers_string(out: &mut Vec<u8>, value: &str) -> usize {
    let start = out.len();
    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
    out.push(0);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    start
}

fn write_u16_le(out: &mut [u8], offset: usize, value: u16) {
    out[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32_le(out: &mut [u8], offset: usize, value: u32) {
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_i32_le(out: &mut [u8], offset: usize, value: i32) {
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
    ]))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
        *bytes.get(offset + 2)?,
        *bytes.get(offset + 3)?,
    ]))
}

fn read_i32_le(bytes: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_le_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
        *bytes.get(offset + 2)?,
        *bytes.get(offset + 3)?,
    ]))
}

fn read_flatbuffers_string(
    bytes: &[u8],
    table: usize,
    vtable: usize,
    field: usize,
) -> Option<String> {
    let vtable_len = read_u16_le(bytes, vtable)? as usize;
    let field_offset_pos = vtable.checked_add(4 + field * 2)?;
    if field_offset_pos + 2 > vtable + vtable_len {
        return None;
    }
    let field_offset = read_u16_le(bytes, field_offset_pos)? as usize;
    if field_offset == 0 {
        return None;
    }
    let slot = table.checked_add(field_offset)?;
    let string_start = slot.checked_add(read_u32_le(bytes, slot)? as usize)?;
    let len = read_u32_le(bytes, string_start)? as usize;
    let data_start = string_start.checked_add(4)?;
    let data_end = data_start.checked_add(len)?;
    Some(
        std::str::from_utf8(bytes.get(data_start..data_end)?)
            .ok()?
            .to_string(),
    )
}

fn extract_id_flatbuffers(bytes: &[u8]) -> Option<String> {
    let table = read_u32_le(bytes, 0)? as usize;
    let vtable_offset = read_i32_le(bytes, table)?;
    let vtable = if vtable_offset >= 0 {
        table.checked_sub(vtable_offset as usize)?
    } else {
        table.checked_add((-vtable_offset) as usize)?
    };
    read_flatbuffers_string(bytes, table, vtable, 0)
}

// ---------------------------------------------------------------------------
// GCS (chat.vibe) mode.
//
// Unlike the akka-style {"id","payload"} echo protocol, the chat.vibe Go server
// (reached via gcs-router:3001) expects:
//   * a connect URL with query params:
//       /gcs/ws/?userId=<oid>&deviceId=<oid>&conversationIds=<urlenc(["<oid>"])>
//     (one conv id => gcs-router pins the connection to a pod via conv-hash)
//   * application frames shaped as
//       {"Meta":{}, "List":[{"@vibe-meta":{}, "@vibe-type":"MongoChatMessage",
//                            "@vibe-data":"<inner MongoChatMessage JSON string>"}]}
//   * the server fans every accepted message out to ALL clients subscribed to
//     that conversation as {"Meta":{...}, "Messages":[ <message> ]}.
//
// Each sender embeds its send time (µs) in the message marker so any receiver
// can compute true end-to-end fan-out latency (all clients share this process's
// wall clock). No auth is required in-cluster.
// ---------------------------------------------------------------------------

static OID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 24-hex-char value shaped like a Mongo ObjectId (8 hex seconds + 16 hex of
/// process/counter entropy). Unique within the process; the server only needs a
/// valid non-zero ObjectId.
fn object_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let ts = now.as_secs() as u32;
    let ctr = OID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = process::id() as u64;
    let lo = (now.subsec_nanos() as u64) ^ (pid << 32) ^ ctr.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    format!("{:08x}{:016x}", ts, lo)
}

/// RFC3339 UTC timestamp with millisecond precision (e.g. 2026-05-31T20:50:00.123Z).
/// gcs unmarshals CreatedAt/DateFirstOnServer into Go time.Time, which requires
/// valid RFC3339; hand-rolled to avoid a chrono dep in the on-pod cargo build.
fn rfc3339_now() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (hour, min, sec) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y, m, d, hour, min, sec, millis
    )
}

/// Howard Hinnant's days->civil algorithm; `z` is days since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Percent-encode per RFC3986 unreserved set; used for the conversationIds JSON.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn gcs_connect_url(
    base: &str,
    user_id: &str,
    device_id: &str,
    conv_id: &str,
    encoding: MessageEncoding,
) -> String {
    let conv_ids_json = format!("[\"{}\"]", conv_id);
    let wire = if encoding == MessageEncoding::Protobuf {
        "&wire=protobuf"
    } else {
        ""
    };
    format!(
        "{}/gcs/ws/?userId={}&deviceId={}&conversationIds={}{}",
        base.trim_end_matches('/'),
        user_id,
        device_id,
        url_encode(&conv_ids_json),
        wire
    )
}

/// Build the outer envelope carrying one MongoChatMessage. `marker` goes into
/// `Messages[0]` and is the latency beacon receivers parse.
fn build_gcs_chat_frame(
    conv_id: &str,
    user_id: &str,
    members: &[String],
    marker: &str,
    encoding: MessageEncoding,
) -> Message {
    if encoding == MessageEncoding::Protobuf {
        return Message::Binary(encode_gcs_protobuf_chat_frame(
            conv_id, user_id, members, marker,
        ));
    }

    let now = rfc3339_now();
    let priority = members
        .iter()
        .map(|u| format!("\"{}\"", u))
        .collect::<Vec<_>>()
        .join(",");
    let inner = format!(
        r#"{{"_id":"{id}","IsGroupChat":{group},"PriorityUserIds":[{priority}],"CreatedByUserId":"{u}","CreatedBy":"{u}","CreatedAt":"{now}","ChatId":"{conv}","Messages":["{marker}"],"DateCreatedOnDevice":"{now}","DateFirstOnServer":"{now}"}}"#,
        id = object_id(),
        group = members.len() > 2,
        priority = priority,
        u = user_id,
        now = now,
        conv = conv_id,
        marker = marker
    );
    Message::Text(format!(
        r#"{{"Meta":{{}},"List":[{{"@vibe-meta":{{}},"@vibe-type":"MongoChatMessage","@vibe-data":"{}"}}]}}"#,
        json_escape(&inner)
    ))
}

fn encode_gcs_protobuf_chat_frame(
    conv_id: &str,
    user_id: &str,
    members: &[String],
    marker: &str,
) -> Vec<u8> {
    let now_ms = now_millis();
    let mut message = Vec::with_capacity(256 + marker.len() + members.len() * 28);
    push_protobuf_string_field(&mut message, 1, &object_id());
    push_protobuf_string_field(&mut message, 3, user_id);
    push_protobuf_string_field(&mut message, 4, conv_id);
    for member in members {
        push_protobuf_string_field(&mut message, 5, member);
    }
    push_protobuf_bool_field(&mut message, 7, members.len() > 2);
    push_protobuf_varint_field(&mut message, 9, members.len() as u64);
    push_protobuf_string_field(&mut message, 10, marker);
    push_protobuf_varint_field(&mut message, 15, now_ms);
    push_protobuf_varint_field(&mut message, 16, now_ms);
    push_protobuf_string_field(&mut message, 23, user_id);
    push_protobuf_varint_field(&mut message, 25, now_ms);

    let mut frame = Vec::with_capacity(message.len() + 32);
    push_protobuf_string_field(&mut frame, 1, "MongoChatMessage");
    push_protobuf_bytes_field(&mut frame, 2, &message);
    frame
}

/// Scan a received frame for our `gcsrt-<client>-<seq>-<sendMicros>` markers,
/// pushing an end-to-end latency (µs) for each one found.
fn parse_gcs_markers(text: &str, out: &mut Vec<u64>) {
    parse_gcs_marker_bytes(text.as_bytes(), out);
}

fn parse_gcs_marker_bytes(bytes: &[u8], out: &mut Vec<u64>) {
    let needle = "gcsrt-";
    let mut idx = 0usize;
    while let Some(pos) = find_bytes(&bytes[idx..], needle.as_bytes()) {
        let start = idx + pos;
        let rest = &bytes[start..];
        if let Some((marker_len, send_us)) = parse_gcs_marker_send_us(rest) {
            out.push(now_micros().saturating_sub(send_us));
            idx = start + marker_len;
        } else {
            idx = start + needle.len();
        }
    }
}

fn parse_gcs_marker_send_us(bytes: &[u8]) -> Option<(usize, u64)> {
    let prefix = b"gcsrt-";
    if !bytes.starts_with(prefix) {
        return None;
    }

    let mut idx = prefix.len();
    idx = consume_ascii_digits(bytes, idx)?;
    if *bytes.get(idx)? != b'-' {
        return None;
    }
    idx += 1;
    idx = consume_ascii_digits(bytes, idx)?;
    if *bytes.get(idx)? != b'-' {
        return None;
    }
    idx += 1;

    let micros_start = idx;
    idx = consume_ascii_digits(bytes, idx)?;
    let send_us = parse_ascii_u64(&bytes[micros_start..idx])?;
    Some((idx, send_us))
}

fn consume_ascii_digits(bytes: &[u8], mut idx: usize) -> Option<usize> {
    let start = idx;
    while matches!(bytes.get(idx), Some(b'0'..=b'9')) {
        idx += 1;
    }
    if idx == start {
        None
    } else {
        Some(idx)
    }
}

fn parse_ascii_u64(bytes: &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for &b in bytes {
        let digit = (b as char).to_digit(10)? as u64;
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    Some(value)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn reported_received_count(load_mode: &str, messages: usize, latency_samples: usize) -> usize {
    if load_mode == LOAD_MODE_GCS {
        messages
    } else {
        latency_samples
    }
}

struct GcsAssignment {
    conv_id: String,
    user_id: String,
    device_id: String,
    members: Arc<Vec<String>>,
    is_sender: bool,
}

/// Pre-compute conversations, members and per-client roles for gcs mode.
fn build_gcs_assignments(config: &Config) -> Vec<Arc<GcsAssignment>> {
    let clients_per_conv = config.gcs_clients_per_conv.max(1);
    let senders_per_conv = if config.gcs_senders_per_conv == 0 {
        clients_per_conv
    } else {
        config.gcs_senders_per_conv.min(clients_per_conv)
    };
    let conv_count = (config.client_count + clients_per_conv - 1) / clients_per_conv;

    let mut members: Vec<Vec<String>> = vec![Vec::new(); conv_count];
    for i in 0..config.client_count {
        members[i / clients_per_conv].push(object_id());
    }
    let conv_ids: Vec<String> = (0..conv_count).map(|_| object_id()).collect();
    let members_arc: Vec<Arc<Vec<String>>> = members.into_iter().map(Arc::new).collect();

    (0..config.client_count)
        .map(|i| {
            let c = i / clients_per_conv;
            let idx = i % clients_per_conv;
            Arc::new(GcsAssignment {
                conv_id: conv_ids[c].clone(),
                user_id: members_arc[c][idx].clone(),
                device_id: object_id(),
                members: Arc::clone(&members_arc[c]),
                is_sender: idx < senders_per_conv,
            })
        })
        .collect()
}

/// Shared receive loop: count frames and record fan-out latency from markers.
async fn gcs_read_loop<S>(mut reader: S, stats: &Stats, latencies: &Mutex<Histogram<u64>>)
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let mut lat_buf: Vec<u64> = Vec::new();
    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Text(text)) => {
                stats.messages.fetch_add(1, Ordering::Relaxed);
                lat_buf.clear();
                parse_gcs_markers(&text, &mut lat_buf);
                for latency_us in lat_buf.drain(..) {
                    stats.record_latency_us(latency_us, latencies);
                }
            }
            Ok(Message::Binary(bytes)) => {
                stats.messages.fetch_add(1, Ordering::Relaxed);
                lat_buf.clear();
                parse_gcs_marker_bytes(&bytes, &mut lat_buf);
                for latency_us in lat_buf.drain(..) {
                    stats.record_latency_us(latency_us, latencies);
                }
            }
            Ok(_) => {}
            Err(_) => {
                stats.receive_errors.fetch_add(1, Ordering::Relaxed);
                break;
            }
        }
    }
}

async fn run_gcs_client(
    client_id: usize,
    assignment: Arc<GcsAssignment>,
    config: Arc<Config>,
    stats: Arc<Stats>,
    latencies: Arc<Mutex<Histogram<u64>>>,
) -> ! {
    let connect_timeout = Duration::from_secs(config.connect_timeout_seconds);
    let reconnect_delay = Duration::from_millis(config.reconnect_delay_ms);
    let send_interval =
        Duration::from_nanos((1_000_000_000.0 / config.messages_per_second_per_client) as u64);
    let url = gcs_connect_url(
        &config.target_ws_url,
        &assignment.user_id,
        &assignment.device_id,
        &assignment.conv_id,
        config.gcs_message_encoding,
    );

    loop {
        stats.attempted.fetch_add(1, Ordering::Relaxed);
        match timeout(connect_timeout, connect_async(&url)).await {
            Ok(Ok((socket, _response))) => {
                stats.connected.fetch_add(1, Ordering::Relaxed);
                stats.open.fetch_add(1, Ordering::Relaxed);

                if assignment.is_sender {
                    let (mut writer, reader) = socket.split();
                    let send_stats = Arc::clone(&stats);
                    let conv = assignment.conv_id.clone();
                    let user = assignment.user_id.clone();
                    let members = Arc::clone(&assignment.members);
                    let encoding = config.gcs_message_encoding;
                    let sender = tokio::spawn(async move {
                        let mut seq: u64 = 0;
                        let mut ticker = tokio::time::interval(send_interval);
                        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        loop {
                            ticker.tick().await;
                            seq = seq.wrapping_add(1);
                            let marker = format!("gcsrt-{}-{}-{}", client_id, seq, now_micros());
                            let frame =
                                build_gcs_chat_frame(&conv, &user, &members, &marker, encoding);
                            if writer.send(frame).await.is_err() {
                                break;
                            }
                            send_stats.sent.fetch_add(1, Ordering::Relaxed);
                        }
                    });
                    gcs_read_loop(reader, &stats, &latencies).await;
                    sender.abort();
                } else {
                    gcs_read_loop(socket, &stats, &latencies).await;
                }

                stats.open.fetch_sub(1, Ordering::Relaxed);
            }
            Ok(Err(error)) => {
                stats.failed.fetch_add(1, Ordering::Relaxed);
                if stats.should_log_connect_error() {
                    eprintln!("gcs connect failed client={} error={}", client_id, error);
                }
            }
            Err(_elapsed) => {
                stats.failed.fetch_add(1, Ordering::Relaxed);
                if stats.should_log_connect_error() {
                    eprintln!("gcs connect timeout client={}", client_id);
                }
            }
        }
        sleep(reconnect_delay).await;
    }
}

async fn run_client(client_id: usize, config: Arc<Config>, stats: Arc<Stats>) -> ! {
    let connect_timeout = Duration::from_secs(config.connect_timeout_seconds);
    let receive_timeout = Duration::from_secs(config.receive_timeout_seconds);
    let reconnect_delay = Duration::from_millis(config.reconnect_delay_ms);
    let hold_duration = Duration::from_secs(config.hold_seconds);

    loop {
        stats.attempted.fetch_add(1, Ordering::Relaxed);
        let connect_result = timeout(connect_timeout, connect_async(&config.target_ws_url)).await;

        match connect_result {
            Ok(Ok((mut socket, _response))) => {
                stats.connected.fetch_add(1, Ordering::Relaxed);
                stats.open.fetch_add(1, Ordering::Relaxed);

                let _ = socket
                    .send(Message::Text(format!("ping-rs-{client_id}")))
                    .await;

                let deadline = Instant::now() + hold_duration;
                while Instant::now() < deadline {
                    match timeout(receive_timeout, socket.next()).await {
                        Ok(Some(Ok(_message))) => {
                            stats.messages.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok(Some(Err(_))) | Ok(None) => {
                            break;
                        }
                        Err(_) => {
                            // No message within timeout; keep connection open.
                        }
                    }
                }

                let _ = socket.close(None).await;
                stats.open.fetch_sub(1, Ordering::Relaxed);
            }
            Ok(Err(error)) => {
                stats.failed.fetch_add(1, Ordering::Relaxed);
                if stats.should_log_connect_error() {
                    eprintln!("connect failed client={} error={}", client_id, error);
                }
            }
            Err(_elapsed) => {
                stats.failed.fetch_add(1, Ordering::Relaxed);
                if stats.should_log_connect_error() {
                    eprintln!("connect timeout client={}", client_id);
                }
            }
        }

        sleep(reconnect_delay).await;
    }
}

async fn report_stats(
    config: Arc<Config>,
    stats: Arc<Stats>,
    latencies: Arc<Mutex<Histogram<u64>>>,
) -> ! {
    let period = Duration::from_secs(config.report_interval_seconds);
    loop {
        sleep(period).await;
        let attempted = stats.attempted.load(Ordering::Relaxed);
        let connected = stats.connected.load(Ordering::Relaxed);
        let failed = stats.failed.load(Ordering::Relaxed);
        let open = stats.open.load(Ordering::Relaxed);
        let messages = stats.messages.load(Ordering::Relaxed);

        // pipeline and gcs both produce per-message latency samples.
        if config.load_mode == LOAD_MODE_PIPELINE || config.load_mode == LOAD_MODE_GCS {
            // Take a snapshot of the histogram so the read doesn't race with concurrent writes.
            let (p50, p95, p99, max, sample_count, mean_us) = match latencies.lock() {
                Ok(hist) => {
                    let n = hist.len();
                    (
                        hist.value_at_quantile(0.50),
                        hist.value_at_quantile(0.95),
                        hist.value_at_quantile(0.99),
                        hist.max(),
                        n,
                        hist.mean(),
                    )
                }
                Err(_) => (0, 0, 0, 0, 0, 0.0),
            };
            let sent = stats.sent.load(Ordering::Relaxed);
            let latency_samples = stats.received.load(Ordering::Relaxed);
            let received = reported_received_count(&config.load_mode, messages, latency_samples);
            let receive_errors = stats.receive_errors.load(Ordering::Relaxed);
            let correlation_misses = stats.correlation_misses.load(Ordering::Relaxed);
            let in_flight = stats.in_flight.load(Ordering::Relaxed);

            // received/sent ratio approximates conversation fan-out in gcs mode.
            println!(
                "ws-loadtest-rs {}-report attempted={} connected={} failed={} open={} messages={} \
                 sent={} received={} latency_samples={} in_flight={} correlation_misses={} receive_errors={} \
                 p50_us={} p95_us={} p99_us={} max_us={} mean_us={:.0} sample={}",
                config.load_mode,
                attempted,
                connected,
                failed,
                open,
                messages,
                sent,
                received,
                latency_samples,
                in_flight,
                correlation_misses,
                receive_errors,
                p50,
                p95,
                p99,
                max,
                mean_us,
                sample_count
            );
        } else {
            println!(
                "ws-loadtest-rs report attempted={} connected={} failed={} open={} messages={}",
                attempted, connected, failed, open, messages
            );
        }
    }
}

#[tokio::main]
async fn main() {
    if env::var("CONTAINER_POOL_URL").is_ok() {
        if let Err(error) = run_container_pool_smoke().await {
            eprintln!("ws-loadtest-rs container-pool-smoke failed: {error}");
            std::process::exit(1);
        }
        return;
    }

    let config = Arc::new(load_config());
    let stats = Arc::new(Stats::default());
    // Track latencies 1µs..60s with 3-significant-digit resolution. 60s upper bound is high
    // enough to absorb any reasonable timeout in the pipeline; sample size is bounded by
    // the per-client send rate × reporting interval.
    let latencies = Arc::new(Mutex::new(
        Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).expect("hdrhistogram bounds"),
    ));

    println!(
        "ws-loadtest-rs starting target_ws_url={} client_count={} load_mode={} hold_seconds={} \
         connect_timeout_seconds={} receive_timeout_seconds={} reconnect_delay_ms={} \
         ramp_delay_ms={} report_interval_seconds={} messages_per_second_per_client={} \
         message_payload={:?} message_encodings={} gcs_message_encoding={} loadtest_transports={} \
         correlation_timeout_seconds={}",
        config.target_ws_url,
        config.client_count,
        config.load_mode,
        config.hold_seconds,
        config.connect_timeout_seconds,
        config.receive_timeout_seconds,
        config.reconnect_delay_ms,
        config.ramp_delay_ms,
        config.report_interval_seconds,
        config.messages_per_second_per_client,
        config.message_payload,
        format_message_encodings(&config.message_encodings),
        config.gcs_message_encoding.as_str(),
        config.loadtest_transports,
        config.correlation_timeout_seconds
    );

    let reporter_config = Arc::clone(&config);
    let reporter_stats = Arc::clone(&stats);
    let reporter_latencies = Arc::clone(&latencies);
    tokio::spawn(async move {
        report_stats(reporter_config, reporter_stats, reporter_latencies).await;
    });

    // gcs mode needs deterministic conversation/member assignments shared across
    // clients so each conversation's clients hash to the same gcs pod and fan
    // out to one another.
    let gcs_assignments: Vec<Arc<GcsAssignment>> = if config.load_mode == LOAD_MODE_GCS {
        let assignments = build_gcs_assignments(&config);
        let cpc = config.gcs_clients_per_conv.max(1);
        let convs = (config.client_count + cpc - 1) / cpc;
        println!(
            "ws-loadtest-rs gcs-setup conversations={} clients_per_conv={} senders_per_conv={}",
            convs,
            config.gcs_clients_per_conv,
            if config.gcs_senders_per_conv == 0 {
                config.gcs_clients_per_conv
            } else {
                config.gcs_senders_per_conv.min(config.gcs_clients_per_conv)
            }
        );
        assignments
    } else {
        Vec::new()
    };

    let ramp = Duration::from_millis(config.ramp_delay_ms);
    for client_id in 0..config.client_count {
        let client_config = Arc::clone(&config);
        let client_stats = Arc::clone(&stats);
        let client_latencies = Arc::clone(&latencies);
        let mode = config.load_mode.clone();
        let assignment = gcs_assignments.get(client_id).cloned();
        tokio::spawn(async move {
            if mode == LOAD_MODE_GCS {
                let assignment = assignment.expect("gcs assignment for every client");
                run_gcs_client(
                    client_id,
                    assignment,
                    client_config,
                    client_stats,
                    client_latencies,
                )
                .await;
            } else if mode == LOAD_MODE_PIPELINE {
                run_pipeline_client(client_id, client_config, client_stats, client_latencies).await;
            } else {
                run_client(client_id, client_config, client_stats).await;
            }
        });
        sleep(ramp).await;
    }

    loop {
        sleep(Duration::from_secs(60)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcs_marker_parser_handles_text_delimiter() {
        let send_us = now_micros().saturating_sub(1_000);
        let frame = format!(r#"{{"Messages":["gcsrt-12-34-{send_us}"]}}"#);
        let mut latencies = Vec::new();

        parse_gcs_markers(&frame, &mut latencies);

        assert_eq!(latencies.len(), 1);
        assert!(latencies[0] >= 1_000);
    }

    #[test]
    fn gcs_marker_parser_stops_before_following_protobuf_field_key() {
        let send_us = now_micros().saturating_sub(1_000);
        let mut frame = format!("prefix-gcsrt-12-34-{send_us}").into_bytes();
        frame.extend_from_slice(&[0x78, 0x80, 0x81, 0x01]);
        let mut latencies = Vec::new();

        parse_gcs_marker_bytes(&frame, &mut latencies);

        assert_eq!(latencies.len(), 1);
        assert!(latencies[0] >= 1_000);
    }

    #[test]
    fn gcs_marker_parser_skips_malformed_marker() {
        let mut latencies = Vec::new();

        parse_gcs_marker_bytes(b"gcsrt-12-nope-123 gcsrt-12-34-", &mut latencies);

        assert!(latencies.is_empty());
    }

    #[test]
    fn gcs_report_received_counts_frames_not_latency_samples() {
        assert_eq!(reported_received_count(LOAD_MODE_GCS, 42, 0), 42);
        assert_eq!(reported_received_count(LOAD_MODE_PIPELINE, 42, 7), 7);
    }
}
