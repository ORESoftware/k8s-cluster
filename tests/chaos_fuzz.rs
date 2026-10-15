//! Chaos + fuzz invariant suite for the Rust broker.
//!
//! Drives the broker through randomized sequences of every public client
//! operation — exclusive locks, semaphores, RW reads/writes, composite
//! (multi-key) acquires — interleaved with random disconnects, holder
//! drops, and TTL pressure. Each test asserts a global invariant
//! (exclusion, semaphore cap, fencing monotonicity, composite atomicity,
//! recovery after drops) over the full run.
//!
//! Determinism: every test uses a tiny xorshift64 PRNG with a seed taken
//! from the `LMX_FUZZ_SEED` env var (or a per-test default). Failed
//! assertions print the seed so the run is reproducible. The default
//! seed is fixed so the suite is repeatable in CI.
//!
//! Why a fresh test file rather than additions to `integration.rs`:
//! `integration.rs` already pushes well past 1.7k lines, and these
//! tests need their own seeded-RNG harness, an oracle that tracks every
//! grant/release, and helpers that don't fit the per-test pattern of
//! the existing file. Keeping them isolated also lets us run *just* the
//! chaos suite via `cargo test --test chaos_fuzz`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dd_rust_network_mutex::{
    server::{run as run_server, ServerConfig},
    BrokerConfig, Client, ClientConfig, RwClient,
};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::task::JoinSet;

type FencingTokenStream = HashMap<String, Vec<(std::time::Instant, u64)>>;

// =========================================================================
// PRNG + seed helpers
// =========================================================================

/// xorshift64* — minimal seedable PRNG. We only need uniform-ish u64s for
/// "pick op", "pick key", "pick client" decisions; high-quality
/// statistics are unnecessary.
#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // Avoid the all-zeros fixed-point by xor-mixing with a constant.
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn gen_range(&mut self, lo: usize, hi_exclusive: usize) -> usize {
        debug_assert!(hi_exclusive > lo);
        lo + (self.next_u64() as usize) % (hi_exclusive - lo)
    }
    /// Sample a u32 in [0, n). Used for picking number of keys, max, etc.
    fn gen_u32(&mut self, n: u32) -> u32 {
        debug_assert!(n > 0);
        (self.next_u64() as u32) % n
    }
    /// Returns true with probability `pct/100`.
    fn pct(&mut self, pct: u32) -> bool {
        debug_assert!(pct <= 100);
        (self.next_u64() % 100) < pct as u64
    }
}

fn seed_for(test_name: &str, default_seed: u64) -> u64 {
    if let Ok(s) = std::env::var("LMX_FUZZ_SEED") {
        if let Ok(n) = s.parse() {
            eprintln!("[{test_name}] using LMX_FUZZ_SEED={n}");
            return n;
        }
    }
    eprintln!(
        "[{test_name}] using default seed {default_seed} (override with LMX_FUZZ_SEED=<u64>)",
    );
    default_seed
}

// =========================================================================
// Server harness
// =========================================================================

/// Bind an ephemeral 127.0.0.1 port without keeping the listener.
async fn pick_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Spawn a broker on an ephemeral TCP port. Returns the port and a
/// shutdown-trigger channel. Caller should `tokio::spawn` the returned
/// future and drop the trigger to stop the broker — though tests can
/// rely on test teardown to abort tasks.
async fn start_broker(broker_cfg: BrokerConfig) -> u16 {
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
    tokio::spawn(async move {
        if let Err(err) = run_server(config).await {
            eprintln!("broker exited: {err:?}");
        }
    });
    // Give the listener a moment to bind. 50ms matches `integration.rs`.
    tokio::time::sleep(Duration::from_millis(50)).await;
    port
}

fn client_cfg(timeout_ms: u64) -> ClientConfig {
    ClientConfig {
        auth_token: None,
        default_request_timeout: Duration::from_millis(timeout_ms),
    }
}

// =========================================================================
// Oracle — global invariant tracker
// =========================================================================

