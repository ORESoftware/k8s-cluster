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

/// LOAD_MODE values.
const LOAD_MODE_HOLD: &str = "hold";
const LOAD_MODE_PIPELINE: &str = "pipeline";

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
    /// Pipeline-mode: drop unmatched-request entries older than this (memory bound on
    /// pending-request map).
    correlation_timeout_seconds: u64,
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
        correlation_timeout_seconds: env_u64("CORRELATION_TIMEOUT_SECONDS", 10),
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
    let send_interval = Duration::from_nanos(
        (1_000_000_000.0 / config.messages_per_second_per_client) as u64,
    );

    let payload = config.message_payload.clone();

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
                let sender = tokio::spawn(async move {
                    let mut seq: u64 = 0;
                    let mut ticker = tokio::time::interval(send_interval);
                    // Skip the initial tick that fires immediately so the rate is honoured.
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    loop {
                        ticker.tick().await;
                        seq = seq.wrapping_add(1);
                        let id = format!("c{client_id}-{seq}");
                        let frame = format!(
                            r#"{{"id":"{}","payload":"{}"}}"#,
                            id,
                            json_escape(&send_payload)
                        );
                        if let Ok(mut map) = send_pending.lock() {
                            map.insert(id.clone(), Instant::now());
                            send_stats.in_flight.store(map.len(), Ordering::Relaxed);
                        }
                        if writer.send(Message::Text(frame)).await.is_err() {
                            break;
                        }
                        send_stats.sent.fetch_add(1, Ordering::Relaxed);
                    }
                });

                // Receiver task: parses each frame, extracts `id`, matches against pending map.
                while let Some(message) = reader.next().await {
                    match message {
                        Ok(Message::Text(text)) => {
                            stats.messages.fetch_add(1, Ordering::Relaxed);
                            if let Some(id) = extract_id(&text) {
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
                        Ok(_) => {
                            // Ignore ping/pong/binary frames.
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
                eprintln!("connect failed client={} error={}", client_id, error);
            }
            Err(_elapsed) => {
                stats.failed.fetch_add(1, Ordering::Relaxed);
                eprintln!("connect timeout client={}", client_id);
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
                eprintln!("connect failed client={} error={}", client_id, error);
            }
            Err(_elapsed) => {
                stats.failed.fetch_add(1, Ordering::Relaxed);
                eprintln!("connect timeout client={}", client_id);
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

        if config.load_mode == LOAD_MODE_PIPELINE {
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
            let received = stats.received.load(Ordering::Relaxed);
            let receive_errors = stats.receive_errors.load(Ordering::Relaxed);
            let correlation_misses = stats.correlation_misses.load(Ordering::Relaxed);
            let in_flight = stats.in_flight.load(Ordering::Relaxed);

            println!(
                "ws-loadtest-rs pipeline-report attempted={} connected={} failed={} open={} \
                 sent={} received={} in_flight={} correlation_misses={} receive_errors={} \
                 p50_us={} p95_us={} p99_us={} max_us={} mean_us={:.0} sample={}",
                attempted,
                connected,
                failed,
                open,
                sent,
                received,
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
         message_payload={:?} correlation_timeout_seconds={}",
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
        config.correlation_timeout_seconds
    );

    let reporter_config = Arc::clone(&config);
    let reporter_stats = Arc::clone(&stats);
    let reporter_latencies = Arc::clone(&latencies);
    tokio::spawn(async move {
        report_stats(reporter_config, reporter_stats, reporter_latencies).await;
    });

    let ramp = Duration::from_millis(config.ramp_delay_ms);
    for client_id in 0..config.client_count {
        let client_config = Arc::clone(&config);
        let client_stats = Arc::clone(&stats);
        let client_latencies = Arc::clone(&latencies);
        let mode = config.load_mode.clone();
        tokio::spawn(async move {
            if mode == LOAD_MODE_PIPELINE {
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
