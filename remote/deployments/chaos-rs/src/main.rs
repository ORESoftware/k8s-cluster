//! dd-chaos — cluster chaos-engineering loops.
//!
//! Three independent tokio loops, each gated by safety guards:
//!   - pod-kill          randomly deletes a bounded number of Running pods in target
//!                       namespaces (skips protected namespaces + protected-labelled pods).
//!   - deployment-jitter removes one replica from an allow-listed Deployment, holds, and
//!                       restores it — exercising recovery from partial replica loss.
//!   - nats-probe        a request/reply responder + prober that measures live NATS RTT
//!                       and jitter against the cluster server.
//!
//! Every destructive action is published as an auditable `ChaosExperiments` record and a
//! `ChaosEvents` lifecycle event. Guards: a global kill switch (`CHAOS_ENABLED`, default
//! off), a dry-run default (`CHAOS_DRY_RUN`, default on), a per-tick blast-radius cap, and
//! a protected-namespace blocklist enforced in code regardless of RBAC.
//!
//! The Kubernetes API is reached with a raw `reqwest` client + ServiceAccount bearer
//! token, mirroring `idle-reaper-rs` (no kube-rs dependency). True network partitioning
//! needs a CNI/mesh fault layer (e.g. Chaos Mesh); this service covers pod/deployment-
//! level faults plus NATS latency observation.

use std::env;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::{
    extract::State,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use futures_util::StreamExt;
use reqwest::{Certificate, Client};
use serde_json::{json, Value};

use dd_nats_subject_defs::{
    CHAOS_EVENTS_SUBJECT, CHAOS_EXPERIMENTS_SUBJECT, CHAOS_PROBE_QUEUE_GROUP, CHAOS_PROBE_SUBJECT,
};

const SERVICE_NAME: &str = "dd-chaos";

#[derive(Clone)]
struct Config {
    enabled: bool,
    dry_run: bool,
    namespaces: Vec<String>,
    protected_namespaces: Vec<String>,
    protected_label: String,
    pod_label_selector: String,
    blast_radius: usize,
    pod_kill_enabled: bool,
    pod_kill_interval: Duration,
    jitter_enabled: bool,
    jitter_targets: Vec<(String, String)>,
    jitter_interval: Duration,
    jitter_hold: Duration,
    jitter_min_replicas: i64,
    probe_enabled: bool,
    probe_interval: Duration,
    k8s_timeout: Duration,
}

#[derive(Default)]
struct Metrics {
    pod_kills_total: AtomicU64,
    pods_targeted_total: AtomicU64,
    jitter_events_total: AtomicU64,
    probe_samples_total: AtomicU64,
    probe_rtt_micros: AtomicU64,
    probe_jitter_micros: AtomicU64,
    guard_aborts_total: AtomicU64,
    errors_total: AtomicU64,
}

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    experiments_subject: String,
    events_subject: String,
    probe_subject: String,
    config: Config,
    metrics: Arc<Metrics>,
    node_id: String,
}

// ---------------------------------------------------------------------------
// Deterministic RNG (xorshift64*) — seeded from the wall clock, no `rand` dep.
// ---------------------------------------------------------------------------

struct Rng {
    state: u64,
}

impl Rng {
    fn from_clock() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x1234_5678);
        Self {
            state: nanos | 1,
        }
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Fisher-Yates partial shuffle: move `take` random elements to the front.
    fn sample<T>(&mut self, items: &mut Vec<T>, take: usize) {
        let n = items.len();
        let take = take.min(n);
        for i in 0..take {
            let j = i + (self.next_u64() as usize) % (n - i);
            items.swap(i, j);
        }
        items.truncate(take);
    }
}

// ---------------------------------------------------------------------------
// Kubernetes API (raw reqwest + ServiceAccount bearer token)
// ---------------------------------------------------------------------------