/// Shared oracle that records every observed grant/release across all
/// clients and asserts global invariants live (during the run, not
/// after). Each test owns one `Oracle`; clients post events to it.
#[derive(Default)]
struct Oracle {
    /// For each key, the set of currently-active *single-key* exclusive
    /// holders' lock_uuids. With single-key exclusive, this set must
    /// have size <= 1 unless `max > 1` is configured (semaphore mode).
    exclusive_holders: HashMap<String, HashSet<String>>,
    /// For each key, the active read-lock holders.
    read_holders: HashMap<String, HashSet<String>>,
    /// For each key, the active write-lock holder (RW writer).
    write_holders: HashMap<String, HashSet<String>>,
    /// For each key, the active composite-lock uuids (each composite uuid
    /// can appear under multiple keys).
    composite_holders: HashMap<String, HashSet<String>>,
    /// Per-key ceiling on simultaneous semaphore holders. `1` for plain
    /// exclusive locks; >1 for semaphore tests.
    max_per_key: HashMap<String, u32>,
    /// Set of fencing tokens observed for each key. Across the whole
    /// run, no two grants on the same key may share a token. We do NOT
    /// check broker-mint order here — multiple async tasks record into
    /// the oracle in receive-order, which doesn't preserve the broker's
    /// internal grant order, so a "strictly monotonic" check would
    /// produce false positives. Strict monotonicity is verified
    /// separately in `fuzz_fencing_strictly_monotonic_per_key`, which
    /// sorts by client-side observation time on a workload that's
    /// effectively sequential per key (exclusive lock → serialised).
    seen_fencing_per_key: HashMap<String, HashSet<u64>>,
    /// Count of invariant violations recorded. Tests assert == 0 at end.
    violations: Vec<String>,
}

impl Oracle {
    fn new() -> Self {
        Self::default()
    }

    fn set_max(&mut self, key: &str, max: u32) {
        self.max_per_key.insert(key.to_string(), max);
    }

    fn assert_fencing_unique(&mut self, key: &str, token: u64) {
        let set = self
            .seen_fencing_per_key
            .entry(key.to_string())
            .or_default();
        if !set.insert(token) {
            self.violations
                .push(format!("duplicate-fencing-token key={key} token={token}",));
        }
    }

    fn record_exclusive_grant(&mut self, key: &str, lock_uuid: &str, fencing: Option<u64>) {
        let holders = self.exclusive_holders.entry(key.to_string()).or_default();
        holders.insert(lock_uuid.to_string());
        let max = *self.max_per_key.get(key).unwrap_or(&1);
        // Cross-check: no overlap with RW writer / readers / composite.
        if let Some(w) = self.write_holders.get(key) {
            if !w.is_empty() {
                self.violations
                    .push(format!("exclusive-while-rw-writer key={key} writers={w:?}",));
            }
        }
        if let Some(r) = self.read_holders.get(key) {
            if !r.is_empty() {
                self.violations.push(format!(
                    "exclusive-while-rw-readers key={key} readers={r:?}",
                ));
            }
        }
        if let Some(c) = self.composite_holders.get(key) {
            if !c.is_empty() {
                self.violations.push(format!(
                    "exclusive-while-composite key={key} composites={c:?}",
                ));
            }
        }
        if (holders.len() as u32) > max {
            self.violations.push(format!(
                "semaphore-cap-exceeded key={key} count={} max={max}",
                holders.len(),
            ));
        }
        if let Some(t) = fencing {
            self.assert_fencing_unique(key, t);
        }
    }

    fn record_exclusive_release(&mut self, key: &str, lock_uuid: &str) {
        if let Some(set) = self.exclusive_holders.get_mut(key) {
            set.remove(lock_uuid);
        }
    }

    fn record_read_grant(&mut self, key: &str, lock_uuid: &str, fencing: Option<u64>) {
        if let Some(w) = self.write_holders.get(key) {
            if !w.is_empty() {
                self.violations
                    .push(format!("rw-read-while-writer key={key} writers={w:?}",));
            }
        }
        if let Some(e) = self.exclusive_holders.get(key) {
            if !e.is_empty() {
                self.violations
                    .push(format!("rw-read-while-exclusive key={key} excl={e:?}",));
            }
        }
        self.read_holders
            .entry(key.to_string())
            .or_default()
            .insert(lock_uuid.to_string());
        if let Some(t) = fencing {
            self.assert_fencing_unique(key, t);
        }
    }

    fn record_read_release(&mut self, key: &str, lock_uuid: &str) {
        if let Some(set) = self.read_holders.get_mut(key) {
            set.remove(lock_uuid);
        }
    }

    fn record_write_grant(&mut self, key: &str, lock_uuid: &str, fencing: Option<u64>) {
        if let Some(r) = self.read_holders.get(key) {
            if !r.is_empty() {
                self.violations
                    .push(format!("rw-write-while-readers key={key} readers={r:?}",));
            }
        }
        if let Some(w) = self.write_holders.get(key) {
            if !w.is_empty() {
                self.violations
                    .push(format!("rw-write-while-writer key={key} writers={w:?}",));
            }
        }
        if let Some(e) = self.exclusive_holders.get(key) {
            if !e.is_empty() {
                self.violations
                    .push(format!("rw-write-while-exclusive key={key} excl={e:?}",));
            }
        }
        self.write_holders
            .entry(key.to_string())
            .or_default()
            .insert(lock_uuid.to_string());
        if let Some(t) = fencing {
            self.assert_fencing_unique(key, t);
        }
    }

