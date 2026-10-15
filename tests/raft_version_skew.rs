//! Opt-in mixed-binary BrokerRaft smoke.
//!
//! This is skipped by default because it needs an older BrokerRaft binary on
//! disk. Run it from the repo root after building the current binary:
//!
//!   LMX_RAFT_VERSION_SKEW=1 \
//!   LMX_RAFT_OLD_BIN=/path/to/old/dd-rust-network-mutex \
//!   cargo test --test raft_version_skew -- --ignored --nocapture

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

#[derive(Debug)]
struct NodeSpec {
    id: String,
    http_port: u16,
    raft_port: u16,
    data_dir: PathBuf,
    config_path: PathBuf,
    log_path: PathBuf,
}

#[derive(Debug)]
struct VersionSkewCluster {
    root: PathBuf,
    nodes: Vec<NodeSpec>,
    children: Vec<Option<Child>>,
}

#[derive(Debug, Clone)]
struct LockHistoryOp {
    key: String,
    invoke_order: usize,
    response_order: usize,
    result: LockHistoryResult,
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

#[derive(Clone)]
struct HistoryPhase {
    endpoint: String,
    key: String,
    history: Arc<Mutex<Vec<LockHistoryOp>>>,
    order: Arc<AtomicUsize>,
    phase: usize,
    workers: usize,
    steps: usize,
    label: String,
}

impl Drop for VersionSkewCluster {
    fn drop(&mut self) {
        for child in &mut self.children {
            if let Some(mut child) = child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn mixed_binary_rolling_upgrade_preserves_no_wait_linearizability() {
    if !env_flag("LMX_RAFT_VERSION_SKEW") {
        eprintln!(
            "skipping BrokerRaft mixed-binary smoke; set LMX_RAFT_VERSION_SKEW=1 and LMX_RAFT_OLD_BIN=/path/to/old/binary"
        );
        return;
    }

    let old_bin = required_bin("LMX_RAFT_OLD_BIN");
    let new_bin = new_binary_path();
    let old_bin = fs::canonicalize(&old_bin)
        .unwrap_or_else(|err| panic!("failed to canonicalize old binary {old_bin:?}: {err}"));
    let new_bin = fs::canonicalize(&new_bin)
        .unwrap_or_else(|err| panic!("failed to canonicalize new binary {new_bin:?}: {err}"));
    assert_ne!(
        old_bin, new_bin,
        "LMX_RAFT_OLD_BIN and the new binary must be distinct for mixed-binary evidence"
    );

    let root = unique_temp_dir("lmx-raft-version-skew");
    let mut cluster = VersionSkewCluster::new(root).expect("create version-skew cluster");
    for index in 0..cluster.nodes.len() {
        cluster.start_node(index, &old_bin).expect("start old node");
    }
    cluster.wait_for_all_http(Duration::from_secs(10)).await;
    cluster.wait_for_leader(Duration::from_secs(15)).await;

    let endpoint = format!("127.0.0.1:{}", cluster.nodes[0].http_port);
    let key = format!("lmx-version-skew-{}", uuid_short());
    let history = Arc::new(Mutex::new(Vec::<LockHistoryOp>::new()));
    let order = Arc::new(AtomicUsize::new(0));

    run_no_wait_history_phase(HistoryPhase {
        endpoint: endpoint.clone(),
        key: key.clone(),
        history: Arc::clone(&history),
        order: Arc::clone(&order),
        phase: 0,
        workers: 3,
        steps: 6,
        label: "old-cluster".to_string(),
    })
    .await;

    for index in 0..cluster.nodes.len() {
        cluster.stop_node(index).expect("stop old node");
        cluster.start_node(index, &new_bin).expect("start new node");
        cluster
            .wait_for_node_http(index, Duration::from_secs(10))
            .await;
        cluster.wait_for_leader(Duration::from_secs(20)).await;
        run_no_wait_history_phase(HistoryPhase {
            endpoint: endpoint.clone(),
            key: key.clone(),
            history: Arc::clone(&history),
            order: Arc::clone(&order),
            phase: index + 1,
            workers: 3,
            steps: 6,
            label: "rolling-upgrade".to_string(),
        })
        .await;
    }

    cluster.wait_for_all_http(Duration::from_secs(10)).await;
    cluster.wait_for_leader(Duration::from_secs(15)).await;
    acquire_release_with_retry(&endpoint, &format!("{key}-final"), Duration::from_secs(20))
        .await
        .expect("post-upgrade acquire/release");

    let history = history.lock().expect("history").clone();
    assert_lock_history_linearizable(&history);
}

impl VersionSkewCluster {
    fn new(root: PathBuf) -> std::io::Result<Self> {
        fs::create_dir_all(&root)?;
        let mut nodes = Vec::new();
        for index in 0..3 {
            let id = format!("node-{}", index + 1);
            nodes.push(NodeSpec {
                id,
                http_port: free_port()?,
                raft_port: free_port()?,
                data_dir: root.join(format!("node-{}-data", index + 1)),
                config_path: root.join(format!("node-{}.toml", index + 1)),
                log_path: root.join(format!("node-{}.log", index + 1)),
            });
        }
        for index in 0..nodes.len() {
            write_node_config(&nodes, index)?;
        }
        let children = (0..nodes.len()).map(|_| None).collect();
        Ok(Self {
            root,
            nodes,
            children,
        })
    }

    fn start_node(&mut self, index: usize, binary: &Path) -> std::io::Result<()> {
        assert!(
            self.children[index].is_none(),
            "node {index} should not already be running"
        );
        let node = &self.nodes[index];
        fs::create_dir_all(&node.data_dir)?;
        let log = File::create(&node.log_path)?;
        let log_err = log.try_clone()?;
        let child = Command::new(binary)
            .env_clear()
            .env("LMX_CONFIG", &node.config_path)
            .env("RUST_LOG", "info")
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .spawn()?;
        self.children[index] = Some(child);
        Ok(())
    }

    fn stop_node(&mut self, index: usize) -> std::io::Result<()> {
        if let Some(mut child) = self.children[index].take() {
            let _ = child.kill();
            let _ = child.wait()?;
        }
        Ok(())
    }

    async fn wait_for_all_http(&mut self, timeout: Duration) {
        for index in 0..self.nodes.len() {
            self.wait_for_node_http(index, timeout).await;
        }
    }

    async fn wait_for_node_http(&mut self, index: usize, timeout: Duration) {
        let started = Instant::now();
        loop {
            self.assert_node_still_running(index);
            if try_http_json(&self.endpoint(index), "GET", "/raft/status", None)
                .await
                .is_ok()
            {
                return;
            }
            assert!(
                started.elapsed() < timeout,
                "timed out waiting for node {index} HTTP; log={}",
                self.log_tail(index)
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn wait_for_leader(&mut self, timeout: Duration) -> usize {
        let started = Instant::now();
        loop {
            let mut statuses = Vec::new();
            for index in 0..self.nodes.len() {
                self.assert_node_still_running(index);
                if let Ok((200, status)) =
                    try_http_json(&self.endpoint(index), "GET", "/raft/status", None).await
                {
                    statuses.push((index, status));
                }
            }
            if statuses.len() == self.nodes.len() {
                let leaders = statuses
                    .iter()
                    .filter_map(|(idx, status)| {
                        status["isLeader"].as_bool().filter(|v| *v).map(|_| *idx)
                    })
                    .collect::<Vec<_>>();
                if leaders.len() == 1 {
                    let leader_id = statuses
                        .iter()
                        .find(|(idx, _)| *idx == leaders[0])
                        .and_then(|(_, status)| status["nodeId"].as_str())
                        .unwrap_or_default()
                        .to_string();
                    if statuses
                        .iter()
                        .all(|(_, status)| status["leaderId"].as_str() == Some(leader_id.as_str()))
                    {
                        return leaders[0];
                    }
                }
            }
            assert!(
                started.elapsed() < timeout,
                "timed out waiting for mixed-binary leader; statuses={statuses:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn endpoint(&self, index: usize) -> String {
        format!("127.0.0.1:{}", self.nodes[index].http_port)
    }

    fn assert_node_still_running(&mut self, index: usize) {
        if let Some(child) = &mut self.children[index] {
            match child.try_wait() {
                Ok(Some(status)) => panic!(
                    "node {index} exited early with {status}; log={}",
                    self.log_tail(index)
                ),
                Ok(None) => {}
                Err(err) => panic!("failed to inspect node {index}: {err}"),
            }
        }
    }

    fn log_tail(&self, index: usize) -> String {
        fs::read_to_string(&self.nodes[index].log_path)
            .map(|text| {
                let lines = text.lines().rev().take(40).collect::<Vec<_>>();
                lines.into_iter().rev().collect::<Vec<_>>().join("\n")
            })
            .unwrap_or_else(|err| format!("failed to read log: {err}"))
    }
}

fn write_node_config(nodes: &[NodeSpec], index: usize) -> std::io::Result<()> {
    let node = &nodes[index];
    let mut text = String::new();
    text.push_str("[server]\n");
    text.push_str("bind_host = \"127.0.0.1\"\n");
    text.push_str("disable_tcp = true\n");
    text.push_str("disable_http = false\n");
    text.push_str(&format!("http_port = {}\n", node.http_port));
    text.push_str("\n[broker]\n");
    text.push_str("default_ttl_ms = 4000\n");
    text.push_str("max_lock_holders = 1\n");
    text.push_str("\n[raft]\n");
    text.push_str("enabled = true\n");
    text.push_str(&format!("node_id = \"{}\"\n", node.id));
    text.push_str(&format!("bind_addr = \"127.0.0.1:{}\"\n", node.raft_port));
    text.push_str(&format!(
        "advertise_addr = \"127.0.0.1:{}\"\n",
        node.raft_port
    ));
    text.push_str(&format!("data_dir = \"{}\"\n", node.data_dir.display()));
    text.push_str("data_dir_lock = true\n");
    text.push_str("heartbeat_interval_ms = 50\n");
    text.push_str("election_timeout_min_ms = 150\n");
    text.push_str("election_timeout_max_ms = 300\n");
    text.push_str("snapshot_interval_ms = 60000\n");
    text.push_str("snapshot_max_log_entries = 100000\n");
    text.push_str("snapshot_max_log_bytes = 67108864\n");
    text.push_str("snapshot_max_log_age_ms = 60000\n");
    text.push_str("trailing_log_entries = 1000\n");
    text.push_str("append_entries_max_entries = 64\n");
    text.push_str("append_entries_max_bytes = 262144\n");
    text.push_str("append_entries_max_inline_batches = 16\n");
    text.push_str("target_quorum_extra_fanout = 0\n");
    text.push_str("client_batch_max_entries = 16\n");
    text.push_str("client_pipeline_max_batches = 2\n");
    text.push_str("client_batch_max_delay_ms = 1\n");
    text.push_str("proxy_retry_budget_ms = 2000\n");
    text.push_str("sync_log = true\n");
    text.push_str("sync_commit = true\n");
    for peer in nodes {
        text.push_str("\n[[raft.peers]]\n");
        text.push_str(&format!("id = \"{}\"\n", peer.id));
        text.push_str(&format!("addr = \"127.0.0.1:{}\"\n", peer.raft_port));
    }
    fs::write(&node.config_path, text)
}

async fn run_no_wait_history_phase(config: HistoryPhase) {
    let start = Arc::new(tokio::sync::Barrier::new(config.workers));
    let mut tasks = Vec::new();
    for worker in 0..config.workers {
        let endpoint = config.endpoint.clone();
        let key = config.key.clone();
        let history = Arc::clone(&config.history);
        let order = Arc::clone(&config.order);
        let start = Arc::clone(&start);
        let phase = config.phase;
        let steps = config.steps;
        let label = config.label.clone();
        tasks.push(tokio::spawn(async move {
            start.wait().await;
            for step in 0..steps {
                let request_id = format!(
                    "{label}-acquire-{phase}-{worker}-{step}-{}",
                    uuid::Uuid::new_v4()
                );
                let invoke = order.fetch_add(1, Ordering::SeqCst);
                let (status, acquire) = http_json_retrying_unavailable(
                    &endpoint,
                    "POST",
                    "/v1/lock",
                    json!({
                        "key": key,
                        "ttlMs": 5000,
                        "waitMs": 0,
                        "requestId": request_id,
                    }),
                    Duration::from_secs(20),
                )
                .await;
                let response = order.fetch_add(1, Ordering::SeqCst);
                assert_eq!(status, 200, "{label} acquire response: {acquire:?}");
                if acquire["acquired"].as_bool() == Some(true) {
                    let lock_uuid = acquire["lockUuid"]
                        .as_str()
                        .unwrap_or_else(|| panic!("{label} acquire missing lockUuid: {acquire:?}"))
                        .to_string();
                    history.lock().expect("history").push(LockHistoryOp {
                        key: key.clone(),
                        invoke_order: invoke,
                        response_order: response,
                        result: LockHistoryResult::Acquired {
                            lock_uuid: lock_uuid.clone(),
                        },
                    });
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    let release_id = format!(
                        "{label}-release-{phase}-{worker}-{step}-{}",
                        uuid::Uuid::new_v4()
                    );
                    let release_invoke = order.fetch_add(1, Ordering::SeqCst);
                    let (status, release) = http_json_retrying_unavailable(
                        &endpoint,
                        "POST",
                        "/v1/unlock",
                        json!({
                            "key": key,
                            "lockUuid": lock_uuid.clone(),
                            "requestId": release_id,
                        }),
                        Duration::from_secs(20),
                    )
                    .await;
                    let release_response = order.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(status, 200, "{label} release response: {release:?}");
                    assert_eq!(release["unlocked"], true, "{label} release: {release:?}");
                    history.lock().expect("history").push(LockHistoryOp {
                        key: key.clone(),
                        invoke_order: release_invoke,
                        response_order: release_response,
                        result: LockHistoryResult::Released { lock_uuid },
                    });
                } else {
                    assert_eq!(acquire["acquired"], false, "{label} acquire: {acquire:?}");
                    history.lock().expect("history").push(LockHistoryOp {
                        key: key.clone(),
                        invoke_order: invoke,
                        response_order: response,
                        result: LockHistoryResult::NotAcquired,
                    });
                }
            }
        }));
    }
    for task in tasks {
        task.await.expect("history worker");
    }
}

async fn acquire_release_with_retry(
    endpoint: &str,
    key: &str,
    timeout: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        match acquire_release_once(endpoint, key).await {
            Ok(()) => return Ok(()),
            Err(err) if started.elapsed() < timeout => {
                eprintln!("retrying final acquire/release: {err}");
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

async fn acquire_release_once(endpoint: &str, key: &str) -> Result<(), String> {
    let (status, acquired) = try_http_json(
        endpoint,
        "POST",
        "/v1/lock",
        Some(json!({"key": key, "ttlMs": 5000, "requestId": uuid_short()})),
    )
    .await?;
    if status != 200 || acquired["acquired"] != true {
        return Err(format!("acquire status={status} body={acquired:?}"));
    }
    let lock_uuid = acquired["lockUuid"]
        .as_str()
        .ok_or_else(|| format!("acquire missing lockUuid: {acquired:?}"))?
        .to_string();
    let (status, released) = try_http_json(
        endpoint,
        "POST",
        "/v1/unlock",
        Some(json!({"key": key, "lockUuid": lock_uuid, "requestId": uuid_short()})),
    )
    .await?;
    if status != 200 || released["unlocked"] != true {
        return Err(format!("release status={status} body={released:?}"));
    }
    Ok(())
}

async fn http_json_retrying_unavailable(
    endpoint: &str,
    method: &str,
    path: &str,
    body: Value,
    timeout: Duration,
) -> (u16, Value) {
    let started = Instant::now();
    loop {
        match try_http_json(endpoint, method, path, Some(body.clone())).await {
            Ok((status, parsed)) if status != 0 && status != 503 => return (status, parsed),
            Ok((_, _)) | Err(_) if started.elapsed() < timeout => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Ok((status, parsed)) => return (status, parsed),
            Err(err) => panic!("timed out waiting for available HTTP response: {err}"),
        }
    }
}

async fn try_http_json(
    endpoint: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<(u16, Value), String> {
    let (status, body) = http_request(endpoint, method, path, body)
        .await
        .map_err(|err| format!("HTTP request failed: {err}"))?;
    let parsed = serde_json::from_str(&body)
        .map_err(|err| format!("failed to parse JSON body: {err}; body={body:?}"))?;
    Ok((status, parsed))
}

async fn http_request(
    endpoint: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> std::io::Result<(u16, String)> {
    let body = body
        .map(|value| serde_json::to_vec(&value).unwrap())
        .unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: {endpoint}\r\n\
         Connection: close\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n",
        body.len()
    );
    let mut stream = TcpStream::connect(endpoint)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(request.as_bytes())?;
    stream.write_all(&body)?;
    stream.flush()?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    let status = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let body = raw
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

fn assert_lock_history_linearizable(history: &[LockHistoryOp]) {
    assert!(!history.is_empty(), "history should contain operations");
    let mut by_key = BTreeMap::<String, Vec<LockHistoryOp>>::new();
    for op in history {
        by_key.entry(op.key.clone()).or_default().push(op.clone());
    }
    for (key, ops) in by_key {
        assert_linearizable_key_history(&key, &ops);
    }
}

fn assert_linearizable_key_history(key: &str, ops: &[LockHistoryOp]) {
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
        if search_linearized_history(done | bit, next_holder, all_done, ops, predecessors, memo) {
            return true;
        }
    }
    false
}

fn apply_linear_model_op(holder: u16, op: LinearModelOp) -> Option<u16> {
    match op {
        LinearModelOp::AcquireGranted(lock_id) if holder == 0 => Some(lock_id),
        LinearModelOp::AcquireGranted(_) => None,
        LinearModelOp::AcquireRejected if holder != 0 => Some(holder),
        LinearModelOp::AcquireRejected => None,
        LinearModelOp::Release(lock_id) if holder == lock_id => Some(0),
        LinearModelOp::Release(_) => None,
    }
}

fn free_port() -> std::io::Result<u16> {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr().map(|addr| addr.port()))
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&path).expect("create temp dir");
    path
}

fn required_bin(env_name: &str) -> PathBuf {
    let value = std::env::var(env_name)
        .unwrap_or_else(|_| panic!("{env_name} must point to an executable BrokerRaft binary"));
    let path = PathBuf::from(value);
    assert!(path.exists(), "{env_name} path does not exist: {path:?}");
    path
}

fn new_binary_path() -> PathBuf {
    if let Ok(path) = std::env::var("LMX_RAFT_NEW_BIN") {
        return PathBuf::from(path);
    }
    option_env!("CARGO_BIN_EXE_dd-rust-network-mutex")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "LMX_RAFT_NEW_BIN is unset and Cargo did not expose CARGO_BIN_EXE_dd-rust-network-mutex"
            )
        })
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn uuid_short() -> String {
    uuid::Uuid::new_v4()
        .to_string()
        .split('-')
        .next()
        .unwrap()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(invoke_order: usize, response_order: usize, result: LockHistoryResult) -> LockHistoryOp {
        LockHistoryOp {
            key: "version-skew-key".to_string(),
            invoke_order,
            response_order,
            result,
        }
    }

    #[test]
    fn version_skew_history_checker_accepts_serial_no_wait_lock_history() {
        let history = vec![
            op(
                0,
                1,
                LockHistoryResult::Acquired {
                    lock_uuid: "lock-a".to_string(),
                },
            ),
            op(2, 3, LockHistoryResult::NotAcquired),
            op(
                4,
                5,
                LockHistoryResult::Released {
                    lock_uuid: "lock-a".to_string(),
                },
            ),
            op(
                6,
                7,
                LockHistoryResult::Acquired {
                    lock_uuid: "lock-b".to_string(),
                },
            ),
            op(
                8,
                9,
                LockHistoryResult::Released {
                    lock_uuid: "lock-b".to_string(),
                },
            ),
        ];

        assert_lock_history_linearizable(&history);
    }

    #[test]
    #[should_panic(expected = "no linearization found")]
    fn version_skew_history_checker_rejects_overlapping_grants() {
        let history = vec![
            op(
                0,
                1,
                LockHistoryResult::Acquired {
                    lock_uuid: "lock-a".to_string(),
                },
            ),
            op(
                2,
                3,
                LockHistoryResult::Acquired {
                    lock_uuid: "lock-b".to_string(),
                },
            ),
        ];

        assert_lock_history_linearizable(&history);
    }

    #[test]
    fn version_skew_cluster_writes_three_node_incremental_raft_config() {
        let root = unique_temp_dir("lmx-raft-version-skew-config-test");
        let cluster = VersionSkewCluster::new(root).expect("create version-skew test cluster");

        assert_eq!(cluster.nodes.len(), 3);
        for (index, node) in cluster.nodes.iter().enumerate() {
            let config = fs::read_to_string(&node.config_path)
                .unwrap_or_else(|err| panic!("read config for version-skew node {index}: {err}"));
            assert!(config.contains("[raft]\n"));
            assert!(config.contains("enabled = true\n"));
            assert!(config.contains("data_dir_lock = true\n"));
            assert!(config.contains("append_entries_max_entries = 64\n"));
            assert!(config.contains("append_entries_max_bytes = 262144\n"));
            assert!(config.contains("append_entries_max_inline_batches = 16\n"));
            assert!(config.contains("target_quorum_extra_fanout = 0\n"));
            assert!(config.contains("client_batch_max_entries = 16\n"));
            assert_eq!(
                config.matches("[[raft.peers]]").count(),
                3,
                "node {index} config should include all peers: {config}"
            );
            for peer in &cluster.nodes {
                assert!(
                    config.contains(&format!("id = \"{}\"", peer.id)),
                    "node {index} config missing peer {}: {config}",
                    peer.id
                );
                assert!(
                    config.contains(&format!("addr = \"127.0.0.1:{}\"", peer.raft_port)),
                    "node {index} config missing peer addr {}: {config}",
                    peer.raft_port
                );
            }
        }
    }
}
