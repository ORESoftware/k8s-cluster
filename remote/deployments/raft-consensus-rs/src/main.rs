// dd-raft-consensus
//
// An in-process Raft consensus simulator. It runs a virtual cluster of N nodes
// through a discrete-tick message-passing network, driving leader election and
// log replication under configurable chaos — message drops, time-boxed network
// partitions, and node crash/restart schedules. It is a simulator, not a real
// distributed node: deterministic, reproducible, and designed to be hammered by
// the existing loadtest harnesses, pairing with nats / network-mutex.
//
// Each run returns the committed log, the term→leader history, election counts,
// and an explicit divergence check (Raft's State Machine Safety property: no two
// nodes may commit different commands at the same index). Per-step transitions
// are fanned out on `dd.remote.raft.consensus.events` for live observation and
// chaos-test assertions.

use std::{
    collections::HashMap,
    env,
    error::Error,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dd_nats_subject_defs::{
    RAFT_CONSENSUS_EVENTS_SUBJECT, RAFT_CONSENSUS_RESULTS_SUBJECT,
    RAFT_PROPOSE_REQUESTS_QUEUE_GROUP, RAFT_PROPOSE_REQUESTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 1024 * 1024;
const MAX_NODES: usize = 9;
const MAX_TICKS: u64 = 50_000;
const MAX_COMMANDS: usize = 5_000;
const MAX_STREAM_EVENTS: usize = 4_000;
const MAX_PARTITIONS: usize = 64;
const MAX_CRASHES: usize = 256;
const DEFAULT_MAX_INFLIGHT: usize = 16;
/// Skip publishing a result larger than this (NATS default max_payload is ~1 MiB).
const MAX_PUBLISH_BYTES: usize = 900_000;

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    result_subject: String,
    event_subject: String,
    runtime_subject: String,
    metrics: Arc<Metrics>,
    /// Bounds concurrent simulations so a request/NATS flood cannot spawn
    /// unbounded CPU-heavy runs.
    inflight: Arc<tokio::sync::Semaphore>,
    /// Optional shared secret; when set, HTTP compute requests must present it.
    auth_secret: Option<String>,
}

#[derive(Default)]
struct Metrics {
    requests_total: AtomicU64,
    runs_total: AtomicU64,
    elections_total: AtomicU64,
    commits_total: AtomicU64,
    divergences_total: AtomicU64,
    errors_total: AtomicU64,
    rejected_busy_total: AtomicU64,
    auth_failures_total: AtomicU64,
    nats_messages_total: AtomicU64,
}

// ---------- PRNG ----------

struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Rng {
            state: seed ^ 0x9E3779B97F4A7C15,
        }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            lo
        } else {
            lo + self.next_u64() % (hi - lo + 1)
        }
    }
}