    fn record_write_release(&mut self, key: &str, lock_uuid: &str) {
        if let Some(set) = self.write_holders.get_mut(key) {
            set.remove(lock_uuid);
        }
    }

    fn record_composite_grant(
        &mut self,
        keys: &[String],
        lock_uuid: &str,
        fencings: &BTreeMap<String, u64>,
    ) {
        for key in keys {
            // Composite holders must not overlap with any exclusive,
            // reader, writer, or another composite on the same key.
            if let Some(e) = self.exclusive_holders.get(key) {
                if !e.is_empty() {
                    self.violations
                        .push(format!("composite-while-exclusive key={key} excl={e:?}",));
                }
            }
            if let Some(r) = self.read_holders.get(key) {
                if !r.is_empty() {
                    self.violations
                        .push(format!("composite-while-readers key={key} readers={r:?}",));
                }
            }
            if let Some(w) = self.write_holders.get(key) {
                if !w.is_empty() {
                    self.violations
                        .push(format!("composite-while-writer key={key} writers={w:?}",));
                }
            }
            if let Some(c) = self.composite_holders.get(key) {
                if !c.is_empty() {
                    self.violations
                        .push(format!("composite-while-composite key={key} others={c:?}",));
                }
            }
            self.composite_holders
                .entry(key.clone())
                .or_default()
                .insert(lock_uuid.to_string());
            if let Some(t) = fencings.get(key) {
                self.assert_fencing_unique(key, *t);
            }
        }
    }

    fn record_composite_release(&mut self, keys: &[String], lock_uuid: &str) {
        for key in keys {
            if let Some(set) = self.composite_holders.get_mut(key) {
                set.remove(lock_uuid);
            }
        }
    }

    fn assert_clean(&self) -> Result<(), String> {
        if !self.violations.is_empty() {
            return Err(format!(
                "{} invariant violation(s): {:?}",
                self.violations.len(),
                self.violations,
            ));
        }
        for (k, set) in &self.exclusive_holders {
            if !set.is_empty() {
                return Err(format!("leaked exclusive holders on key={k}: {set:?}"));
            }
        }
        for (k, set) in &self.read_holders {
            if !set.is_empty() {
                return Err(format!("leaked readers on key={k}: {set:?}"));
            }
        }
        for (k, set) in &self.write_holders {
            if !set.is_empty() {
                return Err(format!("leaked writers on key={k}: {set:?}"));
            }
        }
        for (k, set) in &self.composite_holders {
            if !set.is_empty() {
                return Err(format!("leaked composites on key={k}: {set:?}"));
            }
        }
        Ok(())
    }
}

// =========================================================================
// 1. Single-key exclusive: no two holders ever simultaneously
// =========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fuzz_exclusive_no_double_grant() {
    let seed = seed_for("fuzz_exclusive_no_double_grant", 0xCAFE_F00D);
    let port = start_broker(BrokerConfig::default()).await;
    let oracle = Arc::new(Mutex::new(Oracle::new()));

    let key_count = 4;
    let client_count = 12;
    let ops_per_client = 30;
    let keys: Vec<String> = (0..key_count).map(|i| format!("excl-key-{i}")).collect();
    for k in &keys {
        oracle.lock().await.set_max(k, 1);
    }

    let mut joinset = JoinSet::new();
    for client_idx in 0..client_count {
        let oracle = oracle.clone();
        let keys = keys.clone();
        let mut my_rng = Rng::new(seed.wrapping_add(client_idx as u64 * 0xABCD));
        joinset.spawn(async move {
            let client = Client::connect_tcp(("127.0.0.1", port), client_cfg(8_000))
                .await
                .unwrap();
            for _ in 0..ops_per_client {
                let key = &keys[my_rng.gen_range(0, keys.len())];
                let ttl = Duration::from_millis(2_000 + my_rng.gen_u32(2_000) as u64);
                match client.acquire(key, ttl).await {
                    Ok(guard) => {
                        let token = guard.fencing_token;
                        oracle
                            .lock()
                            .await
                            .record_exclusive_grant(key, &guard.lock_uuid, token);
                        // Simulate some held-time so contention is real.
                        let hold = my_rng.gen_u32(3) as u64;
                        if hold > 0 {
                            tokio::time::sleep(Duration::from_millis(hold)).await;
                        }
                        oracle
                            .lock()
                            .await
                            .record_exclusive_release(key, &guard.lock_uuid);
                        client.release(&guard).await.unwrap();
                    }
                    Err(err) => {
                        // Timeouts are tolerated under heavy contention with
                        // a tight 8s default; the invariant of "no double
                        // grant" still holds.
                        eprintln!("[seed={seed}] client {client_idx} acquire failed: {err}");
                    }
                }
            }
        });
    }
    while let Some(r) = joinset.join_next().await {
        r.unwrap();
    }

    let result = oracle.lock().await.assert_clean();
    if let Err(e) = result {
        panic!("[seed={seed}] {e}");
    }
}

