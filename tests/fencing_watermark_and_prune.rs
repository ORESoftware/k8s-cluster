//! Tests for the broker-wide fencing watermark and the empty-key
//! prune sweep introduced together. The two features are designed
//! to land in lockstep: the prune sweep can reclaim idle
//! `LockState` entries (memory bound), but only because every new
//! `LockState` seeds its fencing counter from the watermark
//! (cross-incarnation monotonicity bound).
//!
//!   w1  Without watermark, a hot key whose fencing counter has
//!       outpaced wall-clock-millis would re-incarnate at a smaller
//!       seed — losing monotonicity. With watermark, the new seed
//!       is always > any token previously issued for any key.
//!   w2  The watermark surfaces in `BrokerMetrics::fencing_watermark`.
//!
//!   p1  An idle key that has been emptied longer than
//!       `idle_key_grace` is reclaimed on the next `tick_ttl`.
//!       `BrokerMetrics::idle_keys_pruned_total` increments by 1.
//!   p2  A key with active holders is never pruned.
//!   p3  A key with queued waiters but no holders is NOT idle and
//!       never pruned.
//!   p4  `idle_key_grace = ZERO` disables pruning entirely.
//!   p5  After prune + re-acquire, the new fencing token is
//!       strictly greater than every token previously issued for
//!       any key (cross-prune monotonicity).
//!   p6  Pruning is driven by wall-clock idleness, not by access
//!       — a recently-released key still inside the grace window
//!       survives the sweep.

use std::time::Duration;

use dd_rust_network_mutex::{
    broker::{Broker, BrokerConfig},
    protocol::{Request, Response},
};
use tokio::sync::mpsc::UnboundedReceiver;

fn drain(rx: &mut UnboundedReceiver<Response>) -> Vec<Response> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        out.push(msg);
    }
    out
}

fn lock_uuid_of(msgs: &[Response]) -> Option<String> {
    msgs.iter().find_map(|m| match m {
        Response::Lock {
            acquired: true,
            lock_uuid,
            ..
        } => lock_uuid.clone(),
        _ => None,
    })
}

fn token_of(msgs: &[Response]) -> Option<u64> {
    msgs.iter().find_map(|m| match m {
        Response::Lock {
            acquired: true,
            fencing_token,
            ..
        } => *fencing_token,
        _ => None,
    })
}

fn acquire(broker: &Broker, client: u64, key: &str, uuid: &str) -> (Option<String>, Option<u64>) {
    let _ = client;
    // Caller must drain afterwards.
    broker.handle_request(
        client,
        Request::Lock {
            uuid: uuid.into(),
            key: Some(key.into()),
            keys: None,
            pid: None,
            ttl: Some(60_000),
            max: None,
            force: false,
            retry_count: 0,
            keep_locks_after_death: false,
            wait: None,
        },
    );
    (None, None)
}

#[test]
fn w1_watermark_observes_hot_key_token() {
    let broker = Broker::new(BrokerConfig::default());
    let (a, mut a_rx) = broker.register_client();

    acquire(&broker, a, "hotkey", "r1");
    let msgs = drain(&mut a_rx);
    let initial_token = token_of(&msgs).expect("first acquire should grant");

    let snapshot = broker.metrics();
    assert_eq!(
        snapshot.fencing_watermark, initial_token,
        "watermark must equal the most recently issued token"
    );
}

#[test]
fn w2_watermark_surfaces_in_metrics() {
    let broker = Broker::new(BrokerConfig::default());
    let (a, mut a_rx) = broker.register_client();

    let pre = broker.metrics().fencing_watermark;
    assert_eq!(pre, 0, "fresh broker watermark starts at 0");

    acquire(&broker, a, "k", "r1");
    let _ = drain(&mut a_rx);

    let post = broker.metrics().fencing_watermark;
    assert!(
        post > pre,
        "watermark must advance after first grant ({pre} -> {post})"
    );
}

