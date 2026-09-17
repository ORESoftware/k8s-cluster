//! Deterministic liveness fuzz for blocking (wait=true) composite + single-key
//! acquisition. The broker serializes every request under one lock, so any
//! concurrent execution is equivalent to *some* sequential ordering of atomic
//! `handle_request` calls. This fuzz drives randomized sequential orderings and
//! checks a decidable liveness property:
//!
//!   DRAIN-TO-EMPTY: from any state, if we release every fully-held lock (and
//!   cascade the grants that unblocks), the system must fully drain — i.e. no
//!   waiter is left queued once all keys are free. A waiter that never gets its
//!   grant even though every key is free is a *missed wakeup* / liveness bug.
//!
//! A seeded failure here is a concrete, replayable reproduction.

use dd_rust_network_mutex::{
    broker::{Broker, BrokerConfig},
    protocol::{Request, Response},
};
use tokio::sync::mpsc::UnboundedReceiver;

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

#[derive(Clone)]
#[allow(dead_code)] // some fields are retained for clarity / future assertions
enum Agent {
    Idle,
    Waiting {
        req_uuid: String,
        keys: Vec<String>,
    },
    Holding {
        req_uuid: String,
        lock_uuid: String,
        keys: Vec<String>,
    },
}

fn lock_req(uuid: &str, key: &str) -> Request {
    Request::Lock {
        uuid: uuid.into(),
        key: Some(key.into()),
        keys: None,
        pid: None,
        ttl: Some(600_000),
        max: None,
        force: false,
        retry_count: 0,
        keep_locks_after_death: false,
        wait: Some(true),
    }
}

