//! In-memory Raft cluster simulator.
//!
//! `RaftSim` runs real [`BrokerRaft`](crate::BrokerRaft) nodes in one Tokio
//! process and routes peer RPCs through memory instead of TCP sockets. It is
//! intended for deterministic-ish integration tests, fuzz harnesses, and local
//! experiments that need the Raft backend without binding ports.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::broker::BrokerConfig;
use crate::broker_raft::{BrokerRaft, RaftInMemoryNetwork, RaftProgressSnapshot};
use crate::protocol::{Request, Response};
use crate::{BrokerRaftConfig, BrokerRaftError, RaftPeerConfig};

const DEFAULT_SIM_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(25);
const DEFAULT_SIM_ELECTION_TIMEOUT_MIN: Duration = Duration::from_millis(150);
const DEFAULT_SIM_ELECTION_TIMEOUT_MAX: Duration = Duration::from_millis(300);
const DEFAULT_SIM_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Lower bound applied to every simulation wait-for-condition timeout. Callers
/// pass values tuned for an idle machine (a few seconds); under heavy parallel
/// test load on a many-core box the simulated cluster can take longer to
/// converge, so we floor the deadline generously. This only raises the ceiling
/// before giving up — every wait returns as soon as its condition holds, so the
/// happy path is unaffected — which removes load-induced timeout flakiness.
const SIM_WAIT_FLOOR: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct RaftSimConfig {
    pub node_count: usize,
    pub data_dir: PathBuf,
    pub cleanup_data_dir: bool,
    pub broker: BrokerConfig,
    pub heartbeat_interval: Duration,
    pub election_timeout_min: Duration,
    pub election_timeout_max: Duration,
    pub snapshot_interval: Duration,
    pub snapshot_max_log_entries: u64,
    pub snapshot_max_log_bytes: u64,
    pub trailing_log_entries: u64,
    pub append_entries_max_entries: usize,
    pub append_entries_max_bytes: usize,
    pub append_entries_max_inline_batches: usize,
    pub sync_log: bool,
    pub sync_commit: bool,
    pub peer_token: Option<String>,
}

impl Default for RaftSimConfig {
    fn default() -> Self {
        crate::routine_id!("ddl-routine-raft-sim-config-default-1");
        let defaults = BrokerRaftConfig::default();
        Self {
            node_count: 3,
            data_dir: std::env::temp_dir().join(format!("lmx-raft-sim-{}", Uuid::new_v4())),
            cleanup_data_dir: true,
            broker: BrokerConfig::default(),
            heartbeat_interval: DEFAULT_SIM_HEARTBEAT_INTERVAL,
            election_timeout_min: DEFAULT_SIM_ELECTION_TIMEOUT_MIN,
            election_timeout_max: DEFAULT_SIM_ELECTION_TIMEOUT_MAX,
            snapshot_interval: defaults.snapshot_interval,
            snapshot_max_log_entries: defaults.snapshot_max_log_entries,
            snapshot_max_log_bytes: defaults.snapshot_max_log_bytes,
            trailing_log_entries: defaults.trailing_log_entries,
            append_entries_max_entries: defaults.append_entries_max_entries,
            append_entries_max_bytes: defaults.append_entries_max_bytes,
            append_entries_max_inline_batches: defaults.append_entries_max_inline_batches,
            sync_log: defaults.sync_log,
            sync_commit: defaults.sync_commit,
            peer_token: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaftSimLock {
    pub key: String,
    pub lock_uuid: String,
    pub fencing_token: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum RaftSimError {
    #[error(transparent)]
    Raft(#[from] BrokerRaftError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("raft simulation timed out after {timeout_ms}ms waiting for {operation}")]
    Timeout {
        operation: &'static str,
        timeout_ms: u64,
    },
    #[error("raft simulation node `{0}` was not found")]
    NodeNotFound(String),
    #[error("raft simulation request `{request_id}` completed without a broker response")]
    NoResponse { request_id: String },
    #[error("raft simulation request `{request_id}` returned unexpected response: {response:?}")]
    UnexpectedResponse {
        request_id: String,
        response: Box<Response>,
    },
}

pub struct RaftSim {
    nodes: BTreeMap<String, BrokerRaft>,
    tasks: BTreeMap<String, Vec<JoinHandle<()>>>,
    network: Arc<RaftInMemoryNetwork>,
    data_dir: PathBuf,
    cleanup_data_dir: bool,
}

impl RaftSim {
    pub async fn new(node_count: usize) -> Result<Self, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-new-1");
        Self::with_config(RaftSimConfig {
            node_count,
            ..RaftSimConfig::default()
        })
        .await
    }

    pub async fn with_config(config: RaftSimConfig) -> Result<Self, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-with-config-1");
        if config.node_count < 3 {
            return Err(BrokerRaftError::InvalidConfig(
                "raft simulation requires at least 3 nodes".into(),
            )
            .into());
        }
        fs::create_dir_all(&config.data_dir)?;
        let network = Arc::new(RaftInMemoryNetwork::new());
        let peers = sim_peers(config.node_count);
        let mut nodes = BTreeMap::new();

        for peer in &peers {
            let raft_config = BrokerRaftConfig {
                enabled: true,
                node_id: peer.id.clone(),
                bind_addr: Some("127.0.0.1:0".parse().expect("valid sim bind addr")),
                advertise_addr: Some(peer.addr.clone()),
                data_dir: config.data_dir.join(&peer.id),
                data_dir_lock: false,
                broker: config.broker.clone(),
                heartbeat_interval: config.heartbeat_interval,
                election_timeout_min: config.election_timeout_min,
                election_timeout_max: config.election_timeout_max,
                snapshot_interval: config.snapshot_interval,
                snapshot_max_log_entries: config.snapshot_max_log_entries,
                snapshot_max_log_bytes: config.snapshot_max_log_bytes,
                trailing_log_entries: config.trailing_log_entries,
                append_entries_max_entries: config.append_entries_max_entries,
                append_entries_max_bytes: config.append_entries_max_bytes,
                append_entries_max_inline_batches: config.append_entries_max_inline_batches,
                sync_log: config.sync_log,
                sync_commit: config.sync_commit,
                peer_token: config.peer_token.clone(),
                peers: peers.clone(),
                ..BrokerRaftConfig::default()
            };

            let node = BrokerRaft::open_in_memory(raft_config, Arc::clone(&network))?;
            network.register(node.clone());
            nodes.insert(peer.id.clone(), node);
        }

        let mut tasks = BTreeMap::new();
        for (node_id, node) in &nodes {
            tasks.insert(node_id.clone(), node.spawn_in_memory_raft_tasks());
        }

        Ok(Self {
            nodes,
            tasks,
            network,
            data_dir: config.data_dir,
            cleanup_data_dir: config.cleanup_data_dir,
        })
    }

    pub fn data_dir(&self) -> &Path {
        crate::routine_id!("ddl-routine-raft-sim-data-dir-1");
        &self.data_dir
    }

    pub fn node_ids(&self) -> Vec<String> {
        crate::routine_id!("ddl-routine-raft-sim-node-ids-1");
        self.nodes.keys().cloned().collect()
    }

    pub fn node(&self, node_id: &str) -> Option<&BrokerRaft> {
        crate::routine_id!("ddl-routine-raft-sim-node-1");
        self.nodes.get(node_id)
    }

    pub fn nodes(&self) -> &BTreeMap<String, BrokerRaft> {
        crate::routine_id!("ddl-routine-raft-sim-nodes-1");
        &self.nodes
    }

    pub fn restart_node(&mut self, node_id: &str) -> Result<(), RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-restart-node-1");
        let config = self
            .nodes
            .get(node_id)
            .ok_or_else(|| RaftSimError::NodeNotFound(node_id.to_string()))?
            .config()
            .clone();
        if let Some(handles) = self.tasks.remove(node_id) {
            for handle in handles {
                handle.abort();
            }
        }
        self.network.unregister(node_id);
        self.nodes.remove(node_id);

        let node = BrokerRaft::open_in_memory(config, Arc::clone(&self.network))?;
        self.network.register(node.clone());
        self.tasks
            .insert(node_id.to_string(), node.spawn_in_memory_raft_tasks());
        self.nodes.insert(node_id.to_string(), node);
        Ok(())
    }

    pub fn add_node(&mut self, node_id: impl Into<String>) -> Result<RaftPeerConfig, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-add-node-1");
        let node_id = node_id.into();
        if self.nodes.contains_key(&node_id) {
            return Err(BrokerRaftError::InvalidConfig(format!(
                "raft simulation node `{node_id}` already exists"
            ))
            .into());
        }
        let template = self.nodes.values().next().ok_or_else(|| {
            BrokerRaftError::InvalidConfig("raft simulation has no existing config template".into())
        })?;
        let peer = RaftPeerConfig {
            id: node_id.clone(),
            addr: format!("memory://{node_id}"),
        };
        let mut config = template.config().clone();
        config.node_id = node_id.clone();
        config.bind_addr = Some("127.0.0.1:0".parse().expect("valid sim bind addr"));
        config.advertise_addr = Some(peer.addr.clone());
        config.data_dir = self.data_dir.join(&node_id);
        config.data_dir_lock = false;
        config.peers = template.active_peers();

        let node = BrokerRaft::open_in_memory(config, Arc::clone(&self.network))?;
        self.network.register(node.clone());
        self.tasks
            .insert(node_id.clone(), node.spawn_in_memory_raft_tasks());
        self.nodes.insert(node_id, node);
        Ok(peer)
    }

    pub fn progress(&self) -> Vec<RaftProgressSnapshot> {
        crate::routine_id!("ddl-routine-raft-sim-progress-1");
        self.nodes
            .values()
            .map(BrokerRaft::progress_snapshot)
            .collect()
    }

    pub fn leader(&self) -> Option<BrokerRaft> {
        crate::routine_id!("ddl-routine-raft-sim-leader-1");
        self.nodes.values().find(|node| node.is_leader()).cloned()
    }

    pub fn ready_leader(&self) -> Option<BrokerRaft> {
        crate::routine_id!("ddl-routine-raft-sim-ready-leader-1");
        self.nodes
            .values()
            .find(|node| node.is_leader_ready())
            .cloned()
    }

    pub async fn wait_for_leader(&self, timeout: Duration) -> Result<BrokerRaft, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-wait-leader-1");
        let deadline = deadline_after(timeout.max(SIM_WAIT_FLOOR));
        loop {
            if let Some(leader) = self.ready_leader() {
                return Ok(leader);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(RaftSimError::Timeout {
                    operation: "ready raft leader",
                    timeout_ms: duration_ms_u64(timeout),
                });
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub async fn wait_for_quorum_commit(
        &self,
        index: u64,
        timeout: Duration,
    ) -> Result<Vec<RaftProgressSnapshot>, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-wait-quorum-commit-1");
        let deadline = deadline_after(timeout.max(SIM_WAIT_FLOOR));
        loop {
            let progress = self.progress();
            let membership = progress
                .iter()
                .find(|snapshot| snapshot.is_leader || snapshot.leader_id.is_some())
                .or_else(|| progress.first())
                .map(|snapshot| snapshot.membership.clone());
            let committed = progress
                .iter()
                .filter(|snapshot| snapshot.commit_index >= index)
                .map(|snapshot| snapshot.node_id.clone())
                .collect::<BTreeSet<_>>();
            if membership
                .as_ref()
                .is_some_and(|membership| membership.quorum_met(&committed))
            {
                return Ok(progress);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(RaftSimError::Timeout {
                    operation: "raft quorum commit",
                    timeout_ms: duration_ms_u64(timeout),
                });
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub async fn run_on_node(
        &self,
        node_id: &str,
        request: Request,
        request_uuid: &str,
        wait: Duration,
        is_acquire: bool,
    ) -> Result<Option<Response>, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-run-node-1");
        retry_sim_response(
            "raft node response",
            DEFAULT_SIM_REQUEST_TIMEOUT,
            |_| {
                let request = request.clone();
                async move {
                    let node = self
                        .nodes
                        .get(node_id)
                        .ok_or_else(|| RaftSimError::NodeNotFound(node_id.to_string()))?;
                    node.run_ephemeral(request, request_uuid, wait, is_acquire)
                        .await
                        .map_err(Into::into)
                }
            },
            |_| false,
        )
        .await
    }

    pub async fn run_on_leader(
        &self,
        request: Request,
        request_uuid: &str,
        wait: Duration,
        is_acquire: bool,
    ) -> Result<Option<Response>, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-run-leader-1");
        retry_sim_response(
            "ready raft leader",
            DEFAULT_SIM_REQUEST_TIMEOUT,
            |remaining| {
                let request = request.clone();
                async move {
                    let leader = self.wait_for_leader(remaining).await?;
                    leader
                        .run_ephemeral(request, request_uuid, wait, is_acquire)
                        .await
                        .map_err(Into::into)
                }
            },
            retryable_sim_leader_error,
        )
        .await
    }

    pub async fn change_membership(&self, peers: Vec<RaftPeerConfig>) -> Result<u64, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-change-membership-1");
        let deadline = deadline_after(DEFAULT_SIM_REQUEST_TIMEOUT);
        let mut last_not_leader: Option<BrokerRaftError> = None;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                if let Some(err) = last_not_leader {
                    return Err(err.into());
                }
                return Err(RaftSimError::Timeout {
                    operation: "ready raft leader",
                    timeout_ms: duration_ms_u64(DEFAULT_SIM_REQUEST_TIMEOUT),
                });
            }
            let leader = self
                .wait_for_leader(deadline.saturating_duration_since(now))
                .await?;
            match leader.change_membership(peers.clone()).await {
                Ok(index) => return Ok(index),
                Err(err @ BrokerRaftError::NotLeader { .. }) => {
                    last_not_leader = Some(err);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(err) => return Err(err.into()),
            }
        }
    }

    pub async fn acquire(&self, key: impl Into<String>) -> Result<RaftSimLock, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-acquire-1");
        self.acquire_with_wait(key, Duration::ZERO).await
    }

    pub async fn acquire_with_wait(
        &self,
        key: impl Into<String>,
        wait: Duration,
    ) -> Result<RaftSimLock, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-acquire-wait-1");
        let key = key.into();
        let request_uuid = format!("sim-acquire-{}", Uuid::new_v4());
        let response = self
            .run_on_leader(
                Request::Lock {
                    uuid: request_uuid.clone(),
                    key: Some(key.clone()),
                    keys: None,
                    pid: None,
                    ttl: None,
                    max: None,
                    force: false,
                    retry_count: 0,
                    keep_locks_after_death: false,
                    wait: None,
                },
                &request_uuid,
                wait,
                true,
            )
            .await?
            .ok_or_else(|| RaftSimError::NoResponse {
                request_id: request_uuid.clone(),
            })?;
        match response {
            Response::Lock {
                acquired: true,
                lock_uuid: Some(lock_uuid),
                fencing_token,
                ..
            } => Ok(RaftSimLock {
                key,
                lock_uuid,
                fencing_token,
            }),
            response => Err(RaftSimError::UnexpectedResponse {
                request_id: request_uuid,
                response: Box::new(response),
            }),
        }
    }

    pub async fn release(&self, lock: &RaftSimLock) -> Result<Response, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-release-1");
        self.release_key(&lock.key, Some(lock.lock_uuid.clone()), false)
            .await
    }

    pub async fn release_key(
        &self,
        key: impl Into<String>,
        lock_uuid: Option<String>,
        force: bool,
    ) -> Result<Response, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-release-key-1");
        let key = key.into();
        let request_uuid = format!("sim-release-{}", Uuid::new_v4());
        self.run_on_leader(
            Request::Unlock {
                uuid: request_uuid.clone(),
                key: Some(key),
                keys: None,
                lock_uuid,
                force,
            },
            &request_uuid,
            Duration::from_secs(2),
            false,
        )
        .await?
        .ok_or(RaftSimError::NoResponse {
            request_id: request_uuid,
        })
    }

    pub fn disconnect_node(&self, node_id: &str) {
        crate::routine_id!("ddl-routine-raft-sim-disconnect-node-1");
        self.network.disconnect_node(node_id);
    }

    pub fn reconnect_node(&self, node_id: &str) {
        crate::routine_id!("ddl-routine-raft-sim-reconnect-node-1");
        self.network.reconnect_node(node_id);
    }

    pub fn partition(&self, left: &[impl AsRef<str>], right: &[impl AsRef<str>]) {
        crate::routine_id!("ddl-routine-raft-sim-partition-1");
        let left = left
            .iter()
            .map(|node_id| node_id.as_ref().to_string())
            .collect::<Vec<_>>();
        let right = right
            .iter()
            .map(|node_id| node_id.as_ref().to_string())
            .collect::<Vec<_>>();
        self.network.partition(&left, &right);
    }

    pub fn heal(&self) {
        crate::routine_id!("ddl-routine-raft-sim-heal-1");
        self.network.heal();
    }
}

