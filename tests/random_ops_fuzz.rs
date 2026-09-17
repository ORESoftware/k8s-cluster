//! Seeded random-operation fuzz for the in-process broker API.
//!
//! This is a state-machine test: each generated request is applied to the
//! broker and to a small shadow model, then we assert broker metrics and
//! lock-info snapshots against the model. The default run is 10k operations;
//! override with `LMX_RANDOM_OPS=<n>` and replay failures with
//! `LMX_FUZZ_SEED=<u64>`.

#![allow(clippy::too_many_arguments)]

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dd_rust_network_mutex::{
    broker::{Broker, BrokerConfig, ClientId},
    protocol::{Request, Response, MAX_COMPOSITE_KEYS, PROTOCOL_VERSION},
    server::{run as run_server, ServerConfig},
    Client, ClientConfig, RwClient,
};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::Command as TokioCommand;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinSet;

#[derive(Clone)]
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
        debug_assert!(n > 0);
        (self.next_u64() % n as u64) as usize
    }

    fn pct(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HoldKind {
    Exclusive,
    Read,
    Write,
}

#[derive(Debug, Clone)]
struct Hold {
    client: ClientId,
    kind: HoldKind,
    keys: Vec<String>,
    ttl: bool,
    grant_seq: u64,
}

#[derive(Debug, Clone)]
struct Pending {
    client: ClientId,
    kind: HoldKind,
    keys: Vec<String>,
    ttl: bool,
    wait: bool,
}

#[derive(Default)]
struct Model {
    holds: BTreeMap<String, Hold>,
    pending: BTreeMap<String, Pending>,
    seen_tokens: BTreeMap<String, BTreeSet<u64>>,
    grant_seq: u64,
}

struct Agent {
    cid: ClientId,
    rx: UnboundedReceiver<Response>,
}

fn seed_for() -> u64 {
    if let Ok(s) = std::env::var("LMX_FUZZ_SEED") {
        if let Ok(seed) = s.parse() {
            eprintln!("[random_ops_fuzz] using LMX_FUZZ_SEED={seed}");
            return seed;
        }
    }
    let seed = 0x0D15_EA5E_D00D_F00D;
    eprintln!("[random_ops_fuzz] using default seed {seed} (override with LMX_FUZZ_SEED=<u64>)");
    seed
}

fn ops_for() -> usize {
    std::env::var("LMX_RANDOM_OPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000)
}

fn async_ops_for() -> usize {
    std::env::var("LMX_ASYNC_RANDOM_OPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000)
}

fn cross_lang_ops_for() -> usize {
    std::env::var("LMX_CROSS_LANG_RANDOM_OPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600)
}

fn drain(rx: &mut UnboundedReceiver<Response>) -> Vec<Response> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        out.push(msg);
    }
    out
}

fn normalize_keys(mut keys: Vec<String>) -> Vec<String> {
    keys.sort();
    keys.dedup();
    keys
}

fn choose_keys(rng: &mut Rng, keys: &[String], min: usize, max: usize) -> Vec<String> {
    let want = min + rng.below(max - min + 1);
    let mut pool = keys.to_vec();
    let mut out = Vec::new();
    for _ in 0..want.min(pool.len()) {
        let idx = rng.below(pool.len());
        out.push(pool.remove(idx));
    }
    normalize_keys(out)
}

fn single_lock_req(uuid: &str, key: &str, wait: bool, ttl: bool, force: bool) -> Request {
    Request::Lock {
        uuid: uuid.into(),
        key: Some(key.into()),
        keys: None,
        pid: None,
        ttl: ttl.then_some(60_000),
        max: None,
        force,
        retry_count: 0,
        keep_locks_after_death: false,
        wait: Some(wait),
    }
}

fn composite_lock_req(uuid: &str, keys: &[String], ttl: bool) -> Request {
    Request::Lock {
        uuid: uuid.into(),
        key: None,
        keys: Some(keys.to_vec()),
        pid: None,
        ttl: ttl.then_some(60_000),
        max: None,
        force: false,
        retry_count: 0,
        keep_locks_after_death: false,
        wait: Some(false),
    }
}

fn unlock_req(uuid: &str, keys: &[String], lock_uuid: Option<String>, force: bool) -> Request {
    Request::Unlock {
        uuid: uuid.into(),
        key: (keys.len() == 1).then(|| keys[0].clone()),
        keys: (keys.len() > 1).then(|| keys.to_vec()),
        lock_uuid,
        force,
    }
}

impl Model {
    fn insert_pending(&mut self, uuid: String, pending: Pending) {
        let old = self.pending.insert(uuid.clone(), pending);
        assert!(old.is_none(), "duplicate pending request uuid {uuid}");
    }

    fn remove_hold(&mut self, lock_uuid: &str) {
        let removed = self.holds.remove(lock_uuid);
        assert!(removed.is_some(), "model missing held lock {lock_uuid}");
    }

    fn remove_holds_intersecting(&mut self, keys: &[String]) {
        let target: BTreeSet<&str> = keys.iter().map(String::as_str).collect();
        self.holds
            .retain(|_, h| !h.keys.iter().any(|k| target.contains(k.as_str())));
    }

    fn remove_client(&mut self, cid: ClientId) {
        self.pending.retain(|_, p| p.client != cid);
        self.holds.retain(|_, h| h.client != cid);
    }

    fn expire_ttl_holds(&mut self) -> usize {
        let mut expired = 0;
        self.holds.retain(|_, h| {
            if h.ttl {
                expired += 1;
                false
            } else {
                true
            }
        });
        expired
    }

    fn sorted_holds(&self) -> Vec<(String, Hold)> {
        let mut holds: Vec<_> = self
            .holds
            .iter()
            .map(|(lock_uuid, hold)| (lock_uuid.clone(), hold.clone()))
            .collect();
        holds.sort_by_key(|(_, h)| h.grant_seq);
        holds
    }

    fn sorted_holds_of_kind(&self, kind: HoldKind) -> Vec<(String, Hold)> {
        self.sorted_holds()
            .into_iter()
            .filter(|(_, h)| h.kind == kind)
            .collect()
    }

    fn check_token(&mut self, key: &str, token: u64, seed: u64, op: usize) {
        assert!(
            token > 0,
            "fencing token must be positive for key={key}: token={token} seed={seed} op={op}"
        );
        let inserted = self
            .seen_tokens
            .entry(key.to_string())
            .or_default()
            .insert(token);
        assert!(
            inserted,
            "duplicate fencing token for key={key}: token={token} seed={seed} op={op}"
        );
    }

    fn record_grant(
        &mut self,
        uuid: &str,
        client: ClientId,
        lock_uuid: String,
        keys: Vec<String>,
        kind: HoldKind,
        seed: u64,
        op: usize,
    ) {
        let pending = self
            .pending
            .remove(uuid)
            .unwrap_or_else(|| panic!("grant for unknown request uuid={uuid} seed={seed} op={op}"));
        assert_eq!(
            pending.client, client,
            "grant went to wrong client for uuid={uuid} seed={seed} op={op}"
        );
        assert_eq!(
            pending.kind, kind,
            "grant kind mismatch for uuid={uuid} seed={seed} op={op}"
        );
        assert_eq!(
            normalize_keys(pending.keys.clone()),
            normalize_keys(keys.clone()),
            "grant keys mismatch for uuid={uuid} seed={seed} op={op}"
        );

        self.grant_seq += 1;
        let old = self.holds.insert(
            lock_uuid.clone(),
            Hold {
                client,
                kind,
                keys,
                ttl: pending.ttl,
                grant_seq: self.grant_seq,
            },
        );
        assert!(
            old.is_none(),
            "duplicate lock_uuid grant lock_uuid={lock_uuid} seed={seed} op={op}"
        );
    }

    fn handle_lock_response(
        &mut self,
        client: ClientId,
        uuid: String,
        key: String,
        acquired: bool,
        lock_uuid: Option<String>,
        fencing_token: Option<u64>,
        error: Option<String>,
        seed: u64,
        op: usize,
    ) {
        if acquired {
            let token = fencing_token
                .unwrap_or_else(|| panic!("granted lock missing token seed={seed} op={op}"));
            self.check_token(&key, token, seed, op);
            let lock_uuid = lock_uuid
                .unwrap_or_else(|| panic!("granted lock missing lock_uuid seed={seed} op={op}"));
            self.record_grant(
                &uuid,
                client,
                lock_uuid,
                vec![key],
                HoldKind::Exclusive,
                seed,
                op,
            );
        } else {
            let pending = self.pending.get(&uuid).unwrap_or_else(|| {
                panic!("lock denial for unknown request uuid={uuid} seed={seed} op={op}")
            });
            if !pending.wait || error.is_some() {
                self.pending.remove(&uuid);
            }
        }
    }

    fn handle_composite_response(
        &mut self,
        client: ClientId,
        uuid: String,
        keys: Vec<String>,
        acquired: bool,
        lock_uuid: Option<String>,
        fencing_tokens: Option<BTreeMap<String, u64>>,
        error: Option<String>,
        seed: u64,
        op: usize,
    ) {
        if acquired {
            let tokens = fencing_tokens
                .unwrap_or_else(|| panic!("granted composite missing tokens seed={seed} op={op}"));
            assert_eq!(
                tokens.len(),
                keys.len(),
                "composite token count mismatch seed={seed} op={op}"
            );
            for key in &keys {
                let token = *tokens.get(key).unwrap_or_else(|| {
                    panic!("missing composite token for key={key} seed={seed} op={op}")
                });
                self.check_token(key, token, seed, op);
            }
            let lock_uuid = lock_uuid.unwrap_or_else(|| {
                panic!("granted composite missing lock_uuid seed={seed} op={op}")
            });
            self.record_grant(
                &uuid,
                client,
                lock_uuid,
                keys,
                HoldKind::Exclusive,
                seed,
                op,
            );
        } else {
            let pending = self.pending.get(&uuid).unwrap_or_else(|| {
                panic!("composite denial for unknown request uuid={uuid} seed={seed} op={op}")
            });
            if !pending.wait || error.is_some() {
                self.pending.remove(&uuid);
            }
        }
    }

    fn handle_rw_response(
        &mut self,
        client: ClientId,
        uuid: String,
        key: String,
        granted: bool,
        lock_uuid: Option<String>,
        fencing_token: Option<u64>,
        kind: HoldKind,
        seed: u64,
        op: usize,
    ) {
        if granted {
            let token = fencing_token
                .unwrap_or_else(|| panic!("granted rw lock missing token seed={seed} op={op}"));
            self.check_token(&key, token, seed, op);
            let lock_uuid = lock_uuid
                .unwrap_or_else(|| panic!("granted rw lock missing lock_uuid seed={seed} op={op}"));
            self.record_grant(&uuid, client, lock_uuid, vec![key], kind, seed, op);
        } else {
            assert!(
                self.pending.contains_key(&uuid),
                "queued rw response for unknown uuid={uuid} seed={seed} op={op}"
            );
        }
    }

    fn assert_lock_info(
        &self,
        key: &str,
        is_locked: bool,
        lockholder_uuids: Vec<String>,
        lock_request_count: usize,
        readers_count: u32,
        writer_flag: bool,
        seed: u64,
        op: usize,
    ) {
        let mut expected_holders = BTreeSet::new();
        let mut expected_readers = 0usize;
        let mut expected_writer = false;
        for (lock_uuid, hold) in &self.holds {
            if hold.keys.iter().any(|k| k == key) {
                expected_holders.insert(lock_uuid.clone());
                match hold.kind {
                    HoldKind::Exclusive => {}
                    HoldKind::Read => expected_readers += 1,
                    HoldKind::Write => expected_writer = true,
                }
            }
        }
        let actual_holders: BTreeSet<_> = lockholder_uuids.into_iter().collect();
        let expected_waiters = self
            .pending
            .values()
            .filter(|p| p.keys.len() == 1 && p.keys[0] == key)
            .count();

        assert_eq!(
            actual_holders, expected_holders,
            "LockInfo holders mismatch for key={key} seed={seed} op={op}"
        );
        assert_eq!(
            is_locked,
            !expected_holders.is_empty(),
            "LockInfo is_locked mismatch for key={key} seed={seed} op={op}"
        );
        assert_eq!(
            readers_count as usize, expected_readers,
            "LockInfo readers mismatch for key={key} seed={seed} op={op}"
        );
        assert_eq!(
            writer_flag, expected_writer,
            "LockInfo writer mismatch for key={key} seed={seed} op={op}"
        );
        assert_eq!(
            lock_request_count, expected_waiters,
            "LockInfo queue depth mismatch for key={key} seed={seed} op={op}"
        );
    }

    fn assert_invariants(&self, broker: &Broker, live_clients: usize, seed: u64, op: usize) {
        let mut by_key: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
        for hold in self.holds.values() {
            for key in &hold.keys {
                let counts = by_key.entry(key.as_str()).or_default();
                match hold.kind {
                    HoldKind::Exclusive => counts.0 += 1,
                    HoldKind::Read => counts.1 += 1,
                    HoldKind::Write => counts.2 += 1,
                }
            }
        }

        for (key, (exclusive, readers, writers)) in by_key {
            assert!(
                writers <= 1,
                "more than one writer for key={key} seed={seed} op={op}"
            );
            if writers > 0 {
                assert_eq!(
                    exclusive, 0,
                    "writer overlapped exclusive holder for key={key} seed={seed} op={op}"
                );
                assert_eq!(
                    readers, 0,
                    "writer overlapped readers for key={key} seed={seed} op={op}"
                );
            }
            if exclusive > 0 {
                assert_eq!(
                    exclusive, 1,
                    "multiple exclusive holders for key={key} seed={seed} op={op}"
                );
                assert_eq!(
                    readers, 0,
                    "exclusive holder overlapped readers for key={key} seed={seed} op={op}"
                );
            }
        }

        let expected_holders: u64 = self.holds.values().map(|h| h.keys.len() as u64).sum();
        let metrics = broker.metrics();
        assert_eq!(
            metrics.holders, expected_holders,
            "broker/model holder count mismatch seed={seed} op={op}"
        );
        assert_eq!(
            metrics.waiters,
            self.pending.len() as u64,
            "broker/model waiter count mismatch seed={seed} op={op}"
        );
        assert_eq!(
            metrics.clients, live_clients as u64,
            "broker/model live client count mismatch seed={seed} op={op}"
        );
    }
}

fn handle_response(model: &mut Model, client: ClientId, msg: Response, seed: u64, op: usize) {
    match msg {
        Response::Lock {
            uuid,
            key,
            acquired,
            lock_uuid,
            fencing_token,
            error,
            ..
        } => model.handle_lock_response(
            client,
            uuid,
            key,
            acquired,
            lock_uuid,
            fencing_token,
            error,
            seed,
            op,
        ),
        Response::CompositeLock {
            uuid,
            keys,
            acquired,
            lock_uuid,
            fencing_tokens,
            error,
        } => model.handle_composite_response(
            client,
            uuid,
            keys,
            acquired,
            lock_uuid,
            fencing_tokens,
            error,
            seed,
            op,
        ),
        Response::RegisterReadResult {
            uuid,
            key,
            granted,
            lock_uuid,
            fencing_token,
            ..
        } => model.handle_rw_response(
            client,
            uuid,
            key,
            granted,
            lock_uuid,
            fencing_token,
            HoldKind::Read,
            seed,
            op,
        ),
        Response::RegisterWriteResult {
            uuid,
            key,
            granted,
            lock_uuid,
            fencing_token,
            ..
        } => model.handle_rw_response(
            client,
            uuid,
            key,
            granted,
            lock_uuid,
            fencing_token,
            HoldKind::Write,
            seed,
            op,
        ),
        Response::LockInfo {
            key,
            is_locked,
            lockholder_uuids,
            lock_request_count,
            readers_count,
            writer_flag,
            ..
        } => model.assert_lock_info(
            &key,
            is_locked,
            lockholder_uuids,
            lock_request_count,
            readers_count,
            writer_flag,
            seed,
            op,
        ),
        Response::Unlock { .. }
        | Response::EndReadResult { .. }
        | Response::EndWriteResult { .. }
        | Response::Version { .. }
        | Response::Auth { .. }
        | Response::LsResult { .. }
        | Response::Reelection { .. }
        | Response::Error { .. }
        | Response::Ok { .. } => {}
    }
}

fn drain_all(model: &mut Model, agents: &mut [Agent], seed: u64, op: usize) {
    for agent in agents {
        for msg in drain(&mut agent.rx) {
            handle_response(model, agent.cid, msg, seed, op);
        }
    }
}

fn random_held_lock(model: &Model, rng: &mut Rng) -> Option<(String, Hold)> {
    let holds = model.sorted_holds();
    if holds.is_empty() {
        None
    } else {
        Some(holds[rng.below(holds.len())].clone())
    }
}

fn random_held_lock_of_kind(
    model: &Model,
    rng: &mut Rng,
    kind: HoldKind,
) -> Option<(String, Hold)> {
    let holds = model.sorted_holds_of_kind(kind);
    if holds.is_empty() {
        None
    } else {
        Some(holds[rng.below(holds.len())].clone())
    }
}

fn run_seed(seed: u64, ops: usize) {
    let broker = Broker::new(BrokerConfig {
        ttl_sweep_interval: Duration::ZERO,
        idle_key_grace: Duration::ZERO,
        ..BrokerConfig::default()
    });
    let mut agents: Vec<Agent> = (0..8)
        .map(|_| {
            let (cid, rx) = broker.register_client();
            Agent { cid, rx }
        })
        .collect();
    let keys: Vec<String> = (0..6).map(|i| format!("rop-{seed}-{i}")).collect();

    let mut model = Model::default();
    let mut rng = Rng::new(seed);
    let mut req_seq = 0u64;

    for op in 0..ops {
        req_seq += 1;
        let roll = rng.below(100);
        let agent_idx = rng.below(agents.len());
        let cid = agents[agent_idx].cid;
        let uuid = format!("op{req_seq}");

        match roll {
            0..=24 => {
                let key = keys[rng.below(keys.len())].clone();
                let wait = rng.pct(60);
                let ttl = rng.pct(70);
                let force = wait && rng.pct(7);
                model.insert_pending(
                    uuid.clone(),
                    Pending {
                        client: cid,
                        kind: HoldKind::Exclusive,
                        keys: vec![key.clone()],
                        ttl,
                        wait,
                    },
                );
                broker.handle_request(cid, single_lock_req(&uuid, &key, wait, ttl, force));
            }
            25..=39 => {
                let chosen = choose_keys(&mut rng, &keys, 2, MAX_COMPOSITE_KEYS.min(keys.len()));
                let ttl = rng.pct(70);
                model.insert_pending(
                    uuid.clone(),
                    Pending {
                        client: cid,
                        kind: HoldKind::Exclusive,
                        keys: chosen.clone(),
                        ttl,
                        wait: false,
                    },
                );
                broker.handle_request(cid, composite_lock_req(&uuid, &chosen, ttl));
            }
            40..=52 => {
                let key = keys[rng.below(keys.len())].clone();
                model.insert_pending(
                    uuid.clone(),
                    Pending {
                        client: cid,
                        kind: HoldKind::Read,
                        keys: vec![key.clone()],
                        ttl: false,
                        wait: true,
                    },
                );
                broker.handle_request(cid, Request::RegisterRead { uuid, key });
            }
            53..=63 => {
                let key = keys[rng.below(keys.len())].clone();
                model.insert_pending(
                    uuid.clone(),
                    Pending {
                        client: cid,
                        kind: HoldKind::Write,
                        keys: vec![key.clone()],
                        ttl: false,
                        wait: true,
                    },
                );
                broker.handle_request(cid, Request::RegisterWrite { uuid, key });
            }
            64..=76 => {
                if let Some((lock_uuid, hold)) = random_held_lock(&model, &mut rng) {
                    assert!(
                        agents.iter().any(|a| a.cid == hold.client),
                        "model referenced non-live client {}",
                        hold.client
                    );
                    model.remove_hold(&lock_uuid);
                    broker.handle_request(
                        hold.client,
                        unlock_req(&uuid, &hold.keys, Some(lock_uuid), false),
                    );
                }
            }
            77..=82 => {
                if rng.pct(50) {
                    if let Some((_, hold)) =
                        random_held_lock_of_kind(&model, &mut rng, HoldKind::Read)
                    {
                        let key = hold.keys[0].clone();
                        model.holds.retain(|_, h| {
                            !(h.client == hold.client
                                && h.kind == HoldKind::Read
                                && h.keys.iter().any(|k| k == &key))
                        });
                        broker.handle_request(hold.client, Request::EndRead { uuid, key });
                    }
                } else if let Some((lock_uuid, hold)) =
                    random_held_lock_of_kind(&model, &mut rng, HoldKind::Write)
                {
                    let key = hold.keys[0].clone();
                    model.remove_hold(&lock_uuid);
                    broker.handle_request(hold.client, Request::EndWrite { uuid, key });
                }
            }
            83..=87 => {
                if let Some((lock_uuid, hold)) = random_held_lock(&model, &mut rng) {
                    if rng.pct(50) {
                        let operator = agents[rng.below(agents.len())].cid;
                        model.remove_hold(&lock_uuid);
                        broker.handle_request(
                            operator,
                            unlock_req(&uuid, &hold.keys, Some(lock_uuid), true),
                        );
                    } else {
                        let operator = agents[rng.below(agents.len())].cid;
                        model.remove_holds_intersecting(&hold.keys);
                        broker.handle_request(operator, unlock_req(&uuid, &hold.keys, None, true));
                    }
                }
            }
            88..=91 => {
                let idx = rng.below(agents.len());
                let old_cid = agents[idx].cid;
                model.remove_client(old_cid);
                broker.drop_client(old_cid);
                let (new_cid, new_rx) = broker.register_client();
                agents[idx] = Agent {
                    cid: new_cid,
                    rx: new_rx,
                };
            }
            92..=94 => {
                let expected = model.expire_ttl_holds();
                let evicted = broker.tick_ttl(Instant::now() + Duration::from_secs(3600));
                assert_eq!(
                    evicted, expected,
                    "TTL eviction count mismatch seed={seed} op={op}"
                );
            }
            _ => {
                let key = keys[rng.below(keys.len())].clone();
                match rng.below(5) {
                    0 => broker.handle_request(cid, Request::LockInfo { uuid, key }),
                    1 => broker.handle_request(cid, Request::Ls { uuid }),
                    2 => broker.handle_request(cid, Request::Heartbeat { uuid }),
                    3 => broker.handle_request(
                        cid,
                        Request::Version {
                            uuid,
                            value: PROTOCOL_VERSION.into(),
                        },
                    ),
                    _ => broker.handle_request(
                        cid,
                        Request::Auth {
                            uuid,
                            token: "unused-in-in-process-broker".into(),
                        },
                    ),
                }
            }
        }

        drain_all(&mut model, &mut agents, seed, op);
        model.assert_invariants(&broker, agents.len(), seed, op);
    }

    for agent in &agents {
        model.remove_client(agent.cid);
        broker.drop_client(agent.cid);
    }
    model.assert_invariants(&broker, 0, seed, ops);
    assert_eq!(broker.metrics().holders, 0, "holders leaked after cleanup");
    assert_eq!(broker.metrics().waiters, 0, "waiters leaked after cleanup");
}

#[test]
fn random_order_operations_hold_invariants_for_10k_ops() {
    run_seed(seed_for(), ops_for());
}

#[derive(Debug, Clone)]
struct AsyncHold {
    kind: HoldKind,
    keys: Vec<String>,
}

#[derive(Default)]
struct AsyncOracle {
    active: BTreeMap<String, AsyncHold>,
    seen_tokens: BTreeMap<String, BTreeSet<u64>>,
}

impl AsyncOracle {
    fn check_tokens(
        &mut self,
        keys: &[String],
        tokens: &BTreeMap<String, u64>,
        seed: u64,
        op: usize,
    ) {
        for key in keys {
            let token = *tokens.get(key).unwrap_or_else(|| {
                panic!("async grant missing token for key={key} seed={seed} op={op}")
            });
            assert!(
                token > 0,
                "async grant returned non-positive token key={key} token={token} seed={seed} op={op}"
            );
            let inserted = self
                .seen_tokens
                .entry(key.clone())
                .or_default()
                .insert(token);
            assert!(
                inserted,
                "async grant duplicated fencing token key={key} token={token} seed={seed} op={op}"
            );
        }
    }

    fn on_grant(
        &mut self,
        lock_uuid: String,
        kind: HoldKind,
        keys: Vec<String>,
        tokens: BTreeMap<String, u64>,
        seed: u64,
        op: usize,
    ) {
        self.check_tokens(&keys, &tokens, seed, op);
        for key in &keys {
            let mut exclusive = 0usize;
            let mut readers = 0usize;
            let mut writers = 0usize;
            for hold in self.active.values() {
                if hold.keys.iter().any(|k| k == key) {
                    match hold.kind {
                        HoldKind::Exclusive => exclusive += 1,
                        HoldKind::Read => readers += 1,
                        HoldKind::Write => writers += 1,
                    }
                }
            }

            match kind {
                HoldKind::Exclusive => {
                    assert_eq!(
                        exclusive, 0,
                        "async exclusive grant overlapped exclusive key={key} seed={seed} op={op}"
                    );
                    assert_eq!(
                        readers, 0,
                        "async exclusive grant overlapped readers key={key} seed={seed} op={op}"
                    );
                    assert_eq!(
                        writers, 0,
                        "async exclusive grant overlapped writer key={key} seed={seed} op={op}"
                    );
                }
                HoldKind::Read => {
                    assert_eq!(
                        exclusive, 0,
                        "async read grant overlapped exclusive key={key} seed={seed} op={op}"
                    );
                    assert_eq!(
                        writers, 0,
                        "async read grant overlapped writer key={key} seed={seed} op={op}"
                    );
                }
                HoldKind::Write => {
                    assert_eq!(
                        exclusive, 0,
                        "async write grant overlapped exclusive key={key} seed={seed} op={op}"
                    );
                    assert_eq!(
                        readers, 0,
                        "async write grant overlapped readers key={key} seed={seed} op={op}"
                    );
                    assert_eq!(
                        writers, 0,
                        "async write grant overlapped writer key={key} seed={seed} op={op}"
                    );
                }
            }
        }

        let old = self
            .active
            .insert(lock_uuid.clone(), AsyncHold { kind, keys });
        assert!(
            old.is_none(),
            "async duplicate active lock_uuid={lock_uuid} seed={seed} op={op}"
        );
    }

    fn on_release(&mut self, lock_uuid: &str, seed: u64, op: usize) {
        let removed = self.active.remove(lock_uuid);
        assert!(
            removed.is_some(),
            "async release of unknown lock_uuid={lock_uuid} seed={seed} op={op}"
        );
    }

    fn assert_clean(&self) {
        assert!(
            self.active.is_empty(),
            "async oracle still has active locks after workers finished: {:?}",
            self.active
        );
    }
}

async fn pick_tcp_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

async fn start_tcp_broker() -> (u16, tokio::task::JoinHandle<()>) {
    start_tcp_broker_on("127.0.0.1").await
}

async fn start_tcp_broker_on(bind_host: &str) -> (u16, tokio::task::JoinHandle<()>) {
    let port = pick_tcp_port().await;
    let addr: SocketAddr = format!("{bind_host}:{port}").parse().unwrap();
    let cfg = ServerConfig {
        tcp_bind: Some(addr),
        uds_path: None,
        http_bind: None,
        auth_token: None,
        broker: BrokerConfig::default(),
        tcp_nodelay: true,
        tcp_quickack: false,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    };
    let handle = tokio::spawn(async move {
        if let Err(err) = run_server(cfg).await {
            eprintln!("async fuzz broker exited: {err:?}");
        }
    });
    tokio::time::sleep(Duration::from_millis(75)).await;
    (port, handle)
}

fn client_cfg(timeout_ms: u64) -> ClientConfig {
    ClientConfig {
        auth_token: None,
        default_request_timeout: Duration::from_millis(timeout_ms),
    }
}

fn lock_tokens_for(
    keys: &[String],
    single_token: Option<u64>,
    tokens: &BTreeMap<String, u64>,
) -> BTreeMap<String, u64> {
    if !tokens.is_empty() {
        return tokens.clone();
    }
    let mut out = BTreeMap::new();
    if let (Some(key), Some(token)) = (keys.first(), single_token) {
        out.insert(key.clone(), token);
    }
    out
}

async fn async_hold_briefly(rng: &mut Rng) {
    match rng.below(4) {
        0 => tokio::task::yield_now().await,
        n => tokio::time::sleep(Duration::from_millis(n as u64)).await,
    }
}

async fn run_async_seed(seed: u64, ops: usize) {
    let (port, server) = start_tcp_broker().await;
    let oracle = Arc::new(Mutex::new(AsyncOracle::default()));
    let keys: Vec<String> = (0..5).map(|i| format!("async-rop-{seed}-{i}")).collect();
    let workers = 12usize;
    let ops_per_worker = (ops / workers).max(1);
    let mut tasks = JoinSet::new();

    for worker in 0..workers {
        let oracle = Arc::clone(&oracle);
        let keys = keys.clone();
        tasks.spawn(async move {
            let client = Client::connect_tcp(("127.0.0.1", port), client_cfg(20_000))
                .await
                .expect("async fuzz client connect");
            let rw = RwClient::connect_tcp(("127.0.0.1", port), client_cfg(20_000))
                .await
                .expect("async fuzz rw client connect");
            let mut rng = Rng::new(seed.wrapping_add((worker as u64 + 1) * 0xA5A5_0001));

            for local_op in 0..ops_per_worker {
                let op = worker * ops_per_worker + local_op;
                let roll = rng.below(100);
                match roll {
                    0..=29 => {
                        let key = keys[rng.below(keys.len())].clone();
                        if let Some(guard) = client
                            .try_acquire(&key, Duration::from_millis(60_000))
                            .await
                            .expect("try_acquire should not error")
                        {
                            let tokens = lock_tokens_for(
                                &guard.keys,
                                guard.fencing_token,
                                &guard.fencing_tokens,
                            );
                            oracle.lock().unwrap().on_grant(
                                guard.lock_uuid.clone(),
                                HoldKind::Exclusive,
                                guard.keys.clone(),
                                tokens,
                                seed,
                                op,
                            );
                            async_hold_briefly(&mut rng).await;
                            oracle
                                .lock()
                                .unwrap()
                                .on_release(&guard.lock_uuid, seed, op);
                            client.release(&guard).await.expect("try_acquire release");
                        }
                    }
                    30..=49 => {
                        let key = keys[rng.below(keys.len())].clone();
                        let guard = client
                            .acquire(&key, Duration::from_millis(60_000))
                            .await
                            .expect("blocking acquire should not error");
                        let tokens = lock_tokens_for(
                            &guard.keys,
                            guard.fencing_token,
                            &guard.fencing_tokens,
                        );
                        oracle.lock().unwrap().on_grant(
                            guard.lock_uuid.clone(),
                            HoldKind::Exclusive,
                            guard.keys.clone(),
                            tokens,
                            seed,
                            op,
                        );
                        async_hold_briefly(&mut rng).await;
                        oracle
                            .lock()
                            .unwrap()
                            .on_release(&guard.lock_uuid, seed, op);
                        client
                            .release(&guard)
                            .await
                            .expect("blocking acquire release");
                    }
                    50..=64 => {
                        let chosen =
                            choose_keys(&mut rng, &keys, 2, MAX_COMPOSITE_KEYS.min(keys.len()));
                        let refs: Vec<&str> = chosen.iter().map(String::as_str).collect();
                        if let Some(guard) = client
                            .try_acquire_composite(&refs, Duration::from_millis(60_000))
                            .await
                            .expect("try_acquire_composite should not error")
                        {
                            let tokens = lock_tokens_for(
                                &guard.keys,
                                guard.fencing_token,
                                &guard.fencing_tokens,
                            );
                            oracle.lock().unwrap().on_grant(
                                guard.lock_uuid.clone(),
                                HoldKind::Exclusive,
                                guard.keys.clone(),
                                tokens,
                                seed,
                                op,
                            );
                            async_hold_briefly(&mut rng).await;
                            oracle
                                .lock()
                                .unwrap()
                                .on_release(&guard.lock_uuid, seed, op);
                            client.release(&guard).await.expect("try composite release");
                        }
                    }
                    65..=74 => {
                        let chosen =
                            choose_keys(&mut rng, &keys, 2, MAX_COMPOSITE_KEYS.min(keys.len()));
                        let refs: Vec<&str> = chosen.iter().map(String::as_str).collect();
                        let guard = client
                            .acquire_composite(&refs, Duration::from_millis(60_000))
                            .await
                            .expect("blocking composite should not error");
                        let tokens = lock_tokens_for(
                            &guard.keys,
                            guard.fencing_token,
                            &guard.fencing_tokens,
                        );
                        oracle.lock().unwrap().on_grant(
                            guard.lock_uuid.clone(),
                            HoldKind::Exclusive,
                            guard.keys.clone(),
                            tokens,
                            seed,
                            op,
                        );
                        async_hold_briefly(&mut rng).await;
                        oracle
                            .lock()
                            .unwrap()
                            .on_release(&guard.lock_uuid, seed, op);
                        client
                            .release(&guard)
                            .await
                            .expect("blocking composite release");
                    }
                    75..=91 => {
                        let key = keys[rng.below(keys.len())].clone();
                        let guard = rw.acquire_read(&key).await.expect("rw read acquire");
                        let mut tokens = BTreeMap::new();
                        if let Some(token) = guard.fencing_token {
                            tokens.insert(guard.key.clone(), token);
                        }
                        oracle.lock().unwrap().on_grant(
                            guard.lock_uuid.clone(),
                            HoldKind::Read,
                            vec![guard.key.clone()],
                            tokens,
                            seed,
                            op,
                        );
                        async_hold_briefly(&mut rng).await;
                        let lock_uuid = guard.lock_uuid.clone();
                        oracle.lock().unwrap().on_release(&lock_uuid, seed, op);
                        guard.release().await.expect("rw read release");
                    }
                    _ => {
                        let key = keys[rng.below(keys.len())].clone();
                        let guard = rw.acquire_write(&key).await.expect("rw write acquire");
                        let mut tokens = BTreeMap::new();
                        if let Some(token) = guard.fencing_token {
                            tokens.insert(guard.key.clone(), token);
                        }
                        oracle.lock().unwrap().on_grant(
                            guard.lock_uuid.clone(),
                            HoldKind::Write,
                            vec![guard.key.clone()],
                            tokens,
                            seed,
                            op,
                        );
                        async_hold_briefly(&mut rng).await;
                        let lock_uuid = guard.lock_uuid.clone();
                        oracle.lock().unwrap().on_release(&lock_uuid, seed, op);
                        guard.release().await.expect("rw write release");
                    }
                }
            }
        });
    }

    while let Some(result) = tasks.join_next().await {
        result.expect("async fuzz worker panicked");
    }

    oracle.lock().unwrap().assert_clean();
    let inspector = Client::connect_tcp(("127.0.0.1", port), client_cfg(5_000))
        .await
        .expect("async fuzz inspector connect");
    for key in &keys {
        let info = inspector.lock_info(key).await.expect("final lock_info");
        assert!(
            !info.is_locked,
            "async fuzz leaked key={key}: final lock_info={info:?}"
        );
        assert_eq!(
            info.lock_request_count, 0,
            "async fuzz leaked waiters for key={key}: final lock_info={info:?}"
        );
    }
    server.abort();
    let _ = server.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_tcp_random_operations_reject_invalid_states() {
    run_async_seed(
        seed_for().wrapping_add(0xA5A5_A5A5_A5A5_A5A5),
        async_ops_for(),
    )
    .await;
}

#[derive(Clone)]
struct ExternalWorker {
    lang: &'static str,
    cwd: PathBuf,
    program: String,
    args: Vec<String>,
    broker_host: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CrossToolchain {
    Local,
    Docker,
    Nix,
}

#[derive(Debug, Deserialize)]
struct WorkerEvent {
    event: String,
    lang: String,
    worker: String,
    #[serde(default, rename = "lockUuid")]
    lock_uuid: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    keys: Vec<String>,
    #[serde(default)]
    tokens: BTreeMap<String, u64>,
    #[serde(default)]
    ops: Option<usize>,
}

fn command_available(name: &str) -> bool {
    matches!(
        StdCommand::new(name)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status(),
        Ok(s) if s.success()
    )
}

fn ensure_command(name: &str) {
    assert!(
        command_available(name),
        "cross-language fuzz requires `{name}` on PATH"
    );
}

fn run_prepare_command(cwd: &Path, program: &str, args: &[&str]) {
    let status = StdCommand::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap_or_else(|err| panic!("failed to run {program} {args:?} in {cwd:?}: {err}"));
    assert!(
        status.success(),
        "preparing cross-language worker failed: {program} {args:?} in {cwd:?}"
    );
}

fn run_prepare_shell(cwd: &Path, program: &str, args: &[String]) {
    let status = StdCommand::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap_or_else(|err| panic!("failed to run {program} {args:?} in {cwd:?}: {err}"));
    assert!(
        status.success(),
        "preparing cross-language worker failed: {program} {args:?} in {cwd:?}"
    );
}

fn local_toolchain_ready(root: &Path) -> bool {
    ["node", "npm", "dart", "java", "javac", "gleam", "erl"]
        .into_iter()
        .all(command_available)
        && root.join("clients/ts/node_modules/.bin/tsx").exists()
        && root
            .join("clients/dart/.dart_tool/package_config.json")
            .exists()
}

fn cross_toolchain_mode(root: &Path) -> CrossToolchain {
    match std::env::var("LMX_CROSS_LANG_TOOLCHAIN")
        .unwrap_or_else(|_| "auto".into())
        .as_str()
    {
        "local" => CrossToolchain::Local,
        "docker" => CrossToolchain::Docker,
        "nix" => CrossToolchain::Nix,
        "auto" => {
            if local_toolchain_ready(root) {
                CrossToolchain::Local
            } else if command_available("docker") {
                CrossToolchain::Docker
            } else if command_available("nix") {
                CrossToolchain::Nix
            } else {
                panic!(
                    "cross-language fuzz needs local tools, Docker, or Nix; set LMX_CROSS_LANG_TOOLCHAIN=local|docker|nix"
                );
            }
        }
        other => {
            panic!("unknown LMX_CROSS_LANG_TOOLCHAIN={other}; expected auto|local|docker|nix")
        }
    }
}

fn nix_args(packages: &[&str], shell: &str) -> Vec<String> {
    let mut args = vec![
        "--extra-experimental-features".to_string(),
        "nix-command flakes".to_string(),
        "shell".to_string(),
    ];
    args.extend(packages.iter().map(|p| format!("nixpkgs#{p}")));
    args.extend([
        "--command".to_string(),
        "bash".to_string(),
        "-c".to_string(),
        shell.to_string(),
    ]);
    args
}

fn docker_image(lang: &str, default: &str) -> String {
    std::env::var(format!("LMX_DOCKER_{}_IMAGE", lang.to_ascii_uppercase()))
        .unwrap_or_else(|_| default.to_string())
}

fn docker_run_args(root: &Path, image: String, workdir: &str, shell: &str) -> Vec<String> {
    vec![
        "run".into(),
        "--rm".into(),
        "-i".into(),
        "--add-host=host.docker.internal:host-gateway".into(),
        "-v".into(),
        format!("{}:/repo", root.display()),
        "-w".into(),
        workdir.into(),
        "-e".into(),
        "LIVE_MUTEX_HOST".into(),
        "-e".into(),
        "LIVE_MUTEX_PORT".into(),
        "-e".into(),
        "LMX_WORKER_LANG".into(),
        "-e".into(),
        "LMX_WORKER_ID".into(),
        "-e".into(),
        "LMX_WORKER_SEED".into(),
        "-e".into(),
        "LMX_WORKER_OPS".into(),
        "-e".into(),
        "LMX_FUZZ_KEY_PREFIX".into(),
        "-e".into(),
        "LMX_FUZZ_KEY_COUNT".into(),
        image,
        "sh".into(),
        "-lc".into(),
        shell.into(),
    ]
}

fn docker_prepare_args(root: &Path, image: String, workdir: &str, shell: &str) -> Vec<String> {
    vec![
        "run".into(),
        "--rm".into(),
        "-v".into(),
        format!("{}:/repo", root.display()),
        "-w".into(),
        workdir.into(),
        image,
        "sh".into(),
        "-lc".into(),
        shell.into(),
    ]
}

fn prepare_cross_language_workers(root: &Path, mode: CrossToolchain) {
    match mode {
        CrossToolchain::Local => {
            for cmd in ["node", "npm", "dart", "java", "javac", "gleam", "erl"] {
                ensure_command(cmd);
            }

            let tsx = root.join("clients/ts/node_modules/.bin/tsx");
            assert!(
                tsx.exists(),
                "cross-language fuzz requires clients/ts/node_modules; run `npm install` in clients/ts"
            );
            let dart_packages = root.join("clients/dart/.dart_tool/package_config.json");
            assert!(
                dart_packages.exists(),
                "cross-language fuzz requires Dart packages; run `dart pub get` in clients/dart"
            );

            run_prepare_command(&root.join("clients/java"), "./build.sh", &[]);
            run_prepare_command(&root.join("clients/gleam"), "gleam", &["build"]);
        }
        CrossToolchain::Docker => {
            ensure_command("docker");
            run_prepare_shell(
                root,
                "docker",
                &docker_prepare_args(
                    root,
                    docker_image("typescript", "node:22-bookworm"),
                    "/repo/clients/ts",
                    "npm ci",
                ),
            );
            run_prepare_shell(
                root,
                "docker",
                &docker_prepare_args(
                    root,
                    docker_image("dart", "dart:stable"),
                    "/repo/clients/dart",
                    "dart pub get",
                ),
            );
            run_prepare_shell(
                root,
                "docker",
                &docker_prepare_args(
                    root,
                    docker_image("java", "eclipse-temurin:17"),
                    "/repo/clients/java",
                    "mkdir -p out && javac --release 17 -d out $(find src -name '*.java')",
                ),
            );
            run_prepare_shell(
                root,
                "docker",
                &docker_prepare_args(
                    root,
                    docker_image("gleam", "ghcr.io/gleam-lang/gleam:v1.12.0-erlang"),
                    "/repo/clients/gleam",
                    "gleam build",
                ),
            );
        }
        CrossToolchain::Nix => {
            ensure_command("nix");
            run_prepare_shell(
                root,
                "nix",
                &nix_args(&["nodejs_22"], "cd clients/ts && npm ci"),
            );
            run_prepare_shell(
                root,
                "nix",
                &nix_args(&["dart"], "cd clients/dart && dart pub get"),
            );
            run_prepare_shell(
                root,
                "nix",
                &nix_args(
                    &["jdk17"],
                    "cd clients/java && mkdir -p out && javac --release 17 -d out $(find src -name '*.java')",
                ),
            );
            run_prepare_shell(
                root,
                "nix",
                &nix_args(
                    &["gleam", "erlang", "rebar3"],
                    "cd clients/gleam && gleam build",
                ),
            );
        }
    }
}

fn external_worker_specs(root: &Path, mode: CrossToolchain) -> Vec<ExternalWorker> {
    match mode {
        CrossToolchain::Local => local_external_worker_specs(root),
        CrossToolchain::Docker => docker_external_worker_specs(root),
        CrossToolchain::Nix => nix_external_worker_specs(root),
    }
}

fn local_external_worker_specs(root: &Path) -> Vec<ExternalWorker> {
    vec![
        ExternalWorker {
            lang: "typescript",
            cwd: root.join("clients/ts"),
            program: "npm".into(),
            args: ["exec", "--", "tsx", "src/cross-language-worker.ts"]
                .into_iter()
                .map(String::from)
                .collect(),
            broker_host: "127.0.0.1".into(),
        },
        ExternalWorker {
            lang: "dart",
            cwd: root.join("clients/dart"),
            program: "dart".into(),
            args: ["run", "bin/cross_language_worker.dart"]
                .into_iter()
                .map(String::from)
                .collect(),
            broker_host: "127.0.0.1".into(),
        },
        ExternalWorker {
            lang: "gleam",
            cwd: root.join("clients/gleam"),
            program: "gleam".into(),
            args: ["run", "-m", "cross_language_worker"]
                .into_iter()
                .map(String::from)
                .collect(),
            broker_host: "127.0.0.1".into(),
        },
        ExternalWorker {
            lang: "java",
            cwd: root.join("clients/java"),
            program: "java".into(),
            args: [
                "-cp",
                "out",
                "com.oresoftware.networkmutex.CrossLanguageWorker",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            broker_host: "127.0.0.1".into(),
        },
    ]
}

fn docker_external_worker_specs(root: &Path) -> Vec<ExternalWorker> {
    let host = std::env::var("LMX_CROSS_LANG_DOCKER_HOST")
        .unwrap_or_else(|_| "host.docker.internal".into());
    vec![
        ExternalWorker {
            lang: "typescript",
            cwd: root.to_path_buf(),
            program: "docker".into(),
            args: docker_run_args(
                root,
                docker_image("typescript", "node:22-bookworm"),
                "/repo/clients/ts",
                "npm exec -- tsx src/cross-language-worker.ts",
            ),
            broker_host: host.clone(),
        },
        ExternalWorker {
            lang: "dart",
            cwd: root.to_path_buf(),
            program: "docker".into(),
            args: docker_run_args(
                root,
                docker_image("dart", "dart:stable"),
                "/repo/clients/dart",
                "dart run bin/cross_language_worker.dart",
            ),
            broker_host: host.clone(),
        },
        ExternalWorker {
            lang: "gleam",
            cwd: root.to_path_buf(),
            program: "docker".into(),
            args: docker_run_args(
                root,
                docker_image("gleam", "ghcr.io/gleam-lang/gleam:v1.12.0-erlang"),
                "/repo/clients/gleam",
                "gleam run -m cross_language_worker",
            ),
            broker_host: host.clone(),
        },
        ExternalWorker {
            lang: "java",
            cwd: root.to_path_buf(),
            program: "docker".into(),
            args: docker_run_args(
                root,
                docker_image("java", "eclipse-temurin:17"),
                "/repo/clients/java",
                "java -cp out com.oresoftware.networkmutex.CrossLanguageWorker",
            ),
            broker_host: host,
        },
    ]
}

fn nix_external_worker_specs(root: &Path) -> Vec<ExternalWorker> {
    vec![
        ExternalWorker {
            lang: "typescript",
            cwd: root.to_path_buf(),
            program: "nix".into(),
            args: nix_args(
                &["nodejs_22"],
                "cd clients/ts && npm exec -- tsx src/cross-language-worker.ts",
            ),
            broker_host: "127.0.0.1".into(),
        },
        ExternalWorker {
            lang: "dart",
            cwd: root.to_path_buf(),
            program: "nix".into(),
            args: nix_args(
                &["dart"],
                "cd clients/dart && dart run bin/cross_language_worker.dart",
            ),
            broker_host: "127.0.0.1".into(),
        },
        ExternalWorker {
            lang: "gleam",
            cwd: root.to_path_buf(),
            program: "nix".into(),
            args: nix_args(
                &["gleam", "erlang", "rebar3"],
                "cd clients/gleam && gleam run -m cross_language_worker",
            ),
            broker_host: "127.0.0.1".into(),
        },
        ExternalWorker {
            lang: "java",
            cwd: root.to_path_buf(),
            program: "nix".into(),
            args: nix_args(
                &["jdk17"],
                "cd clients/java && java -cp out com.oresoftware.networkmutex.CrossLanguageWorker",
            ),
            broker_host: "127.0.0.1".into(),
        },
    ]
}

fn parse_worker_kind(kind: &str) -> HoldKind {
    match kind {
        "exclusive" => HoldKind::Exclusive,
        "read" => HoldKind::Read,
        "write" => HoldKind::Write,
        other => panic!("unknown cross-language worker hold kind `{other}`"),
    }
}

async fn run_external_worker(
    spec: ExternalWorker,
    worker_idx: usize,
    port: u16,
    seed: u64,
    ops: usize,
    key_prefix: String,
    key_count: usize,
    oracle: Arc<Mutex<AsyncOracle>>,
) -> Result<(), String> {
    let worker_id = format!("{}-{worker_idx}", spec.lang);
    let mut child = TokioCommand::new(&spec.program)
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .env("LIVE_MUTEX_HOST", &spec.broker_host)
        .env("LIVE_MUTEX_PORT", port.to_string())
        .env("LMX_WORKER_LANG", spec.lang)
        .env("LMX_WORKER_ID", &worker_id)
        .env("LMX_WORKER_SEED", seed.to_string())
        .env("LMX_WORKER_OPS", ops.to_string())
        .env("LMX_FUZZ_KEY_PREFIX", &key_prefix)
        .env("LMX_FUZZ_KEY_COUNT", key_count.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("spawn {worker_id} failed: {err}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{worker_id}: missing stdout pipe"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("{worker_id}: missing stderr pipe"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("{worker_id}: missing stdin pipe"))?;

    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut buf).await;
        buf
    });

    let mut saw_done = false;
    let mut lines = BufReader::new(stdout).lines();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|err| format!("{worker_id}: stdout read failed: {err}"))?
    {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            continue;
        }
        let event: WorkerEvent = serde_json::from_str(trimmed)
            .map_err(|err| format!("{worker_id}: bad event JSON `{trimmed}`: {err}"))?;
        match event.event.as_str() {
            "grant" => {
                let lock_uuid = event
                    .lock_uuid
                    .clone()
                    .ok_or_else(|| format!("{worker_id}: grant missing lockUuid"))?;
                let kind = event
                    .kind
                    .as_deref()
                    .ok_or_else(|| format!("{worker_id}: grant missing kind"))?;
                oracle.lock().unwrap().on_grant(
                    lock_uuid,
                    parse_worker_kind(kind),
                    event.keys,
                    event.tokens,
                    seed,
                    worker_idx,
                );
                stdin
                    .write_all(b"ack\n")
                    .await
                    .map_err(|err| format!("{worker_id}: ack write failed: {err}"))?;
                stdin
                    .flush()
                    .await
                    .map_err(|err| format!("{worker_id}: ack flush failed: {err}"))?;
            }
            "release" => {
                let lock_uuid = event
                    .lock_uuid
                    .as_deref()
                    .ok_or_else(|| format!("{worker_id}: release missing lockUuid"))?;
                oracle
                    .lock()
                    .unwrap()
                    .on_release(lock_uuid, seed, worker_idx);
                stdin
                    .write_all(b"ack\n")
                    .await
                    .map_err(|err| format!("{worker_id}: ack write failed: {err}"))?;
                stdin
                    .flush()
                    .await
                    .map_err(|err| format!("{worker_id}: ack flush failed: {err}"))?;
            }
            "done" => {
                assert_eq!(
                    event.lang, spec.lang,
                    "{worker_id}: done event reported wrong lang"
                );
                assert_eq!(
                    event.worker, worker_id,
                    "{worker_id}: done event reported wrong worker id"
                );
                assert_eq!(
                    event.ops,
                    Some(ops),
                    "{worker_id}: done event reported wrong op count"
                );
                saw_done = true;
            }
            other => return Err(format!("{worker_id}: unknown event `{other}`")),
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|err| format!("{worker_id}: wait failed: {err}"))?;
    let stderr = stderr_task
        .await
        .map_err(|err| format!("{worker_id}: stderr task failed: {err}"))?;
    if !status.success() {
        return Err(format!(
            "{worker_id}: exited with {status}; stderr:\n{stderr}"
        ));
    }
    if !saw_done {
        return Err(format!("{worker_id}: exited without a done event"));
    }
    Ok(())
}

async fn run_rust_cross_language_worker(
    worker_idx: usize,
    port: u16,
    seed: u64,
    ops: usize,
    key_prefix: String,
    key_count: usize,
    oracle: Arc<Mutex<AsyncOracle>>,
) {
    let client = Client::connect_tcp(("127.0.0.1", port), client_cfg(20_000))
        .await
        .expect("cross-language rust client connect");
    let rw = RwClient::connect_tcp(("127.0.0.1", port), client_cfg(20_000))
        .await
        .expect("cross-language rust rw client connect");
    let keys: Vec<String> = (0..key_count)
        .map(|i| format!("{key_prefix}-{i}"))
        .collect();
    let mut rng = Rng::new(seed);

    for op in 0..ops {
        let roll = rng.below(100);
        match roll {
            0..=29 => {
                let key = keys[rng.below(keys.len())].clone();
                if let Some(guard) = client
                    .try_acquire(&key, Duration::from_millis(60_000))
                    .await
                    .expect("cross-language rust try_acquire")
                {
                    let tokens =
                        lock_tokens_for(&guard.keys, guard.fencing_token, &guard.fencing_tokens);
                    oracle.lock().unwrap().on_grant(
                        guard.lock_uuid.clone(),
                        HoldKind::Exclusive,
                        guard.keys.clone(),
                        tokens,
                        seed,
                        op,
                    );
                    async_hold_briefly(&mut rng).await;
                    oracle
                        .lock()
                        .unwrap()
                        .on_release(&guard.lock_uuid, seed, op);
                    client
                        .release(&guard)
                        .await
                        .expect("cross-language rust release");
                }
            }
            30..=49 => {
                let key = keys[rng.below(keys.len())].clone();
                let guard = client
                    .acquire(&key, Duration::from_millis(60_000))
                    .await
                    .expect("cross-language rust acquire");
                let tokens =
                    lock_tokens_for(&guard.keys, guard.fencing_token, &guard.fencing_tokens);
                oracle.lock().unwrap().on_grant(
                    guard.lock_uuid.clone(),
                    HoldKind::Exclusive,
                    guard.keys.clone(),
                    tokens,
                    seed,
                    op,
                );
                async_hold_briefly(&mut rng).await;
                oracle
                    .lock()
                    .unwrap()
                    .on_release(&guard.lock_uuid, seed, op);
                client
                    .release(&guard)
                    .await
                    .expect("cross-language rust release");
            }
            50..=64 => {
                let chosen = choose_keys(&mut rng, &keys, 2, MAX_COMPOSITE_KEYS.min(keys.len()));
                let refs: Vec<&str> = chosen.iter().map(String::as_str).collect();
                if let Some(guard) = client
                    .try_acquire_composite(&refs, Duration::from_millis(60_000))
                    .await
                    .expect("cross-language rust try_acquire_composite")
                {
                    let tokens =
                        lock_tokens_for(&guard.keys, guard.fencing_token, &guard.fencing_tokens);
                    oracle.lock().unwrap().on_grant(
                        guard.lock_uuid.clone(),
                        HoldKind::Exclusive,
                        guard.keys.clone(),
                        tokens,
                        seed,
                        op,
                    );
                    async_hold_briefly(&mut rng).await;
                    oracle
                        .lock()
                        .unwrap()
                        .on_release(&guard.lock_uuid, seed, op);
                    client
                        .release(&guard)
                        .await
                        .expect("cross-language rust release");
                }
            }
            65..=74 => {
                let chosen = choose_keys(&mut rng, &keys, 2, MAX_COMPOSITE_KEYS.min(keys.len()));
                let refs: Vec<&str> = chosen.iter().map(String::as_str).collect();
                let guard = client
                    .acquire_composite(&refs, Duration::from_millis(60_000))
                    .await
                    .expect("cross-language rust acquire_composite");
                let tokens =
                    lock_tokens_for(&guard.keys, guard.fencing_token, &guard.fencing_tokens);
                oracle.lock().unwrap().on_grant(
                    guard.lock_uuid.clone(),
                    HoldKind::Exclusive,
                    guard.keys.clone(),
                    tokens,
                    seed,
                    op,
                );
                async_hold_briefly(&mut rng).await;
                oracle
                    .lock()
                    .unwrap()
                    .on_release(&guard.lock_uuid, seed, op);
                client
                    .release(&guard)
                    .await
                    .expect("cross-language rust release");
            }
            75..=91 => {
                let key = keys[rng.below(keys.len())].clone();
                let guard = rw
                    .acquire_read(&key)
                    .await
                    .expect("cross-language rust read");
                let mut tokens = BTreeMap::new();
                if let Some(token) = guard.fencing_token {
                    tokens.insert(guard.key.clone(), token);
                }
                oracle.lock().unwrap().on_grant(
                    guard.lock_uuid.clone(),
                    HoldKind::Read,
                    vec![guard.key.clone()],
                    tokens,
                    seed,
                    op,
                );
                async_hold_briefly(&mut rng).await;
                let lock_uuid = guard.lock_uuid.clone();
                oracle.lock().unwrap().on_release(&lock_uuid, seed, op);
                guard
                    .release()
                    .await
                    .expect("cross-language rust read release");
            }
            _ => {
                let key = keys[rng.below(keys.len())].clone();
                let guard = rw
                    .acquire_write(&key)
                    .await
                    .expect("cross-language rust write");
                let mut tokens = BTreeMap::new();
                if let Some(token) = guard.fencing_token {
                    tokens.insert(guard.key.clone(), token);
                }
                oracle.lock().unwrap().on_grant(
                    guard.lock_uuid.clone(),
                    HoldKind::Write,
                    vec![guard.key.clone()],
                    tokens,
                    seed,
                    op,
                );
                async_hold_briefly(&mut rng).await;
                let lock_uuid = guard.lock_uuid.clone();
                oracle.lock().unwrap().on_release(&lock_uuid, seed, op);
                guard
                    .release()
                    .await
                    .expect("cross-language rust write release");
            }
        }
    }

    let _ = worker_idx;
}

async fn run_cross_language_seed(seed: u64, ops: usize) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mode = cross_toolchain_mode(&root);
    prepare_cross_language_workers(&root, mode);

    let bind_host = if mode == CrossToolchain::Docker {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    };
    let (port, server) = start_tcp_broker_on(bind_host).await;
    let oracle = Arc::new(Mutex::new(AsyncOracle::default()));
    let key_prefix = format!("cross-rop-{seed}");
    let key_count = 5usize;
    let ops_per_worker = (ops / 12).max(1);
    let mut tasks = JoinSet::new();

    for rust_worker in 0..3 {
        let oracle = Arc::clone(&oracle);
        let key_prefix = key_prefix.clone();
        tasks.spawn(async move {
            run_rust_cross_language_worker(
                rust_worker,
                port,
                seed.wrapping_add(0x5255_5354 + rust_worker as u64),
                ops_per_worker,
                key_prefix,
                key_count,
                oracle,
            )
            .await;
            Ok::<(), String>(())
        });
    }

    let external_plan = [
        ("typescript", 3usize),
        ("dart", 2usize),
        ("gleam", 2usize),
        ("java", 2usize),
    ];
    let specs = external_worker_specs(&root, mode);
    let mut worker_idx = 3usize;
    for (lang, count) in external_plan {
        let spec = specs
            .iter()
            .find(|s| s.lang == lang)
            .unwrap_or_else(|| panic!("missing external worker spec for {lang}"))
            .clone();
        for _ in 0..count {
            let oracle = Arc::clone(&oracle);
            let key_prefix = key_prefix.clone();
            let spec = spec.clone();
            let seed = seed.wrapping_add(0xC045_5000 + worker_idx as u64 * 7919);
            let idx = worker_idx;
            tasks.spawn(async move {
                run_external_worker(
                    spec,
                    idx,
                    port,
                    seed,
                    ops_per_worker,
                    key_prefix,
                    key_count,
                    oracle,
                )
                .await
            });
            worker_idx += 1;
        }
    }
    assert_eq!(
        worker_idx, 12,
        "cross-language worker plan must use 12 workers"
    );

    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(err)) => panic!("cross-language worker failed: {err}"),
            Err(err) => panic!("cross-language worker task panicked: {err}"),
        }
    }

    oracle.lock().unwrap().assert_clean();
    let inspector = Client::connect_tcp(("127.0.0.1", port), client_cfg(5_000))
        .await
        .expect("cross-language inspector connect");
    for i in 0..key_count {
        let key = format!("{key_prefix}-{i}");
        let info = inspector
            .lock_info(&key)
            .await
            .expect("cross-language final lock_info");
        assert!(
            !info.is_locked,
            "cross-language fuzz leaked key={key}: final lock_info={info:?}"
        );
        assert_eq!(
            info.lock_request_count, 0,
            "cross-language fuzz leaked waiters for key={key}: final lock_info={info:?}"
        );
    }
    server.abort();
    let _ = server.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Node/TypeScript, Dart, Gleam/Erlang, and Java toolchains"]
async fn cross_language_tcp_random_operations_reject_invalid_states() {
    run_cross_language_seed(
        seed_for().wrapping_add(0xC405_5A11_C405_5A11),
        cross_lang_ops_for(),
    )
    .await;
}
