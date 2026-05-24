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
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::protocol::{Request, Response, MAX_COMPOSITE_KEYS};
use crate::queue::LinkedQueue;

pub type ClientId = u64;
pub type Sender = mpsc::UnboundedSender<Response>;

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
    /// (`100`).
    pub max_concurrency_cap: u32,
}

impl Default for BrokerConfig {
    fn default() -> Self {
        Self {
            default_ttl: Duration::from_millis(4000),
            max_lock_holders: 1,
            ttl_sweep_interval: Duration::from_millis(10),
            max_concurrency_cap: crate::protocol::DEFAULT_MAX_CONCURRENCY_CAP,
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

#[derive(Debug, Clone, PartialEq, Eq)]
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
        Self {
            exclusive_holders: HashMap::new(),
            max: max.max(1),
            readers: HashMap::new(),
            writer: None,
            queue: LinkedQueue::new(),
            fencing_counter: 0,
            timestamp_emptied: None,
        }
    }

    #[allow(dead_code)] // wired up by future TTL/GC sweeps
    fn is_idle(&self) -> bool {
        self.exclusive_holders.is_empty()
            && self.readers.is_empty()
            && self.writer.is_none()
            && self.queue.is_empty()
    }

    fn next_fencing_token(&mut self) -> u64 {
        self.fencing_counter = self.fencing_counter.wrapping_add(1).max(1);
        self.fencing_counter
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
    keep_locks_after_death: bool,
    kind: PendingKind,
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
}

impl BrokerState {
    fn new(config: BrokerConfig) -> Self {
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
        let Some(ttl) = ttl else { return };
        if ttl.is_zero() {
            return;
        }
        let deadline = Instant::now() + ttl;
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
        // The per-key starting `max` is clamped to the broker-wide
        // ceiling, so a misconfigured `LMX_MAX_LOCK_HOLDERS` can't
        // smuggle a giant default past `max_concurrency_cap`.
        let cap = self.config.max_concurrency_cap.max(1);
        let max = self.config.max_lock_holders.min(cap).max(1);
        self.locks.entry(key.to_string()).or_insert_with(|| LockState::new(max))
    }

    #[allow(dead_code)] // wired up by future TTL/GC sweeps
    fn maybe_gc(&mut self, key: &str) {
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
}

impl Broker {
    pub fn new(config: BrokerConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(BrokerState::new(config))),
        }
    }

    /// Register a new client connection. Returns the client's id and a
    /// pre-built `mpsc::UnboundedReceiver<Response>` the listener should pump
    /// onto its socket.
    pub fn register_client(&self) -> (ClientId, mpsc::UnboundedReceiver<Response>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut state = self.state.lock();
        let id = state.next_client_id;
        state.next_client_id += 1;
        state.clients.insert(
            id,
            ClientHandle {
                sender: tx,
                held_lock_uuids: HashMap::new(),
                pending_request_uuids: Vec::new(),
            },
        );
        (id, rx)
    }

    /// Drop a client, releasing every lock it held and pruning every queued
    /// request it owned. Callers should invoke this exactly once when the
    /// client's transport goes away.
    pub fn drop_client(&self, client: ClientId) {
        let mut state = self.state.lock();
        let Some(mut handle) = state.clients.remove(&client) else {
            return;
        };

        // Release pending waiters first so they cannot win an unlock race.
        for (key, request_uuid) in handle.pending_request_uuids.drain(..) {
            if let Some(lock) = state.locks.get_mut(&key) {
                lock.queue.remove(&request_uuid);
            }
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
            }
        }

        // After dropping holders, try to grant the next waiter on every key.
        let keys: Vec<String> = state.locks.keys().cloned().collect();
        for key in keys {
            self.try_grant_next(&mut state, &key);
        }
    }

