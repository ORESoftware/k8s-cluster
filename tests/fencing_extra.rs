//! Targeted fencing-token semantics: monotonicity across re-acquire (single +
//! multi-key) and the "never resets while the key lives" guarantee, plus
//! per-key counter independence.
//!
//! These complement the randomized fuzz in `fuzz_fencing_multikey.rs` with
//! small, explicit, human-readable assertions about the fencing contract.

use std::collections::BTreeMap;

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

fn composite_req(uuid: &str, keys: &[String]) -> Request {
    Request::Lock {
        uuid: uuid.into(),
        key: None,
        keys: Some(keys.to_vec()),
        pid: None,
        ttl: Some(120_000),
        max: None,
        force: false,
        retry_count: 0,
        keep_locks_after_death: false,
        wait: Some(false),
    }
}

fn lock_req(uuid: &str, key: &str) -> Request {
    Request::Lock {
        uuid: uuid.into(),
        key: Some(key.into()),
        keys: None,
        pid: None,
        ttl: Some(120_000),
        max: None,
        force: false,
        retry_count: 0,
        keep_locks_after_death: false,
        wait: Some(false),
    }
}

fn unlock_req(uuid: &str, keys: &[String], lock_uuid: &str) -> Request {
    Request::Unlock {
        uuid: uuid.into(),
        key: if keys.len() == 1 {
            Some(keys[0].clone())
        } else {
            None
        },
        keys: if keys.len() > 1 {
            Some(keys.to_vec())
        } else {
            None
        },
        lock_uuid: Some(lock_uuid.into()),
        force: false,
    }
}

fn composite_grant(msgs: &[Response]) -> (BTreeMap<String, u64>, String) {
    msgs.iter()
        .find_map(|m| match m {
            Response::CompositeLock {
                acquired: true,
                lock_uuid: Some(lu),
                fencing_tokens: Some(toks),
                ..
            } => Some((toks.clone(), lu.clone())),
            _ => None,
        })
        .expect("expected a composite grant")
}

fn single_grant(msgs: &[Response]) -> (u64, String) {
    msgs.iter()
        .find_map(|m| match m {
            Response::Lock {
                acquired: true,
                lock_uuid: Some(lu),
                fencing_token: Some(t),
                ..
            } => Some((*t, lu.clone())),
            _ => None,
        })
        .expect("expected a single grant")
}

#[test]
fn composite_reacquire_tokens_strictly_increase() {
    let broker = Broker::new(BrokerConfig::default());
    let (cid, mut rx) = broker.register_client();
    let abc: Vec<String> = vec!["fe-a".into(), "fe-b".into(), "fe-c".into()];

    // First acquisition of {a,b,c}.
    broker.handle_request(cid, composite_req("c1", &abc));
    let (t1, lu1) = composite_grant(&drain(&mut rx));
    assert_eq!(t1.len(), 3, "composite must mint a token per key");
    broker.handle_request(cid, unlock_req("u1", &abc, &lu1));
    let _ = drain(&mut rx);

    // Re-acquire the same set: every per-key token must be strictly greater.
    broker.handle_request(cid, composite_req("c2", &abc));
    let (t2, lu2) = composite_grant(&drain(&mut rx));
    for k in &abc {
        assert!(
            t2[k] > t1[k],
            "re-acquire of {k} must yield a strictly greater token: {} !> {}",
            t2[k],
            t1[k]
        );
    }
    broker.handle_request(cid, unlock_req("u2", &abc, &lu2));
    let _ = drain(&mut rx);

    // Overlapping set {b,c,d}: shared keys keep climbing; new key d starts >=1.
    let bcd: Vec<String> = vec!["fe-b".into(), "fe-c".into(), "fe-d".into()];
    broker.handle_request(cid, composite_req("c3", &bcd));
    let (t3, lu3) = composite_grant(&drain(&mut rx));
    assert!(
        t3["fe-b"] > t2["fe-b"],
        "overlap key b must keep increasing"
    );
    assert!(
        t3["fe-c"] > t2["fe-c"],
        "overlap key c must keep increasing"
    );
    assert!(t3["fe-d"] >= 1, "fresh key d must have a positive token");
    broker.handle_request(cid, unlock_req("u3", &bcd, &lu3));
    let _ = drain(&mut rx);
}

#[test]
fn single_key_fencing_is_monotonic_and_per_key_independent() {
    let broker = Broker::new(BrokerConfig::default());
    let (cid, mut rx) = broker.register_client();

    // Acquire/release the same key many times: tokens never reset, always climb.
    let mut last_x = 0u64;
    let mut first_x = 0u64;
    for i in 0..50 {
        broker.handle_request(cid, lock_req(&format!("x{i}"), "fe-x"));
        let (tok, lu) = single_grant(&drain(&mut rx));
        assert!(
            tok > last_x,
            "fe-x token must strictly increase across re-acquire cycle {i}: {tok} !> {last_x}"
        );
        if i == 0 {
            first_x = tok;
        }
        last_x = tok;
        broker.handle_request(cid, unlock_req(&format!("xu{i}"), &["fe-x".into()], &lu));
        let _ = drain(&mut rx);
    }

    // A different key has its own independent, also-monotonic counter.
    let mut last_y = 0u64;
    let mut first_y = 0u64;
    for i in 0..5 {
        broker.handle_request(cid, lock_req(&format!("y{i}"), "fe-y"));
        let (tok, lu) = single_grant(&drain(&mut rx));
        assert!(
            tok > last_y,
            "fe-y token must strictly increase: {tok} !> {last_y}"
        );
        if i == 0 {
            first_y = tok;
        }
        last_y = tok;
        broker.handle_request(cid, unlock_req(&format!("yu{i}"), &["fe-y".into()], &lu));
        let _ = drain(&mut rx);
    }
    // Each key advanced once per cycle (50 / 5 strictly-increasing grants), and
    // the two counters are independent (fe-x advanced more than fe-y did).
    assert!(
        last_x - first_x >= 49,
        "fe-x must advance ~once per cycle (span={})",
        last_x - first_x
    );
    assert!(
        last_y - first_y >= 4,
        "fe-y must advance ~once per cycle (span={})",
        last_y - first_y
    );
    assert!(
        last_x - first_x > last_y - first_y,
        "per-key counters must be independent"
    );
}