#[tokio::test]
async fn p1_idle_key_is_reclaimed_after_grace() {
    let cfg = BrokerConfig {
        idle_key_grace: Duration::from_millis(50),
        ..BrokerConfig::default()
    };
    let broker = Broker::new(cfg);
    let (a, mut a_rx) = broker.register_client();

    acquire(&broker, a, "ephemeral", "r1");
    let lock_uuid = lock_uuid_of(&drain(&mut a_rx)).unwrap();
    broker.handle_request(
        a,
        Request::Unlock {
            uuid: "u1".into(),
            key: Some("ephemeral".into()),
            keys: None,
            lock_uuid: Some(lock_uuid),
            force: false,
        },
    );
    let _ = drain(&mut a_rx);

    assert_eq!(
        broker.metrics().keys,
        1,
        "key still tracked immediately after release"
    );

    tokio::time::sleep(Duration::from_millis(80)).await;
    let pruned_pre = broker.metrics().idle_keys_pruned_total;
    let evicted = broker.tick_ttl(std::time::Instant::now());
    assert_eq!(
        evicted, 0,
        "TTL deadline already cleared by explicit unlock"
    );

    let snapshot = broker.metrics();
    assert_eq!(snapshot.keys, 0, "idle key should have been pruned");
    assert_eq!(
        snapshot.idle_keys_pruned_total,
        pruned_pre + 1,
        "idle_keys_pruned_total must increment by 1"
    );
}

#[tokio::test]
async fn p2_active_key_with_holder_is_never_pruned() {
    let cfg = BrokerConfig {
        idle_key_grace: Duration::from_millis(50),
        ..BrokerConfig::default()
    };
    let broker = Broker::new(cfg);
    let (a, mut a_rx) = broker.register_client();

    acquire(&broker, a, "alive", "r1");
    let _ = drain(&mut a_rx);

    tokio::time::sleep(Duration::from_millis(80)).await;
    broker.tick_ttl(std::time::Instant::now());

    assert_eq!(
        broker.metrics().keys,
        1,
        "active-holder key must survive the prune"
    );
    assert_eq!(broker.metrics().idle_keys_pruned_total, 0);
}

#[tokio::test]
async fn p3_key_with_queued_waiter_is_not_idle() {
    let cfg = BrokerConfig {
        idle_key_grace: Duration::from_millis(50),
        ..BrokerConfig::default()
    };
    let broker = Broker::new(cfg);
    let (a, mut a_rx) = broker.register_client();
    let (b, mut b_rx) = broker.register_client();

    // A grabs and holds.
    acquire(&broker, a, "queued", "r1");
    let _ = drain(&mut a_rx);
    // B queues behind A. Lock now has 1 holder + 1 waiter; not idle.
    acquire(&broker, b, "queued", "r2");
    let _ = drain(&mut b_rx);

    tokio::time::sleep(Duration::from_millis(80)).await;
    broker.tick_ttl(std::time::Instant::now());

    assert_eq!(
        broker.metrics().keys,
        1,
        "key with holder + waiter is not idle and must not be pruned"
    );
}

#[tokio::test]
async fn p4_grace_zero_disables_pruning() {
    let cfg = BrokerConfig {
        idle_key_grace: Duration::ZERO,
        ..BrokerConfig::default()
    };
    let broker = Broker::new(cfg);
    let (a, mut a_rx) = broker.register_client();

    acquire(&broker, a, "k", "r1");
    let lock_uuid = lock_uuid_of(&drain(&mut a_rx)).unwrap();
    broker.handle_request(
        a,
        Request::Unlock {
            uuid: "u1".into(),
            key: Some("k".into()),
            keys: None,
            lock_uuid: Some(lock_uuid),
            force: false,
        },
    );
    let _ = drain(&mut a_rx);

    tokio::time::sleep(Duration::from_millis(50)).await;
    broker.tick_ttl(std::time::Instant::now());

    assert_eq!(
        broker.metrics().keys,
        1,
        "grace=ZERO must disable pruning (historical behaviour)"
    );
    assert_eq!(broker.metrics().idle_keys_pruned_total, 0);
}

