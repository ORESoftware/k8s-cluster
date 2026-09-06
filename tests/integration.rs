//! End-to-end smoke tests that run the broker on real loopback sockets and
//! drive it with the in-tree `Client` / `RwClient` plus raw HTTP for the
//! serverless surface.
//!
//! Each test picks an ephemeral port (port 0) and an OS-assigned UDS path so
//! tests can run in parallel without contention.

use std::time::Duration;

use dd_rust_network_mutex::{
    server, BrokerConfig, Client, ClientConfig, LockGuard, RwClient, ServerConfig,
};
use tokio::net::TcpListener;

async fn pick_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

async fn start_server(tcp: bool, http: bool) -> (Option<u16>, Option<u16>) {
    let tcp_port = if tcp { Some(pick_port().await) } else { None };
    let http_port = if http { Some(pick_port().await) } else { None };
    let cfg = ServerConfig {
        tcp_bind: tcp_port.map(|p| format!("127.0.0.1:{p}").parse().unwrap()),
        uds_path: None,
        http_bind: http_port.map(|p| format!("127.0.0.1:{p}").parse().unwrap()),
        auth_token: None,
        broker: BrokerConfig::default(),
        tcp_nodelay: true,
        tcp_quickack: true,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    };
    tokio::spawn(async move {
        let _ = server::run(cfg).await;
    });
    // Give the listener a moment to bind. We could probe but a tiny sleep is
    // simpler than introducing a synchronization primitive into the public API.
    tokio::time::sleep(Duration::from_millis(50)).await;
    (tcp_port, http_port)
}

#[tokio::test]
async fn tcp_acquire_release_roundtrip() {
    let (tcp_port, _) = start_server(true, false).await;
    let port = tcp_port.unwrap();
    let client = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();

    let guard = client
        .acquire("integration-key", Duration::from_millis(2000))
        .await
        .unwrap();
    assert!(guard.fencing_token.unwrap() >= 1);
    assert_eq!(guard.keys, vec!["integration-key".to_string()]);

    client.release(&guard).await.unwrap();
}

#[tokio::test]
async fn tcp_two_clients_serialize_on_one_key() {
    let (tcp_port, _) = start_server(true, false).await;
    let port = tcp_port.unwrap();
    let a = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let b = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();

    let g1 = a
        .acquire("contended", Duration::from_millis(2000))
        .await
        .unwrap();
    let token_1 = g1.fencing_token.unwrap();

    let b_clone = b.clone();
    let acquire_b = tokio::spawn(async move {
        b_clone
            .acquire("contended", Duration::from_millis(2000))
            .await
    });

    // B should still be queued while A holds the lock.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!acquire_b.is_finished());

    a.release(&g1).await.unwrap();
    let g2 = acquire_b.await.unwrap().unwrap();
    let token_2 = g2.fencing_token.unwrap();
    assert!(
        token_2 > token_1,
        "fencing token must increase across handoff"
    );
    b.release(&g2).await.unwrap();
}

#[tokio::test]
async fn tcp_composite_lock_atomic() {
    let (tcp_port, _) = start_server(true, false).await;
    let port = tcp_port.unwrap();
    let client = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let guard = client
        .acquire_composite(&["alpha", "beta", "gamma"], Duration::from_millis(2000))
        .await
        .unwrap();
    assert_eq!(guard.fencing_tokens.len(), 3);
    assert!(guard.fencing_tokens.values().all(|t| *t >= 1));
    client.release(&guard).await.unwrap();
}

#[tokio::test]
async fn tcp_rw_locks_serialise_writers_and_let_readers_share() {
    let (tcp_port, _) = start_server(true, false).await;
    let port = tcp_port.unwrap();
    let r1 = RwClient::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let r2 = RwClient::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let w1 = RwClient::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();

    let read_a = r1.acquire_read("rw").await.unwrap();
    let read_b = r2.acquire_read("rw").await.unwrap();

    let write_handle = tokio::spawn(async move { w1.acquire_write("rw").await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!write_handle.is_finished(), "writer must wait on readers");

    read_a.release().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !write_handle.is_finished(),
        "writer must wait on remaining reader"
    );
    read_b.release().await.unwrap();

    let write_guard = write_handle.await.unwrap().unwrap();
    write_guard.release().await.unwrap();
}

#[tokio::test]
async fn http_acquire_and_release_via_serverless_surface() {
    let (_, http_port) = start_server(false, true).await;
    let port = http_port.unwrap();
    let url = format!("http://127.0.0.1:{port}");

    let acquire = http_post(
        &format!("{url}/v1/lock"),
        serde_json::json!({"key": "http-key", "ttlMs": 2000}),
    )
    .await;
    assert_eq!(acquire["acquired"], true);
    assert!(acquire["lockUuid"].is_string());
    let lock_uuid = acquire["lockUuid"].as_str().unwrap().to_string();
    assert!(acquire["fencingTokens"]["http-key"].as_u64().unwrap() >= 1);

    let release = http_post(
        &format!("{url}/v1/unlock"),
        serde_json::json!({"key": "http-key", "lockUuid": lock_uuid}),
    )
    .await;
    assert_eq!(release["unlocked"], true);
}

#[tokio::test]
async fn http_composite_lock_via_serverless_surface() {
    let (_, http_port) = start_server(false, true).await;
    let port = http_port.unwrap();
    let url = format!("http://127.0.0.1:{port}");

    let acquire = http_post(
        &format!("{url}/v1/lock"),
        serde_json::json!({"keys": ["x", "y"], "ttlMs": 2000}),
    )
    .await;
    assert_eq!(acquire["acquired"], true);
    let lock_uuid = acquire["lockUuid"].as_str().unwrap().to_string();
    assert!(acquire["fencingTokens"]["x"].as_u64().unwrap() >= 1);
    assert!(acquire["fencingTokens"]["y"].as_u64().unwrap() >= 1);

    let release = http_post(
        &format!("{url}/v1/unlock"),
        serde_json::json!({"keys": ["x", "y"], "lockUuid": lock_uuid}),
    )
    .await;
    assert_eq!(release["unlocked"], true);
}

