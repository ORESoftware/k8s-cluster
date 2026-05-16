use futures_util::{SinkExt, StreamExt};
use std::env;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::time::{sleep, timeout, Duration, Instant};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

const DEFAULT_WS_URL: &str = "ws://dd-gleamlang-server.default.svc.cluster.local:8081/ws";

#[derive(Debug, Default)]
struct Stats {
    attempted: AtomicUsize,
    connected: AtomicUsize,
    failed: AtomicUsize,
    open: AtomicUsize,
    messages: AtomicUsize,
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
    }
}

async fn run_client(
    client_id: usize,
    config: Arc<Config>,
    stats: Arc<Stats>,
) -> ! {
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

async fn report_stats(config: Arc<Config>, stats: Arc<Stats>) -> ! {
    let period = Duration::from_secs(config.report_interval_seconds);
    loop {
        sleep(period).await;
        let attempted = stats.attempted.load(Ordering::Relaxed);
        let connected = stats.connected.load(Ordering::Relaxed);
        let failed = stats.failed.load(Ordering::Relaxed);
        let open = stats.open.load(Ordering::Relaxed);
        let messages = stats.messages.load(Ordering::Relaxed);

        println!(
            "ws-loadtest-rs report attempted={} connected={} failed={} open={} messages={}",
            attempted, connected, failed, open, messages
        );
    }
}

#[tokio::main]
async fn main() {
    let config = Arc::new(load_config());
    let stats = Arc::new(Stats::default());

    println!(
        "ws-loadtest-rs starting target_ws_url={} client_count={} hold_seconds={} connect_timeout_seconds={} receive_timeout_seconds={} reconnect_delay_ms={} ramp_delay_ms={} report_interval_seconds={}",
        config.target_ws_url,
        config.client_count,
        config.hold_seconds,
        config.connect_timeout_seconds,
        config.receive_timeout_seconds,
        config.reconnect_delay_ms,
        config.ramp_delay_ms,
        config.report_interval_seconds
    );

    let reporter_config = Arc::clone(&config);
    let reporter_stats = Arc::clone(&stats);
    tokio::spawn(async move {
        report_stats(reporter_config, reporter_stats).await;
    });

    let ramp = Duration::from_millis(config.ramp_delay_ms);
    for client_id in 0..config.client_count {
        let client_config = Arc::clone(&config);
        let client_stats = Arc::clone(&stats);
        tokio::spawn(async move {
            run_client(client_id, client_config, client_stats).await;
        });
        sleep(ramp).await;
    }

    loop {
        sleep(Duration::from_secs(60)).await;
    }
}