    /// Handle a single inbound request from `client`. Sends responses back
    /// via the client's mpsc sender (no return value because there can be
    /// many or zero outbound messages: e.g. composite-lock partial progress).
    pub fn handle_request(&self, client: ClientId, request: Request) {
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
                force: _force,
                retry_count: _retry_count,
                keep_locks_after_death,
            } => {
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
                    let err = "`max` must be >= 1; omit the field to keep the existing concurrency level".to_string();
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
                        );
                    }
                    (None, Some(ks)) => {
                        self.handle_composite_lock(&mut state, client, uuid, ks, pid, ttl);
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
                    (Some(k), Some(mut ks)) => {
                        ks.push(k);
                        ks
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
                self.handle_register_read(&mut state, client, uuid, key);
            }
            Request::RegisterWrite { uuid, key } => {
                self.handle_register_write(&mut state, client, uuid, key);
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
    ) {
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
            let token = lock.next_fencing_token();
            let lock_uuid = Uuid::new_v4().to_string();
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
            lock.timestamp_emptied = None;
            let queue_depth = lock.queue.len();
            self.track_holder(state, client, &lock_uuid, std::slice::from_ref(&key), RwHoldKind::Exclusive);
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

        // Otherwise queue it.
        let request_uuid = uuid.clone();
        lock.queue.push_back(
            request_uuid.clone(),
            PendingRequest {
                request_uuid: request_uuid.clone(),
                client,
                pid,
                ttl,
                keep_locks_after_death,
                kind: PendingKind::Exclusive,
            },
        );
        let depth = lock.queue.len();
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

    fn handle_composite_lock(
        &self,
        state: &mut BrokerState,
        client: ClientId,
        uuid: String,
        mut keys: Vec<String>,
        pid: Option<i64>,
        ttl: Option<Duration>,
    ) {
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
                .map(|s| s.writer.is_none() && s.readers.is_empty() && s.exclusive_holders.is_empty())
                .unwrap_or(true)
        });

        let composite_lock_uuid = Uuid::new_v4().to_string();
        if all_free {
            let mut tokens: BTreeMap<String, u64> = BTreeMap::new();
            for k in &keys {
                let lock = state.lock_or_default(k);
                let token = lock.next_fencing_token();
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
                lock.timestamp_emptied = None;
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

        // Otherwise queue on the first (smallest) key. Each grant moves us to
        // the next key, until all granted_keys == all_keys.
        let head = keys[0].clone();
        let pending = PendingRequest {
            request_uuid: uuid.clone(),
            client,
            pid,
            ttl,
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
        let mut total_unlocked = false;
        let mut last_depth: usize = 0;

        let target_lock_uuid = match lock_uuid.clone() {
            Some(v) => v,
            None if force => String::new(),
            None => {
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

        for key in &keys {
            let Some(lock) = state.locks.get_mut(key) else {
                continue;
            };
            if force {
                lock.exclusive_holders.clear();
                lock.readers.clear();
                lock.writer = None;
                total_unlocked = true;
            } else {
                let removed_exclusive =
                    lock.exclusive_holders.remove(&target_lock_uuid).is_some();
                let removed_reader = lock.readers.remove(&target_lock_uuid).is_some();
                let removed_writer = lock
                    .writer
                    .as_ref()
                    .is_some_and(|w| w.lock_uuid == target_lock_uuid);
                if removed_writer {
                    lock.writer = None;
                }
                if removed_exclusive || removed_reader || removed_writer {
                    total_unlocked = true;
                }
            }
            last_depth = lock.queue.len();
        }

        // Drop the holder bookkeeping on the client side.
        if !target_lock_uuid.is_empty() {
            if let Some(handle) = state.clients.get_mut(&client) {
                handle.held_lock_uuids.remove(&target_lock_uuid);
            }
        }

        // Now wake waiters on every key we just touched.
        for key in &keys {
            self.try_grant_next(state, key);
        }

        self.send(
            state,
            client,
            Response::Unlock {
                uuid,
                keys,
                unlocked: total_unlocked,
                lock_request_count: last_depth,
                error: None,
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
    ) {
        let lock = state.lock_or_default(&key);
        if lock.writer.is_none()
            && lock.exclusive_holders.is_empty()
            && lock.queue.is_empty()
        {
            let token = lock.next_fencing_token();
            let lock_uuid = Uuid::new_v4().to_string();
            lock.readers.insert(
                lock_uuid.clone(),
                RwHolder {
                    client,
                    fencing_token: token,
                    lock_uuid: lock_uuid.clone(),
                },
            );
            lock.timestamp_emptied = None;
            let readers = lock.readers.len() as u32;
            self.track_holder(state, client, &lock_uuid, std::slice::from_ref(&key), RwHoldKind::Read);
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
                keep_locks_after_death: false,
                kind: PendingKind::Reader,
            },
        );
        let writer_flag = lock.writer.is_some();
        let readers = lock.readers.len() as u32;
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
    ) {
        let lock = state.lock_or_default(&key);
        if lock.writer.is_none() && lock.readers.is_empty() && lock.exclusive_holders.is_empty() {
            let token = lock.next_fencing_token();
            let lock_uuid = Uuid::new_v4().to_string();
            lock.writer = Some(RwHolder {
                client,
                fencing_token: token,
                lock_uuid: lock_uuid.clone(),
            });
            lock.timestamp_emptied = None;
            self.track_holder(state, client, &lock_uuid, std::slice::from_ref(&key), RwHoldKind::Write);
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
                keep_locks_after_death: false,
                kind: PendingKind::Writer,
            },
        );
        let writer_flag = lock.writer.is_some();
        let readers = lock.readers.len() as u32;
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
    fn try_grant_next(&self, state: &mut BrokerState, key: &str) {
        loop {
            let made_progress = self.try_grant_once(state, key);
            if !made_progress {
                break;
            }
        }
    }

    fn try_grant_once(&self, state: &mut BrokerState, key: &str) -> bool {
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
                let token = lock.next_fencing_token();
                let lock_uuid = Uuid::new_v4().to_string();
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
                let token = lock.next_fencing_token();
                let lock_uuid = Uuid::new_v4().to_string();
                lock.readers.insert(
                    lock_uuid.clone(),
                    RwHolder {
                        client: head.client,
                        fencing_token: token,
                        lock_uuid: lock_uuid.clone(),
                    },
                );
                let readers = lock.readers.len() as u32;
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
                let token = lock.next_fencing_token();
                let lock_uuid = Uuid::new_v4().to_string();
                lock.writer = Some(RwHolder {
                    client: head.client,
                    fencing_token: token,
                    lock_uuid: lock_uuid.clone(),
                });
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

        let token = lock.next_fencing_token();
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

        self.untrack_pending(state, pop.client, key, &pop.request_uuid);

        if remaining_keys.is_empty() {
            // Whole composite is now granted.
            self.track_holder(
                state,
                pop.client,
                &composite_lock_uuid,
                &all_keys,
                RwHoldKind::Exclusive,
            );
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
        if let Some(handle) = state.clients.get_mut(&client) {
            handle
                .pending_request_uuids
                .retain(|(k, u)| !(k == key && u == request_uuid));
        }
    }

    fn send(&self, state: &BrokerState, client: ClientId, response: Response) {
        if let Some(handle) = state.clients.get(&client) {
            let _ = handle.sender.send(response);
        }
    }

    /// Push a `Response` directly to a connected client. Returns false if the
    /// client is gone or its outbound channel is closed. Used by transport
    /// listeners to surface protocol-level errors (e.g. malformed JSON, auth
    /// failures) without a fake `Request` round-trip.
    pub fn try_send(&self, client: ClientId, response: Response) -> bool {
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
        let mut state = self.state.lock();
        if let Some(handle) = state.clients.get_mut(&client) {
            handle.held_lock_uuids.remove(lock_uuid);
        }
    }

    /// Snapshot of broker counters used by `/metrics`.
    pub fn metrics(&self) -> BrokerMetrics {
        let state = self.state.lock();
        let mut total_holders = 0u64;
        let mut total_waiters = 0u64;
        for lock in state.locks.values() {
            total_holders += (lock.exclusive_holders.len()
                + lock.readers.len()
                + lock.writer.iter().count()) as u64;
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
        }
    }

    /// `Instant` at which this broker started accepting requests. Used by
    /// the HTML status page (upstream `live-mutex#108`) to render an
    /// uptime string. Cheap — single mutex + copy.
    pub fn started_at(&self) -> Instant {
        self.state.lock().started_at
    }

    /// Top `n` keys by current contention (`holders + waiters`),
    /// descending. Cheap-ish — walks `state.locks` once under the
    /// mutex, then partial-sorts. Intended for the HTML status page;
    /// don't call from a hot path. Returns at most `n` entries; ties
    /// are broken arbitrarily (HashMap iteration order).
    pub fn top_keys(&self, n: usize) -> Vec<KeyContentionSnapshot> {
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
            .filter(|s| {
                s.exclusive_holders + s.readers + s.writers + s.waiters > 0
            })
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
        let mut state = self.state.lock();
        // Pop everything in `..= (now, u64::MAX)` from the BTreeMap. We
        // collect the keys first to avoid holding a borrow on
        // `state.deadlines` while we mutate `state.locks`.
        let cutoff = (now, u64::MAX);
        let expired_keys: Vec<(Instant, u64)> = state
            .deadlines
            .range(..=cutoff)
            .map(|(k, _)| *k)
            .collect();
        if expired_keys.is_empty() {
            return 0;
        }

        let mut evicted_keys: std::collections::HashSet<String> =
            std::collections::HashSet::new();
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
                        RwHoldKind::Exclusive => {
                            s.exclusive_holders.contains_key(&entry.lock_uuid)
                        }
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
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            out.push(msg);
        }
        out
    }

    #[test]
    fn exclusive_lock_granted_then_queued() {
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
            assert!(token > last, "fencing token went backwards: {token} <= {last}");
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
        assert!(c_msgs.iter().any(|m| matches!(
            m,
            Response::Lock {
                acquired: true,
                ..
            }
        )));

        let b_msgs = drain(&mut b_rx);
        assert!(b_msgs.iter().any(|m| matches!(
            m,
            Response::CompositeLock {
                acquired: true,
                ..
            }
        )));
    }

    #[test]
    fn rw_writer_blocks_readers_until_done() {
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
            assert!(m.iter().any(|m| matches!(
                m,
                Response::RegisterReadResult { granted: false, .. }
            )));
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
            assert!(m.iter().any(|m| matches!(
                m,
                Response::RegisterReadResult { granted: true, .. }
            )));
        }
    }

    #[test]
    fn dropping_client_releases_locks_and_queue() {
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
        assert!(post.iter().any(|m| matches!(
            m,
            Response::Lock {
                acquired: true,
                ..
            }
        )));
    }

    /// `tick_ttl` evicts the expired holder and grants the next waiter in
    /// a single pass — without requiring a per-lock `tokio::time::sleep`.
    /// This is the structural fix from upstream `live-mutex#13`.
    #[test]
    fn tick_ttl_evicts_expired_holder_and_grants_next_waiter() {
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
            },
        );
        let pre = drain(&mut b_rx);
        assert!(pre.iter().any(|m| matches!(m, Response::Lock { acquired: false, .. })));

        // Single sweep with a synthesized future Instant — no real wall
        // time burned, no per-lock timer involved.
        let future = Instant::now() + Duration::from_secs(1);
        let evicted = broker.tick_ttl(future);
        assert_eq!(evicted, 1, "exactly one holder should have been TTL-evicted");

        // B should now hold the lock; the metric counter should reflect 1.
        let post = drain(&mut b_rx);
        assert!(
            post.iter().any(|m| matches!(m, Response::Lock { acquired: true, .. })),
            "B should be granted after A's TTL eviction; got {post:?}"
        );

        let snapshot = broker.metrics();
        assert_eq!(snapshot.ttl_evictions_total, 1);
    }

    /// A second sweep with no expired holders is a no-op — the BTreeMap
    /// `range(..=now)` returns empty and we don't touch the LockState.
    #[test]
    fn tick_ttl_is_idempotent_when_nothing_expired() {
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
            },
        );
        let _ = drain(&mut b_rx);

        let future = Instant::now() + Duration::from_secs(1);
        assert_eq!(broker.tick_ttl(future), 1);

        let post = drain(&mut b_rx);
        assert!(
            post.iter().any(|m| matches!(m, Response::Lock { acquired: true, .. })),
            "B should be granted on `x` after A's composite TTL-evicts; got {post:?}"
        );
    }

    /// Semaphore semantics: with `max=3`, three simultaneous holders are
    /// admitted on the same key, and a fourth has to queue. Each
    /// holder gets its own `lock_uuid` and a strictly increasing
    /// fencing token so a downstream resource can disambiguate slots.
    #[test]
    fn concurrency_max_three_admits_three_holders_then_queues_fourth() {
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
        assert!(grants[0].2 && grants[1].2 && grants[2].2, "first three must be granted");
        assert!(!grants[3].2, "fourth must be queued");

        // Each granted holder has a distinct lock_uuid and a unique
        // fencing token. Fencing tokens are monotonically increasing so
        // a downstream resource can order operations by recency.
        let uuids: std::collections::HashSet<_> =
            grants[..3].iter().map(|g| g.0.clone()).collect();
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
            post.iter().any(|m| matches!(m, Response::Lock { acquired: true, .. })),
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
            after.iter().any(|m| matches!(m, Response::Lock { acquired: true, .. })),
            "after force-unlock, B should hold the lock; got {after:?}"
        );
    }

    /// Releasing with the wrong `lock_uuid` is a no-op for that key —
    /// the real holder keeps holding. The broker reports
    /// `unlocked: false` so the caller can distinguish from success.
    #[test]
    fn unlock_with_wrong_lock_uuid_is_a_no_op() {
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
        assert!(err.is_some(), "empty composite must be rejected; got {msgs:?}");
    }

    /// Composite locks reject more than `MAX_COMPOSITE_KEYS` keys.
    #[test]
    fn composite_rejects_oversized_keyset() {
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
            },
        );
        let msgs = drain(&mut a_rx);
        assert!(
            msgs.iter().any(|m| matches!(m, Response::CompositeLock { acquired: false, error: Some(_), .. })),
            "oversized composite must be rejected; got {msgs:?}"
        );
    }

    /// `ttl=Some(0)` does not register a deadline — so a sweeper running
    /// arbitrarily far into the future doesn't evict the lock.
    #[test]
    fn ttl_zero_does_not_register_deadline() {
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
            },
        );
        let pre = drain(&mut c_rx);
        assert!(
            pre.iter().any(|m| matches!(m, Response::Lock { acquired: false, .. })),
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
            err.as_deref().is_some_and(|e| e.contains("`max` must be >= 1")),
            "max=0 must be rejected with a clear error; got {msgs:?}"
        );
        // No holder, no waiter, no per-key state — broker is untouched.
        let snapshot = broker.metrics();
        assert_eq!(snapshot.holders, 0, "rejected request must not register a holder");
        assert_eq!(snapshot.waiters, 0, "rejected request must not enqueue a waiter");
        assert_eq!(snapshot.keys, 0, "rejected request must not create a per-key LockState");
    }

    /// Composite path: `keys: [...]` plus `max=Some(0)` is rejected
    /// the same way, with a `compositeLock` response shape (matching
    /// the request shape) so cross-runtime clients that switch on
    /// `type` see a consistent variant.
    #[test]
    fn concurrency_max_zero_on_composite_is_rejected() {
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
        assert_eq!(broker.metrics().keys, 0, "no per-key state should be created");
    }
}
