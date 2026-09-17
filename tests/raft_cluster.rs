//! End-to-end smoke tests for the HTTP BrokerRaft backend.
//!
//! These run three loopback Raft nodes, wait for election, then exercise the
//! load-balancer shape: HTTP requests can land on followers and get proxied to
//! the elected leader, while commits still require a quorum.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dd_rust_network_mutex::{server, BrokerConfig, BrokerRaftConfig, RaftPeerConfig, ServerConfig};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

static RAFT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct RaftCluster {
    raft_ports: Vec<u16>,
    http_ports: Vec<u16>,
    peers: Vec<RaftPeerConfig>,
    initial_peers: Vec<RaftPeerConfig>,
    data_dir: PathBuf,
    tuning: RaftClusterTuning,
    handles: Vec<Option<JoinHandle<Result<(), String>>>>,
}

impl RaftCluster {
    async fn abort_node(&mut self, index: usize) {
        if let Some(handle) = self.handles[index].take() {
            handle.abort();
            let _ = handle.await;
        }
        wait_for_http_unavailable(self.http_ports[index], "/raft/status").await;
    }

    async fn restart_node(&mut self, index: usize) {
        self.abort_node(index).await;
        self.spawn_node_until_ready(index, self.tuning.clone())
            .await;
    }

    async fn restart_node_with_tuning(&mut self, index: usize, tuning: RaftClusterTuning) {
        self.abort_node(index).await;
        self.spawn_node_until_ready(index, tuning).await;
    }

    async fn spawn_node_until_ready(&mut self, index: usize, tuning: RaftClusterTuning) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            self.handles[index] = Some(spawn_raft_node(
                index,
                &self.raft_ports,
                &self.http_ports,
                &self.data_dir,
                &self.initial_peers,
                &tuning,
            ));
            match wait_for_http_port_or_node_exit(
                &mut self.handles[index],
                self.http_ports[index],
                index,
                deadline,
            )
            .await
            {
                Ok(()) => return,
                Err(err)
                    if err.contains("already locked by this process")
                        && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
                Err(err) => panic!("{err}"),
            }
        }
    }
}

impl Drop for RaftCluster {
    fn drop(&mut self) {
        for handle in self.handles.iter().flatten() {
            handle.abort();
        }
        let _ = fs::remove_dir_all(&self.data_dir);
    }
}

#[derive(Debug, Clone)]
struct RaftClusterTuning {
    heartbeat_interval: Duration,
    election_timeout_min: Duration,
    election_timeout_max: Duration,
    snapshot_interval: Duration,
    snapshot_max_log_entries: u64,
    snapshot_max_log_bytes: u64,
    trailing_log_entries: u64,
    append_entries_max_entries: usize,
    append_entries_max_bytes: usize,
    append_entries_max_inline_batches: usize,
    target_quorum_extra_fanout: usize,
    install_snapshot_chunk_bytes: usize,
    client_batch_max_entries: usize,
    client_pipeline_max_batches: usize,
    client_batch_max_pending: usize,
    client_batch_max_delay: Duration,
    client_response_cache_max_entries: usize,
    proxy_retry_budget: Duration,
    peer_token: Option<String>,
}

impl Default for RaftClusterTuning {
    fn default() -> Self {
        let defaults = BrokerRaftConfig::default();
        Self {
            heartbeat_interval: Duration::from_millis(50),
            election_timeout_min: Duration::from_millis(600),
            election_timeout_max: Duration::from_millis(1_200),
            snapshot_interval: defaults.snapshot_interval,
            snapshot_max_log_entries: defaults.snapshot_max_log_entries,
            snapshot_max_log_bytes: defaults.snapshot_max_log_bytes,
            trailing_log_entries: defaults.trailing_log_entries,
            append_entries_max_entries: defaults.append_entries_max_entries,
            append_entries_max_bytes: defaults.append_entries_max_bytes,
            append_entries_max_inline_batches: defaults.append_entries_max_inline_batches,
            target_quorum_extra_fanout: defaults.target_quorum_extra_fanout,
            install_snapshot_chunk_bytes: defaults.install_snapshot_chunk_bytes,
            client_batch_max_entries: defaults.client_batch_max_entries,
            client_pipeline_max_batches: defaults.client_pipeline_max_batches,
            client_batch_max_pending: defaults.client_batch_max_pending,
            client_batch_max_delay: defaults.client_batch_max_delay,
            client_response_cache_max_entries: defaults.client_response_cache_max_entries,
            proxy_retry_budget: defaults.proxy_retry_budget,
            peer_token: None,
        }
    }
}

struct TestLb {
    port: u16,
    handle: JoinHandle<()>,
}

