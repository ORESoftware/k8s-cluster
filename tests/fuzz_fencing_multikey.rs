//! Model-based fuzz + targeted stress for fencing tokens and multi-key
//! (composite) locks.
//!
//! The broker's `handle_request` is synchronous, so a *no-wait* workload is
//! fully deterministic: each request resolves to a terminal acquired:true /
//! acquired:false on the spot. That lets us drive thousands of randomized ops
//! against a shadow model and assert, after every single op:
//!
//! * mutual exclusion   — a key is held by at most one lock at a time;
//! * composite atomicity — a composite grant locks ALL its keys or none;
//! * fencing monotonic   — each grant's per-key token is strictly greater
//!   than any token previously issued for that key;
//! * no-wait correctness — acquired matches the model's free/held view, and
//!   a contended no-wait acquire mutates nothing.
//!
//! A second, scenario-style test exercises the *wait* (queue → grant) path and
//! checks fencing monotonicity through the cascade of FIFO grants.

use std::collections::HashMap;

use dd_rust_network_mutex::{
    broker::{Broker, BrokerConfig},
    protocol::{Request, Response},
};
use tokio::sync::mpsc::UnboundedReceiver;

// -- tiny deterministic RNG (xorshift64*) — no external crate ---------------
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15 | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % (n as u64)) as usize
    }
}

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
        ttl: Some(120_000),
        max: None,
        force: false,
        retry_count: 0,
        keep_locks_after_death: false,
        wait: Some(wait),
    }
}