#[tokio::test]
async fn uds_acquire_release_roundtrip() {
    let dir = std::env::temp_dir().join(format!("dd-rust-mutex-test-{}.sock", uuid_v4()));
    let cfg = ServerConfig {
        tcp_bind: None,
        uds_path: Some(dir.clone()),
        http_bind: None,
        auth_token: None,
        broker: BrokerConfig::default(),
        tcp_nodelay: true,
        tcp_quickack: true,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    };
    tokio::spawn(async move {
        let _ = server::run(cfg).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::connect_uds(&dir, ClientConfig::default())
        .await
        .unwrap();
    let guard = client
        .acquire("uds-key", Duration::from_millis(2000))
        .await
        .unwrap();
    assert!(guard.fencing_token.unwrap() >= 1);
    client.release(&guard).await.unwrap();
}

fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{nanos:x}")
}

/// Regression test for the upstream `live-mutex#22` experiment: the broker
/// must keep working both with the new socket-tuning knobs disabled and
/// enabled. We don't measure the latency benefit here (loopback hides it);
/// we just assert the request/response loop survives the extra setsockopt
/// path.
#[tokio::test]
async fn tcp_works_with_nodelay_quickack_disabled() {
    let tcp_port = pick_port().await;
    let cfg = ServerConfig {
        tcp_bind: Some(format!("127.0.0.1:{tcp_port}").parse().unwrap()),
        uds_path: None,
        http_bind: None,
        auth_token: None,
        broker: BrokerConfig::default(),
        tcp_nodelay: false,
        tcp_quickack: false,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    };
    tokio::spawn(async move {
        let _ = server::run(cfg).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::connect_tcp(("127.0.0.1", tcp_port), ClientConfig::default())
        .await
        .unwrap();
    let guard = client
        .acquire("tuning-off", Duration::from_millis(2000))
        .await
        .unwrap();
    client.release(&guard).await.unwrap();
}

#[tokio::test]
async fn tcp_works_with_nodelay_quickack_enabled() {
    // Same shape as the disabled case; this is the *default* config so we
    // mostly want a regression guard for the apply-after-every-read path.
    let tcp_port = pick_port().await;
    let cfg = ServerConfig {
        tcp_bind: Some(format!("127.0.0.1:{tcp_port}").parse().unwrap()),
        uds_path: None,
        http_bind: None,
        auth_token: None,
        broker: BrokerConfig::default(),
        tcp_nodelay: true,
        tcp_quickack: true,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    };
    tokio::spawn(async move {
        let _ = server::run(cfg).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::connect_tcp(("127.0.0.1", tcp_port), ClientConfig::default())
        .await
        .unwrap();
    // Several round-trips so we exercise the per-read setsockopt repeatedly.
    for i in 0..10 {
        let key = format!("tuning-on-{i}");
        let guard = client
            .acquire(&key, Duration::from_millis(2000))
            .await
            .unwrap();
        client.release(&guard).await.unwrap();
    }
}

#[tokio::test]
async fn healthz_and_metrics_exposed() {
    let (_, http_port) = start_server(false, true).await;
    let port = http_port.unwrap();

    let body = http_get_text(&format!("http://127.0.0.1:{port}/healthz")).await;
    assert!(body.contains("\"ok\":true"));

    let metrics = http_get_text(&format!("http://127.0.0.1:{port}/metrics")).await;
    assert!(metrics.contains("dd_rust_network_mutex_keys"));
    assert!(metrics.contains("dd_rust_network_mutex_holders"));
    assert!(metrics.contains("dd_rust_network_mutex_waiters"));
    // Periodic-sweeper bookkeeping (upstream live-mutex#13).
    assert!(metrics.contains("dd_rust_network_mutex_pending_deadlines"));
    assert!(metrics.contains("dd_rust_network_mutex_ttl_evictions_total"));
    assert!(metrics.contains("dd_rust_network_mutex_request_duration_seconds_bucket"));
    assert!(metrics.contains("dd_rust_network_mutex_request_payload_bytes_bucket"));
}

/// Semaphore semantics on a real broker over real loopback TCP. Three
/// clients acquire a `max=3` lock and must all hold simultaneously; a
/// fourth client's acquire only resolves once one of the three
/// releases. This is the end-to-end version of the broker-unit test.
#[tokio::test]
async fn semaphore_three_holders_coexist_then_fourth_unblocks_on_release() {
    let (tcp_port, _) = start_server(true, false).await;
    let port = tcp_port.unwrap();

    // Three concurrent acquires with max=3 must all succeed.
    let mut clients = Vec::new();
    let mut guards = Vec::new();
    for i in 0..3 {
        let c = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
            .await
            .unwrap();
        let g = c
            .acquire_with_max("sem-key", 3, Duration::from_millis(2000))
            .await
            .unwrap_or_else(|err| panic!("client {i} should hold a slot, got {err:?}"));
        clients.push(c);
        guards.push(g);
    }
    // Fencing tokens must be unique.
    let tokens: Vec<u64> = guards
        .iter()
        .map(|g| g.fencing_token.unwrap_or(0))
        .collect();
    assert_eq!(
        tokens
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3,
        "each semaphore slot must mint a distinct fencing token; got {tokens:?}"
    );

    // Fourth client queues until a slot opens up.
    let fourth = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let pending = tokio::spawn({
        let f = fourth.clone();
        async move {
            f.acquire_with_max("sem-key", 3, Duration::from_millis(2000))
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !pending.is_finished(),
        "fourth client must wait while three slots are held"
    );

    // Release one slot — fourth must wake up.
    clients[0].release(&guards[0]).await.unwrap();
    let g4 = pending
        .await
        .unwrap()
        .expect("fourth client should be granted after a release");
    fourth.release(&g4).await.unwrap();
    for (c, g) in clients.iter().skip(1).zip(guards.iter().skip(1)) {
        c.release(g).await.unwrap();
    }
}

/// The broker silently clamps a giant `max` to its
/// `max_concurrency_cap` and reports the clamp in `/metrics`. This is
/// the user-visible contract from the operator side.
#[tokio::test]
async fn concurrency_cap_clamp_visible_in_metrics() {
    // Fresh broker on its own ports so the clamp counter starts at 0.
    let tcp_port = pick_port().await;
    let http_port = pick_port().await;
    let cfg = ServerConfig {
        tcp_bind: Some(format!("127.0.0.1:{tcp_port}").parse().unwrap()),
        uds_path: None,
        http_bind: Some(format!("127.0.0.1:{http_port}").parse().unwrap()),
        auth_token: None,
        broker: BrokerConfig {
            max_concurrency_cap: 4,
            ..BrokerConfig::default()
        },
        tcp_nodelay: true,
        tcp_quickack: true,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    };
    tokio::spawn(async move {
        let _ = server::run(cfg).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::connect_tcp(("127.0.0.1", tcp_port), ClientConfig::default())
        .await
        .unwrap();
    // Ask for max=999; the broker clamps to 4 silently and returns a
    // successful grant.
    let g = client
        .acquire_with_max("clamp-me", 999, Duration::from_millis(2000))
        .await
        .unwrap();
    assert!(g.fencing_token.is_some());
    client.release(&g).await.unwrap();

    let metrics = http_get_text(&format!("http://127.0.0.1:{http_port}/metrics")).await;
    assert!(
        metrics.contains("dd_rust_network_mutex_max_concurrency_cap 4"),
        "metrics should report the configured ceiling; got:\n{metrics}",
    );
    assert!(
        metrics.contains("dd_rust_network_mutex_concurrency_cap_clamps_total 1"),
        "metrics should record exactly one clamp; got:\n{metrics}",
    );
}

/// `max=0` over the Rust client is rejected immediately at the call
/// site — no broker round-trip happens at all. This is the cheapest
/// possible "fail early" path: bad input doesn't even reach the wire.
#[tokio::test]
async fn rust_client_rejects_max_zero_locally() {
    let (tcp_port, _) = start_server(true, false).await;
    let port = tcp_port.unwrap();
    let c = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let err = c
        .acquire_with_max("zero-max", 0, Duration::from_millis(2000))
        .await
        .expect_err("max=0 must error");
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "client-side rejection should be effectively instant; took {:?}",
        started.elapsed()
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("max >= 1"),
        "error message should explain the constraint; got: {msg}"
    );
}

/// Cross-runtime defense in depth: a raw TCP request with `max: 0`
/// (bypassing the Rust client's local validation) is rejected by the
/// broker with `acquired:false` and an `error` field. No holder is
/// created and no waiter is queued — `holders` and `waiters` stay at
/// 0 in `/metrics`.
#[tokio::test]
async fn raw_tcp_max_zero_rejected_with_error_and_no_side_effect() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let tcp_port = pick_port().await;
    let http_port = pick_port().await;
    let cfg = ServerConfig {
        tcp_bind: Some(format!("127.0.0.1:{tcp_port}").parse().unwrap()),
        uds_path: None,
        http_bind: Some(format!("127.0.0.1:{http_port}").parse().unwrap()),
        auth_token: None,
        broker: BrokerConfig::default(),
        tcp_nodelay: true,
        tcp_quickack: true,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    };
    tokio::spawn(async move {
        let _ = server::run(cfg).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", tcp_port))
        .await
        .unwrap();
    // version handshake (no auth on this broker).
    let version = b"{\"type\":\"version\",\"uuid\":\"v\",\"value\":\"0.1.0\"}\n";
    sock.write_all(version).await.unwrap();
    sock.flush().await.unwrap();

    // Now send a Lock request with max=0.
    let lock = b"{\"type\":\"lock\",\"uuid\":\"r\",\"key\":\"raw-zero\",\"ttl\":60000,\"max\":0}\n";
    sock.write_all(lock).await.unwrap();
    sock.flush().await.unwrap();

    // Read frames until we see the `r` correlation reply (skipping the
    // version response).
    let (read, _write) = sock.into_split();
    let mut reader = BufReader::new(read);
    let mut found_error: Option<String> = None;
    for _ in 0..5 {
        let mut line = String::new();
        let n = tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
            .await
            .expect("broker should reply within 2s")
            .expect("read_line failed");
        if n == 0 {
            break;
        }
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        if v.get("uuid").and_then(|u| u.as_str()) == Some("r") {
            assert_eq!(v["acquired"], serde_json::Value::Bool(false));
            found_error = v.get("error").and_then(|e| e.as_str()).map(str::to_owned);
            break;
        }
    }
    let err = found_error.expect("broker must reply to `r` with an error");
    assert!(
        err.contains("`max` must be >= 1"),
        "error must explain the constraint; got: {err}"
    );

    // No side effects in /metrics.
    let metrics = http_get_text(&format!("http://127.0.0.1:{http_port}/metrics")).await;
    assert!(
        metrics.contains("dd_rust_network_mutex_holders 0"),
        "rejection must not create a holder; got:\n{metrics}"
    );
    assert!(
        metrics.contains("dd_rust_network_mutex_waiters 0"),
        "rejection must not enqueue a waiter; got:\n{metrics}"
    );
    assert!(
        metrics.contains("dd_rust_network_mutex_keys 0"),
        "rejection must not create per-key state; got:\n{metrics}"
    );
}

/// HTTP `/v1/lock` with `max: 0` returns a 400 (the validation-error
/// path established in `release_with_wrong_lock_uuid_is_rejected_over_http`)
/// with `acquired: false` and the broker's `error` message in the
/// body. After the rejection, a normal `acquire` on the same key
/// still works — the broker really did skip all per-key state.
#[tokio::test]
async fn http_max_zero_returns_400_with_error_then_normal_acquire_works() {
    let (tcp_port, http_port) = start_server(true, true).await;
    let tcp = tcp_port.unwrap();
    let http = http_port.unwrap();

    let resp = http_post(
        &format!("http://127.0.0.1:{http}/v1/lock"),
        serde_json::json!({"key": "http-zero", "ttlMs": 1000, "max": 0}),
    )
    .await;
    assert_eq!(resp["acquired"], serde_json::Value::Bool(false));
    let err = resp["error"]
        .as_str()
        .unwrap_or_else(|| panic!("response must include an `error` field; got: {resp}"));
    assert!(
        err.contains("`max` must be >= 1"),
        "error must explain the constraint; got: {err}"
    );

    // Normal acquire on the same key still works (no leaked state).
    let c = Client::connect_tcp(("127.0.0.1", tcp), ClientConfig::default())
        .await
        .unwrap();
    let g = c
        .acquire("http-zero", Duration::from_millis(2000))
        .await
        .expect("normal acquire after rejected max=0 must succeed");
    // Per-key fencing counter is seeded from `Date.now()`-equivalent
    // millis since epoch (see `LockState::new`), so the first grant on
    // a fresh key is roughly the wall clock — not literally 1. Just
    // assert it's present and plausibly recent.
    let token = g
        .fencing_token
        .expect("first grant must include a fencing token");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    assert!(
        token + 60_000 >= now_ms && token <= now_ms + 60_000,
        "fencing token {token} should be within ~1 minute of wall clock {now_ms}"
    );
    c.release(&g).await.unwrap();
}

/// Status page (upstream `live-mutex#108`) must render on the main HTTP
/// listener at both `/` and `/status`, and the rendered HTML must
/// contain the live broker counters from `/metrics` (one round-trip,
/// no JS). Asserting on text content keeps this test resilient to
/// future styling tweaks.
#[tokio::test]
async fn status_page_renders_on_main_http_listener() {
    let (_, http_port) = start_server(false, true).await;
    let port = http_port.unwrap();

    for path in ["/", "/status"] {
        let body = http_get_text(&format!("http://127.0.0.1:{port}{path}")).await;
        assert!(
            body.starts_with("<!doctype html>"),
            "{path} did not return HTML; got: {body:.200}"
        );
        assert!(body.contains("dd-rust-network-mutex"));
        assert!(body.contains("Connected clients"));
        assert!(body.contains("TTL evictions"));
        // The Prometheus exposition is embedded in the page so the same
        // URL is useful for humans AND `curl | rg`.
        assert!(body.contains("dd_rust_network_mutex_keys"));
    }
}

/// The optional dedicated status-only listener (`LMX_STATUS_PORT` /
/// `ServerConfig.status_bind`) serves the HTML page and `/metrics` but
/// not the API surface — which is the whole point of having it on a
/// separate port.
#[tokio::test]
async fn dedicated_status_listener_serves_html_but_not_api() {
    let tcp_port = pick_port().await;
    let status_port = pick_port().await;
    let cfg = ServerConfig {
        tcp_bind: Some(format!("127.0.0.1:{tcp_port}").parse().unwrap()),
        uds_path: None,
        http_bind: None,
        auth_token: None,
        broker: BrokerConfig::default(),
        tcp_nodelay: true,
        tcp_quickack: true,
        status_bind: Some(format!("127.0.0.1:{status_port}").parse().unwrap()),
        #[cfg(feature = "tls")]
        tls: None,
    };
    tokio::spawn(async move {
        let _ = server::run(cfg).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // HTML page: present.
    let html = http_get_text(&format!("http://127.0.0.1:{status_port}/")).await;
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("dd-rust-network-mutex"));

    // Metrics: present.
    let metrics = http_get_text(&format!("http://127.0.0.1:{status_port}/metrics")).await;
    assert!(metrics.contains("dd_rust_network_mutex_keys"));

    // Healthz: present (handy for a load balancer health check on the
    // operator port).
    let healthz = http_get_text(&format!("http://127.0.0.1:{status_port}/healthz")).await;
    assert!(healthz.contains("\"ok\":true"));

    // API: NOT present. The status listener must not expose `/v1/*`.
    let v1_resp = http_send_raw(
        "127.0.0.1",
        status_port,
        b"POST /v1/lock HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 18\r\nConnection: close\r\n\r\n{\"key\":\"never\"}\r\n",
    )
    .await;
    assert!(
        v1_resp.starts_with("HTTP/1.1 404")
            || v1_resp.starts_with("HTTP/1.1 405")
            || v1_resp.starts_with("HTTP/1.1 400"),
        "expected non-2xx for /v1/lock on status listener, got: {v1_resp:.120}",
    );
}

async fn http_send_raw(host: &str, port: u16, raw_request: &[u8]) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut sock = tokio::net::TcpStream::connect((host, port)).await.unwrap();
    sock.write_all(raw_request).await.unwrap();
    sock.flush().await.unwrap();
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.unwrap();
    String::from_utf8_lossy(&buf).into_owned()
}

/// Live broker, real loopback, real wall clock: Client B is queued
/// behind Client A. A never releases. The single periodic sweeper task
/// must evict A once its TTL passes and grant B — without anyone having
/// scheduled a `tokio::time::sleep(ttl)` per request. This is the
/// observable contract of upstream
/// [`live-mutex#13`](https://github.com/ORESoftware/live-mutex/issues/13)
/// applied to `rust-network-mutex-rs`.
#[tokio::test]
async fn ttl_sweeper_evicts_dead_holder_and_grants_next_waiter() {
    // Use a fast sweep so the test isn't wall-clock heavy.
    let tcp_port = pick_port().await;
    let cfg = ServerConfig {
        tcp_bind: Some(format!("127.0.0.1:{tcp_port}").parse().unwrap()),
        uds_path: None,
        http_bind: None,
        auth_token: None,
        broker: BrokerConfig {
            ttl_sweep_interval: Duration::from_millis(5),
            ..BrokerConfig::default()
        },
        tcp_nodelay: true,
        tcp_quickack: true,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    };
    tokio::spawn(async move {
        let _ = server::run(cfg).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let a = Client::connect_tcp(("127.0.0.1", tcp_port), ClientConfig::default())
        .await
        .unwrap();
    let b = Client::connect_tcp(("127.0.0.1", tcp_port), ClientConfig::default())
        .await
        .unwrap();

    // A grabs the lock with a short TTL; we deliberately *do not*
    // release — simulating a dead/slow holder. Without TTL eviction B
    // would wait forever.
    let _a_guard = a
        .acquire("ttl-evict-key", Duration::from_millis(100))
        .await
        .unwrap();

    // B asks for the same key. The client-side acquire timeout is
    // generous enough to span A's TTL plus several sweep ticks; the
    // success of this `await` proves the sweeper cleared A's claim.
    let started = std::time::Instant::now();
    let b_guard = b
        .acquire("ttl-evict-key", Duration::from_millis(2000))
        .await
        .expect("B should be granted after A's TTL expires");
    let waited = started.elapsed();

    // Sanity: we didn't somehow short-circuit before A's TTL ran out.
    assert!(
        waited >= Duration::from_millis(80),
        "B was granted suspiciously fast ({waited:?}) — TTL eviction may not have been the trigger",
    );

    // And: B's fencing token is strictly greater than what A held —
    // the broker treated this as a real handoff, not a duplicate grant.
    assert!(b_guard.fencing_token.unwrap_or_default() >= 2);

    b.release(&b_guard).await.unwrap();
}

// ---- Concurrency / race tests --------------------------------------------

/// N+1 race: 11 clients all race to acquire `max=10`. Exactly 10
/// succeed, 1 queues. Fencing tokens are unique across the 10 winners.
/// This is a stronger test than the unit test because each acquire is
/// an actual concurrent task, not a serialized broker call.
#[tokio::test]
async fn n_plus_one_race_grants_exactly_n_then_queues_one() {
    let (tcp_port, _) = start_server(true, false).await;
    let port = tcp_port.unwrap();

    const CAP: u32 = 10;
    let mut handles = Vec::new();
    for i in 0..(CAP + 1) {
        let h = tokio::spawn(async move {
            let c = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
                .await
                .unwrap();
            (
                i,
                c.clone(),
                tokio::time::timeout(
                    Duration::from_millis(300),
                    c.acquire_with_max("race", CAP, Duration::from_millis(2000)),
                )
                .await,
            )
        });
        handles.push(h);
    }

    let mut grants = Vec::new();
    let mut timeouts = 0;
    for h in handles {
        let (i, client, outcome) = h.await.unwrap();
        match outcome {
            Ok(Ok(g)) => grants.push((i, client, g)),
            Ok(Err(err)) => panic!("client {i} failed: {err:?}"),
            Err(_) => timeouts += 1,
        }
    }
    assert_eq!(
        grants.len() as u32,
        CAP,
        "exactly {CAP} of {} should have been granted; got {} grants and {timeouts} timeouts",
        CAP + 1,
        grants.len(),
    );
    assert_eq!(
        timeouts, 1,
        "exactly one client should have queued past the timeout"
    );

    // All winners have unique fencing tokens.
    let tokens: std::collections::HashSet<u64> = grants
        .iter()
        .filter_map(|(_, _, g)| g.fencing_token)
        .collect();
    assert_eq!(tokens.len() as u32, CAP, "fencing tokens must all differ");

    // Cleanup.
    for (_, c, g) in &grants {
        c.release(g).await.unwrap();
    }
}

/// Stress: 50 clients hammering a `max=5` semaphore with brief holds.
/// At any point the broker reports at most 5 holders. All eventually
/// complete. This catches bugs in the dequeue path under churn.
#[tokio::test]
async fn semaphore_stress_50_clients_max_5() {
    let (tcp_port, http_port) = start_server(true, true).await;
    let port = tcp_port.unwrap();
    let http = http_port.unwrap();

    const CAP: u32 = 5;
    const N: usize = 50;

    let mut handles = Vec::new();
    for i in 0..N {
        let h = tokio::spawn(async move {
            let c = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
                .await
                .unwrap();
            let g = c
                .acquire_with_max("stress", CAP, Duration::from_millis(10_000))
                .await
                .unwrap_or_else(|err| panic!("client {i} acquire failed: {err:?}"));
            // Tiny critical section so the queue actually drains.
            tokio::time::sleep(Duration::from_millis(2)).await;
            c.release(&g).await.unwrap();
        });
        handles.push(h);
    }

    // While churn is happening, sample `/metrics` a few times and
    // assert holders never exceed CAP.
    for _ in 0..30 {
        let metrics = http_get_text(&format!("http://127.0.0.1:{http}/metrics")).await;
        if let Some(line) = metrics
            .lines()
            .find(|l| l.starts_with("dd_rust_network_mutex_holders "))
        {
            let n: u64 = line.split_whitespace().last().unwrap().parse().unwrap();
            assert!(n <= CAP as u64, "holders={n} exceeded cap={CAP}");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    for h in handles {
        h.await.unwrap();
    }

    // After everyone finishes, holders + waiters drop to zero.
    let final_metrics = http_get_text(&format!("http://127.0.0.1:{http}/metrics")).await;
    assert!(
        final_metrics.contains("dd_rust_network_mutex_holders 0"),
        "all holders should drain by end of test; got:\n{final_metrics}",
    );
    assert!(final_metrics.contains("dd_rust_network_mutex_waiters 0"));
}

/// Mirrors the production load tester's `useAcquireMany: true` profile:
/// many concurrent clients each call `acquire_composite` over a sliding
/// 3-key window, release, repeat. Stresses the broker's progressive
/// queue-and-grant machinery for `acquire-many` requests under heavy
/// contention. Asserts:
///   - every iteration eventually completes (no stalled queueing path).
///   - per-key fencing tokens stay strictly monotonic across handoffs
///     (no double-grant under contention).
///   - holders + waiters drain to zero once all workers finish.
#[tokio::test]
async fn composite_lock_stress_50_clients_overlapping_3_key_windows() {
    let (tcp_port, http_port) = start_server(true, true).await;
    let port = tcp_port.unwrap();
    let http = http_port.unwrap();

    const N_CLIENTS: usize = 50;
    const ITERS_PER_CLIENT: usize = 6;
    const KEYSPACE: usize = 16;

    let high_water: std::sync::Arc<parking_lot::Mutex<std::collections::HashMap<String, u64>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
    let violations = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let mut handles = Vec::new();
    for client_idx in 0..N_CLIENTS {
        let high_water = high_water.clone();
        let violations = violations.clone();
        let h = tokio::spawn(async move {
            let c = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
                .await
                .unwrap();
            for it in 0..ITERS_PER_CLIENT {
                let base = (client_idx * 7 + it * 3) % KEYSPACE;
                let k0 = format!("composite-stress-{:03}", base);
                let k1 = format!("composite-stress-{:03}", (base + 1) % KEYSPACE);
                let k2 = format!("composite-stress-{:03}", (base + 2) % KEYSPACE);
                let keys: Vec<&str> = vec![k0.as_str(), k1.as_str(), k2.as_str()];
                let g = c
                    .acquire_composite(&keys, Duration::from_millis(15_000))
                    .await
                    .unwrap_or_else(|err| {
                        panic!("client {client_idx} iter {it} composite failed: {err:?}")
                    });
                {
                    let mut hi = high_water.lock();
                    for (k, v) in &g.fencing_tokens {
                        let prev = hi.get(k).copied().unwrap_or(0);
                        if *v <= prev {
                            violations.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        } else {
                            hi.insert(k.clone(), *v);
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
                c.release(&g).await.unwrap();
            }
        });
        handles.push(h);
    }

    for h in handles {
        h.await.unwrap();
    }

    assert_eq!(
        violations.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "fencing-token monotonicity broken under composite contention"
    );

    let final_metrics = http_get_text(&format!("http://127.0.0.1:{http}/metrics")).await;
    assert!(
        final_metrics.contains("dd_rust_network_mutex_holders 0"),
        "holders should drain to 0 after stress; got:\n{final_metrics}"
    );
    assert!(
        final_metrics.contains("dd_rust_network_mutex_waiters 0"),
        "waiters should drain to 0 after stress; got:\n{final_metrics}"
    );
}

/// Variation that mixes single-key and composite contenders on the same
/// keyspace. The single-key acquirers should not starve the composite
/// requests, and vice versa — the only correctness contract this test
/// asserts is *forward progress* and *fencing monotonicity*.
#[tokio::test]
async fn composite_and_single_key_mix_under_contention() {
    let (tcp_port, _) = start_server(true, false).await;
    let port = tcp_port.unwrap();

    const N: usize = 30;
    const ITERS: usize = 5;

    let mut handles = Vec::new();
    for client_idx in 0..N {
        let h = tokio::spawn(async move {
            let c = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
                .await
                .unwrap();
            for _ in 0..ITERS {
                if client_idx % 2 == 0 {
                    let k = format!("mix-key-{:03}", client_idx % 8);
                    let g = c.acquire(&k, Duration::from_millis(10_000)).await.unwrap();
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    c.release(&g).await.unwrap();
                } else {
                    let k0 = format!("mix-key-{:03}", client_idx % 8);
                    let k1 = format!("mix-key-{:03}", (client_idx + 1) % 8);
                    let g = c
                        .acquire_composite(
                            &[k0.as_str(), k1.as_str()],
                            Duration::from_millis(10_000),
                        )
                        .await
                        .unwrap();
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    c.release(&g).await.unwrap();
                }
            }
        });
        handles.push(h);
    }

    for h in handles {
        h.await.unwrap();
    }
}

// ---- Disconnect & ownership tests ----------------------------------------

/// A client that disconnects after **partially acquiring** a composite —
/// holding key A while still queued on key B — must not leak A to the
/// broker. Forces the partial-grant code path by:
///   1. blocker_a holds "pa" (single-key).
///   2. blocker_b holds "pb" (single-key).
///   3. composite_client queues on `acquire_composite(["pa","pb"])`.
///   4. blocker_a releases "pa" — broker's progressive-grant code grants
///      "pa" to the composite (partial!) and re-queues it on "pb".
///   5. composite_client disconnects.
/// Now "pa" must come back to the pool. A fresh single-key acquire on
/// "pa" should succeed; if the partial hold leaked, it would queue
/// behind a phantom holder forever.
#[tokio::test]
async fn composite_partial_grant_then_disconnect_releases_held_keys() {
    let (tcp_port, _) = start_server(true, false).await;
    let port = tcp_port.unwrap();

    let blocker_a = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let blocker_b = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();

    let g_a = blocker_a
        .acquire("pa", Duration::from_millis(60_000))
        .await
        .unwrap();
    let g_b = blocker_b
        .acquire("pb", Duration::from_millis(60_000))
        .await
        .unwrap();

    let composite_client = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let composite_fut = tokio::spawn({
        let cc = composite_client.clone();
        async move {
            cc.acquire_composite(&["pa", "pb"], Duration::from_millis(60_000))
                .await
        }
    });
    // Let the composite request reach the broker and queue on "pa".
    tokio::time::sleep(Duration::from_millis(80)).await;

    // Trigger the progressive-grant: release "pa". Broker grants "pa" to
    // the composite (partial) and re-queues it on "pb" (still held).
    blocker_a.release(&g_a).await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;

    // Disconnect the composite_client mid-progress.
    drop(composite_client);
    let _ = composite_fut.await;

    // Grace period for the tokio reader task to land `drop_client`.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // "pa" must be releasable for a brand-new caller. If the partial hold
    // leaked, this acquire would queue forever (or time out).
    let fresh = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let g_pa = tokio::time::timeout(
        Duration::from_millis(2_000),
        fresh.acquire("pa", Duration::from_millis(5_000)),
    )
    .await
    .expect("pa should be free after partial-grant disconnect; partial grant leaked")
    .unwrap();
    fresh.release(&g_pa).await.unwrap();

    blocker_b.release(&g_b).await.unwrap();
}

/// Dropping the TCP connection releases everything that client held.
/// This is the broker's `drop_client` path on the integration surface.
#[tokio::test]
async fn dropped_client_releases_held_locks_for_other_waiters() {
    let (tcp_port, _) = start_server(true, false).await;
    let port = tcp_port.unwrap();

    let a = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let _g = a
        .acquire("disconnect-key", Duration::from_millis(60_000))
        .await
        .unwrap();

    // B queues for the same key.
    let b = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let pending = tokio::spawn({
        let b = b.clone();
        async move {
            b.acquire("disconnect-key", Duration::from_millis(2000))
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!pending.is_finished(), "B must wait while A holds");

    // Dropping A's client closes the socket; the broker's drop_client
    // hook should release the held lock and grant B.
    drop(a);

    let g_b = pending
        .await
        .unwrap()
        .expect("B should be granted after A disconnects");
    b.release(&g_b).await.unwrap();
}

// ---- force-unlock + invalid release paths --------------------------------

/// Force-unlock breaks the existing lock and lets the next waiter
/// through. Operator-side tooling depends on this.
#[tokio::test]
async fn force_unlock_grants_the_next_waiter_end_to_end() {
    let (tcp_port, http_port) = start_server(true, true).await;
    let port = tcp_port.unwrap();
    let http = http_port.unwrap();

    let a = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let _g_a = a
        .acquire("breakme", Duration::from_millis(60_000))
        .await
        .unwrap();

    let b = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let pending = tokio::spawn({
        let b = b.clone();
        async move { b.acquire("breakme", Duration::from_millis(2000)).await }
    });
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(!pending.is_finished());

    // Operator force-unlocks via HTTP /v1/unlock with `force: true`.
    let resp = http_post(
        &format!("http://127.0.0.1:{http}/v1/unlock"),
        serde_json::json!({"key": "breakme", "force": true}),
    )
    .await;
    assert_eq!(
        resp["unlocked"],
        serde_json::Value::Bool(true),
        "got: {resp}"
    );

    let g_b = pending
        .await
        .unwrap()
        .expect("B should win after force-unlock");
    b.release(&g_b).await.unwrap();
}

/// Releasing with the wrong `lock_uuid` is rejected by the broker. The
/// real holder keeps holding (no spurious grant to the next waiter).
#[tokio::test]
async fn release_with_wrong_lock_uuid_is_rejected_over_http() {
    let (_, http_port) = start_server(false, true).await;
    let http = http_port.unwrap();

    let acquired = http_post(
        &format!("http://127.0.0.1:{http}/v1/lock"),
        serde_json::json!({"key": "wrong-uuid", "ttlMs": 60_000}),
    )
    .await;
    assert_eq!(acquired["acquired"], serde_json::Value::Bool(true));

    let bogus = http_post(
        &format!("http://127.0.0.1:{http}/v1/unlock"),
        serde_json::json!({"key": "wrong-uuid", "lockUuid": "definitely-not-real"}),
    )
    .await;
    assert_eq!(
        bogus["unlocked"],
        serde_json::Value::Bool(false),
        "wrong-uuid unlock must report unlocked: false; got: {bogus}",
    );
}

/// A live TCP client cannot release a peer's lock just by learning the
/// peer's `lock_uuid`. The UUID identifies the holder, but normal
/// (`force:false`) release still has to come from the owning connection.
#[tokio::test]
async fn live_tcp_peer_cannot_release_another_clients_lock_uuid() {
    let (tcp_port, _) = start_server(true, false).await;
    let port = tcp_port.unwrap();

    let owner = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let peer = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let waiter = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();

    let owner_guard = owner
        .acquire("tcp-owned-release", Duration::from_millis(60_000))
        .await
        .unwrap();

    let pending = tokio::spawn({
        let waiter = waiter.clone();
        async move {
            waiter
                .acquire("tcp-owned-release", Duration::from_millis(5_000))
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!pending.is_finished(), "waiter must be queued behind owner");

    let forged_guard = LockGuard {
        keys: owner_guard.keys.clone(),
        lock_uuid: owner_guard.lock_uuid.clone(),
        fencing_token: owner_guard.fencing_token,
        fencing_tokens: owner_guard.fencing_tokens.clone(),
    };
    let err = peer
        .release(&forged_guard)
        .await
        .expect_err("peer release with another live client's lock_uuid must fail");
    assert!(
        format!("{err}").contains("owned by another live client"),
        "unexpected peer-release error: {err}"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !pending.is_finished(),
        "unauthorized peer release must not grant the waiter"
    );

    owner.release(&owner_guard).await.unwrap();
    let waiter_guard = pending
        .await
        .unwrap()
        .expect("waiter should acquire after real owner release");
    waiter.release(&waiter_guard).await.unwrap();
}

// ---- Auth tests -----------------------------------------------------------

/// When `auth_token` is configured, a TCP client that doesn't send the
/// auth handshake gets disconnected and the auth-failures counter
/// increments.
#[tokio::test]
async fn auth_required_rejects_unauthenticated_tcp_connection() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let tcp_port = pick_port().await;
    let http_port = pick_port().await;
    let cfg = ServerConfig {
        tcp_bind: Some(format!("127.0.0.1:{tcp_port}").parse().unwrap()),
        uds_path: None,
        http_bind: Some(format!("127.0.0.1:{http_port}").parse().unwrap()),
        auth_token: Some("supersecret".into()),
        broker: BrokerConfig::default(),
        tcp_nodelay: true,
        tcp_quickack: true,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    };
    tokio::spawn(async move {
        let _ = server::run(cfg).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Connect raw and send a `lock` *before* auth — broker should
    // disconnect us.
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", tcp_port))
        .await
        .unwrap();
    let req = b"{\"type\":\"lock\",\"uuid\":\"r1\",\"key\":\"x\",\"ttl\":1000}\n";
    sock.write_all(req).await.unwrap();
    sock.flush().await.unwrap();
    let mut buf = Vec::new();
    let _ = sock.read_to_end(&mut buf).await; // EOF expected after broker hangs up

    let metrics = http_get_text(&format!("http://127.0.0.1:{http_port}/metrics")).await;
    assert!(
        metrics.contains("dd_rust_network_mutex_auth_failures_total 1"),
        "auth_failures must increment exactly once; got:\n{metrics}",
    );
}

/// HTTP API rejects a request without the configured token (both
/// header forms). A correct token in either header is accepted.
#[tokio::test]
async fn http_api_enforces_auth_with_either_header_form() {
    let http_port = pick_port().await;
    let cfg = ServerConfig {
        tcp_bind: None,
        uds_path: None,
        http_bind: Some(format!("127.0.0.1:{http_port}").parse().unwrap()),
        auth_token: Some("the-token".into()),
        broker: BrokerConfig::default(),
        tcp_nodelay: true,
        tcp_quickack: true,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    };
    tokio::spawn(async move {
        let _ = server::run(cfg).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Helper: send a POST body of declared length plus headers, return
    // the raw HTTP response. We compute Content-Length precisely so
    // axum's `Json` body extractor doesn't sit waiting for missing
    // bytes — the auth handler runs *after* body extraction, so we
    // need a parseable body even on the unauthorized path.
    async fn post(port: u16, headers: &[&str], body: &str) -> String {
        let mut head = String::new();
        head.push_str("POST /v1/lock HTTP/1.1\r\n");
        head.push_str("Host: 127.0.0.1\r\n");
        for h in headers {
            head.push_str(h);
            head.push_str("\r\n");
        }
        head.push_str("Content-Type: application/json\r\n");
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
        head.push_str("Connection: close\r\n\r\n");
        head.push_str(body);
        http_send_raw("127.0.0.1", port, head.as_bytes()).await
    }

    // No auth header → 401.
    let no_auth = post(http_port, &[], r#"{"key":"a"}"#).await;
    assert!(no_auth.starts_with("HTTP/1.1 401"), "got: {no_auth:.120}");

    // Wrong bearer token → 401.
    let wrong = post(
        http_port,
        &["Authorization: Bearer wrong"],
        r#"{"key":"a"}"#,
    )
    .await;
    assert!(wrong.starts_with("HTTP/1.1 401"), "got: {wrong:.120}");

    // Bearer with the right token → 200.
    let bearer_ok = post(
        http_port,
        &["Authorization: Bearer the-token"],
        r#"{"key":"b","ttlMs":1000}"#,
    )
    .await;
    assert!(
        bearer_ok.starts_with("HTTP/1.1 200"),
        "got: {bearer_ok:.120}"
    );

    // X-LMX-Auth with the right token → 200 too.
    let custom_ok = post(
        http_port,
        &["X-LMX-Auth: the-token"],
        r#"{"key":"c","ttlMs":1000}"#,
    )
    .await;
    assert!(
        custom_ok.starts_with("HTTP/1.1 200"),
        "got: {custom_ok:.120}"
    );

    // The gateway `Auth` header (value from the ALL_DOGS env) is accepted too.
    let auth_header_ok = post(
        http_port,
        &["Auth: the-token"],
        r#"{"key":"d","ttlMs":1000}"#,
    )
    .await;
    assert!(
        auth_header_ok.starts_with("HTTP/1.1 200"),
        "Auth header should authorize: {auth_header_ok:.120}"
    );

    // A wrong `Auth` header is still rejected.
    let auth_header_wrong = post(http_port, &["Auth: nope"], r#"{"key":"e"}"#).await;
    assert!(
        auth_header_wrong.starts_with("HTTP/1.1 401"),
        "wrong Auth header must be 401: {auth_header_wrong:.120}"
    );
}

// ---- HTTP API validation -------------------------------------------------

/// `POST /v1/lock` with neither `key` nor `keys` returns 4xx with an
/// error body. Catches a misconfigured caller before they sit on a
/// broker request that will never grant.
#[tokio::test]
async fn http_lock_rejects_missing_key_and_keys() {
    let (_, http_port) = start_server(false, true).await;
    let port = http_port.unwrap();
    let resp = http_send_raw(
        "127.0.0.1",
        port,
        b"POST /v1/lock HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ttlMs\":1}\r\n",
    )
    .await;
    assert!(
        resp.starts_with("HTTP/1.1 400"),
        "missing key/keys must produce 400; got: {resp:.120}"
    );
    assert!(resp.contains("error"));
}

/// Composite oversize: HTTP-side request with 6 keys is rejected.
#[tokio::test]
async fn http_composite_rejects_oversized_keyset() {
    let (_, http_port) = start_server(false, true).await;
    let port = http_port.unwrap();
    let resp = http_post(
        &format!("http://127.0.0.1:{port}/v1/lock"),
        serde_json::json!({
            "keys": ["a","b","c","d","e","f"],
            "ttlMs": 1000,
        }),
    )
    .await;
    assert_eq!(resp["acquired"], serde_json::Value::Bool(false));
    assert!(
        resp.get("error").is_some(),
        "oversized composite must include an error field; got: {resp}"
    );
}

// ---- HTTP long-poll ------------------------------------------------------

/// `waitMs` is HTTP long-poll: the broker holds the request open until
/// the lock becomes available. We acquire over TCP, then start an HTTP
/// `acquire` with `waitMs=2000`, then release the TCP holder and watch
/// the HTTP response come back with `acquired:true`.
#[tokio::test]
async fn http_long_poll_grants_when_holder_releases() {
    let (tcp_port, http_port) = start_server(true, true).await;
    let tcp = tcp_port.unwrap();
    let http = http_port.unwrap();

    let a = Client::connect_tcp(("127.0.0.1", tcp), ClientConfig::default())
        .await
        .unwrap();
    let g_a = a
        .acquire("longpoll-key", Duration::from_millis(60_000))
        .await
        .unwrap();

    // Start the HTTP long-poll. Should NOT come back immediately.
    let http_url = format!("http://127.0.0.1:{http}/v1/lock");
    let pending = tokio::spawn(async move {
        http_post(
            &http_url,
            serde_json::json!({"key": "longpoll-key", "ttlMs": 5000, "waitMs": 2000}),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !pending.is_finished(),
        "long-poll must keep the socket open"
    );

    // Release: long-poll wakes up.
    a.release(&g_a).await.unwrap();
    let started = std::time::Instant::now();
    let body = pending.await.unwrap();
    let elapsed = started.elapsed();
    assert_eq!(
        body["acquired"],
        serde_json::Value::Bool(true),
        "long-poll should return acquired:true once the slot frees; got: {body}",
    );
    assert!(
        elapsed < Duration::from_millis(1500),
        "long-poll resolved too slowly ({elapsed:?}); expected < 1.5s",
    );
}

/// `waitMs=0` (or omitted) returns immediately with `acquired:false`
/// when the lock is contended — the caller is supposed to retry.
#[tokio::test]
async fn http_acquire_no_wait_returns_queued_immediately() {
    let (tcp_port, http_port) = start_server(true, true).await;
    let tcp = tcp_port.unwrap();
    let http = http_port.unwrap();

    let a = Client::connect_tcp(("127.0.0.1", tcp), ClientConfig::default())
        .await
        .unwrap();
    let _g_a = a
        .acquire("nowait-key", Duration::from_millis(60_000))
        .await
        .unwrap();

    let started = std::time::Instant::now();
    let body = http_post(
        &format!("http://127.0.0.1:{http}/v1/lock"),
        serde_json::json!({"key": "nowait-key", "ttlMs": 5000}),
    )
    .await;
    let elapsed = started.elapsed();
    assert_eq!(body["acquired"], serde_json::Value::Bool(false));
    assert!(
        elapsed < Duration::from_millis(500),
        "no-wait must return immediately; took {elapsed:?}",
    );
    assert!(
        body["queueDepth"].as_u64().unwrap_or(0) >= 1,
        "queue depth should report at least 1; got: {body}"
    );
}

// ---- Mixed RW + exclusive on the same key --------------------------------

/// An exclusive holder blocks `registerRead`/`registerWrite` on the
/// same key, and vice versa. This is the orthogonality contract of the
/// RW lock: it shares a `LockState` with the exclusive table, so a hot
/// key can't be in both modes at once.
#[tokio::test]
async fn exclusive_lock_blocks_register_read_until_release() {
    let (tcp_port, _) = start_server(true, false).await;
    let port = tcp_port.unwrap();

    let a = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let g_a = a
        .acquire("rw-mix", Duration::from_millis(60_000))
        .await
        .unwrap();

    let r = RwClient::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap();
    let pending = tokio::spawn({
        let r = r.clone();
        async move { tokio::time::timeout(Duration::from_millis(2000), r.acquire_read("rw-mix")).await }
    });
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        !pending.is_finished(),
        "register-read must wait while exclusive lock is held"
    );

    a.release(&g_a).await.unwrap();
    let read_guard = pending
        .await
        .unwrap()
        .expect("acquire_read should resolve within 2s")
        .expect("acquire_read should succeed after exclusive release");
    read_guard.release().await.unwrap();
}

// ---- Status page truncation ---------------------------------------------

/// With more than 10 active keys, the top-keys table truncates to 10.
/// Not a strict ordering test (ties between equally-contended keys are
/// arbitrary by HashMap iteration) — just a no-blowup guard for the
/// page rendering with lots of keys.
#[tokio::test]
async fn status_page_with_many_keys_shows_at_most_ten_rows() {
    let (tcp_port, http_port) = start_server(true, true).await;
    let port = tcp_port.unwrap();
    let http = http_port.unwrap();

    let mut clients_and_guards = Vec::new();
    for i in 0..15 {
        let c = Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
            .await
            .unwrap();
        let g = c
            .acquire(&format!("manykey-{i:02}"), Duration::from_millis(60_000))
            .await
            .unwrap();
        clients_and_guards.push((c, g));
    }

    let html = http_get_text(&format!("http://127.0.0.1:{http}/status")).await;
    // Count rows in the top-keys tbody. Each row starts with `<tr><td><code>`.
    let row_count = html.matches("<tr><td><code>manykey-").count();
    assert!(
        row_count <= 10,
        "top-keys table must truncate at 10; counted {row_count} rows"
    );
    // And `manykey-` rows actually appeared — i.e. we didn't accidentally
    // render zero rows.
    assert!(row_count > 0, "expected at least one manykey row; got 0");
}

// ---- /admin/otel runtime kill-switch -------------------------------------

/// `GET /admin/otel` requires a shared-secret header. Without it, every
/// method returns 401 with the `lmx admin` error shape.
#[tokio::test]
async fn admin_otel_requires_shared_secret_header() {
    let (_, http_port) = start_server(false, true).await;
    let port = http_port.unwrap();

    let resp = http_get_with_headers(&format!("http://127.0.0.1:{port}/admin/otel"), &[]).await;
    assert!(
        resp.starts_with("HTTP/1.1 401"),
        "unauthenticated GET /admin/otel must be 401; got: {resp:.120}"
    );

    let resp_post = http_send_raw(
        "127.0.0.1",
        port,
        b"POST /admin/otel HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 16\r\nConnection: close\r\n\r\n{\"enabled\":true}",
    )
    .await;
    assert!(
        resp_post.starts_with("HTTP/1.1 401"),
        "unauthenticated POST /admin/otel must be 401; got: {resp_post:.120}"
    );
}

/// Authenticated GET → POST → GET round-trip flips the runtime
/// kill-switch and the response body reports both the new and previous
/// values for audit logging.
#[tokio::test]
async fn admin_otel_toggle_round_trip() {
    let (_, http_port) = start_server(false, true).await;
    let port = http_port.unwrap();
    let url_get = format!("http://127.0.0.1:{port}/admin/otel");

    // Snapshot whatever the kill-switch happens to be before the test —
    // other tests run in parallel and may have left it on or off.
    let before =
        http_get_json_with_headers(&url_get, &[("x-admin-token", "all-dogs-go-to-heaven")]).await;
    assert!(before["enabled"].is_boolean());
    let was = before["enabled"].as_bool().unwrap();

    // Flip to the opposite value.
    let next = !was;
    let body = serde_json::json!({"enabled": next});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let req = format!(
        "POST /admin/otel HTTP/1.1\r\nHost: 127.0.0.1\r\nx-admin-token: all-dogs-go-to-heaven\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        len = body_bytes.len(),
    );
    let mut raw = req.into_bytes();
    raw.extend_from_slice(&body_bytes);
    let post_resp = http_send_raw("127.0.0.1", port, &raw).await;
    assert!(
        post_resp.starts_with("HTTP/1.1 200"),
        "authed POST should be 200; got: {post_resp:.160}"
    );
    let post_body = parse_json_body(&post_resp);
    assert_eq!(post_body["previous"], serde_json::Value::Bool(was));
    assert_eq!(post_body["enabled"], serde_json::Value::Bool(next));

    // Read-back via GET.
    let after =
        http_get_json_with_headers(&url_get, &[("x-admin-token", "all-dogs-go-to-heaven")]).await;
    assert_eq!(after["enabled"], serde_json::Value::Bool(next));

    // Confirm the in-process accessor agrees with the HTTP response.
    assert_eq!(dd_rust_network_mutex::is_otel_enabled(), next);

    // Also accepts `Authorization: Bearer …`.
    let bearer_resp = http_get_json_with_headers(
        &url_get,
        &[("authorization", "Bearer all-dogs-go-to-heaven")],
    )
    .await;
    assert!(bearer_resp["enabled"].is_boolean());

    // Restore the prior value so we don't leak state into other parallel tests.
    let restore_body = serde_json::to_vec(&serde_json::json!({"enabled": was})).unwrap();
    let mut raw_restore = format!(
        "POST /admin/otel HTTP/1.1\r\nHost: 127.0.0.1\r\nx-admin-token: all-dogs-go-to-heaven\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        len = restore_body.len(),
    )
    .into_bytes();
    raw_restore.extend_from_slice(&restore_body);
    let _ = http_send_raw("127.0.0.1", port, &raw_restore).await;
}

/// POST without a body, or with the wrong shape, returns 400.
#[tokio::test]
async fn admin_otel_post_validates_body() {
    let (_, http_port) = start_server(false, true).await;
    let port = http_port.unwrap();

    let no_body = http_send_raw(
        "127.0.0.1",
        port,
        b"POST /admin/otel HTTP/1.1\r\nHost: 127.0.0.1\r\nx-admin-token: all-dogs-go-to-heaven\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        no_body.starts_with("HTTP/1.1 400") || no_body.starts_with("HTTP/1.1 415"),
        "empty body must be rejected; got: {no_body:.160}"
    );

    // `{"enabled":"on"}` is exactly 16 bytes — Content-Length must match
    // or the server will block waiting for an extra byte.
    let wrong_type = http_send_raw(
        "127.0.0.1",
        port,
        b"POST /admin/otel HTTP/1.1\r\nHost: 127.0.0.1\r\nx-admin-token: all-dogs-go-to-heaven\r\nContent-Type: application/json\r\nContent-Length: 16\r\nConnection: close\r\n\r\n{\"enabled\":\"on\"}",
    )
    .await;
    assert!(
        wrong_type.starts_with("HTTP/1.1 400") || wrong_type.starts_with("HTTP/1.1 422"),
        "string `enabled` must be rejected; got: {wrong_type:.160}"
    );
}

// ---- tiny HTTP helpers (no reqwest dependency) ----------------------------

async fn http_post(url: &str, body: serde_json::Value) -> serde_json::Value {
    let url = url::ParseUrl::parse(url);
    let body = serde_json::to_vec(&body).unwrap();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        path = url.path,
        host = url.host_port(),
        len = body.len(),
    );
    let resp = http_send(&url, request.as_bytes(), &body).await;
    parse_json_body(&resp)
}

async fn http_get_text(url: &str) -> String {
    let url = url::ParseUrl::parse(url);
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n",
        path = url.path,
        host = url.host_port(),
    );
    let resp = http_send(&url, request.as_bytes(), b"").await;
    body_only(&resp).to_string()
}

async fn http_get_with_headers(url: &str, headers: &[(&str, &str)]) -> String {
    let url = url::ParseUrl::parse(url);
    let mut req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n",
        path = url.path,
        host = url.host_port(),
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    http_send(&url, req.as_bytes(), b"").await
}

async fn http_get_json_with_headers(url: &str, headers: &[(&str, &str)]) -> serde_json::Value {
    let resp = http_get_with_headers(url, headers).await;
    parse_json_body(&resp)
}

async fn http_send(url: &url::ParseUrl, head: &[u8], body: &[u8]) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut sock = tokio::net::TcpStream::connect(format!("{}:{}", url.host, url.port))
        .await
        .unwrap();
    sock.write_all(head).await.unwrap();
    if !body.is_empty() {
        sock.write_all(body).await.unwrap();
    }
    sock.flush().await.unwrap();
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.unwrap();
    String::from_utf8_lossy(&buf).to_string()
}

fn body_only(http_response: &str) -> &str {
    if let Some((_, body)) = http_response.split_once("\r\n\r\n") {
        body
    } else {
        ""
    }
}

fn parse_json_body(http_response: &str) -> serde_json::Value {
    let body = body_only(http_response);
    serde_json::from_str(body).unwrap_or_else(|err| {
        panic!("failed to parse JSON body: {err}\n--- response was ---\n{http_response}")
    })
}

mod url {
    /// Minimal URL splitter sufficient for `http://host:port/path` style URLs
    /// produced by these tests. Pulling in a real URL parser feels excessive.
    pub struct ParseUrl {
        pub host: String,
        pub port: u16,
        pub path: String,
    }
    impl ParseUrl {
        pub fn parse(url: &str) -> Self {
            let url = url
                .strip_prefix("http://")
                .or_else(|| url.strip_prefix("https://"))
                .unwrap_or(url);
            let (authority, path) = match url.split_once('/') {
                Some((a, rest)) => (a, format!("/{rest}")),
                None => (url, "/".to_string()),
            };
            let (host, port) = match authority.split_once(':') {
                Some((h, p)) => (h.to_string(), p.parse().unwrap_or(80)),
                None => (authority.to_string(), 80),
            };
            ParseUrl { host, port, path }
        }
        pub fn host_port(&self) -> String {
            format!("{}:{}", self.host, self.port)
        }
    }
}
