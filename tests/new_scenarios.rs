//! Additional end-to-end scenario coverage layered on top of `integration.rs`.
//!
//! These focus on invariants that the cross-runtime conformance harness also
//! exercises against the upstream Node `live-mutex` broker:
//!
//! * semaphore (`max > 1`) concurrency limits,
//! * the mutual-exclusion invariant under heavy contention (a shared
//!   "currently held" counter that must never exceed the cap),
//! * composite locks requested in opposite key order serialise cleanly,
//! * fencing tokens are unique + monotonic across handoffs and across
//!   concurrent semaphore slots,
//! * `lock_info` / `ls` reflect held-vs-released state.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dd_rust_network_mutex::{server, BrokerConfig, Client, ClientConfig, ServerConfig};
use tokio::net::TcpListener;

async fn pick_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

async fn start_tcp_server() -> u16 {
    let port = pick_port().await;
    let cfg = ServerConfig {
        tcp_bind: Some(format!("127.0.0.1:{port}").parse().unwrap()),
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
    port
}

async fn connect(port: u16) -> Client {
    Client::connect_tcp(("127.0.0.1", port), ClientConfig::default())
        .await
        .unwrap()
}

/// `max = 3` lets three holders in simultaneously and queues the fourth until
/// a slot frees up.
#[tokio::test]
async fn semaphore_allows_max_holders_then_queues() {
    let port = start_tcp_server().await;
    let a = connect(port).await;
    let b = connect(port).await;
    let c = connect(port).await;
    let d = connect(port).await;

    let key = "sem-key";
    let ttl = Duration::from_millis(5000);
    let ga = a.acquire_with_max(key, 3, ttl).await.unwrap();
    let gb = b.acquire_with_max(key, 3, ttl).await.unwrap();
    let gc = c.acquire_with_max(key, 3, ttl).await.unwrap();

    // All three slots issue distinct lock_uuids + fencing tokens.
    let tokens: BTreeSet<u64> = [&ga, &gb, &gc]
        .iter()
        .map(|g| g.fencing_token.unwrap())
        .collect();
    assert_eq!(
        tokens.len(),
        3,
        "each semaphore slot needs a unique fencing token"
    );

    // Fourth acquire must queue while the semaphore is saturated.
    let d_clone = d.clone();
    let acquire_d = tokio::spawn(async move { d_clone.acquire_with_max(key, 3, ttl).await });
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !acquire_d.is_finished(),
        "4th holder must wait while 3/3 slots are held"
    );

    a.release(&ga).await.unwrap();
    let gd = acquire_d.await.unwrap().unwrap();
    assert!(gd.fencing_token.unwrap() > 0);

    b.release(&gb).await.unwrap();
    c.release(&gc).await.unwrap();
    d.release(&gd).await.unwrap();
}

/// `max = 0` is rejected by the typed helper before it hits the wire.
#[tokio::test]
async fn semaphore_zero_is_rejected() {
    let port = start_tcp_server().await;
    let client = connect(port).await;
    let err = client
        .acquire_with_max("zero", 0, Duration::from_millis(1000))
        .await;
    assert!(err.is_err(), "max=0 must be rejected, got {err:?}");
}

/// Under heavy contention on a single exclusive key, the broker must never let
/// two holders in at once, every grant must be unique, and no acquisition can
/// be lost.
#[tokio::test]
async fn mutual_exclusion_invariant_under_contention() {
    let port = start_tcp_server().await;

    const WORKERS: usize = 8;
    const ITERS: usize = 25;
    let key = "hot-key";
    let active = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for _ in 0..WORKERS {
        let client = connect(port).await;
        let active = active.clone();
        let max_seen = max_seen.clone();
        handles.push(tokio::spawn(async move {
            let mut tokens = Vec::with_capacity(ITERS);
            for _ in 0..ITERS {
                let g = client
                    .acquire(key, Duration::from_millis(5000))
                    .await
                    .unwrap();
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                // Record the high-water mark of concurrent holders.
                max_seen.fetch_max(now, Ordering::SeqCst);
                assert_eq!(now, 1, "exclusive lock held by more than one client");
                tokio::task::yield_now().await;
                active.fetch_sub(1, Ordering::SeqCst);
                tokens.push(g.fencing_token.unwrap());
                client.release(&g).await.unwrap();
            }
            tokens
        }));
    }

    let mut all_tokens = Vec::new();
    for h in handles {
        all_tokens.extend(h.await.unwrap());
    }

    assert_eq!(
        max_seen.load(Ordering::SeqCst),
        1,
        "mutual exclusion violated"
    );
    assert_eq!(all_tokens.len(), WORKERS * ITERS, "lost acquisitions");
    let unique: BTreeSet<u64> = all_tokens.iter().copied().collect();
    assert_eq!(
        unique.len(),
        all_tokens.len(),
        "fencing tokens must be globally unique"
    );
}

/// Fencing tokens strictly increase across sequential handoffs of one key.
#[tokio::test]
async fn fencing_tokens_strictly_increase_across_handoffs() {
    let port = start_tcp_server().await;
    let client = connect(port).await;
    let key = "mono-key";
    let mut last = 0u64;
    for i in 0..12 {
        let g = client
            .acquire(key, Duration::from_millis(2000))
            .await
            .unwrap();
        let t = g.fencing_token.unwrap();
        assert!(t > last, "token must increase: iter {i}, {t} !> {last}");
        last = t;
        client.release(&g).await.unwrap();
    }
}

/// Two composite requests for the same key set in opposite order serialise
/// cleanly (broker sorts keys internally — no deadlock, no partial hold).
#[tokio::test]
async fn composite_reversed_order_serialises_without_deadlock() {
    let port = start_tcp_server().await;
    let a = connect(port).await;
    let b = connect(port).await;

    let ga = a
        .acquire_composite(&["x", "y"], Duration::from_millis(5000))
        .await
        .unwrap();
    assert_eq!(ga.fencing_tokens.len(), 2);

    // B requests the same keys in reverse order; it must queue, not deadlock.
    let b_clone = b.clone();
    let acquire_b = tokio::spawn(async move {
        b_clone
            .acquire_composite(&["y", "x"], Duration::from_millis(5000))
            .await
    });
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !acquire_b.is_finished(),
        "B must wait for A's composite hold"
    );

    a.release(&ga).await.unwrap();
    let gb = acquire_b.await.unwrap().unwrap();
    assert_eq!(gb.fencing_tokens.len(), 2);
    // Each key's token advanced past A's hold.
    for k in ["x", "y"] {
        assert!(
            gb.fencing_tokens[k] > ga.fencing_tokens[k],
            "token for {k} must advance across the handoff"
        );
    }
    b.release(&gb).await.unwrap();
}

/// `lock_info` and `ls` reflect held vs released state.
#[tokio::test]
async fn lock_info_and_ls_reflect_state() {
    let port = start_tcp_server().await;
    let client = connect(port).await;
    let key = "introspect-key";

    let g = client
        .acquire(key, Duration::from_millis(5000))
        .await
        .unwrap();
    let info = client.lock_info(key).await.unwrap();
    assert!(info.is_locked, "lock_info should report held");
    assert_eq!(info.lockholder_uuids.len(), 1);

    let listed = client.ls().await.unwrap();
    assert!(
        listed.iter().any(|k| k == key),
        "ls should include held key"
    );

    client.release(&g).await.unwrap();
    let info_after = client.lock_info(key).await.unwrap();
    assert!(!info_after.is_locked, "lock_info should report released");
}
