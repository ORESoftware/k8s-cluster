//! End-to-end smoke test against a deployed BrokerRaft HTTP service.
//!
//! Skipped by default. Run from a network location that can reach the
//! Kubernetes Service / load balancer with:
//!
//!   LMX_LIVE_RAFT_HTTP=dd-rust-network-mutex-raft.default.svc.cluster.local:6971 \
//!   cargo test --test k8s_raft_live_smoke -- --ignored --nocapture

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const DEFAULT_LIVE_HTTP_REQUEST_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_KUBECTL_REQUEST_TIMEOUT: &str = "5s";
const DEFAULT_KUBECTL_PROCESS_TIMEOUT_MS: u64 = 15_000;
const KUBECTL_OUTPUT_PREVIEW_BYTES: usize = 4096;

fn require_http() -> String {
    env::var("LMX_LIVE_RAFT_HTTP").expect(
        "LMX_LIVE_RAFT_HTTP must be set (e.g. dd-rust-network-mutex-raft.default.svc.cluster.local:6971)",
    )
}

#[derive(Debug, Clone)]
struct LiveHttpLockHistoryOp {
    key: String,
    invoke_order: usize,
    response_order: usize,
    result: LiveHttpLockHistoryResult,
}

#[derive(Debug, Clone)]
enum LiveHttpLockHistoryResult {
    Acquired { lock_uuid: String },
    NotAcquired,
    Released { lock_uuid: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveHttpLinearModelOp {
    AcquireGranted(u16),
    AcquireRejected,
    Release(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LiveFullLogMetrics {
    reads_total: u64,
    read_failures_total: u64,
    read_bytes_total: u64,
    read_entries_total: u64,
    rewrites_total: u64,
    rewrite_failures_total: u64,
    rewrite_entries_total: u64,
    rewrite_bytes_total: u64,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn live_raft_http_acquire_release_via_lb_service() {
    let endpoint = require_http();
    let expected_cluster_size = expected_live_cluster_size();
    let expected_quorum_size = expected_live_quorum_size(expected_cluster_size);
    let key = format!("lmx-live-raft-{}", uuid_short());

    let (status, status_body) = http_json(&endpoint, "GET", "/raft/status", None).await;
    assert_eq!(status, 200, "raft status response: {status_body:?}");
    assert_live_raft_cluster_status(
        &status_body,
        expected_cluster_size,
        expected_quorum_size,
        "raft status response",
    );

    let metrics_endpoints = live_metrics_endpoints(expected_cluster_size);
    let before_full_log = if let Some(endpoints) = &metrics_endpoints {
        Some(current_live_full_log_metrics(endpoints).await)
    } else {
        eprintln!(
            "skipping live full-log metric guard; set LMX_LIVE_RAFT_METRICS_ENDPOINTS to stable pod/service HTTP endpoints"
        );
        None
    };

    let (status, acquired) = http_json(
        &endpoint,
        "POST",
        "/v1/lock",
        Some(json!({"key": key, "ttlMs": 5000})),
    )
    .await;
    assert_eq!(status, 200, "acquire response: {acquired:?}");
    assert_eq!(acquired["acquired"], true, "acquire response: {acquired:?}");
    let lock_uuid = acquired["lockUuid"].as_str().expect("lockUuid").to_string();

    let (status, released) = http_json(
        &endpoint,
        "POST",
        "/v1/unlock",
        Some(json!({"key": key, "lockUuid": lock_uuid})),
    )
    .await;
    assert_eq!(status, 200, "release response: {released:?}");
    assert_eq!(released["unlocked"], true, "release response: {released:?}");

    if let (Some(endpoints), Some(before_full_log)) = (&metrics_endpoints, &before_full_log) {
        assert_live_full_log_metrics_unchanged(
            endpoints,
            before_full_log,
            "live LB acquire/release",
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn live_raft_http_survives_kubectl_leader_delete_when_enabled() {
    if !env_flag("LMX_LIVE_RAFT_KUBECTL_FAILOVER") {
        eprintln!(
            "skipping kubectl-driven BrokerRaft failover smoke; set LMX_LIVE_RAFT_KUBECTL_FAILOVER=true to enable"
        );
        return;
    }

    let endpoint = require_http();
    let expected_cluster_size = expected_live_cluster_size();
    let expected_quorum_size = expected_live_quorum_size(expected_cluster_size);
    let namespace = env::var("LMX_LIVE_RAFT_NAMESPACE").unwrap_or_else(|_| "default".into());
    let statefulset =
        env::var("LMX_LIVE_RAFT_STATEFULSET").unwrap_or_else(|_| "live-mutex-rs-raft".into());
    let before = wait_for_raft_status_with_leader(&endpoint, Duration::from_secs(30)).await;
    assert_live_raft_cluster_status(
        &before,
        expected_cluster_size,
        expected_quorum_size,
        "pre-failover status",
    );
    let leader = observed_leader_id(&before)
        .unwrap_or_else(|| panic!("pre-failover status should expose a leader: {before:?}"));
    let pod = env::var("LMX_LIVE_RAFT_LEADER_POD").unwrap_or(leader);
    let old_pod_uid = kubectl_pod_uid(&namespace, &pod);

    let prefix = format!("lmx-live-raft-failover-{}", uuid_short());
    let history_key = format!("{prefix}-history");
    let history = Arc::new(Mutex::new(Vec::<LiveHttpLockHistoryOp>::new()));
    let order = Arc::new(AtomicUsize::new(0));
    let metrics_endpoints = live_metrics_endpoints(expected_cluster_size);

    let before_phase_full_log = if let Some(endpoints) = &metrics_endpoints {
        Some(current_live_full_log_metrics(endpoints).await)
    } else {
        eprintln!(
            "skipping live full-log metric guard; set LMX_LIVE_RAFT_METRICS_ENDPOINTS to stable pod/service HTTP endpoints"
        );
        None
    };
    run_live_http_no_wait_history_phase(LiveHttpNoWaitHistoryPhase {
        endpoint: endpoint.clone(),
        keys: vec![history_key.clone()],
        history: Arc::clone(&history),
        order: Arc::clone(&order),
        phase: 0,
        workers: 2,
        steps: 3,
        label: "live-k8s-before-failover".to_string(),
    })
    .await;
    if let (Some(endpoints), Some(before_phase_full_log)) =
        (&metrics_endpoints, &before_phase_full_log)
    {
        assert_live_full_log_metrics_unchanged(
            endpoints,
            before_phase_full_log,
            "live k8s pre-failover history traffic",
        )
        .await;
    }

    acquire_release_with_retry(
        &endpoint,
        &format!("{prefix}-before"),
        Duration::from_secs(15),
    )
    .await
    .expect("pre-failover acquire/release through BrokerRaft service");

    let during_history = tokio::spawn(run_live_http_no_wait_history_phase(
        LiveHttpNoWaitHistoryPhase {
            endpoint: endpoint.clone(),
            keys: vec![history_key],
            history: Arc::clone(&history),
            order: Arc::clone(&order),
            phase: 1,
            workers: 4,
            steps: 10,
            label: "live-k8s-during-failover".to_string(),
        },
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;

    run_kubectl(&[
        "-n",
        namespace.as_str(),
        "delete",
        "pod",
        pod.as_str(),
        "--wait=false",
    ]);
    wait_for_pod_uid_change(&namespace, &pod, &old_pod_uid, Duration::from_secs(180));

    let after = wait_for_raft_status_with_leader(&endpoint, Duration::from_secs(90)).await;
    assert_live_raft_cluster_status(
        &after,
        expected_cluster_size,
        expected_quorum_size,
        "post-failover status",
    );

    during_history
        .await
        .expect("live k8s failover history worker should not panic");

    let after_failover_full_log = if let Some(endpoints) = &metrics_endpoints {
        Some(current_live_full_log_metrics(endpoints).await)
    } else {
        None
    };
    acquire_release_with_retry(
        &endpoint,
        &format!("{prefix}-after"),
        Duration::from_secs(90),
    )
    .await
    .expect("post-failover acquire/release through BrokerRaft service");
    if let (Some(endpoints), Some(after_failover_full_log)) =
        (&metrics_endpoints, &after_failover_full_log)
    {
        assert_live_full_log_metrics_unchanged(
            endpoints,
            after_failover_full_log,
            "live k8s post-failover acquire/release",
        )
        .await;
    }

    let history = history.lock().expect("live k8s history").clone();
    assert_live_http_lock_history_linearizable(&history);

    let rollout_target = format!("statefulset/{statefulset}");
    run_kubectl(&[
        "-n",
        namespace.as_str(),
        "rollout",
        "status",
        rollout_target.as_str(),
        "--timeout=180s",
    ]);
}

struct LiveHttpNoWaitHistoryPhase {
    endpoint: String,
    keys: Vec<String>,
    history: Arc<Mutex<Vec<LiveHttpLockHistoryOp>>>,
    order: Arc<AtomicUsize>,
    phase: usize,
    workers: usize,
    steps: usize,
    label: String,
}

async fn run_live_http_no_wait_history_phase(phase_spec: LiveHttpNoWaitHistoryPhase) {
    let LiveHttpNoWaitHistoryPhase {
        endpoint,
        keys,
        history,
        order,
        phase,
        workers,
        steps,
        label,
    } = phase_spec;
    assert!(!keys.is_empty(), "live history phase needs lock keys");
    assert!(workers > 0, "live history phase needs workers");
    let start = Arc::new(tokio::sync::Barrier::new(workers));
    let mut tasks = Vec::new();

    for worker in 0..workers {
        let endpoint = endpoint.clone();
        let keys = keys.clone();
        let history = Arc::clone(&history);
        let order = Arc::clone(&order);
        let start = Arc::clone(&start);
        let label = label.clone();
        tasks.push(tokio::spawn(async move {
            start.wait().await;
            let mut state =
                0xA11C_E571_5AFE_F00Du64 ^ ((phase as u64).wrapping_shl(17)) ^ worker as u64;
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
                let (status, acquire) = http_json_retrying_unavailable(
                    &endpoint,
                    "POST",
                    "/v1/lock",
                    json!({
                        "key": key.clone(),
                        "ttlMs": 5000,
                        "waitMs": 0,
                        "requestId": acquire_request_id,
                    }),
                    &format!("{label} acquire phase={phase} worker={worker} step={step}"),
                    Duration::from_secs(90),
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
                    history
                        .lock()
                        .expect("live k8s history")
                        .push(LiveHttpLockHistoryOp {
                            key: key.clone(),
                            invoke_order: acquire_invoke,
                            response_order: acquire_response,
                            result: LiveHttpLockHistoryResult::Acquired {
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
                    let (status, release) = http_json_retrying_unavailable(
                        &endpoint,
                        "POST",
                        "/v1/unlock",
                        json!({
                            "key": key.clone(),
                            "lockUuid": lock_uuid.clone(),
                            "requestId": release_request_id,
                        }),
                        &format!("{label} release phase={phase} worker={worker} step={step}"),
                        Duration::from_secs(90),
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
                    history
                        .lock()
                        .expect("live k8s history")
                        .push(LiveHttpLockHistoryOp {
                            key,
                            invoke_order: release_invoke,
                            response_order: release_response,
                            result: LiveHttpLockHistoryResult::Released { lock_uuid },
                        });
                } else {
                    assert_eq!(
                        acquire["acquired"],
                        false,
                        "{label} acquire phase={phase} worker={worker} step={step} should return a boolean acquired field: {acquire:?}"
                    );
                    history
                        .lock()
                        .expect("live k8s history")
                        .push(LiveHttpLockHistoryOp {
                            key,
                            invoke_order: acquire_invoke,
                            response_order: acquire_response,
                            result: LiveHttpLockHistoryResult::NotAcquired,
                        });
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
        }));
    }

    for task in tasks {
        task.await
            .expect("live HTTP history phase worker should not panic");
    }
}

async fn http_json(endpoint: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    try_http_json(endpoint, method, path, body)
        .await
        .expect("HTTP JSON request failed")
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

fn live_metrics_endpoints(expected_cluster_size: u64) -> Option<Vec<String>> {
    let value = env::var("LMX_LIVE_RAFT_METRICS_ENDPOINTS").ok()?;
    let endpoints = parse_live_metrics_endpoints(&value);
    assert_live_metrics_endpoint_set_covers_expected_cluster(&endpoints, expected_cluster_size);
    Some(endpoints)
}

fn parse_live_metrics_endpoints(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|endpoint| !endpoint.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>()
}

fn assert_live_metrics_endpoint_set_covers_expected_cluster(
    endpoints: &[String],
    expected_cluster_size: u64,
) {
    let expected_cluster_size = usize::try_from(expected_cluster_size)
        .expect("LMX_LIVE_RAFT_EXPECTED_CLUSTER_SIZE should fit in usize");
    assert!(
        !endpoints.is_empty(),
        "LMX_LIVE_RAFT_METRICS_ENDPOINTS was set but did not contain any endpoints"
    );
    assert_eq!(
        endpoints.len(),
        expected_cluster_size,
        "LMX_LIVE_RAFT_METRICS_ENDPOINTS must contain exactly one stable HTTP endpoint per expected Raft node"
    );
    let unique = endpoints.iter().collect::<BTreeSet<_>>();
    assert_eq!(
        unique.len(),
        endpoints.len(),
        "LMX_LIVE_RAFT_METRICS_ENDPOINTS must not contain duplicate endpoints"
    );
}

fn expected_live_cluster_size() -> u64 {
    env::var("LMX_LIVE_RAFT_EXPECTED_CLUSTER_SIZE")
        .ok()
        .map(|value| parse_live_positive_u64("LMX_LIVE_RAFT_EXPECTED_CLUSTER_SIZE", value.trim()))
        .unwrap_or(3)
}

fn expected_live_quorum_size(cluster_size: u64) -> u64 {
    let quorum_size = env::var("LMX_LIVE_RAFT_EXPECTED_QUORUM_SIZE")
        .ok()
        .map(|value| parse_live_positive_u64("LMX_LIVE_RAFT_EXPECTED_QUORUM_SIZE", value.trim()))
        .unwrap_or_else(|| cluster_size / 2 + 1);
    assert!(
        quorum_size <= cluster_size,
        "LMX_LIVE_RAFT_EXPECTED_QUORUM_SIZE ({quorum_size}) cannot exceed expected cluster size ({cluster_size})"
    );
    quorum_size
}

fn parse_live_positive_u64(name: &str, value: &str) -> u64 {
    let parsed = value
        .parse::<u64>()
        .unwrap_or_else(|err| panic!("{name} must be a positive integer; got {value:?}: {err}"));
    assert!(parsed > 0, "{name} must be positive; got {parsed}");
    parsed
}

fn live_http_request_timeout_from(value: Option<&str>) -> Duration {
    let millis = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| parse_live_positive_u64("LMX_LIVE_RAFT_HTTP_REQUEST_TIMEOUT_MS", value))
        .unwrap_or(DEFAULT_LIVE_HTTP_REQUEST_TIMEOUT_MS);
    Duration::from_millis(millis)
}

fn live_http_request_timeout() -> Duration {
    let value = env::var("LMX_LIVE_RAFT_HTTP_REQUEST_TIMEOUT_MS").ok();
    live_http_request_timeout_from(value.as_deref())
}

fn kubectl_request_timeout_arg_from(value: Option<&str>) -> Option<String> {
    let value = value
        .map(str::trim)
        .unwrap_or(DEFAULT_KUBECTL_REQUEST_TIMEOUT);
    if value.is_empty() || value == "0" || value.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(format!("--request-timeout={value}"))
    }
}

fn kubectl_request_timeout_arg() -> Option<String> {
    let value = env::var("LMX_LIVE_RAFT_KUBECTL_REQUEST_TIMEOUT").ok();
    kubectl_request_timeout_arg_from(value.as_deref())
}

fn kubectl_process_timeout_from(value: Option<&str>) -> Duration {
    let millis = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| parse_live_positive_u64("LMX_LIVE_RAFT_KUBECTL_PROCESS_TIMEOUT_MS", value))
        .unwrap_or(DEFAULT_KUBECTL_PROCESS_TIMEOUT_MS);
    Duration::from_millis(millis)
}

fn kubectl_process_timeout() -> Duration {
    let value = env::var("LMX_LIVE_RAFT_KUBECTL_PROCESS_TIMEOUT_MS").ok();
    kubectl_process_timeout_from(value.as_deref())
}

fn assert_live_raft_cluster_status(
    status: &Value,
    expected_cluster_size: u64,
    expected_quorum_size: u64,
    label: &str,
) {
    assert_eq!(
        status["clusterSize"].as_u64(),
        Some(expected_cluster_size),
        "{label}: {status:?}"
    );
    assert_eq!(
        status["quorumSize"].as_u64(),
        Some(expected_quorum_size),
        "{label}: {status:?}"
    );
}

async fn current_live_full_log_metrics(
    endpoints: &[String],
) -> BTreeMap<String, LiveFullLogMetrics> {
    let mut metrics = BTreeMap::new();
    for endpoint in endpoints {
        metrics.insert(
            endpoint.clone(),
            current_live_full_log_metric(endpoint).await,
        );
    }
    metrics
}

async fn current_live_full_log_metric(endpoint: &str) -> LiveFullLogMetrics {
    let metrics = http_text(endpoint, "GET", "/metrics", None)
        .await
        .unwrap_or_else(|err| {
            panic!("failed to scrape live BrokerRaft metrics at {endpoint}: {err}")
        });
    LiveFullLogMetrics {
        reads_total: prometheus_metric_value(
            &metrics,
            "dd_rust_network_mutex_raft_log_full_reads_total",
        )
        .unwrap_or_else(|| panic!("full-log read metric missing at {endpoint}")),
        read_failures_total: prometheus_metric_value(
            &metrics,
            "dd_rust_network_mutex_raft_log_full_read_failures_total",
        )
        .unwrap_or_else(|| panic!("full-log read-failure metric missing at {endpoint}")),
        read_bytes_total: prometheus_metric_value(
            &metrics,
            "dd_rust_network_mutex_raft_log_full_read_bytes_total",
        )
        .unwrap_or_else(|| panic!("full-log read-bytes metric missing at {endpoint}")),
        read_entries_total: prometheus_metric_value(
            &metrics,
            "dd_rust_network_mutex_raft_log_full_read_entries_total",
        )
        .unwrap_or_else(|| panic!("full-log read-entries metric missing at {endpoint}")),
        rewrites_total: prometheus_metric_value(
            &metrics,
            "dd_rust_network_mutex_raft_log_full_rewrites_total",
        )
        .unwrap_or_else(|| panic!("full-log rewrite metric missing at {endpoint}")),
        rewrite_failures_total: prometheus_metric_value(
            &metrics,
            "dd_rust_network_mutex_raft_log_full_rewrite_failures_total",
        )
        .unwrap_or_else(|| panic!("full-log rewrite-failure metric missing at {endpoint}")),
        rewrite_entries_total: prometheus_metric_value(
            &metrics,
            "dd_rust_network_mutex_raft_log_full_rewrite_entries_total",
        )
        .unwrap_or_else(|| panic!("full-log rewrite-entries metric missing at {endpoint}")),
        rewrite_bytes_total: prometheus_metric_value(
            &metrics,
            "dd_rust_network_mutex_raft_log_full_rewrite_bytes_total",
        )
        .unwrap_or_else(|| panic!("full-log rewrite-bytes metric missing at {endpoint}")),
    }
}

async fn assert_live_full_log_metrics_unchanged(
    endpoints: &[String],
    before: &BTreeMap<String, LiveFullLogMetrics>,
    label: &str,
) {
    let after = current_live_full_log_metrics(endpoints).await;
    assert_eq!(
        &after, before,
        "{label} should not use full-log scans, read-failure, rewrite, or rewrite-failure paths; before={before:?} after={after:?}"
    );
}

fn prometheus_metric_value(metrics: &str, name: &str) -> Option<u64> {
    let mut total = 0.0_f64;
    let mut found = false;
    for line in metrics.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(raw_name) = parts.next() else {
            continue;
        };
        let metric_name = raw_name.split('{').next().unwrap_or(raw_name);
        if metric_name != name {
            continue;
        }
        let Some(raw_value) = parts.next() else {
            continue;
        };
        let Ok(value) = raw_value.parse::<f64>() else {
            continue;
        };
        if value.is_finite() && value >= 0.0 {
            total += value;
            found = true;
        }
    }
    found.then(|| total as u64)
}

async fn http_json_retrying_unavailable(
    endpoint: &str,
    method: &str,
    path: &str,
    body: Value,
    label: &str,
    timeout: Duration,
) -> (u16, Value) {
    let started = Instant::now();
    loop {
        let latest = match try_http_json(endpoint, method, path, Some(body.clone())).await {
            Ok((status, parsed)) if status != 0 && status != 503 => return (status, parsed),
            Ok((status, parsed)) => format!("status={status} body={parsed:?}"),
            Err(err) => err,
        };
        assert!(
            started.elapsed() < timeout,
            "{label} did not receive an available BrokerRaft response before timeout; latest={latest}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn wait_for_raft_status_with_leader(endpoint: &str, timeout: Duration) -> Value {
    let started = Instant::now();
    loop {
        let latest = match try_http_json(endpoint, "GET", "/raft/status", None).await {
            Ok((200, status)) if observed_leader_id(&status).is_some() => return status,
            Ok((code, status)) => format!("status={code} body={status:?}"),
            Err(err) => err,
        };
        assert!(
            started.elapsed() < timeout,
            "timed out waiting for BrokerRaft service status with observed leader; latest={latest}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn acquire_release_with_retry(
    endpoint: &str,
    key: &str,
    timeout: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let latest = match acquire_release_once(endpoint, key).await {
            Ok(()) => return Ok(()),
            Err(err) => err,
        };
        if started.elapsed() >= timeout {
            return Err(format!(
                "timed out acquiring/releasing {key:?} through BrokerRaft service; latest={latest}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn acquire_release_once(endpoint: &str, key: &str) -> Result<(), String> {
    let (status, acquired) = try_http_json(
        endpoint,
        "POST",
        "/v1/lock",
        Some(json!({"key": key, "ttlMs": 5000})),
    )
    .await?;
    if status != 200 || acquired["acquired"] != true {
        return Err(format!(
            "acquire response status={status} body={acquired:?}"
        ));
    }
    let lock_uuid = acquired["lockUuid"]
        .as_str()
        .ok_or_else(|| format!("acquire response missing lockUuid: {acquired:?}"))?
        .to_string();
    let (status, released) = try_http_json(
        endpoint,
        "POST",
        "/v1/unlock",
        Some(json!({"key": key, "lockUuid": lock_uuid})),
    )
    .await?;
    if status != 200 || released["unlocked"] != true {
        return Err(format!(
            "release response status={status} body={released:?}"
        ));
    }
    Ok(())
}

async fn http_text(
    endpoint: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<String, String> {
    let (status, body) = http_request(endpoint, method, path, body)
        .await
        .map_err(|err| format!("HTTP request failed: {err}"))?;
    if status != 200 {
        return Err(format!("HTTP {method} {path} returned {status}: {body:?}"));
    }
    Ok(body)
}

fn assert_live_http_lock_history_linearizable(history: &[LiveHttpLockHistoryOp]) {
    assert!(
        !history.is_empty(),
        "live HTTP linearizability history should contain operations"
    );
    let mut by_key = BTreeMap::<String, Vec<LiveHttpLockHistoryOp>>::new();
    for op in history {
        by_key.entry(op.key.clone()).or_default().push(op.clone());
    }
    for (key, ops) in by_key {
        assert_live_http_linearizable_key_history(&key, &ops);
    }
}

fn assert_live_http_linearizable_key_history(key: &str, ops: &[LiveHttpLockHistoryOp]) {
    assert!(
        ops.len() < 128,
        "live linearizability checker supports fewer than 128 operations per key; key={key} ops={}",
        ops.len()
    );
    let mut lock_ids = BTreeMap::<String, u16>::new();
    let mut granted_ids = BTreeSet::<u16>::new();
    for op in ops {
        let lock_uuid = match &op.result {
            LiveHttpLockHistoryResult::Acquired { lock_uuid }
            | LiveHttpLockHistoryResult::Released { lock_uuid } => lock_uuid,
            LiveHttpLockHistoryResult::NotAcquired => continue,
        };
        if !lock_ids.contains_key(lock_uuid) {
            let next_id = u16::try_from(lock_ids.len().saturating_add(1))
                .expect("live linearizable history lock id should fit in u16");
            lock_ids.insert(lock_uuid.clone(), next_id);
        }
    }

    let model_ops = ops
        .iter()
        .map(|op| match &op.result {
            LiveHttpLockHistoryResult::Acquired { lock_uuid } => {
                let id = *lock_ids
                    .get(lock_uuid)
                    .expect("granted lock uuid should be indexed");
                assert!(
                    granted_ids.insert(id),
                    "lock uuid {lock_uuid} was granted more than once for key {key}"
                );
                LiveHttpLinearModelOp::AcquireGranted(id)
            }
            LiveHttpLockHistoryResult::NotAcquired => LiveHttpLinearModelOp::AcquireRejected,
            LiveHttpLockHistoryResult::Released { lock_uuid } => LiveHttpLinearModelOp::Release(
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
        search_live_http_linearized_history(0, 0, all_done, &model_ops, &predecessors, &mut memo),
        "no live HTTP linearization found for key {key}; ops={ops:?}"
    );
}

fn search_live_http_linearized_history(
    done: u128,
    holder: u16,
    all_done: u128,
    ops: &[LiveHttpLinearModelOp],
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
        let Some(next_holder) = apply_live_http_linear_model_op(holder, *op) else {
            continue;
        };
        if search_live_http_linearized_history(
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

fn apply_live_http_linear_model_op(holder: u16, op: LiveHttpLinearModelOp) -> Option<u16> {
    match op {
        LiveHttpLinearModelOp::AcquireGranted(lock_id) if holder == 0 => Some(lock_id),
        LiveHttpLinearModelOp::AcquireGranted(_) => None,
        LiveHttpLinearModelOp::AcquireRejected if holder != 0 => Some(holder),
        LiveHttpLinearModelOp::AcquireRejected => None,
        LiveHttpLinearModelOp::Release(lock_id) if holder == lock_id => Some(0),
        LiveHttpLinearModelOp::Release(_) => None,
    }
}

fn observed_leader_id(status: &Value) -> Option<String> {
    status
        .get("leaderId")
        .and_then(Value::as_str)
        .filter(|leader| !leader.is_empty())
        .map(str::to_string)
        .or_else(|| {
            (status.get("isLeader").and_then(Value::as_bool) == Some(true))
                .then(|| {
                    status
                        .get("nodeId")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .flatten()
        })
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn run_kubectl(args: &[&str]) {
    let output = run_kubectl_output(args);
    assert!(
        output.status.success(),
        "kubectl command failed: {} {}\nstdout:\n{}\nstderr:\n{}",
        env::var("KUBECTL").unwrap_or_else(|_| "kubectl".into()),
        args.join(" "),
        kubectl_output_preview(&output.stdout),
        kubectl_output_preview(&output.stderr)
    );
}

fn run_kubectl_output(args: &[&str]) -> std::process::Output {
    let kubectl = env::var("KUBECTL").unwrap_or_else(|_| "kubectl".into());
    let mut command = Command::new(&kubectl);
    if let Ok(context) = env::var("LMX_LIVE_RAFT_KUBE_CONTEXT") {
        if !context.is_empty() {
            command.arg("--context").arg(context);
        }
    }
    if let Some(timeout_arg) = kubectl_request_timeout_arg() {
        command.arg(timeout_arg);
    }
    command.args(args);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let process_timeout = kubectl_process_timeout();
    let started = Instant::now();
    let mut child = command
        .spawn()
        .unwrap_or_else(|err| panic!("failed to spawn {kubectl}: {err}"));

    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .unwrap_or_else(|err| panic!("failed to collect {kubectl} output: {err}"));
            }
            Ok(None) if started.elapsed() >= process_timeout => {
                let _ = child.kill();
                let output = child.wait_with_output().unwrap_or_else(|err| {
                    panic!("failed to collect timed out {kubectl} output: {err}")
                });
                panic!(
                    "kubectl command timed out after {process_timeout:?}: {} {}\nstdout:\n{}\nstderr:\n{}",
                    kubectl,
                    args.join(" "),
                    kubectl_output_preview(&output.stdout),
                    kubectl_output_preview(&output.stderr)
                );
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(err) => panic!("failed to wait for {kubectl}: {err}"),
        }
    }
}

fn kubectl_output_preview(output: &[u8]) -> String {
    let output = String::from_utf8_lossy(output);
    if output.len() <= KUBECTL_OUTPUT_PREVIEW_BYTES {
        return output.into_owned();
    }

    let mut preview = String::new();
    for (idx, ch) in output.char_indices() {
        if idx >= KUBECTL_OUTPUT_PREVIEW_BYTES {
            break;
        }
        preview.push(ch);
    }
    format!("{preview}\n... truncated after {KUBECTL_OUTPUT_PREVIEW_BYTES} bytes ...")
}

fn kubectl_pod_uid(namespace: &str, pod: &str) -> String {
    let output = run_kubectl_output(&[
        "-n",
        namespace,
        "get",
        "pod",
        pod,
        "-o",
        "jsonpath={.metadata.uid}",
    ]);
    assert!(
        output.status.success(),
        "failed to read BrokerRaft leader pod UID for {pod}\nstdout:\n{}\nstderr:\n{}",
        kubectl_output_preview(&output.stdout),
        kubectl_output_preview(&output.stderr)
    );
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(
        !uid.is_empty(),
        "BrokerRaft leader pod {pod} should have a non-empty UID"
    );
    uid
}

fn wait_for_pod_uid_change(namespace: &str, pod: &str, old_uid: &str, timeout: Duration) {
    let started = Instant::now();
    loop {
        let output = run_kubectl_output(&[
            "-n",
            namespace,
            "get",
            "pod",
            pod,
            "-o",
            "jsonpath={.metadata.uid}",
        ]);
        if output.status.success() {
            let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !uid.is_empty() && uid != old_uid {
                return;
            }
        }
        assert!(
            started.elapsed() < timeout,
            "timed out waiting for BrokerRaft leader pod {pod} UID to change from {old_uid}"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
}

async fn http_request(
    endpoint: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> std::io::Result<(u16, String)> {
    let timeout = live_http_request_timeout();
    match tokio::time::timeout(timeout, http_request_once(endpoint, method, path, body)).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("live BrokerRaft HTTP request timed out after {timeout:?}: {method} {endpoint}{path}"),
        )),
    }
}

async fn http_request_once(
    endpoint: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> std::io::Result<(u16, String)> {
    let (host, port) = parse_host_port(endpoint);
    let body = body
        .map(|value| serde_json::to_vec(&value).unwrap())
        .unwrap_or_default();
    let auth = env::var("LMX_LIVE_RAFT_AUTH_TOKEN")
        .ok()
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Connection: close\r\n\
         Content-Type: application/json\r\n\
         {auth}\
         Content-Length: {}\r\n\
         \r\n",
        body.len()
    );

    let mut stream = TcpStream::connect((host.as_str(), port)).await?;
    stream.write_all(request.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await?;

    let mut raw = String::new();
    stream.read_to_string(&mut raw).await?;
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

fn parse_host_port(endpoint: &str) -> (String, u16) {
    let endpoint = endpoint
        .strip_prefix("http://")
        .unwrap_or(endpoint)
        .trim_end_matches('/');
    let (host, port) = endpoint
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("LMX_LIVE_RAFT_HTTP must be host:port, got {endpoint:?}"));
    let port = port
        .parse::<u16>()
        .unwrap_or_else(|_| panic!("invalid port in LMX_LIVE_RAFT_HTTP: {endpoint:?}"));
    (host.to_string(), port)
}

fn uuid_short() -> String {
    let s = uuid::Uuid::new_v4().to_string();
    s.split('-').next().unwrap().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct TestEnvVarGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl TestEnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = env::var_os(key);
            env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for TestEnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                env::set_var(self.key, previous);
            } else {
                env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn parse_live_metrics_endpoints_trims_and_drops_empty_entries() {
        assert_eq!(
            parse_live_metrics_endpoints(" pod-0:6971, ,pod-1:6971,, pod-2:6971 "),
            vec![
                "pod-0:6971".to_string(),
                "pod-1:6971".to_string(),
                "pod-2:6971".to_string(),
            ]
        );
        assert!(parse_live_metrics_endpoints(" , , ").is_empty());
    }

    #[test]
    fn live_metrics_endpoint_set_requires_one_endpoint_per_expected_node() {
        assert_live_metrics_endpoint_set_covers_expected_cluster(
            &[
                "pod-0:6971".to_string(),
                "pod-1:6971".to_string(),
                "pod-2:6971".to_string(),
            ],
            3,
        );
    }

    #[test]
    #[should_panic(expected = "did not contain any endpoints")]
    fn live_metrics_endpoint_set_rejects_empty_when_env_was_set() {
        assert_live_metrics_endpoint_set_covers_expected_cluster(&[], 3);
    }

    #[test]
    #[should_panic(expected = "exactly one stable HTTP endpoint per expected Raft node")]
    fn live_metrics_endpoint_set_rejects_partial_cluster_coverage() {
        assert_live_metrics_endpoint_set_covers_expected_cluster(&["pod-0:6971".to_string()], 3);
    }

    #[test]
    #[should_panic(expected = "must not contain duplicate endpoints")]
    fn live_metrics_endpoint_set_rejects_duplicates() {
        assert_live_metrics_endpoint_set_covers_expected_cluster(
            &[
                "pod-0:6971".to_string(),
                "pod-1:6971".to_string(),
                "pod-1:6971".to_string(),
            ],
            3,
        );
    }

    #[test]
    fn parse_live_positive_u64_accepts_positive_values() {
        assert_eq!(parse_live_positive_u64("TEST_VALUE", "5"), 5);
    }

    #[test]
    #[should_panic(expected = "TEST_VALUE must be positive")]
    fn parse_live_positive_u64_rejects_zero() {
        parse_live_positive_u64("TEST_VALUE", "0");
    }

    #[test]
    #[should_panic(expected = "TEST_VALUE must be a positive integer")]
    fn parse_live_positive_u64_rejects_non_numeric_values() {
        parse_live_positive_u64("TEST_VALUE", "five");
    }

    #[test]
    fn live_http_request_timeout_defaults_and_parses_millis() {
        assert_eq!(
            live_http_request_timeout_from(None),
            Duration::from_millis(DEFAULT_LIVE_HTTP_REQUEST_TIMEOUT_MS)
        );
        assert_eq!(
            live_http_request_timeout_from(Some("2500")),
            Duration::from_millis(2500)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_http_request_times_out_against_half_open_service() {
        let _env_lock = TEST_ENV_LOCK.lock().await;
        let _timeout = TestEnvVarGuard::set("LMX_LIVE_RAFT_HTTP_REQUEST_TIMEOUT_MS", "40");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind half-open HTTP listener");
        let endpoint = listener.local_addr().expect("listener addr").to_string();

        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.expect("accept half-open client");
            tokio::time::sleep(Duration::from_millis(250)).await;
        });

        let started = Instant::now();
        let err = http_request(&endpoint, "GET", "/raft/status", None)
            .await
            .expect_err("half-open live HTTP request should time out");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "half-open live HTTP request should fail near the configured timeout, not wait for server close"
        );

        server.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[should_panic(expected = "kubectl command timed out after")]
    async fn kubectl_process_timeout_kills_stuck_child() {
        let _env_lock = TEST_ENV_LOCK.lock().await;
        let _kubectl = TestEnvVarGuard::set("KUBECTL", "/bin/sleep");
        let _context = TestEnvVarGuard::set("LMX_LIVE_RAFT_KUBE_CONTEXT", "");
        let _request_timeout =
            TestEnvVarGuard::set("LMX_LIVE_RAFT_KUBECTL_REQUEST_TIMEOUT", "none");
        let _process_timeout =
            TestEnvVarGuard::set("LMX_LIVE_RAFT_KUBECTL_PROCESS_TIMEOUT_MS", "40");

        let _ = run_kubectl_output(&["5"]);
    }

    #[test]
    fn kubectl_request_timeout_arg_defaults_to_fail_fast() {
        assert_eq!(
            kubectl_request_timeout_arg_from(None),
            Some("--request-timeout=5s".to_string())
        );
        assert_eq!(
            kubectl_request_timeout_arg_from(Some(" 2s ")),
            Some("--request-timeout=2s".to_string())
        );
    }

    #[test]
    fn kubectl_request_timeout_arg_can_be_disabled_explicitly() {
        assert_eq!(kubectl_request_timeout_arg_from(Some("")), None);
        assert_eq!(kubectl_request_timeout_arg_from(Some("0")), None);
        assert_eq!(kubectl_request_timeout_arg_from(Some("none")), None);
    }

    #[test]
    fn kubectl_process_timeout_defaults_and_parses_millis() {
        assert_eq!(
            kubectl_process_timeout_from(None),
            Duration::from_millis(DEFAULT_KUBECTL_PROCESS_TIMEOUT_MS)
        );
        assert_eq!(
            kubectl_process_timeout_from(Some("250")),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn kubectl_output_preview_truncates_large_outputs() {
        let output = vec![b'x'; KUBECTL_OUTPUT_PREVIEW_BYTES + 10];
        let preview = kubectl_output_preview(&output);
        assert!(preview.contains("truncated after 4096 bytes"));
        assert!(preview.starts_with(&"x".repeat(KUBECTL_OUTPUT_PREVIEW_BYTES)));
        assert!(!preview.starts_with(&"x".repeat(KUBECTL_OUTPUT_PREVIEW_BYTES + 1)));
    }

    #[test]
    fn assert_live_raft_cluster_status_accepts_configurable_values() {
        let status = json!({
            "clusterSize": 5,
            "quorumSize": 3,
        });
        assert_live_raft_cluster_status(&status, 5, 3, "test status");
    }

    #[test]
    fn prometheus_metric_value_sums_labeled_series_and_ignores_bad_values() {
        let metrics = "\
# HELP ignored help\n\
dd_rust_network_mutex_raft_log_full_reads_total{node=\"a\"} 1\n\
dd_rust_network_mutex_raft_log_full_reads_total{node=\"b\"} 2\n\
dd_rust_network_mutex_raft_log_full_reads_total NaN\n\
dd_rust_network_mutex_raft_log_full_read_failures_total 0\n\
dd_rust_network_mutex_raft_log_full_read_bytes_total 10\n\
dd_rust_network_mutex_raft_log_full_read_entries_total 4\n\
dd_rust_network_mutex_raft_log_full_rewrites_total 0\n\
dd_rust_network_mutex_raft_log_full_rewrite_failures_total 0\n\
dd_rust_network_mutex_raft_log_full_rewrite_entries_total 0\n\
dd_rust_network_mutex_raft_log_full_rewrite_bytes_total 0\n\
bad_line\n";

        assert_eq!(
            prometheus_metric_value(metrics, "dd_rust_network_mutex_raft_log_full_reads_total"),
            Some(3)
        );
        assert_eq!(
            prometheus_metric_value(
                metrics,
                "dd_rust_network_mutex_raft_log_full_rewrites_total"
            ),
            Some(0)
        );
        assert_eq!(
            prometheus_metric_value(
                metrics,
                "dd_rust_network_mutex_raft_log_full_read_bytes_total"
            ),
            Some(10)
        );
        assert_eq!(
            prometheus_metric_value(
                metrics,
                "dd_rust_network_mutex_raft_log_full_read_failures_total"
            ),
            Some(0)
        );
        assert_eq!(
            prometheus_metric_value(
                metrics,
                "dd_rust_network_mutex_raft_log_full_rewrite_failures_total"
            ),
            Some(0)
        );
        assert_eq!(prometheus_metric_value(metrics, "missing_metric"), None);
    }
}