// =========================================================================
// 2. Semaphore: holder count never exceeds max under heavy contention
// =========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fuzz_semaphore_cap_invariant() {
    let seed = seed_for("fuzz_semaphore_cap_invariant", 0x0BAD_CAFE);
    let port = start_broker(BrokerConfig::default()).await;
    let oracle = Arc::new(Mutex::new(Oracle::new()));

    let key = "sem-key";
    let max: u32 = 5;
    oracle.lock().await.set_max(key, max);
    let client_count = 25;
    let ops_per_client = 25;

    let mut joinset = JoinSet::new();
    for client_idx in 0..client_count {
        let oracle = oracle.clone();
        let mut my_rng = Rng::new(seed.wrapping_add(client_idx as u64 * 0x55AA));
        joinset.spawn(async move {
            let client = Client::connect_tcp(("127.0.0.1", port), client_cfg(10_000))
                .await
                .unwrap();
            for _ in 0..ops_per_client {
                let ttl = Duration::from_millis(1_500 + my_rng.gen_u32(1_500) as u64);
                match client.acquire_with_max(key, max, ttl).await {
                    Ok(guard) => {
                        oracle.lock().await.record_exclusive_grant(
                            key,
                            &guard.lock_uuid,
                            guard.fencing_token,
                        );
                        // Hold long enough that >max concurrent acquires
                        // would force a violation if cap were broken.
                        let hold = my_rng.gen_u32(5) as u64;
                        if hold > 0 {
                            tokio::time::sleep(Duration::from_millis(hold)).await;
                        }
                        oracle
                            .lock()
                            .await
                            .record_exclusive_release(key, &guard.lock_uuid);
                        client.release(&guard).await.unwrap();
                    }
                    Err(err) => {
                        eprintln!("[seed={seed}] semaphore acquire failed: {err}");
                    }
                }
            }
        });
    }
    while let Some(r) = joinset.join_next().await {
        r.unwrap();
    }

    let result = oracle.lock().await.assert_clean();
    if let Err(e) = result {
        panic!("[seed={seed}] {e}");
    }
}

// =========================================================================
// 3. RW lock: classic readers-writer exclusion under random workload
// =========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fuzz_rw_lock_safety() {
    let seed = seed_for("fuzz_rw_lock_safety", 0xDEAD_BEEF);
    let port = start_broker(BrokerConfig::default()).await;
    let oracle = Arc::new(Mutex::new(Oracle::new()));

    let keys: Vec<String> = (0..3).map(|i| format!("rw-key-{i}")).collect();
    let client_count = 20;
    let ops_per_client = 25;

    let mut joinset = JoinSet::new();
    for client_idx in 0..client_count {
        let oracle = oracle.clone();
        let keys = keys.clone();
        let mut my_rng = Rng::new(seed.wrapping_add(client_idx as u64 * 0x9999));
        joinset.spawn(async move {
            let rw = RwClient::connect_tcp(("127.0.0.1", port), client_cfg(10_000))
                .await
                .unwrap();
            for _ in 0..ops_per_client {
                let key = &keys[my_rng.gen_range(0, keys.len())];
                // 70% reads, 30% writes — typical RW workload.
                if my_rng.pct(70) {
                    match rw.acquire_read(key).await {
                        Ok(g) => {
                            let lock_uuid = g.lock_uuid.clone();
                            oracle
                                .lock()
                                .await
                                .record_read_grant(key, &lock_uuid, g.fencing_token);
                            let hold = my_rng.gen_u32(4) as u64;
                            if hold > 0 {
                                tokio::time::sleep(Duration::from_millis(hold)).await;
                            }
                            oracle.lock().await.record_read_release(key, &lock_uuid);
                            g.release().await.unwrap();
                        }
                        Err(e) => eprintln!("[seed={seed}] read failed: {e}"),
                    }
                } else {
                    match rw.acquire_write(key).await {
                        Ok(g) => {
                            let lock_uuid = g.lock_uuid.clone();
                            oracle.lock().await.record_write_grant(
                                key,
                                &lock_uuid,
                                g.fencing_token,
                            );
                            let hold = my_rng.gen_u32(4) as u64;
                            if hold > 0 {
                                tokio::time::sleep(Duration::from_millis(hold)).await;
                            }
                            oracle.lock().await.record_write_release(key, &lock_uuid);
                            g.release().await.unwrap();
                        }
                        Err(e) => eprintln!("[seed={seed}] write failed: {e}"),
                    }
                }
            }
        });
    }
    while let Some(r) = joinset.join_next().await {
        r.unwrap();
    }

    let result = oracle.lock().await.assert_clean();
    if let Err(e) = result {
        panic!("[seed={seed}] {e}");
    }
}