// ---------- Request / response ----------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RaftRequest {
    request_id: Option<String>,
    nodes: Option<usize>,
    ticks: Option<u64>,
    seed: Option<u64>,
    election_timeout_min: Option<u64>,
    election_timeout_max: Option<u64>,
    heartbeat_interval: Option<u64>,
    network_latency: Option<u64>,
    drop_probability: Option<f64>,
    /// Explicit commands; each may pin a tick. Otherwise `commandCount` commands
    /// are injected at a steady cadence.
    #[serde(default)]
    commands: Vec<CommandInput>,
    command_count: Option<usize>,
    #[serde(default)]
    partitions: Vec<PartitionInput>,
    #[serde(default)]
    crashes: Vec<CrashInput>,
    include_events: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandInput {
    value: String,
    tick: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartitionInput {
    from_tick: u64,
    to_tick: u64,
    /// Groups of node ids that can only talk within their own group.
    groups: Vec<Vec<usize>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrashInput {
    node: usize,
    down_from: u64,
    up_at: u64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RaftResponse {
    ok: bool,
    request_id: String,
    nodes: usize,
    ticks: u64,
    elections: u64,
    leader_changes: u64,
    committed_entries: usize,
    commands_submitted: usize,
    messages_sent: u64,
    messages_dropped: u64,
    safety_holds: bool,
    divergences: Vec<Divergence>,
    committed_log: Vec<CommittedEntry>,
    leader_history: Vec<LeaderTerm>,
    node_summaries: Vec<NodeSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    events: Option<Vec<Event>>,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CommittedEntry {
    index: usize,
    term: u64,
    command: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LeaderTerm {
    term: u64,
    leader: usize,
    elected_at_tick: u64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct NodeSummary {
    id: usize,
    role: String,
    current_term: u64,
    log_length: usize,
    commit_index: usize,
    alive: bool,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Divergence {
    index: usize,
    existing: String,
    conflicting: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Event {
    tick: u64,
    node: usize,
    kind: String,
    detail: String,
}

fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn env_usize(key: &str, fallback: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|&value| value > 0)
        .unwrap_or(fallback)
}

/// Resolve the optional auth secret from the service-specific key, falling back
/// to the shared `SERVER_AUTH_SECRET`. Empty values are treated as unset.
fn optional_auth_secret(primary: &str) -> Option<String> {
    [primary, "SERVER_AUTH_SECRET"]
        .iter()
        .filter_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
}

/// Timing-safe comparison so auth checks don't leak the secret via response time.
fn constant_time_equals(candidate: &str, expected: &str) -> bool {
    let candidate = candidate.as_bytes();
    let expected = expected.as_bytes();
    if candidate.len() != expected.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in candidate.iter().zip(expected.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

/// Optional shared-secret gate. Open when no secret is configured (matching the
/// sibling compute services); when set, the compute endpoint requires a matching
/// `x-server-auth` (or `auth`) header.
fn check_auth(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    let secret = state.auth_secret.as_deref()?;
    let authorized = ["x-server-auth", "auth"]
        .iter()
        .filter_map(|name| headers.get(*name))
        .filter_map(|value| value.to_str().ok())
        .any(|value| constant_time_equals(value, secret));
    if authorized {
        None
    } else {
        state
            .metrics
            .auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        Some(
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "ok": false, "error": "unauthorized" })),
            )
                .into_response(),
        )
    }
}

// ---------- Raft model ----------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Follower,
    Candidate,
    Leader,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Follower => "follower",
            Role::Candidate => "candidate",
            Role::Leader => "leader",
        }
    }
}

#[derive(Clone)]
struct Entry {
    term: u64,
    command: String,
}

struct Node {
    id: usize,
    alive: bool,
    role: Role,
    current_term: u64,
    voted_for: Option<usize>,
    log: Vec<Entry>,
    commit_index: usize,
    votes: usize,
    election_deadline: u64,
    next_index: Vec<usize>,
    match_index: Vec<usize>,
}

#[derive(Clone)]
enum Msg {
    RequestVote {
        term: u64,
        candidate: usize,
        last_log_index: usize,
        last_log_term: u64,
    },
    RequestVoteResp {
        term: u64,
        granted: bool,
    },
    AppendEntries {
        term: u64,
        leader: usize,
        prev_log_index: usize,
        prev_log_term: u64,
        entries: Vec<Entry>,
        leader_commit: usize,
    },
    AppendEntriesResp {
        term: u64,
        success: bool,
        match_index: usize,
    },
}

#[derive(Clone)]
struct Envelope {
    from: usize,
    to: usize,
    msg: Msg,
}

/// A time-boxed network partition with per-node group membership precomputed so
/// reachability is an O(1) array lookup, independent of how the request listed
/// the groups. `node_group[i]` is the group index for node i, or -1 if unlisted.
struct PartitionWindow {
    from_tick: u64,
    to_tick: u64,
    node_group: Vec<i64>,
}

struct Sim {
    nodes: Vec<Node>,
    rng: Rng,
    n: usize,
    election_min: u64,
    election_max: u64,
    heartbeat: u64,
    latency: u64,
    drop_probability: f64,
    partitions: Vec<PartitionWindow>,
    outbox: HashMap<u64, Vec<Envelope>>,
    events: Vec<Event>,
    include_events: bool,
    // bookkeeping
    elections: u64,
    leader_changes: u64,
    messages_sent: u64,
    messages_dropped: u64,
    commits_total: u64,
    committed_log: Vec<CommittedEntry>,
    leader_history: Vec<LeaderTerm>,
    divergences: Vec<Divergence>,
}

impl Sim {
    fn majority(&self) -> usize {
        self.n / 2 + 1
    }

    fn last_log_index(node: &Node) -> usize {
        node.log.len()
    }

    fn last_log_term(node: &Node) -> u64 {
        node.log.last().map(|e| e.term).unwrap_or(0)
    }

    fn record_event(&mut self, tick: u64, node: usize, kind: &str, detail: String) {
        if self.include_events && self.events.len() < MAX_STREAM_EVENTS {
            self.events.push(Event {
                tick,
                node,
                kind: kind.to_string(),
                detail,
            });
        }
    }

    fn can_communicate(&self, a: usize, b: usize, tick: u64) -> bool {
        for partition in &self.partitions {
            if tick >= partition.from_tick && tick < partition.to_tick {
                let group_a = partition.node_group[a];
                let group_b = partition.node_group[b];
                // Nodes in different groups (or either unlisted, -1) cannot talk.
                if group_a < 0 || group_b < 0 || group_a != group_b {
                    return false;
                }
            }
        }
        true
    }

    fn send(&mut self, from: usize, to: usize, msg: Msg, tick: u64) {
        self.messages_sent += 1;
        if !self.nodes[from].alive || !self.nodes[to].alive {
            self.messages_dropped += 1;
            return;
        }
        if !self.can_communicate(from, to, tick) {
            self.messages_dropped += 1;
            return;
        }
        if self.rng.uniform() < self.drop_probability {
            self.messages_dropped += 1;
            return;
        }
        let deliver_at = tick + self.latency.max(1);
        self.outbox
            .entry(deliver_at)
            .or_default()
            .push(Envelope { from, to, msg });
    }

    fn reset_election_deadline(&mut self, i: usize, tick: u64) {
        let jitter = self.rng.range(self.election_min, self.election_max);
        self.nodes[i].election_deadline = tick + jitter;
    }

    fn become_follower(&mut self, i: usize, term: u64, tick: u64) {
        let was_leader = self.nodes[i].role == Role::Leader;
        self.nodes[i].role = Role::Follower;
        self.nodes[i].current_term = term;
        self.nodes[i].voted_for = None;
        self.nodes[i].votes = 0;
        self.reset_election_deadline(i, tick);
        if was_leader {
            self.record_event(tick, i, "step-down", format!("reverted to follower at term {term}"));
        }
    }

    fn become_candidate(&mut self, i: usize, tick: u64) {
        let term = self.nodes[i].current_term + 1;
        self.nodes[i].role = Role::Candidate;
        self.nodes[i].current_term = term;
        self.nodes[i].voted_for = Some(i);
        self.nodes[i].votes = 1;
        self.reset_election_deadline(i, tick);
        self.elections += 1;
        self.record_event(tick, i, "election-start", format!("candidate for term {term}"));

        let last_log_index = Self::last_log_index(&self.nodes[i]);
        let last_log_term = Self::last_log_term(&self.nodes[i]);
        for peer in 0..self.n {
            if peer == i {
                continue;
            }
            self.send(
                i,
                peer,
                Msg::RequestVote {
                    term,
                    candidate: i,
                    last_log_index,
                    last_log_term,
                },
                tick,
            );
        }
        // Single-node cluster elects itself immediately.
        if self.majority() == 1 {
            self.become_leader(i, tick);
        }
    }

    fn become_leader(&mut self, i: usize, tick: u64) {
        self.nodes[i].role = Role::Leader;
        self.nodes[i].votes = 0;
        let next = Self::last_log_index(&self.nodes[i]) + 1;
        self.nodes[i].next_index = vec![next; self.n];
        self.nodes[i].match_index = vec![0; self.n];
        let term = self.nodes[i].current_term;
        self.leader_changes += 1;
        self.leader_history.push(LeaderTerm {
            term,
            leader: i,
            elected_at_tick: tick,
        });
        self.record_event(tick, i, "elected-leader", format!("won term {term}"));
        self.broadcast_append(i, tick);
    }

    fn broadcast_append(&mut self, i: usize, tick: u64) {
        if self.nodes[i].role != Role::Leader {
            return;
        }
        let term = self.nodes[i].current_term;
        let leader_commit = self.nodes[i].commit_index;
        for peer in 0..self.n {
            if peer == i {
                continue;
            }
            let prev_log_index = self.nodes[i].next_index[peer].saturating_sub(1);
            let prev_log_term = if prev_log_index == 0 {
                0
            } else {
                self.nodes[i]
                    .log
                    .get(prev_log_index - 1)
                    .map(|e| e.term)
                    .unwrap_or(0)
            };
            let entries: Vec<Entry> = self.nodes[i].log[prev_log_index..].to_vec();
            self.send(
                i,
                peer,
                Msg::AppendEntries {
                    term,
                    leader: i,
                    prev_log_index,
                    prev_log_term,
                    entries,
                    leader_commit,
                },
                tick,
            );
        }
    }

    fn deliver(&mut self, env: Envelope, tick: u64) {
        let to = env.to;
        if !self.nodes[to].alive {
            return;
        }
        match env.msg {
            Msg::RequestVote {
                term,
                candidate,
                last_log_index,
                last_log_term,
            } => self.handle_request_vote(to, term, candidate, last_log_index, last_log_term, tick),
            Msg::RequestVoteResp { term, granted } => {
                self.handle_vote_resp(to, env.from, term, granted, tick)
            }
            Msg::AppendEntries {
                term,
                leader,
                prev_log_index,
                prev_log_term,
                entries,
                leader_commit,
            } => self.handle_append(
                to,
                term,
                leader,
                prev_log_index,
                prev_log_term,
                entries,
                leader_commit,
                tick,
            ),
            Msg::AppendEntriesResp {
                term,
                success,
                match_index,
            } => self.handle_append_resp(to, env.from, term, success, match_index, tick),
        }
    }

    fn handle_request_vote(
        &mut self,
        i: usize,
        term: u64,
        candidate: usize,
        last_log_index: usize,
        last_log_term: u64,
        tick: u64,
    ) {
        if term > self.nodes[i].current_term {
            self.become_follower(i, term, tick);
        }
        let mut granted = false;
        if term >= self.nodes[i].current_term {
            let our_last_term = Self::last_log_term(&self.nodes[i]);
            let our_last_index = Self::last_log_index(&self.nodes[i]);
            let log_ok = last_log_term > our_last_term
                || (last_log_term == our_last_term && last_log_index >= our_last_index);
            let can_vote = self.nodes[i].voted_for.is_none()
                || self.nodes[i].voted_for == Some(candidate);
            if log_ok && can_vote {
                granted = true;
                self.nodes[i].voted_for = Some(candidate);
                self.reset_election_deadline(i, tick);
            }
        }
        let reply_term = self.nodes[i].current_term;
        self.send(
            i,
            candidate,
            Msg::RequestVoteResp {
                term: reply_term,
                granted,
            },
            tick,
        );
    }

    fn handle_vote_resp(&mut self, i: usize, _from: usize, term: u64, granted: bool, tick: u64) {
        if term > self.nodes[i].current_term {
            self.become_follower(i, term, tick);
            return;
        }
        if self.nodes[i].role != Role::Candidate || term != self.nodes[i].current_term {
            return;
        }
        if granted {
            self.nodes[i].votes += 1;
            if self.nodes[i].votes >= self.majority() {
                self.become_leader(i, tick);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_append(
        &mut self,
        i: usize,
        term: u64,
        leader: usize,
        prev_log_index: usize,
        prev_log_term: u64,
        entries: Vec<Entry>,
        leader_commit: usize,
        tick: u64,
    ) {
        if term < self.nodes[i].current_term {
            let reply_term = self.nodes[i].current_term;
            self.send(
                i,
                leader,
                Msg::AppendEntriesResp {
                    term: reply_term,
                    success: false,
                    match_index: 0,
                },
                tick,
            );
            return;
        }
        // Valid leader for this term: become/stay follower and reset timer.
        self.become_follower(i, term, tick);

        // Consistency check.
        let consistent = if prev_log_index == 0 {
            true
        } else {
            self.nodes[i]
                .log
                .get(prev_log_index - 1)
                .map(|e| e.term == prev_log_term)
                .unwrap_or(false)
        };

        if !consistent {
            let reply_term = self.nodes[i].current_term;
            self.send(
                i,
                leader,
                Msg::AppendEntriesResp {
                    term: reply_term,
                    success: false,
                    match_index: 0,
                },
                tick,
            );
            return;
        }

        // Append entries, truncating conflicts.
        let mut idx = prev_log_index;
        for entry in entries {
            idx += 1;
            if let Some(existing) = self.nodes[i].log.get(idx - 1) {
                if existing.term != entry.term {
                    self.nodes[i].log.truncate(idx - 1);
                    self.nodes[i].log.push(entry);
                }
            } else {
                self.nodes[i].log.push(entry);
            }
        }
        let match_index = idx;

        if leader_commit > self.nodes[i].commit_index {
            self.nodes[i].commit_index = leader_commit.min(self.nodes[i].log.len());
        }

        let reply_term = self.nodes[i].current_term;
        self.send(
            i,
            leader,
            Msg::AppendEntriesResp {
                term: reply_term,
                success: true,
                match_index,
            },
            tick,
        );
    }

    fn handle_append_resp(
        &mut self,
        i: usize,
        from: usize,
        term: u64,
        success: bool,
        match_index: usize,
        tick: u64,
    ) {
        if term > self.nodes[i].current_term {
            self.become_follower(i, term, tick);
            return;
        }
        if self.nodes[i].role != Role::Leader || term != self.nodes[i].current_term {
            return;
        }
        if success {
            self.nodes[i].match_index[from] = match_index;
            self.nodes[i].next_index[from] = match_index + 1;
            self.maybe_commit(i, tick);
        } else {
            let next = &mut self.nodes[i].next_index[from];
            *next = (*next).saturating_sub(1).max(1);
        }
    }

    fn maybe_commit(&mut self, i: usize, tick: u64) {
        let term = self.nodes[i].current_term;
        let log_len = self.nodes[i].log.len();
        // Find the highest index replicated on a majority whose entry is from the
        // current term (Raft commit rule).
        let mut commit = self.nodes[i].commit_index;
        for n in (self.nodes[i].commit_index + 1)..=log_len {
            if self.nodes[i].log[n - 1].term != term {
                continue;
            }
            let mut count = 1; // leader replicates to itself
            for peer in 0..self.n {
                if peer != i && self.nodes[i].match_index[peer] >= n {
                    count += 1;
                }
            }
            if count >= self.majority() {
                commit = n;
            }
        }
        if commit > self.nodes[i].commit_index {
            let old = self.nodes[i].commit_index;
            self.nodes[i].commit_index = commit;
            for index in (old + 1)..=commit {
                let entry = self.nodes[i].log[index - 1].clone();
                self.record_commit(index, entry, tick);
            }
        }
    }

    fn record_commit(&mut self, index: usize, entry: Entry, tick: u64) {
        self.commits_total += 1;
        if let Some(existing) = self.committed_log.get(index - 1) {
            // State Machine Safety violation if a committed index disagrees.
            if existing.term != entry.term || existing.command != entry.command {
                self.divergences.push(Divergence {
                    index,
                    existing: existing.command.clone(),
                    conflicting: entry.command.clone(),
                });
                self.record_event(
                    tick,
                    usize::MAX,
                    "divergence",
                    format!("index {index} committed twice with different commands"),
                );
            }
            return;
        }
        if index == self.committed_log.len() + 1 {
            self.record_event(
                tick,
                usize::MAX,
                "commit",
                format!("index {index} term {} = {}", entry.term, entry.command),
            );
            self.committed_log.push(CommittedEntry {
                index,
                term: entry.term,
                command: entry.command,
            });
        }
    }

    fn tick(&mut self, tick: u64) {
        // Deliver everything scheduled for this tick.
        if let Some(batch) = self.outbox.remove(&tick) {
            for env in batch {
                self.deliver(env, tick);
            }
        }
        // Election timeouts and leader heartbeats.
        for i in 0..self.n {
            if !self.nodes[i].alive {
                continue;
            }
            match self.nodes[i].role {
                Role::Leader => {
                    if tick.is_multiple_of(self.heartbeat) {
                        self.broadcast_append(i, tick);
                    }
                }
                _ => {
                    if tick >= self.nodes[i].election_deadline {
                        self.become_candidate(i, tick);
                    }
                }
            }
        }
    }

    fn leader(&self) -> Option<usize> {
        // The legitimate leader is the live leader with the highest term.
        let mut best: Option<usize> = None;
        for node in &self.nodes {
            if node.alive && node.role == Role::Leader {
                match best {
                    Some(b) if self.nodes[b].current_term >= node.current_term => {}
                    _ => best = Some(node.id),
                }
            }
        }
        best
    }

    fn submit(&mut self, command: String, tick: u64) -> bool {
        if let Some(leader) = self.leader() {
            let term = self.nodes[leader].current_term;
            self.nodes[leader].log.push(Entry {
                term,
                command: command.clone(),
            });
            self.record_event(tick, leader, "client-append", format!("term {term} = {command}"));
            true
        } else {
            false
        }
    }
}

fn run_simulation(request: RaftRequest) -> Result<RaftResponse, String> {
    let request_id = request
        .request_id
        .clone()
        .unwrap_or_else(|| format!("raft-{}", now_ms()));
    let n = request.nodes.unwrap_or(5).clamp(1, MAX_NODES);
    let ticks = request.ticks.unwrap_or(2_000).clamp(1, MAX_TICKS);
    let election_min = request.election_timeout_min.unwrap_or(15).max(2);
    let election_max = request.election_timeout_max.unwrap_or(30).max(election_min + 1);
    let heartbeat = request.heartbeat_interval.unwrap_or(5).max(1);
    let latency = request.network_latency.unwrap_or(1).clamp(1, 1_000);
    let drop_probability = request.drop_probability.unwrap_or(0.0);
    if !(0.0..=1.0).contains(&drop_probability) {
        return Err("dropProbability must be in [0, 1]".to_string());
    }
    if heartbeat >= election_min {
        return Err("heartbeatInterval must be smaller than electionTimeoutMin to keep leaders stable".to_string());
    }
    if request.crashes.len() > MAX_CRASHES {
        return Err(format!("too many crashes; max {MAX_CRASHES}"));
    }
    for crash in &request.crashes {
        if crash.node >= n {
            return Err(format!("crash references unknown node {}", crash.node));
        }
    }
    if request.partitions.len() > MAX_PARTITIONS {
        return Err(format!("too many partitions; max {MAX_PARTITIONS}"));
    }
    // Precompute per-node group membership so reachability checks are O(1) per
    // message regardless of how many ids the request crammed into the groups.
    let mut partition_windows: Vec<PartitionWindow> = Vec::with_capacity(request.partitions.len());
    for partition in &request.partitions {
        let mut node_group = vec![-1i64; n];
        for (group_index, group) in partition.groups.iter().enumerate() {
            for &node in group {
                if node >= n {
                    return Err(format!("partition references unknown node {node}"));
                }
                node_group[node] = group_index as i64;
            }
        }
        partition_windows.push(PartitionWindow {
            from_tick: partition.from_tick,
            to_tick: partition.to_tick,
            node_group,
        });
    }

    let mut rng = Rng::new(request.seed.unwrap_or(0x5A1D_C0DE));
    // Build the schedule of client commands.
    let mut command_schedule: Vec<(u64, String)> = Vec::new();
    if !request.commands.is_empty() {
        if request.commands.len() > MAX_COMMANDS {
            return Err(format!("too many commands; max {MAX_COMMANDS}"));
        }
        // Spread un-pinned commands across the back two-thirds of the run.
        let start = ticks / 5;
        let span = (ticks - start).max(1);
        for (k, command) in request.commands.iter().enumerate() {
            let tick = command.tick.unwrap_or_else(|| {
                start + (span * k as u64) / request.commands.len().max(1) as u64
            });
            command_schedule.push((tick.min(ticks - 1), command.value.clone()));
        }
    } else {
        let count = request.command_count.unwrap_or(20).min(MAX_COMMANDS);
        let start = ticks / 5;
        let span = (ticks - start).max(1);
        for k in 0..count {
            let tick = start + (span * k as u64) / count.max(1) as u64;
            command_schedule.push((tick.min(ticks.saturating_sub(1)), format!("cmd-{k}")));
        }
    }
    let commands_submitted_total = command_schedule.len();
    command_schedule.sort_by_key(|(tick, _)| *tick);
    let mut command_cursor = 0usize;

    // Initialise nodes with staggered election deadlines.
    let mut nodes = Vec::with_capacity(n);
    for id in 0..n {
        let deadline = rng.range(election_min, election_max);
        nodes.push(Node {
            id,
            alive: true,
            role: Role::Follower,
            current_term: 0,
            voted_for: None,
            log: Vec::new(),
            commit_index: 0,
            votes: 0,
            election_deadline: deadline,
            next_index: vec![1; n],
            match_index: vec![0; n],
        });
    }

    let mut sim = Sim {
        nodes,
        rng,
        n,
        election_min,
        election_max,
        heartbeat,
        latency,
        drop_probability,
        partitions: partition_windows,
        outbox: HashMap::new(),
        events: Vec::new(),
        include_events: request.include_events.unwrap_or(false),
        elections: 0,
        leader_changes: 0,
        messages_sent: 0,
        messages_dropped: 0,
        commits_total: 0,
        committed_log: Vec::new(),
        leader_history: Vec::new(),
        divergences: Vec::new(),
    };

    let crashes = request.crashes;
    let mut unsubmitted = 0usize;

    for current in 0..ticks {
        // Apply crash/restart schedule.
        for crash in &crashes {
            if current == crash.down_from && sim.nodes[crash.node].alive {
                sim.nodes[crash.node].alive = false;
                sim.record_event(current, crash.node, "crash", "node went down".to_string());
            }
            if current == crash.up_at && !sim.nodes[crash.node].alive {
                sim.nodes[crash.node].alive = true;
                // On restart a node resumes as a follower (persistent state kept).
                sim.nodes[crash.node].role = Role::Follower;
                sim.reset_election_deadline(crash.node, current);
                sim.record_event(current, crash.node, "restart", "node came back up".to_string());
            }
        }

        // Inject any commands scheduled for this tick.
        while command_cursor < command_schedule.len()
            && command_schedule[command_cursor].0 == current
        {
            let (_, command) = command_schedule[command_cursor].clone();
            if !sim.submit(command, current) {
                unsubmitted += 1;
            }
            command_cursor += 1;
        }

        sim.tick(current);
    }

    let safety_holds = sim.divergences.is_empty();
    let node_summaries = sim
        .nodes
        .iter()
        .map(|node| NodeSummary {
            id: node.id,
            role: node.role.as_str().to_string(),
            current_term: node.current_term,
            log_length: node.log.len(),
            commit_index: node.commit_index,
            alive: node.alive,
        })
        .collect();

    let mut warnings = Vec::new();
    if unsubmitted > 0 {
        warnings.push(format!(
            "{unsubmitted} command(s) arrived with no available leader and were dropped (expected under heavy chaos)"
        ));
    }
    if !safety_holds {
        warnings.push(
            "State Machine Safety violated: committed log diverged — this indicates a simulation bug, not normal Raft behaviour".to_string(),
        );
    }

    let include_events = sim.include_events;
    Ok(RaftResponse {
        ok: true,
        request_id,
        nodes: n,
        ticks,
        elections: sim.elections,
        leader_changes: sim.leader_changes,
        committed_entries: sim.committed_log.len(),
        commands_submitted: commands_submitted_total,
        messages_sent: sim.messages_sent,
        messages_dropped: sim.messages_dropped,
        safety_holds,
        divergences: sim.divergences,
        committed_log: sim.committed_log,
        leader_history: sim.leader_history,
        node_summaries,
        events: if include_events {
            Some(sim.events)
        } else {
            None
        },
        warnings,
        generated_at_ms: now_ms(),
    })
}

async fn run_in_background(request: RaftRequest) -> Result<RaftResponse, String> {
    tokio::task::spawn_blocking(move || run_simulation(request))
        .await
        .map_err(|error| format!("raft task join failed: {error}"))?
}

async fn publish_result(state: &AppState, response: &RaftResponse) {
    let Some(nats) = &state.nats else {
        return;
    };
    let payload = match serde_json::to_vec(&json!({
        "messageKind": "raft.consensus.result",
        "schemaVersion": "raft.consensus.v1",
        "source": "dd-raft-consensus",
        "result": response,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::error!("failed to encode raft result: {error}");
            return;
        }
    };
    if payload.len() > MAX_PUBLISH_BYTES {
        tracing::error!(
            "raft result too large to publish: bytes={} max={MAX_PUBLISH_BYTES}; the compact events summary is still sent",
            payload.len()
        );
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
    } else if let Err(error) = nats
        .publish(state.result_subject.clone(), payload.into())
        .await
    {
        tracing::error!("failed to publish raft result: {error}");
    }
    // Fan out a compact transition summary on the consensus events subject.
    let _ = nats
        .publish(
            state.event_subject.clone(),
            json!({
                "messageKind": "raft.consensus.events",
                "source": "dd-raft-consensus",
                "requestId": response.request_id,
                "elections": response.elections,
                "leaderChanges": response.leader_changes,
                "committedEntries": response.committed_entries,
                "safetyHolds": response.safety_holds,
                "leaderHistory": response.leader_history,
                "atMs": now_ms(),
            })
            .to_string()
            .into(),
        )
        .await;
    let _ = nats
        .publish(
            state.runtime_subject.clone(),
            json!({
                "type": "raft.consensus.result",
                "source": "dd-raft-consensus",
                "requestId": response.request_id,
                "nodes": response.nodes,
                "elections": response.elections,
                "committedEntries": response.committed_entries,
                "safetyHolds": response.safety_holds,
                "atMs": now_ms(),
            })
            .to_string()
            .into(),
        )
        .await;
}

fn record_metrics(metrics: &Metrics, response: &RaftResponse) {
    metrics.runs_total.fetch_add(1, Ordering::Relaxed);
    metrics
        .elections_total
        .fetch_add(response.elections, Ordering::Relaxed);
    metrics
        .commits_total
        .fetch_add(response.committed_entries as u64, Ordering::Relaxed);
    if !response.safety_holds {
        metrics
            .divergences_total
            .fetch_add(response.divergences.len() as u64, Ordering::Relaxed);
    }
}

async fn healthz() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": "dd-raft-consensus",
        "mode": "raft-sim-chaos-nats",
        "atMs": now_ms(),
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let m = &state.metrics;
    let body = format!(
        "# HELP dd_raft_requests_total HTTP simulation requests.\n\
         # TYPE dd_raft_requests_total counter\n\
         dd_raft_requests_total {}\n\
         # HELP dd_raft_runs_total Simulations completed.\n\
         # TYPE dd_raft_runs_total counter\n\
         dd_raft_runs_total {}\n\
         # HELP dd_raft_elections_total Elections held across runs.\n\
         # TYPE dd_raft_elections_total counter\n\
         dd_raft_elections_total {}\n\
         # HELP dd_raft_commits_total Committed entries across runs.\n\
         # TYPE dd_raft_commits_total counter\n\
         dd_raft_commits_total {}\n\
         # HELP dd_raft_divergences_total Safety violations detected.\n\
         # TYPE dd_raft_divergences_total counter\n\
         dd_raft_divergences_total {}\n\
         # HELP dd_raft_errors_total Simulation or message errors.\n\
         # TYPE dd_raft_errors_total counter\n\
         dd_raft_errors_total {}\n\
         # HELP dd_raft_rejected_busy_total Requests shed because the inflight cap was full.\n\
         # TYPE dd_raft_rejected_busy_total counter\n\
         dd_raft_rejected_busy_total {}\n\
         # HELP dd_raft_auth_failures_total Rejected unauthenticated/invalid-secret requests.\n\
         # TYPE dd_raft_auth_failures_total counter\n\
         dd_raft_auth_failures_total {}\n\
         # HELP dd_raft_nats_messages_total NATS simulation requests received.\n\
         # TYPE dd_raft_nats_messages_total counter\n\
         dd_raft_nats_messages_total {}\n",
        m.requests_total.load(Ordering::Relaxed),
        m.runs_total.load(Ordering::Relaxed),
        m.elections_total.load(Ordering::Relaxed),
        m.commits_total.load(Ordering::Relaxed),
        m.divergences_total.load(Ordering::Relaxed),
        m.errors_total.load(Ordering::Relaxed),
        m.rejected_busy_total.load(Ordering::Relaxed),
        m.auth_failures_total.load(Ordering::Relaxed),
        m.nats_messages_total.load(Ordering::Relaxed),
    );
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
        .into_response()
}

async fn simulate_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RaftRequest>,
) -> Response {
    if let Some(response) = check_auth(&state, &headers) {
        return response;
    }
    state.metrics.requests_total.fetch_add(1, Ordering::Relaxed);
    let Ok(_permit) = state.inflight.clone().try_acquire_owned() else {
        state
            .metrics
            .rejected_busy_total
            .fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "ok": false, "error": "server busy; retry later" })),
        )
            .into_response();
    };
    match run_in_background(request).await {
        Ok(response) => {
            record_metrics(&state.metrics, &response);
            publish_result(&state, &response).await;
            Json(response).into_response()
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    }
}

async fn run_nats_loop(state: AppState, subject: String, queue_group: String) {
    let Some(nats) = state.nats.clone() else {
        tracing::info!("raft-consensus nats loop disabled: NATS_URL is not configured");
        return;
    };
    tracing::info!(
        "raft-consensus nats loop starting: subject={subject} queue_group={queue_group} resultSubject={}",
        state.result_subject
    );
    loop {
        let mut subscription = match nats.queue_subscribe(subject.clone(), queue_group.clone()).await {
            Ok(subscription) => subscription,
            Err(error) => {
                tracing::error!("raft-consensus nats subscribe failed: {error}; retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        while let Some(message) = subscription.next().await {
            state
                .metrics
                .nats_messages_total
                .fetch_add(1, Ordering::Relaxed);
            let payload = message.payload.to_vec();
            if payload.len() > MAX_NATS_PAYLOAD_BYTES {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                tracing::error!(
                    "raft-consensus rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                    payload.len()
                );
                continue;
            }
            // Backpressure: wait for an inflight slot before taking on more work so a
            // NATS flood can't spawn unbounded simulations. NATS buffers/redelivers.
            let Ok(permit) = state.inflight.clone().acquire_owned().await else {
                continue;
            };
            let task_state = state.clone();
            tokio::spawn(async move {
                let _permit = permit;
                match serde_json::from_slice::<RaftRequest>(&payload) {
                    Ok(request) => match run_in_background(request).await {
                        Ok(response) => {
                            record_metrics(&task_state.metrics, &response);
                            publish_result(&task_state, &response).await;
                        }
                        Err(error) => {
                            task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                            tracing::error!("raft-consensus failed nats simulate: {error}");
                        }
                    },
                    Err(error) => {
                        task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                        tracing::error!("raft-consensus invalid nats request: {error}");
                    }
                }
            });
        }
        tracing::error!("raft-consensus subscription ended; re-subscribing in 5s");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = dd_telemetry::init("dd-raft-consensus");

    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8135").parse::<u16>()?;
    let nats = match env::var("NATS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(url) => match async_nats::connect(&url).await {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::error!("dd-raft-consensus NATS connect failed ({url}): {error}");
                None
            }
        },
        None => None,
    };
    let max_inflight = env_usize("RAFT_MAX_INFLIGHT", DEFAULT_MAX_INFLIGHT);
    let state = AppState {
        nats,
        result_subject: env_value("RAFT_RESULT_SUBJECT", RAFT_CONSENSUS_RESULTS_SUBJECT),
        event_subject: env_value("RAFT_EVENT_SUBJECT", RAFT_CONSENSUS_EVENTS_SUBJECT),
        runtime_subject: env_value("RAFT_RUNTIME_SUBJECT", RUNTIME_EVENTS_SUBJECT),
        metrics: Arc::new(Metrics::default()),
        inflight: Arc::new(tokio::sync::Semaphore::new(max_inflight)),
        auth_secret: optional_auth_secret("RAFT_AUTH_SECRET"),
    };
    let subject = env_value("RAFT_PROPOSE_SUBJECT", RAFT_PROPOSE_REQUESTS_SUBJECT);
    let queue_group = env_value("RAFT_QUEUE_GROUP", RAFT_PROPOSE_REQUESTS_QUEUE_GROUP);
    tokio::spawn(run_nats_loop(state.clone(), subject, queue_group));

    let app = Router::new()
        .route("/", get(healthz))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/simulate", post(simulate_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    tracing::info!("dd-raft-consensus listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state(secret: Option<String>) -> AppState {
        AppState {
            nats: None,
            result_subject: String::new(),
            event_subject: String::new(),
            runtime_subject: String::new(),
            metrics: Arc::new(Metrics::default()),
            inflight: Arc::new(tokio::sync::Semaphore::new(1)),
            auth_secret: secret,
        }
    }

    #[test]
    fn auth_open_when_no_secret() {
        assert!(check_auth(&test_state(None), &HeaderMap::new()).is_none());
    }

    #[test]
    fn auth_enforced_when_secret_set() {
        let state = test_state(Some("s3cret".to_string()));
        assert!(check_auth(&state, &HeaderMap::new()).is_some());
        let mut good = HeaderMap::new();
        good.insert("x-server-auth", "s3cret".parse().unwrap());
        assert!(check_auth(&state, &good).is_none());
    }

    fn base() -> RaftRequest {
        RaftRequest {
            request_id: None,
            nodes: Some(5),
            ticks: Some(3_000),
            seed: Some(1),
            election_timeout_min: Some(15),
            election_timeout_max: Some(30),
            heartbeat_interval: Some(5),
            network_latency: Some(1),
            drop_probability: Some(0.0),
            commands: Vec::new(),
            command_count: Some(20),
            partitions: Vec::new(),
            crashes: Vec::new(),
            include_events: Some(false),
        }
    }

    #[test]
    fn elects_leader_and_commits_in_stable_cluster() {
        let response = run_simulation(base()).unwrap();
        assert!(response.elections >= 1);
        assert!(response.leader_changes >= 1);
        assert!(response.committed_entries > 0, "expected commits, got {}", response.committed_entries);
        assert!(response.safety_holds);
    }

    #[test]
    fn safety_holds_under_message_drops() {
        let mut request = base();
        request.drop_probability = Some(0.2);
        request.seed = Some(99);
        let response = run_simulation(request).unwrap();
        assert!(response.safety_holds, "safety must hold despite drops");
    }

    #[test]
    fn safety_holds_through_partition() {
        let mut request = base();
        request.partitions = vec![PartitionInput {
            from_tick: 500,
            to_tick: 1200,
            groups: vec![vec![0, 1], vec![2, 3, 4]],
        }];
        let response = run_simulation(request).unwrap();
        assert!(response.safety_holds);
        // Majority side (2,3,4) keeps making progress.
        assert!(response.committed_entries > 0);
    }

    #[test]
    fn rejects_bad_heartbeat() {
        let mut request = base();
        request.heartbeat_interval = Some(40);
        assert!(run_simulation(request).is_err());
    }

    #[test]
    fn rejects_too_many_partitions() {
        let mut request = base();
        request.partitions = (0..=MAX_PARTITIONS)
            .map(|_| PartitionInput {
                from_tick: 0,
                to_tick: 1,
                groups: vec![vec![0]],
            })
            .collect();
        assert!(run_simulation(request).is_err());
    }
}
