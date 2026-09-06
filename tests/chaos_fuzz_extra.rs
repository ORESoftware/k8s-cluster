//! Additional chaos / robustness tests for the Rust broker.
//!
//! These extend `chaos_fuzz.rs` with four scenarios that target operational
//! properties the existing suite doesn't exercise:
//!
//! 1. `tcp_disconnect_burst_releases_holders` — N clients each acquire and
//!    drop their TCP socket without releasing. The broker must reap every
//!    holder and the queue, and `lock_info` must report the key clean within
//!    a bounded window. This asserts the `drop_client` path is correct under
//!    a thundering-herd disconnect.
//!
//! 2. `high_contention_fairness_is_fifo` — M clients enqueue in a known
//!    order on a single hot key (each starts after the previous one is
//!    confirmed enqueued). Every grant order must equal the enqueue order
//!    and every fencing token must be strictly monotonic.
//!
//! 3. `rw_writer_eventually_granted_under_reader_load` — a steady stream of
//!    readers contends with a single writer. The writer must be granted
//!    within a bounded latency that does not grow unboundedly with reader
//!    arrivals (no writer starvation).
//!
//! 4. `fencing_token_advances_across_broker_restart` — a token minted from
//!    broker A and a token minted from broker B (started after A is
//!    stopped) on the same key must be strictly increasing across the
//!    process boundary, since the counter seeds from wall-clock millis at
//!    startup. This protects downstream Kleppmann-pattern consumers from
//!    accepting stale fences after a broker rotation.
//!
//! Each test is self-contained so failures point at one property.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dd_rust_network_mutex::{
    server::{run as run_server, ServerConfig},
    BrokerConfig, Client, ClientConfig, RwClient,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinSet;

// ---------------------------------------------------------------------------
// Shared harness — kept compact since each integration test compiles as its
// own crate and Rust integration tests can't import from peer files cleanly.
// ---------------------------------------------------------------------------