impl Drop for RaftSim {
    fn drop(&mut self) {
        crate::routine_id!("ddl-routine-raft-sim-drop-1");
        for handles in self.tasks.values() {
            for handle in handles {
                handle.abort();
            }
        }
        for node_id in self.nodes.keys() {
            self.network.unregister(node_id);
        }
        if self.cleanup_data_dir {
            let _ = fs::remove_dir_all(&self.data_dir);
        }
    }
}

fn sim_peers(node_count: usize) -> Vec<RaftPeerConfig> {
    crate::routine_id!("ddl-routine-raft-sim-peers-1");
    (1..=node_count)
        .map(|idx| RaftPeerConfig {
            id: format!("node-{idx}"),
            addr: format!("memory://node-{idx}"),
        })
        .collect()
}

fn deadline_after(timeout: Duration) -> tokio::time::Instant {
    crate::routine_id!("ddl-routine-raft-sim-deadline-after-1");
    let now = tokio::time::Instant::now();
    now.checked_add(timeout)
        .unwrap_or_else(|| now + Duration::from_secs(365 * 24 * 60 * 60))
}

async fn retry_sim_response<F, Fut, R>(
    operation: &'static str,
    timeout: Duration,
    mut call: F,
    mut retry_error: R,
) -> Result<Option<Response>, RaftSimError>
where
    F: FnMut(Duration) -> Fut,
    Fut: Future<Output = Result<Option<Response>, RaftSimError>>,
    R: FnMut(&RaftSimError) -> bool,
{
    crate::routine_id!("ddl-routine-raft-sim-retry-response-1");
    let deadline = deadline_after(timeout);
    let mut last_retryable_error = None;
    let mut saw_pending_response = false;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            if let Some(err) = last_retryable_error {
                return Err(err);
            }
            if saw_pending_response {
                return Ok(None);
            }
            return Err(RaftSimError::Timeout {
                operation,
                timeout_ms: duration_ms_u64(timeout),
            });
        }

        match call(deadline.saturating_duration_since(now)).await {
            Ok(Some(response)) => return Ok(Some(response)),
            Ok(None) => {
                saw_pending_response = true;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(err) if retry_error(&err) => {
                last_retryable_error = Some(err);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

fn retryable_sim_leader_error(err: &RaftSimError) -> bool {
    crate::routine_id!("ddl-routine-raft-sim-retryable-leader-error-1");
    matches!(
        err,
        RaftSimError::Raft(
            BrokerRaftError::NotLeader { .. }
                | BrokerRaftError::QuorumUnavailable { .. }
                | BrokerRaftError::ClientProposalUncertain { .. }
        )
    )
}

fn duration_ms_u64(duration: Duration) -> u64 {
    crate::routine_id!("ddl-routine-raft-sim-duration-ms-1");
    duration.as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RaftMembership;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_cluster_elects_leader_and_replicates_lock() {
        let sim = RaftSim::new(3).await.expect("start in-memory raft sim");
        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader elected");
        assert!(leader.is_leader_ready());

        let lock = sim
            .acquire(format!("sim-key-{}", Uuid::new_v4()))
            .await
            .expect("acquire through raft sim");
        let release = sim.release(&lock).await.expect("release through raft sim");
        assert!(matches!(release, Response::Unlock { unlocked: true, .. }));

        let target_index = sim
            .ready_leader()
            .expect("ready leader after release")
            .commit_index();
        let committed = sim
            .wait_for_quorum_commit(target_index, Duration::from_secs(2))
            .await
            .expect("quorum commit after release")
            .into_iter()
            .filter(|progress| progress.commit_index >= target_index)
            .count();
        assert!(committed >= 2, "expected quorum to commit lock and unlock");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_cluster_proxies_requests_from_followers() {
        let sim = RaftSim::new(3).await.expect("start in-memory raft sim");
        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader elected");
        let follower_id = sim
            .node_ids()
            .into_iter()
            .find(|node_id| node_id != leader.config().node_id.as_str())
            .expect("follower id");
        let request_uuid = format!("sim-follower-acquire-{}", Uuid::new_v4());
        let key = format!("sim-follower-key-{}", Uuid::new_v4());
        let response = sim
            .run_on_node(
                &follower_id,
                Request::Lock {
                    uuid: request_uuid.clone(),
                    key: Some(key),
                    keys: None,
                    pid: None,
                    ttl: None,
                    max: None,
                    force: false,
                    retry_count: 0,
                    keep_locks_after_death: false,
                    wait: None,
                },
                &request_uuid,
                Duration::ZERO,
                true,
            )
            .await
            .expect("proxied follower request")
            .expect("broker response");
        assert!(matches!(response, Response::Lock { acquired: true, .. }));
    }

    #[tokio::test]
    async fn retry_sim_response_retries_pending_until_response_arrives() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let response = Response::Auth {
            uuid: "retry-pending".to_string(),
            ok: true,
            error: None,
        };

        let result = retry_sim_response(
            "pending response",
            Duration::from_secs(1),
            |_| {
                let attempts = Arc::clone(&attempts);
                let response = response.clone();
                async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt < 2 {
                        Ok(None)
                    } else {
                        Ok(Some(response))
                    }
                }
            },
            |_| false,
        )
        .await
        .expect("pending response retry should not error")
        .expect("response should arrive");

        assert!(matches!(result, Response::Auth { ok: true, .. }));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_sim_response_retries_configured_transient_errors() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let response = Response::Auth {
            uuid: "retry-error".to_string(),
            ok: true,
            error: None,
        };

        let result = retry_sim_response(
            "transient leader error",
            Duration::from_secs(1),
            |_| {
                let attempts = Arc::clone(&attempts);
                let response = response.clone();
                async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt < 2 {
                        Err(RaftSimError::Raft(BrokerRaftError::NotLeader {
                            leader_id: Some("node-2".to_string()),
                            leader_addr: None,
                        }))
                    } else {
                        Ok(Some(response))
                    }
                }
            },
            |err| matches!(err, RaftSimError::Raft(BrokerRaftError::NotLeader { .. })),
        )
        .await
        .expect("transient leader errors should retry")
        .expect("response should arrive");

        assert!(matches!(result, Response::Auth { ok: true, .. }));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_seeded_chaos_preserves_lock_model_after_failover_and_restarts() {
        let mut sim = RaftSim::new(3).await.expect("start in-memory raft sim");
        let mut rng = SimRng::new(0xC0DE_5EED_D15C_A11C);
        let node_ids = sim.node_ids();
        let keys = (0..5)
            .map(|idx| format!("sim-chaos-key-{idx}-{}", Uuid::new_v4()))
            .collect::<Vec<_>>();
        let mut held = BTreeMap::<String, RaftSimLock>::new();

        let healthy_full_log_before = full_log_metrics_for_nodes(&sim, &node_ids);
        let pre_partition_index =
            run_seeded_lock_model_steps(&sim, &node_ids, &keys, &mut held, &mut rng, 12, "healthy")
                .await
                .expect("healthy seeded lock-model operations should complete");
        wait_for_all_applied(&sim, pre_partition_index, Duration::from_secs(5))
            .await
            .expect("healthy operations should apply everywhere before partition");
        assert_full_log_metrics_unchanged(
            &sim,
            &healthy_full_log_before,
            "healthy seeded chaos traffic",
        );

        let old_leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader before chaos partition");
        let old_leader_id = old_leader.config().node_id.clone();
        let majority = node_ids
            .iter()
            .filter(|node_id| *node_id != &old_leader_id)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(majority.len(), 2);

        sim.partition(
            &[old_leader_id.as_str()],
            &[majority[0].as_str(), majority[1].as_str()],
        );

        let stale_leader_full_log_before =
            full_log_metrics_for_nodes(&sim, std::slice::from_ref(&old_leader_id));
        let stale_commit_before = old_leader.commit_index();
        let stale_apply_before = old_leader.progress_snapshot().last_applied;
        let stale_result = acquire_key_on_node(
            &sim,
            &old_leader_id,
            format!("sim-chaos-stale-{}", Uuid::new_v4()),
            "stale-leader",
        )
        .await;
        assert!(
            stale_result.is_err() || stale_result.as_ref().is_ok_and(Option::is_none),
            "isolated stale leader must not grant a write: {stale_result:?}"
        );
        let stale_progress = old_leader.progress_snapshot();
        assert_eq!(
            stale_progress.commit_index, stale_commit_before,
            "isolated stale leader must not commit during minority partition"
        );
        assert_eq!(
            stale_progress.last_applied, stale_apply_before,
            "isolated stale leader must not apply during minority partition"
        );
        assert_full_log_metrics_unchanged(
            &sim,
            &stale_leader_full_log_before,
            "seeded chaos stale-leader minority write attempt",
        );

        wait_for_ready_leader_in(&sim, &majority, Duration::from_secs(5))
            .await
            .expect("majority partition should elect a ready leader");
        let partition_full_log_before = full_log_metrics_for_nodes(&sim, &majority);
        let partition_index = run_seeded_lock_model_steps(
            &sim,
            &majority,
            &keys,
            &mut held,
            &mut rng,
            8,
            "majority-partition",
        )
        .await
        .expect("majority-side seeded lock-model operations should complete");
        let majority_ids = majority.iter().cloned().collect::<BTreeSet<_>>();
        wait_for_applied_on(&sim, &majority_ids, partition_index, Duration::from_secs(5))
            .await
            .expect("majority-side operations should apply on the majority partition");
        assert_full_log_metrics_unchanged(
            &sim,
            &partition_full_log_before,
            "seeded chaos majority-partition traffic",
        );

        sim.restart_node(&old_leader_id)
            .expect("restart isolated old leader with durable local state");
        let repair_full_log_before = full_log_metrics_for_nodes(&sim, &node_ids);
        sim.heal();
        wait_for_all_applied(&sim, partition_index, Duration::from_secs(5))
            .await
            .expect("healed cluster should repair and apply majority partition writes");
        assert_full_log_metrics_unchanged(
            &sim,
            &repair_full_log_before,
            "seeded chaos stale-leader restart repair",
        );

        for node_id in &node_ids {
            let survivors = node_ids
                .iter()
                .filter(|candidate| *candidate != node_id)
                .cloned()
                .collect::<Vec<_>>();
            let survivor_full_log_before = full_log_metrics_for_nodes(&sim, &survivors);
            sim.restart_node(node_id)
                .expect("restart healed raft node during chaos test");
            sim.wait_for_leader(Duration::from_secs(5))
                .await
                .expect("cluster should keep electing after rolling restart");
            assert_full_log_metrics_unchanged(
                &sim,
                &survivor_full_log_before,
                "seeded chaos survivor convergence during rolling restart",
            );
        }
        wait_for_all_applied(&sim, partition_index, Duration::from_secs(5))
            .await
            .expect("rolling restarts should preserve applied state");

        let cleanup_full_log_before = full_log_metrics_for_nodes(&sim, &node_ids);
        release_all_held_on_seeded_nodes(&sim, &node_ids, &mut held, &mut rng, "cleanup")
            .await
            .expect("cleanup releases should complete after restarts");
        let cleanup_index = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("ready leader after cleanup")
            .commit_index();
        wait_for_all_applied(&sim, cleanup_index, Duration::from_secs(5))
            .await
            .expect("cleanup releases should apply on every node");
        assert_full_log_metrics_unchanged(
            &sim,
            &cleanup_full_log_before,
            "seeded chaos cleanup releases",
        );

        let post_cleanup_full_log_before = full_log_metrics_for_nodes(&sim, &node_ids);
        for (idx, key) in keys.iter().enumerate() {
            let node_id = &node_ids[idx % node_ids.len()];
            let lock = acquire_key_on_node(&sim, node_id, key.clone(), "post-cleanup")
                .await
                .expect("post-cleanup acquire should reach leader")
                .expect("post-cleanup key should grant");
            release_lock_on_node(&sim, node_id, &lock, "post-cleanup")
                .await
                .expect("post-cleanup release should succeed");
        }
        let post_cleanup_index = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader after post-cleanup seeded chaos traffic")
            .commit_index();
        wait_for_all_applied(&sim, post_cleanup_index, Duration::from_secs(5))
            .await
            .expect("post-cleanup seeded chaos traffic should apply on every node");
        assert_full_log_metrics_unchanged(
            &sim,
            &post_cleanup_full_log_before,
            "seeded chaos post-cleanup traffic",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn in_memory_concurrent_partition_heal_preserves_single_key_linearizability() {
        #[derive(Debug, Clone)]
        struct GrantInterval {
            key: String,
            lock_uuid: String,
            acquire_order: usize,
            release_invocation_order: usize,
        }

        let sim = Arc::new(RaftSim::new(3).await.expect("start in-memory raft sim"));
        let node_ids = sim.node_ids();
        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader before concurrent partition");
        let old_leader_id = leader.config().node_id.clone();
        let majority = node_ids
            .iter()
            .filter(|node_id| *node_id != &old_leader_id)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(majority.len(), 2);
        sim.partition(
            &[old_leader_id.as_str()],
            &[majority[0].as_str(), majority[1].as_str()],
        );
        wait_for_ready_leader_in(&sim, &majority, Duration::from_secs(5))
            .await
            .expect("majority partition should elect a ready leader");

        let keys = (0..3)
            .map(|idx| format!("sim-linear-key-{idx}-{}", Uuid::new_v4()))
            .collect::<Vec<_>>();
        let unreleased = Arc::new(Mutex::new(BTreeMap::<String, String>::new()));
        let history = Arc::new(Mutex::new(Vec::<GrantInterval>::new()));
        let order = Arc::new(AtomicUsize::new(1));
        let healed = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();

        for worker in 0..8 {
            let sim = Arc::clone(&sim);
            let majority = majority.clone();
            let all_nodes = node_ids.clone();
            let keys = keys.clone();
            let unreleased = Arc::clone(&unreleased);
            let history = Arc::clone(&history);
            let order = Arc::clone(&order);
            let healed = Arc::clone(&healed);
            tasks.push(tokio::spawn(async move {
                let mut state = 0xA11C_E000_u64 + worker as u64;
                for step in 0..24 {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    let key = keys[(state as usize) % keys.len()].clone();
                    let candidate_nodes = if healed.load(Ordering::SeqCst) == 0 {
                        &majority
                    } else {
                        &all_nodes
                    };
                    let node_id = &candidate_nodes
                        [(state.rotate_left(17) as usize + step) % candidate_nodes.len()];
                    let lock =
                        match acquire_key_on_node(&sim, node_id, key.clone(), "linear-partition")
                            .await
                        {
                            Ok(Some(lock)) => lock,
                            Ok(None) => {
                                tokio::time::sleep(Duration::from_millis(1)).await;
                                continue;
                            }
                            Err(err) if retryable_sim_leader_error(&err) => {
                                tokio::time::sleep(Duration::from_millis(2)).await;
                                continue;
                            }
                            Err(err) => panic!("linearizability acquire failed: {err:?}"),
                        };
                    let acquire_order = order.fetch_add(1, Ordering::SeqCst);
                    {
                        let mut held = unreleased.lock().expect("unreleased lock map");
                        assert!(
                            held.insert(lock.key.clone(), lock.lock_uuid.clone())
                                .is_none(),
                            "two successful Raft lock grants overlapped before any release was invoked for key {}",
                            lock.key
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(1 + ((worker + step) % 4) as u64))
                        .await;
                    let release_invocation_order = order.fetch_add(1, Ordering::SeqCst);
                    {
                        let mut held = unreleased.lock().expect("unreleased lock map");
                        assert_eq!(
                            held.remove(&lock.key).as_deref(),
                            Some(lock.lock_uuid.as_str()),
                            "release observed a different unreleased holder for {}",
                            lock.key
                        );
                    }
                    release_lock_on_node(&sim, node_id, &lock, "linear-partition")
                        .await
                        .expect("linearizability release should succeed");
                    history.lock().expect("history").push(GrantInterval {
                        key: lock.key,
                        lock_uuid: lock.lock_uuid,
                        acquire_order,
                        release_invocation_order,
                    });
                }
            }));
        }

        tokio::time::sleep(Duration::from_millis(125)).await;
        sim.heal();
        healed.store(1, Ordering::SeqCst);

        for task in tasks {
            task.await
                .expect("concurrent linearizability worker should not panic");
        }
        assert!(
            unreleased.lock().expect("unreleased lock map").is_empty(),
            "all successful grants should have been released"
        );
        let final_index = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader after concurrent partition heal")
            .commit_index();
        wait_for_all_applied(&sim, final_index, Duration::from_secs(5))
            .await
            .expect("concurrent partition/heal history should apply on all nodes");

        let history = history.lock().expect("history").clone();
        assert!(
            !history.is_empty(),
            "concurrent partition/heal test should record successful grants"
        );
        for (left_idx, left) in history.iter().enumerate() {
            for right in history.iter().skip(left_idx + 1) {
                if left.key == right.key {
                    assert!(
                        left.release_invocation_order < right.acquire_order
                            || right.release_invocation_order < left.acquire_order,
                        "successful grants for {} overlapped before a release invocation: left={left:?} right={right:?}",
                        left.key
                    );
                    assert_ne!(
                        left.lock_uuid, right.lock_uuid,
                        "distinct grants for {} should not reuse lock UUIDs",
                        left.key
                    );
                }
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn in_memory_partition_heal_no_wait_history_is_linearizable() {
        let sim = Arc::new(RaftSim::new(3).await.expect("start in-memory raft sim"));
        let node_ids = sim.node_ids();
        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader before no-wait history partition");
        let old_leader_id = leader.config().node_id.clone();
        let majority = node_ids
            .iter()
            .filter(|node_id| *node_id != &old_leader_id)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(majority.len(), 2);
        sim.partition(
            &[old_leader_id.as_str()],
            &[majority[0].as_str(), majority[1].as_str()],
        );
        wait_for_ready_leader_in(&sim, &majority, Duration::from_secs(5))
            .await
            .expect("majority partition should elect a ready leader");

        let keys = (0..2)
            .map(|idx| format!("sim-linear-history-key-{idx}-{}", Uuid::new_v4()))
            .collect::<Vec<_>>();
        let history = Arc::new(Mutex::new(Vec::<LockHistoryOp>::new()));
        let order = Arc::new(AtomicUsize::new(1));
        let healed = Arc::new(AtomicUsize::new(0));
        let full_log_before = full_log_metrics_for_nodes(&sim, &node_ids);
        let mut tasks = Vec::new();

        for worker in 0..4 {
            let sim = Arc::clone(&sim);
            let majority = majority.clone();
            let all_nodes = node_ids.clone();
            let keys = keys.clone();
            let history = Arc::clone(&history);
            let order = Arc::clone(&order);
            let healed = Arc::clone(&healed);
            tasks.push(tokio::spawn(async move {
                let mut state = 0x1EAF_5EED_u64 + worker as u64;
                for step in 0..10 {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    let key = keys[(state as usize) % keys.len()].clone();
                    let candidate_nodes = if healed.load(Ordering::SeqCst) == 0 {
                        &majority
                    } else {
                        &all_nodes
                    };
                    let node_id = &candidate_nodes
                        [(state.rotate_left(11) as usize + step) % candidate_nodes.len()];
                    let acquire_invoke = order.fetch_add(1, Ordering::SeqCst);
                    let acquire =
                        acquire_key_on_node(&sim, node_id, key.clone(), "linear-history").await;
                    let acquire_response = order.fetch_add(1, Ordering::SeqCst);
                    let lock = match acquire {
                        Ok(Some(lock)) => {
                            history.lock().expect("history").push(LockHistoryOp {
                                key: key.clone(),
                                invoke_order: acquire_invoke,
                                response_order: acquire_response,
                                result: LockHistoryResult::Acquired {
                                    lock_uuid: lock.lock_uuid.clone(),
                                },
                            });
                            Some(lock)
                        }
                        Ok(None) => {
                            history.lock().expect("history").push(LockHistoryOp {
                                key,
                                invoke_order: acquire_invoke,
                                response_order: acquire_response,
                                result: LockHistoryResult::NotAcquired,
                            });
                            tokio::time::sleep(Duration::from_millis(1)).await;
                            None
                        }
                        Err(err) if retryable_sim_leader_error(&err) => {
                            tokio::time::sleep(Duration::from_millis(2)).await;
                            None
                        }
                        Err(err) => panic!("linearizable history acquire failed: {err:?}"),
                    };
                    let Some(lock) = lock else {
                        continue;
                    };
                    tokio::time::sleep(Duration::from_millis(1 + ((worker + step) % 3) as u64))
                        .await;
                    let release_invoke = order.fetch_add(1, Ordering::SeqCst);
                    release_lock_on_node(&sim, node_id, &lock, "linear-history")
                        .await
                        .expect("linearizable history release should succeed");
                    let release_response = order.fetch_add(1, Ordering::SeqCst);
                    history.lock().expect("history").push(LockHistoryOp {
                        key: lock.key,
                        invoke_order: release_invoke,
                        response_order: release_response,
                        result: LockHistoryResult::Released {
                            lock_uuid: lock.lock_uuid,
                        },
                    });
                }
            }));
        }

        tokio::time::sleep(Duration::from_millis(75)).await;
        sim.heal();
        healed.store(1, Ordering::SeqCst);

        for task in tasks {
            task.await
                .expect("linearizable history worker should not panic");
        }
        let final_index = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader after linearizable history heal")
            .commit_index();
        wait_for_all_applied(&sim, final_index, Duration::from_secs(5))
            .await
            .expect("linearizable history should apply on all nodes");
        assert_full_log_metrics_unchanged(
            &sim,
            &full_log_before,
            "partition/heal no-wait history traffic",
        );

        let history = history.lock().expect("history").clone();
        assert!(
            history
                .iter()
                .any(|op| matches!(op.result, LockHistoryResult::Acquired { .. })),
            "linearizable history test should record successful acquires"
        );
        assert!(
            history
                .iter()
                .any(|op| matches!(op.result, LockHistoryResult::NotAcquired)),
            "linearizable history test should record contended no-wait acquires"
        );
        assert_lock_history_linearizable(&history);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn in_memory_stale_leader_restart_no_wait_history_is_linearizable() {
        let mut sim = RaftSim::new(3).await.expect("start in-memory raft sim");
        let node_ids = sim.node_ids();
        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader before stale-leader restart history partition");
        let old_leader_id = leader.config().node_id.clone();
        let majority = node_ids
            .iter()
            .filter(|node_id| *node_id != &old_leader_id)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(majority.len(), 2);
        sim.partition(
            &[old_leader_id.as_str()],
            &[majority[0].as_str(), majority[1].as_str()],
        );
        wait_for_ready_leader_in(&sim, &majority, Duration::from_secs(5))
            .await
            .expect("majority partition should elect a ready leader");

        let keys = vec![format!(
            "sim-stale-restart-linear-history-key-{}",
            Uuid::new_v4()
        )];
        let history = Arc::new(Mutex::new(Vec::<LockHistoryOp>::new()));
        let order = Arc::new(AtomicUsize::new(1));
        let partition_full_log_before = full_log_metrics_for_nodes(&sim, &majority);
        let sim_phase = Arc::new(sim);
        run_no_wait_history_phase(NoWaitHistoryPhase {
            sim: Arc::clone(&sim_phase),
            node_ids: majority.clone(),
            keys: keys.clone(),
            history: Arc::clone(&history),
            order: Arc::clone(&order),
            phase: 0,
            workers: 3,
            steps: 5,
            label: "stale-restart-history",
        })
        .await;
        let partition_index =
            wait_for_ready_leader_in(&sim_phase, &majority, Duration::from_secs(5))
                .await
                .expect("majority leader after stale-restart history partition phase")
                .commit_index();
        let majority_ids = majority.iter().cloned().collect::<BTreeSet<_>>();
        wait_for_applied_on(
            &sim_phase,
            &majority_ids,
            partition_index,
            Duration::from_secs(5),
        )
        .await
        .expect("majority should apply history before stale leader restart");
        assert_full_log_metrics_unchanged(
            &sim_phase,
            &partition_full_log_before,
            "majority partition traffic",
        );

        sim = match Arc::try_unwrap(sim_phase) {
            Ok(sim) => sim,
            Err(_) => panic!("stale-leader history phase kept simulator references alive"),
        };
        sim.restart_node(&old_leader_id)
            .expect("isolated stale leader should restart with durable local state");
        let repair_full_log_before = full_log_metrics_for_nodes(&sim, &node_ids);
        sim.heal();
        wait_for_all_applied(&sim, partition_index, Duration::from_secs(5))
            .await
            .expect("healed cluster should repair stale leader through partition history");
        assert_full_log_metrics_unchanged(
            &sim,
            &repair_full_log_before,
            "stale-leader restart repair",
        );

        let sim = Arc::new(sim);
        let post_heal_full_log_before = full_log_metrics_for_nodes(&sim, &node_ids);
        run_no_wait_history_phase(NoWaitHistoryPhase {
            sim: Arc::clone(&sim),
            node_ids,
            keys,
            history: Arc::clone(&history),
            order: Arc::clone(&order),
            phase: 1,
            workers: 3,
            steps: 5,
            label: "stale-restart-history",
        })
        .await;
        let final_index = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader after stale-leader restart history heal")
            .commit_index();
        wait_for_all_applied(&sim, final_index, Duration::from_secs(5))
            .await
            .expect("stale-leader restart history should apply on all nodes");
        assert_full_log_metrics_unchanged(&sim, &post_heal_full_log_before, "post-heal traffic");

        let history = history.lock().expect("history").clone();
        assert!(
            history
                .iter()
                .any(|op| matches!(op.result, LockHistoryResult::Acquired { .. })),
            "stale-leader restart history should record successful acquires"
        );
        assert!(
            history
                .iter()
                .any(|op| matches!(op.result, LockHistoryResult::NotAcquired)),
            "stale-leader restart history should record contended no-wait acquires"
        );
        assert_lock_history_linearizable(&history);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn in_memory_rolling_restart_no_wait_history_is_linearizable() {
        let mut sim = RaftSim::new(3).await.expect("start in-memory raft sim");
        let restart_order = sim.node_ids();
        let keys = vec![format!("sim-rolling-linear-history-key-{}", Uuid::new_v4())];
        let history = Arc::new(Mutex::new(Vec::<LockHistoryOp>::new()));
        let order = Arc::new(AtomicUsize::new(1));

        for (phase, restart_node_id) in restart_order.iter().enumerate() {
            sim.wait_for_leader(Duration::from_secs(5))
                .await
                .expect("leader before rolling-restart history phase");
            let phase_full_log_before = full_log_metrics_for_nodes(&sim, &restart_order);
            let sim_phase = Arc::new(sim);
            run_no_wait_history_phase(NoWaitHistoryPhase {
                sim: Arc::clone(&sim_phase),
                node_ids: sim_phase.node_ids(),
                keys: keys.clone(),
                history: Arc::clone(&history),
                order: Arc::clone(&order),
                phase,
                workers: 3,
                steps: 5,
                label: "rolling-history",
            })
            .await;
            let phase_index = sim_phase
                .wait_for_leader(Duration::from_secs(5))
                .await
                .expect("leader after rolling-restart history phase")
                .commit_index();
            wait_for_all_applied(&sim_phase, phase_index, Duration::from_secs(5))
                .await
                .expect("history phase should apply before rolling restart");
            assert_full_log_metrics_unchanged(
                &sim_phase,
                &phase_full_log_before,
                "rolling-restart history traffic phase",
            );

            sim = match Arc::try_unwrap(sim_phase) {
                Ok(sim) => sim,
                Err(_) => panic!("rolling-restart history phase kept simulator references alive"),
            };
            sim.restart_node(restart_node_id)
                .expect("rolling-restart history node should restart");
            let restart_repair_full_log_before = full_log_metrics_for_nodes(&sim, &restart_order);
            let restart_index = sim
                .wait_for_leader(Duration::from_secs(5))
                .await
                .expect("leader after rolling-restart history node restart")
                .commit_index();
            wait_for_all_applied(&sim, restart_index, Duration::from_secs(5))
                .await
                .expect("cluster should converge after rolling restart");
            assert_full_log_metrics_unchanged(
                &sim,
                &restart_repair_full_log_before,
                "rolling-restart convergence repair",
            );
        }

        let sim = Arc::new(sim);
        let final_phase_full_log_before = full_log_metrics_for_nodes(&sim, &restart_order);
        run_no_wait_history_phase(NoWaitHistoryPhase {
            sim: Arc::clone(&sim),
            node_ids: sim.node_ids(),
            keys,
            history: Arc::clone(&history),
            order: Arc::clone(&order),
            phase: restart_order.len(),
            workers: 3,
            steps: 5,
            label: "rolling-history",
        })
        .await;
        let final_index = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader after final rolling-restart history phase")
            .commit_index();
        wait_for_all_applied(&sim, final_index, Duration::from_secs(5))
            .await
            .expect("final rolling-restart history should apply on all nodes");
        assert_full_log_metrics_unchanged(
            &sim,
            &final_phase_full_log_before,
            "final rolling-restart history traffic phase",
        );

        let history = history.lock().expect("history").clone();
        assert!(
            history
                .iter()
                .any(|op| matches!(op.result, LockHistoryResult::Acquired { .. })),
            "rolling-restart history test should record successful acquires"
        );
        assert!(
            history
                .iter()
                .any(|op| matches!(op.result, LockHistoryResult::NotAcquired)),
            "rolling-restart history test should record contended no-wait acquires"
        );
        assert_lock_history_linearizable(&history);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_composite_lock_survives_failover_restart_and_log_repair() {
        let mut sim = RaftSim::new(3).await.expect("start in-memory raft sim");
        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader before composite acquire");
        let old_leader_id = leader.config().node_id.clone();
        let node_ids = sim.node_ids();
        let follower_id = node_ids
            .iter()
            .find(|node_id| *node_id != &old_leader_id)
            .expect("follower for composite acquire")
            .clone();
        let composite_keys = vec![
            format!("sim-composite-a-{}", Uuid::new_v4()),
            format!("sim-composite-b-{}", Uuid::new_v4()),
            format!("sim-composite-c-{}", Uuid::new_v4()),
        ];

        let composite =
            acquire_composite_on_node(&sim, &follower_id, composite_keys.clone(), "composite")
                .await
                .expect("composite acquire should reach leader")
                .expect("composite acquire should grant");
        assert_eq!(
            composite.keys, composite_keys,
            "composite response should preserve sorted member keys"
        );

        let overlapping_single = acquire_key_on_node(
            &sim,
            &follower_id,
            composite.keys[0].clone(),
            "composite-overlap",
        )
        .await
        .expect("overlapping single should reach leader");
        assert!(
            overlapping_single.is_none(),
            "single-key acquire must not grant while composite holds that key"
        );

        let overlapping_composite = acquire_composite_on_node(
            &sim,
            &follower_id,
            vec![
                composite.keys[1].clone(),
                format!("sim-composite-extra-{}", Uuid::new_v4()),
            ],
            "composite-overlap",
        )
        .await
        .expect("overlapping composite should reach leader");
        assert!(
            overlapping_composite.is_none(),
            "composite acquire must use union overlap semantics"
        );

        let before_partition_index = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader after composite overlap checks")
            .commit_index();
        wait_for_all_applied(&sim, before_partition_index, Duration::from_secs(5))
            .await
            .expect("composite acquire and overlap checks should apply before partition");

        let majority = node_ids
            .iter()
            .filter(|node_id| *node_id != &old_leader_id)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(majority.len(), 2);
        sim.partition(
            &[old_leader_id.as_str()],
            &[majority[0].as_str(), majority[1].as_str()],
        );
        sim.restart_node(&old_leader_id)
            .expect("restart old leader while it is isolated");

        wait_for_ready_leader_in(&sim, &majority, Duration::from_secs(5))
            .await
            .expect("majority should elect leader while old leader is isolated");
        release_composite_on_node(&sim, &majority[0], &composite, "composite-failover")
            .await
            .expect("majority-side composite release should commit");
        let release_index = wait_for_ready_leader_in(&sim, &majority, Duration::from_secs(5))
            .await
            .expect("majority leader after composite release")
            .commit_index();
        let majority_ids = majority.iter().cloned().collect::<BTreeSet<_>>();
        wait_for_applied_on(&sim, &majority_ids, release_index, Duration::from_secs(5))
            .await
            .expect("majority should apply composite release");

        let majority_reacquired = acquire_key_on_node(
            &sim,
            &majority[0],
            composite.keys[0].clone(),
            "composite-majority-reacquire",
        )
        .await
        .expect("majority-side single acquire should reach leader")
        .expect("released composite member should grant on majority side");
        let reacquire_index = wait_for_ready_leader_in(&sim, &majority, Duration::from_secs(5))
            .await
            .expect("majority leader after member-key reacquire")
            .commit_index();
        wait_for_applied_on(&sim, &majority_ids, reacquire_index, Duration::from_secs(5))
            .await
            .expect("majority should apply member-key reacquire before heal");

        sim.heal();
        wait_for_all_applied(&sim, reacquire_index, Duration::from_secs(5))
            .await
            .expect("restarted old leader should repair through composite release and member-key reacquire");

        let repaired_old_leader_overlap = acquire_key_on_node(
            &sim,
            &old_leader_id,
            composite.keys[0].clone(),
            "composite-healed-overlap",
        )
        .await
        .expect("old leader should proxy or handle overlapping acquire after heal");
        assert!(
            repaired_old_leader_overlap.is_none(),
            "repaired old leader must not double-grant a key reacquired by the majority partition"
        );
        release_lock_on_node(
            &sim,
            &old_leader_id,
            &majority_reacquired,
            "composite-healed-majority-release",
        )
        .await
        .expect("old leader should proxy or handle release of majority reacquire after heal");
        let final_release_index = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader after releasing majority reacquire")
            .commit_index();
        wait_for_all_applied(&sim, final_release_index, Duration::from_secs(5))
            .await
            .expect("reacquired member release should apply on all nodes");

        for key in &composite.keys {
            let lock = acquire_key_on_node(&sim, &old_leader_id, key.clone(), "composite-healed")
                .await
                .expect("old leader should proxy or handle post-heal single acquire")
                .expect("released composite member key should grant after heal");
            release_lock_on_node(&sim, &old_leader_id, &lock, "composite-healed")
                .await
                .expect("post-heal single release should succeed");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_five_node_cluster_tolerates_two_failures_then_recovers() {
        // Mirrors the production 5-node Raft topology (quorum = 3): the cluster
        // keeps committing with 3 nodes connected (tolerating 2 failures), a
        // 2-node minority cannot commit, and a heal restores full service.
        let sim = RaftSim::new(5)
            .await
            .expect("start 5-node in-memory raft sim");
        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("initial leader elected");
        let leader_id = leader.config().node_id.clone();
        assert_eq!(sim.node_ids().len(), 5, "sim should run five voters");

        // Baseline commit with all five connected.
        let baseline = sim
            .acquire(format!("five-baseline-{}", Uuid::new_v4()))
            .await
            .expect("baseline acquire with five nodes");
        sim.release(&baseline)
            .await
            .expect("release baseline with five nodes");

        // Fail two non-leader nodes; the leader plus two others (3 of 5) keep
        // quorum.
        let failed_two = sim
            .node_ids()
            .into_iter()
            .filter(|node_id| node_id != &leader_id)
            .take(2)
            .collect::<Vec<_>>();
        let survivors_three = sim
            .node_ids()
            .into_iter()
            .filter(|node_id| !failed_two.contains(node_id))
            .collect::<Vec<_>>();
        assert_eq!(survivors_three.len(), 3);
        assert!(survivors_three.contains(&leader_id));

        sim.partition(
            &survivors_three
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            &failed_two.iter().map(String::as_str).collect::<Vec<_>>(),
        );

        // 3 of 5 is still a quorum, so the majority side keeps committing.
        wait_for_ready_leader_in(&sim, &survivors_three, Duration::from_secs(5))
            .await
            .expect("three-node majority keeps a ready leader");
        let held = sim
            .acquire(format!("five-quorum3-{}", Uuid::new_v4()))
            .await
            .expect("majority of three commits while two nodes are down");
        sim.release(&held)
            .await
            .expect("release on the three-node majority");

        // The two-node minority side must not be able to commit a write.
        let minority_uuid = format!("five-minority-{}", Uuid::new_v4());
        let minority_result = sim
            .run_on_node(
                &failed_two[0],
                Request::Lock {
                    uuid: minority_uuid.clone(),
                    key: Some(format!("five-minority-key-{}", Uuid::new_v4())),
                    keys: None,
                    pid: None,
                    ttl: None,
                    max: None,
                    force: false,
                    retry_count: 0,
                    keep_locks_after_death: false,
                    wait: None,
                },
                &minority_uuid,
                Duration::ZERO,
                true,
            )
            .await;
        assert!(
            matches!(
                minority_result,
                Err(RaftSimError::Raft(
                    BrokerRaftError::QuorumUnavailable { .. }
                        | BrokerRaftError::ClientProposalUncertain { .. }
                )) | Err(RaftSimError::Raft(BrokerRaftError::NotLeader { .. }))
                    | Err(RaftSimError::Timeout { .. })
            ),
            "a 2-of-5 minority must not commit a write: {minority_result:?}"
        );

        // Heal the partition: full connectivity restores a single cluster.
        sim.heal();
        sim.wait_for_leader(Duration::from_secs(5))
            .await
            .expect("a leader is re-established after heal");
        let recovered = sim
            .acquire(format!("five-healed-{}", Uuid::new_v4()))
            .await
            .expect("the healed five-node cluster commits again");
        sim.release(&recovered)
            .await
            .expect("release on the healed cluster");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_majority_partition_elects_new_leader_and_heals_old_leader() {
        let sim = RaftSim::new(3).await.expect("start in-memory raft sim");
        let old_leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("initial leader elected");
        let old_leader_id = old_leader.config().node_id.clone();
        let majority = sim
            .node_ids()
            .into_iter()
            .filter(|node_id| node_id != &old_leader_id)
            .collect::<Vec<_>>();
        assert_eq!(
            majority.len(),
            2,
            "3-node sim should leave a 2-node majority"
        );

        let baseline = sim
            .acquire(format!("sim-pre-partition-{}", Uuid::new_v4()))
            .await
            .expect("baseline acquire before partition");
        let baseline_index = sim
            .ready_leader()
            .expect("ready leader after baseline acquire")
            .commit_index();
        sim.wait_for_quorum_commit(baseline_index, Duration::from_secs(2))
            .await
            .expect("baseline should be quorum-committed before partition");
        sim.release(&baseline)
            .await
            .expect("release baseline before partition");

        sim.partition(
            &[old_leader_id.as_str()],
            &[majority[0].as_str(), majority[1].as_str()],
        );

        let isolated_commit_before = old_leader.commit_index();
        let isolated_applied_before = old_leader.progress_snapshot().last_applied;
        let isolated_request_uuid = format!("sim-isolated-acquire-{}", Uuid::new_v4());
        let isolated_result = sim
            .run_on_node(
                &old_leader_id,
                Request::Lock {
                    uuid: isolated_request_uuid.clone(),
                    key: Some(format!("sim-isolated-key-{}", Uuid::new_v4())),
                    keys: None,
                    pid: None,
                    ttl: None,
                    max: None,
                    force: false,
                    retry_count: 0,
                    keep_locks_after_death: false,
                    wait: None,
                },
                &isolated_request_uuid,
                Duration::ZERO,
                true,
            )
            .await;
        assert!(
            matches!(
                isolated_result,
                Err(RaftSimError::Raft(
                    BrokerRaftError::QuorumUnavailable { .. }
                        | BrokerRaftError::ClientProposalUncertain { .. }
                )) | Err(RaftSimError::Raft(BrokerRaftError::NotLeader { .. }))
            ),
            "isolated old leader must not grant a write without quorum: {isolated_result:?}"
        );
        let isolated_progress = old_leader.progress_snapshot();
        assert_eq!(
            isolated_progress.commit_index, isolated_commit_before,
            "isolated old leader must not commit the failed proposal"
        );
        assert_eq!(
            isolated_progress.last_applied, isolated_applied_before,
            "isolated old leader must not apply the failed proposal"
        );

        let new_leader = wait_for_ready_leader_in(&sim, &majority, Duration::from_secs(5))
            .await
            .expect("majority partition should elect a new ready leader");
        let new_leader_id = new_leader.config().node_id.clone();
        assert_ne!(new_leader_id, old_leader_id);

        let partition_key = format!("sim-post-partition-{}", Uuid::new_v4());
        let request_uuid = format!("sim-partition-acquire-{}", Uuid::new_v4());
        let acquire_response = sim
            .run_on_node(
                &majority[0],
                Request::Lock {
                    uuid: request_uuid.clone(),
                    key: Some(partition_key.clone()),
                    keys: None,
                    pid: None,
                    ttl: None,
                    max: None,
                    force: false,
                    retry_count: 0,
                    keep_locks_after_death: false,
                    wait: None,
                },
                &request_uuid,
                Duration::ZERO,
                true,
            )
            .await
            .expect("majority-side request should reach the new leader")
            .expect("majority-side acquire response");
        let partition_lock_uuid = match acquire_response {
            Response::Lock {
                acquired: true,
                lock_uuid: Some(lock_uuid),
                ..
            } => lock_uuid,
            response => panic!("partition acquire should grant; got {response:?}"),
        };
        let partition_commit_index = new_leader.commit_index();
        sim.wait_for_quorum_commit(partition_commit_index, Duration::from_secs(2))
            .await
            .expect("majority-side acquire should commit on quorum");

        sim.heal();
        wait_for_all_applied(&sim, partition_commit_index, Duration::from_secs(5))
            .await
            .expect("old leader should catch up after heal");

        let release_uuid = format!("sim-healed-release-{}", Uuid::new_v4());
        let release_response = sim
            .run_on_node(
                &old_leader_id,
                Request::Unlock {
                    uuid: release_uuid.clone(),
                    key: Some(partition_key),
                    keys: None,
                    lock_uuid: Some(partition_lock_uuid),
                    force: false,
                },
                &release_uuid,
                Duration::from_secs(2),
                false,
            )
            .await
            .expect("old leader should proxy or handle after healing")
            .expect("release response through healed old leader");
        assert!(matches!(
            release_response,
            Response::Unlock { unlocked: true, .. }
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_restarted_old_leader_repairs_uncommitted_suffix_after_heal() {
        let mut sim = RaftSim::new(3).await.expect("start in-memory raft sim");
        let old_leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("initial leader elected");
        let old_leader_id = old_leader.config().node_id.clone();
        let majority = sim
            .node_ids()
            .into_iter()
            .filter(|node_id| node_id != &old_leader_id)
            .collect::<Vec<_>>();
        assert_eq!(
            majority.len(),
            2,
            "3-node sim should leave a 2-node majority"
        );

        sim.partition(
            &[old_leader_id.as_str()],
            &[majority[0].as_str(), majority[1].as_str()],
        );

        let isolated_request_uuid = format!("sim-isolated-restart-{}", Uuid::new_v4());
        let isolated_result = sim
            .run_on_node(
                &old_leader_id,
                Request::Lock {
                    uuid: isolated_request_uuid.clone(),
                    key: Some(format!("sim-isolated-restart-key-{}", Uuid::new_v4())),
                    keys: None,
                    pid: None,
                    ttl: None,
                    max: None,
                    force: false,
                    retry_count: 0,
                    keep_locks_after_death: false,
                    wait: None,
                },
                &isolated_request_uuid,
                Duration::ZERO,
                true,
            )
            .await;
        assert!(
            matches!(
                isolated_result,
                Err(RaftSimError::Raft(
                    BrokerRaftError::QuorumUnavailable { .. }
                        | BrokerRaftError::ClientProposalUncertain { .. }
                )) | Err(RaftSimError::Raft(BrokerRaftError::NotLeader { .. }))
            ),
            "isolated old leader must not grant a write without quorum: {isolated_result:?}"
        );

        sim.restart_node(&old_leader_id)
            .expect("restart isolated old leader with its local durable suffix");

        let new_leader = wait_for_ready_leader_in(&sim, &majority, Duration::from_secs(5))
            .await
            .expect("majority partition should elect a new ready leader");
        let partition_key = format!("sim-restart-repair-{}", Uuid::new_v4());
        let request_uuid = format!("sim-restart-repair-acquire-{}", Uuid::new_v4());
        let acquire_response = sim
            .run_on_node(
                &majority[0],
                Request::Lock {
                    uuid: request_uuid.clone(),
                    key: Some(partition_key.clone()),
                    keys: None,
                    pid: None,
                    ttl: None,
                    max: None,
                    force: false,
                    retry_count: 0,
                    keep_locks_after_death: false,
                    wait: None,
                },
                &request_uuid,
                Duration::ZERO,
                true,
            )
            .await
            .expect("majority-side request should reach the new leader")
            .expect("majority-side acquire response");
        let lock_uuid = match acquire_response {
            Response::Lock {
                acquired: true,
                lock_uuid: Some(lock_uuid),
                ..
            } => lock_uuid,
            response => panic!("majority acquire should grant; got {response:?}"),
        };
        let target_index = new_leader.commit_index();
        sim.wait_for_quorum_commit(target_index, Duration::from_secs(2))
            .await
            .expect("majority-side acquire should commit on quorum");

        sim.heal();
        wait_for_all_applied(&sim, target_index, Duration::from_secs(5))
            .await
            .expect("restarted old leader should repair its suffix and catch up after heal");

        let release_uuid = format!("sim-restart-repair-release-{}", Uuid::new_v4());
        let release_response = sim
            .run_on_node(
                &old_leader_id,
                Request::Unlock {
                    uuid: release_uuid.clone(),
                    key: Some(partition_key),
                    keys: None,
                    lock_uuid: Some(lock_uuid),
                    force: false,
                },
                &release_uuid,
                Duration::from_secs(2),
                false,
            )
            .await
            .expect("restarted old leader should proxy or handle after heal")
            .expect("release response through restarted old leader");
        assert!(matches!(
            release_response,
            Response::Unlock { unlocked: true, .. }
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_lagging_follower_catches_up_over_bounded_append_entries_without_snapshot() {
        let sim = RaftSim::with_config(RaftSimConfig {
            snapshot_interval: Duration::from_secs(60 * 60),
            snapshot_max_log_entries: u64::MAX,
            snapshot_max_log_bytes: u64::MAX,
            trailing_log_entries: 10_000,
            append_entries_max_entries: 2,
            append_entries_max_bytes: usize::MAX,
            append_entries_max_inline_batches: 2,
            election_timeout_min: Duration::from_secs(10),
            election_timeout_max: Duration::from_secs(12),
            ..RaftSimConfig::default()
        })
        .await
        .expect("start append-only catch-up in-memory raft sim");
        let leader = sim
            .wait_for_leader(Duration::from_secs(15))
            .await
            .expect("initial leader elected");
        let leader_id = leader.config().node_id.clone();
        let lagging_id = sim
            .node_ids()
            .into_iter()
            .find(|node_id| node_id != &leader_id)
            .expect("lagging follower id");
        let seed = sim
            .acquire(format!("sim-append-lag-seed-{}", Uuid::new_v4()))
            .await
            .expect("seed acquire before disconnecting lagging follower");
        sim.release(&seed)
            .await
            .expect("seed release before disconnecting lagging follower");

        sim.disconnect_node(&lagging_id);
        for idx in 0..6 {
            let lock = sim
                .acquire(format!("sim-append-lag-{idx}-{}", Uuid::new_v4()))
                .await
                .expect("majority acquire while follower is disconnected");
            sim.release(&lock)
                .await
                .expect("majority release while follower is disconnected");
        }

        let target_before_reconnect = sim
            .node(&leader_id)
            .expect("leader node still present")
            .commit_index();
        let lagging_before = sim
            .node(&lagging_id)
            .expect("lagging node still present")
            .progress_snapshot();
        assert!(
            lagging_before.commit_index < target_before_reconnect,
            "disconnected follower should lag before append-only catch-up"
        );

        let batches_before = append_entries_batches_total(&sim);
        let sent_before = append_entries_sent_total(&sim);
        let snapshot_fallbacks_before = append_snapshot_fallbacks_total(&sim);
        let snapshot_installs_before = install_snapshot_successes_total(&sim);
        let catchup_node_ids = sim.node_ids();
        let catchup_full_log_before = full_log_metrics_for_nodes(&sim, &catchup_node_ids);

        sim.reconnect_node(&lagging_id);
        let trigger = sim
            .acquire(format!("sim-append-trigger-{}", Uuid::new_v4()))
            .await
            .expect("trigger acquire after reconnect");
        sim.release(&trigger)
            .await
            .expect("trigger release after reconnect");
        let target_index = sim
            .ready_leader()
            .expect("ready leader after append-only trigger")
            .commit_index();

        wait_for_all_applied(&sim, target_index, Duration::from_secs(5))
            .await
            .expect("lagging follower should catch up through bounded AppendEntries");

        let batches_delta = append_entries_batches_total(&sim).saturating_sub(batches_before);
        let sent_delta = append_entries_sent_total(&sim).saturating_sub(sent_before);
        assert!(
            batches_delta >= 2,
            "bounded catch-up should require multiple AppendEntries batches, got {batches_delta}"
        );
        assert!(
            sent_delta >= 4,
            "bounded catch-up should send retained suffix entries, got {sent_delta}"
        );
        assert_eq!(
            append_snapshot_fallbacks_total(&sim),
            snapshot_fallbacks_before,
            "ordinary retained-suffix catch-up must not fall back to InstallSnapshot"
        );
        assert_full_log_metrics_unchanged(
            &sim,
            &catchup_full_log_before,
            "ordinary retained-suffix catch-up",
        );
        assert_eq!(
            install_snapshot_successes_total(&sim),
            snapshot_installs_before,
            "ordinary retained-suffix catch-up must not install a snapshot"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_lagging_follower_recovers_via_install_snapshot_after_compaction() {
        let sim = RaftSim::with_config(RaftSimConfig {
            snapshot_interval: Duration::from_secs(60 * 60),
            snapshot_max_log_entries: 4,
            snapshot_max_log_bytes: u64::MAX,
            trailing_log_entries: 0,
            election_timeout_min: Duration::from_secs(3),
            election_timeout_max: Duration::from_secs(4),
            ..RaftSimConfig::default()
        })
        .await
        .expect("start compacting in-memory raft sim");
        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("initial leader elected");
        let leader_id = leader.config().node_id.clone();
        let lagging_id = sim
            .node_ids()
            .into_iter()
            .find(|node_id| node_id != &leader_id)
            .expect("lagging follower id");
        let seed = sim
            .acquire(format!("sim-snapshot-lag-seed-{}", Uuid::new_v4()))
            .await
            .expect("seed acquire before disconnecting snapshot lagging follower");
        sim.release(&seed)
            .await
            .expect("seed release before disconnecting snapshot lagging follower");
        let leader_compactions_before = leader.telemetry_snapshot().log_compactions_total;
        let snapshot_fallbacks_before = append_snapshot_fallbacks_total(&sim);
        let snapshot_installs_before = install_snapshot_successes_total(&sim);

        sim.disconnect_node(&lagging_id);
        for idx in 0..8 {
            let lock = sim
                .acquire(format!("sim-snapshot-lag-{idx}-{}", Uuid::new_v4()))
                .await
                .expect("majority acquire while follower is disconnected");
            sim.release(&lock)
                .await
                .expect("majority release while follower is disconnected");
        }

        wait_for_node_compaction(
            &sim,
            &leader_id,
            leader_compactions_before,
            Duration::from_secs(2),
        )
        .await
        .expect("leader should compact retained entries while follower lags");
        let target_before_reconnect = sim
            .node(&leader_id)
            .expect("leader node still present")
            .commit_index();
        let lagging_before = sim
            .node(&lagging_id)
            .expect("lagging node still present")
            .progress_snapshot();
        assert!(
            lagging_before.commit_index < target_before_reconnect,
            "disconnected follower should lag before snapshot catch-up"
        );
        let leader_snapshot_full_log_before =
            full_log_metrics_for_nodes(&sim, std::slice::from_ref(&leader_id));
        let leader_snapshot_compactions_before = sim
            .node(&leader_id)
            .expect("leader node before snapshot catch-up")
            .telemetry_snapshot()
            .log_compactions_total;

        sim.reconnect_node(&lagging_id);
        let trigger = sim
            .acquire(format!("sim-snapshot-trigger-{}", Uuid::new_v4()))
            .await
            .expect("trigger acquire after reconnect");
        sim.release(&trigger)
            .await
            .expect("trigger release after reconnect");
        let target_index = sim
            .ready_leader()
            .expect("ready leader after snapshot trigger")
            .commit_index();

        wait_for_install_snapshot_success(&sim, snapshot_installs_before, Duration::from_secs(5))
            .await
            .expect("lagging follower should receive InstallSnapshot");
        assert!(
            append_snapshot_fallbacks_total(&sim) > snapshot_fallbacks_before,
            "leader should fall back from AppendEntries to InstallSnapshot"
        );
        let leader_snapshot_full_log_after =
            full_log_metrics_for_nodes(&sim, std::slice::from_ref(&leader_id));
        let leader_full_log_before = leader_snapshot_full_log_before
            .get(&leader_id)
            .expect("leader full-log baseline");
        let leader_full_log_after = leader_snapshot_full_log_after
            .get(&leader_id)
            .expect("leader full-log after snapshot catch-up");
        assert_eq!(
            leader_full_log_after.reads_total, leader_full_log_before.reads_total,
            "leader InstallSnapshot catch-up should not perform a full retained-log scan"
        );
        assert_eq!(
            leader_full_log_after.read_entries_total, leader_full_log_before.read_entries_total,
            "leader InstallSnapshot catch-up should not read retained-log entries with a full scan"
        );
        assert_eq!(
            leader_full_log_after.read_bytes_total, leader_full_log_before.read_bytes_total,
            "leader InstallSnapshot catch-up should not read retained-log bytes with a full scan"
        );
        let leader_snapshot_compactions_after = sim
            .node(&leader_id)
            .expect("leader node after snapshot catch-up")
            .telemetry_snapshot()
            .log_compactions_total;
        let leader_rewrite_delta = leader_full_log_after
            .rewrites_total
            .saturating_sub(leader_full_log_before.rewrites_total);
        let leader_compaction_delta =
            leader_snapshot_compactions_after.saturating_sub(leader_snapshot_compactions_before);
        assert!(
            leader_rewrite_delta <= leader_compaction_delta,
            "leader InstallSnapshot catch-up should not rewrite the retained log except for real compaction; rewrite_delta={leader_rewrite_delta} compaction_delta={leader_compaction_delta}"
        );
        wait_for_all_applied(&sim, target_index, Duration::from_secs(5))
            .await
            .expect("all nodes should apply through the post-snapshot write");
        let lagging_after = sim
            .node(&lagging_id)
            .expect("lagging node after reconnect")
            .progress_snapshot();
        assert!(
            lagging_after.last_applied >= target_index,
            "lagging follower should catch up through target index {target_index}, got {}",
            lagging_after.last_applied
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_cluster_expands_membership_to_five_and_commits_with_new_quorum() {
        let mut sim = RaftSim::new(3).await.expect("start in-memory raft sim");
        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("initial leader elected");
        let mut new_peers = leader.active_peers();

        let seed = sim
            .acquire(format!("sim-membership-seed-{}", Uuid::new_v4()))
            .await
            .expect("seed acquire before membership expansion");
        sim.release(&seed)
            .await
            .expect("seed release before membership expansion");

        let peer4 = sim.add_node("node-4").expect("add node-4 as learner");
        let peer5 = sim.add_node("node-5").expect("add node-5 as learner");
        new_peers.push(peer4);
        new_peers.push(peer5);

        let final_membership_index = sim
            .change_membership(new_peers.clone())
            .await
            .expect("expand membership through joint consensus");
        wait_for_all_applied(&sim, final_membership_index, Duration::from_secs(5))
            .await
            .expect("all five nodes should apply final membership");

        for progress in sim.progress() {
            assert!(
                !progress.membership_joint,
                "node {} should leave joint consensus",
                progress.node_id
            );
            assert_eq!(
                progress.membership.active_peers(),
                new_peers,
                "node {} should converge on the five-node membership",
                progress.node_id
            );
        }

        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader after membership expansion");
        assert_eq!(leader.active_cluster_size(), 5);
        assert_eq!(leader.active_quorum_size(), 3);

        let lock = sim
            .acquire(format!("sim-membership-five-quorum-{}", Uuid::new_v4()))
            .await
            .expect("acquire under five-node membership");
        let commit_index = sim
            .ready_leader()
            .expect("ready leader after five-node acquire")
            .commit_index();
        let progress = sim
            .wait_for_quorum_commit(commit_index, Duration::from_secs(2))
            .await
            .expect("five-node membership should commit with quorum");
        let committed = progress
            .iter()
            .filter(|node| node.commit_index >= commit_index)
            .count();
        assert!(
            committed >= 3,
            "five-node membership requires a three-node quorum, got {committed}"
        );
        sim.release(&lock)
            .await
            .expect("release under five-node membership");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_cluster_expands_with_snapshot_catchup_for_new_learners() {
        let mut sim = RaftSim::with_config(RaftSimConfig {
            snapshot_interval: Duration::from_secs(60 * 60),
            snapshot_max_log_entries: 4,
            snapshot_max_log_bytes: u64::MAX,
            trailing_log_entries: 0,
            ..RaftSimConfig::default()
        })
        .await
        .expect("start compacting in-memory raft sim");
        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("initial leader elected");
        let leader_id = leader.config().node_id.clone();
        let leader_compactions_before = leader.telemetry_snapshot().log_compactions_total;

        for idx in 0..4 {
            let lock = sim
                .acquire(format!(
                    "sim-learner-snapshot-seed-{idx}-{}",
                    Uuid::new_v4()
                ))
                .await
                .expect("seed acquire before adding snapshot learners");
            sim.release(&lock)
                .await
                .expect("seed release before adding snapshot learners");
        }
        wait_for_node_compaction(
            &sim,
            &leader_id,
            leader_compactions_before,
            Duration::from_secs(2),
        )
        .await
        .expect("leader should compact before learner expansion");

        let snapshot_installs_before = install_snapshot_successes_total(&sim);
        let snapshot_fallbacks_before = append_snapshot_fallbacks_total(&sim);
        let mut new_peers = sim
            .node(&leader_id)
            .expect("leader node after compaction")
            .active_peers();
        let peer4 = sim.add_node("node-4").expect("add node-4 after compaction");
        let peer5 = sim.add_node("node-5").expect("add node-5 after compaction");
        let new_learner_ids = [peer4.id.clone(), peer5.id.clone()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        new_peers.push(peer4);
        new_peers.push(peer5);

        let final_membership_index = sim
            .change_membership(new_peers.clone())
            .await
            .expect("expand compacted cluster through snapshot learner catch-up");
        wait_for_install_snapshot_success(&sim, snapshot_installs_before, Duration::from_secs(5))
            .await
            .expect("new learners should be caught up with InstallSnapshot");
        assert!(
            append_snapshot_fallbacks_total(&sim) > snapshot_fallbacks_before,
            "learner catch-up should fall back from AppendEntries to InstallSnapshot"
        );
        wait_for_all_applied(&sim, final_membership_index, Duration::from_secs(5))
            .await
            .expect("all nodes should apply final membership after snapshot catch-up");

        let final_membership = RaftMembership::from_simple(new_peers.clone());
        for learner_id in &new_learner_ids {
            let progress = sim
                .node(learner_id)
                .expect("promoted learner present")
                .progress_snapshot();
            assert_eq!(
                progress.membership, final_membership,
                "promoted learner {learner_id} should converge on final membership"
            );
            assert!(
                progress.last_applied >= final_membership_index,
                "promoted learner {learner_id} should apply through membership index"
            );
        }

        let lock = sim
            .acquire(format!("sim-learner-snapshot-quorum-{}", Uuid::new_v4()))
            .await
            .expect("acquire after snapshot-backed learner promotion");
        let commit_index = sim
            .ready_leader()
            .expect("ready leader after snapshot-backed expansion")
            .commit_index();
        let progress = sim
            .wait_for_quorum_commit(commit_index, Duration::from_secs(2))
            .await
            .expect("five-node cluster should commit after snapshot-backed expansion");
        let committed = progress
            .iter()
            .filter(|node| node.commit_index >= commit_index)
            .count();
        assert!(
            committed >= 3,
            "snapshot-backed expansion should use a three-node quorum, got {committed}"
        );
        sim.release(&lock)
            .await
            .expect("release after snapshot-backed expansion");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_expanded_membership_survives_restarts_and_commits() {
        let mut sim = RaftSim::new(3).await.expect("start in-memory raft sim");
        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("initial leader elected");
        let mut new_peers = leader.active_peers();
        new_peers.push(sim.add_node("node-4").expect("add node-4"));
        new_peers.push(sim.add_node("node-5").expect("add node-5"));

        let final_membership_index = sim
            .change_membership(new_peers.clone())
            .await
            .expect("expand membership before restart");
        wait_for_all_applied(&sim, final_membership_index, Duration::from_secs(5))
            .await
            .expect("all nodes should apply expanded membership before restart");

        let restart_ids = sim.node_ids();
        for node_id in &restart_ids {
            sim.restart_node(node_id)
                .expect("restart node after membership expansion");
        }
        wait_for_all_applied(&sim, final_membership_index, Duration::from_secs(5))
            .await
            .expect("all restarted nodes should replay expanded membership");

        let final_membership = RaftMembership::from_simple(new_peers.clone());
        for progress in sim.progress() {
            assert_eq!(
                progress.membership, final_membership,
                "restarted node {} should recover expanded membership",
                progress.node_id
            );
            assert!(!progress.membership_joint);
            assert!(progress.last_applied >= final_membership_index);
        }

        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader after restarting expanded membership");
        assert_eq!(leader.active_cluster_size(), 5);
        assert_eq!(leader.active_quorum_size(), 3);

        let lock = sim
            .acquire(format!("sim-expanded-restart-quorum-{}", Uuid::new_v4()))
            .await
            .expect("acquire after expanded membership restart");
        let commit_index = sim
            .ready_leader()
            .expect("ready leader after restarted acquire")
            .commit_index();
        let progress = sim
            .wait_for_quorum_commit(commit_index, Duration::from_secs(2))
            .await
            .expect("restarted expanded cluster should commit with quorum");
        let committed = progress
            .iter()
            .filter(|node| node.commit_index >= commit_index)
            .count();
        assert!(
            committed >= 3,
            "restarted expanded membership requires a three-node quorum, got {committed}"
        );
        sim.release(&lock)
            .await
            .expect("release after expanded membership restart");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_cluster_shrinks_membership_and_removed_nodes_reject_clients() {
        let sim = RaftSim::new(5).await.expect("start five-node raft sim");
        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("initial five-node leader elected");
        let leader_id = leader.config().node_id.clone();
        let active_peers = leader.active_peers();
        assert_eq!(active_peers.len(), 5);

        let mut final_peers = Vec::new();
        final_peers.push(
            active_peers
                .iter()
                .find(|peer| peer.id == leader_id)
                .expect("leader peer in active membership")
                .clone(),
        );
        final_peers.extend(
            active_peers
                .iter()
                .filter(|peer| peer.id != leader_id)
                .take(2)
                .cloned(),
        );
        final_peers.sort_by(|left, right| left.id.cmp(&right.id));
        let final_ids = final_peers
            .iter()
            .map(|peer| peer.id.clone())
            .collect::<BTreeSet<_>>();
        let removed_ids = active_peers
            .iter()
            .filter(|peer| !final_ids.contains(&peer.id))
            .map(|peer| peer.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(final_ids.len(), 3);
        assert_eq!(removed_ids.len(), 2);

        let seed = sim
            .acquire(format!("sim-shrink-seed-{}", Uuid::new_v4()))
            .await
            .expect("seed acquire before membership shrink");
        sim.release(&seed)
            .await
            .expect("seed release before membership shrink");

        let final_index = sim
            .change_membership(final_peers.clone())
            .await
            .expect("shrink membership through joint consensus");
        wait_for_applied_on(&sim, &final_ids, final_index, Duration::from_secs(5))
            .await
            .expect("remaining voters should apply final simple membership");
        wait_for_removed_member_guards(&sim, &removed_ids, Duration::from_secs(5))
            .await
            .expect("removed voters should observe removed-member client guard");

        let final_membership = RaftMembership::from_simple(final_peers.clone());
        for node_id in &final_ids {
            let progress = sim
                .node(node_id)
                .expect("remaining voter present")
                .progress_snapshot();
            assert_eq!(
                progress.membership, final_membership,
                "remaining voter {node_id} should converge on final membership"
            );
            assert!(!progress.membership_joint);
        }

        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader after membership shrink");
        assert!(final_ids.contains(leader.config().node_id.as_str()));
        assert_eq!(leader.active_cluster_size(), 3);
        assert_eq!(leader.active_quorum_size(), 2);

        let lock = sim
            .acquire(format!("sim-shrink-quorum-{}", Uuid::new_v4()))
            .await
            .expect("acquire under shrunk membership");
        let commit_index = sim
            .ready_leader()
            .expect("ready leader after shrunk acquire")
            .commit_index();
        wait_for_applied_on(&sim, &final_ids, commit_index, Duration::from_secs(5))
            .await
            .expect("remaining voters should apply post-shrink write");
        sim.release(&lock)
            .await
            .expect("release under shrunk membership");

        for removed_id in &removed_ids {
            assert_removed_node_rejects_client(&sim, removed_id).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_removed_voters_stay_guarded_after_restart() {
        let mut sim = RaftSim::new(5).await.expect("start five-node raft sim");
        let leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("initial five-node leader elected");
        let leader_id = leader.config().node_id.clone();
        let active_peers = leader.active_peers();
        assert_eq!(active_peers.len(), 5);

        let mut final_peers = Vec::new();
        final_peers.push(
            active_peers
                .iter()
                .find(|peer| peer.id == leader_id)
                .expect("leader peer in active membership")
                .clone(),
        );
        final_peers.extend(
            active_peers
                .iter()
                .filter(|peer| peer.id != leader_id)
                .take(2)
                .cloned(),
        );
        final_peers.sort_by(|left, right| left.id.cmp(&right.id));
        let final_ids = final_peers
            .iter()
            .map(|peer| peer.id.clone())
            .collect::<BTreeSet<_>>();
        let removed_ids = active_peers
            .iter()
            .filter(|peer| !final_ids.contains(&peer.id))
            .map(|peer| peer.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(final_ids.len(), 3);
        assert_eq!(removed_ids.len(), 2);

        let final_index = sim
            .change_membership(final_peers.clone())
            .await
            .expect("shrink membership before removed-node restarts");
        wait_for_applied_on(&sim, &final_ids, final_index, Duration::from_secs(5))
            .await
            .expect("remaining voters should apply final simple membership");
        wait_for_removed_member_guards(&sim, &removed_ids, Duration::from_secs(5))
            .await
            .expect("removed voters should activate guards before restart");

        for removed_id in &removed_ids {
            sim.restart_node(removed_id)
                .expect("restart removed voter after final membership");
        }
        wait_for_removed_member_guards(&sim, &removed_ids, Duration::from_secs(5))
            .await
            .expect("removed voters should reload guards from disk after restart");

        for removed_id in &removed_ids {
            assert_removed_node_rejects_client(&sim, removed_id).await;
        }

        let final_membership = RaftMembership::from_simple(final_peers);
        for node_id in &final_ids {
            let progress = sim
                .node(node_id)
                .expect("remaining voter present after removed restarts")
                .progress_snapshot();
            assert_eq!(
                progress.membership, final_membership,
                "remaining voter {node_id} should keep final membership"
            );
        }

        let final_node_ids = final_ids.iter().cloned().collect::<Vec<_>>();
        let leader = wait_for_ready_leader_in(&sim, &final_node_ids, Duration::from_secs(5))
            .await
            .expect("remaining voters should still elect/keep a ready leader");
        assert_eq!(leader.active_cluster_size(), 3);
        assert_eq!(leader.active_quorum_size(), 2);

        let lock = sim
            .acquire(format!("sim-removed-restart-quorum-{}", Uuid::new_v4()))
            .await
            .expect("remaining cluster should commit after removed voters restart");
        let commit_index = sim
            .ready_leader()
            .expect("ready leader after removed-restart acquire")
            .commit_index();
        wait_for_applied_on(&sim, &final_ids, commit_index, Duration::from_secs(5))
            .await
            .expect("remaining voters should apply post-removed-restart write");
        sim.release(&lock)
            .await
            .expect("release after removed voters restart");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_memory_cluster_removes_current_leader_and_remaining_voters_continue() {
        let sim = RaftSim::new(5).await.expect("start five-node raft sim");
        let seed = sim
            .acquire(format!("sim-remove-leader-seed-{}", Uuid::new_v4()))
            .await
            .expect("seed acquire before removing leader");
        sim.release(&seed)
            .await
            .expect("seed release before removing leader");

        let old_leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader before membership change");
        let old_leader_id = old_leader.config().node_id.clone();
        let active_peers = old_leader.active_peers();
        assert_eq!(active_peers.len(), 5);

        let mut final_peers = active_peers
            .iter()
            .filter(|peer| peer.id != old_leader_id)
            .take(3)
            .cloned()
            .collect::<Vec<_>>();
        final_peers.sort_by(|left, right| left.id.cmp(&right.id));
        let final_ids = final_peers
            .iter()
            .map(|peer| peer.id.clone())
            .collect::<BTreeSet<_>>();
        let removed_ids = active_peers
            .iter()
            .filter(|peer| !final_ids.contains(&peer.id))
            .map(|peer| peer.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(final_ids.len(), 3);
        assert_eq!(removed_ids.len(), 2);
        assert!(
            removed_ids.contains(&old_leader_id),
            "the current leader must be removed by this scenario"
        );

        let final_index = old_leader
            .change_membership(final_peers.clone())
            .await
            .expect("current leader should commit its own removal");
        wait_for_applied_on(&sim, &final_ids, final_index, Duration::from_secs(5))
            .await
            .expect("remaining voters should apply final membership after leader removal");
        wait_for_removed_member_guards(&sim, &removed_ids, Duration::from_secs(5))
            .await
            .expect("removed voters should activate local client guards");

        let final_membership = RaftMembership::from_simple(final_peers);
        for node_id in &final_ids {
            let progress = sim
                .node(node_id)
                .expect("remaining voter present")
                .progress_snapshot();
            assert_eq!(
                progress.membership, final_membership,
                "remaining voter {node_id} should converge on final membership"
            );
            assert!(!progress.membership_joint);
        }

        let final_node_ids = final_ids.iter().cloned().collect::<Vec<_>>();
        let new_leader = wait_for_ready_leader_in(&sim, &final_node_ids, Duration::from_secs(5))
            .await
            .expect("remaining voters should elect a ready leader");
        assert_ne!(new_leader.config().node_id, old_leader_id);
        assert_eq!(new_leader.active_cluster_size(), 3);
        assert_eq!(new_leader.active_quorum_size(), 2);

        assert_removed_node_rejects_client(&sim, &old_leader_id).await;
        for removed_id in removed_ids
            .iter()
            .filter(|node_id| *node_id != &old_leader_id)
        {
            assert_removed_node_rejects_client(&sim, removed_id).await;
        }

        let lock = sim
            .acquire(format!("sim-remove-leader-quorum-{}", Uuid::new_v4()))
            .await
            .expect("remaining voters should commit after leader removal");
        let commit_index = sim
            .ready_leader()
            .expect("ready leader after post-removal acquire")
            .commit_index();
        wait_for_applied_on(&sim, &final_ids, commit_index, Duration::from_secs(5))
            .await
            .expect("remaining voters should apply post-removal write");
        sim.release(&lock)
            .await
            .expect("release after leader removal");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn in_memory_shrunk_membership_failover_restarts_and_history_stay_incremental() {
        let mut sim = RaftSim::new(5).await.expect("start five-node raft sim");
        let seed = sim
            .acquire(format!("sim-shrink-failover-seed-{}", Uuid::new_v4()))
            .await
            .expect("seed acquire before shrink/failover scenario");
        sim.release(&seed)
            .await
            .expect("seed release before shrink/failover scenario");

        let old_leader = sim
            .wait_for_leader(Duration::from_secs(5))
            .await
            .expect("leader before shrink/failover scenario");
        let old_leader_id = old_leader.config().node_id.clone();
        let active_peers = old_leader.active_peers();
        assert_eq!(active_peers.len(), 5);

        let mut final_peers = active_peers
            .iter()
            .filter(|peer| peer.id != old_leader_id)
            .take(3)
            .cloned()
            .collect::<Vec<_>>();
        final_peers.sort_by(|left, right| left.id.cmp(&right.id));
        let final_ids = final_peers
            .iter()
            .map(|peer| peer.id.clone())
            .collect::<BTreeSet<_>>();
        let final_node_ids = final_ids.iter().cloned().collect::<Vec<_>>();
        let removed_ids = active_peers
            .iter()
            .filter(|peer| !final_ids.contains(&peer.id))
            .map(|peer| peer.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(final_ids.len(), 3);
        assert_eq!(removed_ids.len(), 2);
        assert!(
            removed_ids.contains(&old_leader_id),
            "the initial leader must be removed by this shrink/failover scenario"
        );

        let final_index = old_leader
            .change_membership(final_peers.clone())
            .await
            .expect("initial leader should commit its own removal");
        wait_for_applied_on(&sim, &final_ids, final_index, Duration::from_secs(5))
            .await
            .expect("remaining voters should apply the shrunk membership");
        wait_for_removed_member_guards(&sim, &removed_ids, Duration::from_secs(5))
            .await
            .expect("removed voters should guard public client requests");

        for removed_id in &removed_ids {
            assert_removed_node_rejects_client(&sim, removed_id).await;
        }

        let current_leader =
            wait_for_ready_leader_in(&sim, &final_node_ids, Duration::from_secs(5))
                .await
                .expect("shrunk cluster should elect a ready leader");
        let isolated_leader_id = current_leader.config().node_id.clone();
        let survivor_ids = final_node_ids
            .iter()
            .filter(|node_id| *node_id != &isolated_leader_id)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(survivor_ids.len(), 2);
        let survivor_set = survivor_ids.iter().cloned().collect::<BTreeSet<_>>();

        sim.partition(
            &[isolated_leader_id.as_str()],
            &[survivor_ids[0].as_str(), survivor_ids[1].as_str()],
        );
        let survivor_leader = wait_for_ready_leader_in(&sim, &survivor_ids, Duration::from_secs(5))
            .await
            .expect("survivor quorum should elect a replacement leader");
        assert_ne!(
            survivor_leader.config().node_id,
            isolated_leader_id,
            "replacement leader should come from the two-node survivor quorum"
        );

        let partition_full_log_before = full_log_metrics_for_nodes(&sim, &survivor_ids);
        let partition_lock = acquire_key_on_node(
            &sim,
            &survivor_ids[0],
            format!("sim-shrink-failover-partition-{}", Uuid::new_v4()),
            "shrink-failover-partition",
        )
        .await
        .expect("survivor quorum acquire should complete")
        .expect("survivor quorum acquire should grant");
        release_lock_on_node(
            &sim,
            &survivor_ids[1],
            &partition_lock,
            "shrink-failover-partition",
        )
        .await
        .expect("survivor quorum release should complete");
        let partition_index = wait_for_ready_leader_in(&sim, &survivor_ids, Duration::from_secs(5))
            .await
            .expect("survivor quorum leader after partition traffic")
            .commit_index();
        wait_for_applied_on(&sim, &survivor_set, partition_index, Duration::from_secs(5))
            .await
            .expect("survivor quorum should apply partition traffic");
        assert_full_log_metrics_unchanged(
            &sim,
            &partition_full_log_before,
            "shrunk survivor-quorum partition traffic",
        );

        sim.restart_node(&isolated_leader_id)
            .expect("restart isolated voter with durable local state");
        let repair_full_log_before = full_log_metrics_for_nodes(&sim, &final_node_ids);
        sim.heal();
        wait_for_applied_on(&sim, &final_ids, partition_index, Duration::from_secs(5))
            .await
            .expect("healed shrunk cluster should repair the restarted voter");
        assert_full_log_metrics_unchanged(
            &sim,
            &repair_full_log_before,
            "shrunk cluster restarted-voter repair",
        );

        sim.restart_node(&survivor_ids[0])
            .expect("rolling restart one survivor after heal");
        let restored_leader =
            wait_for_ready_leader_in(&sim, &final_node_ids, Duration::from_secs(5))
                .await
                .expect("shrunk cluster should keep service after rolling restart");
        assert_eq!(restored_leader.active_cluster_size(), 3);
        assert_eq!(restored_leader.active_quorum_size(), 2);
        let restored_index = restored_leader.commit_index();
        wait_for_applied_on(&sim, &final_ids, restored_index, Duration::from_secs(5))
            .await
            .expect("shrunk cluster should converge after rolling restart");

        let history_full_log_before = full_log_metrics_for_nodes(&sim, &final_node_ids);
        let sim = Arc::new(sim);
        let history = Arc::new(Mutex::new(Vec::<LockHistoryOp>::new()));
        let order = Arc::new(AtomicUsize::new(1));
        run_no_wait_history_phase(NoWaitHistoryPhase {
            sim: Arc::clone(&sim),
            node_ids: final_node_ids.clone(),
            keys: vec![format!(
                "sim-shrink-failover-history-key-{}",
                Uuid::new_v4()
            )],
            history: Arc::clone(&history),
            order: Arc::clone(&order),
            phase: 0,
            workers: 3,
            steps: 5,
            label: "shrink-failover-history",
        })
        .await;
        let final_commit_index =
            wait_for_ready_leader_in(&sim, &final_node_ids, Duration::from_secs(5))
                .await
                .expect("leader after shrink/failover history")
                .commit_index();
        wait_for_applied_on(&sim, &final_ids, final_commit_index, Duration::from_secs(5))
            .await
            .expect("shrunk cluster should apply post-failover history");
        assert_full_log_metrics_unchanged(
            &sim,
            &history_full_log_before,
            "shrunk post-failover no-wait history traffic",
        );

        let history = history.lock().expect("history").clone();
        assert!(
            history
                .iter()
                .any(|op| matches!(op.result, LockHistoryResult::Acquired { .. })),
            "shrink/failover history should record successful acquires"
        );
        assert!(
            history
                .iter()
                .any(|op| matches!(op.result, LockHistoryResult::NotAcquired)),
            "shrink/failover history should record contended no-wait acquires"
        );
        assert_lock_history_linearizable(&history);
    }

    #[derive(Debug, Clone)]
    struct SimRng {
        state: u64,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RaftSimCompositeLock {
        keys: Vec<String>,
        lock_uuid: String,
    }

    #[derive(Debug, Clone)]
    struct LockHistoryOp {
        key: String,
        invoke_order: usize,
        response_order: usize,
        result: LockHistoryResult,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct FullLogMetricSnapshot {
        reads_total: u64,
        read_failures_total: u64,
        read_bytes_total: u64,
        read_entries_total: u64,
        rewrites_total: u64,
        rewrite_failures_total: u64,
        rewrite_entries_total: u64,
        rewrite_bytes_total: u64,
    }

    #[derive(Debug, Clone)]
    enum LockHistoryResult {
        Acquired { lock_uuid: String },
        NotAcquired,
        Released { lock_uuid: String },
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LinearModelOp {
        AcquireGranted(u16),
        AcquireRejected,
        Release(u16),
    }

    impl SimRng {
        fn new(seed: u64) -> Self {
            crate::routine_id!("ddl-routine-raft-sim-test-rng-new-1");
            Self { state: seed }
        }

        fn next_u64(&mut self) -> u64 {
            crate::routine_id!("ddl-routine-raft-sim-test-rng-next-1");
            self.state = self
                .state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.state
        }

        fn index(&mut self, len: usize) -> usize {
            crate::routine_id!("ddl-routine-raft-sim-test-rng-index-1");
            debug_assert!(len > 0);
            (self.next_u64() as usize) % len
        }
    }

    fn full_log_metrics_for_nodes(
        sim: &RaftSim,
        node_ids: &[String],
    ) -> BTreeMap<String, FullLogMetricSnapshot> {
        crate::routine_id!("ddl-routine-raft-sim-test-full-log-metrics-nodes-1");
        node_ids
            .iter()
            .map(|node_id| {
                let node = sim
                    .node(node_id)
                    .unwrap_or_else(|| panic!("node {node_id} should exist for metric snapshot"));
                (node_id.clone(), full_log_metrics_for_node(node))
            })
            .collect()
    }

    fn full_log_metrics_for_node(node: &BrokerRaft) -> FullLogMetricSnapshot {
        crate::routine_id!("ddl-routine-raft-sim-test-full-log-metrics-node-1");
        let metrics = node.raft_metrics_text();
        FullLogMetricSnapshot {
            reads_total: metric_value(&metrics, "dd_rust_network_mutex_raft_log_full_reads_total"),
            read_failures_total: metric_value(
                &metrics,
                "dd_rust_network_mutex_raft_log_full_read_failures_total",
            ),
            read_bytes_total: metric_value(
                &metrics,
                "dd_rust_network_mutex_raft_log_full_read_bytes_total",
            ),
            read_entries_total: metric_value(
                &metrics,
                "dd_rust_network_mutex_raft_log_full_read_entries_total",
            ),
            rewrites_total: metric_value(
                &metrics,
                "dd_rust_network_mutex_raft_log_full_rewrites_total",
            ),
            rewrite_failures_total: metric_value(
                &metrics,
                "dd_rust_network_mutex_raft_log_full_rewrite_failures_total",
            ),
            rewrite_entries_total: metric_value(
                &metrics,
                "dd_rust_network_mutex_raft_log_full_rewrite_entries_total",
            ),
            rewrite_bytes_total: metric_value(
                &metrics,
                "dd_rust_network_mutex_raft_log_full_rewrite_bytes_total",
            ),
        }
    }

    fn assert_full_log_metrics_unchanged(
        sim: &RaftSim,
        before: &BTreeMap<String, FullLogMetricSnapshot>,
        label: &str,
    ) {
        crate::routine_id!("ddl-routine-raft-sim-test-full-log-metrics-unchanged-1");
        let node_ids = before.keys().cloned().collect::<Vec<_>>();
        let after = full_log_metrics_for_nodes(sim, &node_ids);
        assert_eq!(
            &after, before,
            "{label} should not use full-log scans, read failures, rewrites, or rewrite failures; before={before:?} after={after:?}"
        );
    }

    fn metric_value(metrics: &str, name: &str) -> u64 {
        crate::routine_id!("ddl-routine-raft-sim-test-metric-value-1");
        for line in metrics.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            if parts.next() == Some(name) {
                let value = parts
                    .next()
                    .unwrap_or_else(|| panic!("metric {name} has no value"));
                return value.parse::<u64>().unwrap_or_else(|err| {
                    panic!("metric {name} value {value:?} is invalid: {err}")
                });
            }
        }
        panic!("metric {name} missing from raft metrics");
    }

    fn assert_lock_history_linearizable(history: &[LockHistoryOp]) {
        crate::routine_id!("ddl-routine-raft-sim-test-linearizable-history-1");
        let mut by_key = BTreeMap::<String, Vec<LockHistoryOp>>::new();
        for op in history {
            by_key.entry(op.key.clone()).or_default().push(op.clone());
        }
        for (key, ops) in by_key {
            assert_linearizable_key_history(&key, &ops);
        }
    }

    fn assert_linearizable_key_history(key: &str, ops: &[LockHistoryOp]) {
        crate::routine_id!("ddl-routine-raft-sim-test-linearizable-key-history-1");
        assert!(
            ops.len() < 128,
            "linearizability checker supports fewer than 128 operations per key; key={key} ops={}",
            ops.len()
        );
        let mut lock_ids = BTreeMap::<String, u16>::new();
        let mut granted_ids = BTreeSet::<u16>::new();
        for op in ops {
            let lock_uuid = match &op.result {
                LockHistoryResult::Acquired { lock_uuid }
                | LockHistoryResult::Released { lock_uuid } => lock_uuid,
                LockHistoryResult::NotAcquired => continue,
            };
            if !lock_ids.contains_key(lock_uuid) {
                let next_id = u16::try_from(lock_ids.len().saturating_add(1))
                    .expect("linearizable history lock id should fit in u16");
                lock_ids.insert(lock_uuid.clone(), next_id);
            }
        }

        let model_ops = ops
            .iter()
            .map(|op| match &op.result {
                LockHistoryResult::Acquired { lock_uuid } => {
                    let id = *lock_ids
                        .get(lock_uuid)
                        .expect("granted lock uuid should be indexed");
                    assert!(
                        granted_ids.insert(id),
                        "lock uuid {lock_uuid} was granted more than once for key {key}"
                    );
                    LinearModelOp::AcquireGranted(id)
                }
                LockHistoryResult::NotAcquired => LinearModelOp::AcquireRejected,
                LockHistoryResult::Released { lock_uuid } => LinearModelOp::Release(
                    *lock_ids
                        .get(lock_uuid)
                        .expect("released lock uuid should be indexed"),
                ),
            })
            .collect::<Vec<_>>();
        let predecessors = ops
            .iter()
            .map(|candidate| {
                ops.iter()
                    .enumerate()
                    .filter_map(|(idx, predecessor)| {
                        (predecessor.response_order < candidate.invoke_order)
                            .then_some(1u128 << idx)
                    })
                    .fold(0u128, |acc, bit| acc | bit)
            })
            .collect::<Vec<_>>();
        let all_done = if model_ops.is_empty() {
            0
        } else {
            (1u128 << model_ops.len()) - 1
        };
        let mut memo = BTreeSet::<(u128, u16)>::new();
        assert!(
            search_linearized_history(0, 0, all_done, &model_ops, &predecessors, &mut memo),
            "no linearization found for key {key}; ops={ops:?}"
        );
    }

    fn search_linearized_history(
        done: u128,
        holder: u16,
        all_done: u128,
        ops: &[LinearModelOp],
        predecessors: &[u128],
        memo: &mut BTreeSet<(u128, u16)>,
    ) -> bool {
        crate::routine_id!("ddl-routine-raft-sim-test-search-linear-history-1");
        if done == all_done {
            return true;
        }
        if !memo.insert((done, holder)) {
            return false;
        }
        for (idx, op) in ops.iter().enumerate() {
            let bit = 1u128 << idx;
            if done & bit != 0 || predecessors[idx] & !done != 0 {
                continue;
            }
            let Some(next_holder) = apply_linear_model_op(holder, *op) else {
                continue;
            };
            if search_linearized_history(done | bit, next_holder, all_done, ops, predecessors, memo)
            {
                return true;
            }
        }
        false
    }

    fn apply_linear_model_op(holder: u16, op: LinearModelOp) -> Option<u16> {
        crate::routine_id!("ddl-routine-raft-sim-test-apply-linear-op-1");
        match op {
            LinearModelOp::AcquireGranted(lock_id) if holder == 0 => Some(lock_id),
            LinearModelOp::AcquireGranted(_) => None,
            LinearModelOp::AcquireRejected if holder != 0 => Some(holder),
            LinearModelOp::AcquireRejected => None,
            LinearModelOp::Release(lock_id) if holder == lock_id => Some(0),
            LinearModelOp::Release(_) => None,
        }
    }

    struct NoWaitHistoryPhase {
        sim: Arc<RaftSim>,
        node_ids: Vec<String>,
        keys: Vec<String>,
        history: Arc<Mutex<Vec<LockHistoryOp>>>,
        order: Arc<AtomicUsize>,
        phase: usize,
        workers: usize,
        steps: usize,
        label: &'static str,
    }

    async fn run_no_wait_history_phase(phase_spec: NoWaitHistoryPhase) {
        crate::routine_id!("ddl-routine-raft-sim-test-no-wait-history-phase-1");
        let NoWaitHistoryPhase {
            sim,
            node_ids,
            keys,
            history,
            order,
            phase,
            workers,
            steps,
            label,
        } = phase_spec;
        assert!(!node_ids.is_empty(), "history phase needs node candidates");
        assert!(!keys.is_empty(), "history phase needs lock keys");
        assert!(workers > 0, "history phase needs workers");
        let start = Arc::new(tokio::sync::Barrier::new(workers));
        let label = label.to_string();
        let mut tasks = Vec::new();

        for worker in 0..workers {
            let sim = Arc::clone(&sim);
            let node_ids = node_ids.clone();
            let keys = keys.clone();
            let history = Arc::clone(&history);
            let order = Arc::clone(&order);
            let start = Arc::clone(&start);
            let label = label.clone();
            tasks.push(tokio::spawn(async move {
                start.wait().await;
                let mut state =
                    0xCAFE_F00D_D15C_A11Du64 ^ ((phase as u64).wrapping_shl(17)) ^ worker as u64;
                for step in 0..steps {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    let key = keys[(state as usize) % keys.len()].clone();
                    let node_id =
                        &node_ids[(state.rotate_left(11) as usize + step) % node_ids.len()];
                    let acquire_invoke = order.fetch_add(1, Ordering::SeqCst);
                    let acquire = acquire_key_on_node(&sim, node_id, key.clone(), &label).await;
                    let acquire_response = order.fetch_add(1, Ordering::SeqCst);
                    let lock = match acquire {
                        Ok(Some(lock)) => {
                            history.lock().expect("history").push(LockHistoryOp {
                                key: key.clone(),
                                invoke_order: acquire_invoke,
                                response_order: acquire_response,
                                result: LockHistoryResult::Acquired {
                                    lock_uuid: lock.lock_uuid.clone(),
                                },
                            });
                            Some(lock)
                        }
                        Ok(None) => {
                            history.lock().expect("history").push(LockHistoryOp {
                                key,
                                invoke_order: acquire_invoke,
                                response_order: acquire_response,
                                result: LockHistoryResult::NotAcquired,
                            });
                            tokio::time::sleep(Duration::from_millis(1)).await;
                            None
                        }
                        Err(err) if retryable_sim_leader_error(&err) => {
                            tokio::time::sleep(Duration::from_millis(2)).await;
                            None
                        }
                        Err(err) => panic!("{label} acquire failed during phase {phase}: {err:?}"),
                    };
                    let Some(lock) = lock else {
                        continue;
                    };
                    tokio::time::sleep(Duration::from_millis(2 + ((worker + step) % 3) as u64))
                        .await;
                    let release_invoke = order.fetch_add(1, Ordering::SeqCst);
                    release_lock_on_node(&sim, node_id, &lock, &label)
                        .await
                        .expect("history phase release should succeed");
                    let release_response = order.fetch_add(1, Ordering::SeqCst);
                    history.lock().expect("history").push(LockHistoryOp {
                        key: lock.key,
                        invoke_order: release_invoke,
                        response_order: release_response,
                        result: LockHistoryResult::Released {
                            lock_uuid: lock.lock_uuid,
                        },
                    });
                }
            }));
        }

        for task in tasks {
            task.await.expect("history phase worker should not panic");
        }
    }

    async fn run_seeded_lock_model_steps(
        sim: &RaftSim,
        node_ids: &[String],
        keys: &[String],
        held: &mut BTreeMap<String, RaftSimLock>,
        rng: &mut SimRng,
        steps: usize,
        label: &str,
    ) -> Result<u64, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-test-run-seeded-lock-model-1");
        let mut last_index = sim
            .wait_for_leader(Duration::from_secs(5))
            .await?
            .commit_index();
        for step in 0..steps {
            let node_id = &node_ids[rng.index(node_ids.len())];
            let key = keys[rng.index(keys.len())].clone();
            let should_release = !held.is_empty() && (step % 3 == 2 || held.contains_key(&key));

            if should_release {
                let release_key = if held.contains_key(&key) {
                    key
                } else {
                    let held_keys = held.keys().cloned().collect::<Vec<_>>();
                    held_keys[rng.index(held_keys.len())].clone()
                };
                let lock = held
                    .remove(&release_key)
                    .expect("selected held lock should be present");
                release_lock_on_node(sim, node_id, &lock, label).await?;
            } else if let Some(existing) = held.get(&key) {
                let contended =
                    acquire_key_on_node(sim, node_id, existing.key.clone(), label).await?;
                assert!(
                    contended.is_none(),
                    "{label} step {step} granted a second holder for {}",
                    existing.key
                );
            } else if let Some(lock) = acquire_key_on_node(sim, node_id, key.clone(), label).await?
            {
                assert!(
                    held.insert(key.clone(), lock).is_none(),
                    "{label} step {step} replaced an existing held lock for {key}"
                );
            }

            let leader = sim.wait_for_leader(Duration::from_secs(5)).await?;
            last_index = leader.commit_index();
            sim.wait_for_quorum_commit(last_index, Duration::from_secs(5))
                .await?;
        }
        Ok(last_index)
    }

    async fn acquire_composite_on_node(
        sim: &RaftSim,
        node_id: &str,
        keys: Vec<String>,
        label: &str,
    ) -> Result<Option<RaftSimCompositeLock>, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-test-acquire-composite-node-1");
        let request_uuid = format!("sim-{label}-composite-acquire-{}", Uuid::new_v4());
        let response = sim
            .run_on_node(
                node_id,
                Request::Lock {
                    uuid: request_uuid.clone(),
                    key: None,
                    keys: Some(keys),
                    pid: None,
                    ttl: None,
                    max: None,
                    force: false,
                    retry_count: 0,
                    keep_locks_after_death: false,
                    wait: None,
                },
                &request_uuid,
                Duration::ZERO,
                true,
            )
            .await?
            .ok_or_else(|| RaftSimError::NoResponse {
                request_id: request_uuid.clone(),
            })?;
        match response {
            Response::CompositeLock {
                acquired: true,
                keys,
                lock_uuid: Some(lock_uuid),
                fencing_tokens,
                ..
            } => {
                if let Some(tokens) = &fencing_tokens {
                    assert_eq!(
                        tokens.len(),
                        keys.len(),
                        "composite grant should include one fencing token per member key"
                    );
                }
                Ok(Some(RaftSimCompositeLock { keys, lock_uuid }))
            }
            Response::CompositeLock {
                acquired: false, ..
            } => Ok(None),
            response => Err(RaftSimError::UnexpectedResponse {
                request_id: request_uuid,
                response: Box::new(response),
            }),
        }
    }

    async fn acquire_key_on_node(
        sim: &RaftSim,
        node_id: &str,
        key: String,
        label: &str,
    ) -> Result<Option<RaftSimLock>, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-test-acquire-key-node-1");
        let request_uuid = format!("sim-{label}-acquire-{}", Uuid::new_v4());
        let response = sim
            .run_on_node(
                node_id,
                Request::Lock {
                    uuid: request_uuid.clone(),
                    key: Some(key.clone()),
                    keys: None,
                    pid: None,
                    ttl: None,
                    max: None,
                    force: false,
                    retry_count: 0,
                    keep_locks_after_death: false,
                    wait: None,
                },
                &request_uuid,
                Duration::ZERO,
                true,
            )
            .await?
            .ok_or_else(|| RaftSimError::NoResponse {
                request_id: request_uuid.clone(),
            })?;
        match response {
            Response::Lock {
                acquired: true,
                lock_uuid: Some(lock_uuid),
                fencing_token,
                ..
            } => Ok(Some(RaftSimLock {
                key,
                lock_uuid,
                fencing_token,
            })),
            Response::Lock {
                acquired: false, ..
            } => Ok(None),
            response => Err(RaftSimError::UnexpectedResponse {
                request_id: request_uuid,
                response: Box::new(response),
            }),
        }
    }

    async fn release_composite_on_node(
        sim: &RaftSim,
        node_id: &str,
        lock: &RaftSimCompositeLock,
        label: &str,
    ) -> Result<(), RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-test-release-composite-node-1");
        let request_uuid = format!("sim-{label}-composite-release-{}", Uuid::new_v4());
        let request = Request::Unlock {
            uuid: request_uuid.clone(),
            key: None,
            keys: Some(lock.keys.clone()),
            lock_uuid: Some(lock.lock_uuid.clone()),
            force: false,
        };
        let response = retry_sim_response(
            "raft node composite release",
            DEFAULT_SIM_REQUEST_TIMEOUT,
            |_| {
                let request = request.clone();
                let request_uuid = request_uuid.clone();
                async move {
                    sim.run_on_node(
                        node_id,
                        request,
                        &request_uuid,
                        Duration::from_secs(2),
                        false,
                    )
                    .await
                }
            },
            retryable_sim_leader_error,
        )
        .await?
        .ok_or_else(|| RaftSimError::NoResponse {
            request_id: request_uuid.clone(),
        })?;
        match response {
            Response::Unlock { unlocked: true, .. } => Ok(()),
            response => Err(RaftSimError::UnexpectedResponse {
                request_id: request_uuid,
                response: Box::new(response),
            }),
        }
    }

    async fn release_lock_on_node(
        sim: &RaftSim,
        node_id: &str,
        lock: &RaftSimLock,
        label: &str,
    ) -> Result<(), RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-test-release-lock-node-1");
        let request_uuid = format!("sim-{label}-release-{}", Uuid::new_v4());
        let request = Request::Unlock {
            uuid: request_uuid.clone(),
            key: Some(lock.key.clone()),
            keys: None,
            lock_uuid: Some(lock.lock_uuid.clone()),
            force: false,
        };
        let response = retry_sim_response(
            "raft node release",
            DEFAULT_SIM_REQUEST_TIMEOUT,
            |_| {
                let request = request.clone();
                let request_uuid = request_uuid.clone();
                async move {
                    sim.run_on_node(
                        node_id,
                        request,
                        &request_uuid,
                        Duration::from_secs(2),
                        false,
                    )
                    .await
                }
            },
            retryable_sim_leader_error,
        )
        .await?
        .ok_or_else(|| RaftSimError::NoResponse {
            request_id: request_uuid.clone(),
        })?;
        match response {
            Response::Unlock { unlocked: true, .. } => Ok(()),
            response => Err(RaftSimError::UnexpectedResponse {
                request_id: request_uuid,
                response: Box::new(response),
            }),
        }
    }

    async fn release_all_held_on_seeded_nodes(
        sim: &RaftSim,
        node_ids: &[String],
        held: &mut BTreeMap<String, RaftSimLock>,
        rng: &mut SimRng,
        label: &str,
    ) -> Result<(), RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-test-release-all-held-1");
        let keys = held.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            let node_id = &node_ids[rng.index(node_ids.len())];
            let lock = held
                .remove(&key)
                .expect("held lock key collected from map should still exist");
            release_lock_on_node(sim, node_id, &lock, label).await?;
            let leader = sim.wait_for_leader(Duration::from_secs(5)).await?;
            sim.wait_for_quorum_commit(leader.commit_index(), Duration::from_secs(5))
                .await?;
        }
        Ok(())
    }

    async fn wait_for_ready_leader_in(
        sim: &RaftSim,
        node_ids: &[String],
        timeout: Duration,
    ) -> Result<BrokerRaft, RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-test-wait-leader-in-1");
        let allowed = node_ids.iter().cloned().collect::<BTreeSet<_>>();
        let deadline = deadline_after(timeout.max(SIM_WAIT_FLOOR));
        loop {
            if let Some(leader) = allowed
                .iter()
                .filter_map(|node_id| sim.node(node_id))
                .find(|node| node.is_leader_ready())
                .cloned()
            {
                return Ok(leader);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(RaftSimError::Timeout {
                    operation: "ready raft leader in node subset",
                    timeout_ms: duration_ms_u64(timeout),
                });
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn wait_for_all_applied(
        sim: &RaftSim,
        index: u64,
        timeout: Duration,
    ) -> Result<(), RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-test-wait-all-applied-1");
        let deadline = deadline_after(timeout.max(SIM_WAIT_FLOOR));
        loop {
            let progress = sim.progress();
            if progress
                .iter()
                .all(|node| node.commit_index >= index && node.last_applied >= index)
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(RaftSimError::Timeout {
                    operation: "all raft nodes applied index",
                    timeout_ms: duration_ms_u64(timeout),
                });
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn wait_for_applied_on(
        sim: &RaftSim,
        node_ids: &BTreeSet<String>,
        index: u64,
        timeout: Duration,
    ) -> Result<(), RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-test-wait-applied-on-1");
        let deadline = deadline_after(timeout.max(SIM_WAIT_FLOOR));
        loop {
            let all_applied = node_ids.iter().all(|node_id| {
                sim.node(node_id)
                    .map(|node| {
                        let progress = node.progress_snapshot();
                        progress.commit_index >= index && progress.last_applied >= index
                    })
                    .unwrap_or(false)
            });
            if all_applied {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(RaftSimError::Timeout {
                    operation: "selected raft nodes applied index",
                    timeout_ms: duration_ms_u64(timeout),
                });
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn wait_for_removed_member_guards(
        sim: &RaftSim,
        node_ids: &[String],
        timeout: Duration,
    ) -> Result<(), RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-test-wait-removed-member-guards-1");
        let deadline = deadline_after(timeout.max(SIM_WAIT_FLOOR));
        loop {
            let all_guarded = node_ids.iter().all(|node_id| {
                sim.node(node_id)
                    .map(|node| removed_member_guard_active(&node.progress_snapshot()))
                    .unwrap_or(false)
            });
            if all_guarded {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(RaftSimError::Timeout {
                    operation: "removed raft member client guard",
                    timeout_ms: duration_ms_u64(timeout),
                });
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn removed_member_guard_active(progress: &RaftProgressSnapshot) -> bool {
        crate::routine_id!("ddl-routine-raft-sim-test-removed-member-guard-1");
        match &progress.membership {
            RaftMembership::Simple { peers } => {
                !peers.iter().any(|peer| peer.id == progress.node_id)
            }
            RaftMembership::Joint {
                old_peers,
                new_peers,
            } => {
                old_peers.iter().any(|peer| peer.id == progress.node_id)
                    && !new_peers.iter().any(|peer| peer.id == progress.node_id)
            }
        }
    }

    async fn assert_removed_node_rejects_client(sim: &RaftSim, removed_id: &str) {
        crate::routine_id!("ddl-routine-raft-sim-test-removed-node-rejects-client-1");
        let telemetry_before = sim
            .node(removed_id)
            .expect("removed node present")
            .telemetry_snapshot();
        let request_uuid = format!("sim-removed-client-{}", Uuid::new_v4());
        let err = sim
            .run_on_node(
                removed_id,
                Request::Lock {
                    uuid: request_uuid.clone(),
                    key: Some(format!("sim-removed-key-{}", Uuid::new_v4())),
                    keys: None,
                    pid: None,
                    ttl: None,
                    max: None,
                    force: false,
                    retry_count: 0,
                    keep_locks_after_death: false,
                    wait: None,
                },
                &request_uuid,
                Duration::ZERO,
                true,
            )
            .await
            .expect_err("removed node should reject public client request");
        assert!(
            matches!(err, RaftSimError::Raft(BrokerRaftError::NotLeader { .. })),
            "removed node {removed_id} should return NotLeader, got {err:?}"
        );
        let telemetry_after = sim
            .node(removed_id)
            .expect("removed node present after reject")
            .telemetry_snapshot();
        assert_eq!(
            telemetry_after.client_removed_member_rejections_total,
            telemetry_before
                .client_removed_member_rejections_total
                .saturating_add(1),
            "removed node {removed_id} should count the local client rejection"
        );
        assert_eq!(
            telemetry_after.proxy_requests_forwarded_total,
            telemetry_before.proxy_requests_forwarded_total,
            "removed node {removed_id} must reject locally instead of proxying"
        );
    }

    fn append_entries_batches_total(sim: &RaftSim) -> u64 {
        crate::routine_id!("ddl-routine-raft-sim-test-append-batches-total-1");
        sim.nodes()
            .values()
            .map(|node| node.telemetry_snapshot().append_entries_batches_total)
            .sum()
    }

    fn append_entries_sent_total(sim: &RaftSim) -> u64 {
        crate::routine_id!("ddl-routine-raft-sim-test-append-sent-total-1");
        sim.nodes()
            .values()
            .map(|node| node.telemetry_snapshot().append_entries_sent_total)
            .sum()
    }

    fn append_snapshot_fallbacks_total(sim: &RaftSim) -> u64 {
        crate::routine_id!("ddl-routine-raft-sim-test-append-snapshot-fallbacks-1");
        sim.nodes()
            .values()
            .map(|node| node.telemetry_snapshot().append_snapshot_fallbacks_total)
            .sum()
    }

    fn install_snapshot_successes_total(sim: &RaftSim) -> u64 {
        crate::routine_id!("ddl-routine-raft-sim-test-install-snapshot-successes-1");
        sim.nodes()
            .values()
            .map(|node| node.telemetry_snapshot().install_snapshot_successes_total)
            .sum()
    }

    async fn wait_for_node_compaction(
        sim: &RaftSim,
        node_id: &str,
        previous_compactions: u64,
        timeout: Duration,
    ) -> Result<(), RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-test-wait-node-compaction-1");
        let deadline = deadline_after(timeout.max(SIM_WAIT_FLOOR));
        loop {
            let node = sim
                .node(node_id)
                .ok_or_else(|| RaftSimError::NodeNotFound(node_id.to_string()))?;
            if node.telemetry_snapshot().log_compactions_total > previous_compactions {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(RaftSimError::Timeout {
                    operation: "raft node log compaction",
                    timeout_ms: duration_ms_u64(timeout),
                });
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn wait_for_install_snapshot_success(
        sim: &RaftSim,
        previous_successes: u64,
        timeout: Duration,
    ) -> Result<(), RaftSimError> {
        crate::routine_id!("ddl-routine-raft-sim-test-wait-install-snapshot-1");
        let deadline = deadline_after(timeout.max(SIM_WAIT_FLOOR));
        loop {
            if install_snapshot_successes_total(sim) > previous_successes {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(RaftSimError::Timeout {
                    operation: "raft InstallSnapshot success",
                    timeout_ms: duration_ms_u64(timeout),
                });
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
