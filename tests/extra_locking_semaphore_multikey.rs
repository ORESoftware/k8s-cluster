//! Extra broker-level coverage for the three lock primitives, driven through the
//! same in-process `Broker` API the deployed BrokerRaft service runs:
//!
//!   e1  exclusive single-key handoff issues strictly increasing fencing tokens.
//!   e2  a `max`-capped key behaves as a counting semaphore: `max` concurrent
//!       holders are admitted with distinct lock_uuids, the next no-wait request
//!       fails fast, and a waiting request is granted once a permit frees.
//!   e3  composite (multi-key) locks are atomic and mutually exclusive on any
//!       shared key, expose a per-key fencing token, yet run disjoint sets in
//!       parallel.

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

fn lock_req(uuid: &str, key: &str, max: Option<u32>, wait: bool) -> Request {
    Request::Lock {
        uuid: uuid.into(),
        key: Some(key.into()),
        keys: None,
        pid: None,
        ttl: Some(60_000),
        max,
        force: false,
        retry_count: 0,
        keep_locks_after_death: false,
        wait: Some(wait),
    }
}

fn composite_req(uuid: &str, keys: &[&str], wait: bool) -> Request {
    Request::Lock {
        uuid: uuid.into(),
        key: None,
        keys: Some(keys.iter().map(|s| s.to_string()).collect()),
        pid: None,
        ttl: Some(60_000),
        max: None,
        force: false,
        retry_count: 0,
        keep_locks_after_death: false,
        wait: Some(wait),
    }
}

fn unlock_key(uuid: &str, key: &str, lock_uuid: &str) -> Request {
    Request::Unlock {
        uuid: uuid.into(),
        key: Some(key.into()),
        keys: None,
        lock_uuid: Some(lock_uuid.into()),
        force: false,
    }
}

fn single_grant(msgs: &[Response]) -> Option<(bool, Option<String>, Option<u64>)> {
    msgs.iter().find_map(|m| match m {
        Response::Lock {
            acquired,
            lock_uuid,
            fencing_token,
            ..
        } => Some((*acquired, lock_uuid.clone(), *fencing_token)),
        _ => None,
    })
}

fn composite_grant(
    msgs: &[Response],
) -> Option<(
    bool,
    Option<String>,
    Option<std::collections::BTreeMap<String, u64>>,
)> {
    msgs.iter().find_map(|m| match m {
        Response::CompositeLock {
            acquired,
            lock_uuid,
            fencing_tokens,
            ..
        } => Some((*acquired, lock_uuid.clone(), fencing_tokens.clone())),
        _ => None,
    })
}

#[test]
fn e1_exclusive_handoff_issues_monotonic_fencing_tokens() {
    let broker = Broker::new(BrokerConfig::default());
    let (a, mut a_rx) = broker.register_client();
    let (b, mut b_rx) = broker.register_client();

    broker.handle_request(a, lock_req("a1", "k", None, true));
    let (acq_a, uuid_a, fence_a) = single_grant(&drain(&mut a_rx)).expect("A reply");
    assert!(acq_a, "A should hold k");
    let fence_a = fence_a.expect("A fencing token");

    // B no-wait while A holds -> fails fast (mutual exclusion).
    broker.handle_request(b, lock_req("b1", "k", None, false));
    let (acq_b, _, _) = single_grant(&drain(&mut b_rx)).expect("B reply");
    assert!(!acq_b, "B must fail fast while A holds k");

    // A releases, B re-acquires with a STRICTLY HIGHER fencing token.
    broker.handle_request(
        a,
        unlock_key("a-unlock", "k", &uuid_a.expect("A lock_uuid")),
    );
    let _ = drain(&mut a_rx);
    broker.handle_request(b, lock_req("b2", "k", None, false));
    let (acq_b2, _, fence_b2) = single_grant(&drain(&mut b_rx)).expect("B retry reply");
    assert!(acq_b2, "B should acquire after A releases");
    let fence_b2 = fence_b2.expect("B fencing token");
    assert!(
        fence_b2 > fence_a,
        "fencing token must strictly increase across handoff: {fence_a} -> {fence_b2}"
    );
}

