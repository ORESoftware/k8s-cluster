//! Caller-controlled wait / no-wait semantics for `Lock` (single-key) and
//! composite (`keys`) acquisitions.
//!
//! Background: the broker historically *always* enqueued a contended request
//! and replied `acquired:false` as a "you're queued" notice, then later sent
//! `acquired:true` once the key(s) freed. Cross-runtime clients that resolved
//! on the first reply abandoned the queued request — which the broker still
//! granted — leaking the locked key(s) forever. The `wait` flag lets a caller
//! opt into fail-fast (`wait:false`) so the request is *never* enqueued and
//! therefore can never leak a deferred grant.
//!
//!   w1  no-wait single-key on a held key returns `acquired:false` immediately
//!       and does NOT enqueue (proven by: holder releases, then a fresh
//!       no-wait acquire succeeds — there was no leftover waiter ahead of it).
//!   w2  no-wait composite on an overlapping held key returns `acquired:false`
//!       immediately, leaves no waiter, and does not partially lock the free
//!       members (proven by a later no-wait acquire of a disjoint set + a
//!       no-wait acquire of the previously-contended set both succeeding).
//!   w3  wait composite on a contended key DOES enqueue and is granted with
//!       `acquired:true` after the holder releases.

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

fn lock_req(uuid: &str, key: &str, wait: bool) -> Request {
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

fn single_grant(msgs: &[Response]) -> Option<(bool, Option<String>)> {
    msgs.iter().find_map(|m| match m {
        Response::Lock {
            acquired,
            lock_uuid,
            ..
        } => Some((*acquired, lock_uuid.clone())),
        _ => None,
    })
}

fn composite_grant(msgs: &[Response]) -> Option<(bool, Option<String>)> {
    msgs.iter().find_map(|m| match m {
        Response::CompositeLock {
            acquired,
            lock_uuid,
            ..
        } => Some((*acquired, lock_uuid.clone())),
        _ => None,
    })
}

#[test]
fn w1_no_wait_single_key_fails_fast_without_enqueue() {
    let broker = Broker::new(BrokerConfig::default());
    let (a, mut a_rx) = broker.register_client();
    let (b, mut b_rx) = broker.register_client();

    // A holds `k`.
    broker.handle_request(a, lock_req("a1", "k", true));
    let (acquired_a, lock_uuid_a) = single_grant(&drain(&mut a_rx)).expect("A reply");
    assert!(acquired_a, "A should hold k");
    let lock_uuid_a = lock_uuid_a.expect("A lock_uuid");

    // B no-wait acquires `k` → immediate acquired:false, NOT enqueued.
    broker.handle_request(b, lock_req("b1", "k", false));
    let (acquired_b, _) = single_grant(&drain(&mut b_rx)).expect("B reply");
    assert!(!acquired_b, "B no-wait must fail fast on a held key");

    // A releases. If B had been enqueued, the broker would now grant B and
    // emit an unsolicited acquired:true. It must NOT (no-wait never queued).
    broker.handle_request(
        a,
        Request::Unlock {
            uuid: "a-unlock".into(),
            key: Some("k".into()),
            keys: None,
            lock_uuid: Some(lock_uuid_a),
            force: false,
        },
    );
    let b_after_release = drain(&mut b_rx);
    assert!(
        single_grant(&b_after_release).is_none(),
        "no-wait B must not receive a deferred grant; got {b_after_release:?}"
    );

    // A fresh no-wait acquire now succeeds because nothing is queued ahead.
    broker.handle_request(b, lock_req("b2", "k", false));
    let (acquired_b2, _) = single_grant(&drain(&mut b_rx)).expect("B retry reply");
    assert!(
        acquired_b2,
        "B no-wait should succeed once k is free and unqueued"
    );
}

#[test]
fn w2_no_wait_composite_fails_fast_and_leaves_no_partial_state() {
    let broker = Broker::new(BrokerConfig::default());
    let (a, mut a_rx) = broker.register_client();
    let (b, mut b_rx) = broker.register_client();

    // A holds composite [x, y].
    broker.handle_request(a, composite_req("a1", &["x", "y"], true));
    let (acquired_a, lock_uuid_a) = composite_grant(&drain(&mut a_rx)).expect("A composite reply");
    assert!(acquired_a, "A should hold [x,y]");
    let lock_uuid_a = lock_uuid_a.expect("A composite lock_uuid");

    // B no-wait composite [y, z] overlaps on `y` (held) → immediate false.
    broker.handle_request(b, composite_req("b1", &["y", "z"], false));
    let (acquired_b, _) = composite_grant(&drain(&mut b_rx)).expect("B composite reply");
    assert!(
        !acquired_b,
        "B no-wait composite must fail fast when y is held"
    );

    // The failed no-wait attempt must NOT have left `z` partially locked.
    // A different client can grab `z` alone, no-wait, right now.
    let (c, mut c_rx) = broker.register_client();
    broker.handle_request(c, lock_req("c1", "z", false));
    let (acquired_c, _) = single_grant(&drain(&mut c_rx)).expect("C reply");
    assert!(
        acquired_c,
        "z must be free after B's rolled-back no-wait composite"
    );

    // Release A's composite. B was never enqueued, so no deferred grant.
    broker.handle_request(
        a,
        Request::Unlock {
            uuid: "a-unlock".into(),
            key: None,
            keys: Some(vec!["x".into(), "y".into()]),
            lock_uuid: Some(lock_uuid_a),
            force: false,
        },
    );
    let b_after = drain(&mut b_rx);
    assert!(
        composite_grant(&b_after).is_none(),
        "no-wait composite B must not receive a deferred grant; got {b_after:?}"
    );
}

#[test]
fn w3_wait_composite_is_queued_and_granted_after_release() {
    let broker = Broker::new(BrokerConfig::default());
    let (a, mut a_rx) = broker.register_client();
    let (b, mut b_rx) = broker.register_client();

    // A holds composite [m, n].
    broker.handle_request(a, composite_req("a1", &["m", "n"], true));
    let (acquired_a, lock_uuid_a) = composite_grant(&drain(&mut a_rx)).expect("A composite reply");
    assert!(acquired_a, "A should hold [m,n]");
    let lock_uuid_a = lock_uuid_a.expect("A composite lock_uuid");

    // B waits on composite [n, o]; `n` is held → enqueued (acquired:false
    // notice), no terminal grant yet.
    broker.handle_request(b, composite_req("b1", &["n", "o"], true));
    let b_queued = drain(&mut b_rx);
    let (acquired_b_first, _) = composite_grant(&b_queued).expect("B queued notice");
    assert!(!acquired_b_first, "first B frame is the queued notice");

    // A releases [m, n]; the broker should now grant B's queued composite.
    broker.handle_request(
        a,
        Request::Unlock {
            uuid: "a-unlock".into(),
            key: None,
            keys: Some(vec!["m".into(), "n".into()]),
            lock_uuid: Some(lock_uuid_a),
            force: false,
        },
    );
    let b_after = drain(&mut b_rx);
    let granted = b_after
        .iter()
        .any(|m| matches!(m, Response::CompositeLock { acquired: true, .. }));
    assert!(
        granted,
        "waiting B must receive acquired:true after A releases; got {b_after:?}"
    );
}