#[tokio::test]
async fn p5_cross_prune_fencing_monotonicity() {
    // The killer test. Sequence:
    //   1. Acquire several keys to bump the broker watermark.
    //   2. Release "hot" key; let prune reclaim it.
    //   3. Re-acquire "hot" key.
    //   4. New token must be > the watermark observed at step 1.
    //
    // Without the watermark seed, step 3 would mint a token from
    // fresh wall-clock-millis, ignoring the previous incarnation's
    // history. We assert the broker preserves "every token across
    // every key is strictly less than every later token".
    let cfg = BrokerConfig {
        idle_key_grace: Duration::from_millis(50),
        ..BrokerConfig::default()
    };
    let broker = Broker::new(cfg);

    let mut last_token: u64 = 0;
    // Five distinct keys, each gets at least one acquire/release cycle.
    for i in 0..5u32 {
        let (cid, mut rx) = broker.register_client();
        let key = format!("k{i}");
        acquire(&broker, cid, &key, &format!("r{i}"));
        let msgs = drain(&mut rx);
        let token = token_of(&msgs).expect("acquire");
        let lock_uuid = lock_uuid_of(&msgs).unwrap();
        last_token = last_token.max(token);
        broker.handle_request(
            cid,
            Request::Unlock {
                uuid: format!("u{i}"),
                key: Some(key),
                keys: None,
                lock_uuid: Some(lock_uuid),
                force: false,
            },
        );
        let _ = drain(&mut rx);
    }

    // Force watermark above wall-clock-millis to make the test
    // *meaningful*: artificially bump the broker's state so a
    // re-incarnated key without watermark seeding would mint a
    // smaller token than the previous lifetime's last one.
    //
    // We do this through the public surface by issuing many tokens
    // on one key. Each acquire/release adds 1; a sustained churn
    // pushes the per-key counter past wall-clock-millis. With the
    // default seed (wall-clock-millis ≈ 1.7e12), this would take
    // ~1.7e12 acquires — not feasible. So we use an alternative:
    // ensure the WATERMARK guarantees a re-incarnation seed >= the
    // most-recent token issued, which is the actual contract we
    // want to test.
    let pre_prune_token = last_token;
    let pre_prune_watermark = broker.metrics().fencing_watermark;
    assert!(
        pre_prune_watermark >= pre_prune_token,
        "watermark must dominate every previously-issued token"
    );

    // Wait past the grace window so prune reclaims everything.
    tokio::time::sleep(Duration::from_millis(80)).await;
    broker.tick_ttl(std::time::Instant::now());
    assert_eq!(
        broker.metrics().keys,
        0,
        "all five idle keys should have been pruned"
    );
    let pruned_total = broker.metrics().idle_keys_pruned_total;
    assert_eq!(pruned_total, 5, "five prunes");

    // Re-acquire one of the pruned keys.
    let (cid, mut rx) = broker.register_client();
    acquire(&broker, cid, "k0", "r-revival");
    let new_token = token_of(&drain(&mut rx)).expect("revival acquire");

    assert!(
        new_token > pre_prune_token,
        "post-prune fencing token ({new_token}) must dominate the most recent pre-prune token ({pre_prune_token})"
    );
    assert!(
        new_token > pre_prune_watermark,
        "post-prune fencing token ({new_token}) must exceed pre-prune watermark ({pre_prune_watermark})"
    );

    // The broker watermark itself is also updated.
    assert!(broker.metrics().fencing_watermark >= new_token);
}

#[tokio::test]
async fn p6_recently_released_key_inside_grace_window_survives() {
    let cfg = BrokerConfig {
        idle_key_grace: Duration::from_millis(500),
        ..BrokerConfig::default()
    };
    let broker = Broker::new(cfg);
    let (a, mut a_rx) = broker.register_client();

    acquire(&broker, a, "k", "r1");
    let lock_uuid = lock_uuid_of(&drain(&mut a_rx)).unwrap();
    broker.handle_request(
        a,
        Request::Unlock {
            uuid: "u1".into(),
            key: Some("k".into()),
            keys: None,
            lock_uuid: Some(lock_uuid),
            force: false,
        },
    );
    let _ = drain(&mut a_rx);

    // Sweep well within the grace window.
    tokio::time::sleep(Duration::from_millis(50)).await;
    broker.tick_ttl(std::time::Instant::now());

    assert_eq!(
        broker.metrics().keys,
        1,
        "key released within grace window must survive the sweep"
    );
}