async fn k8s_client(timeout: Duration) -> Result<(Client, String, String), String> {
    let host = env_value("KUBERNETES_SERVICE_HOST", "kubernetes.default.svc");
    let port = env_value("KUBERNETES_SERVICE_PORT", "443");
    let token = match env::var("K8S_SA_TOKEN").ok().filter(|t| !t.trim().is_empty()) {
        Some(token) => token,
        None => tokio::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/token")
            .await
            .map_err(|error| format!("read service account token: {error}"))?,
    };
    let mut builder = Client::builder().timeout(timeout);
    if let Ok(ca) = tokio::fs::read("/var/run/secrets/kubernetes.io/serviceaccount/ca.crt").await {
        if let Ok(cert) = Certificate::from_pem(&ca) {
            builder = builder.add_root_certificate(cert);
        }
    }
    let client = builder
        .build()
        .map_err(|error| format!("build k8s client: {error}"))?;
    Ok((client, format!("https://{host}:{port}"), token))
}

async fn k8s_get(client: &Client, base: &str, token: &str, path: &str) -> Result<Value, String> {
    let response = client
        .get(format!("{base}{path}"))
        .bearer_auth(token.trim())
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| format!("k8s GET {path}: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("k8s GET {path} -> {}", response.status()));
    }
    response
        .json()
        .await
        .map_err(|error| format!("k8s GET {path} decode: {error}"))
}

async fn k8s_delete(client: &Client, base: &str, token: &str, path: &str) -> Result<(), String> {
    let response = client
        .delete(format!("{base}{path}"))
        .bearer_auth(token.trim())
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| format!("k8s DELETE {path}: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("k8s DELETE {path} -> {}", response.status()));
    }
    Ok(())
}

