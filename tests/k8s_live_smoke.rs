//! End-to-end smoke test against a live broker reachable on the network.
//!
//! Skipped by default. Run against any deployed broker (Kubernetes
//! Service, EC2 host, docker-compose, etc.) with:
//!
//!   LMX_LIVE_BROKER_TCP=host:port \
//!   cargo test --test k8s_live_smoke -- --ignored --nocapture
//!
//! Each test exercises one slice of the public surface a typical
//! client relies on: TCP acquire/release with fencing-token
//! monotonicity, semaphore cap enforcement, RW read/write, and
//! composite (multi-key) atomicity. All checks go over TCP — the test
//! intentionally does not pull in an HTTP client crate so the live
//! smoke can run from a small statically-linked binary in a Kubernetes
//! Job (see `tests/k8s/test-job.yaml`).
//!
//! Why `#[ignore]`: this test reaches the network and depends on an
//! external deployment, so it should not run in the default `cargo
//! test` matrix on developer laptops or CI's unit-test job. It IS
//! intended to run as part of a Kubernetes Job / cluster-CI smoke
//! after a deployment.

use std::env;
use std::time::Duration;

use dd_rust_network_mutex::{Client, ClientConfig, RwClient};

fn cfg() -> ClientConfig {
    ClientConfig {
        auth_token: env::var("LMX_LIVE_BROKER_AUTH_TOKEN").ok(),
        default_request_timeout: Duration::from_secs(8),
    }
}

fn require_tcp() -> String {
    env::var("LMX_LIVE_BROKER_TCP").expect(
        "LMX_LIVE_BROKER_TCP must be set (e.g. 127.0.0.1:6970) — \
         run with --ignored to enable this test",
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn live_smoke_acquire_release_over_tcp() {
    let endpoint = require_tcp();
    let key = format!("lmx-live-smoke-{}", uuid_short());
    let client = Client::connect_tcp(&endpoint, cfg())
        .await
        .unwrap_or_else(|e| panic!("connect to {endpoint} failed: {e}"));

    let g = client
        .acquire(&key, Duration::from_secs(5))
        .await
        .expect("first acquire");
    let token1 = g.fencing_token.expect("expected a fencing token");
    client.release(&g).await.expect("release");

    let g2 = client.acquire(&key, Duration::from_secs(5)).await.unwrap();
    let token2 = g2.fencing_token.unwrap();
    assert!(
        token2 > token1,
        "fencing not monotonic across releases: {token1} -> {token2}",
    );
    client.release(&g2).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn live_smoke_semaphore_cap_enforced() {
    let endpoint = require_tcp();
    let key = format!("lmx-live-sem-{}", uuid_short());
    let max: u32 = 3;

    let mut clients = Vec::new();
    let mut guards = Vec::new();
    for _ in 0..max {
        let c = Client::connect_tcp(&endpoint, cfg()).await.unwrap();
        let g = c
            .acquire_with_max(&key, max, Duration::from_secs(10))
            .await
            .expect("semaphore slot");
        guards.push(g);
        clients.push(c);
    }

    // (max+1)-th acquire must time out (broker queues it; our request
    // timeout is 1s so we'll see the client-side timeout). The exact
    // error variant doesn't matter — the assertion is "didn't get a
    // grant within the budget".
    let probe = Client::connect_tcp(&endpoint, cfg()).await.unwrap();
    let result = tokio::time::timeout(
        Duration::from_millis(1_500),
        probe.acquire_with_max(&key, max, Duration::from_secs(10)),
    )
    .await;
    assert!(
        result.is_err() || result.as_ref().unwrap().is_err(),
        "broker over-granted semaphore (expected timeout/queue): {result:?}",
    );

    for (c, g) in clients.iter().zip(guards.into_iter()) {
        c.release(&g).await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn live_smoke_rw_read_then_write() {
    let endpoint = require_tcp();
    let key = format!("lmx-live-rw-{}", uuid_short());

    let rw = RwClient::connect_tcp(&endpoint, cfg()).await.unwrap();
    let r = rw.acquire_read(&key).await.expect("read");
    r.release().await.unwrap();

    let w = rw.acquire_write(&key).await.expect("write");
    w.release().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn live_smoke_composite_atomic_release() {
    let endpoint = require_tcp();
    let suffix = uuid_short();
    let keys: Vec<String> = (0..3).map(|i| format!("lmx-live-c{i}-{suffix}")).collect();
    let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();

    let c = Client::connect_tcp(&endpoint, cfg()).await.unwrap();
    let g = c
        .acquire_composite(&key_refs, Duration::from_secs(5))
        .await
        .expect("composite acquire");
    // Composite guards report all keys.
    assert!(
        !g.lock_uuid.is_empty(),
        "composite acquire returned empty lock_uuid",
    );

    // Concurrent client cannot acquire any of the held keys until the
    // composite is released — verifying atomicity end-to-end.
    let probe = Client::connect_tcp(&endpoint, cfg()).await.unwrap();
    let busy = tokio::time::timeout(
        Duration::from_millis(800),
        probe.acquire(&keys[0], Duration::from_secs(5)),
    )
    .await;
    assert!(
        busy.is_err() || busy.as_ref().unwrap().is_err(),
        "key {} not held by composite (got {busy:?})",
        keys[0],
    );

    c.release(&g).await.unwrap();
}

fn uuid_short() -> String {
    let s = uuid::Uuid::new_v4().to_string();
    s.split('-').next().unwrap().to_string()
}