// =========================================================================
// 4. Composite atomicity + union semantics under heavy contention
// =========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fuzz_composite_atomicity() {
    let seed = seed_for("fuzz_composite_atomicity", 0xF00D_BABE);
    let port = start_broker(BrokerConfig::default()).await;
    let oracle = Arc::new(Mutex::new(Oracle::new()));

    let keys: Vec<String> = (0..6).map(|i| format!("c-key-{i}")).collect();
    for k in &keys {
        oracle.lock().await.set_max(k, 1);
    }
    let client_count = 12;
    let ops_per_client = 25;

    let mut joinset = JoinSet::new();
    for client_idx in 0..client_count {
        let oracle = oracle.clone();
        let keys = keys.clone();
        let mut my_rng = Rng::new(seed.wrapping_add(client_idx as u64 * 0x4242));
        joinset.spawn(async move {
            let client = Client::connect_tcp(("127.0.0.1", port), client_cfg(15_000))
                .await
                .unwrap();
            for _ in 0..ops_per_client {
                // Pick 1..=5 keys (broker max is 5).
                let n = (my_rng.gen_range(1, 6)).min(keys.len());
                let mut chosen: BTreeSet<usize> = BTreeSet::new();
                while chosen.len() < n {
                    chosen.insert(my_rng.gen_range(0, keys.len()));
                }
                let chosen_keys: Vec<String> = chosen.iter().map(|i| keys[*i].clone()).collect();
                let chosen_refs: Vec<&str> = chosen_keys.iter().map(|s| s.as_str()).collect();
                let ttl = Duration::from_millis(2_500 + my_rng.gen_u32(1_500) as u64);
                match client.acquire_composite(&chosen_refs, ttl).await {
                    Ok(guard) => {
                        oracle.lock().await.record_composite_grant(
                            &chosen_keys,
                            &guard.lock_uuid,
                            &guard.fencing_tokens,
                        );
                        let hold = my_rng.gen_u32(4) as u64;
                        if hold > 0 {
                            tokio::time::sleep(Duration::from_millis(hold)).await;
                        }
                        oracle
                            .lock()
                            .await
                            .record_composite_release(&chosen_keys, &guard.lock_uuid);
                        client.release(&guard).await.unwrap();
                    }
                    Err(e) => {
                        eprintln!("[seed={seed}] composite failed: {e}");
                    }
                }
            }
        });
    }
    while let Some(r) = joinset.join_next().await {
        r.unwrap();
    }

    let result = oracle.lock().await.assert_clean();
    if let Err(e) = result {
        panic!("[seed={seed}] {e}");
    }
}