async fn k8s_patch_scale(
    client: &Client,
    base: &str,
    token: &str,
    path: &str,
    replicas: i64,
) -> Result<(), String> {
    let response = client
        .patch(format!("{base}{path}"))
        .bearer_auth(token.trim())
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/merge-patch+json")
        .json(&json!({ "spec": { "replicas": replicas } }))
        .send()
        .await
        .map_err(|error| format!("k8s PATCH {path}: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("k8s PATCH {path} -> {}", response.status()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Pod-kill loop
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct PodTarget {
    namespace: String,
    name: String,
}

async fn pod_kill_loop(state: AppState) {
    let config = &state.config;
    println!(
        "{SERVICE_NAME} pod-kill loop: interval={}s blastRadius={} namespaces={:?} dryRun={} enabled={}",
        config.pod_kill_interval.as_secs(),
        config.blast_radius,
        config.namespaces,
        config.dry_run,
        config.enabled,
    );
    loop {
        tokio::time::sleep(config.pod_kill_interval).await;
        if let Err(error) = pod_kill_tick(&state).await {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!("{SERVICE_NAME} pod-kill tick failed: {error}");
        }
    }
}

async fn pod_kill_tick(state: &AppState) -> Result<(), String> {
    let config = &state.config;
    let (client, base, token) = k8s_client(config.k8s_timeout).await?;

    let mut candidates: Vec<PodTarget> = Vec::new();
    for namespace in &config.namespaces {
        if is_protected_namespace(config, namespace) {
            continue;
        }
        let mut path = format!("/api/v1/namespaces/{namespace}/pods");
        if !config.pod_label_selector.is_empty() {
            path.push_str(&format!("?labelSelector={}", config.pod_label_selector));
        }
        let list = match k8s_get(&client, &base, &token, &path).await {
            Ok(list) => list,
            Err(error) => {
                eprintln!("{SERVICE_NAME} list pods in {namespace} failed: {error}");
                continue;
            }
        };
        for item in list["items"].as_array().into_iter().flatten() {
            if pod_is_eligible(config, item) {
                candidates.push(PodTarget {
                    namespace: namespace.clone(),
                    name: item["metadata"]["name"].as_str().unwrap_or_default().to_string(),
                });
            }
        }
    }

    if candidates.is_empty() {
        return Ok(());
    }

    let mut rng = Rng::from_clock();
    rng.sample(&mut candidates, config.blast_radius);
    state
        .metrics
        .pods_targeted_total
        .fetch_add(candidates.len() as u64, Ordering::Relaxed);

    let experiment_id = format!("chaos-{}", now_ms());
    publish_experiment(
        state,
        &experiment_id,
        "pod-kill",
        json!({
            "targets": candidates.iter().map(|p| json!({"namespace": p.namespace, "pod": p.name})).collect::<Vec<_>>(),
            "blastRadius": config.blast_radius,
            "dryRun": config.dry_run || !config.enabled,
        }),
    )
    .await;

    for target in &candidates {
        let armed = config.enabled && !config.dry_run;
        publish_event(
            state,
            "pod-selected",
            json!({"experimentId": &experiment_id, "namespace": target.namespace, "pod": target.name, "armed": armed}),
        )
        .await;
        if !armed {
            state.metrics.guard_aborts_total.fetch_add(1, Ordering::Relaxed);
            println!(
                "{SERVICE_NAME} would kill pod {}/{} (guarded: enabled={} dryRun={})",
                target.namespace, target.name, config.enabled, config.dry_run
            );
            continue;
        }
        let path = format!("/api/v1/namespaces/{}/pods/{}", target.namespace, target.name);
        match k8s_delete(&client, &base, &token, &path).await {
            Ok(()) => {
                state.metrics.pod_kills_total.fetch_add(1, Ordering::Relaxed);
                publish_event(
                    state,
                    "pod-killed",
                    json!({"experimentId": &experiment_id, "namespace": target.namespace, "pod": target.name}),
                )
                .await;
                println!("{SERVICE_NAME} killed pod {}/{}", target.namespace, target.name);
            }
            Err(error) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                eprintln!("{SERVICE_NAME} delete pod {}/{} failed: {error}", target.namespace, target.name);
            }
        }
    }
    Ok(())
}

fn pod_is_eligible(config: &Config, item: &Value) -> bool {
    if item["status"]["phase"].as_str() != Some("Running") {
        return false;
    }
    if item["metadata"]["deletionTimestamp"].is_string() {
        return false;
    }
    let labels = &item["metadata"]["labels"];
    if labels[&config.protected_label].as_str().is_some_and(|v| v == "true") {
        return false;
    }
    item["metadata"]["name"].as_str().is_some_and(|name| !name.is_empty())
}

fn is_protected_namespace(config: &Config, namespace: &str) -> bool {
    config
        .protected_namespaces
        .iter()
        .any(|protected| protected == namespace)
}

// ---------------------------------------------------------------------------
// Deployment-jitter loop
// ---------------------------------------------------------------------------

async fn deployment_jitter_loop(state: AppState) {
    let config = &state.config;
    println!(
        "{SERVICE_NAME} jitter loop: interval={}s hold={}s targets={:?} dryRun={} enabled={}",
        config.jitter_interval.as_secs(),
        config.jitter_hold.as_secs(),
        config.jitter_targets,
        config.dry_run,
        config.enabled,
    );
    loop {
        tokio::time::sleep(config.jitter_interval).await;
        if let Err(error) = deployment_jitter_tick(&state).await {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!("{SERVICE_NAME} jitter tick failed: {error}");
        }
    }
}

async fn deployment_jitter_tick(state: &AppState) -> Result<(), String> {
    let config = &state.config;
    if config.jitter_targets.is_empty() {
        return Ok(());
    }
    let mut rng = Rng::from_clock();
    let pick = (rng.next_u64() as usize) % config.jitter_targets.len();
    let (namespace, name) = config.jitter_targets[pick].clone();
    if is_protected_namespace(config, &namespace) {
        return Ok(());
    }

    let (client, base, token) = k8s_client(config.k8s_timeout).await?;
    let path = format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}");
    let deployment = k8s_get(&client, &base, &token, &path).await?;
    let original = deployment["spec"]["replicas"].as_i64().unwrap_or(1);
    let reduced = (original - 1).max(config.jitter_min_replicas);

    let experiment_id = format!("chaos-{}", now_ms());
    let armed = config.enabled && !config.dry_run && reduced < original;
    publish_experiment(
        state,
        &experiment_id,
        "deployment-jitter",
        json!({
            "namespace": namespace,
            "deployment": name,
            "originalReplicas": original,
            "reducedReplicas": reduced,
            "holdSeconds": config.jitter_hold.as_secs(),
            "armed": armed,
        }),
    )
    .await;

    if !armed {
        state.metrics.guard_aborts_total.fetch_add(1, Ordering::Relaxed);
        println!(
            "{SERVICE_NAME} would jitter {namespace}/{name} {original}->{reduced} (guarded: enabled={} dryRun={})",
            config.enabled, config.dry_run
        );
        return Ok(());
    }

    let scale_path = format!("{path}/scale");
    k8s_patch_scale(&client, &base, &token, &scale_path, reduced).await?;
    state.metrics.jitter_events_total.fetch_add(1, Ordering::Relaxed);
    publish_event(
        state,
        "replica-removed",
        json!({"experimentId": &experiment_id, "namespace": namespace, "deployment": name, "replicas": reduced}),
    )
    .await;
    println!("{SERVICE_NAME} jittered {namespace}/{name} {original}->{reduced}, restoring in {}s", config.jitter_hold.as_secs());

    tokio::time::sleep(config.jitter_hold).await;

    // Best-effort restore on a fresh client (token/cert may have rotated during the hold).
    match k8s_client(config.k8s_timeout).await {
        Ok((client, base, token)) => {
            if let Err(error) = k8s_patch_scale(&client, &base, &token, &scale_path, original).await {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                eprintln!("{SERVICE_NAME} restore {namespace}/{name} to {original} failed: {error}");
            } else {
                publish_event(
                    state,
                    "replica-restored",
                    json!({"experimentId": &experiment_id, "namespace": namespace, "deployment": name, "replicas": original}),
                )
                .await;
            }
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!("{SERVICE_NAME} restore client build failed: {error}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// NATS latency probe (responder + prober)
// ---------------------------------------------------------------------------

async fn probe_responder(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        return;
    };
    loop {
        let mut subscription = match nats
            .queue_subscribe(state.probe_subject.clone(), CHAOS_PROBE_QUEUE_GROUP.to_string())
            .await
        {
            Ok(subscription) => subscription,
            Err(error) => {
                eprintln!("{SERVICE_NAME} probe responder subscribe failed: {error}; retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        while let Some(message) = subscription.next().await {
            if let Some(reply) = message.reply {
                let _ = nats.publish(reply, message.payload).await;
            }
        }
        eprintln!("{SERVICE_NAME} probe responder subscription ended; re-subscribing in 5s");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn probe_loop(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        return;
    };
    let interval = state.config.probe_interval;
    println!("{SERVICE_NAME} nats-probe loop: interval={}s subject={}", interval.as_secs(), state.probe_subject);
    let mut last_rtt_micros: Option<u64> = None;
    loop {
        tokio::time::sleep(interval).await;
        let started = Instant::now();
        let payload = json!({"ping": now_ms(), "node": state.node_id}).to_string();
        match tokio::time::timeout(
            Duration::from_secs(5),
            nats.request(state.probe_subject.clone(), payload.into()),
        )
        .await
        {
            Ok(Ok(_)) => {
                let rtt = started.elapsed().as_micros() as u64;
                let jitter = last_rtt_micros.map(|prev| prev.abs_diff(rtt)).unwrap_or(0);
                last_rtt_micros = Some(rtt);
                state.metrics.probe_samples_total.fetch_add(1, Ordering::Relaxed);
                state.metrics.probe_rtt_micros.store(rtt, Ordering::Relaxed);
                state.metrics.probe_jitter_micros.store(jitter, Ordering::Relaxed);
                publish_event(
                    &state,
                    "nats-probe",
                    json!({"rttMicros": rtt, "jitterMicros": jitter}),
                )
                .await;
            }
            Ok(Err(error)) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                eprintln!("{SERVICE_NAME} nats probe request failed: {error}");
            }
            Err(_) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                eprintln!("{SERVICE_NAME} nats probe timed out");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// NATS publish helpers
// ---------------------------------------------------------------------------

async fn publish_experiment(state: &AppState, experiment_id: &str, kind: &str, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let record = json!({
        "schema": "dd.chaos.experiment.v1",
        "service": SERVICE_NAME,
        "nodeId": state.node_id,
        "experimentId": experiment_id,
        "kind": kind,
        "dryRun": state.config.dry_run || !state.config.enabled,
        "payload": payload,
        "timeMs": now_ms(),
    });
    if let Ok(bytes) = serde_json::to_vec(&record) {
        let _ = nats.publish(state.experiments_subject.clone(), bytes.into()).await;
    }
}

async fn publish_event(state: &AppState, event_name: &str, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let event = json!({
        "schema": "dd.chaos.event.v1",
        "service": SERVICE_NAME,
        "nodeId": state.node_id,
        "eventName": event_name,
        "payload": payload,
        "timeMs": now_ms(),
    });
    if let Ok(bytes) = serde_json::to_vec(&event) {
        let _ = nats.publish(state.events_subject.clone(), bytes.into()).await;
    }
}

// ---------------------------------------------------------------------------
// HTTP (health + metrics)
// ---------------------------------------------------------------------------

async fn healthz() -> impl IntoResponse {
    Json(json!({"ok": true, "service": SERVICE_NAME}))
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "enabled": state.config.enabled,
        "dryRun": state.config.dry_run,
        "nats": state.nats.is_some(),
    }))
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let m = &state.metrics;
    let armed = if state.config.enabled && !state.config.dry_run { 1 } else { 0 };
    let body = format!(
        concat!(
            "# HELP dd_chaos_armed Whether destructive actions are live (1) or guarded (0).\n",
            "# TYPE dd_chaos_armed gauge\n",
            "dd_chaos_armed {}\n",
            "# HELP dd_chaos_pod_kills_total Pods actually deleted.\n",
            "# TYPE dd_chaos_pod_kills_total counter\n",
            "dd_chaos_pod_kills_total {}\n",
            "# HELP dd_chaos_pods_targeted_total Pods selected as kill candidates.\n",
            "# TYPE dd_chaos_pods_targeted_total counter\n",
            "dd_chaos_pods_targeted_total {}\n",
            "# HELP dd_chaos_jitter_events_total Deployment replica-jitter actions applied.\n",
            "# TYPE dd_chaos_jitter_events_total counter\n",
            "dd_chaos_jitter_events_total {}\n",
            "# HELP dd_chaos_probe_samples_total NATS RTT probe samples taken.\n",
            "# TYPE dd_chaos_probe_samples_total counter\n",
            "dd_chaos_probe_samples_total {}\n",
            "# HELP dd_chaos_probe_rtt_seconds Last NATS request/reply round-trip time.\n",
            "# TYPE dd_chaos_probe_rtt_seconds gauge\n",
            "dd_chaos_probe_rtt_seconds {:.6}\n",
            "# HELP dd_chaos_probe_jitter_seconds Last NATS RTT jitter (delta vs previous sample).\n",
            "# TYPE dd_chaos_probe_jitter_seconds gauge\n",
            "dd_chaos_probe_jitter_seconds {:.6}\n",
            "# HELP dd_chaos_guard_aborts_total Actions skipped by the kill switch / dry-run guard.\n",
            "# TYPE dd_chaos_guard_aborts_total counter\n",
            "dd_chaos_guard_aborts_total {}\n",
            "# HELP dd_chaos_errors_total Errors across k8s and NATS paths.\n",
            "# TYPE dd_chaos_errors_total counter\n",
            "dd_chaos_errors_total {}\n",
        ),
        armed,
        m.pod_kills_total.load(Ordering::Relaxed),
        m.pods_targeted_total.load(Ordering::Relaxed),
        m.jitter_events_total.load(Ordering::Relaxed),
        m.probe_samples_total.load(Ordering::Relaxed),
        m.probe_rtt_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0,
        m.probe_jitter_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0,
        m.guard_aborts_total.load(Ordering::Relaxed),
        m.errors_total.load(Ordering::Relaxed),
    );
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        body,
    )
}

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn env_value(key: &str, default: &str) -> String {
    env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn env_bool(key: &str, default: bool) -> bool {
    match env::var(key).ok() {
        Some(value) => matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        None => default,
    }
}

fn env_csv(key: &str, default: &str) -> Vec<String> {
    env_value(key, default)
        .split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// Parse `namespace/name` deployment targets, dropping malformed entries.
fn parse_targets(raw: &[String]) -> Vec<(String, String)> {
    raw.iter()
        .filter_map(|item| item.split_once('/'))
        .map(|(ns, name)| (ns.trim().to_string(), name.trim().to_string()))
        .filter(|(ns, name)| !ns.is_empty() && !name.is_empty())
        .collect()
}

fn load_config() -> Config {
    Config {
        enabled: env_bool("CHAOS_ENABLED", false),
        dry_run: env_bool("CHAOS_DRY_RUN", true),
        namespaces: env_csv("CHAOS_NAMESPACES", "ai-ml,dd-dev"),
        protected_namespaces: env_csv(
            "CHAOS_PROTECTED_NAMESPACES",
            "kube-system,kube-public,kube-node-lease,messaging,default",
        ),
        protected_label: env_value("CHAOS_PROTECTED_LABEL", "dd.dev/chaos-protected"),
        pod_label_selector: env_value("CHAOS_POD_LABEL_SELECTOR", ""),
        blast_radius: env_u64("CHAOS_BLAST_RADIUS", 1).max(1) as usize,
        pod_kill_enabled: env_bool("CHAOS_POD_KILL_ENABLED", true),
        pod_kill_interval: Duration::from_secs(env_u64("CHAOS_POD_KILL_INTERVAL_SECONDS", 300).max(10)),
        jitter_enabled: env_bool("CHAOS_JITTER_ENABLED", false),
        jitter_targets: parse_targets(&env_csv("CHAOS_JITTER_DEPLOYMENTS", "")),
        jitter_interval: Duration::from_secs(env_u64("CHAOS_JITTER_INTERVAL_SECONDS", 600).max(30)),
        jitter_hold: Duration::from_secs(env_u64("CHAOS_JITTER_HOLD_SECONDS", 60).max(1)),
        jitter_min_replicas: env_u64("CHAOS_JITTER_MIN_REPLICAS", 1) as i64,
        probe_enabled: env_bool("CHAOS_PROBE_ENABLED", true),
        probe_interval: Duration::from_secs(env_u64("CHAOS_PROBE_INTERVAL_SECONDS", 60).max(5)),
        k8s_timeout: Duration::from_secs(env_u64("CHAOS_K8S_TIMEOUT_SECONDS", 15).clamp(3, 60)),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8133").parse::<u16>()?;
    let node_id = env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "dd-chaos".to_string());
    let config = load_config();

    let nats = match env::var("NATS_URL").ok().filter(|v| !v.trim().is_empty()) {
        Some(url) => match async_nats::connect(&url).await {
            Ok(client) => {
                println!("{SERVICE_NAME} connected to NATS at {url}");
                Some(client)
            }
            Err(error) => {
                eprintln!("{SERVICE_NAME} NATS connect failed ({error}); continuing without it");
                None
            }
        },
        None => None,
    };

    let state = AppState {
        nats,
        experiments_subject: env_value("CHAOS_EXPERIMENTS_SUBJECT", CHAOS_EXPERIMENTS_SUBJECT),
        events_subject: env_value("CHAOS_EVENTS_SUBJECT", CHAOS_EVENTS_SUBJECT),
        probe_subject: env_value("CHAOS_PROBE_SUBJECT", CHAOS_PROBE_SUBJECT),
        config,
        metrics: Arc::new(Metrics::default()),
        node_id,
    };

    println!(
        "{SERVICE_NAME} starting (enabled={} dryRun={}) — destructive actions are {}",
        state.config.enabled,
        state.config.dry_run,
        if state.config.enabled && !state.config.dry_run { "ARMED" } else { "guarded" }
    );

    if state.config.pod_kill_enabled {
        tokio::spawn(pod_kill_loop(state.clone()));
    }
    if state.config.jitter_enabled {
        tokio::spawn(deployment_jitter_loop(state.clone()));
    }
    if state.config.probe_enabled && state.nats.is_some() {
        tokio::spawn(probe_responder(state.clone()));
        tokio::spawn(probe_loop(state.clone()));
    }

    let app = Router::new()
        .route("/", get(readyz))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    println!("{SERVICE_NAME} listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