fn composite_req(uuid: &str, keys: &[String], wait: bool) -> Request {
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
        wait: Some(wait),
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

/// A lock currently held in the shadow model.
struct HeldLock {
    lock_uuid: String,
    keys: Vec<String>,
}

fn run_fuzz_seed(seed: u64, ops: usize) {
    let broker = Broker::new(BrokerConfig::default());
    let n_clients = 8;
    let clients: Vec<_> = (0..n_clients).map(|_| broker.register_client()).collect();
    // Detach receivers so we can mutably index clients by id.
    let mut rxs: Vec<UnboundedReceiver<Response>> = Vec::new();
    let mut cids = Vec::new();
    for (cid, rx) in clients {
        cids.push(cid);
        rxs.push(rx);
    }

    let keys: Vec<String> = (0..6).map(|i| format!("fz{seed}-{i}")).collect();

    // Shadow model.
    let mut held_by_key: HashMap<String, usize> = HashMap::new(); // key -> client idx
    let mut held: Vec<Vec<HeldLock>> = (0..n_clients).map(|_| Vec::new()).collect();
    let mut last_token: HashMap<String, u64> = HashMap::new();

    let mut rng = Rng::new(seed);
    let mut req_seq = 0u64;

    let check_token = |key: &str, tok: u64, last_token: &mut HashMap<String, u64>| {
        assert!(tok >= 1, "fencing token must be >= 1 for {key}, got {tok}");
        let prev = last_token.get(key).copied().unwrap_or(0);
        assert!(
            tok > prev,
            "fencing token for {key} not strictly increasing: prev={prev} new={tok} (seed={seed})"
        );
        last_token.insert(key.to_string(), tok);
    };

    for _ in 0..ops {
        let ci = rng.below(n_clients);
        let cid = cids[ci];
        let roll = rng.below(100);

        if roll < 35 {
            // ---- single-key no-wait acquire ----
            let key = keys[rng.below(keys.len())].clone();
            req_seq += 1;
            let uuid = format!("s{req_seq}");
            broker.handle_request(cid, lock_req(&uuid, &key, false));
            let msgs = drain(&mut rxs[ci]);
            let (acquired, lock_uuid, token) = msgs
                .iter()
                .find_map(|m| match m {
                    Response::Lock {
                        acquired,
                        lock_uuid,
                        fencing_token,
                        ..
                    } => Some((*acquired, lock_uuid.clone(), *fencing_token)),
                    _ => None,
                })
                .expect("single lock reply");
            let expected_free = !held_by_key.contains_key(&key);
            assert_eq!(
                acquired, expected_free,
                "single acquire({key}) acquired={acquired} but model free={expected_free} (seed={seed})"
            );
            if acquired {
                let tok = token.expect("granted single lock has token");
                check_token(&key, tok, &mut last_token);
                let lu = lock_uuid.expect("granted single lock has lock_uuid");
                held_by_key.insert(key.clone(), ci);
                held[ci].push(HeldLock {
                    lock_uuid: lu,
                    keys: vec![key],
                });
            }
        } else if roll < 70 {
            // ---- composite no-wait acquire (2..=4 distinct keys) ----
            let want = 2 + rng.below(3);
            let mut chosen: Vec<String> = Vec::new();
            let mut pool = keys.clone();
            for _ in 0..want {
                if pool.is_empty() {
                    break;
                }
                let idx = rng.below(pool.len());
                chosen.push(pool.remove(idx));
            }
            chosen.sort();
            req_seq += 1;
            let uuid = format!("c{req_seq}");
            broker.handle_request(cid, composite_req(&uuid, &chosen, false));
            let msgs = drain(&mut rxs[ci]);
            let (acquired, lock_uuid, tokens) = msgs
                .iter()
                .find_map(|m| match m {
                    Response::CompositeLock {
                        acquired,
                        lock_uuid,
                        fencing_tokens,
                        ..
                    } => Some((*acquired, lock_uuid.clone(), fencing_tokens.clone())),
                    _ => None,
                })
                .expect("composite reply");
            let expected_free = chosen.iter().all(|k| !held_by_key.contains_key(k));
            assert_eq!(
                acquired, expected_free,
                "composite acquire({chosen:?}) acquired={acquired} but model all-free={expected_free} (seed={seed})"
            );
            if acquired {
                let toks = tokens.expect("granted composite has tokens");
                // Atomicity: a token for every requested key, nothing extra.
                assert_eq!(
                    toks.len(),
                    chosen.len(),
                    "composite atomicity: token map {toks:?} != keys {chosen:?} (seed={seed})"
                );
                for k in &chosen {
                    let tok = *toks.get(k).expect("composite token for member key");
                    check_token(k, tok, &mut last_token);
                    held_by_key.insert(k.clone(), ci);
                }
                let lu = lock_uuid.expect("granted composite has lock_uuid");
                held[ci].push(HeldLock {
                    lock_uuid: lu,
                    keys: chosen,
                });
            }
        } else {
            // ---- release a randomly chosen held lock ----
            if held[ci].is_empty() {
                continue;
            }
            let idx = rng.below(held[ci].len());
            let lock = held[ci].remove(idx);
            req_seq += 1;
            let uuid = format!("u{req_seq}");
            broker.handle_request(cid, unlock_req(&uuid, &lock.keys, &lock.lock_uuid));
            let msgs = drain(&mut rxs[ci]);
            let unlocked = msgs
                .iter()
                .any(|m| matches!(m, Response::Unlock { unlocked: true, .. }));
            assert!(
                unlocked,
                "release of {:?} (lock_uuid={}) was rejected: {msgs:?} (seed={seed})",
                lock.keys, lock.lock_uuid
            );
            for k in &lock.keys {
                let owner = held_by_key.remove(k);
                assert_eq!(
                    owner,
                    Some(ci),
                    "model: {k} freed by wrong owner (seed={seed})"
                );
            }
        }
    }

    // Teardown: release everything still held, then prove nothing leaked by
    // grabbing every key in one composite, no-wait.
    for ci in 0..n_clients {
        let locks = std::mem::take(&mut held[ci]);
        for lock in locks {
            req_seq += 1;
            broker.handle_request(
                cids[ci],
                unlock_req(&format!("fu{req_seq}"), &lock.keys, &lock.lock_uuid),
            );
            let _ = drain(&mut rxs[ci]);
            for k in &lock.keys {
                held_by_key.remove(k);
            }
        }
    }
    assert!(
        held_by_key.is_empty(),
        "model still holds keys after teardown (seed={seed})"
    );

    // Prove nothing leaked: every key must be individually grabbable, no-wait.
    // (Composite cap is 5 keys, so we probe per key rather than all at once.)
    for k in &keys {
        req_seq += 1;
        broker.handle_request(cids[0], lock_req(&format!("final{req_seq}"), k, false));
        let msgs = drain(&mut rxs[0]);
        let got = msgs.iter().find_map(|m| match m {
            Response::Lock {
                acquired,
                lock_uuid,
                ..
            } => Some((*acquired, lock_uuid.clone())),
            _ => None,
        });
        let (acquired, lu) = got.expect("final probe reply");
        assert!(
            acquired,
            "after teardown key {k} must be free; got {msgs:?} (seed={seed})"
        );
        // Release it again so repeated probes stay clean.
        broker.handle_request(
            cids[0],
            unlock_req(
                &format!("finalu{req_seq}"),
                std::slice::from_ref(k),
                &lu.unwrap(),
            ),
        );
        let _ = drain(&mut rxs[0]);
    }
}

#[test]
fn fuzz_no_wait_fencing_and_composite_invariants() {
    for &seed in &[1u64, 7, 42, 1337, 99_991, 2_718_281] {
        run_fuzz_seed(seed, 4000);
    }
}

#[test]
fn wait_queue_grants_are_fencing_monotonic_fifo() {
    let broker = Broker::new(BrokerConfig::default());
    let holders: Vec<_> = (0..4).map(|_| broker.register_client()).collect();
    let mut cids = Vec::new();
    let mut rxs = Vec::new();
    for (c, rx) in holders {
        cids.push(c);
        rxs.push(rx);
    }
    let key = "fifo-key";

    // First holder acquires.
    broker.handle_request(cids[0], lock_req("h0", key, true));
    let mut last = 0u64;
    let mut held_uuid = {
        let msgs = drain(&mut rxs[0]);
        let (tok, lu) = msgs
            .iter()
            .find_map(|m| match m {
                Response::Lock {
                    acquired: true,
                    fencing_token: Some(t),
                    lock_uuid: Some(u),
                    ..
                } => Some((*t, u.clone())),
                _ => None,
            })
            .expect("h0 grant");
        assert!(tok > last, "first token must be positive");
        last = tok;
        lu
    };

    // The rest queue (wait=true).
    for i in 1..4 {
        broker.handle_request(cids[i], lock_req(&format!("h{i}"), key, true));
        let msgs = drain(&mut rxs[i]);
        let queued = msgs.iter().any(|m| {
            matches!(
                m,
                Response::Lock {
                    acquired: false,
                    ..
                }
            )
        });
        assert!(queued, "h{i} should be queued");
    }

    // Cascade releases; each successive holder must grant with a strictly
    // greater fencing token, in FIFO order.
    for i in 1..4 {
        broker.handle_request(
            cids[i - 1],
            unlock_req(&format!("rel{i}"), &[key.to_string()], &held_uuid),
        );
        let _ = drain(&mut rxs[i - 1]);
        let msgs = drain(&mut rxs[i]);
        let (tok, lu) = msgs
            .iter()
            .find_map(|m| match m {
                Response::Lock {
                    acquired: true,
                    fencing_token: Some(t),
                    lock_uuid: Some(u),
                    ..
                } => Some((*t, u.clone())),
                _ => None,
            })
            .unwrap_or_else(|| panic!("h{i} did not get a grant after release; got {msgs:?}"));
        assert!(
            tok > last,
            "fencing token must strictly increase through the wait queue: prev={last} new={tok}"
        );
        last = tok;
        held_uuid = lu;
    }
}