// =========================================================================
// 5. Mixed workload: exclusive + RW + composite simultaneously on shared keyspace
// =========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fuzz_mixed_workload_invariants() {
    let seed = seed_for("fuzz_mixed_workload_invariants", 0xABCD_EF12);
    let port = start_broker(BrokerConfig::default()).await;
    let oracle = Arc::new(Mutex::new(Oracle::new()));

    let keys: Vec<String> = (0..4).map(|i| format!("mix-key-{i}")).collect();
    for k in &keys {
        oracle.lock().await.set_max(k, 1);
    }
    let client_count = 16;
    let ops_per_client = 25;

    let mut joinset = JoinSet::new();
    for client_idx in 0..client_count {
        let oracle = oracle.clone();
        let keys = keys.clone();
        let mut my_rng = Rng::new(seed.wrapping_add(client_idx as u64 * 0x1357));
        joinset.spawn(async move {
            let excl = Client::connect_tcp(("127.0.0.1", port), client_cfg(15_000))
                .await
                .unwrap();
            let rw = RwClient::connect_tcp(("127.0.0.1", port), client_cfg(15_000))
                .await
                .unwrap();
            for _ in 0..ops_per_client {
                let r = my_rng.gen_u32(100);
                let key = &keys[my_rng.gen_range(0, keys.len())];
                let ttl = Duration::from_millis(2_000 + my_rng.gen_u32(1_500) as u64);
                if r < 35 {
                    if let Ok(g) = excl.acquire(key, ttl).await {
                        oracle.lock().await.record_exclusive_grant(
                            key,
                            &g.lock_uuid,
                            g.fencing_token,
                        );
                        oracle
                            .lock()
                            .await
                            .record_exclusive_release(key, &g.lock_uuid);
                        excl.release(&g).await.ok();
                    }
                } else if r < 65 {
                    if let Ok(g) = rw.acquire_read(key).await {
                        let id = g.lock_uuid.clone();
                        oracle
                            .lock()
                            .await
                            .record_read_grant(key, &id, g.fencing_token);
                        oracle.lock().await.record_read_release(key, &id);
                        g.release().await.ok();
                    }
                } else if r < 85 {
                    if let Ok(g) = rw.acquire_write(key).await {
                        let id = g.lock_uuid.clone();
                        oracle
                            .lock()
                            .await
                            .record_write_grant(key, &id, g.fencing_token);
                        oracle.lock().await.record_write_release(key, &id);
                        g.release().await.ok();
                    }
                } else {
                    let n = my_rng.gen_range(2, 5).min(keys.len());
                    let mut chosen = BTreeSet::new();
                    while chosen.len() < n {
                        chosen.insert(my_rng.gen_range(0, keys.len()));
                    }
                    let ck: Vec<String> = chosen.iter().map(|i| keys[*i].clone()).collect();
                    let cr: Vec<&str> = ck.iter().map(|s| s.as_str()).collect();
                    if let Ok(g) = excl.acquire_composite(&cr, ttl).await {
                        oracle.lock().await.record_composite_grant(
                            &ck,
                            &g.lock_uuid,
                            &g.fencing_tokens,
                        );
                        oracle
                            .lock()
                            .await
                            .record_composite_release(&ck, &g.lock_uuid);
                        excl.release(&g).await.ok();
                    }
                }
            }
        });
    }
    while let Some(r) = joinset.join_next().await {
        r.unwrap();
    }

    let result = oracle.lock().await.assert_clean();
    if let Err(e) = result {
        panic!("[seed={seed}] {e}");
    }
}

// =========================================================================
// 6. Fencing tokens: strictly monotonic per key across many acquire-release cycles
// =========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fuzz_fencing_strictly_monotonic_per_key() {
    let seed = seed_for("fuzz_fencing_strictly_monotonic_per_key", 0x1357_9BDF);
    let port = start_broker(BrokerConfig::default()).await;

    let keys: Vec<String> = (0..3).map(|i| format!("fence-key-{i}")).collect();
    let client_count = 12;
    let cycles_per_client = 30;
    // Per-key collected token streams — flatten and assert strict mono.
    let collected: Arc<Mutex<FencingTokenStream>> = Arc::new(Mutex::new(HashMap::new()));

    let mut joinset = JoinSet::new();
    for client_idx in 0..client_count {
        let collected = collected.clone();
        let keys = keys.clone();
        let mut my_rng = Rng::new(seed.wrapping_add(client_idx as u64 * 0x0F0F));
        joinset.spawn(async move {
            let c = Client::connect_tcp(("127.0.0.1", port), client_cfg(15_000))
                .await
                .unwrap();
            for _ in 0..cycles_per_client {
                let key = &keys[my_rng.gen_range(0, keys.len())];
                let g = c.acquire(key, Duration::from_millis(2_000)).await.unwrap();
                let token = g.fencing_token.expect("single-key grant must carry token");
                let now = std::time::Instant::now();
                collected
                    .lock()
                    .await
                    .entry(key.clone())
                    .or_default()
                    .push((now, token));
                c.release(&g).await.unwrap();
            }
        });
    }
    while let Some(r) = joinset.join_next().await {
        r.unwrap();
    }

    let collected = collected.lock().await;
    for (key, mut series) in collected.clone().into_iter() {
        // Sort by wall-clock observation order. The broker hands tokens
        // out in grant order, which is also the order in which holders
        // existed; sorting here normalizes for the racing inserts above.
        series.sort_by_key(|(t, _)| *t);
        let mut prev: u64 = 0;
        for (i, (_, token)) in series.iter().enumerate() {
            assert!(
                *token > prev,
                "[seed={seed}] non-monotonic fencing on key={key} at obs {i}: prev={prev}, got={token}",
            );
            prev = *token;
        }
    }
}

