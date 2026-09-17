//! Broker (server) core: synchronized lock state shared by all clients.
//!
//! The broker is single-threaded *with respect to lock state*: every mutation
//! happens under one `parking_lot::Mutex<BrokerState>`. Connection I/O is
//! pushed off via per-client `mpsc` outbound channels, so the broker never
//! blocks on slow clients while holding the state lock. This mirrors the
//! upstream Node.js model (single event loop) but takes advantage of Tokio's
//! multi-threaded runtime for fan-out delivery.
//!
//! Three flavors of lock live here:
//!
//! 1. **Exclusive** (`Lock` / `Unlock`) — a single holder per key with `max`
//!    optionally permitting a small semaphore. Pending requests wait in a per-
//!    key `LinkedQueue`. On unlock we wake exactly one waiter.
//! 2. **Reader-Writer** (`RegisterRead` / `RegisterWrite` / `EndRead` /
//!    `EndWrite`) — multiple concurrent readers, exclusive writer. Writers
//!    wait in the same per-key queue, so readers/writers share FIFO fairness.
//! 3. **Composite** (`Lock` with `keys`) — atomic acquisition of up to
//!    `MAX_COMPOSITE_KEYS` keys. Deadlock-free by globally sorting keys before
//!    queuing, so any two composite requests obtain locks in the same order.
//!
//! Every successful exclusive/composite/RW grant returns a monotonically
//! increasing per-key `fencing_token` that callers can attach to downstream
//! writes (Kleppmann fencing).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::protocol::{Request, Response, MAX_COMPOSITE_KEYS};
use crate::queue::LinkedQueue;

pub type ClientId = u64;
pub type Sender = mpsc::UnboundedSender<Response>;
type ResponseObserver = Arc<dyn Fn(&Response) + Send + Sync + 'static>;
const SNAPSHOT_DETACHED_CLIENT: ClientId = 0;

/// Public construction options.
#[derive(Debug, Clone)]
pub struct BrokerConfig {
    pub default_ttl: Duration,
    /// Default `max` (concurrency level) for keys that haven't had an
    /// explicit `max` passed on `lock`. `1` means classic mutex
    /// semantics (one holder at a time); higher values turn each key
    /// into a counting semaphore. Always clamped at
    /// `max_concurrency_cap`.
    pub max_lock_holders: u32,
    /// How often the periodic TTL sweep runs (see upstream
    /// [`live-mutex#13`](https://github.com/ORESoftware/live-mutex/issues/13)
    /// — "instead create a setTimeout, every 10 ms or so"). Defaults to
    /// `10ms`. Set `Duration::ZERO` to disable the sweeper entirely
    /// (locks will never auto-evict; useful for tests that drive
    /// `Broker::tick_ttl` directly with a synthetic `Instant`).
    pub ttl_sweep_interval: Duration,
    /// Hard upper bound on per-key concurrency. A per-request `max`
    /// above this is silently clamped and counted; the operator can
    /// raise the ceiling explicitly if they have a workload that needs
    /// it. Default: [`crate::protocol::DEFAULT_MAX_CONCURRENCY_CAP`]
    /// (`1_000`).
    pub max_concurrency_cap: u32,
    /// How long a `LockState` must remain fully idle (no holders, no
    /// readers, no writer, no waiters) before the periodic TTL sweep
    /// reclaims it from `state.locks`. The point of the grace period
    /// is to absorb bursty workloads where keys cycle between active
    /// and idle within milliseconds, without paying the cost of
    /// destroying and rebuilding `LockState` on every cycle.
    ///
    /// Set to `Duration::ZERO` to disable empty-key pruning entirely
    /// (the historical behaviour: `state.locks` grows monotonically
    /// with the set of distinct keys ever observed). Default
    /// `60s`.
    ///
    /// Cross-incarnation fencing-token monotonicity is preserved
    /// regardless of pruning thanks to the broker-wide
    /// `fencing_watermark` (see `BrokerMetrics::fencing_watermark`):
    /// a freshly-materialised `LockState` always seeds its per-key
    /// counter from `max(wall_clock_ms, watermark)`, so a hot key
    /// that gets pruned and re-acquired never mints a smaller
    /// token than was previously in use for any key.
    pub idle_key_grace: Duration,
}

impl Default for BrokerConfig {
    fn default() -> Self {
        crate::routine_id!("ddl-routine-G0Rs-3QRGfpIGaKxIT");
        Self {
            default_ttl: Duration::from_millis(4000),
            max_lock_holders: 1,
            ttl_sweep_interval: Duration::from_millis(10),
            max_concurrency_cap: crate::protocol::DEFAULT_MAX_CONCURRENCY_CAP,
            idle_key_grace: Duration::from_secs(60),
        }
    }
}

#[derive(Debug)]
struct ClientHandle {
    sender: Sender,
    held_lock_uuids: HashMap<String, KeysOfLock>, // lock_uuid -> keys it holds
    pending_request_uuids: Vec<(String, String)>, // (key, request_uuid) so we can purge waiters
}

#[derive(Debug, Clone)]
struct KeysOfLock {
    keys: Vec<String>,
    rw_kind: RwHoldKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum RwHoldKind {
    Exclusive,
    Read,
    Write,
}

#[derive(Debug)]
struct LockState {
    /// Active exclusive holders keyed by `lock_uuid`.
    exclusive_holders: HashMap<String, ExclusiveHolder>,
    /// Maximum simultaneous holders; defaults to 1.
    max: u32,
    /// Active readers keyed by `lock_uuid`.
    readers: HashMap<String, RwHolder>,
    /// Active writer keyed by `lock_uuid`. At most one.
    writer: Option<RwHolder>,
    /// FIFO queue of pending lock requests for this key.
    queue: LinkedQueue<String, PendingRequest>,
    /// Per-key monotonic fencing-token counter.
    fencing_counter: u64,
    /// Set when nobody is holding or waiting; used by GC sweeps.
    timestamp_emptied: Option<Instant>,
}

impl LockState {
    fn new(max: u32) -> Self {
        crate::routine_id!("ddl-routine-dBj7gUl_DGMNSi9kLz");
        Self {
            exclusive_holders: HashMap::new(),
            max: max.max(1),
            readers: HashMap::new(),
            writer: None,
            queue: LinkedQueue::new(),
            // Seed the per-key fencing counter from wall-clock millis
            // since epoch. Subsequent grants increment by 1, so:
            //   * monotonicity is still strictly counter-driven,
            //   * tokens are wall-clock-aligned for human inspection,
            //   * after a broker restart, the same key's tokens jump
            //     to a fresh `now`, strictly greater than any prior
            //     incarnation's tokens (assuming the system clock
            //     didn't roll back further than uptime).
            //
            // We use millis (not nanos) because the wire format is JSON
            // and a JS client's `Number` only safely represents integers
            // up to 2^53 (~9e15). Nanos-since-epoch is ~1.7e18 today and
            // would silently lose precision when deserialised by a JS
            // client; keeping the upstream `live-mutex` and us byte-for-
            // byte compatible matters for cross-runtime tests.
            //
            // `SystemTime::UNIX_EPOCH` can theoretically fail on platforms
            // with a clock set before 1970-01-01; treat that as 0 (we
            // start counting from 1 below) since this is purely an
            // observability nicety and per-key strict monotonicity
            // remains intact regardless of the seed.
            fencing_counter: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            timestamp_emptied: None,
        }
    }

    fn is_idle(&self) -> bool {
        crate::routine_id!("ddl-routine-EXScnKwI3L7i7chNkf");
        self.exclusive_holders.is_empty()
            && self.readers.is_empty()
            && self.writer.is_none()
            && self.queue.is_empty()
    }

    fn next_fencing_token(&mut self) -> u64 {
        crate::routine_id!("ddl-routine-V5cwqbCaR6r3T4yENj");
        self.fencing_counter = self.fencing_counter.wrapping_add(1).max(1);
        self.fencing_counter
    }