impl Drop for TestLb {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

struct RegularHttpServer {
    port: u16,
    handle: JoinHandle<()>,
}

impl Drop for RegularHttpServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn pick_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

async fn start_regular_http_server() -> RegularHttpServer {
    let port = pick_port().await;
    let config = ServerConfig {
        tcp_bind: None,
        uds_path: None,
        http_bind: Some(format!("127.0.0.1:{port}").parse().unwrap()),
        auth_token: None,
        broker: BrokerConfig::default(),
        tcp_nodelay: true,
        tcp_quickack: true,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    };
    let handle = tokio::spawn(async move {
        let _ = server::run(config).await;
    });
    wait_for_http_port(port, "/healthz").await;
    RegularHttpServer { port, handle }
}

async fn start_cluster() -> RaftCluster {
    start_cluster_with_nodes(3, 3).await
}

async fn start_cluster_with_nodes(total_nodes: usize, initial_voters: usize) -> RaftCluster {
    start_cluster_with_tuning(total_nodes, initial_voters, RaftClusterTuning::default()).await
}

async fn start_cluster_with_tuning(
    total_nodes: usize,
    initial_voters: usize,
    tuning: RaftClusterTuning,
) -> RaftCluster {
    assert!(initial_voters <= total_nodes);
    let mut raft_ports = Vec::new();
    let mut http_ports = Vec::new();
    for _ in 0..total_nodes {
        raft_ports.push(pick_port().await);
        http_ports.push(pick_port().await);
    }

    let peers: Vec<RaftPeerConfig> = raft_ports
        .iter()
        .enumerate()
        .map(|(idx, port)| RaftPeerConfig {
            id: format!("node-{}", idx + 1),
            addr: format!("127.0.0.1:{port}"),
        })
        .collect();
    let initial_peers = peers
        .iter()
        .take(initial_voters)
        .cloned()
        .collect::<Vec<_>>();

    let data_dir = std::env::temp_dir().join(format!("lmx-raft-cluster-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&data_dir).unwrap();

    let mut handles = Vec::new();
    for idx in 0..total_nodes {
        handles.push(Some(spawn_raft_node(
            idx,
            &raft_ports,
            &http_ports,
            &data_dir,
            &initial_peers,
            &tuning,
        )));
    }

    let cluster = RaftCluster {
        raft_ports,
        http_ports,
        peers,
        initial_peers,
        data_dir,
        tuning,
        handles,
    };
    wait_for_http(&cluster).await;
    cluster
}

fn spawn_raft_node(
    index: usize,
    raft_ports: &[u16],
    http_ports: &[u16],
    data_dir: &std::path::Path,
    initial_peers: &[RaftPeerConfig],
    tuning: &RaftClusterTuning,
) -> JoinHandle<Result<(), String>> {
    let node_id = format!("node-{}", index + 1);
    let raft = BrokerRaftConfig {
        enabled: true,
        node_id: node_id.clone(),
        bind_addr: Some(format!("127.0.0.1:{}", raft_ports[index]).parse().unwrap()),
        advertise_addr: Some(format!("127.0.0.1:{}", raft_ports[index])),
        data_dir: data_dir.join(&node_id),
        heartbeat_interval: tuning.heartbeat_interval,
        election_timeout_min: tuning.election_timeout_min,
        election_timeout_max: tuning.election_timeout_max,
        snapshot_interval: tuning.snapshot_interval,
        snapshot_max_log_entries: tuning.snapshot_max_log_entries,
        snapshot_max_log_bytes: tuning.snapshot_max_log_bytes,
        trailing_log_entries: tuning.trailing_log_entries,
        append_entries_max_entries: tuning.append_entries_max_entries,
        append_entries_max_bytes: tuning.append_entries_max_bytes,
        append_entries_max_inline_batches: tuning.append_entries_max_inline_batches,
        target_quorum_extra_fanout: tuning.target_quorum_extra_fanout,
        install_snapshot_chunk_bytes: tuning.install_snapshot_chunk_bytes,
        client_batch_max_entries: tuning.client_batch_max_entries,
        client_pipeline_max_batches: tuning.client_pipeline_max_batches,
        client_batch_max_pending: tuning.client_batch_max_pending,
        client_batch_max_delay: tuning.client_batch_max_delay,
        client_response_cache_max_entries: tuning.client_response_cache_max_entries,
        proxy_retry_budget: tuning.proxy_retry_budget,
        peer_token: tuning.peer_token.clone(),
        peers: initial_peers.to_vec(),
        broker: BrokerConfig::default(),
        ..BrokerRaftConfig::default()
    };

    let config = ServerConfig {
        tcp_bind: None,
        uds_path: None,
        http_bind: Some(format!("127.0.0.1:{}", http_ports[index]).parse().unwrap()),
        auth_token: None,
        broker: BrokerConfig::default(),
        tcp_nodelay: true,
        tcp_quickack: true,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    };

    tokio::spawn(async move {
        match server::run_raft(config, raft).await {
            Ok(()) => Err(format!("raft node {node_id} exited unexpectedly")),
            Err(err) => Err(format!("raft node {node_id} failed: {err}")),
        }
    })
}

async fn wait_for_http(cluster: &RaftCluster) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut up = 0usize;
        for port in &cluster.http_ports {
            if http_get_json(*port, "/raft/status").await.is_some() {
                up += 1;
            }
        }
        if up == cluster.http_ports.len() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for raft HTTP listeners"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_for_http_port(port: u16, path: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if http_get_json(port, path).await.is_some() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for HTTP listener on port {port}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_for_http_port_or_node_exit(
    handle: &mut Option<JoinHandle<Result<(), String>>>,
    port: u16,
    index: usize,
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    loop {
        if http_get_json(port, "/raft/status").await.is_some() {
            return Ok(());
        }
        if handle.as_ref().is_some_and(|handle| handle.is_finished()) {
            let result = handle
                .take()
                .expect("finished node handle should exist")
                .await;
            let exit = match result {
                Ok(Ok(())) => "raft node task returned successfully".to_string(),
                Ok(Err(err)) => err,
                Err(err) => format!("raft node task join failed: {err}"),
            };
            return Err(format!(
                "raft node {index} exited before HTTP listener on port {port} became ready: {exit}"
            ));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for HTTP listener on port {port}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_for_http_unavailable(port: u16, path: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if http_get_json(port, path).await.is_none() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for HTTP listener on port {port} to stop serving {path}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_for_leader(cluster: &RaftCluster) -> usize {
    let nodes: Vec<usize> = (0..cluster.http_ports.len()).collect();
    wait_for_leader_among(cluster, &nodes).await
}

async fn wait_for_leader_among(cluster: &RaftCluster, nodes: &[usize]) -> usize {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let mut statuses = Vec::new();
        for index in nodes {
            let port = cluster.http_ports[*index];
            if let Some(status) = http_get_json(port, "/raft/status").await {
                statuses.push((*index, status));
            }
        }
        if statuses.len() == nodes.len() {
            let leaders: Vec<usize> = statuses
                .iter()
                .filter_map(|(idx, status)| {
                    status["isLeader"].as_bool().filter(|v| *v).map(|_| *idx)
                })
                .collect();
            if leaders.len() == 1 {
                let leader_id = statuses
                    .iter()
                    .find(|(idx, _)| *idx == leaders[0])
                    .and_then(|(_, status)| status["nodeId"].as_str())
                    .unwrap();
                let all_know_leader = statuses
                    .iter()
                    .all(|(_, status)| status["leaderId"].as_str() == Some(leader_id));
                if all_know_leader {
                    return leaders[0];
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for single raft leader; latest statuses={statuses:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_simple_membership(
    cluster: &RaftCluster,
    nodes: &[usize],
    expected_ids: &BTreeSet<String>,
    expected_cluster_size: u64,
    expected_quorum_size: u64,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut latest = Vec::new();
    loop {
        latest.clear();
        let mut converged = true;
        for index in nodes {
            let port = cluster.http_ports[*index];
            let Some(status) = http_get_json(port, "/raft/status").await else {
                converged = false;
                latest.push(format!("{port}: status unavailable"));
                continue;
            };
            let peer_ids = simple_membership_peer_ids(&status["membership"]);
            let ok = status["membershipJoint"] == false
                && status["clusterSize"].as_u64() == Some(expected_cluster_size)
                && status["quorumSize"].as_u64() == Some(expected_quorum_size)
                && peer_ids.as_ref() == Some(expected_ids);
            latest.push(format!("{port}: status={status:?} peers={peer_ids:?}"));
            converged &= ok;
        }
        if converged {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for raft membership convergence; latest={latest:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn simple_membership_peer_ids(membership: &Value) -> Option<BTreeSet<String>> {
    if membership["state"] != "simple" {
        return None;
    }
    Some(
        membership
            .get("peers")?
            .as_array()?
            .iter()
            .filter_map(|peer| peer["id"].as_str().map(str::to_string))
            .collect(),
    )
}

fn membership_target_peer_ids(membership: &Value) -> Option<BTreeSet<String>> {
    match membership["state"].as_str()? {
        "simple" => Some(
            membership
                .get("peers")?
                .as_array()?
                .iter()
                .filter_map(|peer| peer["id"].as_str().map(str::to_string))
                .collect(),
        ),
        "joint" => Some(
            membership
                .get("newPeers")?
                .as_array()?
                .iter()
                .filter_map(|peer| peer["id"].as_str().map(str::to_string))
                .collect(),
        ),
        _ => None,
    }
}

async fn wait_for_zero_holders_and_waiters(cluster: &RaftCluster) {
    let nodes: Vec<usize> = (0..cluster.http_ports.len()).collect();
    wait_for_zero_holders_and_waiters_among(cluster, &nodes).await;
}

async fn wait_for_zero_holders_and_waiters_among(cluster: &RaftCluster, nodes: &[usize]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut last = Vec::new();
    loop {
        let mut all_zero = true;
        last.clear();
        for index in nodes {
            let port = cluster.http_ports[*index];
            let Some(metrics) = http_get_text(port, "/metrics").await else {
                all_zero = false;
                last.push(format!("{port}: metrics unavailable"));
                continue;
            };
            let holders = metric_value(&metrics, "dd_rust_network_mutex_holders");
            let waiters = metric_value(&metrics, "dd_rust_network_mutex_waiters");
            let status = http_get_json(port, "/raft/status").await;
            last.push(format!(
                "{port}: holders={holders:?} waiters={waiters:?} status={status:?}"
            ));
            all_zero &= holders == Some(0) && waiters == Some(0);
        }
        if all_zero {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for raft nodes to clear holders/waiters; latest={last:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_zero_holders_and_waiters_on_port(port: u16) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut latest = String::new();
    loop {
        if let Some(metrics) = http_get_text(port, "/metrics").await {
            let holders = metric_value(&metrics, "dd_rust_network_mutex_holders");
            let waiters = metric_value(&metrics, "dd_rust_network_mutex_waiters");
            latest = format!("holders={holders:?} waiters={waiters:?}");
            if holders == Some(0) && waiters == Some(0) {
                return;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for backend on port {port} to clear holders/waiters; latest={latest}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_status_index_at_least(port: u16, field: &str, min: u64) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut latest = None;
    loop {
        if let Some(status) = http_get_json(port, "/raft/status").await {
            let value = status[field].as_u64();
            latest = Some(status.clone());
            if value.is_some_and(|value| value >= min) {
                return status;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for /raft/status field {field} >= {min} on port {port}; latest={latest:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_status_indexes_at_least_among(
    cluster: &RaftCluster,
    nodes: &[usize],
    min_commit_index: u64,
    min_last_applied: u64,
) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut latest = Vec::new();
    loop {
        latest.clear();
        let mut converged = true;
        let mut statuses = Vec::new();
        for index in nodes {
            let port = cluster.http_ports[*index];
            let Some(status) = http_get_json(port, "/raft/status").await else {
                converged = false;
                latest.push(format!("{port}: status unavailable"));
                continue;
            };
            let commit_index = status["commitIndex"].as_u64();
            let last_applied = status["lastApplied"].as_u64();
            latest.push(format!(
                "{port}: commitIndex={commit_index:?} lastApplied={last_applied:?} status={status:?}"
            ));
            converged &= commit_index.is_some_and(|value| value >= min_commit_index)
                && last_applied.is_some_and(|value| value >= min_last_applied);
            statuses.push(status);
        }
        if converged && statuses.len() == nodes.len() {
            return statuses;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for raft status indexes commitIndex >= {min_commit_index} and lastApplied >= {min_last_applied}; latest={latest:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_metric_at_least(port: u16, name: &str, min: u64) -> u64 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut latest = None;
    loop {
        if let Some(metrics) = http_get_text(port, "/metrics").await {
            let value = metric_value(&metrics, name);
            latest = value;
            if value.is_some_and(|value| value >= min) {
                return value.unwrap();
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for metric {name} >= {min} on port {port}; latest={latest:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn current_metric(port: u16, name: &str) -> u64 {
    let metrics = http_get_text(port, "/metrics")
        .await
        .unwrap_or_else(|| panic!("metrics unavailable on port {port}"));
    metric_value(&metrics, name).unwrap_or_else(|| panic!("metric {name} missing on port {port}"))
}

async fn current_metric_sum_for_nodes(cluster: &RaftCluster, nodes: &[usize], name: &str) -> u64 {
    let mut total = 0_u64;
    for index in nodes {
        total = total.saturating_add(current_metric(cluster.http_ports[*index], name).await);
    }
    total
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FullLogMetrics {
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
struct HttpLockHistoryOp {
    key: String,
    invoke_order: usize,
    response_order: usize,
    result: HttpLockHistoryResult,
}

#[derive(Debug, Clone)]
enum HttpLockHistoryResult {
    Acquired { lock_uuid: String },
    NotAcquired,
    Released { lock_uuid: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HttpLinearModelOp {
    AcquireGranted(u16),
    AcquireRejected,
    Release(u16),
}

async fn current_full_log_metrics(port: u16) -> FullLogMetrics {
    FullLogMetrics {
        reads_total: current_metric(port, "dd_rust_network_mutex_raft_log_full_reads_total").await,
        read_failures_total: current_metric(
            port,
            "dd_rust_network_mutex_raft_log_full_read_failures_total",
        )
        .await,
        read_bytes_total: current_metric(
            port,
            "dd_rust_network_mutex_raft_log_full_read_bytes_total",
        )
        .await,
        read_entries_total: current_metric(
            port,
            "dd_rust_network_mutex_raft_log_full_read_entries_total",
        )
        .await,
        rewrites_total: current_metric(port, "dd_rust_network_mutex_raft_log_full_rewrites_total")
            .await,
        rewrite_failures_total: current_metric(
            port,
            "dd_rust_network_mutex_raft_log_full_rewrite_failures_total",
        )
        .await,
        rewrite_entries_total: current_metric(
            port,
            "dd_rust_network_mutex_raft_log_full_rewrite_entries_total",
        )
        .await,
        rewrite_bytes_total: current_metric(
            port,
            "dd_rust_network_mutex_raft_log_full_rewrite_bytes_total",
        )
        .await,
    }
}

async fn current_full_log_metrics_for_nodes(
    cluster: &RaftCluster,
    nodes: &[usize],
) -> BTreeMap<usize, FullLogMetrics> {
    let mut metrics = BTreeMap::new();
    for index in nodes {
        metrics.insert(
            *index,
            current_full_log_metrics(cluster.http_ports[*index]).await,
        );
    }
    metrics
}

async fn assert_full_log_metrics_unchanged(
    cluster: &RaftCluster,
    nodes: &[usize],
    before: &BTreeMap<usize, FullLogMetrics>,
    label: &str,
) {
    let after = current_full_log_metrics_for_nodes(cluster, nodes).await;
    assert_eq!(
        &after, before,
        "{label} should not use full-log scans, read-failure, rewrite, or rewrite-failure paths; before={before:?} after={after:?}"
    );
}

async fn change_membership_retrying(
    cluster: &RaftCluster,
    current_nodes: &[usize],
    target: &[RaftPeerConfig],
    expected_size: u64,
    expected_quorum: u64,
    label: &str,
) -> Value {
    assert!(
        !current_nodes.is_empty(),
        "{label}: current membership node set must not be empty"
    );
    let body = json!({ "peers": target });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut attempts = 0usize;
    loop {
        attempts = attempts.saturating_add(1);
        let leader = wait_for_leader_among(cluster, current_nodes).await;
        match http_request(
            "POST",
            cluster.http_ports[leader],
            "/raft/membership",
            Some(body.clone()),
        )
        .await
        {
            Ok((status, raw_body)) => match serde_json::from_str::<Value>(&raw_body) {
                Ok(membership) => {
                    if status == 200 {
                        assert_eq!(
                            membership["clusterSize"].as_u64(),
                            Some(expected_size),
                            "{label} clusterSize: {membership:?}"
                        );
                        assert_eq!(
                            membership["quorumSize"].as_u64(),
                            Some(expected_quorum),
                            "{label} quorumSize: {membership:?}"
                        );
                        return membership;
                    }
                    let latest = format!(
                        "status={status} body={membership:?} leader={leader} attempts={attempts}"
                    );
                    if status == 503 && tokio::time::Instant::now() < deadline {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                    panic!(
                        "{label}: membership change failed after {attempts} attempt(s): {latest}"
                    );
                }
                Err(err) => {
                    let latest = format!(
                            "status={status} parse_error={err} raw_body={raw_body:?} leader={leader} attempts={attempts}"
                        );
                    if (status == 0 || status == 503 || raw_body.is_empty())
                        && tokio::time::Instant::now() < deadline
                    {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                    panic!(
                            "{label}: membership response was not JSON after {attempts} attempt(s): {latest}"
                        );
                }
            },
            Err(err) => {
                let latest = format!("io_error={err} leader={leader} attempts={attempts}");
                if tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
                panic!("{label}: membership request failed after {attempts} attempt(s): {latest}");
            }
        }
    }
}

async fn assert_no_raft_auth_rejections(cluster: &RaftCluster, nodes: &[usize], label: &str) {
    for index in nodes {
        let port = cluster.http_ports[*index];
        assert_eq!(
            current_metric(port, "dd_rust_network_mutex_raft_rpc_auth_rejections_total").await,
            0,
            "{label}: node {index} rejected at least one authenticated Raft peer RPC"
        );
    }
}

async fn start_round_robin_lb(backends: Vec<u16>) -> TestLb {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let next = Arc::new(AtomicUsize::new(0));
    let handle = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let backends = backends.clone();
            let next = next.clone();
            tokio::spawn(async move {
                let _ = proxy_http_once(stream, backends, next).await;
            });
        }
    });
    TestLb { port, handle }
}

fn assert_http_lock_history_linearizable(history: &[HttpLockHistoryOp]) {
    assert!(
        !history.is_empty(),
        "HTTP linearizability history should contain operations"
    );
    let mut by_key = BTreeMap::<String, Vec<HttpLockHistoryOp>>::new();
    for op in history {
        by_key.entry(op.key.clone()).or_default().push(op.clone());
    }
    for (key, ops) in by_key {
        assert_http_linearizable_key_history(&key, &ops);
    }
}

fn assert_http_linearizable_key_history(key: &str, ops: &[HttpLockHistoryOp]) {
    assert!(
        ops.len() < 128,
        "linearizability checker supports fewer than 128 operations per key; key={key} ops={}",
        ops.len()
    );
    let mut lock_ids = BTreeMap::<String, u16>::new();
    let mut granted_ids = BTreeSet::<u16>::new();
    for op in ops {
        let lock_uuid = match &op.result {
            HttpLockHistoryResult::Acquired { lock_uuid }
            | HttpLockHistoryResult::Released { lock_uuid } => lock_uuid,
            HttpLockHistoryResult::NotAcquired => continue,
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
            HttpLockHistoryResult::Acquired { lock_uuid } => {
                let id = *lock_ids
                    .get(lock_uuid)
                    .expect("granted lock uuid should be indexed");
                assert!(
                    granted_ids.insert(id),
                    "lock uuid {lock_uuid} was granted more than once for key {key}"
                );
                HttpLinearModelOp::AcquireGranted(id)
            }
            HttpLockHistoryResult::NotAcquired => HttpLinearModelOp::AcquireRejected,
            HttpLockHistoryResult::Released { lock_uuid } => HttpLinearModelOp::Release(
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
                    (predecessor.response_order < candidate.invoke_order).then_some(1u128 << idx)
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
        search_http_linearized_history(0, 0, all_done, &model_ops, &predecessors, &mut memo),
        "no linearization found for key {key}; ops={ops:?}"
    );
}

fn search_http_linearized_history(
    done: u128,
    holder: u16,
    all_done: u128,
    ops: &[HttpLinearModelOp],
    predecessors: &[u128],
    memo: &mut BTreeSet<(u128, u16)>,
) -> bool {
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
        let Some(next_holder) = apply_http_linear_model_op(holder, *op) else {
            continue;
        };
        if search_http_linearized_history(
            done | bit,
            next_holder,
            all_done,
            ops,
            predecessors,
            memo,
        ) {
            return true;
        }
    }
    false
}

fn apply_http_linear_model_op(holder: u16, op: HttpLinearModelOp) -> Option<u16> {
    match op {
        HttpLinearModelOp::AcquireGranted(lock_id) if holder == 0 => Some(lock_id),
        HttpLinearModelOp::AcquireGranted(_) => None,
        HttpLinearModelOp::AcquireRejected if holder != 0 => Some(holder),
        HttpLinearModelOp::AcquireRejected => None,
        HttpLinearModelOp::Release(lock_id) if holder == lock_id => Some(0),
        HttpLinearModelOp::Release(_) => None,
    }
}

struct HttpNoWaitHistoryPhase {
    port: u16,
    keys: Vec<String>,
    history: Arc<Mutex<Vec<HttpLockHistoryOp>>>,
    order: Arc<AtomicUsize>,
    phase: usize,
    workers: usize,
    steps: usize,
    label: String,
}

async fn run_http_no_wait_history_phase(phase_spec: HttpNoWaitHistoryPhase) {
    let HttpNoWaitHistoryPhase {
        port,
        keys,
        history,
        order,
        phase,
        workers,
        steps,
        label,
    } = phase_spec;
    assert!(!keys.is_empty(), "history phase needs lock keys");
    assert!(workers > 0, "history phase needs workers");
    let start = Arc::new(tokio::sync::Barrier::new(workers));
    let mut tasks = Vec::new();

    for worker in 0..workers {
        let keys = keys.clone();
        let history = Arc::clone(&history);
        let order = Arc::clone(&order);
        let start = Arc::clone(&start);
        let label = label.clone();
        tasks.push(tokio::spawn(async move {
            start.wait().await;
            let mut state =
                0xC011_EC7E_D15C_A11Du64 ^ ((phase as u64).wrapping_shl(17)) ^ worker as u64;
            for step in 0..steps {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let key = keys[(state as usize) % keys.len()].clone();
                let acquire_request_id = format!(
                    "{label}-acquire-{phase}-{worker}-{step}-{}",
                    uuid::Uuid::new_v4()
                );
                let acquire_invoke = order.fetch_add(1, Ordering::SeqCst);
                let (status, acquire) = http_post_json_retrying_unavailable(
                    port,
                    "/v1/lock",
                    json!({
                        "key": key.clone(),
                        "ttlMs": 5000,
                        "waitMs": 0,
                        "requestId": acquire_request_id,
                    }),
                    &format!("{label} acquire phase={phase} worker={worker} step={step}"),
                )
                .await;
                let acquire_response = order.fetch_add(1, Ordering::SeqCst);
                assert_eq!(
                    status, 200,
                    "{label} acquire phase={phase} worker={worker} step={step} response: {acquire:?}"
                );
                if acquire["acquired"].as_bool() == Some(true) {
                    let lock_uuid = acquire["lockUuid"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!(
                                "{label} acquire phase={phase} worker={worker} step={step} missing lockUuid: {acquire:?}"
                            )
                        })
                        .to_string();
                    history.lock().expect("history").push(HttpLockHistoryOp {
                        key: key.clone(),
                        invoke_order: acquire_invoke,
                        response_order: acquire_response,
                        result: HttpLockHistoryResult::Acquired {
                            lock_uuid: lock_uuid.clone(),
                        },
                    });
                    tokio::time::sleep(Duration::from_millis(
                        2 + ((worker + step) % 3) as u64,
                    ))
                    .await;
                    let release_request_id = format!(
                        "{label}-release-{phase}-{worker}-{step}-{}",
                        uuid::Uuid::new_v4()
                    );
                    let release_invoke = order.fetch_add(1, Ordering::SeqCst);
                    let (status, release) = http_post_json_retrying_unavailable(
                        port,
                        "/v1/unlock",
                        json!({
                            "key": key.clone(),
                            "lockUuid": lock_uuid.clone(),
                            "requestId": release_request_id,
                        }),
                        &format!("{label} release phase={phase} worker={worker} step={step}"),
                    )
                    .await;
                    let release_response = order.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(
                        status, 200,
                        "{label} release phase={phase} worker={worker} step={step} response: {release:?}"
                    );
                    assert_eq!(
                        release["unlocked"],
                        true,
                        "{label} release phase={phase} worker={worker} step={step} response: {release:?}"
                    );
                    history.lock().expect("history").push(HttpLockHistoryOp {
                        key,
                        invoke_order: release_invoke,
                        response_order: release_response,
                        result: HttpLockHistoryResult::Released { lock_uuid },
                    });
                } else {
                    assert_eq!(
                        acquire["acquired"],
                        false,
                        "{label} acquire phase={phase} worker={worker} step={step} should return a boolean acquired field: {acquire:?}"
                    );
                    history.lock().expect("history").push(HttpLockHistoryOp {
                        key,
                        invoke_order: acquire_invoke,
                        response_order: acquire_response,
                        result: HttpLockHistoryResult::NotAcquired,
                    });
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
        }));
    }

    for task in tasks {
        task.await
            .expect("HTTP history phase worker should not panic");
    }
}

struct LbMembershipChurnHistoryStep<'a> {
    cluster: &'a RaftCluster,
    active_nodes: &'a [usize],
    target_nodes: &'a [usize],
    expected_size: u64,
    expected_quorum: u64,
    phase: usize,
    history: Arc<Mutex<Vec<HttpLockHistoryOp>>>,
    order: Arc<AtomicUsize>,
    label: &'a str,
}

async fn run_lb_membership_churn_history_step(step: LbMembershipChurnHistoryStep<'_>) {
    let LbMembershipChurnHistoryStep {
        cluster,
        active_nodes,
        target_nodes,
        expected_size,
        expected_quorum,
        phase,
        history,
        order,
        label,
    } = step;
    let lb = start_round_robin_lb(
        active_nodes
            .iter()
            .map(|idx| cluster.http_ports[*idx])
            .collect(),
    )
    .await;
    let key = format!(
        "raft-http-linearizable-membership-{label}-{phase}-{}",
        uuid::Uuid::new_v4()
    );
    let all_nodes = (0..cluster.http_ports.len()).collect::<Vec<_>>();
    let before_full_log = current_full_log_metrics_for_nodes(cluster, &all_nodes).await;
    let target_peers = target_nodes
        .iter()
        .map(|idx| cluster.peers[*idx].clone())
        .collect::<Vec<_>>();
    let expected_ids = target_nodes
        .iter()
        .map(|idx| cluster.peers[*idx].id.clone())
        .collect::<BTreeSet<_>>();

    let traffic = {
        let lb_port = lb.port;
        let history = Arc::clone(&history);
        let order = Arc::clone(&order);
        let keys = vec![key];
        let label = label.to_string();
        tokio::spawn(async move {
            let _lb = lb;
            run_http_no_wait_history_phase(HttpNoWaitHistoryPhase {
                port: lb_port,
                keys,
                history,
                order,
                phase,
                workers: 4,
                steps: 12,
                label: format!("lb-membership-churn-{label}"),
            })
            .await;
        })
    };

    tokio::time::sleep(Duration::from_millis(8)).await;
    change_membership_retrying(
        cluster,
        active_nodes,
        &target_peers,
        expected_size,
        expected_quorum,
        label,
    )
    .await;
    wait_for_simple_membership(
        cluster,
        target_nodes,
        &expected_ids,
        expected_size,
        expected_quorum,
    )
    .await;
    traffic
        .await
        .expect("membership churn traffic worker should not panic");
    wait_for_zero_holders_and_waiters_among(cluster, target_nodes).await;
    let leader = wait_for_leader_among(cluster, target_nodes).await;
    let status = http_get_json(cluster.http_ports[leader], "/raft/status")
        .await
        .expect("leader status after membership churn phase");
    let commit_index = status["commitIndex"]
        .as_u64()
        .expect("leader commitIndex after membership churn phase");
    wait_for_status_indexes_at_least_among(cluster, target_nodes, commit_index, commit_index).await;
    assert_full_log_metrics_unchanged(
        cluster,
        &all_nodes,
        &before_full_log,
        "membership churn HTTP LB traffic and catch-up",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_http_followers_proxy_acquire_release_after_quorum_commit() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let cluster = start_cluster().await;
    let leader = wait_for_leader(&cluster).await;
    for (idx, port) in cluster.http_ports.iter().enumerate() {
        let (status, body) = http_request("GET", *port, "/raft/leaderz", None)
            .await
            .expect("leaderz request");
        let parsed: Value = serde_json::from_str(&body).expect("leaderz JSON");
        assert_eq!(parsed["syncLog"], true, "leaderz body: {parsed:?}");
        assert_eq!(parsed["syncCommit"], true, "leaderz body: {parsed:?}");
        assert_eq!(
            parsed["unsafeDurability"], false,
            "leaderz body: {parsed:?}"
        );
        if idx == leader {
            assert_eq!(status, 200, "leader leaderz body: {parsed:?}");
            assert_eq!(parsed["isLeader"], true);
            assert_eq!(parsed["isLeaderReady"], true);
            assert!(
                parsed["leaderQuorumAgeMs"].as_u64().is_some(),
                "leader should expose quorum freshness age: {parsed:?}"
            );
            assert!(
                parsed["leaderQuorumTimeoutMs"]
                    .as_u64()
                    .is_some_and(|timeout| timeout > 0),
                "leader should expose quorum freshness timeout: {parsed:?}"
            );
        } else {
            assert_eq!(status, 503, "follower leaderz body: {parsed:?}");
            assert_eq!(parsed["isLeader"], false);
            assert_eq!(parsed["isLeaderReady"], false);
            assert_eq!(parsed["leaderQuorumAgeMs"], Value::Null);
            assert!(parsed["leaderId"].as_str().is_some());
            assert!(
                parsed["leaderQuorumTimeoutMs"]
                    .as_u64()
                    .is_some_and(|timeout| timeout > 0),
                "follower should expose quorum freshness timeout: {parsed:?}"
            );
        }
    }
    let progress = http_get_json(cluster.http_ports[leader], "/raft/progress")
        .await
        .expect("leader progress");
    assert_eq!(progress["isLeader"], true, "progress body: {progress:?}");
    assert_eq!(progress["syncLog"], true, "progress body: {progress:?}");
    assert_eq!(progress["syncCommit"], true, "progress body: {progress:?}");
    assert_eq!(
        progress["unsafeDurability"], false,
        "progress body: {progress:?}"
    );
    let status = http_get_json(cluster.http_ports[leader], "/raft/status")
        .await
        .expect("leader status");
    assert_eq!(status["syncLog"], true, "status body: {status:?}");
    assert_eq!(status["syncCommit"], true, "status body: {status:?}");
    assert_eq!(status["unsafeDurability"], false, "status body: {status:?}");
    assert_eq!(
        progress["peers"].as_array().map(Vec::len),
        Some(3),
        "progress should list all active peers: {progress:?}"
    );
    assert!(
        progress["peers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|peer| peer["isSelf"] == true
                && peer["caughtUp"] == true
                && peer["membershipRole"] == "voter"),
        "progress should include caught-up self voter: {progress:?}"
    );
    let learners = http_get_json(cluster.http_ports[leader], "/raft/learners")
        .await
        .expect("leader learners");
    assert_eq!(learners["isLeader"], true, "learners body: {learners:?}");
    assert_eq!(
        learners["learners"].as_array().map(Vec::len),
        Some(0),
        "fresh cluster should not have staged learners: {learners:?}"
    );
    let follower_a = (leader + 1) % 3;
    let follower_b = (leader + 2) % 3;
    let key = format!("raft-key-{}", uuid::Uuid::new_v4());

    let (status, acquire) = http_post_json(
        cluster.http_ports[follower_a],
        "/v1/lock",
        json!({"key": key, "ttlMs": 5000}),
    )
    .await;
    assert_eq!(status, 200, "acquire response: {acquire:?}");
    assert_eq!(acquire["acquired"], true, "acquire response: {acquire:?}");
    let lock_uuid = acquire["lockUuid"].as_str().unwrap().to_string();

    let (status, queued) = http_post_json(
        cluster.http_ports[follower_b],
        "/v1/lock",
        json!({"key": key, "ttlMs": 5000, "waitMs": 50}),
    )
    .await;
    assert_eq!(status, 200, "queued response: {queued:?}");
    assert_eq!(
        queued["acquired"], false,
        "contended short-poll acquire should time out: {queued:?}"
    );

    let (status, release) = http_post_json(
        cluster.http_ports[follower_b],
        "/v1/unlock",
        json!({"key": key, "lockUuid": lock_uuid}),
    )
    .await;
    assert_eq!(status, 200, "release response: {release:?}");
    assert_eq!(release["unlocked"], true, "release response: {release:?}");
    wait_for_zero_holders_and_waiters(&cluster).await;

    let (status, reacquire) = http_post_json(
        cluster.http_ports[follower_a],
        "/v1/lock",
        json!({"key": key, "ttlMs": 5000}),
    )
    .await;
    assert_eq!(status, 200, "reacquire response: {reacquire:?}");
    assert_eq!(
        reacquire["acquired"], true,
        "lock should be acquirable after quorum release: {reacquire:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_http_follower_proxy_uses_authenticated_direct_peer_rpc() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let tuning = RaftClusterTuning {
        peer_token: Some(format!("raft-peer-token-{}", uuid::Uuid::new_v4())),
        ..RaftClusterTuning::default()
    };
    let cluster = start_cluster_with_tuning(3, 3, tuning).await;
    let leader = wait_for_leader(&cluster).await;
    let follower = (leader + 1) % 3;
    let follower_status = http_get_json(cluster.http_ports[follower], "/raft/status")
        .await
        .expect("follower status");
    assert_eq!(
        follower_status["isLeader"], false,
        "test target should be a follower before public HTTP write: {follower_status:?}"
    );

    let key = format!("raft-auth-proxy-key-{}", uuid::Uuid::new_v4());
    let (status, acquire) = http_post_json(
        cluster.http_ports[follower],
        "/v1/lock",
        json!({"key": key, "ttlMs": 5000}),
    )
    .await;
    assert_eq!(status, 200, "follower acquire response: {acquire:?}");
    assert_eq!(
        acquire["acquired"], true,
        "follower acquire response: {acquire:?}"
    );
    let lock_uuid = acquire["lockUuid"].as_str().unwrap().to_string();

    let (status, release) = http_post_json(
        cluster.http_ports[follower],
        "/v1/unlock",
        json!({"key": key, "lockUuid": lock_uuid}),
    )
    .await;
    assert_eq!(status, 200, "follower release response: {release:?}");
    assert_eq!(
        release["unlocked"], true,
        "follower release response: {release:?}"
    );
    wait_for_zero_holders_and_waiters(&cluster).await;

    wait_for_metric_at_least(
        cluster.http_ports[follower],
        "dd_rust_network_mutex_raft_proxy_requests_forwarded_total",
        2,
    )
    .await;
    wait_for_metric_at_least(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_proxy_requests_handled_total",
        2,
    )
    .await;
    assert_no_raft_auth_rejections(
        &cluster,
        &[leader],
        "direct peer proxy should carry raft.peer_token",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_membership_change_uses_authenticated_peer_rpc_for_learner_catchup() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let tuning = RaftClusterTuning {
        append_entries_max_entries: 2,
        append_entries_max_bytes: usize::MAX,
        peer_token: Some(format!("raft-peer-token-{}", uuid::Uuid::new_v4())),
        ..RaftClusterTuning::default()
    };
    let cluster = start_cluster_with_tuning(4, 3, tuning).await;
    let initial_nodes = vec![0, 1, 2];
    let leader = wait_for_leader_among(&cluster, &initial_nodes).await;

    for step in 0..4 {
        let key = format!(
            "raft-auth-membership-catchup-{step}-{}",
            uuid::Uuid::new_v4()
        );
        let (status, acquire) = http_post_json(
            cluster.http_ports[leader],
            "/v1/lock",
            json!({"key": key, "ttlMs": 5000}),
        )
        .await;
        assert_eq!(status, 200, "pre-membership acquire response: {acquire:?}");
        assert_eq!(
            acquire["acquired"], true,
            "pre-membership acquire response: {acquire:?}"
        );
        let lock_uuid = acquire["lockUuid"].as_str().unwrap().to_string();
        let (status, release) = http_post_json(
            cluster.http_ports[leader],
            "/v1/unlock",
            json!({"key": key, "lockUuid": lock_uuid}),
        )
        .await;
        assert_eq!(status, 200, "pre-membership release response: {release:?}");
        assert_eq!(
            release["unlocked"], true,
            "pre-membership release response: {release:?}"
        );
    }
    let before_new_node_appended = current_metric(
        cluster.http_ports[3],
        "dd_rust_network_mutex_raft_follower_append_appended_entries_total",
    )
    .await;

    let (status, membership) = http_post_json(
        cluster.http_ports[leader],
        "/raft/membership",
        json!({"peers": cluster.peers.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "authenticated membership change response: {membership:?}"
    );
    assert_eq!(membership["clusterSize"].as_u64(), Some(4));
    assert_eq!(membership["quorumSize"].as_u64(), Some(3));

    let all_nodes = vec![0, 1, 2, 3];
    let expected_ids = cluster
        .peers
        .iter()
        .map(|peer| peer.id.clone())
        .collect::<BTreeSet<_>>();
    wait_for_simple_membership(&cluster, &all_nodes, &expected_ids, 4, 3).await;
    wait_for_zero_holders_and_waiters_among(&cluster, &all_nodes).await;
    assert!(
        current_metric(
            cluster.http_ports[3],
            "dd_rust_network_mutex_raft_follower_append_appended_entries_total"
        )
        .await
            > before_new_node_appended,
        "new voter should catch up through authenticated AppendEntries RPCs"
    );
    assert_no_raft_auth_rejections(
        &cluster,
        &all_nodes,
        "membership change and learner catch-up should use raft.peer_token",
    )
    .await;

    let active_leader = wait_for_leader_among(&cluster, &all_nodes).await;
    let key = format!("raft-auth-membership-post-{}", uuid::Uuid::new_v4());
    let (status, acquire) = http_post_json(
        cluster.http_ports[active_leader],
        "/v1/lock",
        json!({"key": key, "ttlMs": 5000}),
    )
    .await;
    assert_eq!(status, 200, "post-membership acquire response: {acquire:?}");
    assert_eq!(
        acquire["acquired"], true,
        "post-membership acquire response: {acquire:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_lb_round_robin_survives_leader_failover() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let mut cluster = start_cluster().await;
    let old_leader = wait_for_leader(&cluster).await;
    let lb = start_round_robin_lb(cluster.http_ports.clone()).await;
    let key = format!("raft-failover-key-{}", uuid::Uuid::new_v4());

    let (status, acquire) =
        http_post_json(lb.port, "/v1/lock", json!({"key": key, "ttlMs": 5000})).await;
    assert_eq!(status, 200, "acquire response: {acquire:?}");
    assert_eq!(acquire["acquired"], true, "acquire response: {acquire:?}");
    let lock_uuid = acquire["lockUuid"].as_str().unwrap().to_string();
    let acquired_status =
        wait_for_status_index_at_least(cluster.http_ports[old_leader], "lastApplied", 1).await;
    let acquired_index = acquired_status["lastApplied"]
        .as_u64()
        .expect("old leader lastApplied after acquire");

    cluster.abort_node(old_leader).await;
    let survivors: Vec<usize> = (0..cluster.http_ports.len())
        .filter(|idx| *idx != old_leader)
        .collect();
    let new_leader = wait_for_leader_among(&cluster, &survivors).await;
    assert_ne!(new_leader, old_leader);
    wait_for_status_indexes_at_least_among(&cluster, &survivors, acquired_index, acquired_index)
        .await;

    let (status, release) = http_post_json(
        lb.port,
        "/v1/unlock",
        json!({"key": key, "lockUuid": lock_uuid}),
    )
    .await;
    assert_eq!(status, 200, "release response: {release:?}");
    assert_eq!(release["unlocked"], true, "release response: {release:?}");
    let release_status = wait_for_status_index_at_least(
        cluster.http_ports[new_leader],
        "lastApplied",
        acquired_index.saturating_add(1),
    )
    .await;
    let release_index = release_status["lastApplied"]
        .as_u64()
        .expect("new leader lastApplied after release");
    wait_for_status_indexes_at_least_among(&cluster, &survivors, release_index, release_index)
        .await;
    wait_for_zero_holders_and_waiters_among(&cluster, &survivors).await;

    let (status, reacquire) =
        http_post_json(lb.port, "/v1/lock", json!({"key": key, "ttlMs": 5000})).await;
    assert_eq!(status, 200, "reacquire response: {reacquire:?}");
    assert_eq!(
        reacquire["acquired"], true,
        "LB should route to the surviving Raft quorum after leader failover: {reacquire:?}"
    );
    let reacquire_status = wait_for_status_index_at_least(
        cluster.http_ports[new_leader],
        "lastApplied",
        release_index.saturating_add(1),
    )
    .await;
    let reacquire_index = reacquire_status["lastApplied"]
        .as_u64()
        .expect("new leader lastApplied after reacquire");
    wait_for_status_indexes_at_least_among(&cluster, &survivors, reacquire_index, reacquire_index)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_five_voter_cluster_commits_with_three_node_quorum() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let mut cluster = start_cluster_with_nodes(5, 5).await;
    let all_nodes = vec![0, 1, 2, 3, 4];
    let expected_ids = cluster
        .peers
        .iter()
        .map(|peer| peer.id.clone())
        .collect::<BTreeSet<_>>();
    wait_for_simple_membership(&cluster, &all_nodes, &expected_ids, 5, 3).await;

    let initial_leader = wait_for_leader(&cluster).await;
    let victims = all_nodes
        .iter()
        .copied()
        .filter(|idx| *idx != initial_leader)
        .take(2)
        .collect::<Vec<_>>();
    assert_eq!(
        victims.len(),
        2,
        "test should have two non-leader voters to take down"
    );
    let survivors = all_nodes
        .iter()
        .copied()
        .filter(|idx| !victims.contains(idx))
        .collect::<Vec<_>>();
    assert_eq!(
        survivors.len(),
        3,
        "test should leave exactly a 3-of-5 quorum online"
    );

    for victim in &victims {
        cluster.abort_node(*victim).await;
    }
    let active_leader = wait_for_leader_among(&cluster, &survivors).await;
    let survivor_statuses =
        wait_for_status_indexes_at_least_among(&cluster, &survivors, 0, 0).await;
    for status in &survivor_statuses {
        assert_eq!(
            status["clusterSize"].as_u64(),
            Some(5),
            "survivors should retain five-voter membership: {status:?}"
        );
        assert_eq!(
            status["quorumSize"].as_u64(),
            Some(3),
            "survivors should use strict-majority 3-of-5 quorum: {status:?}"
        );
    }

    let lb = start_round_robin_lb(cluster.http_ports.clone()).await;
    let before_full_log = current_full_log_metrics_for_nodes(&cluster, &survivors).await;
    let key = format!("raft-five-voter-quorum-{}", uuid::Uuid::new_v4());
    let (status, acquire) =
        http_post_json(lb.port, "/v1/lock", json!({"key": key, "ttlMs": 5000})).await;
    assert_eq!(status, 200, "3-of-5 acquire response: {acquire:?}");
    assert_eq!(
        acquire["acquired"], true,
        "3-of-5 acquire response: {acquire:?}"
    );
    let lock_uuid = acquire["lockUuid"]
        .as_str()
        .expect("3-of-5 acquire should return a lockUuid")
        .to_string();
    let acquire_status =
        wait_for_status_index_at_least(cluster.http_ports[active_leader], "lastApplied", 1).await;
    let acquire_index = acquire_status["lastApplied"]
        .as_u64()
        .expect("active leader lastApplied after 3-of-5 acquire");
    wait_for_status_indexes_at_least_among(&cluster, &survivors, acquire_index, acquire_index)
        .await;

    let (status, release) = http_post_json(
        lb.port,
        "/v1/unlock",
        json!({"key": key, "lockUuid": lock_uuid}),
    )
    .await;
    assert_eq!(status, 200, "3-of-5 release response: {release:?}");
    assert_eq!(
        release["unlocked"], true,
        "3-of-5 release response: {release:?}"
    );
    let release_status = wait_for_status_index_at_least(
        cluster.http_ports[active_leader],
        "lastApplied",
        acquire_index.saturating_add(1),
    )
    .await;
    let release_index = release_status["lastApplied"]
        .as_u64()
        .expect("active leader lastApplied after 3-of-5 release");
    wait_for_status_indexes_at_least_among(&cluster, &survivors, release_index, release_index)
        .await;
    wait_for_zero_holders_and_waiters_among(&cluster, &survivors).await;
    assert_full_log_metrics_unchanged(
        &cluster,
        &survivors,
        &before_full_log,
        "steady 3-of-5 quorum traffic",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_five_voter_minority_rejects_public_write_without_phantom_lock() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let mut cluster = start_cluster_with_nodes(5, 5).await;
    let all_nodes = vec![0, 1, 2, 3, 4];
    let expected_ids = cluster
        .peers
        .iter()
        .map(|peer| peer.id.clone())
        .collect::<BTreeSet<_>>();
    wait_for_simple_membership(&cluster, &all_nodes, &expected_ids, 5, 3).await;

    let old_leader = wait_for_leader(&cluster).await;
    let baseline_status =
        wait_for_status_index_at_least(cluster.http_ports[old_leader], "lastApplied", 0).await;
    let baseline_applied = baseline_status["lastApplied"]
        .as_u64()
        .expect("old leader baseline lastApplied");
    wait_for_status_indexes_at_least_among(
        &cluster,
        &all_nodes,
        baseline_applied,
        baseline_applied,
    )
    .await;

    let victims = all_nodes
        .iter()
        .copied()
        .filter(|idx| *idx != old_leader)
        .take(3)
        .collect::<Vec<_>>();
    assert_eq!(
        victims.len(),
        3,
        "test should have three non-leader voters to take down"
    );
    let minority = all_nodes
        .iter()
        .copied()
        .filter(|idx| !victims.contains(idx))
        .collect::<Vec<_>>();
    assert!(
        minority.contains(&old_leader),
        "old leader should be left in the two-node minority"
    );
    assert_eq!(
        minority.len(),
        2,
        "test should leave exactly a 2-of-5 minority online"
    );

    for victim in &victims {
        cluster.abort_node(*victim).await;
    }

    let key = format!("raft-five-voter-minority-{}", uuid::Uuid::new_v4());
    let rejected_request_id = format!("raft-five-voter-minority-{}", uuid::Uuid::new_v4());
    let (status, rejected_headers, rejected) = http_post_json_with_headers(
        cluster.http_ports[old_leader],
        "/v1/lock",
        json!({
            "key": key.clone(),
            "ttlMs": 5000,
            "waitMs": 0,
            "requestId": rejected_request_id,
        }),
    )
    .await;
    assert_eq!(
        status, 503,
        "2-of-5 minority write should be rejected: {rejected:?}"
    );
    assert_eq!(
        rejected_headers.get("x-lmx-request-id"),
        Some(&rejected_request_id),
        "minority rejection should echo the retry handle request id"
    );
    assert_ne!(
        rejected["acquired"], true,
        "2-of-5 minority must not report a granted lock: {rejected:?}"
    );
    assert_ne!(
        rejected["outcomeUnknown"], true,
        "active-quorum admission failure happens before append, so the response should not claim an unknown committed outcome: {rejected:?}"
    );
    wait_for_zero_holders_and_waiters_among(&cluster, &minority).await;
    let minority_statuses = wait_for_status_indexes_at_least_among(
        &cluster,
        &minority,
        baseline_applied,
        baseline_applied,
    )
    .await;
    for status in &minority_statuses {
        assert_eq!(
            status["lastApplied"].as_u64(),
            Some(baseline_applied),
            "minority rejection must not apply a client entry: {status:?}"
        );
        assert_eq!(
            status["clusterSize"].as_u64(),
            Some(5),
            "minority survivors should retain five-voter membership: {status:?}"
        );
        assert_eq!(
            status["quorumSize"].as_u64(),
            Some(3),
            "minority survivors should still require 3-of-5 quorum: {status:?}"
        );
    }

    cluster.restart_node(victims[0]).await;
    let restored = all_nodes
        .iter()
        .copied()
        .filter(|idx| !victims[1..].contains(idx))
        .collect::<Vec<_>>();
    assert_eq!(
        restored.len(),
        3,
        "restoring one voter should create exactly a 3-of-5 quorum"
    );
    let restored_leader = wait_for_leader_among(&cluster, &restored).await;
    wait_for_status_indexes_at_least_among(&cluster, &restored, baseline_applied, baseline_applied)
        .await;

    let restored_ports = restored
        .iter()
        .map(|idx| cluster.http_ports[*idx])
        .collect::<Vec<_>>();
    let lb = start_round_robin_lb(restored_ports).await;
    let (status, acquire) =
        http_post_json(lb.port, "/v1/lock", json!({"key": key, "ttlMs": 5000})).await;
    assert_eq!(
        status, 200,
        "restored 3-of-5 acquire after minority rejection: {acquire:?}"
    );
    assert_eq!(
        acquire["acquired"], true,
        "minority rejection must not leave a phantom held lock: {acquire:?}"
    );
    let lock_uuid = acquire["lockUuid"]
        .as_str()
        .expect("restored quorum acquire should return a lockUuid")
        .to_string();
    let acquire_status = wait_for_status_index_at_least(
        cluster.http_ports[restored_leader],
        "lastApplied",
        baseline_applied.saturating_add(1),
    )
    .await;
    let acquire_index = acquire_status["lastApplied"]
        .as_u64()
        .expect("restored leader lastApplied after post-minority acquire");
    wait_for_status_indexes_at_least_among(&cluster, &restored, acquire_index, acquire_index).await;

    let (status, release) = http_post_json(
        lb.port,
        "/v1/unlock",
        json!({"key": key, "lockUuid": lock_uuid}),
    )
    .await;
    assert_eq!(
        status, 200,
        "restored 3-of-5 release after minority rejection: {release:?}"
    );
    assert_eq!(
        release["unlocked"], true,
        "restored 3-of-5 release after minority rejection: {release:?}"
    );
    let release_status = wait_for_status_index_at_least(
        cluster.http_ports[restored_leader],
        "lastApplied",
        acquire_index.saturating_add(1),
    )
    .await;
    let release_index = release_status["lastApplied"]
        .as_u64()
        .expect("restored leader lastApplied after post-minority release");
    wait_for_status_indexes_at_least_among(&cluster, &restored, release_index, release_index).await;
    wait_for_zero_holders_and_waiters_among(&cluster, &restored).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_lb_failover_no_wait_history_is_linearizable() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let mut cluster = start_cluster().await;
    let lb = start_round_robin_lb(cluster.http_ports.clone()).await;
    wait_for_leader(&cluster).await;

    let all_nodes: Vec<usize> = (0..cluster.http_ports.len()).collect();
    let keys = vec![format!(
        "raft-http-linearizable-failover-key-{}",
        uuid::Uuid::new_v4()
    )];
    let history = Arc::new(Mutex::new(Vec::<HttpLockHistoryOp>::new()));
    let order = Arc::new(AtomicUsize::new(0));

    let before_phase0_full_log = current_full_log_metrics_for_nodes(&cluster, &all_nodes).await;
    run_http_no_wait_history_phase(HttpNoWaitHistoryPhase {
        port: lb.port,
        keys: keys.clone(),
        history: Arc::clone(&history),
        order: Arc::clone(&order),
        phase: 0,
        workers: 3,
        steps: 6,
        label: "lb-before-failover".to_string(),
    })
    .await;
    wait_for_zero_holders_and_waiters(&cluster).await;
    assert_full_log_metrics_unchanged(
        &cluster,
        &all_nodes,
        &before_phase0_full_log,
        "pre-failover HTTP LB no-wait traffic",
    )
    .await;

    let old_leader = wait_for_leader(&cluster).await;
    let old_leader_status =
        wait_for_status_index_at_least(cluster.http_ports[old_leader], "lastApplied", 1).await;
    let pre_failover_applied = old_leader_status["lastApplied"]
        .as_u64()
        .expect("old leader lastApplied before failover");
    let survivors: Vec<usize> = all_nodes
        .iter()
        .copied()
        .filter(|idx| *idx != old_leader)
        .collect();
    let before_failover_full_log = current_full_log_metrics_for_nodes(&cluster, &survivors).await;

    cluster.abort_node(old_leader).await;
    let new_leader = wait_for_leader_among(&cluster, &survivors).await;
    assert_ne!(new_leader, old_leader);
    wait_for_status_indexes_at_least_among(
        &cluster,
        &survivors,
        pre_failover_applied,
        pre_failover_applied,
    )
    .await;
    assert_full_log_metrics_unchanged(
        &cluster,
        &survivors,
        &before_failover_full_log,
        "leader failover and survivor convergence",
    )
    .await;

    let before_phase1_full_log = current_full_log_metrics_for_nodes(&cluster, &survivors).await;
    run_http_no_wait_history_phase(HttpNoWaitHistoryPhase {
        port: lb.port,
        keys,
        history: Arc::clone(&history),
        order: Arc::clone(&order),
        phase: 1,
        workers: 3,
        steps: 6,
        label: "lb-after-failover".to_string(),
    })
    .await;
    wait_for_zero_holders_and_waiters_among(&cluster, &survivors).await;
    assert_full_log_metrics_unchanged(
        &cluster,
        &survivors,
        &before_phase1_full_log,
        "post-failover HTTP LB no-wait traffic",
    )
    .await;

    let history = history.lock().expect("history").clone();
    assert_http_lock_history_linearizable(&history);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_lb_rolling_restart_no_wait_history_is_linearizable() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let mut cluster = start_cluster().await;
    let lb = start_round_robin_lb(cluster.http_ports.clone()).await;
    let all_nodes: Vec<usize> = (0..cluster.http_ports.len()).collect();
    let keys = vec![format!(
        "raft-http-linearizable-rolling-key-{}",
        uuid::Uuid::new_v4()
    )];
    let history = Arc::new(Mutex::new(Vec::<HttpLockHistoryOp>::new()));
    let order = Arc::new(AtomicUsize::new(0));

    for (phase, restart_index) in all_nodes.iter().copied().enumerate() {
        wait_for_leader(&cluster).await;
        let before_phase_full_log = current_full_log_metrics_for_nodes(&cluster, &all_nodes).await;
        run_http_no_wait_history_phase(HttpNoWaitHistoryPhase {
            port: lb.port,
            keys: keys.clone(),
            history: Arc::clone(&history),
            order: Arc::clone(&order),
            phase,
            workers: 3,
            steps: 5,
            label: "lb-rolling-restart".to_string(),
        })
        .await;
        wait_for_zero_holders_and_waiters(&cluster).await;
        let leader = wait_for_leader(&cluster).await;
        let phase_status = http_get_json(cluster.http_ports[leader], "/raft/status")
            .await
            .expect("leader status after rolling-restart traffic phase");
        let phase_commit = phase_status["commitIndex"]
            .as_u64()
            .expect("leader commitIndex after rolling-restart traffic phase");
        wait_for_status_indexes_at_least_among(&cluster, &all_nodes, phase_commit, phase_commit)
            .await;
        assert_full_log_metrics_unchanged(
            &cluster,
            &all_nodes,
            &before_phase_full_log,
            "rolling-restart HTTP LB traffic phase",
        )
        .await;

        let survivors = all_nodes
            .iter()
            .copied()
            .filter(|idx| *idx != restart_index)
            .collect::<Vec<_>>();
        let before_restart_survivor_full_log =
            current_full_log_metrics_for_nodes(&cluster, &survivors).await;
        cluster.restart_node(restart_index).await;
        let leader = wait_for_leader(&cluster).await;
        let restart_status = http_get_json(cluster.http_ports[leader], "/raft/status")
            .await
            .expect("leader status after rolling restart");
        let restart_commit = restart_status["commitIndex"]
            .as_u64()
            .expect("leader commitIndex after rolling restart");
        wait_for_status_indexes_at_least_among(
            &cluster,
            &all_nodes,
            restart_commit,
            restart_commit,
        )
        .await;
        wait_for_zero_holders_and_waiters(&cluster).await;
        assert_full_log_metrics_unchanged(
            &cluster,
            &survivors,
            &before_restart_survivor_full_log,
            "rolling-restart survivor convergence",
        )
        .await;
    }

    let before_final_full_log = current_full_log_metrics_for_nodes(&cluster, &all_nodes).await;
    run_http_no_wait_history_phase(HttpNoWaitHistoryPhase {
        port: lb.port,
        keys,
        history: Arc::clone(&history),
        order: Arc::clone(&order),
        phase: all_nodes.len(),
        workers: 3,
        steps: 5,
        label: "lb-rolling-restart-final".to_string(),
    })
    .await;
    wait_for_zero_holders_and_waiters(&cluster).await;
    let leader = wait_for_leader(&cluster).await;
    let final_status = http_get_json(cluster.http_ports[leader], "/raft/status")
        .await
        .expect("leader status after final rolling-restart traffic phase");
    let final_commit = final_status["commitIndex"]
        .as_u64()
        .expect("leader commitIndex after final rolling-restart traffic phase");
    wait_for_status_indexes_at_least_among(&cluster, &all_nodes, final_commit, final_commit).await;
    assert_full_log_metrics_unchanged(
        &cluster,
        &all_nodes,
        &before_final_full_log,
        "final rolling-restart HTTP LB traffic phase",
    )
    .await;

    let history = history.lock().expect("history").clone();
    assert!(
        history
            .iter()
            .any(|op| matches!(op.result, HttpLockHistoryResult::Acquired { .. })),
        "rolling-restart HTTP history should record successful acquires"
    );
    assert!(
        history
            .iter()
            .any(|op| matches!(op.result, HttpLockHistoryResult::NotAcquired)),
        "rolling-restart HTTP history should record contended no-wait acquires"
    );
    assert_http_lock_history_linearizable(&history);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_lb_rolling_config_skew_restart_no_wait_history_is_linearizable() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let mut cluster = start_cluster().await;
    let lb = start_round_robin_lb(cluster.http_ports.clone()).await;
    let all_nodes: Vec<usize> = (0..cluster.http_ports.len()).collect();
    let upgraded_tuning = RaftClusterTuning {
        append_entries_max_entries: 1,
        append_entries_max_bytes: 384,
        append_entries_max_inline_batches: 1,
        target_quorum_extra_fanout: 1,
        install_snapshot_chunk_bytes: 256,
        client_batch_max_entries: 1,
        client_pipeline_max_batches: 1,
        client_batch_max_delay: Duration::from_millis(3),
        client_response_cache_max_entries: 64,
        proxy_retry_budget: Duration::from_secs(3),
        ..RaftClusterTuning::default()
    };
    let keys = vec![format!(
        "raft-http-linearizable-config-skew-key-{}",
        uuid::Uuid::new_v4()
    )];
    let history = Arc::new(Mutex::new(Vec::<HttpLockHistoryOp>::new()));
    let order = Arc::new(AtomicUsize::new(0));

    for (phase, restart_index) in all_nodes.iter().copied().enumerate() {
        wait_for_leader(&cluster).await;
        let survivors = all_nodes
            .iter()
            .copied()
            .filter(|idx| *idx != restart_index)
            .collect::<Vec<_>>();
        let before_survivor_full_log =
            current_full_log_metrics_for_nodes(&cluster, &survivors).await;

        let traffic = {
            let lb_port = lb.port;
            let keys = keys.clone();
            let history = Arc::clone(&history);
            let order = Arc::clone(&order);
            tokio::spawn(async move {
                run_http_no_wait_history_phase(HttpNoWaitHistoryPhase {
                    port: lb_port,
                    keys,
                    history,
                    order,
                    phase,
                    workers: 3,
                    steps: 4,
                    label: "lb-rolling-config-skew".to_string(),
                })
                .await;
            })
        };

        tokio::time::sleep(Duration::from_millis(8)).await;
        cluster
            .restart_node_with_tuning(restart_index, upgraded_tuning.clone())
            .await;
        traffic
            .await
            .expect("rolling config-skew traffic worker should not panic");
        wait_for_zero_holders_and_waiters(&cluster).await;
        let leader = wait_for_leader(&cluster).await;
        let status = http_get_json(cluster.http_ports[leader], "/raft/status")
            .await
            .expect("leader status after rolling config-skew restart");
        let commit_index = status["commitIndex"]
            .as_u64()
            .expect("leader commitIndex after rolling config-skew restart");
        wait_for_status_indexes_at_least_among(&cluster, &all_nodes, commit_index, commit_index)
            .await;
        assert_full_log_metrics_unchanged(
            &cluster,
            &survivors,
            &before_survivor_full_log,
            "survivors during rolling config-skew restart",
        )
        .await;
    }

    let before_final_full_log = current_full_log_metrics_for_nodes(&cluster, &all_nodes).await;
    run_http_no_wait_history_phase(HttpNoWaitHistoryPhase {
        port: lb.port,
        keys,
        history: Arc::clone(&history),
        order: Arc::clone(&order),
        phase: all_nodes.len(),
        workers: 3,
        steps: 4,
        label: "lb-rolling-config-skew-final".to_string(),
    })
    .await;
    wait_for_zero_holders_and_waiters(&cluster).await;
    let leader = wait_for_leader(&cluster).await;
    let status = http_get_json(cluster.http_ports[leader], "/raft/status")
        .await
        .expect("leader status after final config-skew traffic phase");
    let commit_index = status["commitIndex"]
        .as_u64()
        .expect("leader commitIndex after final config-skew traffic phase");
    wait_for_status_indexes_at_least_among(&cluster, &all_nodes, commit_index, commit_index).await;
    assert_full_log_metrics_unchanged(
        &cluster,
        &all_nodes,
        &before_final_full_log,
        "final all-upgraded config-skew HTTP LB traffic phase",
    )
    .await;

    let history = history.lock().expect("history").clone();
    assert!(
        history
            .iter()
            .any(|op| matches!(op.result, HttpLockHistoryResult::Acquired { .. })),
        "rolling config-skew HTTP history should record successful acquires"
    );
    assert!(
        history
            .iter()
            .any(|op| matches!(op.result, HttpLockHistoryResult::NotAcquired)),
        "rolling config-skew HTTP history should record contended no-wait acquires"
    );
    assert_http_lock_history_linearizable(&history);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_lb_live_leader_kill_restart_history_is_linearizable() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let mut cluster = start_cluster().await;
    let lb = start_round_robin_lb(cluster.http_ports.clone()).await;
    let old_leader = wait_for_leader(&cluster).await;
    let all_nodes: Vec<usize> = (0..cluster.http_ports.len()).collect();
    let survivors = all_nodes
        .iter()
        .copied()
        .filter(|idx| *idx != old_leader)
        .collect::<Vec<_>>();
    let keys = vec![format!(
        "raft-http-linearizable-live-kill-key-{}",
        uuid::Uuid::new_v4()
    )];
    let history = Arc::new(Mutex::new(Vec::<HttpLockHistoryOp>::new()));
    let order = Arc::new(AtomicUsize::new(0));
    let before_survivor_full_log = current_full_log_metrics_for_nodes(&cluster, &survivors).await;

    let traffic = {
        let history = Arc::clone(&history);
        let order = Arc::clone(&order);
        let keys = keys.clone();
        tokio::spawn(async move {
            run_http_no_wait_history_phase(HttpNoWaitHistoryPhase {
                port: lb.port,
                keys,
                history,
                order,
                phase: 0,
                workers: 3,
                steps: 4,
                label: "lb-live-leader-kill".to_string(),
            })
            .await;
        })
    };

    tokio::time::sleep(Duration::from_millis(8)).await;
    cluster.abort_node(old_leader).await;
    let new_leader = wait_for_leader_among(&cluster, &survivors).await;
    assert_ne!(new_leader, old_leader);

    tokio::time::sleep(Duration::from_millis(8)).await;
    cluster.restart_node(old_leader).await;

    traffic
        .await
        .expect("live leader kill/restart traffic worker should not panic");
    wait_for_zero_holders_and_waiters(&cluster).await;
    let leader = wait_for_leader(&cluster).await;
    let status = http_get_json(cluster.http_ports[leader], "/raft/status")
        .await
        .expect("leader status after live kill/restart history");
    let commit_index = status["commitIndex"]
        .as_u64()
        .expect("leader commitIndex after live kill/restart history");
    wait_for_status_indexes_at_least_among(&cluster, &all_nodes, commit_index, commit_index).await;
    assert_full_log_metrics_unchanged(
        &cluster,
        &survivors,
        &before_survivor_full_log,
        "surviving nodes during live leader kill/restart HTTP LB traffic",
    )
    .await;

    let history = history.lock().expect("history").clone();
    assert!(
        history
            .iter()
            .any(|op| matches!(op.result, HttpLockHistoryResult::Acquired { .. })),
        "live leader kill/restart HTTP history should record successful acquires"
    );
    assert!(
        history
            .iter()
            .any(|op| matches!(op.result, HttpLockHistoryResult::NotAcquired)),
        "live leader kill/restart HTTP history should record contended no-wait acquires"
    );
    assert_http_lock_history_linearizable(&history);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_lb_request_id_retry_after_failover_uses_replicated_cache_without_append() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let mut cluster = start_cluster().await;
    let lb = start_round_robin_lb(cluster.http_ports.clone()).await;
    wait_for_leader(&cluster).await;
    let all_nodes: Vec<usize> = (0..cluster.http_ports.len()).collect();
    let key = format!("raft-http-idempotent-failover-key-{}", uuid::Uuid::new_v4());
    let request_id = format!("raft-http-idempotent-failover-{}", uuid::Uuid::new_v4());
    let acquire_body = json!({
        "key": key.clone(),
        "ttlMs": 5000,
        "waitMs": 0,
        "requestId": request_id,
    });

    let (status, first) = http_post_json_retrying_unavailable(
        lb.port,
        "/v1/lock",
        acquire_body.clone(),
        "idempotent first acquire before failover",
    )
    .await;
    assert_eq!(status, 200, "first acquire response: {first:?}");
    assert_eq!(first["acquired"], true, "first acquire response: {first:?}");
    let lock_uuid = first["lockUuid"]
        .as_str()
        .expect("first acquire lockUuid")
        .to_string();

    let old_leader = wait_for_leader(&cluster).await;
    let old_leader_status = http_get_json(cluster.http_ports[old_leader], "/raft/status")
        .await
        .expect("old leader status after first acquire");
    let first_commit = old_leader_status["commitIndex"]
        .as_u64()
        .expect("old leader commitIndex after first acquire");
    wait_for_status_indexes_at_least_among(&cluster, &all_nodes, first_commit, first_commit).await;

    cluster.abort_node(old_leader).await;
    let survivors = all_nodes
        .iter()
        .copied()
        .filter(|idx| *idx != old_leader)
        .collect::<Vec<_>>();
    let new_leader = wait_for_leader_among(&cluster, &survivors).await;
    assert_ne!(new_leader, old_leader);
    wait_for_status_indexes_at_least_among(&cluster, &survivors, first_commit, first_commit).await;

    let before_full_log = current_full_log_metrics_for_nodes(&cluster, &survivors).await;
    let before_proposals = current_metric_sum_for_nodes(
        &cluster,
        &survivors,
        "dd_rust_network_mutex_raft_client_proposals_total",
    )
    .await;
    let before_batches = current_metric_sum_for_nodes(
        &cluster,
        &survivors,
        "dd_rust_network_mutex_raft_client_batches_total",
    )
    .await;
    let before_batch_entries = current_metric_sum_for_nodes(
        &cluster,
        &survivors,
        "dd_rust_network_mutex_raft_client_batch_entries_total",
    )
    .await;
    let before_cache_hits = current_metric_sum_for_nodes(
        &cluster,
        &survivors,
        "dd_rust_network_mutex_raft_client_cache_completed_hits_total",
    )
    .await;

    let (status, duplicate) = http_post_json_retrying_unavailable(
        lb.port,
        "/v1/lock",
        acquire_body.clone(),
        "idempotent duplicate acquire after failover",
    )
    .await;
    assert_eq!(status, 200, "duplicate acquire response: {duplicate:?}");
    assert_eq!(
        duplicate["acquired"], true,
        "duplicate acquire should replay the original granted response: {duplicate:?}"
    );
    assert_eq!(
        duplicate["lockUuid"].as_str(),
        Some(lock_uuid.as_str()),
        "duplicate acquire should replay the original lock UUID: first={first:?} duplicate={duplicate:?}"
    );
    assert_eq!(
        current_metric_sum_for_nodes(
            &cluster,
            &survivors,
            "dd_rust_network_mutex_raft_client_proposals_total"
        )
        .await,
        before_proposals,
        "duplicate idempotent retry after failover must not enqueue a new proposal"
    );
    assert_eq!(
        current_metric_sum_for_nodes(
            &cluster,
            &survivors,
            "dd_rust_network_mutex_raft_client_batches_total"
        )
        .await,
        before_batches,
        "duplicate idempotent retry after failover must not submit a new batch"
    );
    assert_eq!(
        current_metric_sum_for_nodes(
            &cluster,
            &survivors,
            "dd_rust_network_mutex_raft_client_batch_entries_total"
        )
        .await,
        before_batch_entries,
        "duplicate idempotent retry after failover must not append another client entry"
    );
    assert!(
        current_metric_sum_for_nodes(
            &cluster,
            &survivors,
            "dd_rust_network_mutex_raft_client_cache_completed_hits_total"
        )
        .await
            > before_cache_hits,
        "duplicate idempotent retry after failover should be served from the completed response cache"
    );
    assert_full_log_metrics_unchanged(
        &cluster,
        &survivors,
        &before_full_log,
        "idempotent retry after leader failover",
    )
    .await;

    let before_conflicts = current_metric_sum_for_nodes(
        &cluster,
        &survivors,
        "dd_rust_network_mutex_raft_client_cache_conflicts_total",
    )
    .await;
    let before_conflict_proposals = current_metric_sum_for_nodes(
        &cluster,
        &survivors,
        "dd_rust_network_mutex_raft_client_proposals_total",
    )
    .await;
    let (status, conflict) = http_post_json_retrying_unavailable(
        lb.port,
        "/v1/lock",
        json!({
            "key": format!("{key}-different"),
            "ttlMs": 5000,
            "waitMs": 0,
            "requestId": acquire_body["requestId"].clone(),
        }),
        "conflicting idempotent acquire after failover",
    )
    .await;
    assert_eq!(
        status, 409,
        "conflicting idempotency payload should return conflict: {conflict:?}"
    );
    assert_eq!(
        current_metric_sum_for_nodes(
            &cluster,
            &survivors,
            "dd_rust_network_mutex_raft_client_proposals_total"
        )
        .await,
        before_conflict_proposals,
        "conflicting idempotency retry must be rejected before proposal"
    );
    assert!(
        current_metric_sum_for_nodes(
            &cluster,
            &survivors,
            "dd_rust_network_mutex_raft_client_cache_conflicts_total"
        )
        .await
            > before_conflicts,
        "conflicting idempotency retry should increment the conflict counter"
    );

    let (status, release) = http_post_json_retrying_unavailable(
        lb.port,
        "/v1/unlock",
        json!({
            "key": key,
            "lockUuid": lock_uuid,
            "requestId": format!("raft-http-idempotent-release-{}", uuid::Uuid::new_v4()),
        }),
        "release after idempotent failover retry",
    )
    .await;
    assert_eq!(status, 200, "release response: {release:?}");
    assert_eq!(release["unlocked"], true, "release response: {release:?}");
    wait_for_zero_holders_and_waiters_among(&cluster, &survivors).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_http_responses_carry_leader_role_headers() {
    // Every HTTP response stamps the node's Raft role and last-known leader so a
    // load balancer can learn the leader from any response (not just a probe).
    let _guard = RAFT_TEST_LOCK.lock().await;
    let cluster = start_cluster().await;
    let leader = wait_for_leader(&cluster).await;
    let leader_id = cluster.peers[leader].id.clone();
    let leader_status = http_get_json(cluster.http_ports[leader], "/raft/status")
        .await
        .expect("leader status");

    // The leader advertises itself as the ready leader.
    assert_eq!(
        http_response_header(cluster.http_ports[leader], "/raft/status", "x-raft-node-id").await,
        Some(leader_id.clone()),
    );
    assert_eq!(
        http_response_header(cluster.http_ports[leader], "/raft/status", "x-raft-role").await,
        Some("leader".to_string()),
    );
    assert_eq!(
        http_response_header(cluster.http_ports[leader], "/raft/status", "x-raft-term").await,
        leader_status["currentTerm"]
            .as_u64()
            .map(|value| value.to_string()),
    );
    assert_eq!(
        http_response_header(
            cluster.http_ports[leader],
            "/raft/status",
            "x-raft-commit-index"
        )
        .await,
        leader_status["commitIndex"]
            .as_u64()
            .map(|value| value.to_string()),
    );
    assert_eq!(
        http_response_header(
            cluster.http_ports[leader],
            "/raft/status",
            "x-raft-last-applied"
        )
        .await,
        leader_status["lastApplied"]
            .as_u64()
            .map(|value| value.to_string()),
    );
    assert_eq!(
        http_response_header(
            cluster.http_ports[leader],
            "/raft/status",
            "x-raft-last-log-index"
        )
        .await,
        leader_status["lastLogIndex"]
            .as_u64()
            .map(|value| value.to_string()),
    );
    assert_eq!(
        http_response_header(
            cluster.http_ports[leader],
            "/raft/status",
            "x-raft-last-log-term"
        )
        .await,
        leader_status["lastLogTerm"]
            .as_u64()
            .map(|value| value.to_string()),
    );
    assert_eq!(
        http_response_header(
            cluster.http_ports[leader],
            "/raft/status",
            "x-raft-leader-ready"
        )
        .await,
        Some("true".to_string()),
    );
    let leader_quorum_age_ms = http_response_header(
        cluster.http_ports[leader],
        "/raft/status",
        "x-raft-leader-quorum-age-ms",
    )
    .await
    .expect("leader quorum-age header")
    .parse::<u64>()
    .expect("leader quorum-age header should be numeric");
    let leader_quorum_timeout_ms = leader_status["leaderQuorumTimeoutMs"]
        .as_u64()
        .expect("leader quorum timeout in status");
    assert!(
        leader_quorum_age_ms <= leader_quorum_timeout_ms,
        "ready leader quorum age should be within timeout; age={leader_quorum_age_ms}, timeout={leader_quorum_timeout_ms}"
    );
    assert_eq!(
        http_response_header(
            cluster.http_ports[leader],
            "/raft/status",
            "x-raft-leader-quorum-timeout-ms"
        )
        .await,
        Some(leader_quorum_timeout_ms.to_string()),
    );
    assert_eq!(
        http_response_header(
            cluster.http_ports[leader],
            "/raft/status",
            "x-raft-membership-joint"
        )
        .await,
        leader_status["membershipJoint"]
            .as_bool()
            .map(|value| value.to_string()),
    );
    assert_eq!(
        http_response_header(
            cluster.http_ports[leader],
            "/raft/status",
            "x-raft-sync-log"
        )
        .await,
        leader_status["syncLog"]
            .as_bool()
            .map(|value| value.to_string()),
    );
    assert_eq!(
        http_response_header(
            cluster.http_ports[leader],
            "/raft/status",
            "x-raft-sync-commit"
        )
        .await,
        leader_status["syncCommit"]
            .as_bool()
            .map(|value| value.to_string()),
    );
    assert_eq!(
        http_response_header(
            cluster.http_ports[leader],
            "/raft/status",
            "x-raft-unsafe-durability"
        )
        .await,
        leader_status["unsafeDurability"]
            .as_bool()
            .map(|value| value.to_string()),
    );
    assert_eq!(
        http_response_header(
            cluster.http_ports[leader],
            "/raft/status",
            "x-raft-leader-id"
        )
        .await,
        Some(leader_id.clone()),
    );
    assert_eq!(
        http_response_header(
            cluster.http_ports[leader],
            "/raft/status",
            "x-raft-leader-addr"
        )
        .await,
        Some(cluster.peers[leader].addr.clone()),
    );

    // A follower reports the follower role but still points at the leader. Allow
    // a brief window for heartbeat-driven leader-hint propagation.
    let follower = (0..cluster.http_ports.len())
        .find(|idx| *idx != leader)
        .expect("a follower exists in a 3-node cluster");
    assert_eq!(
        http_response_header(
            cluster.http_ports[follower],
            "/raft/status",
            "x-raft-node-id"
        )
        .await,
        Some(cluster.peers[follower].id.clone()),
    );
    assert_eq!(
        http_response_header(cluster.http_ports[follower], "/raft/status", "x-raft-role").await,
        Some("follower".to_string()),
    );
    assert_eq!(
        http_response_header(
            cluster.http_ports[follower],
            "/raft/status",
            "x-raft-leader-ready"
        )
        .await,
        Some("false".to_string()),
    );
    assert_eq!(
        http_response_header(
            cluster.http_ports[follower],
            "/raft/status",
            "x-raft-leader-quorum-age-ms"
        )
        .await,
        None,
        "followers should not advertise local leader quorum freshness"
    );
    assert_eq!(
        http_response_header(
            cluster.http_ports[follower],
            "/raft/status",
            "x-raft-leader-quorum-timeout-ms"
        )
        .await,
        leader_status["leaderQuorumTimeoutMs"]
            .as_u64()
            .map(|value| value.to_string()),
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let hinted = http_response_header(
            cluster.http_ports[follower],
            "/raft/status",
            "x-raft-leader-id",
        )
        .await;
        if hinted.as_deref() == Some(leader_id.as_str()) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "follower should advertise the leader id via header; saw {hinted:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_http_acquire_release_echo_request_id_headers() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let cluster = start_cluster().await;
    let leader = wait_for_leader(&cluster).await;
    let port = cluster.http_ports[leader];
    let key = format!("raft-request-id-header-key-{}", uuid::Uuid::new_v4());
    let acquire_request_id = format!("raft-request-id-acquire-{}", uuid::Uuid::new_v4());

    let (status, headers, acquire) = http_post_json_with_headers(
        port,
        "/v1/lock",
        json!({
            "key": key.clone(),
            "ttlMs": 5000,
            "requestId": acquire_request_id,
        }),
    )
    .await;
    assert_eq!(status, 200, "acquire response: {acquire:?}");
    assert_eq!(acquire["acquired"], true, "acquire response: {acquire:?}");
    assert_eq!(
        headers.get("x-lmx-request-id"),
        Some(&acquire_request_id),
        "acquire response should echo supplied request id"
    );
    let lock_uuid = acquire["lockUuid"]
        .as_str()
        .expect("acquire lockUuid")
        .to_string();

    let generated_key = format!("raft-generated-request-id-key-{}", uuid::Uuid::new_v4());
    let (status, generated_headers, generated) = http_post_json_with_headers(
        port,
        "/v1/lock",
        json!({"key": generated_key.clone(), "ttlMs": 5000}),
    )
    .await;
    assert_eq!(status, 200, "generated-id acquire response: {generated:?}");
    assert_eq!(
        generated["acquired"], true,
        "generated-id acquire response: {generated:?}"
    );
    let generated_request_id = generated_headers
        .get("x-lmx-request-id")
        .expect("generated-id acquire should expose generated request id");
    uuid::Uuid::parse_str(generated_request_id)
        .expect("generated request id header should be a UUID");
    let generated_lock_uuid = generated["lockUuid"]
        .as_str()
        .expect("generated acquire lockUuid")
        .to_string();

    let release_request_id = format!("raft-request-id-release-{}", uuid::Uuid::new_v4());
    let (status, release_headers, release) = http_post_json_with_headers(
        port,
        "/v1/unlock",
        json!({
            "key": key,
            "lockUuid": lock_uuid,
            "requestId": release_request_id,
        }),
    )
    .await;
    assert_eq!(status, 200, "release response: {release:?}");
    assert_eq!(release["unlocked"], true, "release response: {release:?}");
    assert_eq!(
        release_headers.get("x-lmx-request-id"),
        Some(&release_request_id),
        "release response should echo supplied request id"
    );

    let (status, generated_release_headers, generated_release) = http_post_json_with_headers(
        port,
        "/v1/unlock",
        json!({"key": generated_key, "lockUuid": generated_lock_uuid}),
    )
    .await;
    assert_eq!(
        status, 200,
        "generated-id cleanup release response: {generated_release:?}"
    );
    assert_eq!(
        generated_release["unlocked"], true,
        "generated-id cleanup release response: {generated_release:?}"
    );
    let generated_release_request_id = generated_release_headers
        .get("x-lmx-request-id")
        .expect("generated-id release should expose generated request id");
    uuid::Uuid::parse_str(generated_release_request_id)
        .expect("generated release request id header should be a UUID");
    wait_for_zero_holders_and_waiters(&cluster).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_follower_proxy_survives_leaderless_failover_window() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let tuning = RaftClusterTuning {
        election_timeout_min: Duration::from_millis(1_200),
        election_timeout_max: Duration::from_millis(1_600),
        proxy_retry_budget: Duration::from_secs(5),
        ..RaftClusterTuning::default()
    };
    let mut cluster = start_cluster_with_tuning(3, 3, tuning).await;
    let old_leader = wait_for_leader(&cluster).await;
    let target_follower = (old_leader + 1) % 3;
    let key = format!("raft-proxy-failover-key-{}", uuid::Uuid::new_v4());

    let (status, acquire) = http_post_json(
        cluster.http_ports[old_leader],
        "/v1/lock",
        json!({"key": key, "ttlMs": 5000}),
    )
    .await;
    assert_eq!(status, 200, "acquire response: {acquire:?}");
    assert_eq!(acquire["acquired"], true, "acquire response: {acquire:?}");
    let lock_uuid = acquire["lockUuid"].as_str().unwrap().to_string();
    let acquired_status =
        wait_for_status_index_at_least(cluster.http_ports[old_leader], "lastApplied", 1).await;
    let acquired_index = acquired_status["lastApplied"]
        .as_u64()
        .expect("old leader lastApplied after acquire");
    let all_nodes = vec![0, 1, 2];
    wait_for_status_indexes_at_least_among(&cluster, &all_nodes, acquired_index, acquired_index)
        .await;

    let survivors: Vec<usize> = (0..cluster.http_ports.len())
        .filter(|idx| *idx != old_leader)
        .collect();
    let before_full_log = current_full_log_metrics_for_nodes(&cluster, &survivors).await;
    cluster.abort_node(old_leader).await;
    let target_port = cluster.http_ports[target_follower];
    let (status, release) = http_post_json(
        target_port,
        "/v1/unlock",
        json!({"key": key, "lockUuid": lock_uuid}),
    )
    .await;
    assert_eq!(
        status, 200,
        "follower proxy should retry through leader churn and release: {release:?}"
    );
    assert_eq!(
        release["unlocked"], true,
        "follower proxy should release after failover: {release:?}"
    );
    let forwarded = current_metric(
        target_port,
        "dd_rust_network_mutex_raft_proxy_requests_forwarded_total",
    )
    .await;
    let retries = current_metric(
        target_port,
        "dd_rust_network_mutex_raft_proxy_request_retries_total",
    )
    .await;
    let target_status = http_get_json(target_port, "/raft/status")
        .await
        .expect("target status after failover release");
    assert!(
        forwarded >= 1 || target_status["isLeader"] == true,
        "failover release should either be proxied by the target follower or handled after it became leader; forwarded={forwarded}, retries={retries}, status={target_status:?}, release={release:?}"
    );

    let new_leader = wait_for_leader_among(&cluster, &survivors).await;
    assert_ne!(new_leader, old_leader);
    let release_status = wait_for_status_index_at_least(
        cluster.http_ports[new_leader],
        "lastApplied",
        acquired_index.saturating_add(1),
    )
    .await;
    let release_index = release_status["lastApplied"]
        .as_u64()
        .expect("new leader lastApplied after release");
    wait_for_status_indexes_at_least_among(&cluster, &survivors, release_index, release_index)
        .await;
    wait_for_zero_holders_and_waiters_among(&cluster, &survivors).await;
    assert_full_log_metrics_unchanged(
        &cluster,
        &survivors,
        &before_full_log,
        "leaderless follower-proxy failover release",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_membership_promotes_new_voters_and_survives_old_majority_loss() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let mut cluster = start_cluster_with_nodes(5, 3).await;
    let initial_nodes = vec![0, 1, 2];
    let old_leader = wait_for_leader_among(&cluster, &initial_nodes).await;
    let key = format!("raft-membership-key-{}", uuid::Uuid::new_v4());

    let (status, acquire) = http_post_json(
        cluster.http_ports[old_leader],
        "/v1/lock",
        json!({"key": key, "ttlMs": 5000}),
    )
    .await;
    assert_eq!(status, 200, "pre-membership acquire response: {acquire:?}");
    assert_eq!(
        acquire["acquired"], true,
        "pre-membership acquire response: {acquire:?}"
    );
    let lock_uuid = acquire["lockUuid"].as_str().unwrap().to_string();

    let (status, membership) = http_post_json(
        cluster.http_ports[old_leader],
        "/raft/membership",
        json!({"peers": cluster.peers.clone()}),
    )
    .await;
    assert_eq!(status, 200, "membership response: {membership:?}");
    assert_eq!(membership["clusterSize"].as_u64(), Some(5));
    assert_eq!(membership["quorumSize"].as_u64(), Some(3));
    let expected_ids = cluster
        .peers
        .iter()
        .map(|peer| peer.id.clone())
        .collect::<BTreeSet<_>>();
    let all_nodes = vec![0, 1, 2, 3, 4];
    wait_for_simple_membership(&cluster, &all_nodes, &expected_ids, 5, 3).await;

    let second_old_voter = initial_nodes
        .iter()
        .copied()
        .find(|idx| *idx != old_leader)
        .unwrap();
    let survivors = (0..cluster.http_ports.len())
        .filter(|idx| *idx != old_leader && *idx != second_old_voter)
        .collect::<Vec<_>>();
    assert_eq!(
        survivors.len(),
        3,
        "test should leave exactly a new 5-node quorum online"
    );
    let shrink_target = survivors
        .iter()
        .map(|idx| cluster.peers[*idx].clone())
        .collect::<Vec<_>>();
    let (status, removal) = http_post_json(
        cluster.http_ports[old_leader],
        "/raft/membership",
        json!({"peers": shrink_target}),
    )
    .await;
    assert_eq!(
        status, 200,
        "membership removal response should commit even when it removes the current leader: {removal:?}"
    );
    assert_eq!(removal["clusterSize"].as_u64(), Some(3));
    assert_eq!(removal["quorumSize"].as_u64(), Some(2));
    let expected_survivor_ids = survivors
        .iter()
        .map(|idx| cluster.peers[*idx].id.clone())
        .collect::<BTreeSet<_>>();
    wait_for_simple_membership(&cluster, &survivors, &expected_survivor_ids, 3, 2).await;
    for removed in [old_leader, second_old_voter] {
        let status = http_get_json(cluster.http_ports[removed], "/raft/status")
            .await
            .unwrap_or_else(|| panic!("removed node {removed} should still serve status"));
        assert_eq!(
            status["isLeader"], false,
            "removed node must step down: {status:?}"
        );
        assert_eq!(
            membership_target_peer_ids(&status["membership"]),
            Some(expected_survivor_ids.clone()),
            "removed node should observe a survivor-only target membership even if it is still in the old side of joint consensus: {status:?}"
        );
        let removed_key = format!(
            "raft-removed-node-write-{}-{}",
            removed,
            uuid::Uuid::new_v4()
        );
        let (status, rejected) = http_post_json(
            cluster.http_ports[removed],
            "/v1/lock",
            json!({"key": removed_key, "ttlMs": 5000, "waitMs": 0}),
        )
        .await;
        assert_eq!(
            status, 503,
            "removed node must not serve direct Raft writes after applying final membership: {rejected:?}"
        );
        assert!(
            rejected["error"].as_str().is_some(),
            "removed node rejection should include an error body: {rejected:?}"
        );
    }

    cluster.abort_node(old_leader).await;
    cluster.abort_node(second_old_voter).await;
    let new_leader = wait_for_leader_among(&cluster, &survivors).await;
    let before_full_log = current_full_log_metrics_for_nodes(&cluster, &survivors).await;
    let baseline_status = http_get_json(cluster.http_ports[new_leader], "/raft/status")
        .await
        .expect("survivor leader status before membership failover traffic");
    let baseline_commit_index = baseline_status["commitIndex"]
        .as_u64()
        .expect("survivor leader commitIndex before membership failover traffic");
    let survivor_ports = survivors
        .iter()
        .map(|idx| cluster.http_ports[*idx])
        .collect::<Vec<_>>();
    let lb = start_round_robin_lb(survivor_ports).await;

    let (status, release) = http_post_json(
        lb.port,
        "/v1/unlock",
        json!({"key": key, "lockUuid": lock_uuid}),
    )
    .await;
    assert_eq!(
        status, 200,
        "release after membership failover response: {release:?}"
    );
    assert_eq!(
        release["unlocked"], true,
        "release after membership failover response: {release:?}"
    );
    wait_for_zero_holders_and_waiters_among(&cluster, &survivors).await;

    let (status, reacquire) =
        http_post_json(lb.port, "/v1/lock", json!({"key": key, "ttlMs": 5000})).await;
    assert_eq!(
        status, 200,
        "reacquire after membership failover response: {reacquire:?}"
    );
    assert_eq!(
        reacquire["acquired"], true,
        "reacquire after membership failover response: {reacquire:?}"
    );
    let leader_after_reacquire = wait_for_leader_among(&cluster, &survivors).await;
    let status = http_get_json(cluster.http_ports[leader_after_reacquire], "/raft/status")
        .await
        .expect("leader status after membership failover reacquire");
    let commit_index = status["commitIndex"]
        .as_u64()
        .expect("leader commitIndex after membership failover reacquire");
    assert!(
        commit_index >= baseline_commit_index.saturating_add(2),
        "membership failover should commit the release and reacquire after survivor-only baseline; baseline={baseline_commit_index} status={status:?}"
    );
    wait_for_status_indexes_at_least_among(&cluster, &survivors, commit_index, commit_index).await;
    assert_full_log_metrics_unchanged(
        &cluster,
        &survivors,
        &before_full_log,
        "post-membership old-majority-loss failover traffic",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_membership_grows_and_shrinks_through_even_size() {
    // Even-sized clusters are valid (quorum is always a strict majority), so a
    // cluster can pass through a 4-node intermediate in both directions:
    //   grow:   3 -> 4 -> 5   (promote staged learners one at a time)
    //   shrink: 5 -> 4 -> 3   (remove non-leaders one at a time)
    // A write must commit at every stable config, and clusterSize/quorumSize
    // must reflect the strict-majority rule (4 -> quorum 3).
    let _guard = RAFT_TEST_LOCK.lock().await;
    // 3 initial voters {0,1,2}; nodes 3 and 4 start as staged learners.
    let cluster = start_cluster_with_nodes(5, 3).await;

    let peers_for = |idxs: &[usize]| {
        idxs.iter()
            .map(|i| cluster.peers[*i].clone())
            .collect::<Vec<_>>()
    };
    let ids_for = |idxs: &[usize]| {
        idxs.iter()
            .map(|i| cluster.peers[*i].id.clone())
            .collect::<BTreeSet<_>>()
    };

    // Post a membership change to whichever node currently leads the given set,
    // and assert the committed cluster/quorum sizes. A membership change right
    // after another can transiently race leader readiness (503 NotLeader), so
    // re-discover the leader and retry for a bounded window, as operator tooling
    // would.
    async fn change_to(
        cluster: &RaftCluster,
        current_nodes: &[usize],
        target: &[RaftPeerConfig],
        expected_size: u64,
        expected_quorum: u64,
        label: &str,
    ) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let leader = wait_for_leader_among(cluster, current_nodes).await;
            let (status, membership) = http_post_json(
                cluster.http_ports[leader],
                "/raft/membership",
                json!({ "peers": target }),
            )
            .await;
            if status == 200 {
                assert_eq!(
                    membership["clusterSize"].as_u64(),
                    Some(expected_size),
                    "{label} clusterSize: {membership:?}"
                );
                assert_eq!(
                    membership["quorumSize"].as_u64(),
                    Some(expected_quorum),
                    "{label} quorumSize: {membership:?}"
                );
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{label} membership change never committed; last response {status}: {membership:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn write_succeeds(cluster: &RaftCluster, nodes: &[usize], key: &str) {
        let leader = wait_for_leader_among(cluster, nodes).await;
        let (status, acquire) = http_post_json(
            cluster.http_ports[leader],
            "/v1/lock",
            json!({ "key": key, "ttlMs": 5000 }),
        )
        .await;
        assert_eq!(status, 200, "write at {} nodes: {acquire:?}", nodes.len());
        assert_eq!(
            acquire["acquired"],
            true,
            "write at {} nodes must commit: {acquire:?}",
            nodes.len()
        );
        let lock_uuid = acquire["lockUuid"].as_str().unwrap().to_string();
        let leader = wait_for_leader_among(cluster, nodes).await;
        let (status, release) = http_post_json(
            cluster.http_ports[leader],
            "/v1/unlock",
            json!({ "key": key, "lockUuid": lock_uuid }),
        )
        .await;
        assert_eq!(status, 200, "release at {} nodes: {release:?}", nodes.len());
        assert_eq!(release["unlocked"], true, "release: {release:?}");
    }

    let v3: Vec<usize> = vec![0, 1, 2];
    let v4: Vec<usize> = vec![0, 1, 2, 3];
    let v5: Vec<usize> = vec![0, 1, 2, 3, 4];

    // Baseline: the 3-voter cluster elects a leader and commits a write.
    wait_for_leader_among(&cluster, &v3).await;
    write_succeeds(&cluster, &v3, "even-grow-at3").await;

    // GROW 3 -> 4 -> 5, promoting one staged learner per step.
    change_to(&cluster, &v3, &peers_for(&v4), 4, 3, "3->4").await;
    wait_for_simple_membership(&cluster, &v4, &ids_for(&v4), 4, 3).await;
    write_succeeds(&cluster, &v4, "even-grow-at4").await;

    change_to(&cluster, &v4, &peers_for(&v5), 5, 3, "4->5").await;
    wait_for_simple_membership(&cluster, &v5, &ids_for(&v5), 5, 3).await;
    write_succeeds(&cluster, &v5, "even-grow-at5").await;

    // SHRINK 5 -> 4 -> 3, removing non-leaders so leadership stays put.
    let leader = wait_for_leader_among(&cluster, &v5).await;
    let non_leaders: Vec<usize> = v5.iter().copied().filter(|i| *i != leader).collect();
    let victim_a = non_leaders[0];
    let victim_b = non_leaders[1];
    let s4: Vec<usize> = v5.iter().copied().filter(|i| *i != victim_a).collect();
    let s3: Vec<usize> = s4.iter().copied().filter(|i| *i != victim_b).collect();

    change_to(&cluster, &v5, &peers_for(&s4), 4, 3, "5->4").await;
    wait_for_simple_membership(&cluster, &s4, &ids_for(&s4), 4, 3).await;
    write_succeeds(&cluster, &s4, "even-shrink-at4").await;

    change_to(&cluster, &s4, &peers_for(&s3), 3, 2, "4->3").await;
    wait_for_simple_membership(&cluster, &s3, &ids_for(&s3), 3, 2).await;
    write_succeeds(&cluster, &s3, "even-shrink-at3").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_lb_membership_churn_no_wait_history_is_linearizable() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let cluster = start_cluster_with_nodes(5, 3).await;
    let history = Arc::new(Mutex::new(Vec::<HttpLockHistoryOp>::new()));
    let order = Arc::new(AtomicUsize::new(0));
    let v3 = vec![0, 1, 2];
    let v4 = vec![0, 1, 2, 3];
    let v5 = vec![0, 1, 2, 3, 4];

    wait_for_leader_among(&cluster, &v3).await;
    run_lb_membership_churn_history_step(LbMembershipChurnHistoryStep {
        cluster: &cluster,
        active_nodes: &v3,
        target_nodes: &v4,
        expected_size: 4,
        expected_quorum: 3,
        phase: 0,
        history: Arc::clone(&history),
        order: Arc::clone(&order),
        label: "grow-3-to-4",
    })
    .await;

    run_lb_membership_churn_history_step(LbMembershipChurnHistoryStep {
        cluster: &cluster,
        active_nodes: &v4,
        target_nodes: &v5,
        expected_size: 5,
        expected_quorum: 3,
        phase: 1,
        history: Arc::clone(&history),
        order: Arc::clone(&order),
        label: "grow-4-to-5",
    })
    .await;

    let leader = wait_for_leader_among(&cluster, &v5).await;
    let shrink_victims = v5
        .iter()
        .copied()
        .filter(|idx| *idx != leader)
        .take(2)
        .collect::<Vec<_>>();
    assert_eq!(
        shrink_victims.len(),
        2,
        "5-node cluster should have two non-leaders available to remove"
    );
    let s4 = v5
        .iter()
        .copied()
        .filter(|idx| *idx != shrink_victims[0])
        .collect::<Vec<_>>();
    let s3 = s4
        .iter()
        .copied()
        .filter(|idx| *idx != shrink_victims[1])
        .collect::<Vec<_>>();

    run_lb_membership_churn_history_step(LbMembershipChurnHistoryStep {
        cluster: &cluster,
        active_nodes: &v5,
        target_nodes: &s4,
        expected_size: 4,
        expected_quorum: 3,
        phase: 2,
        history: Arc::clone(&history),
        order: Arc::clone(&order),
        label: "shrink-5-to-4",
    })
    .await;

    run_lb_membership_churn_history_step(LbMembershipChurnHistoryStep {
        cluster: &cluster,
        active_nodes: &s4,
        target_nodes: &s3,
        expected_size: 3,
        expected_quorum: 2,
        phase: 3,
        history: Arc::clone(&history),
        order: Arc::clone(&order),
        label: "shrink-4-to-3",
    })
    .await;

    let history = history.lock().expect("history").clone();
    assert!(
        history
            .iter()
            .any(|op| matches!(op.result, HttpLockHistoryResult::Acquired { .. })),
        "membership churn HTTP history should record successful acquires"
    );
    assert!(
        history
            .iter()
            .any(|op| matches!(op.result, HttpLockHistoryResult::NotAcquired)),
        "membership churn HTTP history should record contended no-wait acquires"
    );
    assert_http_lock_history_linearizable(&history);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_staged_learner_catches_up_with_install_snapshot_after_compaction() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let tuning = RaftClusterTuning {
        snapshot_max_log_entries: 3,
        snapshot_max_log_bytes: u64::MAX,
        trailing_log_entries: 0,
        append_entries_max_entries: 2,
        append_entries_max_bytes: usize::MAX,
        install_snapshot_chunk_bytes: 128,
        ..RaftClusterTuning::default()
    };
    let cluster = start_cluster_with_tuning(4, 3, tuning).await;
    let leader = wait_for_leader_among(&cluster, &[0, 1, 2]).await;
    let learner = cluster.peers[3].clone();

    for step in 0..8 {
        let key = format!("raft-snapshot-learner-{step}-{}", uuid::Uuid::new_v4());
        let (status, acquire) = http_post_json(
            cluster.http_ports[leader],
            "/v1/lock",
            json!({"key": key, "ttlMs": 5000}),
        )
        .await;
        assert_eq!(
            status, 200,
            "snapshot learner acquire response: {acquire:?}"
        );
        assert_eq!(
            acquire["acquired"], true,
            "snapshot learner acquire response: {acquire:?}"
        );
        let lock_uuid = acquire["lockUuid"].as_str().unwrap().to_string();
        let (status, release) = http_post_json(
            cluster.http_ports[leader],
            "/v1/unlock",
            json!({"key": key, "lockUuid": lock_uuid}),
        )
        .await;
        assert_eq!(
            status, 200,
            "snapshot learner release response: {release:?}"
        );
        assert_eq!(
            release["unlocked"], true,
            "snapshot learner release response: {release:?}"
        );
    }

    let leader_status = http_get_json(cluster.http_ports[leader], "/raft/status")
        .await
        .expect("leader status after compaction writes");
    let pre_stage_commit = leader_status["commitIndex"]
        .as_u64()
        .expect("leader commit index");
    let leader_snapshot_index = wait_for_metric_at_least(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_latest_snapshot_index",
        1,
    )
    .await;
    wait_for_metric_at_least(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_log_compactions_total",
        1,
    )
    .await;
    assert!(
        leader_snapshot_index < pre_stage_commit,
        "test should leave a retained suffix after the compacted prefix; snapshot={leader_snapshot_index} commit={pre_stage_commit}"
    );
    let learner_status = http_get_json(cluster.http_ports[3], "/raft/status")
        .await
        .expect("bootstrap learner status before staging");
    assert_eq!(
        learner_status["lastLogIndex"].as_u64(),
        Some(0),
        "bootstrap learner should start without the leader's compacted log: {learner_status:?}"
    );
    let before_leader_batches = current_metric(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_append_entries_batches_total",
    )
    .await;
    let before_leader_sent = current_metric(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_append_entries_sent_total",
    )
    .await;
    let before_learner_appended = current_metric(
        cluster.http_ports[3],
        "dd_rust_network_mutex_raft_follower_append_appended_entries_total",
    )
    .await;
    let before_leader_compactions = current_metric(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_log_compactions_total",
    )
    .await;
    let before_leader_full_log = current_full_log_metrics(cluster.http_ports[leader]).await;
    let before_learner_full_log = current_full_log_metrics(cluster.http_ports[3]).await;

    let (status, staged) = http_post_json(
        cluster.http_ports[leader],
        "/raft/learners",
        json!({"peers": [learner]}),
    )
    .await;
    assert_eq!(status, 200, "stage learner response: {staged:?}");
    assert_eq!(
        staged["learners"].as_array().map(Vec::len),
        Some(1),
        "stage learner response should list the caught-up learner: {staged:?}"
    );
    let stage_target = staged["progress"]["lastLogIndex"]
        .as_u64()
        .expect("leader last log index after learner staging");

    wait_for_metric_at_least(
        cluster.http_ports[3],
        "dd_rust_network_mutex_raft_install_snapshot_staged_chunks_total",
        1,
    )
    .await;
    wait_for_metric_at_least(
        cluster.http_ports[3],
        "dd_rust_network_mutex_raft_latest_snapshot_index",
        leader_snapshot_index,
    )
    .await;
    wait_for_status_index_at_least(cluster.http_ports[3], "lastApplied", stage_target).await;
    let after_leader_batches = current_metric(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_append_entries_batches_total",
    )
    .await;
    let after_leader_sent = current_metric(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_append_entries_sent_total",
    )
    .await;
    let after_learner_appended = current_metric(
        cluster.http_ports[3],
        "dd_rust_network_mutex_raft_follower_append_appended_entries_total",
    )
    .await;
    let after_leader_compactions = current_metric(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_log_compactions_total",
    )
    .await;
    let after_leader_full_log = current_full_log_metrics(cluster.http_ports[leader]).await;
    let after_learner_full_log = current_full_log_metrics(cluster.http_ports[3]).await;
    let learner_snapshot_index = current_metric(
        cluster.http_ports[3],
        "dd_rust_network_mutex_raft_latest_snapshot_index",
    )
    .await;
    assert_eq!(
        after_leader_full_log.reads_total, before_leader_full_log.reads_total,
        "snapshot learner catch-up should not make the leader perform a full retained-log scan"
    );
    let leader_full_rewrite_delta = after_leader_full_log
        .rewrites_total
        .saturating_sub(before_leader_full_log.rewrites_total);
    let leader_compaction_delta =
        after_leader_compactions.saturating_sub(before_leader_compactions);
    assert!(
        leader_full_rewrite_delta <= leader_compaction_delta,
        "snapshot learner catch-up should not make the leader rewrite the retained log except for real compaction; full_rewrite_delta={leader_full_rewrite_delta} compaction_delta={leader_compaction_delta}"
    );
    assert_eq!(
        after_learner_full_log, before_learner_full_log,
        "empty learner catch-up should install the snapshot and append the retained suffix without full-log scan/rewrite"
    );
    let appended_delta = after_learner_appended.saturating_sub(before_learner_appended);
    let sent_delta = after_leader_sent.saturating_sub(before_leader_sent);
    if learner_snapshot_index < stage_target {
        let retained_suffix_entries = stage_target.saturating_sub(learner_snapshot_index);
        let expected_suffix_batches = retained_suffix_entries.saturating_add(1) / 2;
        assert!(
            appended_delta >= retained_suffix_entries,
            "learner should append retained suffix entries after installing snapshot; before={before_learner_appended} after={after_learner_appended} suffix={retained_suffix_entries} snapshot={learner_snapshot_index} target={stage_target}"
        );
        assert!(
            after_leader_batches.saturating_sub(before_leader_batches) >= expected_suffix_batches,
            "snapshot catch-up should continue with bounded AppendEntries batches for the retained suffix; before={before_leader_batches} after={after_leader_batches} expected_at_least={expected_suffix_batches}"
        );
        assert!(
            sent_delta >= retained_suffix_entries,
            "leader should send retained suffix entries after InstallSnapshot; before={before_leader_sent} after={after_leader_sent} suffix={retained_suffix_entries}"
        );
    } else {
        assert_eq!(
            appended_delta, 0,
            "when the installed snapshot already covers the staging target, learner should not need retained suffix appends"
        );
    }

    let progress = http_get_json(cluster.http_ports[leader], "/raft/progress")
        .await
        .expect("leader progress after learner catch-up");
    assert!(
        progress["peers"].as_array().unwrap().iter().any(|peer| {
            peer["id"].as_str() == Some("node-4")
                && peer["stagedLearner"] == true
                && peer["caughtUp"] == true
        }),
        "leader progress should show node-4 as a caught-up staged learner: {progress:?}"
    );

    let (status, membership) = http_post_json(
        cluster.http_ports[leader],
        "/raft/membership",
        json!({"peers": cluster.peers.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "snapshot-caught learner should promote through joint consensus: {membership:?}"
    );
    assert_eq!(membership["clusterSize"].as_u64(), Some(4));
    assert_eq!(membership["quorumSize"].as_u64(), Some(3));
    let all_nodes = vec![0, 1, 2, 3];
    let expected_ids = cluster
        .peers
        .iter()
        .map(|peer| peer.id.clone())
        .collect::<BTreeSet<_>>();
    wait_for_simple_membership(&cluster, &all_nodes, &expected_ids, 4, 3).await;

    let promoted_key = format!("raft-snapshot-promoted-{}", uuid::Uuid::new_v4());
    let (status, acquire) = http_post_json(
        cluster.http_ports[3],
        "/v1/lock",
        json!({"key": promoted_key, "ttlMs": 5000}),
    )
    .await;
    assert_eq!(
        status, 200,
        "snapshot-promoted voter should accept a proxied or local write: {acquire:?}"
    );
    assert_eq!(
        acquire["acquired"], true,
        "snapshot-promoted voter write should commit: {acquire:?}"
    );
    let promoted_lock_uuid = acquire["lockUuid"].as_str().unwrap().to_string();
    let (status, release) = http_post_json(
        cluster.http_ports[3],
        "/v1/unlock",
        json!({"key": promoted_key, "lockUuid": promoted_lock_uuid}),
    )
    .await;
    assert_eq!(
        status, 200,
        "snapshot-promoted voter should release through the Raft quorum: {release:?}"
    );
    assert_eq!(
        release["unlocked"], true,
        "snapshot-promoted voter release should commit: {release:?}"
    );
    wait_for_zero_holders_and_waiters_among(&cluster, &all_nodes).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_staged_learner_catches_up_over_bounded_append_entries_without_snapshot() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let tuning = RaftClusterTuning {
        snapshot_max_log_entries: 100_000,
        snapshot_max_log_bytes: u64::MAX,
        append_entries_max_entries: 2,
        append_entries_max_bytes: usize::MAX,
        ..RaftClusterTuning::default()
    };
    let cluster = start_cluster_with_tuning(4, 3, tuning).await;
    let leader = wait_for_leader_among(&cluster, &[0, 1, 2]).await;
    let learner = cluster.peers[3].clone();

    for step in 0..5 {
        let key = format!("raft-append-learner-{step}-{}", uuid::Uuid::new_v4());
        let (status, acquire) = http_post_json(
            cluster.http_ports[leader],
            "/v1/lock",
            json!({"key": key, "ttlMs": 5000}),
        )
        .await;
        assert_eq!(status, 200, "append learner acquire response: {acquire:?}");
        assert_eq!(
            acquire["acquired"], true,
            "append learner acquire response: {acquire:?}"
        );
        let lock_uuid = acquire["lockUuid"].as_str().unwrap().to_string();
        let (status, release) = http_post_json(
            cluster.http_ports[leader],
            "/v1/unlock",
            json!({"key": key, "lockUuid": lock_uuid}),
        )
        .await;
        assert_eq!(status, 200, "append learner release response: {release:?}");
        assert_eq!(
            release["unlocked"], true,
            "append learner release response: {release:?}"
        );
    }

    let leader_status = http_get_json(cluster.http_ports[leader], "/raft/status")
        .await
        .expect("leader status before append-only learner staging");
    let pre_stage_commit = leader_status["commitIndex"]
        .as_u64()
        .expect("leader commit index");
    assert!(
        pre_stage_commit >= 10,
        "test should build a multi-batch retained suffix before staging learner: {leader_status:?}"
    );
    assert_eq!(
        current_metric(
            cluster.http_ports[leader],
            "dd_rust_network_mutex_raft_latest_snapshot_index"
        )
        .await,
        0,
        "append-only catch-up test should not compact before staging"
    );
    let learner_status = http_get_json(cluster.http_ports[3], "/raft/status")
        .await
        .expect("bootstrap learner status before append-only staging");
    assert_eq!(
        learner_status["lastLogIndex"].as_u64(),
        Some(0),
        "bootstrap learner should start empty before append-only catch-up: {learner_status:?}"
    );

    let before_batches = current_metric(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_append_entries_batches_total",
    )
    .await;
    let before_sent = current_metric(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_append_entries_sent_total",
    )
    .await;
    let before_sent_bytes = current_metric(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_append_entries_log_bytes_total",
    )
    .await;
    let before_fallbacks = current_metric(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_append_snapshot_fallbacks_total",
    )
    .await;
    let before_leader_full_log = current_full_log_metrics(cluster.http_ports[leader]).await;
    let before_learner_full_log = current_full_log_metrics(cluster.http_ports[3]).await;

    let (status, staged) = http_post_json(
        cluster.http_ports[leader],
        "/raft/learners",
        json!({"peers": [learner]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "append-only stage learner response: {staged:?}"
    );
    let stage_target = staged["progress"]["lastLogIndex"]
        .as_u64()
        .expect("leader last log index after append-only learner staging");
    wait_for_status_index_at_least(cluster.http_ports[3], "lastApplied", stage_target).await;

    let after_batches = current_metric(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_append_entries_batches_total",
    )
    .await;
    let after_sent = current_metric(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_append_entries_sent_total",
    )
    .await;
    let after_sent_bytes = current_metric(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_append_entries_log_bytes_total",
    )
    .await;
    let after_fallbacks = current_metric(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_append_snapshot_fallbacks_total",
    )
    .await;
    let after_leader_full_log = current_full_log_metrics(cluster.http_ports[leader]).await;
    let after_learner_full_log = current_full_log_metrics(cluster.http_ports[3]).await;
    assert_eq!(
        after_fallbacks, before_fallbacks,
        "append-only learner catch-up should not fall back to InstallSnapshot"
    );
    assert_eq!(
        after_leader_full_log, before_leader_full_log,
        "append-only learner catch-up should not make the leader scan or rewrite the retained log"
    );
    assert_eq!(
        after_learner_full_log, before_learner_full_log,
        "append-only learner catch-up should append bounded suffixes without full-log scan/rewrite"
    );
    assert_eq!(
        current_metric(
            cluster.http_ports[3],
            "dd_rust_network_mutex_raft_install_snapshot_staged_chunks_total"
        )
        .await,
        0,
        "append-only learner should not stage snapshot chunks"
    );
    assert_eq!(
        current_metric(
            cluster.http_ports[3],
            "dd_rust_network_mutex_raft_latest_snapshot_index"
        )
        .await,
        0,
        "append-only learner should not install a snapshot"
    );
    let minimum_catchup_batches = stage_target.div_ceil(2);
    assert!(
        after_batches.saturating_sub(before_batches) >= minimum_catchup_batches,
        "bounded catch-up should require multiple AppendEntries batches; before={before_batches} after={after_batches} target={stage_target}"
    );
    assert!(
        after_sent.saturating_sub(before_sent) >= stage_target,
        "learner catch-up should send the retained log entries rather than one full-log rewrite; before={before_sent} after={after_sent} target={stage_target}"
    );
    let sent_delta = after_sent.saturating_sub(before_sent);
    let sent_bytes_delta = after_sent_bytes.saturating_sub(before_sent_bytes);
    let leader_log_bytes = current_metric(
        cluster.http_ports[leader],
        "dd_rust_network_mutex_raft_log_bytes",
    )
    .await;
    assert!(
        sent_delta <= stage_target.saturating_mul(3),
        "learner catch-up should not repeatedly send the retained history; before={before_sent} after={after_sent} target={stage_target}"
    );
    assert!(
        sent_bytes_delta <= leader_log_bytes.saturating_mul(3),
        "learner catch-up should keep AppendEntries bytes proportional to retained log size, not retained history times catch-up batches; before_bytes={before_sent_bytes} after_bytes={after_sent_bytes} leader_log_bytes={leader_log_bytes} target={stage_target}"
    );

    let progress = http_get_json(cluster.http_ports[leader], "/raft/progress")
        .await
        .expect("leader progress after append-only learner catch-up");
    assert!(
        progress["peers"].as_array().unwrap().iter().any(|peer| {
            peer["id"].as_str() == Some("node-4")
                && peer["stagedLearner"] == true
                && peer["caughtUp"] == true
                && peer["matchIndex"]
                    .as_u64()
                    .is_some_and(|idx| idx >= stage_target)
        }),
        "leader progress should show node-4 caught up through bounded AppendEntries: {progress:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broker_and_raft_http_contract_match() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let broker = start_regular_http_server().await;
    assert_http_lock_contract(broker.port, "broker").await;

    let cluster = start_cluster().await;
    let _leader = wait_for_leader(&cluster).await;
    let lb = start_round_robin_lb(cluster.http_ports.clone()).await;
    let all_nodes: Vec<usize> = (0..cluster.http_ports.len()).collect();
    let before_full_log = current_full_log_metrics_for_nodes(&cluster, &all_nodes).await;
    assert_http_lock_contract(lb.port, "raft").await;
    wait_for_zero_holders_and_waiters(&cluster).await;
    assert_full_log_metrics_unchanged(
        &cluster,
        &all_nodes,
        &before_full_log,
        "BrokerRaft public HTTP contract including composite union keys",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raft_lb_seeded_lock_model_fuzz() {
    let _guard = RAFT_TEST_LOCK.lock().await;
    let cluster = start_cluster().await;
    let _leader = wait_for_leader(&cluster).await;
    let lb = start_round_robin_lb(cluster.http_ports.clone()).await;
    run_seeded_http_lock_model(lb.port, 0xB10C_AADE_5EED, 180).await;
    wait_for_zero_holders_and_waiters(&cluster).await;
}

async fn assert_http_lock_contract(port: u16, label: &str) {
    let key = format!("{label}-contract-{}", uuid::Uuid::new_v4());
    let (status, invalid) = http_post_json(
        port,
        "/v1/lock",
        json!({"key": key, "keys": ["other"], "ttlMs": 5000}),
    )
    .await;
    assert_eq!(
        status, 400,
        "{label} invalid key+keys response: {invalid:?}"
    );
    assert!(
        invalid["error"].as_str().is_some(),
        "{label} invalid response should include error: {invalid:?}"
    );

    let (status, acquire) =
        http_post_json(port, "/v1/lock", json!({"key": key, "ttlMs": 5000})).await;
    assert_eq!(status, 200, "{label} acquire response: {acquire:?}");
    assert_eq!(
        acquire["acquired"], true,
        "{label} acquire response: {acquire:?}"
    );
    assert!(
        acquire["fencingTokens"][&key].as_u64().is_some(),
        "{label} acquire should include a fencing token: {acquire:?}"
    );
    let lock_uuid = acquire["lockUuid"].as_str().unwrap().to_string();

    let (status, contended) = http_post_json(
        port,
        "/v1/lock",
        json!({"key": key, "ttlMs": 5000, "waitMs": 25}),
    )
    .await;
    assert_eq!(status, 200, "{label} contended response: {contended:?}");
    assert_eq!(
        contended["acquired"], false,
        "{label} contended short-poll acquire should time out: {contended:?}"
    );

    let (status, wrong_release) = http_post_json(
        port,
        "/v1/unlock",
        json!({"key": key, "lockUuid": "not-the-lock"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "{label} wrong release response: {wrong_release:?}"
    );
    assert_eq!(
        wrong_release["unlocked"], false,
        "{label} wrong UUID must not unlock: {wrong_release:?}"
    );

    let (status, release) = http_post_json(
        port,
        "/v1/unlock",
        json!({"key": key, "lockUuid": lock_uuid}),
    )
    .await;
    assert_eq!(status, 200, "{label} release response: {release:?}");
    assert_eq!(
        release["unlocked"], true,
        "{label} release response: {release:?}"
    );

    let keys = vec![
        format!("{label}-composite-a-{}", uuid::Uuid::new_v4()),
        format!("{label}-composite-b-{}", uuid::Uuid::new_v4()),
    ];
    // Bounded composite (<= RAFT_MAX_COMPOSITE_KEYS distinct keys) is supported
    // identically by the single-node broker and the replicated BrokerRaft front
    // door, so both labels exercise the same union-overlap semantics here.
    {
        let (status, composite) =
            http_post_json(port, "/v1/lock", json!({"keys": keys, "ttlMs": 5000})).await;
        assert_eq!(status, 200, "{label} composite response: {composite:?}");
        assert_eq!(
            composite["acquired"], true,
            "{label} composite response: {composite:?}"
        );
        let composite_uuid = composite["lockUuid"].as_str().unwrap().to_string();
        let composite_keys = composite["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key.as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            composite_keys.len(),
            2,
            "{label} composite should hold both requested keys: {composite:?}"
        );
        for key in &composite_keys {
            assert!(
                composite["fencingTokens"][key].as_u64().is_some(),
                "{label} composite should include fencing token for {key}: {composite:?}"
            );
        }

        let (status, overlapping_single) = http_post_json(
            port,
            "/v1/lock",
            json!({"key": composite_keys[0].clone(), "ttlMs": 5000, "waitMs": 50}),
        )
        .await;
        assert_eq!(
            status, 200,
            "{label} overlapping single response: {overlapping_single:?}"
        );
        assert_eq!(
            overlapping_single["acquired"], false,
            "{label} single-key acquire must not grant while composite holds that key: {overlapping_single:?}"
        );

        let overlap_extra_key = format!("{label}-composite-overlap-{}", uuid::Uuid::new_v4());
        let (status, overlapping_composite) = http_post_json(
            port,
            "/v1/lock",
            json!({"keys": [composite_keys[1].clone(), overlap_extra_key], "ttlMs": 5000, "waitMs": 50}),
        )
        .await;
        assert_eq!(
            status, 200,
            "{label} overlapping composite response: {overlapping_composite:?}"
        );
        assert_eq!(
            overlapping_composite["acquired"], false,
            "{label} composite acquire must use union overlap semantics, not intersection-only semantics: {overlapping_composite:?}"
        );

        let disjoint_key = format!("{label}-composite-disjoint-{}", uuid::Uuid::new_v4());
        let (status, disjoint) = http_post_json(
            port,
            "/v1/lock",
            json!({"key": disjoint_key, "ttlMs": 5000}),
        )
        .await;
        assert_eq!(status, 200, "{label} disjoint response: {disjoint:?}");
        assert_eq!(
            disjoint["acquired"], true,
            "{label} disjoint key should still grant while composite is held: {disjoint:?}"
        );
        let disjoint_uuid = disjoint["lockUuid"].as_str().unwrap().to_string();
        let (status, disjoint_release) = http_post_json(
            port,
            "/v1/unlock",
            json!({"key": disjoint_key, "lockUuid": disjoint_uuid}),
        )
        .await;
        assert_eq!(
            status, 200,
            "{label} disjoint release response: {disjoint_release:?}"
        );
        assert_eq!(
            disjoint_release["unlocked"], true,
            "{label} disjoint release response: {disjoint_release:?}"
        );

        let (status, composite_release) = http_post_json(
            port,
            "/v1/unlock",
            json!({"keys": composite_keys, "lockUuid": composite_uuid}),
        )
        .await;
        assert_eq!(
            status, 200,
            "{label} composite release response: {composite_release:?}"
        );
        assert_eq!(
            composite_release["unlocked"], true,
            "{label} composite release response: {composite_release:?}"
        );
    }

    // The replicated front door caps composite admission at 3 distinct keys; a
    // 4-key acquire is rejected before append. (The single-node broker accepts
    // up to MAX_COMPOSITE_KEYS, so this bound is BrokerRaft-specific.)
    if label == "raft" {
        let oversized_keys = (0..4)
            .map(|i| format!("{label}-oversized-{i}-{}", uuid::Uuid::new_v4()))
            .collect::<Vec<_>>();
        let (status, oversized) = http_post_json(
            port,
            "/v1/lock",
            json!({"keys": oversized_keys, "ttlMs": 5000}),
        )
        .await;
        assert_eq!(status, 400, "{label} oversized composite: {oversized:?}");
        assert_eq!(oversized["acquired"], false);
        assert!(
            oversized["error"]
                .as_str()
                .is_some_and(|error| error.contains("at most 3 distinct keys")),
            "{label} BrokerRaft should reject composite above the key cap: {oversized:?}"
        );
    }

    let force_key = format!("{label}-force-{}", uuid::Uuid::new_v4());
    let (status, force_acquire) =
        http_post_json(port, "/v1/lock", json!({"key": force_key, "ttlMs": 5000})).await;
    assert_eq!(
        status, 200,
        "{label} force acquire response: {force_acquire:?}"
    );
    assert_eq!(force_acquire["acquired"], true);
    let (status, forced) =
        http_post_json(port, "/v1/unlock", json!({"key": force_key, "force": true})).await;
    assert_eq!(status, 200, "{label} force release response: {forced:?}");
    assert_eq!(
        forced["unlocked"], true,
        "{label} force release response: {forced:?}"
    );

    wait_for_zero_holders_and_waiters_on_port(port).await;
}

async fn run_seeded_http_lock_model(port: u16, seed: u64, steps: usize) {
    let mut rng = seed;
    let mut held: BTreeMap<String, String> = BTreeMap::new();
    let mut fencing_watermark: BTreeMap<String, u64> = BTreeMap::new();
    for step in 0..steps {
        let key = format!("raft-fuzz-key-{}", next_fuzz(&mut rng) % 7);
        if let Some(lock_uuid) = held.get(&key).cloned() {
            match next_fuzz(&mut rng) % 4 {
                0 => {
                    let (status, denied) = http_post_json(
                        port,
                        "/v1/lock",
                        json!({"key": key, "ttlMs": 5000, "waitMs": 10}),
                    )
                    .await;
                    assert_eq!(status, 200, "step={step} denied response: {denied:?}");
                    assert_eq!(
                        denied["acquired"], false,
                        "step={step} held key should not double-grant: {denied:?}"
                    );
                }
                1 => {
                    let (status, wrong_release) = http_post_json(
                        port,
                        "/v1/unlock",
                        json!({"key": key, "lockUuid": "wrong-lock-uuid"}),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "step={step} wrong release response: {wrong_release:?}"
                    );
                    assert_eq!(
                        wrong_release["unlocked"], false,
                        "step={step} wrong UUID must not unlock: {wrong_release:?}"
                    );
                }
                2 => {
                    let (status, release) = http_post_json(
                        port,
                        "/v1/unlock",
                        json!({"key": key, "lockUuid": lock_uuid}),
                    )
                    .await;
                    assert_eq!(status, 200, "step={step} release response: {release:?}");
                    assert_eq!(
                        release["unlocked"], true,
                        "step={step} release: {release:?}"
                    );
                    held.remove(&key);
                }
                _ => {
                    let (status, forced) =
                        http_post_json(port, "/v1/unlock", json!({"key": key, "force": true}))
                            .await;
                    assert_eq!(status, 200, "step={step} force response: {forced:?}");
                    assert_eq!(forced["unlocked"], true, "step={step} force: {forced:?}");
                    held.remove(&key);
                }
            }
        } else {
            let (status, acquire) = http_post_json(
                port,
                "/v1/lock",
                json!({"key": key, "ttlMs": 5000, "waitMs": 25}),
            )
            .await;
            assert_eq!(status, 200, "step={step} acquire response: {acquire:?}");
            assert_eq!(
                acquire["acquired"], true,
                "step={step} unheld key should acquire: {acquire:?}"
            );
            let token = acquire["fencingTokens"][&key]
                .as_u64()
                .unwrap_or_else(|| panic!("step={step} missing fencing token: {acquire:?}"));
            if let Some(previous) = fencing_watermark.insert(key.clone(), token) {
                assert!(
                    token > previous,
                    "step={step} fencing token did not increase for {key}: previous={previous} token={token}"
                );
            }
            held.insert(
                key,
                acquire["lockUuid"].as_str().expect("lockUuid").to_string(),
            );
        }
    }

    for (key, lock_uuid) in held {
        let (status, release) = http_post_json(
            port,
            "/v1/unlock",
            json!({"key": key, "lockUuid": lock_uuid}),
        )
        .await;
        assert_eq!(status, 200, "cleanup release response: {release:?}");
        assert_eq!(
            release["unlocked"], true,
            "cleanup release response: {release:?}"
        );
    }
}

fn next_fuzz(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state
}

async fn http_get_json(port: u16, path: &str) -> Option<Value> {
    let (status, body) = http_request("GET", port, path, None).await.ok()?;
    if status != 200 {
        return None;
    }
    serde_json::from_str(&body).ok()
}

async fn http_get_text(port: u16, path: &str) -> Option<String> {
    let (status, body) = http_request("GET", port, path, None).await.ok()?;
    (status == 200).then_some(body)
}

async fn http_post_json(port: u16, path: &str, body: Value) -> (u16, Value) {
    let (status, body) = http_request("POST", port, path, Some(body))
        .await
        .expect("HTTP request failed");
    let parsed = serde_json::from_str(&body)
        .unwrap_or_else(|err| panic!("failed to parse JSON body: {err}; body={body:?}"));
    (status, parsed)
}

async fn http_post_json_with_headers(
    port: u16,
    path: &str,
    body: Value,
) -> (u16, BTreeMap<String, String>, Value) {
    let raw = http_request_raw("POST", port, path, Some(body))
        .await
        .expect("HTTP request failed");
    let (status, headers, body) = parse_http_response(&raw);
    let parsed = serde_json::from_str(&body)
        .unwrap_or_else(|err| panic!("failed to parse JSON body: {err}; body={body:?}"));
    (status, headers, parsed)
}

async fn http_post_json_retrying_unavailable(
    port: u16,
    path: &str,
    body: Value,
    label: &str,
) -> (u16, Value) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut attempts = 0usize;
    let mut latest = String::new();
    loop {
        attempts = attempts.saturating_add(1);
        match http_request("POST", port, path, Some(body.clone())).await {
            Ok((status, raw_body)) => match serde_json::from_str::<Value>(&raw_body) {
                Ok(parsed) => {
                    if status == 503 && tokio::time::Instant::now() < deadline {
                        latest = format!("status={status} body={parsed:?} attempts={attempts}");
                        tokio::time::sleep(Duration::from_millis(25)).await;
                        continue;
                    }
                    return (status, parsed);
                }
                Err(err) => {
                    if (status == 0 || status == 503 || raw_body.is_empty())
                        && tokio::time::Instant::now() < deadline
                    {
                        latest = format!(
                            "status={status} parse_error={err} raw_body={raw_body:?} attempts={attempts}"
                        );
                        tokio::time::sleep(Duration::from_millis(25)).await;
                        continue;
                    }
                    panic!(
                        "{label}: failed to parse JSON body after {attempts} attempt(s): {err}; body={raw_body:?}; latest={latest}"
                    );
                }
            },
            Err(err) => {
                if tokio::time::Instant::now() < deadline {
                    latest = format!("io_error={err} attempts={attempts}");
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    continue;
                }
                panic!(
                    "{label}: HTTP request failed after {attempts} attempt(s): {err}; latest={latest}"
                );
            }
        }
    }
}

async fn http_request(
    method: &str,
    port: u16,
    path: &str,
    body: Option<Value>,
) -> std::io::Result<(u16, String)> {
    let raw = http_request_raw(method, port, path, body).await?;
    let (status, _, body) = parse_http_response(&raw);
    Ok((status, body))
}

async fn http_request_raw(
    method: &str,
    port: u16,
    path: &str,
    body: Option<Value>,
) -> std::io::Result<String> {
    let body = body
        .map(|value| serde_json::to_vec(&value).unwrap())
        .unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Connection: close\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n",
        body.len()
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
    stream.write_all(request.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await?;

    let mut raw = String::new();
    stream.read_to_string(&mut raw).await?;
    Ok(raw)
}

fn parse_http_response(raw: &str) -> (u16, BTreeMap<String, String>, String) {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let mut headers = BTreeMap::new();
    for line in head.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    (status, headers, body.to_string())
}

/// Issue a GET and return the value of a single response header (case-insensitive
/// name match, trimmed value), or `None` if the header is absent.
async fn http_response_header(port: u16, path: &str, header: &str) -> Option<String> {
    let raw = http_request_raw("GET", port, path, None).await.ok()?;
    let (_, headers, _) = parse_http_response(&raw);
    headers.get(&header.to_ascii_lowercase()).cloned()
}

async fn proxy_http_once(
    mut inbound: TcpStream,
    backends: Vec<u16>,
    next: Arc<AtomicUsize>,
) -> std::io::Result<()> {
    let request = read_http_message(&mut inbound).await?;
    for _ in 0..backends.len() {
        let idx = next.fetch_add(1, Ordering::Relaxed) % backends.len();
        let Ok(mut upstream) = TcpStream::connect(("127.0.0.1", backends[idx])).await else {
            continue;
        };
        upstream.write_all(&request).await?;
        upstream.flush().await?;

        let mut response = Vec::new();
        upstream.read_to_end(&mut response).await?;
        inbound.write_all(&response).await?;
        inbound.flush().await?;
        return Ok(());
    }

    inbound
        .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
        .await?;
    inbound.flush().await
}

async fn read_http_message(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(buf);
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(total) = http_message_len(&buf) {
            if buf.len() >= total {
                return Ok(buf);
            }
        }
    }
}

fn http_message_len(buf: &[u8]) -> Option<usize> {
    let header_end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let headers = std::str::from_utf8(&buf[..header_end]).ok()?;
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    Some(header_end + 4 + content_length)
}

fn metric_value(metrics: &str, name: &str) -> Option<u64> {
    metrics.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        (parts.next()? == name)
            .then(|| parts.next()?.parse::<u64>().ok())
            .flatten()
    })
}