// =========================================================================
// 7. Chaos: random client drops mid-flight; broker must recover, no leaks
// =========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chaos_random_drops_recover_to_clean_state() {
    let seed = seed_for("chaos_random_drops_recover_to_clean_state", 0xBEEF_CAFE);
    let port = start_broker(BrokerConfig::default()).await;

    let keys: Vec<String> = (0..4).map(|i| format!("chaos-key-{i}")).collect();
    let client_count = 24;
    let ops_per_client = 12;

    // Stop signal — flipped after spawning to give all clients a chance
    // to start their workload before some of them are dropped.
    let stop = Arc::new(AtomicBool::new(false));

    let mut joinset = JoinSet::new();
    for client_idx in 0..client_count {
        let stop = stop.clone();
        let keys = keys.clone();
        let mut my_rng = Rng::new(seed.wrapping_add(client_idx as u64 * 0xBEEF));
        joinset.spawn(async move {
            // Some clients are flagged to be dropped mid-flight: they
            // simply return early once `stop` is set, which causes
            // `Client` to drop and the broker to release their state.
            let drop_me = my_rng.pct(40);
            let c = Client::connect_tcp(("127.0.0.1", port), client_cfg(8_000))
                .await
                .unwrap();
            for _ in 0..ops_per_client {
                if drop_me && stop.load(Ordering::Relaxed) {
                    // Walk away; Client drop on scope exit triggers
                    // broker `drop_client(client_id)`.
                    return;
                }
                let key = &keys[my_rng.gen_range(0, keys.len())];
                let r = my_rng.gen_u32(100);
                if r < 50 {
                    if let Ok(g) = c.acquire(key, Duration::from_millis(1_500)).await {
                        // Half the time release; half the time abandon
                        // the guard so the holder must be reclaimed via
                        // disconnect or TTL on the broker side.
                        if my_rng.pct(50) {
                            c.release(&g).await.ok();
                        }
                    }
                } else if r < 75 {
                    if let Ok(g) = c
                        .acquire_with_max(key, 4, Duration::from_millis(1_500))
                        .await
                    {
                        if my_rng.pct(50) {
                            c.release(&g).await.ok();
                        }
                    }
                } else {
                    let n = my_rng.gen_range(2, 5);
                    let pool: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
                    let mut chosen = BTreeSet::new();
                    while chosen.len() < n {
                        chosen.insert(my_rng.gen_range(0, pool.len()));
                    }
                    let ck: Vec<&str> = chosen.iter().map(|i| pool[*i]).collect();
                    if let Ok(g) = c.acquire_composite(&ck, Duration::from_millis(1_500)).await {
                        if my_rng.pct(50) {
                            c.release(&g).await.ok();
                        }
                    }
                }
            }
        });
    }

    // Trigger drops 200ms in.
    tokio::time::sleep(Duration::from_millis(200)).await;
    stop.store(true, Ordering::Relaxed);

    // Wait for all clients to finish (those that didn't drop will
    // complete their workload; those that did will return early).
    while let Some(r) = joinset.join_next().await {
        r.unwrap();
    }

    // Allow broker a beat to process disconnect cleanup and any TTL
    // expirations from leaked holders. With BrokerConfig::default(),
    // ttl is 4s, but holders we abandoned have explicit ttl 1500ms;
    // a 2.5s sleep is a comfortable margin.
    tokio::time::sleep(Duration::from_millis(2_500)).await;

    // Probe: a fresh client must be able to immediately acquire every
    // key — otherwise something's leaked.
    let probe = Client::connect_tcp(("127.0.0.1", port), client_cfg(5_000))
        .await
        .unwrap();
    for key in &keys {
        let g = probe
            .acquire(key, Duration::from_millis(500))
            .await
            .unwrap_or_else(|e| panic!("[seed={seed}] probe acquire {key} failed: {e}"));
        probe.release(&g).await.unwrap();
    }
}