#[test]
fn e2_max_capped_key_is_a_counting_semaphore() {
    let broker = Broker::new(BrokerConfig::default());
    let (a, mut a_rx) = broker.register_client();
    let (b, mut b_rx) = broker.register_client();
    let (c, mut c_rx) = broker.register_client();
    let (d, mut d_rx) = broker.register_client();

    // Capacity 2: two concurrent holders of the same key are both admitted.
    broker.handle_request(a, lock_req("a1", "sem", Some(2), true));
    let (acq_a, uuid_a, _) = single_grant(&drain(&mut a_rx)).expect("A reply");
    assert!(acq_a, "first holder admitted");
    let uuid_a = uuid_a.expect("A lock_uuid");

    broker.handle_request(b, lock_req("b1", "sem", Some(2), true));
    let (acq_b, uuid_b, _) = single_grant(&drain(&mut b_rx)).expect("B reply");
    assert!(acq_b, "second holder admitted under cap 2");
    let uuid_b = uuid_b.expect("B lock_uuid");
    assert_ne!(uuid_a, uuid_b, "concurrent holders get distinct lock_uuids");

    // Third holder, no-wait, must fail fast: the two permits are taken.
    broker.handle_request(c, lock_req("c1", "sem", Some(2), false));
    let (acq_c, _, _) = single_grant(&drain(&mut c_rx)).expect("C reply");
    assert!(!acq_c, "third no-wait acquire must fail fast at cap 2");

    // A waiting fourth request is enqueued, then granted once a permit frees.
    broker.handle_request(d, lock_req("d1", "sem", Some(2), true));
    let d_queued = drain(&mut d_rx);
    assert_eq!(
        single_grant(&d_queued).map(|(acq, _, _)| acq),
        Some(false),
        "queued waiter's first frame is the acquired:false notice"
    );

    broker.handle_request(a, unlock_key("a-unlock", "sem", &uuid_a));
    let _ = drain(&mut a_rx);
    let granted = drain(&mut d_rx)
        .iter()
        .any(|m| matches!(m, Response::Lock { acquired: true, .. }));
    assert!(granted, "waiting holder is granted once a permit frees");

    // Cleanup.
    broker.handle_request(b, unlock_key("b-unlock", "sem", &uuid_b));
}

#[test]
fn e3_composite_is_atomic_exclusive_and_parallel_when_disjoint() {
    let broker = Broker::new(BrokerConfig::default());
    let (a, mut a_rx) = broker.register_client();
    let (b, mut b_rx) = broker.register_client();
    let (c, mut c_rx) = broker.register_client();

    // A holds composite [x, y] atomically, with a per-key fencing token each.
    broker.handle_request(a, composite_req("a1", &["x", "y"], true));
    let (acq_a, uuid_a, fences_a) = composite_grant(&drain(&mut a_rx)).expect("A composite reply");
    assert!(acq_a, "A should hold [x,y]");
    let fences_a = fences_a.expect("A fencing tokens");
    assert!(
        fences_a.contains_key("x") && fences_a.contains_key("y"),
        "composite grant exposes a per-key fencing token: {fences_a:?}"
    );

    // B no-wait composite [y, z] overlaps on y (held) -> fails fast.
    broker.handle_request(b, composite_req("b1", &["y", "z"], false));
    let (acq_b, _, _) = composite_grant(&drain(&mut b_rx)).expect("B composite reply");
    assert!(!acq_b, "overlapping composite must fail while y is held");

    // C composite [p, q] is disjoint from [x, y] -> granted in parallel.
    broker.handle_request(c, composite_req("c1", &["p", "q"], false));
    let (acq_c, _, _) = composite_grant(&drain(&mut c_rx)).expect("C composite reply");
    assert!(acq_c, "disjoint composite runs in parallel with A's [x,y]");

    // Releasing A frees x and y; a fresh overlapping no-wait composite now wins.
    broker.handle_request(
        a,
        Request::Unlock {
            uuid: "a-unlock".into(),
            key: None,
            keys: Some(vec!["x".into(), "y".into()]),
            lock_uuid: Some(uuid_a.expect("A composite lock_uuid")),
            force: false,
        },
    );
    let _ = drain(&mut a_rx);
    broker.handle_request(b, composite_req("b2", &["y", "z"], false));
    let (acq_b2, _, _) = composite_grant(&drain(&mut b_rx)).expect("B retry composite reply");
    assert!(acq_b2, "composite [y,z] acquirable once A releases y");
}