async fn pick_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Starts a broker on an ephemeral 127.0.0.1 port. Returns `(port, handle)`;
/// dropping or aborting the handle stops the broker listener and the
/// per-connection tasks it spawned.
async fn start_broker(broker_cfg: BrokerConfig) -> (u16, tokio::task::JoinHandle<()>) {
    let port = pick_port().await;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let config = ServerConfig {
        tcp_bind: Some(addr),
        uds_path: None,
        http_bind: None,
        auth_token: None,
        broker: broker_cfg,
        tcp_nodelay: true,
        tcp_quickack: false,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    };
    let handle = tokio::spawn(async move {
        if let Err(err) = run_server(config).await {
            eprintln!("broker exited: {err:?}");
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (port, handle)
}

fn client_cfg(timeout_ms: u64) -> ClientConfig {
    ClientConfig {
        auth_token: None,
        default_request_timeout: Duration::from_millis(timeout_ms),
    }
}

// ---------------------------------------------------------------------------
// 1. TCP disconnect burst — verify drop_client cleans up holders + queue
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_disconnect_burst_releases_holders() {
    let (port, server) = start_broker(BrokerConfig::default()).await;

    // Acquire N exclusive locks via N independent clients on N distinct keys,
    // then drop every client without releasing. Use distinct keys so the
    // grants succeed without queueing — the cleanup we want to assert is
    // "holder set returns to empty", not "queue drains".
    let n_clients = 80;
    let keys: Vec<String> = (0..n_clients).map(|i| format!("burst-key-{i}")).collect();

    let mut held_uuids: Vec<String> = Vec::new();
    let mut clients: Vec<Client> = Vec::with_capacity(n_clients);
    for key in &keys {
        let client = Client::connect_tcp(("127.0.0.1", port), client_cfg(5_000))
            .await
            .expect("connect_tcp");
        let guard = client
            .acquire(key, Duration::from_secs(60))
            .await
            .expect("acquire");
        held_uuids.push(guard.lock_uuid.clone());
        clients.push(client);
    }

    // Sanity: an inspector client sees each key as held.
    let inspector = Client::connect_tcp(("127.0.0.1", port), client_cfg(5_000))
        .await
        .unwrap();
    for key in &keys {
        let info = inspector.lock_info(key).await.unwrap();
        assert!(
            info.is_locked,
            "expected key {key} held before disconnect, got {info:?}",
        );
    }

    // Drop every holder client. tokio drops the underlying TcpStream which
    // closes the socket; the broker's per-connection task observes EOF and
    // calls `drop_client`.
    drop(clients);

    // Poll until every key is observed unlocked or the deadline passes.
    // The broker reacts to socket close synchronously inside its read loop,
    // so 1s is generous; we use 5s to absorb scheduler jitter.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut still_locked: Vec<String> = keys.clone();
    while !still_locked.is_empty() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let mut next: Vec<String> = Vec::new();
        for key in &still_locked {
            let info = inspector.lock_info(key).await.unwrap();
            if info.is_locked || info.lock_request_count > 0 {
                next.push(key.clone());
            }
        }
        still_locked = next;
    }
    assert!(
        still_locked.is_empty(),
        "broker did not reap holders within 5s, leaked: {still_locked:?}",
    );

    // After cleanup, a fresh client must be able to acquire any of the keys.
    let recover = Client::connect_tcp(("127.0.0.1", port), client_cfg(5_000))
        .await
        .unwrap();
    for key in &keys {
        let g = recover
            .acquire(key, Duration::from_millis(500))
            .await
            .unwrap_or_else(|e| panic!("re-acquire {key} failed after burst: {e}"));
        recover.release(&g).await.unwrap();
    }

    server.abort();
}

// ---------------------------------------------------------------------------
// 2. High-contention FIFO fairness on a single key
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn high_contention_fairness_is_fifo() {
    let (port, server) = start_broker(BrokerConfig::default()).await;

    let key = "fifo-key";
    let n_waiters: usize = 60;

    // Hold the key with one client so all N waiters must queue, then start
    // them one at a time so their enqueue order is deterministic.
    let holder = Client::connect_tcp(("127.0.0.1", port), client_cfg(15_000))
        .await
        .unwrap();
    let holder_guard = holder.acquire(key, Duration::from_secs(60)).await.unwrap();

    // Inspector verifies "ready to wait next" before the next waiter starts.
    let inspector = Client::connect_tcp(("127.0.0.1", port), client_cfg(5_000))
        .await
        .unwrap();

    let granted_order: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::with_capacity(n_waiters)));
    let tokens_in_grant_order: Arc<Mutex<Vec<u64>>> =
        Arc::new(Mutex::new(Vec::with_capacity(n_waiters)));
    let release_signal: Arc<Notify> = Arc::new(Notify::new());

    let mut tasks = JoinSet::new();
    for waiter_id in 0..n_waiters {
        let go = release_signal.clone();
        let granted_order = granted_order.clone();
        let tokens_in_grant_order = tokens_in_grant_order.clone();
        let key_owned = key.to_string();
        tasks.spawn(async move {
            let client = Client::connect_tcp(("127.0.0.1", port), client_cfg(30_000))
                .await
                .expect("waiter connect");
            // The acquire call enqueues this waiter. We can't synchronously
            // know "I'm enqueued" but the inspector loop below polls
            // `lock_request_count` to wait for it.
            let acquire_fut = client.acquire(&key_owned, Duration::from_millis(500));
            let guard = acquire_fut.await.expect("queued acquire");
            granted_order.lock().await.push(waiter_id);
            if let Some(tok) = guard.fencing_token {
                tokens_in_grant_order.lock().await.push(tok);
            }
            // Pause briefly so order is observable on the receiving side
            // before the next waiter is granted.
            go.notified().await;
            client.release(&guard).await.expect("release");
        });

        // Wait until this waiter is observed enqueued in the broker before
        // spawning the next one. This is what makes the enqueue order
        // deterministic.
        let target = waiter_id + 1; // queue depth excluding holder
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let info = inspector.lock_info(key).await.unwrap();
            if info.lock_request_count as usize >= target {
                break;
            }
            if Instant::now() > deadline {
                panic!(
                    "waiter {waiter_id} did not enqueue: observed depth {} after 10s",
                    info.lock_request_count,
                );
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    // Release the original holder. The broker should grant waiters in the
    // order they enqueued. We let each grant happen, observe it, then
    // signal the granted task to release so the next waiter can run.
    holder.release(&holder_guard).await.unwrap();

    let deadline = Instant::now() + Duration::from_secs(30);
    while granted_order.lock().await.len() < n_waiters {
        if Instant::now() > deadline {
            panic!(
                "only {}/{n_waiters} waiters granted",
                granted_order.lock().await.len()
            );
        }
        // Trickle releases so the queue can drain step-by-step.
        release_signal.notify_one();
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    // Drain final notifications so every task can release and exit.
    for _ in 0..n_waiters {
        release_signal.notify_one();
    }
    while let Some(r) = tasks.join_next().await {
        r.expect("waiter task");
    }

    let granted = granted_order.lock().await.clone();
    let tokens = tokens_in_grant_order.lock().await.clone();

    let expected: Vec<usize> = (0..n_waiters).collect();
    assert_eq!(
        granted, expected,
        "FIFO violated: grants out of enqueue order. expected {expected:?} got {granted:?}",
    );

    assert_eq!(tokens.len(), n_waiters);
    for window in tokens.windows(2) {
        assert!(
            window[1] > window[0],
            "fencing tokens not strictly increasing in grant order: {tokens:?}",
        );
    }

    server.abort();
}

// ---------------------------------------------------------------------------
// 3. RW writer is not starved by a steady reader workload
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rw_writer_eventually_granted_under_reader_load() {
    let (port, server) = start_broker(BrokerConfig::default()).await;
    let key = "rw-starv-key";

    // Steady reader load: many short-held reads in a hot loop. Without the
    // shared FIFO queue (readers/writers in the same wait list), writers
    // could be starved. With it, the writer arrives at some point in the
    // queue and is granted in bounded time.
    let stop = Arc::new(AtomicBool::new(false));
    let stop_notify = Arc::new(tokio::sync::Notify::new());
    let read_count = Arc::new(AtomicUsize::new(0));
    let n_reader_clients = 16;
    let mut reader_tasks = JoinSet::new();
    for _ in 0..n_reader_clients {
        let stop = stop.clone();
        let stop_notify = stop_notify.clone();
        let read_count = read_count.clone();
        let key_owned = key.to_string();
        reader_tasks.spawn(async move {
            let rw = RwClient::connect_tcp(("127.0.0.1", port), client_cfg(10_000))
                .await
                .expect("rw connect");
            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                if let Ok(guard) = rw.acquire_read(&key_owned).await {
                    read_count.fetch_add(1, Ordering::Relaxed);
                    // Brief hold to make readers actually contend with the
                    // writer rather than completing in microseconds.
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    let _ = guard.release().await;
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                tokio::select! {
                    _ = stop_notify.notified() => break,
                    _ = tokio::time::sleep(Duration::from_micros(200)) => {}
                }
            }
        });
    }

    // Let readers ramp up so the queue is non-empty when the writer arrives.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let baseline_reads = read_count.load(Ordering::Relaxed);
    assert!(
        baseline_reads > 50,
        "reader workload too light: only {baseline_reads} reads in 200ms",
    );

    // Now request a writer and time the wait. With a fair shared queue this
    // should complete within the reader-hold ceiling × queue-depth, well
    // under our 5s ceiling.
    let writer = RwClient::connect_tcp(("127.0.0.1", port), client_cfg(15_000))
        .await
        .unwrap();
    let started = Instant::now();
    let write_guard = writer
        .acquire_write(key)
        .await
        .expect("writer must be granted");
    let waited = started.elapsed();

    // Stop the readers and let the writer release.
    stop.store(true, Ordering::Relaxed);
    stop_notify.notify_waiters();
    write_guard.release().await.unwrap();

    while let Some(r) = reader_tasks.join_next().await {
        r.expect("reader task");
    }

    let total_reads = read_count.load(Ordering::Relaxed);
    eprintln!(
        "writer waited {}ms, total reads in run: {total_reads}",
        waited.as_millis(),
    );
    assert!(
        waited < Duration::from_secs(5),
        "writer starved for {}ms (expected <5s)",
        waited.as_millis(),
    );

    server.abort();
}

// ---------------------------------------------------------------------------
// 4. Fencing token advances across broker restart on the same key
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fencing_token_advances_across_broker_restart() {
    let key = "restart-fence-key";

    // Broker A: mint a fencing token by acquiring + releasing.
    let (port_a, server_a) = start_broker(BrokerConfig::default()).await;
    let a = Client::connect_tcp(("127.0.0.1", port_a), client_cfg(5_000))
        .await
        .unwrap();
    let g_a = a.acquire(key, Duration::from_secs(2)).await.unwrap();
    let token_a = g_a.fencing_token.expect("broker A returned no token");
    a.release(&g_a).await.unwrap();
    drop(a);
    server_a.abort();

    // Make sure wall-clock has advanced enough that the new broker's
    // wall-clock-millis seed is strictly greater than `token_a`. The seed
    // is `SystemTime::now().duration_since(UNIX_EPOCH).as_millis()` so 5ms
    // is sufficient; we use 10ms to absorb scheduler jitter.
    tokio::time::sleep(Duration::from_millis(10)).await;

    let (port_b, server_b) = start_broker(BrokerConfig::default()).await;
    let b = Client::connect_tcp(("127.0.0.1", port_b), client_cfg(5_000))
        .await
        .unwrap();
    let g_b = b.acquire(key, Duration::from_secs(2)).await.unwrap();
    let token_b = g_b.fencing_token.expect("broker B returned no token");
    b.release(&g_b).await.unwrap();

    assert!(
        token_b > token_a,
        "fencing token did not advance across restart: A={token_a} B={token_b}",
    );

    // Verify that within broker B, repeated acquires on the same key keep
    // tokens strictly monotonic. Many tests cover this in-process; here the
    // intent is to confirm the post-restart counter behaves sanely too.
    let mut last = token_b;
    let mut seen = HashSet::new();
    seen.insert(token_b);
    for _ in 0..16 {
        let g = b.acquire(key, Duration::from_secs(2)).await.unwrap();
        let t = g.fencing_token.unwrap();
        assert!(
            t > last,
            "post-restart token regressed: prev={last} next={t}",
        );
        assert!(
            seen.insert(t),
            "post-restart token duplicated: {t} already seen",
        );
        last = t;
        b.release(&g).await.unwrap();
    }

    server_b.abort();
}

// ---------------------------------------------------------------------------
// Aux smoke: confirm the helper actually exposes a working TCP endpoint.
// Catches helper regressions (e.g. server config drift) before the heavier
// chaos tests run.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_smoke_starts_and_serves() {
    let (port, server) = start_broker(BrokerConfig::default()).await;
    let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    drop(stream); // we just want to know the listener is bound
    server.abort();
}