// =========================================================================
// 8. Chaos: composite partial-grant + disconnect must not leak any keys
// =========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chaos_composite_partial_grant_drops_no_leak() {
    let seed = seed_for("chaos_composite_partial_grant_drops_no_leak", 0xFADE_C0DE);
    let port = start_broker(BrokerConfig::default()).await;
    let keys: Vec<String> = (0..5).map(|i| format!("part-key-{i}")).collect();

    // Stage 1: a long-lived blocker holds key-2 so any composite that
    // includes it will only get a partial grant before getting stuck.
    let blocker = Client::connect_tcp(("127.0.0.1", port), client_cfg(5_000))
        .await
        .unwrap();
    let block_guard = blocker
        .acquire(&keys[2], Duration::from_millis(60_000))
        .await
        .unwrap();

    // Stage 2: spawn N clients each requesting a 4-key composite that
    // *includes* key-2. They all stall partway through. Then we
    // abruptly drop them by aborting their tasks.
    let mut joinset = JoinSet::new();
    let n = 12;
    for client_idx in 0..n {
        let p = port;
        let mut my_rng = Rng::new(seed.wrapping_add(client_idx as u64));
        joinset.spawn(async move {
            let c = Client::connect_tcp(("127.0.0.1", p), client_cfg(60_000))
                .await
                .unwrap();
            // Choose a 3-4 key composite that always includes key-2.
            let n = my_rng.gen_range(3, 5);
            let mut idx = BTreeSet::new();
            idx.insert(2usize);
            while idx.len() < n {
                idx.insert(my_rng.gen_range(0, 5));
            }
            let strs: Vec<String> = idx.iter().map(|i| format!("part-key-{i}")).collect();
            let refs: Vec<&str> = strs.iter().map(|s| s.as_str()).collect();
            // This will block — the inner await is what we want
            // interrupted by the JoinHandle abort below.
            let _ = c
                .acquire_composite(&refs, Duration::from_millis(60_000))
                .await;
        });
    }

    tokio::time::sleep(Duration::from_millis(300)).await;
    joinset.abort_all();
    while let Some(r) = joinset.join_next().await {
        // Ignore JoinError::Cancelled — these tasks are intentionally
        // aborted above. Any panic would still surface here.
        if let Err(e) = r {
            assert!(e.is_cancelled(), "task panicked: {e:?}");
        }
    }

    // Release the blocker.
    blocker.release(&block_guard).await.unwrap();
    drop(blocker);

    // Wait a moment for broker to drain any newly-eligible composite
    // grants and clean up dead clients.
    tokio::time::sleep(Duration::from_millis(800)).await;

    // Probe every key — none should be held.
    let probe = Client::connect_tcp(("127.0.0.1", port), client_cfg(5_000))
        .await
        .unwrap();
    for key in &keys {
        let g = probe
            .acquire(key, Duration::from_millis(500))
            .await
            .unwrap_or_else(|e| panic!("[seed={seed}] probe acquire {key} failed: {e}"));
        probe.release(&g).await.unwrap();
    }
}

// =========================================================================
// 9. Stress: TTL eviction sweeper handles concurrent grants + abandoned holders
// =========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fuzz_ttl_eviction_under_burst() {
    let seed = seed_for("fuzz_ttl_eviction_under_burst", 0x7777_8888);
    // Aggressive sweeper: the sweep interval must be << TTL or evictions
    // pile up. 5ms sweep interval + 100ms TTL exercises the boundary.
    let cfg = BrokerConfig {
        default_ttl: Duration::from_millis(100),
        max_lock_holders: 1,
        ttl_sweep_interval: Duration::from_millis(5),
        max_concurrency_cap: 1_000,
        idle_key_grace: Duration::ZERO,
    };
    let port = start_broker(cfg).await;
    let keys: Vec<String> = (0..3).map(|i| format!("ttl-key-{i}")).collect();

    // Many clients fire short-TTL acquires then walk away (no release
    // call), forcing the broker's sweeper to do all the cleanup.
    let mut joinset = JoinSet::new();
    let n = 30;
    for client_idx in 0..n {
        let keys = keys.clone();
        let mut my_rng = Rng::new(seed.wrapping_add(client_idx as u64 * 11));
        joinset.spawn(async move {
            let c = Client::connect_tcp(("127.0.0.1", port), client_cfg(2_000))
                .await
                .unwrap();
            for _ in 0..6 {
                let key = &keys[my_rng.gen_range(0, keys.len())];
                let _ = c.acquire(key, Duration::from_millis(50)).await; // abandon
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        });
    }
    while let Some(r) = joinset.join_next().await {
        r.unwrap();
    }

    // Wait > 2 * TTL for sweep + disconnect drain.
    tokio::time::sleep(Duration::from_millis(400)).await;

    let probe = Client::connect_tcp(("127.0.0.1", port), client_cfg(5_000))
        .await
        .unwrap();
    for key in &keys {
        let g = probe
            .acquire(key, Duration::from_millis(500))
            .await
            .unwrap_or_else(|e| panic!("[seed={seed}] post-sweep probe failed on {key}: {e}"));
        probe.release(&g).await.unwrap();
    }
}