fn composite_req(uuid: &str, keys: &[String]) -> Request {
    Request::Lock {
        uuid: uuid.into(),
        key: None,
        keys: Some(keys.to_vec()),
        pid: None,
        ttl: Some(600_000),
        max: None,
        force: false,
        retry_count: 0,
        keep_locks_after_death: false,
        wait: Some(true),
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

/// Drain every agent's mailbox; promote any Waiting agent whose terminal grant
/// (`acquired:true`) has arrived to Holding. Returns nothing; mutates `agents`.
fn drain_and_promote(rxs: &mut [UnboundedReceiver<Response>], agents: &mut [Agent]) {
    for i in 0..rxs.len() {
        let mut grant: Option<String> = None; // lock_uuid if granted
        while let Ok(msg) = rxs[i].try_recv() {
            match msg {
                Response::Lock {
                    acquired: true,
                    lock_uuid: Some(lu),
                    ..
                } => grant = Some(lu),
                Response::CompositeLock {
                    acquired: true,
                    lock_uuid: Some(lu),
                    ..
                } => grant = Some(lu),
                _ => {}
            }
        }
        if let Some(lu) = grant {
            if let Agent::Waiting { req_uuid, keys } = &agents[i] {
                agents[i] = Agent::Holding {
                    req_uuid: req_uuid.clone(),
                    lock_uuid: lu,
                    keys: keys.clone(),
                };
            }
        }
    }
}

fn run_seed(seed: u64, rounds: usize, ops_per_round: usize) {
    let broker = Broker::new(BrokerConfig::default());
    let n_agents = 8;
    let mut cids = Vec::new();
    let mut rxs = Vec::new();
    for _ in 0..n_agents {
        let (c, rx) = broker.register_client();
        cids.push(c);
        rxs.push(rx);
    }
    let keys: Vec<String> = (0..4).map(|i| format!("lv{seed}-{i}")).collect();
    let mut agents = vec![Agent::Idle; n_agents];
    let mut rng = Rng::new(seed);
    let mut seq = 0u64;

    for round in 0..rounds {
        for _ in 0..ops_per_round {
            let i = rng.below(n_agents);
            match agents[i].clone() {
                Agent::Idle => {
                    // Issue a blocking acquire (single or composite).
                    seq += 1;
                    if rng.below(100) < 50 {
                        let key = keys[rng.below(keys.len())].clone();
                        let uuid = format!("a{seq}");
                        broker.handle_request(cids[i], lock_req(&uuid, &key));
                        agents[i] = Agent::Waiting {
                            req_uuid: uuid,
                            keys: vec![key],
                        };
                    } else {
                        let want = 2 + rng.below(2);
                        let mut pool = keys.clone();
                        let mut chosen = Vec::new();
                        for _ in 0..want {
                            if pool.is_empty() {
                                break;
                            }
                            chosen.push(pool.remove(rng.below(pool.len())));
                        }
                        chosen.sort();
                        let uuid = format!("a{seq}");
                        broker.handle_request(cids[i], composite_req(&uuid, &chosen));
                        agents[i] = Agent::Waiting {
                            req_uuid: uuid,
                            keys: chosen,
                        };
                    }
                }
                Agent::Holding {
                    keys, lock_uuid, ..
                } => {
                    // Release.
                    seq += 1;
                    broker
                        .handle_request(cids[i], unlock_req(&format!("u{seq}"), &keys, &lock_uuid));
                    agents[i] = Agent::Idle;
                }
                Agent::Waiting { .. } => { /* can't act while queued */ }
            }
            drain_and_promote(&mut rxs, &mut agents);
        }

        // ---- DRAIN-TO-EMPTY liveness check ----
        // Release every full holder, cascading, until no holders remain. If the
        // grant machinery is healthy this also completes every queued waiter
        // (each gets its keys as holders free them). A waiter still queued after
        // all keys are free is a missed-wakeup / liveness bug.
        let mut guard = 0;
        loop {
            guard += 1;
            assert!(
                guard < 100_000,
                "drain loop runaway (seed={seed}, round={round})"
            );
            let holder = agents
                .iter()
                .position(|a| matches!(a, Agent::Holding { .. }));
            let Some(i) = holder else { break };
            if let Agent::Holding {
                keys, lock_uuid, ..
            } = agents[i].clone()
            {
                seq += 1;
                broker.handle_request(cids[i], unlock_req(&format!("d{seq}"), &keys, &lock_uuid));
                agents[i] = Agent::Idle;
            }
            drain_and_promote(&mut rxs, &mut agents);
        }

        let stuck: Vec<(usize, Vec<String>)> = agents
            .iter()
            .enumerate()
            .filter_map(|(i, a)| match a {
                Agent::Waiting { keys, .. } => Some((i, keys.clone())),
                _ => None,
            })
            .collect();
        if !stuck.is_empty() {
            // Dump the broker's *real* per-key state to distinguish a true
            // missed-wakeup (key free yet a waiter queued on it) from a
            // legitimate partial-composite-hold wait.
            eprintln!("---- stuck dump (seed={seed}, round={round}) ----");
            for k in &keys {
                seq += 1;
                broker.handle_request(
                    cids[0],
                    Request::LockInfo {
                        uuid: format!("li{seq}"),
                        key: k.clone(),
                    },
                );
                while let Ok(msg) = rxs[0].try_recv() {
                    if let Response::LockInfo {
                        key,
                        is_locked,
                        lockholder_uuids,
                        lock_request_count,
                        ..
                    } = msg
                    {
                        eprintln!(
                            "  key={key} is_locked={is_locked} holders={lockholder_uuids:?} queue_depth={lock_request_count}"
                        );
                    }
                }
            }
        }
        assert!(
            stuck.is_empty(),
            "LIVENESS BUG (seed={seed}, round={round}): agents still queued after releasing all full holders: {stuck:?}"
        );

        // All idle now — sanity: every key must be individually acquirable.
        for k in &keys {
            seq += 1;
            broker.handle_request(cids[0], lock_req(&format!("p{seq}"), k));
            let mut lu = None;
            while let Ok(msg) = rxs[0].try_recv() {
                if let Response::Lock {
                    acquired: true,
                    lock_uuid: Some(u),
                    ..
                } = msg
                {
                    lu = Some(u);
                }
            }
            let lu = lu.unwrap_or_else(|| {
                panic!("post-drain key {k} not free (seed={seed}, round={round})")
            });
            seq += 1;
            broker.handle_request(
                cids[0],
                unlock_req(&format!("pu{seq}"), std::slice::from_ref(k), &lu),
            );
            let _ = rxs[0].try_recv();
        }
    }
}

#[test]
fn composite_blocking_is_live_under_fuzz() {
    // A spread of seeds + many rounds: the missed-wakeup this regresses against
    // (composite queued on a free head while a later key is contended) first
    // reproduced at seed=1, round=3 with these parameters.
    for seed in 1u64..=60 {
        run_seed(seed.wrapping_mul(2_654_435_761), 60, 24);
    }
}