    fn next_or_forced_fencing_token(&mut self, forced: Option<u64>) -> u64 {
        crate::routine_id!("ddl-routine-broker-next-or-forced-fencing-token-1");
        match forced {
            Some(token) => {
                self.fencing_counter = self.fencing_counter.max(token);
                token
            }
            None => self.next_fencing_token(),
        }
    }
}

#[derive(Debug, Clone)]
struct ExclusiveHolder {
    #[allow(dead_code)] // retained for future per-holder accounting / inspection
    client: ClientId,
    #[allow(dead_code)]
    pid: Option<i64>,
    #[allow(dead_code)]
    fencing_token: u64,
    keep_locks_after_death: bool,
    composite_member: bool,
}

#[derive(Debug, Clone)]
struct RwHolder {
    client: ClientId,
    #[allow(dead_code)]
    fencing_token: u64,
    lock_uuid: String,
}

#[derive(Debug, Clone)]
struct PendingRequest {
    request_uuid: String,
    client: ClientId,
    pid: Option<i64>,
    ttl: Option<Duration>,
    grant_lock_uuid: Option<String>,
    grant_fencing_seed: Option<u64>,
    keep_locks_after_death: bool,
    kind: PendingKind,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct GrantOverrides {
    pub lock_uuid: Option<String>,
    pub fencing_seed: Option<u64>,
}

impl GrantOverrides {
    fn token(&self, offset: u64) -> Option<u64> {
        crate::routine_id!("ddl-routine-broker-grant-overrides-token-1");
        self.fencing_seed.map(|seed| seed.saturating_add(offset))
    }
}

#[derive(Debug, Clone)]
struct CompositeLockRequest {
    client: ClientId,
    uuid: String,
    keys: Vec<String>,
    pid: Option<i64>,
    ttl: Option<Duration>,
    wait: bool,
    grant_overrides: GrantOverrides,
}

#[derive(Debug, Clone)]
enum PendingKind {
    Exclusive,
    Reader,
    Writer,
    /// Composite request that still needs to acquire `remaining_keys` (always
    /// sorted lexicographically). Members are queued one at a time on the
    /// head of `remaining_keys`; on grant we move to the next key.
    Composite {
        all_keys: Vec<String>,
        remaining_keys: Vec<String>,
        granted_keys: Vec<String>,
        granted_tokens: BTreeMap<String, u64>,
        composite_lock_uuid: String,
    },
}

/// One entry per outstanding TTL deadline. Lookup is by `(deadline, seq)`
/// in the `BrokerState.deadlines` BTreeMap; the seq disambiguates
/// concurrent acquires that happen to compute the same `Instant`.
///
/// **Lazy deletion.** When a lock is released early (`handle_unlock`) we
/// do *not* remove the matching deadline entry — at sweep time we
/// re-check `LockState` and skip entries whose lock has already
/// disappeared. That trades a tiny amount of extra memory for keeping
/// the unlock fast path completely off the BTreeMap. This is the same
/// trade-off upstream [`live-mutex#13`](https://github.com/ORESoftware/live-mutex/issues/13)
/// recommends: avoid touching a per-request timer on every unlock.
#[derive(Debug, Clone)]
struct DeadlineEntry {
    lock_uuid: String,
    keys: Vec<String>,
    kind: RwHoldKind,
    /// The client that originally held the lock. Used to remove from
    /// `ClientHandle.held_lock_uuids` if the client is still around.
    client: ClientId,
}

#[derive(Debug)]
struct BrokerState {
    locks: HashMap<String, LockState>,
    clients: HashMap<ClientId, ClientHandle>,
    next_client_id: ClientId,
    config: BrokerConfig,
    /// Single shared deadline index. One BTreeMap entry per holder with a
    /// non-zero TTL; the periodic sweeper pops `range(..=now)` in one
    /// pass — O(log n + k) for the k expired entries — instead of
    /// scheduling a `tokio::time::sleep` per lock. Empty when no lock has
    /// a TTL configured.
    deadlines: std::collections::BTreeMap<(Instant, u64), DeadlineEntry>,
    /// Monotonic seq used to disambiguate `Instant` collisions.
    deadline_seq: u64,
    /// Total number of TTL evictions ever performed by `tick_ttl`. Read
    /// via `Broker::metrics()` and surfaced as
    /// `dd_rust_network_mutex_ttl_evictions_total`.
    ttl_evictions_total: u64,
    /// When the broker was constructed; powers the "uptime" line on the
    /// HTML status page (upstream `live-mutex#108`).
    started_at: Instant,
    /// How many `lock` requests had their `max` field clamped to
    /// `config.max_concurrency_cap`. Surfaced as
    /// `dd_rust_network_mutex_concurrency_cap_clamps_total`; a non-zero
    /// value here means at least one client is asking for a concurrency
    /// level above the broker's ceiling.
    concurrency_cap_clamps_total: u64,
    /// Strictly monotonic upper bound on every fencing token issued
    /// since broker start, across every key. Updated on every grant
    /// (exclusive, composite, reader, writer) via
    /// `observe_fencing_token`. Re-applied as the seed floor whenever
    /// `lock_or_default` materialises a fresh `LockState` so that
    /// pruning a hot key and recreating it cannot mint a smaller
    /// token than was previously in use. Surfaced as
    /// `dd_rust_network_mutex_fencing_watermark`.
    ///
    /// The watermark exists in memory only — it is not persisted
    /// across broker restarts. After a restart the in-memory
    /// watermark resets to 0 and the wall-clock-millis seed in
    /// `LockState::new` takes over again, which preserves
    /// monotonicity *as long as wall-clock progress between starts
    /// exceeds the number of acquires of any single key*. Operators
    /// who need strict cross-restart monotonicity should layer
    /// disk persistence on top in a follow-up.
    fencing_watermark: u64,
    /// Cumulative count of idle `LockState` entries reclaimed by the
    /// periodic empty-key prune sweep (see
    /// `BrokerConfig::idle_key_grace`). Surfaced as
    /// `dd_rust_network_mutex_idle_keys_pruned_total`. A
    /// monotonically increasing value here is healthy on a broker
    /// with churning keys; a flat value means no key has been idle
    /// past the grace window.
    idle_keys_pruned_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrokerRaftSnapshot {
    schema_version: u32,
    metrics: BrokerRaftSnapshotMetrics,
    locks: Vec<BrokerRaftLockSnapshot>,
    deadlines: Vec<BrokerRaftDeadlineSnapshot>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrokerRaftSnapshotMetrics {
    keys: u64,
    holders: u64,
    waiters: u64,
    clients: u64,
    pending_deadlines: u64,
    ttl_evictions_total: u64,
    max_concurrency_cap: u32,
    concurrency_cap_clamps_total: u64,
    fencing_watermark: u64,
    idle_keys_pruned_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrokerRaftLockSnapshot {
    key: String,
    max: u32,
    fencing_counter: u64,
    exclusive_holders: Vec<BrokerRaftExclusiveHolderSnapshot>,
    readers: Vec<BrokerRaftRwHolderSnapshot>,
    writer: Option<BrokerRaftRwHolderSnapshot>,
    #[serde(default)]
    queue: Vec<BrokerRaftPendingRequestSnapshot>,
    idle: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrokerRaftExclusiveHolderSnapshot {
    lock_uuid: String,
    pid: Option<i64>,
    fencing_token: u64,
    keep_locks_after_death: bool,
    composite_member: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrokerRaftRwHolderSnapshot {
    lock_uuid: String,
    fencing_token: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrokerRaftPendingRequestSnapshot {
    request_uuid: String,
    client_id: ClientId,
    pid: Option<i64>,
    ttl_ms: Option<u64>,
    grant_lock_uuid: Option<String>,
    grant_fencing_seed: Option<u64>,
    keep_locks_after_death: bool,
    kind: BrokerRaftPendingKindSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum BrokerRaftPendingKindSnapshot {
    Exclusive,
    Reader,
    Writer,
    Composite {
        all_keys: Vec<String>,
        remaining_keys: Vec<String>,
        granted_keys: Vec<String>,
        granted_tokens: BTreeMap<String, u64>,
        composite_lock_uuid: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrokerRaftDeadlineSnapshot {
    lock_uuid: String,
    keys: Vec<String>,
    kind: RwHoldKind,
    remaining_ms: u64,
}

impl From<BrokerMetrics> for BrokerRaftSnapshotMetrics {
    fn from(metrics: BrokerMetrics) -> Self {
        crate::routine_id!("ddl-routine-broker-raft-snapshot-metrics-from-1");
        Self {
            keys: metrics.keys,
            holders: metrics.holders,
            waiters: metrics.waiters,
            clients: metrics.clients,
            pending_deadlines: metrics.pending_deadlines,
            ttl_evictions_total: metrics.ttl_evictions_total,
            max_concurrency_cap: metrics.max_concurrency_cap,
            concurrency_cap_clamps_total: metrics.concurrency_cap_clamps_total,
            fencing_watermark: metrics.fencing_watermark,
            idle_keys_pruned_total: metrics.idle_keys_pruned_total,
        }
    }
}

impl BrokerState {
    fn new(config: BrokerConfig) -> Self {
        crate::routine_id!("ddl-routine-teWZ7PuRjYTRJlYARn");
        Self {
            locks: HashMap::new(),
            clients: HashMap::new(),
            next_client_id: 1,
            config,
            deadlines: std::collections::BTreeMap::new(),
            deadline_seq: 0,
            ttl_evictions_total: 0,
            started_at: Instant::now(),
            concurrency_cap_clamps_total: 0,
            fencing_watermark: 0,
            idle_keys_pruned_total: 0,
        }
    }

    /// Bump the broker-wide fencing watermark to at least `token`. Cheap —
    /// a single u64 max + maybe a write. Must be called after every
    /// successful grant (exclusive, composite, reader, writer) so that
    /// a freshly-materialised `LockState` for a previously-pruned key
    /// can seed its counter from the watermark and never mint a
    /// smaller token than was previously in use.
    fn observe_fencing_token(&mut self, token: u64) {
        crate::routine_id!("ddl-routine-observe-fencing-token-Q3z");
        if token > self.fencing_watermark {
            self.fencing_watermark = token;
        }
    }

    /// Refresh `lock.timestamp_emptied` for `key` based on the
    /// lock's current idle status. Called from every code path that
    /// could change a `LockState`'s idleness — unlock, end_read,
    /// end_write, drop_client, ttl eviction, or try_grant when the
    /// queue is drained. Works as both "mark idle now" and "mark
    /// active now": exactly one transition per call.
    ///
    /// We deliberately don't maintain a separate idle-key index
    /// here. The prune sweep walks `state.locks` once per
    /// `ttl_sweep_interval` (default 10ms); for brokers with
    /// extremely large key cardinalities this can be revisited by
    /// adding a `BTreeSet<(Instant, String)>` populated alongside
    /// `timestamp_emptied`, but the simple walk keeps the data
    /// structure surface small and matches the cost of the
    /// existing deadline sweep.
    fn maybe_mark_idle(&mut self, key: &str) {
        crate::routine_id!("ddl-routine-maybe-mark-idle-Yv4");
        if let Some(lock) = self.locks.get_mut(key) {
            if lock.is_idle() {
                if lock.timestamp_emptied.is_none() {
                    lock.timestamp_emptied = Some(Instant::now());
                }
            } else if lock.timestamp_emptied.is_some() {
                lock.timestamp_emptied = None;
            }
        }
    }

    /// Resolve the effective concurrency level for a `lock` request.
    /// Honours the per-request `max` if present, falls back to the
    /// per-key `lock.max` (which itself defaults to
    /// `config.max_lock_holders`), clamps to
    /// `config.max_concurrency_cap`, and increments
    /// `concurrency_cap_clamps_total` on clamp. Returns the final cap.
    ///
    /// **Invariant**: `requested` is never `Some(0)` — that case is
    /// rejected eagerly in `handle_request` before this function is
    /// called. We treat it the same as `None` here purely defensively,
    /// in case a future code path bypasses the validation.
    fn resolve_max(&mut self, current_lock_max: u32, requested: Option<u32>) -> u32 {
        crate::routine_id!("ddl-routine-CUS0RH207soda48al_");
        let cap = self.config.max_concurrency_cap.max(1);
        match requested {
            None | Some(0) => current_lock_max.min(cap).max(1),
            Some(m) => {
                if m > cap {
                    self.concurrency_cap_clamps_total =
                        self.concurrency_cap_clamps_total.wrapping_add(1);
                    cap
                } else {
                    m
                }
            }
        }
    }

    /// Schedule a TTL deadline for `lock_uuid`. Skips registration if
    /// `ttl` is `None` or zero (a permanent lock).
    fn schedule_deadline(
        &mut self,
        ttl: Option<Duration>,
        lock_uuid: &str,
        keys: &[String],
        kind: RwHoldKind,
        client: ClientId,
    ) {
        crate::routine_id!("ddl-routine-2lgzzbgohSwSILDhc1");
        let Some(ttl) = ttl else { return };
        if ttl.is_zero() {
            return;
        }
        let Some(deadline) = Instant::now().checked_add(ttl) else {
            // A malicious or buggy raw client can send `ttl` values near
            // `u64::MAX` milliseconds. Treat values outside the platform's
            // `Instant` range as effectively permanent rather than letting
            // request data panic the broker task.
            tracing::warn!(
                target: "lmx::broker",
                lock_uuid,
                ttl_ms = ttl.as_millis(),
                "ttl is too large for this platform; deadline not scheduled",
            );
            return;
        };
        self.deadline_seq = self.deadline_seq.wrapping_add(1);
        self.deadlines.insert(
            (deadline, self.deadline_seq),
            DeadlineEntry {
                lock_uuid: lock_uuid.to_string(),
                keys: keys.to_vec(),
                kind,
                client,
            },
        );
    }

    fn lock_or_default(&mut self, key: &str) -> &mut LockState {
        crate::routine_id!("ddl-routine-1mpkfa_yLbcablw2xr");
        // The per-key starting `max` is clamped to the broker-wide
        // ceiling, so a misconfigured `LMX_MAX_LOCK_HOLDERS` can't
        // smuggle a giant default past `max_concurrency_cap`.
        let cap = self.config.max_concurrency_cap.max(1);
        let max = self.config.max_lock_holders.min(cap).max(1);
        // A freshly-materialised LockState normally seeds its
        // fencing counter from wall-clock-millis. When the broker
        // has previously issued a higher token (recorded in the
        // watermark), we lift the seed so the next grant on this
        // key cannot collide with — or trail — a token already in
        // circulation. Cheap: a u64 max in the cold path of first
        // acquire only.
        let seed_floor = self.fencing_watermark;
        self.locks.entry(key.to_string()).or_insert_with(|| {
            let mut s = LockState::new(max);
            if seed_floor > s.fencing_counter {
                s.fencing_counter = seed_floor;
            }
            s
        })
    }

    #[allow(dead_code)] // wired up by future TTL/GC sweeps
    fn maybe_gc(&mut self, key: &str) {
        crate::routine_id!("ddl-routine-BqokwfXoVWp7HgkhKO");
        if let Some(state) = self.locks.get(key) {
            if state.is_idle() && state.fencing_counter > 0 {
                // Keep the fencing counter alive — drop the entry only after
                // it has been idle for a while. We tag with timestamp_emptied
                // and let an external sweeper (or future ttl) prune it. For
                // now, never GC: keeping the counter preserves the
                // monotonicity guarantee across all reincarnations of `key`.
                let _ = state;
            }
        }
    }
}

/// Public broker handle. Cheaply cloneable; all methods take `&self`.
#[derive(Clone)]
pub struct Broker {
    state: Arc<Mutex<BrokerState>>,
    response_observer: Option<ResponseObserver>,
}

impl Broker {
    pub fn new(config: BrokerConfig) -> Self {
        crate::routine_id!("ddl-routine-V4_qGcXJ5Hjo8hOHBJ");
        Self {
            state: Arc::new(Mutex::new(BrokerState::new(config))),
            response_observer: None,
        }
    }

    pub(crate) fn with_response_observer(
        config: BrokerConfig,
        response_observer: ResponseObserver,
    ) -> Self {
        crate::routine_id!("ddl-routine-broker-with-response-observer-1");
        Self {
            state: Arc::new(Mutex::new(BrokerState::new(config))),
            response_observer: Some(response_observer),
        }
    }

    /// Register a new client connection. Returns the client's id and a
    /// pre-built `mpsc::UnboundedReceiver<Response>` the listener should pump
    /// onto its socket.
    pub fn register_client(&self) -> (ClientId, mpsc::UnboundedReceiver<Response>) {
        crate::routine_id!("ddl-routine-DlZgZB0LiJJZNP7VSQ");
        let (tx, rx) = mpsc::unbounded_channel();
        let mut state = self.state.lock();
        let id = Self::next_available_client_id(&state, state.next_client_id);
        Self::insert_client_handle(&mut state, id, tx);
        (id, rx)
    }

    pub(crate) fn register_client_with_id(
        &self,
        preferred_id: ClientId,
    ) -> (ClientId, mpsc::UnboundedReceiver<Response>) {
        crate::routine_id!("ddl-routine-broker-register-client-with-id-1");
        let (tx, rx) = mpsc::unbounded_channel();
        let mut state = self.state.lock();
        let id = Self::next_available_client_id(&state, preferred_id);
        Self::insert_client_handle(&mut state, id, tx);
        (id, rx)
    }

    fn next_available_client_id(state: &BrokerState, preferred_id: ClientId) -> ClientId {
        crate::routine_id!("ddl-routine-broker-next-available-client-id-1");
        let mut id = preferred_id.max(1);
        while id == SNAPSHOT_DETACHED_CLIENT || state.clients.contains_key(&id) {
            id = id.wrapping_add(1).max(1);
        }
        id
    }

    fn insert_client_handle(state: &mut BrokerState, id: ClientId, sender: Sender) {
        crate::routine_id!("ddl-routine-broker-insert-client-handle-1");
        // `wrapping_add` to defend against a debug-mode panic if a very
        // long-lived broker exhausts u64 (~1 client/ns for ~580 years).
        // Client id 0 is reserved for detached Raft snapshot holders, so both
        // the chosen id and next local id skip it.
        state.next_client_id = id.wrapping_add(1).max(1);
        state.clients.insert(
            id,
            ClientHandle {
                sender,
                held_lock_uuids: HashMap::new(),
                pending_request_uuids: Vec::new(),
            },
        );
    }

    /// Drop a client, releasing every lock it held and pruning every queued
    /// request it owned. Callers should invoke this exactly once when the
    /// client's transport goes away.
    pub fn drop_client(&self, client: ClientId) {
        crate::routine_id!("ddl-routine-gQrzxtKPDCU4Qyiwar");
        let mut state = self.state.lock();
        let Some(mut handle) = state.clients.remove(&client) else {
            // Raft followers replay leader-side ephemeral client IDs without
            // registering transport handles for them. A replicated DropClient
            // means the leader decided this client is gone, so followers must
            // remove both queued work and any holder that may have been granted
            // just before the cleanup entry committed.
            let mut touched_keys: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let mut queued_removals: Vec<(String, String)> = Vec::new();
            let mut exclusive_removals: Vec<(String, String)> = Vec::new();
            let mut reader_removals: Vec<(String, String)> = Vec::new();
            let mut writer_removals: Vec<String> = Vec::new();
            for (key, lock) in state.locks.iter() {
                for (request_uuid, pending) in lock.queue.iter() {
                    if pending.client == client {
                        queued_removals.push((key.clone(), request_uuid.clone()));
                    }
                }
                for (lock_uuid, holder) in lock.exclusive_holders.iter() {
                    if holder.client == client {
                        exclusive_removals.push((key.clone(), lock_uuid.clone()));
                    }
                }
                for (lock_uuid, holder) in lock.readers.iter() {
                    if holder.client == client {
                        reader_removals.push((key.clone(), lock_uuid.clone()));
                    }
                }
                if lock
                    .writer
                    .as_ref()
                    .is_some_and(|writer| writer.client == client)
                {
                    writer_removals.push(key.clone());
                }
            }
            for (key, request_uuid) in queued_removals {
                if let Some(lock) = state.locks.get_mut(&key) {
                    lock.queue.remove(&request_uuid);
                }
                touched_keys.insert(key);
            }
            for (key, lock_uuid) in exclusive_removals {
                if let Some(lock) = state.locks.get_mut(&key) {
                    lock.exclusive_holders.remove(&lock_uuid);
                }
                touched_keys.insert(key);
            }
            for (key, lock_uuid) in reader_removals {
                if let Some(lock) = state.locks.get_mut(&key) {
                    lock.readers.remove(&lock_uuid);
                }
                touched_keys.insert(key);
            }
            for key in writer_removals {
                if let Some(lock) = state.locks.get_mut(&key) {
                    lock.writer = None;
                }
                touched_keys.insert(key);
            }
            for key in touched_keys {
                state.maybe_mark_idle(&key);
                self.try_grant_next(&mut state, &key);
            }
            return;
        };

        // Track every key whose state we might mutate so the final
        // try_grant_next sweep is bounded by the dropped client's
        // touched keys instead of `state.locks.len()`. On a busy
        // broker (millions of keys, frequent client churn), the
        // previous O(N_keys * N_drops) loop showed up at the top of
        // `drop_client`'s flame graph; this caps it at O(touched).
        let mut touched_keys: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Release pending waiters first so they cannot win an unlock race.
        for (key, request_uuid) in handle.pending_request_uuids.drain(..) {
            if let Some(lock) = state.locks.get_mut(&key) {
                lock.queue.remove(&request_uuid);
            }
            touched_keys.insert(key);
        }

        // Release every held lock.
        let held = std::mem::take(&mut handle.held_lock_uuids);
        for (lock_uuid, KeysOfLock { keys, rw_kind }) in held.into_iter() {
            for key in keys {
                let Some(lock) = state.locks.get_mut(&key) else {
                    continue;
                };
                match rw_kind {
                    RwHoldKind::Exclusive => {
                        if let Some(holder) = lock.exclusive_holders.get(&lock_uuid) {
                            // keep_locks_after_death honoured for non-composite
                            // exclusive holders only.
                            if holder.keep_locks_after_death && !holder.composite_member {
                                continue;
                            }
                        }
                        lock.exclusive_holders.remove(&lock_uuid);
                    }
                    RwHoldKind::Read => {
                        lock.readers.remove(&lock_uuid);
                    }
                    RwHoldKind::Write => {
                        if lock
                            .writer
                            .as_ref()
                            .is_some_and(|w| w.lock_uuid == lock_uuid)
                        {
                            lock.writer = None;
                        }
                    }
                }
                touched_keys.insert(key);
            }
        }

        // After dropping holders, try to grant the next waiter on each
        // key the dropped client actually touched.
        for key in touched_keys {
            self.try_grant_next(&mut state, &key);
        }
    }

    /// Handle a single inbound request from `client`. Sends responses back
    /// via the client's mpsc sender (no return value because there can be
    /// many or zero outbound messages: e.g. composite-lock partial progress).
    pub fn handle_request(&self, client: ClientId, request: Request) {
        crate::routine_id!("ddl-routine-broker-handle-request-public-1");
        self.handle_request_with_grant_uuid(client, request, None);
    }

    pub(crate) fn handle_request_with_grant_uuid(
        &self,
        client: ClientId,
        request: Request,
        grant_lock_uuid: Option<String>,
    ) {
        crate::routine_id!("ddl-routine-broker-handle-request-with-grant-uuid-1");
        self.handle_request_with_grant_overrides(
            client,
            request,
            GrantOverrides {
                lock_uuid: grant_lock_uuid,
                fencing_seed: None,
            },
        );
    }

    pub(crate) fn handle_request_with_grant_overrides(
        &self,
        client: ClientId,
        request: Request,
        grant_overrides: GrantOverrides,
    ) {
        crate::routine_id!("ddl-routine-0GBKaWmx2RzXgUnbEr");
        let mut state = self.state.lock();
        match request {
            Request::Version { uuid, value: _ } => {
                self.send(
                    &state,
                    client,
                    Response::Version {
                        uuid,
                        broker_version: crate::protocol::PROTOCOL_VERSION.into(),
                        ok: true,
                        error: None,
                    },
                );
            }
            Request::Auth { uuid, .. } => {
                // Auth is enforced at the listener layer; if we got here the
                // token already matched.
                self.send(
                    &state,
                    client,
                    Response::Auth {
                        uuid,
                        ok: true,
                        error: None,
                    },
                );
            }
            Request::Lock {
                uuid,
                key,
                keys,
                pid,
                ttl,
                max,
                force,
                retry_count: _retry_count,
                keep_locks_after_death,
                wait,
            } => {
                // Default (absent) is wait=true: queue and block until grant,
                // matching the historical broker behaviour. `wait:false` makes
                // the request fail fast (acquired:false) without ever being
                // enqueued, so it can't leak a deferred grant.
                let wait = wait.unwrap_or(true);
                let ttl = ttl.map(Duration::from_millis);
                // `max = Some(0)` is rejected eagerly. It used to mean
                // "preserve the existing per-key cap" (same as
                // `max = None`), but that was a silent foot-gun: a
                // misconfigured caller passing `max: 0` would be told
                // their lock was acquired with the previous (or
                // default) concurrency level instead of being told
                // their request was malformed. Callers who genuinely
                // want "leave the cap as-is" should omit the field.
                if matches!(max, Some(0)) {
                    let err =
                        "`max` must be >= 1; omit the field to keep the existing concurrency level"
                            .to_string();
                    match (&key, &keys) {
                        (_, Some(_)) => self.send(
                            &state,
                            client,
                            Response::CompositeLock {
                                uuid,
                                keys: keys.unwrap_or_default(),
                                acquired: false,
                                lock_uuid: None,
                                fencing_tokens: None,
                                error: Some(err),
                            },
                        ),
                        _ => self.send(
                            &state,
                            client,
                            Response::Lock {
                                uuid,
                                key: key.unwrap_or_default(),
                                acquired: false,
                                lock_request_count: 0,
                                lock_uuid: None,
                                fencing_token: None,
                                readers_count: None,
                                error: Some(err),
                            },
                        ),
                    }
                    return;
                }
                match (key, keys) {
                    (Some(k), None) => {
                        self.handle_exclusive_lock(
                            &mut state,
                            client,
                            uuid,
                            k,
                            pid,
                            ttl,
                            max,
                            keep_locks_after_death,
                            force,
                            wait,
                            grant_overrides,
                        );
                    }
                    (None, Some(ks)) => {
                        self.handle_composite_lock(
                            &mut state,
                            CompositeLockRequest {
                                client,
                                uuid,
                                keys: ks,
                                pid,
                                ttl,
                                wait,
                                grant_overrides,
                            },
                        );
                    }
                    (Some(_), Some(_)) => {
                        self.send(
                            &state,
                            client,
                            Response::Error {
                                uuid,
                                error: "lock request must set either `key` or `keys`, not both"
                                    .into(),
                            },
                        );
                    }
                    (None, None) => {
                        self.send(
                            &state,
                            client,
                            Response::Error {
                                uuid,
                                error: "lock request requires `key` or `keys`".into(),
                            },
                        );
                    }
                }
            }
            Request::Unlock {
                uuid,
                key,
                keys,
                lock_uuid,
                force,
            } => {
                let target_keys = match (key, keys) {
                    (Some(k), None) => vec![k],
                    (None, Some(ks)) => ks,
                    (Some(_), Some(_)) => {
                        self.send(
                            &state,
                            client,
                            Response::Error {
                                uuid,
                                error: "unlock request must set either `key` or `keys`, not both"
                                    .into(),
                            },
                        );
                        return;
                    }
                    (None, None) => {
                        self.send(
                            &state,
                            client,
                            Response::Error {
                                uuid,
                                error: "unlock request requires `key` or `keys`".into(),
                            },
                        );
                        return;
                    }
                };
                self.handle_unlock(&mut state, client, uuid, target_keys, lock_uuid, force);
            }
            Request::RegisterRead { uuid, key } => {
                self.handle_register_read(&mut state, client, uuid, key, grant_overrides);
            }
            Request::RegisterWrite { uuid, key } => {
                self.handle_register_write(&mut state, client, uuid, key, grant_overrides);
            }
            Request::EndRead { uuid, key } => {
                self.handle_end_read(&mut state, client, uuid, key);
            }
            Request::EndWrite { uuid, key } => {
                self.handle_end_write(&mut state, client, uuid, key);
            }
            Request::LockInfo { uuid, key } => {
                let info = state
                    .locks
                    .get(&key)
                    .map(|s| {
                        let holders: Vec<String> = s
                            .exclusive_holders
                            .keys()
                            .cloned()
                            .chain(s.readers.keys().cloned())
                            .chain(s.writer.iter().map(|w| w.lock_uuid.clone()))
                            .collect();
                        (
                            !holders.is_empty(),
                            holders,
                            s.queue.len(),
                            s.readers.len() as u32,
                            s.writer.is_some(),
                        )
                    })
                    .unwrap_or((false, vec![], 0, 0, false));
                self.send(
                    &state,
                    client,
                    Response::LockInfo {
                        uuid,
                        key,
                        is_locked: info.0,
                        lockholder_uuids: info.1,
                        lock_request_count: info.2,
                        readers_count: info.3,
                        writer_flag: info.4,
                    },
                );
            }
            Request::Ls { uuid } => {
                let keys: Vec<String> = state.locks.keys().cloned().collect();
                self.send(&state, client, Response::LsResult { uuid, keys });
            }
            Request::Heartbeat { uuid } => {
                self.send(&state, client, Response::Ok { uuid });
            }
        }
    }

    // ---- exclusive --------------------------------------------------------

    // Several Lock-request fields each map to a distinct broker effect, so we
    // intentionally pass them through individually rather than bundling into a
    // struct just to satisfy clippy.
    #[allow(clippy::too_many_arguments)]
    fn handle_exclusive_lock(
        &self,
        state: &mut BrokerState,
        client: ClientId,
        uuid: String,
        key: String,
        pid: Option<i64>,
        ttl: Option<Duration>,
        max: Option<u32>,
        keep_locks_after_death: bool,
        force: bool,
        wait: bool,
        grant_overrides: GrantOverrides,
    ) {
        crate::routine_id!("ddl-routine--MjJFOFOY7fGYtsmOT");
        // Resolve & clamp the requested concurrency level *before* we
        // touch the LockState. Pulling the current per-key `max` first
        // keeps the resolution rule consistent across the fast path
        // (this function) and the dequeue path (`try_grant_once`).
        let current_max = state
            .locks
            .get(&key)
            .map(|s| s.max)
            .unwrap_or(state.config.max_lock_holders);
        let effective_max = state.resolve_max(current_max, max);
        let lock = state.lock_or_default(&key);
        lock.max = effective_max;
        let cap = lock.max as usize;
        if lock.writer.is_none() && lock.readers.is_empty() && lock.exclusive_holders.len() < cap {
            let token = lock.next_or_forced_fencing_token(grant_overrides.token(0));
            let lock_uuid = grant_overrides
                .lock_uuid
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            lock.exclusive_holders.insert(
                lock_uuid.clone(),
                ExclusiveHolder {
                    client,
                    pid,
                    fencing_token: token,
                    keep_locks_after_death,
                    composite_member: false,
                },
            );
            let queue_depth = lock.queue.len();
            state.observe_fencing_token(token);
            state.maybe_mark_idle(&key);
            self.track_holder(
                state,
                client,
                &lock_uuid,
                std::slice::from_ref(&key),
                RwHoldKind::Exclusive,
            );
            // Single shared deadline index — see upstream live-mutex#13.
            state.schedule_deadline(
                ttl,
                &lock_uuid,
                std::slice::from_ref(&key),
                RwHoldKind::Exclusive,
                client,
            );
            self.send(
                state,
                client,
                Response::Lock {
                    uuid,
                    key,
                    acquired: true,
                    lock_request_count: queue_depth,
                    lock_uuid: Some(lock_uuid),
                    fencing_token: Some(token),
                    readers_count: Some(0),
                    error: None,
                },
            );
            return;
        }

        // No-wait (try-lock): the caller asked to fail fast rather than be
        // enqueued. Report contention immediately and DO NOT queue, so there's
        // no deferred grant for the caller to leak.
        if !wait {
            let depth = state.locks.get(&key).map(|s| s.queue.len()).unwrap_or(0);
            self.send(
                state,
                client,
                Response::Lock {
                    uuid,
                    key,
                    acquired: false,
                    lock_request_count: depth,
                    lock_uuid: None,
                    fencing_token: None,
                    readers_count: Some(0),
                    error: None,
                },
            );
            return;
        }

        // Otherwise queue it. `force: true` jumps to the head of the
        // FIFO (matches upstream live-mutex's writer-preference
        // affordance — when an operator marks an acquire as urgent it
        // bypasses peers that have already been waiting).
        let request_uuid = uuid.clone();
        let pending = PendingRequest {
            request_uuid: request_uuid.clone(),
            client,
            pid,
            ttl,
            grant_lock_uuid: grant_overrides.lock_uuid,
            grant_fencing_seed: grant_overrides.fencing_seed,
            keep_locks_after_death,
            kind: PendingKind::Exclusive,
        };
        if force {
            lock.queue.push_front(request_uuid.clone(), pending);
        } else {
            lock.queue.push_back(request_uuid.clone(), pending);
        }
        let depth = lock.queue.len();
        state.maybe_mark_idle(&key);
        self.track_pending(state, client, &key, &request_uuid);
        self.send(
            state,
            client,
            Response::Lock {
                uuid,
                key,
                acquired: false,
                lock_request_count: depth,
                lock_uuid: None,
                fencing_token: None,
                readers_count: Some(0),
                error: None,
            },
        );
    }

    // ---- composite --------------------------------------------------------

    fn handle_composite_lock(&self, state: &mut BrokerState, request: CompositeLockRequest) {
        let CompositeLockRequest {
            client,
            uuid,
            mut keys,
            pid,
            ttl,
            wait,
            grant_overrides,
        } = request;
        crate::routine_id!("ddl-routine-UD_1TQ6n72nYb_GRcW");
        if keys.is_empty() || keys.len() > MAX_COMPOSITE_KEYS {
            self.send(
                state,
                client,
                Response::CompositeLock {
                    uuid,
                    keys,
                    acquired: false,
                    lock_uuid: None,
                    fencing_tokens: None,
                    error: Some(format!(
                        "composite lock requires 1..={MAX_COMPOSITE_KEYS} keys"
                    )),
                },
            );
            return;
        }
        keys.sort();
        keys.dedup();

        // Try to grab everything atomically — fastest path.
        let all_free = keys.iter().all(|k| {
            state
                .locks
                .get(k)
                .map(|s| {
                    s.writer.is_none() && s.readers.is_empty() && s.exclusive_holders.is_empty()
                })
                .unwrap_or(true)
        });

        let composite_lock_uuid = grant_overrides
            .lock_uuid
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        if all_free {
            let mut tokens: BTreeMap<String, u64> = BTreeMap::new();
            let mut max_token: u64 = 0;
            for (idx, k) in keys.iter().enumerate() {
                let lock = state.lock_or_default(k);
                let token = lock.next_or_forced_fencing_token(grant_overrides.token(idx as u64));
                tokens.insert(k.clone(), token);
                lock.exclusive_holders.insert(
                    composite_lock_uuid.clone(),
                    ExclusiveHolder {
                        client,
                        pid,
                        fencing_token: token,
                        keep_locks_after_death: false,
                        composite_member: true,
                    },
                );
                if token > max_token {
                    max_token = token;
                }
            }
            state.observe_fencing_token(max_token);
            for k in &keys {
                state.maybe_mark_idle(k);
            }
            self.track_holder(
                state,
                client,
                &composite_lock_uuid,
                &keys,
                RwHoldKind::Exclusive,
            );
            state.schedule_deadline(
                ttl,
                &composite_lock_uuid,
                &keys,
                RwHoldKind::Exclusive,
                client,
            );
            self.send(
                state,
                client,
                Response::CompositeLock {
                    uuid,
                    keys,
                    acquired: true,
                    lock_uuid: Some(composite_lock_uuid),
                    fencing_tokens: Some(tokens),
                    error: None,
                },
            );
            return;
        }

        // No-wait (try-lock): fail fast on contention without enqueuing, so a
        // caller that doesn't loop can't leak a deferred composite grant.
        if !wait {
            self.send(
                state,
                client,
                Response::CompositeLock {
                    uuid,
                    keys,
                    acquired: false,
                    lock_uuid: None,
                    fencing_tokens: None,
                    error: None,
                },
            );
            return;
        }

        // Otherwise queue on the first (smallest) key. Each grant moves us to
        // the next key, until all granted_keys == all_keys.
        let head = keys[0].clone();
        let pending = PendingRequest {
            request_uuid: uuid.clone(),
            client,
            pid,
            ttl,
            grant_lock_uuid: None,
            grant_fencing_seed: grant_overrides.fencing_seed,
            keep_locks_after_death: false,
            kind: PendingKind::Composite {
                all_keys: keys.clone(),
                remaining_keys: keys.clone(),
                granted_keys: Vec::new(),
                granted_tokens: BTreeMap::new(),
                composite_lock_uuid: composite_lock_uuid.clone(),
            },
        };
        let lock = state.lock_or_default(&head);
        lock.queue.push_back(uuid.clone(), pending);
        state.maybe_mark_idle(&head);
        self.track_pending(state, client, &head, &uuid);

        // Tell the client they're queued.
        self.send(
            state,
            client,
            Response::CompositeLock {
                uuid,
                keys,
                acquired: false,
                lock_uuid: Some(composite_lock_uuid),
                fencing_tokens: None,
                error: None,
            },
        );

        // Critical: `all_free` was false because *some* member is contended,
        // but the smallest key `head` we just queued on may itself be FREE
        // (the contention is on a later key). Nothing will ever emit a release
        // event for an already-free `head`, so without an explicit kick here
        // the waiter would never be woken — a missed-wakeup deadlock. Drive the
        // scheduler now: if `head` is free, `try_grant_composite` grants it and
        // advances the request key-by-key until it reaches the genuinely
        // contended key, where it parks on a lock that *will* fire a release.
        // If `head` is contended this is a no-op and we correctly wait for its
        // release. Ordering: the queued notice is already sent above, so the
        // client observes acquired:false then (later) acquired:true, matching
        // the wait-mode contract. No full grant can happen in this call because
        // at least one member key is still held.
        self.try_grant_next(state, &head);
    }

    // ---- unlock -----------------------------------------------------------

    fn handle_unlock(
        &self,
        state: &mut BrokerState,
        client: ClientId,
        uuid: String,
        keys: Vec<String>,
        lock_uuid: Option<String>,
        force: bool,
    ) {
        crate::routine_id!("ddl-routine-F6gViY4_MAcAx57bwL");
        if keys.is_empty() {
            self.send(
                state,
                client,
                Response::Error {
                    uuid,
                    error: "unlock request requires at least one key".into(),
                },
            );
            return;
        }

        let mut total_unlocked = false;
        let mut last_depth: usize = 0;
        let live_clients: std::collections::HashSet<ClientId> =
            state.clients.keys().copied().collect();
        let can_unlock_target =
            |owner: ClientId| force || owner == client || !live_clients.contains(&owner);

        // Three legitimate unlock variants:
        //   * `lock_uuid: Some(_)` + `force: false` — release exactly that
        //     holder, but only when the holder belongs to this live client.
        //     Detached holders (HTTP / keep-after-death) have no live owning
        //     client, so their lock_uuid remains a bearer token.
        //     Wrong uuid is a no-op (`unlocked: false`).
        //   * `lock_uuid: Some(_)` + `force: true` — release exactly that
        //     holder, but ignore broker-side ownership/identity checks.
        //     If the uuid does not match anyone, this is a "phantom"
        //     unlock and we MUST NOT wipe peer holders just because
        //     `force: true` is set. Surface as `unlocked: false` with a
        //     descriptive error so callers can distinguish it from
        //     success. (Mirrors live-mutex#131 on the Node side.)
        //   * `lock_uuid: None` + `force: true` — operator escape hatch.
        //     Wipe every holder on every requested key. We additionally
        //     clean up `held_lock_uuids` on each wiped holder's owning
        //     client, so a peer client's bookkeeping stays consistent
        //     with the broker's truth (otherwise `drop_client` later
        //     would attempt to release things the broker has already
        //     evicted; harmless today, but a footgun for any future
        //     code path that trusts `held_lock_uuids`).
        let target_lock_uuid: Option<String> = match (lock_uuid, force) {
            (Some(v), _) => Some(v),
            (None, true) => None,
            (None, false) => {
                self.send(
                    state,
                    client,
                    Response::Unlock {
                        uuid,
                        keys,
                        unlocked: false,
                        lock_request_count: 0,
                        error: Some("unlock requires `_uuid` or `force=true`".into()),
                    },
                );
                return;
            }
        };

        // For wipe-all (target == None) we collect (owner_client,
        // lock_uuid) for every holder we evict so we can purge the
        // entries from each owner's `held_lock_uuids`.
        let mut wiped_holder_ownership: Vec<(ClientId, String)> = Vec::new();
        let mut removed_holder_ownership: Vec<(ClientId, String)> = Vec::new();
        let mut ownership_denied = false;

        for key in &keys {
            let Some(lock) = state.locks.get_mut(key) else {
                continue;
            };
            match &target_lock_uuid {
                Some(target) => {
                    let exclusive_owner = lock.exclusive_holders.get(target).map(|h| h.client);
                    let removed_exclusive = match exclusive_owner {
                        Some(owner) if can_unlock_target(owner) => {
                            lock.exclusive_holders.remove(target);
                            removed_holder_ownership.push((owner, target.clone()));
                            true
                        }
                        Some(_) => {
                            ownership_denied = true;
                            false
                        }
                        None => false,
                    };

                    let reader_owner = lock.readers.get(target).map(|h| h.client);
                    let removed_reader = match reader_owner {
                        Some(owner) if can_unlock_target(owner) => {
                            lock.readers.remove(target);
                            removed_holder_ownership.push((owner, target.clone()));
                            true
                        }
                        Some(_) => {
                            ownership_denied = true;
                            false
                        }
                        None => false,
                    };

                    let writer_owner = lock
                        .writer
                        .as_ref()
                        .and_then(|w| (w.lock_uuid == *target).then_some(w.client));
                    let removed_writer = match writer_owner {
                        Some(owner) if can_unlock_target(owner) => {
                            lock.writer = None;
                            removed_holder_ownership.push((owner, target.clone()));
                            true
                        }
                        Some(_) => {
                            ownership_denied = true;
                            false
                        }
                        None => false,
                    };
                    if removed_exclusive || removed_reader || removed_writer {
                        total_unlocked = true;
                    }
                }
                None => {
                    // Wipe-all: snapshot holder ownership BEFORE clearing
                    // so we can fix up bookkeeping after.
                    for (lu, h) in lock.exclusive_holders.iter() {
                        wiped_holder_ownership.push((h.client, lu.clone()));
                    }
                    for (lu, h) in lock.readers.iter() {
                        wiped_holder_ownership.push((h.client, lu.clone()));
                    }
                    if let Some(w) = &lock.writer {
                        wiped_holder_ownership.push((w.client, w.lock_uuid.clone()));
                    }
                    let any_existed = !lock.exclusive_holders.is_empty()
                        || !lock.readers.is_empty()
                        || lock.writer.is_some();
                    lock.exclusive_holders.clear();
                    lock.readers.clear();
                    lock.writer = None;
                    if any_existed {
                        total_unlocked = true;
                    }
                }
            }
            last_depth = lock.queue.len();
        }

        // Drop the holder bookkeeping on every client whose holder we
        // just evicted.
        match &target_lock_uuid {
            Some(_) => {
                if total_unlocked {
                    for (owner_client, lu) in &removed_holder_ownership {
                        if let Some(handle) = state.clients.get_mut(owner_client) {
                            handle.held_lock_uuids.remove(lu);
                        }
                    }
                }
            }
            None => {
                for (owner_client, lu) in &wiped_holder_ownership {
                    if let Some(handle) = state.clients.get_mut(owner_client) {
                        handle.held_lock_uuids.remove(lu);
                    }
                }
            }
        }

        // Now wake waiters on every key we just touched.
        for key in &keys {
            self.try_grant_next(state, key);
        }

        // `force: true` + `lock_uuid: Some(_)` that didn't match anyone
        // is the "phantom" case: surface a structured error so the
        // caller can distinguish a no-op force-unlock from an honest
        // success.
        let error = if !total_unlocked && ownership_denied {
            target_lock_uuid.as_deref().map(|t| {
                format!(
                    "unlock lock_uuid `{t}` is owned by another live client; use force=true to override"
                )
            })
        } else if !total_unlocked && force {
            target_lock_uuid.as_deref().map(|t| {
                format!(
                    "unlock(force=true) lock_uuid `{t}` did not match any current holder for the requested keys"
                )
            })
        } else {
            None
        };

        self.send(
            state,
            client,
            Response::Unlock {
                uuid,
                keys,
                unlocked: total_unlocked,
                lock_request_count: last_depth,
                error,
            },
        );
    }

    // ---- reader/writer ----------------------------------------------------

    fn handle_register_read(
        &self,
        state: &mut BrokerState,
        client: ClientId,
        uuid: String,
        key: String,
        grant_overrides: GrantOverrides,
    ) {
        crate::routine_id!("ddl-routine-20EN0HnEEFCThg4PVw");
        let lock = state.lock_or_default(&key);
        if lock.writer.is_none() && lock.exclusive_holders.is_empty() && lock.queue.is_empty() {
            let token = lock.next_or_forced_fencing_token(grant_overrides.token(0));
            let lock_uuid = grant_overrides
                .lock_uuid
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            lock.readers.insert(
                lock_uuid.clone(),
                RwHolder {
                    client,
                    fencing_token: token,
                    lock_uuid: lock_uuid.clone(),
                },
            );
            let readers = lock.readers.len() as u32;
            state.observe_fencing_token(token);
            state.maybe_mark_idle(&key);
            self.track_holder(
                state,
                client,
                &lock_uuid,
                std::slice::from_ref(&key),
                RwHoldKind::Read,
            );
            self.send(
                state,
                client,
                Response::RegisterReadResult {
                    uuid,
                    key,
                    readers_count: readers,
                    writer_flag: false,
                    granted: true,
                    lock_uuid: Some(lock_uuid),
                    fencing_token: Some(token),
                },
            );
            return;
        }
        // Queue: behind every existing waiter; will be granted when no writer
        // is active and we're at queue head.
        lock.queue.push_back(
            uuid.clone(),
            PendingRequest {
                request_uuid: uuid.clone(),
                client,
                pid: None,
                ttl: None,
                grant_lock_uuid: grant_overrides.lock_uuid,
                grant_fencing_seed: grant_overrides.fencing_seed,
                keep_locks_after_death: false,
                kind: PendingKind::Reader,
            },
        );
        let writer_flag = lock.writer.is_some();
        let readers = lock.readers.len() as u32;
        state.maybe_mark_idle(&key);
        self.track_pending(state, client, &key, &uuid);
        self.send(
            state,
            client,
            Response::RegisterReadResult {
                uuid,
                key,
                readers_count: readers,
                writer_flag,
                granted: false,
                lock_uuid: None,
                fencing_token: None,
            },
        );
    }

    fn handle_register_write(
        &self,
        state: &mut BrokerState,
        client: ClientId,
        uuid: String,
        key: String,
        grant_overrides: GrantOverrides,
    ) {
        crate::routine_id!("ddl-routine-jtFimB2SzQApojR-Xt");
        let lock = state.lock_or_default(&key);
        if lock.writer.is_none() && lock.readers.is_empty() && lock.exclusive_holders.is_empty() {
            let token = lock.next_or_forced_fencing_token(grant_overrides.token(0));
            let lock_uuid = grant_overrides
                .lock_uuid
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            lock.writer = Some(RwHolder {
                client,
                fencing_token: token,
                lock_uuid: lock_uuid.clone(),
            });
            state.observe_fencing_token(token);
            state.maybe_mark_idle(&key);
            self.track_holder(
                state,
                client,
                &lock_uuid,
                std::slice::from_ref(&key),
                RwHoldKind::Write,
            );
            self.send(
                state,
                client,
                Response::RegisterWriteResult {
                    uuid,
                    key,
                    readers_count: 0,
                    writer_flag: true,
                    granted: true,
                    lock_uuid: Some(lock_uuid),
                    fencing_token: Some(token),
                },
            );
            return;
        }
        lock.queue.push_back(
            uuid.clone(),
            PendingRequest {
                request_uuid: uuid.clone(),
                client,
                pid: None,
                ttl: None,
                grant_lock_uuid: grant_overrides.lock_uuid,
                grant_fencing_seed: grant_overrides.fencing_seed,
                keep_locks_after_death: false,
                kind: PendingKind::Writer,
            },
        );
        let writer_flag = lock.writer.is_some();
        let readers = lock.readers.len() as u32;
        state.maybe_mark_idle(&key);
        self.track_pending(state, client, &key, &uuid);
        self.send(
            state,
            client,
            Response::RegisterWriteResult {
                uuid,
                key,
                readers_count: readers,
                writer_flag,
                granted: false,
                lock_uuid: None,
                fencing_token: None,
            },
        );
    }

    fn handle_end_read(
        &self,
        state: &mut BrokerState,
        client: ClientId,
        uuid: String,
        key: String,
    ) {
        crate::routine_id!("ddl-routine-bcRprfoS-qeSiQGC0b");
        if let Some(lock) = state.locks.get_mut(&key) {
            // Drop *any* readers held by this client. Note: a client can hold
            // multiple read leases on the same key, but typical use is one
            // per call.
            let to_remove: Vec<String> = lock
                .readers
                .iter()
                .filter(|(_, h)| h.client == client)
                .map(|(k, _)| k.clone())
                .collect();
            for k in to_remove {
                lock.readers.remove(&k);
                if let Some(handle) = state.clients.get_mut(&client) {
                    handle.held_lock_uuids.remove(&k);
                }
            }
            let readers = lock.readers.len() as u32;
            self.send(
                state,
                client,
                Response::EndReadResult {
                    uuid,
                    key: key.clone(),
                    readers_count: readers,
                },
            );
        } else {
            self.send(
                state,
                client,
                Response::EndReadResult {
                    uuid,
                    key,
                    readers_count: 0,
                },
            );
            return;
        }
        self.try_grant_next(state, &key);
    }

    fn handle_end_write(
        &self,
        state: &mut BrokerState,
        client: ClientId,
        uuid: String,
        key: String,
    ) {
        crate::routine_id!("ddl-routine-HCcGv21IjU5kyJr3vE");
        if let Some(lock) = state.locks.get_mut(&key) {
            if let Some(w) = lock.writer.take_if(|w| w.client == client) {
                if let Some(handle) = state.clients.get_mut(&client) {
                    handle.held_lock_uuids.remove(&w.lock_uuid);
                }
            }
            let writer_flag = lock.writer.is_some();
            let readers = lock.readers.len() as u32;
            self.send(
                state,
                client,
                Response::EndWriteResult {
                    uuid,
                    key: key.clone(),
                    readers_count: readers,
                    writer_flag,
                },
            );
        }
        self.try_grant_next(state, &key);
    }

    // ---- queue scheduling -------------------------------------------------

    /// Inspect the head of `key`'s queue and grant if possible. Repeats while
    /// progress is made (a granted reader allows the next reader to advance).
    /// Always refreshes the lock's `timestamp_emptied` after the loop so the
    /// empty-key prune sweep sees a coherent idle/active flag.
    fn try_grant_next(&self, state: &mut BrokerState, key: &str) {
        crate::routine_id!("ddl-routine-kgo2IA5f14EZoNkFmn");
        loop {
            let made_progress = self.try_grant_once(state, key);
            if !made_progress {
                break;
            }
        }
        state.maybe_mark_idle(key);
    }

    fn try_grant_once(&self, state: &mut BrokerState, key: &str) -> bool {
        crate::routine_id!("ddl-routine-JpvC7GaYO7SXxieWyJ");
        let Some(lock) = state.locks.get_mut(key) else {
            return false;
        };
        let Some((_, head)) = lock.queue.front() else {
            return false;
        };
        match &head.kind {
            PendingKind::Exclusive => {
                if !(lock.writer.is_none()
                    && lock.readers.is_empty()
                    && (lock.exclusive_holders.len() as u32) < lock.max)
                {
                    return false;
                }
                let head = lock.queue.pop_front().expect("just peeked").1;
                let token = lock.next_or_forced_fencing_token(head.grant_fencing_seed);
                let lock_uuid = head
                    .grant_lock_uuid
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                lock.exclusive_holders.insert(
                    lock_uuid.clone(),
                    ExclusiveHolder {
                        client: head.client,
                        pid: head.pid,
                        fencing_token: token,
                        keep_locks_after_death: head.keep_locks_after_death,
                        composite_member: false,
                    },
                );
                let depth = lock.queue.len();
                state.observe_fencing_token(token);
                self.untrack_pending(state, head.client, key, &head.request_uuid);
                self.track_holder(
                    state,
                    head.client,
                    &lock_uuid,
                    &[key.to_string()],
                    RwHoldKind::Exclusive,
                );
                state.schedule_deadline(
                    head.ttl,
                    &lock_uuid,
                    &[key.to_string()],
                    RwHoldKind::Exclusive,
                    head.client,
                );
                self.send(
                    state,
                    head.client,
                    Response::Lock {
                        uuid: head.request_uuid,
                        key: key.to_string(),
                        acquired: true,
                        lock_request_count: depth,
                        lock_uuid: Some(lock_uuid),
                        fencing_token: Some(token),
                        readers_count: Some(0),
                        error: None,
                    },
                );
                true
            }
            PendingKind::Reader => {
                if lock.writer.is_some() || !lock.exclusive_holders.is_empty() {
                    return false;
                }
                let head = lock.queue.pop_front().expect("just peeked").1;
                let token = lock.next_or_forced_fencing_token(head.grant_fencing_seed);
                let lock_uuid = head
                    .grant_lock_uuid
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                lock.readers.insert(
                    lock_uuid.clone(),
                    RwHolder {
                        client: head.client,
                        fencing_token: token,
                        lock_uuid: lock_uuid.clone(),
                    },
                );
                let readers = lock.readers.len() as u32;
                state.observe_fencing_token(token);
                self.untrack_pending(state, head.client, key, &head.request_uuid);
                self.track_holder(
                    state,
                    head.client,
                    &lock_uuid,
                    &[key.to_string()],
                    RwHoldKind::Read,
                );
                self.send(
                    state,
                    head.client,
                    Response::RegisterReadResult {
                        uuid: head.request_uuid,
                        key: key.to_string(),
                        readers_count: readers,
                        writer_flag: false,
                        granted: true,
                        lock_uuid: Some(lock_uuid),
                        fencing_token: Some(token),
                    },
                );
                true
            }
            PendingKind::Writer => {
                if lock.writer.is_some()
                    || !lock.readers.is_empty()
                    || !lock.exclusive_holders.is_empty()
                {
                    return false;
                }
                let head = lock.queue.pop_front().expect("just peeked").1;
                let token = lock.next_or_forced_fencing_token(head.grant_fencing_seed);
                let lock_uuid = head
                    .grant_lock_uuid
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                lock.writer = Some(RwHolder {
                    client: head.client,
                    fencing_token: token,
                    lock_uuid: lock_uuid.clone(),
                });
                state.observe_fencing_token(token);
                self.untrack_pending(state, head.client, key, &head.request_uuid);
                self.track_holder(
                    state,
                    head.client,
                    &lock_uuid,
                    &[key.to_string()],
                    RwHoldKind::Write,
                );
                self.send(
                    state,
                    head.client,
                    Response::RegisterWriteResult {
                        uuid: head.request_uuid,
                        key: key.to_string(),
                        readers_count: 0,
                        writer_flag: true,
                        granted: true,
                        lock_uuid: Some(lock_uuid),
                        fencing_token: Some(token),
                    },
                );
                true
            }
            PendingKind::Composite { .. } => self.try_grant_composite(state, key),
        }
    }

    fn try_grant_composite(&self, state: &mut BrokerState, key: &str) -> bool {
        crate::routine_id!("ddl-routine-mb7TrINchMa7_5XgzQ");
        let Some(lock) = state.locks.get_mut(key) else {
            return false;
        };
        let Some((_, head)) = lock.queue.front() else {
            return false;
        };
        let PendingKind::Composite {
            all_keys: _,
            remaining_keys,
            granted_keys: _,
            granted_tokens: _,
            composite_lock_uuid: _,
        } = &head.kind
        else {
            return false;
        };
        // Confirm we are queued on the right key (it must be the first
        // remaining one).
        let next_target = remaining_keys.first().cloned();
        let Some(target) = next_target else {
            return false;
        };
        if target != key {
            // Defensive: shouldn't happen but bail rather than panic.
            return false;
        }
        if !(lock.writer.is_none() && lock.readers.is_empty() && lock.exclusive_holders.is_empty())
        {
            return false;
        }

        let pop = lock.queue.pop_front().expect("just peeked").1;
        let PendingKind::Composite {
            all_keys,
            mut remaining_keys,
            mut granted_keys,
            mut granted_tokens,
            composite_lock_uuid,
        } = pop.kind
        else {
            return false;
        };

        let token_offset = all_keys
            .iter()
            .position(|candidate| candidate == key)
            .unwrap_or(0) as u64;
        let token = lock.next_or_forced_fencing_token(
            pop.grant_fencing_seed
                .map(|seed| seed.saturating_add(token_offset)),
        );
        lock.exclusive_holders.insert(
            composite_lock_uuid.clone(),
            ExclusiveHolder {
                client: pop.client,
                pid: pop.pid,
                fencing_token: token,
                keep_locks_after_death: false,
                composite_member: true,
            },
        );
        granted_tokens.insert(key.to_string(), token);
        granted_keys.push(key.to_string());
        remaining_keys.remove(0);
        state.observe_fencing_token(token);

        self.untrack_pending(state, pop.client, key, &pop.request_uuid);

        // Register the partial hold against the client immediately so a
        // mid-flight disconnect can roll it back via `drop_client`. We
        // re-register at the final grant too — the call is idempotent
        // (same lock_uuid + same `all_keys` payload). Without this, a
        // client that drops while holding `granted_keys` but still queued
        // on a remaining key would leak those grants: `drop_client` walks
        // `held_lock_uuids` (which only contained fully-granted
        // composites) and `pending_request_uuids` (which only handles the
        // queued tail), missing the partial state in between.
        self.track_holder(
            state,
            pop.client,
            &composite_lock_uuid,
            &all_keys,
            RwHoldKind::Exclusive,
        );

        if remaining_keys.is_empty() {
            state.schedule_deadline(
                pop.ttl,
                &composite_lock_uuid,
                &all_keys,
                RwHoldKind::Exclusive,
                pop.client,
            );
            self.send(
                state,
                pop.client,
                Response::CompositeLock {
                    uuid: pop.request_uuid,
                    keys: all_keys,
                    acquired: true,
                    lock_uuid: Some(composite_lock_uuid),
                    fencing_tokens: Some(granted_tokens),
                    error: None,
                },
            );
            return true;
        }

        // Move to the next key. Queue the same composite request there.
        let next_key = remaining_keys[0].clone();
        let next_lock = state.lock_or_default(&next_key);
        next_lock.queue.push_back(
            pop.request_uuid.clone(),
            PendingRequest {
                request_uuid: pop.request_uuid.clone(),
                client: pop.client,
                pid: pop.pid,
                ttl: pop.ttl,
                grant_lock_uuid: None,
                grant_fencing_seed: pop.grant_fencing_seed,
                keep_locks_after_death: pop.keep_locks_after_death,
                kind: PendingKind::Composite {
                    all_keys,
                    remaining_keys,
                    granted_keys,
                    granted_tokens,
                    composite_lock_uuid,
                },
            },
        );
        self.track_pending(state, pop.client, &next_key, &pop.request_uuid);
        // Try the next key immediately in case it's also free.
        self.try_grant_next(state, &next_key);
        true
    }

    // ---- bookkeeping helpers ----------------------------------------------

    fn track_holder(
        &self,
        state: &mut BrokerState,
        client: ClientId,
        lock_uuid: &str,
        keys: &[String],
        kind: RwHoldKind,
    ) {
        crate::routine_id!("ddl-routine-nDX6C_jmUhiZ6iUfwe");
        if let Some(handle) = state.clients.get_mut(&client) {
            handle.held_lock_uuids.insert(
                lock_uuid.to_string(),
                KeysOfLock {
                    keys: keys.to_vec(),
                    rw_kind: kind,
                },
            );
        }
    }

    fn track_pending(
        &self,
        state: &mut BrokerState,
        client: ClientId,
        key: &str,
        request_uuid: &str,
    ) {
        crate::routine_id!("ddl-routine-z8LhkAyw33THqG3Cfh");
        if let Some(handle) = state.clients.get_mut(&client) {
            handle
                .pending_request_uuids
                .push((key.to_string(), request_uuid.to_string()));
        }
    }

    fn untrack_pending(
        &self,
        state: &mut BrokerState,
        client: ClientId,
        key: &str,
        request_uuid: &str,
    ) {
        crate::routine_id!("ddl-routine-n491ef9clCYZzZBQnH");
        if let Some(handle) = state.clients.get_mut(&client) {
            handle
                .pending_request_uuids
                .retain(|(k, u)| !(k == key && u == request_uuid));
        }
    }

    fn send(&self, state: &BrokerState, client: ClientId, response: Response) {
        crate::routine_id!("ddl-routine-u2hHnsw12uVIwzoDO9");
        if let Some(observer) = &self.response_observer {
            observer(&response);
        }
        if let Some(handle) = state.clients.get(&client) {
            let _ = handle.sender.send(response);
        }
    }

    /// Push a `Response` directly to a connected client. Returns false if the
    /// client is gone or its outbound channel is closed. Used by transport
    /// listeners to surface protocol-level errors (e.g. malformed JSON, auth
    /// failures) without a fake `Request` round-trip.
    pub fn try_send(&self, client: ClientId, response: Response) -> bool {
        crate::routine_id!("ddl-routine-63KLXrLYAbC6O95P0d");
        let state = self.state.lock();
        match state.clients.get(&client) {
            Some(handle) => handle.sender.send(response).is_ok(),
            None => false,
        }
    }

    /// Remove a lock_uuid from a client's "held" bookkeeping without touching
    /// the underlying lock state. Used by the HTTP layer: each HTTP /v1/lock
    /// request runs in an ephemeral broker client, but the granted lock must
    /// outlive that client (the next HTTP /v1/unlock will release it via
    /// lock_uuid). Detaching here prevents `drop_client` from releasing it.
    pub fn detach_lock_from_client(&self, client: ClientId, lock_uuid: &str) {
        crate::routine_id!("ddl-routine-dIyxZKGdksBxszWr_1");
        let mut state = self.state.lock();
        if let Some(handle) = state.clients.get_mut(&client) {
            handle.held_lock_uuids.remove(lock_uuid);
        }
    }

    /// Convert a holder to a detached bearer-token holder by lock UUID.
    ///
    /// This is used when BrokerRaft replays a cached HTTP acquire response:
    /// the original ephemeral client may still be registered if its task was
    /// interrupted between observing the broker response and running the normal
    /// detach/drop cleanup. A replayed successful acquire must still be
    /// unlockable by lock UUID from a later HTTP request.
    pub fn detach_lock_owner(&self, lock_uuid: &str) {
        crate::routine_id!("ddl-routine-broker-detach-lock-owner-1");
        let mut state = self.state.lock();
        let mut touched_clients = std::collections::HashSet::<ClientId>::new();
        for lock in state.locks.values_mut() {
            if let Some(holder) = lock.exclusive_holders.get_mut(lock_uuid) {
                touched_clients.insert(holder.client);
                holder.client = SNAPSHOT_DETACHED_CLIENT;
            }
            if let Some(holder) = lock.readers.get_mut(lock_uuid) {
                touched_clients.insert(holder.client);
                holder.client = SNAPSHOT_DETACHED_CLIENT;
            }
            if let Some(writer) = lock
                .writer
                .as_mut()
                .filter(|writer| writer.lock_uuid == lock_uuid)
            {
                touched_clients.insert(writer.client);
                writer.client = SNAPSHOT_DETACHED_CLIENT;
            }
        }
        for client in touched_clients {
            if client == SNAPSHOT_DETACHED_CLIENT {
                continue;
            }
            let should_remove = if let Some(handle) = state.clients.get_mut(&client) {
                handle.held_lock_uuids.remove(lock_uuid);
                handle.held_lock_uuids.is_empty() && handle.pending_request_uuids.is_empty()
            } else {
                false
            };
            if should_remove {
                state.clients.remove(&client);
            }
        }
    }

    /// Snapshot of broker counters used by `/metrics`.
    pub fn metrics(&self) -> BrokerMetrics {
        crate::routine_id!("ddl-routine-oQmRZkKSUjdFlC2hsV");
        let state = self.state.lock();
        broker_metrics_from_state(&state)
    }

    pub(crate) fn snapshot_for_raft(&self) -> Result<serde_json::Value, String> {
        crate::routine_id!("ddl-routine-broker-snapshot-for-raft-1");
        let state = self.state.lock();
        let metrics = broker_metrics_from_state(&state);
        let now = Instant::now();
        let mut locks = state
            .locks
            .iter()
            .map(|(key, lock)| {
                let mut exclusive_holders = lock
                    .exclusive_holders
                    .iter()
                    .map(|(lock_uuid, holder)| BrokerRaftExclusiveHolderSnapshot {
                        lock_uuid: lock_uuid.clone(),
                        pid: holder.pid,
                        fencing_token: holder.fencing_token,
                        keep_locks_after_death: holder.keep_locks_after_death,
                        composite_member: holder.composite_member,
                    })
                    .collect::<Vec<_>>();
                exclusive_holders.sort_by(|a, b| a.lock_uuid.cmp(&b.lock_uuid));

                let mut readers = lock
                    .readers
                    .iter()
                    .map(|(lock_uuid, holder)| BrokerRaftRwHolderSnapshot {
                        lock_uuid: lock_uuid.clone(),
                        fencing_token: holder.fencing_token,
                    })
                    .collect::<Vec<_>>();
                readers.sort_by(|a, b| a.lock_uuid.cmp(&b.lock_uuid));

                BrokerRaftLockSnapshot {
                    key: key.clone(),
                    max: lock.max,
                    fencing_counter: lock.fencing_counter,
                    exclusive_holders,
                    readers,
                    writer: lock
                        .writer
                        .as_ref()
                        .map(|holder| BrokerRaftRwHolderSnapshot {
                            lock_uuid: holder.lock_uuid.clone(),
                            fencing_token: holder.fencing_token,
                        }),
                    queue: lock
                        .queue
                        .iter()
                        .map(|(_, pending)| BrokerRaftPendingRequestSnapshot {
                            request_uuid: pending.request_uuid.clone(),
                            client_id: pending.client,
                            pid: pending.pid,
                            ttl_ms: pending.ttl.map(duration_ms_u64),
                            grant_lock_uuid: pending.grant_lock_uuid.clone(),
                            grant_fencing_seed: pending.grant_fencing_seed,
                            keep_locks_after_death: pending.keep_locks_after_death,
                            kind: pending_kind_snapshot_from(&pending.kind),
                        })
                        .collect(),
                    idle: lock.is_idle(),
                }
            })
            .collect::<Vec<_>>();
        locks.sort_by(|a, b| a.key.cmp(&b.key));

        let mut deadlines = state
            .deadlines
            .iter()
            .filter_map(|((deadline, _), entry)| {
                if !deadline_entry_is_still_held(&state, entry) {
                    return None;
                }
                Some(BrokerRaftDeadlineSnapshot {
                    lock_uuid: entry.lock_uuid.clone(),
                    keys: entry.keys.clone(),
                    kind: entry.kind.clone(),
                    remaining_ms: duration_ms_u64(deadline.saturating_duration_since(now)),
                })
            })
            .collect::<Vec<_>>();
        deadlines.sort_by(|a, b| {
            a.lock_uuid
                .cmp(&b.lock_uuid)
                .then_with(|| a.keys.cmp(&b.keys))
        });

        serde_json::to_value(BrokerRaftSnapshot {
            schema_version: 1,
            metrics: metrics.into(),
            locks,
            deadlines,
        })
        .map_err(|err| err.to_string())
    }

    pub(crate) fn validate_raft_snapshot_payload(
        payload: &serde_json::Value,
    ) -> Result<(), String> {
        crate::routine_id!("ddl-routine-broker-validate-raft-snapshot-1");
        if let Some(snapshot) = payload.get("broker") {
            let snapshot: BrokerRaftSnapshot =
                serde_json::from_value(snapshot.clone()).map_err(|err| err.to_string())?;
            validate_broker_raft_snapshot(&snapshot)
        } else {
            Self::validate_idle_snapshot_payload(payload)
        }
    }

    pub(crate) fn install_raft_snapshot(&self, payload: &serde_json::Value) -> Result<(), String> {
        crate::routine_id!("ddl-routine-broker-install-raft-snapshot-1");
        if let Some(snapshot) = payload.get("broker") {
            let snapshot: BrokerRaftSnapshot =
                serde_json::from_value(snapshot.clone()).map_err(|err| err.to_string())?;
            validate_broker_raft_snapshot(&snapshot)?;
            self.install_broker_raft_snapshot(snapshot)
        } else {
            self.install_idle_snapshot(payload)
        }
    }

    pub(crate) fn validate_idle_snapshot_payload(
        payload: &serde_json::Value,
    ) -> Result<(), String> {
        crate::routine_id!("ddl-routine-broker-validate-idle-snapshot-1");
        let metrics = payload
            .get("metrics")
            .ok_or_else(|| "idle broker snapshot is missing metrics".to_string())?;
        for field in ["holders", "waiters", "pendingDeadlines"] {
            let value = metrics
                .get(field)
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| format!("idle broker snapshot missing numeric metrics.{field}"))?;
            if value != 0 {
                return Err(format!(
                    "cannot install non-idle broker snapshot: metrics.{field}={value}"
                ));
            }
        }
        Ok(())
    }

    fn install_broker_raft_snapshot(&self, snapshot: BrokerRaftSnapshot) -> Result<(), String> {
        crate::routine_id!("ddl-routine-broker-install-broker-raft-snapshot-1");
        let mut state = self.state.lock();
        let config = state.config.clone();
        let started_at = state.started_at;
        let now = Instant::now();
        let mut next = BrokerState::new(config);
        next.started_at = started_at;
        next.ttl_evictions_total = snapshot.metrics.ttl_evictions_total;
        next.concurrency_cap_clamps_total = snapshot.metrics.concurrency_cap_clamps_total;
        next.fencing_watermark = snapshot.metrics.fencing_watermark;
        next.idle_keys_pruned_total = snapshot.metrics.idle_keys_pruned_total;

        for lock_snapshot in snapshot.locks {
            let mut lock = LockState::new(
                lock_snapshot
                    .max
                    .max(1)
                    .min(next.config.max_concurrency_cap.max(1)),
            );
            lock.fencing_counter = lock_snapshot.fencing_counter;
            lock.timestamp_emptied = None;

            for holder in lock_snapshot.exclusive_holders {
                lock.fencing_counter = lock.fencing_counter.max(holder.fencing_token);
                next.fencing_watermark = next.fencing_watermark.max(holder.fencing_token);
                lock.exclusive_holders.insert(
                    holder.lock_uuid,
                    ExclusiveHolder {
                        client: SNAPSHOT_DETACHED_CLIENT,
                        pid: holder.pid,
                        fencing_token: holder.fencing_token,
                        keep_locks_after_death: holder.keep_locks_after_death,
                        composite_member: holder.composite_member,
                    },
                );
            }
            for holder in lock_snapshot.readers {
                lock.fencing_counter = lock.fencing_counter.max(holder.fencing_token);
                next.fencing_watermark = next.fencing_watermark.max(holder.fencing_token);
                lock.readers.insert(
                    holder.lock_uuid.clone(),
                    RwHolder {
                        client: SNAPSHOT_DETACHED_CLIENT,
                        fencing_token: holder.fencing_token,
                        lock_uuid: holder.lock_uuid,
                    },
                );
            }
            if let Some(holder) = lock_snapshot.writer {
                lock.fencing_counter = lock.fencing_counter.max(holder.fencing_token);
                next.fencing_watermark = next.fencing_watermark.max(holder.fencing_token);
                lock.writer = Some(RwHolder {
                    client: SNAPSHOT_DETACHED_CLIENT,
                    fencing_token: holder.fencing_token,
                    lock_uuid: holder.lock_uuid,
                });
            }
            for waiter in lock_snapshot.queue {
                next.next_client_id = next.next_client_id.max(waiter.client_id.saturating_add(1));
                let request_uuid = waiter.request_uuid;
                lock.queue.push_back(
                    request_uuid.clone(),
                    PendingRequest {
                        request_uuid,
                        client: waiter.client_id,
                        pid: waiter.pid,
                        ttl: waiter.ttl_ms.map(Duration::from_millis),
                        grant_lock_uuid: waiter.grant_lock_uuid,
                        grant_fencing_seed: waiter.grant_fencing_seed,
                        keep_locks_after_death: waiter.keep_locks_after_death,
                        kind: pending_kind_from_snapshot(waiter.kind),
                    },
                );
            }
            next.fencing_watermark = next.fencing_watermark.max(lock.fencing_counter);
            if lock.is_idle() && lock_snapshot.idle {
                lock.timestamp_emptied = Some(now);
            }
            next.locks.insert(lock_snapshot.key, lock);
        }

        for deadline in snapshot.deadlines {
            if !snapshot_deadline_matches_state(&next, &deadline) {
                return Err(format!(
                    "broker snapshot deadline for `{}` does not match restored holder state",
                    deadline.lock_uuid
                ));
            }
            next.deadline_seq = next.deadline_seq.wrapping_add(1);
            let deadline_at = now
                .checked_add(Duration::from_millis(deadline.remaining_ms))
                .unwrap_or(now);
            next.deadlines.insert(
                (deadline_at, next.deadline_seq),
                DeadlineEntry {
                    lock_uuid: deadline.lock_uuid,
                    keys: deadline.keys,
                    kind: deadline.kind,
                    client: SNAPSHOT_DETACHED_CLIENT,
                },
            );
        }

        *state = next;
        Ok(())
    }

    pub(crate) fn install_idle_snapshot(&self, payload: &serde_json::Value) -> Result<(), String> {
        crate::routine_id!("ddl-routine-broker-install-idle-snapshot-1");
        Self::validate_idle_snapshot_payload(payload)?;
        let metrics = payload
            .get("metrics")
            .ok_or_else(|| "idle broker snapshot is missing metrics".to_string())?;
        let mut state = self.state.lock();
        let config = state.config.clone();
        let started_at = state.started_at;
        let mut next = BrokerState::new(config);
        next.started_at = started_at;
        next.ttl_evictions_total = metrics_u64(metrics, "ttlEvictionsTotal");
        next.concurrency_cap_clamps_total = metrics_u64(metrics, "concurrencyCapClampsTotal");
        next.fencing_watermark = metrics_u64(metrics, "fencingWatermark");
        next.idle_keys_pruned_total = metrics_u64(metrics, "idleKeysPrunedTotal");
        *state = next;
        Ok(())
    }

    /// `Instant` at which this broker started accepting requests. Used by
    /// the HTML status page (upstream `live-mutex#108`) to render an
    /// uptime string. Cheap — single mutex + copy.
    pub fn started_at(&self) -> Instant {
        crate::routine_id!("ddl-routine-9Z1Ac1EJ5x103fNjCk");
        self.state.lock().started_at
    }

    /// Top `n` keys by current contention (`holders + waiters`),
    /// descending. Cheap-ish — walks `state.locks` once under the
    /// mutex, then partial-sorts. Intended for the HTML status page;
    /// don't call from a hot path. Returns at most `n` entries; ties
    /// are broken arbitrarily (HashMap iteration order).
    pub fn top_keys(&self, n: usize) -> Vec<KeyContentionSnapshot> {
        crate::routine_id!("ddl-routine-S2ORKahTJb6iVwyYpY");
        if n == 0 {
            return Vec::new();
        }
        let state = self.state.lock();
        let mut snapshots: Vec<KeyContentionSnapshot> = state
            .locks
            .iter()
            .map(|(key, lock)| {
                let exclusive = lock.exclusive_holders.len() as u64;
                let readers = lock.readers.len() as u64;
                let writer = lock.writer.iter().count() as u64;
                let waiters = lock.queue.len() as u64;
                KeyContentionSnapshot {
                    key: key.clone(),
                    exclusive_holders: exclusive,
                    readers,
                    writers: writer,
                    waiters,
                    fencing_counter: lock.fencing_counter,
                    max: lock.max,
                }
            })
            .filter(|s| s.exclusive_holders + s.readers + s.writers + s.waiters > 0)
            .collect();
        snapshots.sort_by(|a, b| {
            let a_score = a.exclusive_holders + a.readers + a.writers + a.waiters;
            let b_score = b.exclusive_holders + b.readers + b.writers + b.waiters;
            b_score.cmp(&a_score).then_with(|| a.key.cmp(&b.key))
        });
        snapshots.truncate(n);
        snapshots
    }

    /// Sweep the deadline index and force-release every lock whose TTL
    /// has expired by `now`. Returns the number of locks evicted on this
    /// pass. Public so tests can drive eviction synchronously without
    /// waiting for the background sweeper.
    ///
    /// This is the heart of the upstream
    /// [`live-mutex#13`](https://github.com/ORESoftware/live-mutex/issues/13)
    /// optimization: a single pass over a sorted index, regardless of
    /// how many locks are currently held with TTLs.
    pub fn tick_ttl(&self, now: Instant) -> usize {
        crate::routine_id!("ddl-routine-XXcJ382G-X7FQpx0YO");
        let mut state = self.state.lock();
        // Pop everything in `..= (now, u64::MAX)` from the BTreeMap. We
        // collect the keys first to avoid holding a borrow on
        // `state.deadlines` while we mutate `state.locks`.
        let cutoff = (now, u64::MAX);
        let expired_keys: Vec<(Instant, u64)> =
            state.deadlines.range(..=cutoff).map(|(k, _)| *k).collect();

        let mut evicted_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut evicted_count: usize = 0;

        for k in expired_keys {
            let entry = match state.deadlines.remove(&k) {
                Some(e) => e,
                None => continue,
            };

            // Lazy-deletion check: did the holder release before we got
            // here? Look up the lock_uuid in the first key's lock state.
            // If it's gone, this deadline is stale — skip silently.
            let still_held = entry.keys.iter().any(|k| {
                state
                    .locks
                    .get(k)
                    .map(|s| match entry.kind {
                        RwHoldKind::Exclusive => s.exclusive_holders.contains_key(&entry.lock_uuid),
                        RwHoldKind::Read => s.readers.contains_key(&entry.lock_uuid),
                        RwHoldKind::Write => s
                            .writer
                            .as_ref()
                            .is_some_and(|w| w.lock_uuid == entry.lock_uuid),
                    })
                    .unwrap_or(false)
            });
            if !still_held {
                continue;
            }

            // Force-release every key this lock_uuid covers.
            for key in &entry.keys {
                if let Some(lock) = state.locks.get_mut(key) {
                    match entry.kind {
                        RwHoldKind::Exclusive => {
                            lock.exclusive_holders.remove(&entry.lock_uuid);
                        }
                        RwHoldKind::Read => {
                            lock.readers.remove(&entry.lock_uuid);
                        }
                        RwHoldKind::Write => {
                            if lock
                                .writer
                                .as_ref()
                                .is_some_and(|w| w.lock_uuid == entry.lock_uuid)
                            {
                                lock.writer = None;
                            }
                        }
                    }
                }
                evicted_keys.insert(key.clone());
            }

            // Untrack from the holding client (if it's still around).
            if let Some(handle) = state.clients.get_mut(&entry.client) {
                handle.held_lock_uuids.remove(&entry.lock_uuid);
            }

            evicted_count += 1;
            state.ttl_evictions_total = state.ttl_evictions_total.wrapping_add(1);
        }

        // Try to grant the next waiter on every key we touched. Using a
        // set so we don't grant twice for composite-spanning evictions.
        for key in evicted_keys {
            self.try_grant_next(&mut state, &key);
        }

        // Empty-key prune sweep. Walk locks once, drop any that have
        // been idle past `idle_key_grace`. Cross-incarnation fencing
        // monotonicity is preserved because `lock_or_default` seeds
        // every freshly-materialised LockState from
        // `state.fencing_watermark`, which is bumped on every grant.
        //
        // Cost: O(N_keys) per tick. For brokers that habitually
        // accumulate millions of distinct keys this can be revisited
        // by populating a `BTreeSet<(Instant, String)>` alongside
        // `timestamp_emptied` for an O(log N) range query, but the
        // simple walk keeps the data structure surface small and
        // sits at a small fraction of the deadline-sweep cost.
        let grace = state.config.idle_key_grace;
        if !grace.is_zero() {
            if let Some(cutoff) = now.checked_sub(grace) {
                let prune_keys: Vec<String> = state
                    .locks
                    .iter()
                    .filter_map(|(k, l)| match l.timestamp_emptied {
                        Some(t) if t <= cutoff && l.is_idle() => Some(k.clone()),
                        _ => None,
                    })
                    .collect();
                for k in &prune_keys {
                    state.locks.remove(k);
                }
                state.idle_keys_pruned_total = state
                    .idle_keys_pruned_total
                    .wrapping_add(prune_keys.len() as u64);
            }
        }

        evicted_count
    }

    /// Spawn the periodic TTL sweep loop. Must be called from inside a
    /// tokio runtime. Returns the JoinHandle so callers can abort on
    /// shutdown.
    ///
    /// One task. One timer. The timer fires every
    /// `BrokerConfig.ttl_sweep_interval` and processes every expired
    /// holder in a single pass — independent of how many locks are
    /// currently held with TTLs. This is the structural fix upstream
    /// [`live-mutex#13`](https://github.com/ORESoftware/live-mutex/issues/13)
    /// asks for ("instead create a setTimeout, every 10 ms or so").
    ///
    /// If `ttl_sweep_interval == Duration::ZERO` the sweeper is disabled
    /// and the returned handle resolves immediately — the broker will
    /// still accept TTLs but will only evict when callers manually
    /// invoke `tick_ttl`.
    pub fn spawn_ttl_sweeper(&self) -> tokio::task::JoinHandle<()> {
        crate::routine_id!("ddl-routine-uw6ZxhiKx_XgdpHkDa");
        let interval = self.state.lock().config.ttl_sweep_interval;
        let me = self.clone();
        tokio::spawn(async move {
            if interval.is_zero() {
                return;
            }
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Skip the immediate first tick; we want to sweep on the
            // *next* boundary, not synchronously at startup.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let now = Instant::now();
                me.tick_ttl(now);
            }
        })
    }
}

fn metrics_u64(metrics: &serde_json::Value, field: &str) -> u64 {
    crate::routine_id!("ddl-routine-broker-snapshot-metric-u64-1");
    metrics
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

fn broker_metrics_from_state(state: &BrokerState) -> BrokerMetrics {
    crate::routine_id!("ddl-routine-broker-metrics-from-state-1");
    let mut total_holders = 0u64;
    let mut total_waiters = 0u64;
    for lock in state.locks.values() {
        total_holders +=
            (lock.exclusive_holders.len() + lock.readers.len() + lock.writer.iter().count()) as u64;
        total_waiters += lock.queue.len() as u64;
    }
    BrokerMetrics {
        keys: state.locks.len() as u64,
        holders: total_holders,
        waiters: total_waiters,
        clients: state.clients.len() as u64,
        pending_deadlines: state.deadlines.len() as u64,
        ttl_evictions_total: state.ttl_evictions_total,
        max_concurrency_cap: state.config.max_concurrency_cap,
        concurrency_cap_clamps_total: state.concurrency_cap_clamps_total,
        fencing_watermark: state.fencing_watermark,
        idle_keys_pruned_total: state.idle_keys_pruned_total,
    }
}

fn duration_ms_u64(duration: Duration) -> u64 {
    crate::routine_id!("ddl-routine-broker-duration-ms-u64-1");
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn pending_kind_snapshot_from(kind: &PendingKind) -> BrokerRaftPendingKindSnapshot {
    crate::routine_id!("ddl-routine-broker-pending-kind-snapshot-from-1");
    match kind {
        PendingKind::Exclusive => BrokerRaftPendingKindSnapshot::Exclusive,
        PendingKind::Reader => BrokerRaftPendingKindSnapshot::Reader,
        PendingKind::Writer => BrokerRaftPendingKindSnapshot::Writer,
        PendingKind::Composite {
            all_keys,
            remaining_keys,
            granted_keys,
            granted_tokens,
            composite_lock_uuid,
        } => BrokerRaftPendingKindSnapshot::Composite {
            all_keys: all_keys.clone(),
            remaining_keys: remaining_keys.clone(),
            granted_keys: granted_keys.clone(),
            granted_tokens: granted_tokens.clone(),
            composite_lock_uuid: composite_lock_uuid.clone(),
        },
    }
}

fn pending_kind_from_snapshot(kind: BrokerRaftPendingKindSnapshot) -> PendingKind {
    crate::routine_id!("ddl-routine-broker-pending-kind-from-snapshot-1");
    match kind {
        BrokerRaftPendingKindSnapshot::Exclusive => PendingKind::Exclusive,
        BrokerRaftPendingKindSnapshot::Reader => PendingKind::Reader,
        BrokerRaftPendingKindSnapshot::Writer => PendingKind::Writer,
        BrokerRaftPendingKindSnapshot::Composite {
            all_keys,
            remaining_keys,
            granted_keys,
            granted_tokens,
            composite_lock_uuid,
        } => PendingKind::Composite {
            all_keys,
            remaining_keys,
            granted_keys,
            granted_tokens,
            composite_lock_uuid,
        },
    }
}

fn deadline_entry_is_still_held(state: &BrokerState, entry: &DeadlineEntry) -> bool {
    crate::routine_id!("ddl-routine-broker-deadline-entry-held-1");
    entry.keys.iter().any(|key| {
        state
            .locks
            .get(key)
            .map(|lock| lock_contains_uuid_for_kind(lock, &entry.lock_uuid, &entry.kind))
            .unwrap_or(false)
    })
}

fn lock_contains_uuid_for_kind(lock: &LockState, lock_uuid: &str, kind: &RwHoldKind) -> bool {
    crate::routine_id!("ddl-routine-broker-lock-contains-kind-1");
    match kind {
        RwHoldKind::Exclusive => lock.exclusive_holders.contains_key(lock_uuid),
        RwHoldKind::Read => lock.readers.contains_key(lock_uuid),
        RwHoldKind::Write => lock
            .writer
            .as_ref()
            .is_some_and(|writer| writer.lock_uuid == lock_uuid),
    }
}

fn validate_broker_raft_snapshot(snapshot: &BrokerRaftSnapshot) -> Result<(), String> {
    crate::routine_id!("ddl-routine-broker-validate-broker-raft-snapshot-1");
    if snapshot.schema_version != 1 {
        return Err(format!(
            "unsupported broker snapshot schema version {}",
            snapshot.schema_version
        ));
    }
    let mut keys = std::collections::BTreeSet::new();
    let mut total_waiters = 0u64;
    for lock in &snapshot.locks {
        if lock.key.is_empty() {
            return Err("broker snapshot contains an empty lock key".into());
        }
        if !keys.insert(lock.key.clone()) {
            return Err(format!(
                "broker snapshot contains duplicate lock key `{}`",
                lock.key
            ));
        }
        if lock.max == 0 {
            return Err(format!(
                "broker snapshot lock `{}` has invalid max=0",
                lock.key
            ));
        }
        let mut lock_uuids = std::collections::BTreeSet::new();
        for holder in &lock.exclusive_holders {
            if holder.lock_uuid.is_empty() {
                return Err(format!(
                    "broker snapshot lock `{}` has an empty exclusive holder uuid",
                    lock.key
                ));
            }
            if !lock_uuids.insert(holder.lock_uuid.clone()) {
                return Err(format!(
                    "broker snapshot lock `{}` repeats holder uuid `{}`",
                    lock.key, holder.lock_uuid
                ));
            }
        }
        for holder in &lock.readers {
            if holder.lock_uuid.is_empty() {
                return Err(format!(
                    "broker snapshot lock `{}` has an empty reader uuid",
                    lock.key
                ));
            }
            if !lock_uuids.insert(holder.lock_uuid.clone()) {
                return Err(format!(
                    "broker snapshot lock `{}` repeats holder uuid `{}`",
                    lock.key, holder.lock_uuid
                ));
            }
        }
        if let Some(writer) = &lock.writer {
            if writer.lock_uuid.is_empty() {
                return Err(format!(
                    "broker snapshot lock `{}` has an empty writer uuid",
                    lock.key
                ));
            }
            if !lock_uuids.insert(writer.lock_uuid.clone()) {
                return Err(format!(
                    "broker snapshot lock `{}` repeats holder uuid `{}`",
                    lock.key, writer.lock_uuid
                ));
            }
        }
        if lock.idle
            && (!lock.exclusive_holders.is_empty()
                || !lock.readers.is_empty()
                || lock.writer.is_some()
                || !lock.queue.is_empty())
        {
            return Err(format!(
                "broker snapshot lock `{}` is marked idle while it has active state",
                lock.key
            ));
        }
        let mut request_uuids = std::collections::BTreeSet::new();
        for waiter in &lock.queue {
            total_waiters = total_waiters.saturating_add(1);
            validate_broker_raft_waiter_snapshot(&lock.key, waiter)?;
            if !request_uuids.insert(waiter.request_uuid.clone()) {
                return Err(format!(
                    "broker snapshot lock `{}` repeats queued request uuid `{}`",
                    lock.key, waiter.request_uuid
                ));
            }
        }
    }
    if snapshot.metrics.waiters != total_waiters {
        return Err(format!(
            "broker snapshot waiters metric mismatch: metrics.waiters={} queued={}",
            snapshot.metrics.waiters, total_waiters
        ));
    }
    for deadline in &snapshot.deadlines {
        if deadline.lock_uuid.is_empty() {
            return Err("broker snapshot deadline has an empty lock uuid".into());
        }
        if deadline.keys.is_empty() {
            return Err(format!(
                "broker snapshot deadline for `{}` has no keys",
                deadline.lock_uuid
            ));
        }
        if deadline.keys.iter().any(|key| key.is_empty()) {
            return Err(format!(
                "broker snapshot deadline for `{}` contains an empty key",
                deadline.lock_uuid
            ));
        }
        if !snapshot_deadline_matches_locks(&snapshot.locks, deadline) {
            return Err(format!(
                "broker snapshot deadline for `{}` does not match restored holder state",
                deadline.lock_uuid
            ));
        }
    }
    Ok(())
}

fn validate_broker_raft_waiter_snapshot(
    lock_key: &str,
    waiter: &BrokerRaftPendingRequestSnapshot,
) -> Result<(), String> {
    crate::routine_id!("ddl-routine-broker-validate-broker-raft-waiter-snapshot-1");
    if waiter.request_uuid.is_empty() {
        return Err(format!(
            "broker snapshot lock `{lock_key}` has an empty queued request uuid"
        ));
    }
    if waiter
        .grant_lock_uuid
        .as_ref()
        .is_some_and(|lock_uuid| lock_uuid.is_empty())
    {
        return Err(format!(
            "broker snapshot lock `{lock_key}` has an empty queued grant lock uuid"
        ));
    }
    match &waiter.kind {
        BrokerRaftPendingKindSnapshot::Exclusive
        | BrokerRaftPendingKindSnapshot::Reader
        | BrokerRaftPendingKindSnapshot::Writer => Ok(()),
        BrokerRaftPendingKindSnapshot::Composite {
            all_keys,
            remaining_keys,
            granted_keys,
            granted_tokens,
            composite_lock_uuid,
        } => {
            if composite_lock_uuid.is_empty() {
                return Err(format!(
                    "broker snapshot lock `{lock_key}` has an empty composite lock uuid"
                ));
            }
            if all_keys.is_empty() || all_keys.len() > MAX_COMPOSITE_KEYS {
                return Err(format!(
                    "broker snapshot lock `{lock_key}` has invalid composite key count {}",
                    all_keys.len()
                ));
            }
            if remaining_keys.is_empty() {
                return Err(format!(
                    "broker snapshot lock `{lock_key}` has a composite waiter with no remaining keys"
                ));
            }
            if remaining_keys.first().is_some_and(|key| key != lock_key) {
                return Err(format!(
                    "broker snapshot lock `{lock_key}` has composite waiter queued on the wrong key"
                ));
            }
            let mut all = std::collections::BTreeSet::new();
            for key in all_keys {
                if key.is_empty() {
                    return Err(format!(
                        "broker snapshot lock `{lock_key}` has an empty composite key"
                    ));
                }
                if !all.insert(key) {
                    return Err(format!(
                        "broker snapshot lock `{lock_key}` repeats composite key `{key}`"
                    ));
                }
            }
            let mut remaining = std::collections::BTreeSet::new();
            for key in remaining_keys {
                if !all.contains(key) {
                    return Err(format!(
                        "broker snapshot lock `{lock_key}` has composite sub-key `{key}` outside allKeys"
                    ));
                }
                if !remaining.insert(key) {
                    return Err(format!(
                        "broker snapshot lock `{lock_key}` repeats remaining composite key `{key}`"
                    ));
                }
            }
            let mut granted = std::collections::BTreeSet::new();
            for key in granted_keys {
                if !all.contains(key) {
                    return Err(format!(
                        "broker snapshot lock `{lock_key}` has composite sub-key `{key}` outside allKeys"
                    ));
                }
                if !granted.insert(key) {
                    return Err(format!(
                        "broker snapshot lock `{lock_key}` repeats granted composite key `{key}`"
                    ));
                }
                if remaining.contains(key) {
                    return Err(format!(
                        "broker snapshot lock `{lock_key}` has composite key `{key}` in both remainingKeys and grantedKeys"
                    ));
                }
            }
            for key in granted_tokens.keys() {
                if !all.contains(key) {
                    return Err(format!(
                        "broker snapshot lock `{lock_key}` has granted token for unknown key `{key}`"
                    ));
                }
            }
            if remaining
                .union(&granted)
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                != all
            {
                return Err(format!(
                    "broker snapshot lock `{lock_key}` composite waiter does not partition allKeys"
                ));
            }
            let token_keys = granted_tokens
                .keys()
                .collect::<std::collections::BTreeSet<_>>();
            if token_keys != granted {
                return Err(format!(
                    "broker snapshot lock `{lock_key}` composite granted token keys do not match grantedKeys"
                ));
            }
            Ok(())
        }
    }
}

fn snapshot_deadline_matches_state(
    state: &BrokerState,
    deadline: &BrokerRaftDeadlineSnapshot,
) -> bool {
    crate::routine_id!("ddl-routine-broker-snapshot-deadline-matches-1");
    deadline.keys.iter().all(|key| {
        state
            .locks
            .get(key)
            .map(|lock| lock_contains_uuid_for_kind(lock, &deadline.lock_uuid, &deadline.kind))
            .unwrap_or(false)
    })
}

fn snapshot_deadline_matches_locks(
    locks: &[BrokerRaftLockSnapshot],
    deadline: &BrokerRaftDeadlineSnapshot,
) -> bool {
    crate::routine_id!("ddl-routine-broker-snapshot-deadline-matches-locks-1");
    deadline.keys.iter().all(|key| {
        locks
            .iter()
            .find(|lock| &lock.key == key)
            .map(|lock| match deadline.kind {
                RwHoldKind::Exclusive => lock
                    .exclusive_holders
                    .iter()
                    .any(|holder| holder.lock_uuid == deadline.lock_uuid),
                RwHoldKind::Read => lock
                    .readers
                    .iter()
                    .any(|holder| holder.lock_uuid == deadline.lock_uuid),
                RwHoldKind::Write => lock
                    .writer
                    .as_ref()
                    .is_some_and(|holder| holder.lock_uuid == deadline.lock_uuid),
            })
            .unwrap_or(false)
    })
}

#[derive(Debug, Clone, Default)]
pub struct BrokerMetrics {
    pub keys: u64,
    pub holders: u64,
    pub waiters: u64,
    pub clients: u64,
    /// Number of holders currently registered in the deadline index.
    pub pending_deadlines: u64,
    /// Cumulative TTL-driven evictions since broker start.
    pub ttl_evictions_total: u64,
    /// Effective per-key concurrency ceiling. A `lock` request with
    /// `max` above this is silently clamped; see
    /// `concurrency_cap_clamps_total`.
    pub max_concurrency_cap: u32,
    /// Cumulative count of `lock` requests whose `max` was clamped to
    /// `max_concurrency_cap`. Non-zero means at least one client is
    /// asking for more parallelism than the broker is willing to grant.
    pub concurrency_cap_clamps_total: u64,
    /// Strictly monotonic upper bound on every fencing token issued
    /// since broker start, across every key. Re-applied as the seed
    /// floor whenever `lock_or_default` materialises a fresh
    /// `LockState`, so cross-incarnation fencing-token monotonicity
    /// is preserved across empty-key prunes.
    pub fencing_watermark: u64,
    /// Cumulative count of idle `LockState` entries reclaimed by the
    /// periodic empty-key prune sweep
    /// (`BrokerConfig::idle_key_grace`).
    pub idle_keys_pruned_total: u64,
}

/// Per-key contention snapshot used by the HTML status page (upstream
/// `live-mutex#108`). All fields are at-most-best-effort: the broker may
/// service requests in between `top_keys` building this struct and the
/// HTML being rendered.
#[derive(Debug, Clone, Default)]
pub struct KeyContentionSnapshot {
    pub key: String,
    pub exclusive_holders: u64,
    pub readers: u64,
    pub writers: u64,
    pub waiters: u64,
    /// Monotonic per-key fencing counter. Useful for spotting hot keys
    /// (high counter = lots of acquire/release churn).
    pub fencing_counter: u64,
    /// Per-key concurrency ceiling. `1` is classic mutex; `>1` is
    /// semaphore. Useful on the status page so an operator can
    /// immediately see "5/10 holders" instead of "5 holders".
    pub max: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::UnboundedReceiver;

    fn drain(rx: &mut UnboundedReceiver<Response>) -> Vec<Response> {
        crate::routine_id!("ddl-routine-5za2DOl-1aft1UKbOy");
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            out.push(msg);
        }
        out
    }

    fn partial_composite_snapshot_payload() -> serde_json::Value {
        let broker = Broker::new(BrokerConfig::default());
        let (holder, mut holder_rx) = broker.register_client();
        let (waiter, mut waiter_rx) = broker.register_client();
        broker.handle_request(
            holder,
            Request::Lock {
                uuid: "snapshot-partial-holder".into(),
                key: Some("snapshot-partial-b".into()),
                keys: None,
                pid: None,
                ttl: None,
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        assert!(drain(&mut holder_rx)
            .iter()
            .any(|response| matches!(response, Response::Lock { acquired: true, .. })));
        broker.handle_request(
            waiter,
            Request::Lock {
                uuid: "snapshot-partial-composite".into(),
                key: None,
                keys: Some(vec![
                    "snapshot-partial-b".into(),
                    "snapshot-partial-a".into(),
                ]),
                pid: None,
                ttl: None,
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        assert!(drain(&mut waiter_rx).iter().any(|response| matches!(
            response,
            Response::CompositeLock {
                acquired: false,
                ..
            }
        )));
        assert_eq!(broker.metrics().holders, 2);
        assert_eq!(broker.metrics().waiters, 1);
        serde_json::json!({
            "broker": broker.snapshot_for_raft().expect("partial composite snapshot"),
        })
    }

    fn partial_composite_waiter_kind_mut(
        payload: &mut serde_json::Value,
    ) -> &mut serde_json::Value {
        payload["broker"]["locks"]
            .as_array_mut()
            .expect("snapshot locks")
            .iter_mut()
            .find(|lock| lock["key"] == "snapshot-partial-b")
            .expect("lock with queued composite waiter")["queue"]
            .as_array_mut()
            .expect("queued waiter")
            .first_mut()
            .expect("composite waiter")
            .get_mut("kind")
            .expect("composite kind")
    }

    #[test]
    fn register_client_with_preferred_id_does_not_reset_normal_sequence() {
        crate::routine_id!("ddl-routine-broker-test-preferred-client-id-sequence-1");
        let broker = Broker::new(BrokerConfig::default());
        let (first, _first_rx) = broker.register_client();
        let (second, _second_rx) = broker.register_client();
        assert_eq!(first, 1);
        assert_eq!(second, 2);

        broker.drop_client(first);
        let (preferred, _preferred_rx) = broker.register_client_with_id(first);
        assert_eq!(preferred, first);

        let (skipped_live, _skipped_live_rx) = broker.register_client_with_id(second);
        assert_ne!(skipped_live, second);

        let (next, _next_rx) = broker.register_client();
        assert_ne!(next, first);
        assert_ne!(next, second);
        assert!(next > second);
    }

    #[test]
    fn raft_snapshot_restores_active_detached_holder_and_deadline() {
        crate::routine_id!("ddl-routine-broker-test-raft-snapshot-active-holder-1");
        let broker = Broker::new(BrokerConfig::default());
        let (client, mut rx) = broker.register_client();
        broker.handle_request(
            client,
            Request::Lock {
                uuid: "snapshot-lock-request".into(),
                key: Some("snapshot-key".into()),
                keys: None,
                pid: Some(123),
                ttl: Some(10_000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let lock_uuid = drain(&mut rx)
            .into_iter()
            .find_map(|response| match response {
                Response::Lock {
                    acquired: true,
                    lock_uuid,
                    ..
                } => lock_uuid,
                _ => None,
            })
            .expect("lock granted");
        broker.detach_lock_from_client(client, &lock_uuid);
        broker.drop_client(client);
        assert_eq!(broker.metrics().holders, 1);
        assert_eq!(broker.metrics().pending_deadlines, 1);

        let payload = serde_json::json!({
            "broker": broker.snapshot_for_raft().expect("broker snapshot"),
        });
        Broker::validate_raft_snapshot_payload(&payload).expect("valid snapshot payload");

        let restored = Broker::new(BrokerConfig::default());
        restored
            .install_raft_snapshot(&payload)
            .expect("install broker snapshot");
        let metrics = restored.metrics();
        assert_eq!(metrics.holders, 1);
        assert_eq!(metrics.waiters, 0);
        assert_eq!(metrics.pending_deadlines, 1);
        let top = restored.top_keys(1);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].key, "snapshot-key");
        assert_eq!(top[0].exclusive_holders, 1);

        let (unlocker, mut unlock_rx) = restored.register_client();
        restored.handle_request(
            unlocker,
            Request::Unlock {
                uuid: "snapshot-unlock".into(),
                key: Some("snapshot-key".into()),
                keys: None,
                lock_uuid: Some(lock_uuid),
                force: false,
            },
        );
        assert!(matches!(
            drain(&mut unlock_rx).as_slice(),
            [Response::Unlock { unlocked: true, .. }]
        ));
        assert_eq!(restored.metrics().holders, 0);
    }

    #[test]
    fn raft_snapshot_validation_rejects_deadline_without_matching_holder() {
        crate::routine_id!("ddl-routine-broker-test-raft-snapshot-deadline-holder-1");
        let broker = Broker::new(BrokerConfig::default());
        let (client, mut rx) = broker.register_client();
        broker.handle_request(
            client,
            Request::Lock {
                uuid: "snapshot-deadline-request".into(),
                key: Some("snapshot-deadline-key".into()),
                keys: None,
                pid: Some(123),
                ttl: Some(10_000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let lock_uuid = drain(&mut rx)
            .into_iter()
            .find_map(|response| match response {
                Response::Lock {
                    acquired: true,
                    lock_uuid,
                    ..
                } => lock_uuid,
                _ => None,
            })
            .expect("lock granted");
        broker.detach_lock_from_client(client, &lock_uuid);
        let mut payload = serde_json::json!({
            "broker": broker.snapshot_for_raft().expect("broker snapshot"),
        });
        payload["broker"]["deadlines"][0]["lockUuid"] =
            serde_json::Value::String("missing-holder".into());

        let err = Broker::validate_raft_snapshot_payload(&payload)
            .expect_err("deadline without matching holder must be rejected");

        assert!(err.contains("does not match restored holder state"));
    }

    #[test]
    fn raft_snapshot_restores_queued_waiter_and_grants_after_unlock() {
        crate::routine_id!("ddl-routine-broker-test-raft-snapshot-queued-waiter-1");
        let broker = Broker::new(BrokerConfig::default());
        let (holder, mut holder_rx) = broker.register_client();
        let (waiter, mut waiter_rx) = broker.register_client();
        broker.handle_request(
            holder,
            Request::Lock {
                uuid: "snapshot-holder-request".into(),
                key: Some("snapshot-wait-key".into()),
                keys: None,
                pid: None,
                ttl: None,
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let holder_lock_uuid = drain(&mut holder_rx)
            .into_iter()
            .find_map(|response| match response {
                Response::Lock {
                    acquired: true,
                    lock_uuid,
                    ..
                } => lock_uuid,
                _ => None,
            })
            .expect("holder lock granted");
        broker.handle_request(
            waiter,
            Request::Lock {
                uuid: "snapshot-waiter-request".into(),
                key: Some("snapshot-wait-key".into()),
                keys: None,
                pid: Some(456),
                ttl: Some(5_000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        assert!(matches!(
            drain(&mut waiter_rx).as_slice(),
            [Response::Lock {
                acquired: false,
                ..
            }]
        ));
        assert_eq!(broker.metrics().holders, 1);
        assert_eq!(broker.metrics().waiters, 1);

        let payload = serde_json::json!({
            "broker": broker.snapshot_for_raft().expect("broker snapshot with waiter"),
        });
        Broker::validate_raft_snapshot_payload(&payload).expect("valid waiter snapshot payload");

        let restored = Broker::new(BrokerConfig::default());
        restored
            .install_raft_snapshot(&payload)
            .expect("install waiter snapshot");
        let metrics = restored.metrics();
        assert_eq!(metrics.holders, 1);
        assert_eq!(metrics.waiters, 1);

        let (unlocker, mut unlock_rx) = restored.register_client();
        restored.handle_request(
            unlocker,
            Request::Unlock {
                uuid: "snapshot-waiter-unlock".into(),
                key: Some("snapshot-wait-key".into()),
                keys: None,
                lock_uuid: Some(holder_lock_uuid),
                force: false,
            },
        );
        assert!(matches!(
            drain(&mut unlock_rx).as_slice(),
            [Response::Unlock { unlocked: true, .. }]
        ));
        let metrics = restored.metrics();
        assert_eq!(metrics.holders, 1);
        assert_eq!(metrics.waiters, 0);
        let top = restored.top_keys(1);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].key, "snapshot-wait-key");
        assert_eq!(top[0].exclusive_holders, 1);
        assert_eq!(top[0].waiters, 0);
    }

    #[test]
    fn raft_snapshot_restored_waiter_keeps_client_id_for_drop_cleanup() {
        crate::routine_id!("ddl-routine-broker-test-raft-snapshot-waiter-drop-client-1");
        let broker = Broker::new(BrokerConfig::default());
        let (holder, mut holder_rx) = broker.register_client();
        let (waiter, mut waiter_rx) = broker.register_client();
        broker.handle_request(
            holder,
            Request::Lock {
                uuid: "snapshot-drop-holder".into(),
                key: Some("snapshot-drop-key".into()),
                keys: None,
                pid: None,
                ttl: None,
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        assert!(drain(&mut holder_rx)
            .iter()
            .any(|response| { matches!(response, Response::Lock { acquired: true, .. }) }));
        broker.handle_request(
            waiter,
            Request::Lock {
                uuid: "snapshot-drop-waiter".into(),
                key: Some("snapshot-drop-key".into()),
                keys: None,
                pid: None,
                ttl: None,
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        assert!(matches!(
            drain(&mut waiter_rx).as_slice(),
            [Response::Lock {
                acquired: false,
                ..
            }]
        ));

        let payload = serde_json::json!({
            "broker": broker.snapshot_for_raft().expect("broker snapshot with waiter"),
        });
        let restored = Broker::new(BrokerConfig::default());
        restored
            .install_raft_snapshot(&payload)
            .expect("install waiter snapshot");
        assert_eq!(restored.metrics().waiters, 1);

        restored.drop_client(waiter);
        let metrics = restored.metrics();
        assert_eq!(metrics.holders, 1);
        assert_eq!(metrics.waiters, 0);
    }

    #[test]
    fn raft_snapshot_validation_accepts_partial_composite_waiter_partition() {
        crate::routine_id!("ddl-routine-broker-test-snapshot-partial-composite-valid-1");
        let payload = partial_composite_snapshot_payload();

        Broker::validate_raft_snapshot_payload(&payload)
            .expect("partial composite snapshot should validate");

        let restored = Broker::new(BrokerConfig::default());
        restored
            .install_raft_snapshot(&payload)
            .expect("partial composite snapshot should install");
        let metrics = restored.metrics();
        assert_eq!(metrics.holders, 2);
        assert_eq!(metrics.waiters, 1);
    }

    #[test]
    fn raft_snapshot_validation_rejects_composite_waiter_on_wrong_queue_key() {
        crate::routine_id!("ddl-routine-broker-test-snapshot-composite-wrong-queue-key-1");
        let mut payload = partial_composite_snapshot_payload();
        let kind = partial_composite_waiter_kind_mut(&mut payload);
        kind["remainingKeys"] = serde_json::json!(["snapshot-partial-a", "snapshot-partial-b"]);
        kind["grantedKeys"] = serde_json::json!([]);
        kind["grantedTokens"] = serde_json::json!({});

        let err = Broker::validate_raft_snapshot_payload(&payload)
            .expect_err("wrong queued key must be rejected");

        assert!(
            err.contains("queued on the wrong key"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn raft_snapshot_validation_rejects_composite_waiter_duplicate_remaining_key() {
        crate::routine_id!("ddl-routine-broker-test-snapshot-composite-duplicate-remaining-1");
        let mut payload = partial_composite_snapshot_payload();
        let kind = partial_composite_waiter_kind_mut(&mut payload);
        kind["remainingKeys"] = serde_json::json!(["snapshot-partial-b", "snapshot-partial-b"]);

        let err = Broker::validate_raft_snapshot_payload(&payload)
            .expect_err("duplicate remaining key must be rejected");

        assert!(
            err.contains("repeats remaining composite key"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn raft_snapshot_validation_rejects_composite_waiter_overlapping_partition() {
        crate::routine_id!("ddl-routine-broker-test-snapshot-composite-overlap-1");
        let mut payload = partial_composite_snapshot_payload();
        let kind = partial_composite_waiter_kind_mut(&mut payload);
        kind["grantedKeys"] = serde_json::json!(["snapshot-partial-a", "snapshot-partial-b"]);
        kind["grantedTokens"] = serde_json::json!({
            "snapshot-partial-a": 1,
            "snapshot-partial-b": 2,
        });

        let err = Broker::validate_raft_snapshot_payload(&payload)
            .expect_err("overlapping remaining/granted key must be rejected");

        assert!(
            err.contains("in both remainingKeys and grantedKeys"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn raft_snapshot_validation_rejects_composite_waiter_incomplete_partition() {
        crate::routine_id!("ddl-routine-broker-test-snapshot-composite-incomplete-partition-1");
        let mut payload = partial_composite_snapshot_payload();
        let kind = partial_composite_waiter_kind_mut(&mut payload);
        kind["remainingKeys"] = serde_json::json!(["snapshot-partial-b"]);
        kind["grantedKeys"] = serde_json::json!([]);
        kind["grantedTokens"] = serde_json::json!({});

        let err = Broker::validate_raft_snapshot_payload(&payload)
            .expect_err("incomplete composite partition must be rejected");

        assert!(
            err.contains("does not partition allKeys"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn raft_snapshot_validation_rejects_composite_waiter_token_key_mismatch() {
        crate::routine_id!("ddl-routine-broker-test-snapshot-composite-token-mismatch-1");
        let mut payload = partial_composite_snapshot_payload();
        let kind = partial_composite_waiter_kind_mut(&mut payload);
        kind["grantedTokens"] = serde_json::json!({});

        let err = Broker::validate_raft_snapshot_payload(&payload)
            .expect_err("missing granted token must be rejected");

        assert!(
            err.contains("granted token keys do not match grantedKeys"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn exclusive_lock_granted_then_queued() {
        crate::routine_id!("ddl-routine-NtcQmUG_FIY_DfqdQo");
        let broker = Broker::new(BrokerConfig::default());
        let (a, mut a_rx) = broker.register_client();
        let (b, mut b_rx) = broker.register_client();

        broker.handle_request(
            a,
            Request::Lock {
                uuid: "r1".into(),
                key: Some("k".into()),
                keys: None,
                pid: None,
                ttl: Some(1000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let a_msgs = drain(&mut a_rx);
        let lock_uuid_a = match a_msgs.first().unwrap() {
            Response::Lock {
                acquired,
                lock_uuid,
                fencing_token,
                ..
            } => {
                assert!(*acquired);
                assert!(fencing_token.is_some());
                lock_uuid.clone().unwrap()
            }
            other => panic!("unexpected {other:?}"),
        };

        broker.handle_request(
            b,
            Request::Lock {
                uuid: "r2".into(),
                key: Some("k".into()),
                keys: None,
                pid: None,
                ttl: Some(1000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let b_msgs = drain(&mut b_rx);
        match b_msgs.first().unwrap() {
            Response::Lock { acquired, .. } => assert!(!acquired),
            other => panic!("unexpected {other:?}"),
        }

        // unlock A
        broker.handle_request(
            a,
            Request::Unlock {
                uuid: "u1".into(),
                key: Some("k".into()),
                keys: None,
                lock_uuid: Some(lock_uuid_a),
                force: false,
            },
        );
        let _ = drain(&mut a_rx);

        // B should now be granted.
        let b_msgs = drain(&mut b_rx);
        let granted = b_msgs.iter().any(|m| {
            matches!(
                m,
                Response::Lock {
                    acquired: true,
                    fencing_token: Some(_),
                    ..
                }
            )
        });
        assert!(granted, "B was not granted after A unlocked: {b_msgs:?}");
    }

    #[test]
    fn fencing_tokens_are_monotonic() {
        crate::routine_id!("ddl-routine-CC22S8RqzYNARpshGb");
        let broker = Broker::new(BrokerConfig::default());
        let (c, mut rx) = broker.register_client();
        let mut last = 0u64;
        for i in 0..10 {
            broker.handle_request(
                c,
                Request::Lock {
                    uuid: format!("r{i}"),
                    key: Some("z".into()),
                    keys: None,
                    pid: None,
                    ttl: Some(1000),
                    max: None,
                    force: false,
                    retry_count: 0,
                    keep_locks_after_death: false,
                    wait: None,
                },
            );
            let msgs = drain(&mut rx);
            let (lock_uuid, token) = msgs
                .iter()
                .find_map(|m| match m {
                    Response::Lock {
                        acquired: true,
                        lock_uuid: Some(lu),
                        fencing_token: Some(t),
                        ..
                    } => Some((lu.clone(), *t)),
                    _ => None,
                })
                .expect("lock not granted");
            assert!(
                token > last,
                "fencing token went backwards: {token} <= {last}"
            );
            last = token;
            broker.handle_request(
                c,
                Request::Unlock {
                    uuid: format!("u{i}"),
                    key: Some("z".into()),
                    keys: None,
                    lock_uuid: Some(lock_uuid),
                    force: false,
                },
            );
            let _ = drain(&mut rx);
        }
    }

    #[test]
    fn composite_lock_acquires_atomically() {
        crate::routine_id!("ddl-routine-QyOJ2kRN9b4sEd7pdw");
        let broker = Broker::new(BrokerConfig::default());
        let (a, mut a_rx) = broker.register_client();
        let (b, mut b_rx) = broker.register_client();
        let (c, mut c_rx) = broker.register_client();

        // A grabs ["a","b"].
        broker.handle_request(
            a,
            Request::Lock {
                uuid: "ra".into(),
                key: None,
                keys: Some(vec!["b".into(), "a".into()]),
                pid: None,
                ttl: Some(1000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let lock_uuid_a = drain(&mut a_rx)
            .into_iter()
            .find_map(|m| match m {
                Response::CompositeLock {
                    acquired: true,
                    lock_uuid,
                    ..
                } => lock_uuid,
                _ => None,
            })
            .expect("A should hold composite");

        // B asks for ["b","c"] — should queue (b is held).
        broker.handle_request(
            b,
            Request::Lock {
                uuid: "rb".into(),
                key: None,
                keys: Some(vec!["b".into(), "c".into()]),
                pid: None,
                ttl: Some(1000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let b_pre = drain(&mut b_rx);
        assert!(b_pre.iter().any(|m| matches!(
            m,
            Response::CompositeLock {
                acquired: false,
                ..
            }
        )));

        // C asks for ["a"] — should queue.
        broker.handle_request(
            c,
            Request::Lock {
                uuid: "rc".into(),
                key: Some("a".into()),
                keys: None,
                pid: None,
                ttl: Some(1000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let c_pre = drain(&mut c_rx);
        assert!(c_pre.iter().any(|m| matches!(
            m,
            Response::Lock {
                acquired: false,
                ..
            }
        )));

        // A releases composite.
        broker.handle_request(
            a,
            Request::Unlock {
                uuid: "ua".into(),
                key: None,
                keys: Some(vec!["a".into(), "b".into()]),
                lock_uuid: Some(lock_uuid_a),
                force: false,
            },
        );
        let _ = drain(&mut a_rx);

        // C (next on `a`) should be granted; B should also progress (gets b
        // first, then c).
        let c_msgs = drain(&mut c_rx);
        assert!(c_msgs
            .iter()
            .any(|m| matches!(m, Response::Lock { acquired: true, .. })));

        let b_msgs = drain(&mut b_rx);
        assert!(b_msgs
            .iter()
            .any(|m| matches!(m, Response::CompositeLock { acquired: true, .. })));
    }

    #[test]
    fn rw_writer_blocks_readers_until_done() {
        crate::routine_id!("ddl-routine-E8a4TwTjPsQ3d2-exo");
        let broker = Broker::new(BrokerConfig::default());
        let (a, mut a_rx) = broker.register_client();
        let (b, mut b_rx) = broker.register_client();
        let (c, mut c_rx) = broker.register_client();

        broker.handle_request(
            a,
            Request::RegisterWrite {
                uuid: "w1".into(),
                key: "k".into(),
            },
        );
        let _ = drain(&mut a_rx);

        broker.handle_request(
            b,
            Request::RegisterRead {
                uuid: "r1".into(),
                key: "k".into(),
            },
        );
        broker.handle_request(
            c,
            Request::RegisterRead {
                uuid: "r2".into(),
                key: "k".into(),
            },
        );
        for r in [&mut b_rx, &mut c_rx] {
            let m = drain(r);
            assert!(m
                .iter()
                .any(|m| matches!(m, Response::RegisterReadResult { granted: false, .. })));
        }

        broker.handle_request(
            a,
            Request::EndWrite {
                uuid: "ew".into(),
                key: "k".into(),
            },
        );
        let _ = drain(&mut a_rx);

        // Both readers should now be granted.
        for r in [&mut b_rx, &mut c_rx] {
            let m = drain(r);
            assert!(m
                .iter()
                .any(|m| matches!(m, Response::RegisterReadResult { granted: true, .. })));
        }
    }

    #[test]
    fn rw_waiting_writer_prevents_later_reader_barging() {
        crate::routine_id!("ddl-routine-rw-waiting-writer-no-reader-barge-Zh4");
        let broker = Broker::new(BrokerConfig::default());
        let (reader_a, mut reader_a_rx) = broker.register_client();
        let (writer_b, mut writer_b_rx) = broker.register_client();
        let (reader_c, mut reader_c_rx) = broker.register_client();

        broker.handle_request(
            reader_a,
            Request::RegisterRead {
                uuid: "reader-a".into(),
                key: "rw-no-barge".into(),
            },
        );
        assert!(drain(&mut reader_a_rx).iter().any(|m| matches!(
            m,
            Response::RegisterReadResult {
                uuid,
                granted: true,
                ..
            } if uuid == "reader-a"
        )));

        broker.handle_request(
            writer_b,
            Request::RegisterWrite {
                uuid: "writer-b".into(),
                key: "rw-no-barge".into(),
            },
        );
        assert!(drain(&mut writer_b_rx).iter().any(|m| matches!(
            m,
            Response::RegisterWriteResult {
                uuid,
                granted: false,
                readers_count: 1,
                ..
            } if uuid == "writer-b"
        )));

        broker.handle_request(
            reader_c,
            Request::RegisterRead {
                uuid: "reader-c".into(),
                key: "rw-no-barge".into(),
            },
        );
        assert!(drain(&mut reader_c_rx).iter().any(|m| matches!(
            m,
            Response::RegisterReadResult {
                uuid,
                granted: false,
                ..
            } if uuid == "reader-c"
        )));

        broker.handle_request(
            reader_a,
            Request::EndRead {
                uuid: "end-reader-a".into(),
                key: "rw-no-barge".into(),
            },
        );
        let _ = drain(&mut reader_a_rx);

        let writer_msgs = drain(&mut writer_b_rx);
        assert!(
            writer_msgs.iter().any(|m| matches!(
                m,
                Response::RegisterWriteResult {
                    uuid,
                    granted: true,
                    writer_flag: true,
                    ..
                } if uuid == "writer-b"
            )),
            "queued writer should be granted before later reader: {writer_msgs:?}"
        );
        assert!(
            drain(&mut reader_c_rx).is_empty(),
            "later reader must not be granted while queued writer holds the key"
        );

        broker.handle_request(
            writer_b,
            Request::EndWrite {
                uuid: "end-writer-b".into(),
                key: "rw-no-barge".into(),
            },
        );
        let _ = drain(&mut writer_b_rx);

        let reader_c_msgs = drain(&mut reader_c_rx);
        assert!(
            reader_c_msgs.iter().any(|m| matches!(
                m,
                Response::RegisterReadResult {
                    uuid,
                    granted: true,
                    writer_flag: false,
                    ..
                } if uuid == "reader-c"
            )),
            "later reader should be granted after writer completes: {reader_c_msgs:?}"
        );
    }

    #[test]
    fn dropping_client_releases_locks_and_queue() {
        crate::routine_id!("ddl-routine-g9Qk5cWEaz9JgGK8OC");
        let broker = Broker::new(BrokerConfig::default());
        let (a, _a_rx) = broker.register_client();
        let (b, mut b_rx) = broker.register_client();

        broker.handle_request(
            a,
            Request::Lock {
                uuid: "r1".into(),
                key: Some("k".into()),
                keys: None,
                pid: None,
                ttl: Some(1000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        broker.handle_request(
            b,
            Request::Lock {
                uuid: "r2".into(),
                key: Some("k".into()),
                keys: None,
                pid: None,
                ttl: Some(1000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let pre = drain(&mut b_rx);
        assert!(pre.iter().any(|m| matches!(
            m,
            Response::Lock {
                acquired: false,
                ..
            }
        )));
        broker.drop_client(a);
        let post = drain(&mut b_rx);
        assert!(post
            .iter()
            .any(|m| matches!(m, Response::Lock { acquired: true, .. })));
    }

    #[test]
    fn drop_client_without_handle_releases_replayed_holder() {
        crate::routine_id!("ddl-routine-broker-test-drop-replayed-holder-1");
        let broker = Broker::new(BrokerConfig::default());
        broker.handle_request(
            42,
            Request::Lock {
                uuid: "replayed-holder".into(),
                key: Some("k".into()),
                keys: None,
                pid: None,
                ttl: Some(1000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        assert_eq!(broker.metrics().holders, 1);

        broker.drop_client(42);
        let metrics = broker.metrics();
        assert_eq!(metrics.holders, 0);
        assert_eq!(metrics.waiters, 0);
    }

    /// `tick_ttl` evicts the expired holder and grants the next waiter in
    /// a single pass — without requiring a per-lock `tokio::time::sleep`.
    /// This is the structural fix from upstream `live-mutex#13`.
    #[test]
    fn tick_ttl_evicts_expired_holder_and_grants_next_waiter() {
        crate::routine_id!("ddl-routine-4H4ODnZw0ibxFfSMSv");
        let broker = Broker::new(BrokerConfig::default());
        let (a, mut a_rx) = broker.register_client();
        let (b, mut b_rx) = broker.register_client();

        // A acquires with a 50ms TTL.
        broker.handle_request(
            a,
            Request::Lock {
                uuid: "r1".into(),
                key: Some("k".into()),
                keys: None,
                pid: None,
                ttl: Some(50),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let _ = drain(&mut a_rx);

        // B queues behind A.
        broker.handle_request(
            b,
            Request::Lock {
                uuid: "r2".into(),
                key: Some("k".into()),
                keys: None,
                pid: None,
                ttl: Some(1000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let pre = drain(&mut b_rx);
        assert!(pre.iter().any(|m| matches!(
            m,
            Response::Lock {
                acquired: false,
                ..
            }
        )));

        // Single sweep with a synthesized future Instant — no real wall
        // time burned, no per-lock timer involved.
        let future = Instant::now() + Duration::from_secs(1);
        let evicted = broker.tick_ttl(future);
        assert_eq!(
            evicted, 1,
            "exactly one holder should have been TTL-evicted"
        );

        // B should now hold the lock; the metric counter should reflect 1.
        let post = drain(&mut b_rx);
        assert!(
            post.iter()
                .any(|m| matches!(m, Response::Lock { acquired: true, .. })),
            "B should be granted after A's TTL eviction; got {post:?}"
        );

        let snapshot = broker.metrics();
        assert_eq!(snapshot.ttl_evictions_total, 1);
    }

    /// A second sweep with no expired holders is a no-op — the BTreeMap
    /// `range(..=now)` returns empty and we don't touch the LockState.
    #[test]
    fn tick_ttl_is_idempotent_when_nothing_expired() {
        crate::routine_id!("ddl-routine-CWLi3VgQgQbVCXnsf4");
        let broker = Broker::new(BrokerConfig::default());
        let (a, mut a_rx) = broker.register_client();
        broker.handle_request(
            a,
            Request::Lock {
                uuid: "r1".into(),
                key: Some("k".into()),
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
        let _ = drain(&mut a_rx);

        // Sweep with a moment that's well before the deadline.
        let now = Instant::now();
        assert_eq!(broker.tick_ttl(now), 0);
        assert_eq!(broker.metrics().ttl_evictions_total, 0);
        assert_eq!(broker.metrics().pending_deadlines, 1);
    }

    /// Releasing a lock early leaves a stale entry in the deadline index
    /// (intentional lazy deletion — see `DeadlineEntry` doc comment). The
    /// sweeper must skip it without panicking.
    #[test]
    fn tick_ttl_skips_locks_released_before_their_deadline() {
        crate::routine_id!("ddl-routine-1sGlfQa6Nc6QbabX-9");
        let broker = Broker::new(BrokerConfig::default());
        let (a, mut a_rx) = broker.register_client();
        broker.handle_request(
            a,
            Request::Lock {
                uuid: "r1".into(),
                key: Some("k".into()),
                keys: None,
                pid: None,
                ttl: Some(50),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let lock_uuid = drain(&mut a_rx)
            .into_iter()
            .find_map(|m| match m {
                Response::Lock { lock_uuid, .. } => lock_uuid,
                _ => None,
            })
            .expect("A should have acquired");
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

        // Sweep well past the original deadline. There should be no
        // eviction (the lock was already released) but `pending_deadlines`
        // should drop to 0 because we removed the stale entry.
        let future = Instant::now() + Duration::from_secs(1);
        assert_eq!(broker.tick_ttl(future), 0);
        assert_eq!(broker.metrics().ttl_evictions_total, 0);
        assert_eq!(broker.metrics().pending_deadlines, 0);
    }

    /// TTL eviction works on composite holders too: a single deadline
    /// entry covers every key the composite locked, and the sweep
    /// releases all of them in one pass.
    #[test]
    fn tick_ttl_evicts_composite_holder_atomically() {
        crate::routine_id!("ddl-routine-HCvFiziZVug_p4IFCj");
        let broker = Broker::new(BrokerConfig::default());
        let (a, mut a_rx) = broker.register_client();
        let (b, mut b_rx) = broker.register_client();

        broker.handle_request(
            a,
            Request::Lock {
                uuid: "ra".into(),
                key: None,
                keys: Some(vec!["x".into(), "y".into()]),
                pid: None,
                ttl: Some(50),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let _ = drain(&mut a_rx);

        broker.handle_request(
            b,
            Request::Lock {
                uuid: "rb".into(),
                key: Some("x".into()),
                keys: None,
                pid: None,
                ttl: Some(1000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let _ = drain(&mut b_rx);

        let future = Instant::now() + Duration::from_secs(1);
        assert_eq!(broker.tick_ttl(future), 1);

        let post = drain(&mut b_rx);
        assert!(
            post.iter()
                .any(|m| matches!(m, Response::Lock { acquired: true, .. })),
            "B should be granted on `x` after A's composite TTL-evicts; got {post:?}"
        );
    }

    /// Semaphore semantics: with `max=3`, three simultaneous holders are
    /// admitted on the same key, and a fourth has to queue. Each
    /// holder gets its own `lock_uuid` and a strictly increasing
    /// fencing token so a downstream resource can disambiguate slots.
    #[test]
    fn concurrency_max_three_admits_three_holders_then_queues_fourth() {
        crate::routine_id!("ddl-routine-u4_2tpDeH_Ul7hylUn");
        let broker = Broker::new(BrokerConfig::default());
        let mut clients = Vec::new();
        for _ in 0..4 {
            clients.push(broker.register_client());
        }
        let mut grants: Vec<(Option<String>, Option<u64>, bool)> = Vec::new();
        for (i, (cid, rx)) in clients.iter_mut().enumerate() {
            broker.handle_request(
                *cid,
                Request::Lock {
                    uuid: format!("r{i}"),
                    key: Some("semkey".into()),
                    keys: None,
                    pid: None,
                    ttl: Some(60_000),
                    max: Some(3),
                    force: false,
                    retry_count: 0,
                    keep_locks_after_death: false,
                    wait: None,
                },
            );
            let msgs = drain(rx);
            let (lock_uuid, token, acquired) = match msgs.first() {
                Some(Response::Lock {
                    acquired,
                    lock_uuid,
                    fencing_token,
                    ..
                }) => (lock_uuid.clone(), *fencing_token, *acquired),
                other => panic!("unexpected response for client {i}: {other:?}"),
            };
            grants.push((lock_uuid, token, acquired));
        }

        // First three should be granted, fourth queued.
        assert!(
            grants[0].2 && grants[1].2 && grants[2].2,
            "first three must be granted"
        );
        assert!(!grants[3].2, "fourth must be queued");

        // Each granted holder has a distinct lock_uuid and a unique
        // fencing token. Fencing tokens are monotonically increasing so
        // a downstream resource can order operations by recency.
        let uuids: std::collections::HashSet<_> = grants[..3].iter().map(|g| g.0.clone()).collect();
        assert_eq!(uuids.len(), 3, "lock_uuids must be unique across slots");
        let tokens: Vec<u64> = grants[..3].iter().map(|g| g.1.unwrap_or(0)).collect();
        assert!(
            tokens.windows(2).all(|w| w[0] < w[1]),
            "fencing tokens must be strictly increasing across slots; got {tokens:?}"
        );

        let snapshot = broker.metrics();
        assert_eq!(snapshot.holders, 3);
        assert_eq!(snapshot.waiters, 1);

        // Releasing one of the three slots admits the queued waiter
        // exactly once — not twice.
        let release_uuid = grants[0].0.clone().unwrap();
        broker.handle_request(
            clients[0].0,
            Request::Unlock {
                uuid: "u0".into(),
                key: Some("semkey".into()),
                keys: None,
                lock_uuid: Some(release_uuid),
                force: false,
            },
        );
        let _ = drain(&mut clients[0].1);
        let post = drain(&mut clients[3].1);
        assert!(
            post.iter()
                .any(|m| matches!(m, Response::Lock { acquired: true, .. })),
            "fourth client should be granted after one of the three slots releases; got {post:?}"
        );

        let final_snapshot = broker.metrics();
        assert_eq!(final_snapshot.holders, 3);
        assert_eq!(final_snapshot.waiters, 0);
    }

    /// A `lock` request that asks for `max` above the broker's
    /// `max_concurrency_cap` is silently clamped, the clamp is counted
    /// in `concurrency_cap_clamps_total`, and the resulting `LockState`
    /// caps holders at the ceiling — not at the requested value.
    #[test]
    fn concurrency_cap_clamps_oversized_max_request() {
        crate::routine_id!("ddl-routine-pPDJD5a0veKa3TcPdF");
        let cfg = BrokerConfig {
            max_concurrency_cap: 5,
            ..BrokerConfig::default()
        };
        let broker = Broker::new(cfg);
        let (a, mut a_rx) = broker.register_client();

        broker.handle_request(
            a,
            Request::Lock {
                uuid: "r1".into(),
                key: Some("hot".into()),
                keys: None,
                pid: None,
                ttl: Some(60_000),
                max: Some(9999),
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let _ = drain(&mut a_rx);

        let snapshot = broker.metrics();
        assert_eq!(snapshot.concurrency_cap_clamps_total, 1);
        assert_eq!(snapshot.max_concurrency_cap, 5);

        // Verify the lock's per-key cap is the ceiling. Add 5 more
        // acquires; the 5th must fit (cap=5 across all 6 = original a +
        // 5 new = 6 total, so the last queues).
        let mut extras = Vec::new();
        for i in 0..5 {
            let (c, rx) = broker.register_client();
            extras.push((c, rx, format!("extra-{i}")));
        }
        for (c, rx, uuid) in extras.iter_mut() {
            broker.handle_request(
                *c,
                Request::Lock {
                    uuid: uuid.clone(),
                    key: Some("hot".into()),
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
            let _ = drain(rx);
        }
        let post = broker.metrics();
        assert_eq!(post.holders, 5, "cap=5 must hold exactly five holders");
        assert_eq!(post.waiters, 1, "the sixth acquire must be queued");
    }

    /// `force=true` on `unlock` clears every holder for the key, even
    /// holders owned by other clients. Used as the "I'm the operator,
    /// break this lock" escape hatch. Anyone who *thinks* they hold it
    /// will find their `lock_uuid` is no longer registered when they
    /// try to release.
    #[test]
    fn force_unlock_releases_holders_owned_by_other_clients() {
        crate::routine_id!("ddl-routine-_b81ZSIAbk0HRmKaDN");
        let broker = Broker::new(BrokerConfig::default());
        let (a, mut a_rx) = broker.register_client();
        let (b, mut b_rx) = broker.register_client();
        let (c, mut c_rx) = broker.register_client();

        broker.handle_request(
            a,
            Request::Lock {
                uuid: "ra".into(),
                key: Some("forceme".into()),
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
        let _ = drain(&mut a_rx);

        // B queues behind A.
        broker.handle_request(
            b,
            Request::Lock {
                uuid: "rb".into(),
                key: Some("forceme".into()),
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
        let _ = drain(&mut b_rx);

        // C — operator — sends a force-unlock with no `lock_uuid`.
        broker.handle_request(
            c,
            Request::Unlock {
                uuid: "uc".into(),
                key: Some("forceme".into()),
                keys: None,
                lock_uuid: None,
                force: true,
            },
        );
        let _ = drain(&mut c_rx);

        // B should have been promoted to holder.
        let after = drain(&mut b_rx);
        assert!(
            after
                .iter()
                .any(|m| matches!(m, Response::Lock { acquired: true, .. })),
            "after force-unlock, B should hold the lock; got {after:?}"
        );
    }

    /// Releasing with the wrong `lock_uuid` is a no-op for that key —
    /// the real holder keeps holding. The broker reports
    /// `unlocked: false` so the caller can distinguish from success.
    #[test]
    fn unlock_with_wrong_lock_uuid_is_a_no_op() {
        crate::routine_id!("ddl-routine-mFDrBKyagORZcBK9hZ");
        let broker = Broker::new(BrokerConfig::default());
        let (a, mut a_rx) = broker.register_client();
        let (b, mut b_rx) = broker.register_client();

        broker.handle_request(
            a,
            Request::Lock {
                uuid: "ra".into(),
                key: Some("k".into()),
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
        let _ = drain(&mut a_rx);

        broker.handle_request(
            b,
            Request::Unlock {
                uuid: "ub".into(),
                key: Some("k".into()),
                keys: None,
                lock_uuid: Some("definitely-not-the-real-uuid".into()),
                force: false,
            },
        );
        let msgs = drain(&mut b_rx);
        let unlocked = msgs.iter().find_map(|m| match m {
            Response::Unlock { unlocked, .. } => Some(*unlocked),
            _ => None,
        });
        assert_eq!(
            unlocked,
            Some(false),
            "wrong-uuid unlock must report unlocked: false; got {msgs:?}"
        );

        // A still holds (no spurious grant to anyone else).
        let snapshot = broker.metrics();
        assert_eq!(snapshot.holders, 1, "real holder must be untouched");
    }

    /// Composite locks reject empty key arrays.
    #[test]
    fn composite_rejects_empty_keys() {
        crate::routine_id!("ddl-routine-ywtJ3MT9dYVEhxbCys");
        let broker = Broker::new(BrokerConfig::default());
        let (a, mut a_rx) = broker.register_client();
        broker.handle_request(
            a,
            Request::Lock {
                uuid: "r".into(),
                key: None,
                keys: Some(vec![]),
                pid: None,
                ttl: Some(60_000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let msgs = drain(&mut a_rx);
        let err = msgs.iter().find_map(|m| match m {
            Response::CompositeLock {
                acquired: false,
                error: Some(e),
                ..
            } => Some(e.clone()),
            _ => None,
        });
        assert!(
            err.is_some(),
            "empty composite must be rejected; got {msgs:?}"
        );
    }

    /// Composite locks reject more than `MAX_COMPOSITE_KEYS` keys.
    #[test]
    fn composite_rejects_oversized_keyset() {
        crate::routine_id!("ddl-routine-kF2jpYdXiPcycCoi8L");
        let broker = Broker::new(BrokerConfig::default());
        let (a, mut a_rx) = broker.register_client();
        let too_many: Vec<String> = (0..(MAX_COMPOSITE_KEYS + 1))
            .map(|i| format!("k{i}"))
            .collect();
        broker.handle_request(
            a,
            Request::Lock {
                uuid: "r".into(),
                key: None,
                keys: Some(too_many),
                pid: None,
                ttl: Some(60_000),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let msgs = drain(&mut a_rx);
        assert!(
            msgs.iter().any(|m| matches!(
                m,
                Response::CompositeLock {
                    acquired: false,
                    error: Some(_),
                    ..
                }
            )),
            "oversized composite must be rejected; got {msgs:?}"
        );
    }

    /// `ttl=Some(0)` does not register a deadline — so a sweeper running
    /// arbitrarily far into the future doesn't evict the lock.
    #[test]
    fn ttl_zero_does_not_register_deadline() {
        crate::routine_id!("ddl-routine-IXR9BP8iP2QVc-tv8N");
        let broker = Broker::new(BrokerConfig::default());
        let (a, mut a_rx) = broker.register_client();
        broker.handle_request(
            a,
            Request::Lock {
                uuid: "r".into(),
                key: Some("k".into()),
                keys: None,
                pid: None,
                ttl: Some(0),
                max: None,
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let _ = drain(&mut a_rx);
        assert_eq!(broker.metrics().pending_deadlines, 0);
        // Sweeping a year from now must not evict.
        let way_future = Instant::now() + Duration::from_secs(60 * 60 * 24 * 365);
        assert_eq!(broker.tick_ttl(way_future), 0);
        assert_eq!(broker.metrics().holders, 1);
    }

    /// `max=None` preserves the existing per-key concurrency level
    /// instead of resetting it. This matters when one client opts into
    /// semaphore semantics with `max=N` and another (unaware) caller
    /// follows up with `max=None` — we don't want the second caller to
    /// silently revert the key to mutex semantics.
    ///
    /// Note: `max=Some(0)` is *not* equivalent to `max=None` — it is
    /// rejected eagerly with a clear error. See
    /// `concurrency_max_zero_is_rejected_with_clear_error` below.
    #[test]
    fn concurrency_max_none_preserves_existing_per_key_cap() {
        crate::routine_id!("ddl-routine-pFc44VtL7q-2sHdquA");
        let broker = Broker::new(BrokerConfig::default());
        let (a, mut a_rx) = broker.register_client();
        let (b, mut b_rx) = broker.register_client();
        let (c, mut c_rx) = broker.register_client();

        broker.handle_request(
            a,
            Request::Lock {
                uuid: "ra".into(),
                key: Some("k".into()),
                keys: None,
                pid: None,
                ttl: Some(60_000),
                max: Some(2),
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let _ = drain(&mut a_rx);

        // Second caller does not pass `max`; cap should remain 2.
        broker.handle_request(
            b,
            Request::Lock {
                uuid: "rb".into(),
                key: Some("k".into()),
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
        let _ = drain(&mut b_rx);

        // Third caller is queued because cap is 2.
        broker.handle_request(
            c,
            Request::Lock {
                uuid: "rc".into(),
                key: Some("k".into()),
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
        let pre = drain(&mut c_rx);
        assert!(
            pre.iter().any(|m| matches!(
                m,
                Response::Lock {
                    acquired: false,
                    ..
                }
            )),
            "third caller must queue while cap=2; got {pre:?}"
        );
        let snapshot = broker.metrics();
        assert_eq!(snapshot.holders, 2);
        assert_eq!(snapshot.waiters, 1);
    }

    /// `max=Some(0)` is rejected eagerly with a clear error message
    /// and *no* side effects: no holder is created, no waiter queued,
    /// no per-key state mutated. The previous "silently treat as
    /// `None`" behavior was a foot-gun (a misconfigured caller would
    /// be told they had a lock with whatever cap happened to already
    /// be set on the key).
    #[test]
    fn concurrency_max_zero_is_rejected_with_clear_error() {
        crate::routine_id!("ddl-routine-rHoreRKbi5cVFvB9Yw");
        let broker = Broker::new(BrokerConfig::default());
        let (a, mut a_rx) = broker.register_client();

        broker.handle_request(
            a,
            Request::Lock {
                uuid: "r".into(),
                key: Some("zero-max".into()),
                keys: None,
                pid: None,
                ttl: Some(60_000),
                max: Some(0),
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let msgs = drain(&mut a_rx);
        let err = msgs.iter().find_map(|m| match m {
            Response::Lock {
                acquired: false,
                error: Some(e),
                ..
            } => Some(e.clone()),
            _ => None,
        });
        assert!(
            err.as_deref()
                .is_some_and(|e| e.contains("`max` must be >= 1")),
            "max=0 must be rejected with a clear error; got {msgs:?}"
        );
        // No holder, no waiter, no per-key state — broker is untouched.
        let snapshot = broker.metrics();
        assert_eq!(
            snapshot.holders, 0,
            "rejected request must not register a holder"
        );
        assert_eq!(
            snapshot.waiters, 0,
            "rejected request must not enqueue a waiter"
        );
        assert_eq!(
            snapshot.keys, 0,
            "rejected request must not create a per-key LockState"
        );
    }

    /// Composite path: `keys: [...]` plus `max=Some(0)` is rejected
    /// the same way, with a `compositeLock` response shape (matching
    /// the request shape) so cross-runtime clients that switch on
    /// `type` see a consistent variant.
    #[test]
    fn concurrency_max_zero_on_composite_is_rejected() {
        crate::routine_id!("ddl-routine-oTuwSI0_xmF6pYkfFl");
        let broker = Broker::new(BrokerConfig::default());
        let (a, mut a_rx) = broker.register_client();

        broker.handle_request(
            a,
            Request::Lock {
                uuid: "r".into(),
                key: None,
                keys: Some(vec!["a".into(), "b".into()]),
                pid: None,
                ttl: Some(60_000),
                max: Some(0),
                force: false,
                retry_count: 0,
                keep_locks_after_death: false,
                wait: None,
            },
        );
        let msgs = drain(&mut a_rx);
        assert!(
            msgs.iter().any(|m| matches!(
                m,
                Response::CompositeLock {
                    acquired: false,
                    error: Some(_),
                    ..
                }
            )),
            "composite + max=0 must come back as compositeLock with an error; got {msgs:?}"
        );
        assert_eq!(
            broker.metrics().keys,
            0,
            "no per-key state should be created"
        );
    }
}
