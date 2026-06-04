use std::{
    collections::{HashMap, HashSet},
    env,
    os::unix::fs::PermissionsExt,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::{TimeZone, Utc};
use chrono_tz::Tz;
use dd_nats_subject_defs::{
    DD_REMOTE_TASKS_STREAM_NAME, RUNTIME_EVENTS_SUBJECT, THREAD_PREPARER_QUEUE_GROUP,
    THREAD_TASKS_WILDCARD,
};
use dd_shared_interfaces::AgentTaskQueueMessage;
use futures_util::StreamExt;
use reqwest::{Certificate, Client};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{fs, process::Command, sync::Mutex, time::sleep};

const CLUSTER_DOCTOR_PROMPT: &str = r#"
You are the scheduled cluster doctor for ORESoftware/k8s-cluster.

Goal: every run should inspect the EC2 Kubernetes runtime, find concrete
reliability/observability/deployment problems, make a small safe fix in this
repo when there is an actionable issue, run targeted tests, and let the
remote-dev server submit the PR.

Telemetry and runtime surfaces available from inside the cluster:
- Prometheus API: http://dd-prometheus.observability.svc.cluster.local:9090
- Loki API: http://dd-loki.observability.svc.cluster.local:3100
- Grafana UI/API: http://dd-grafana.observability.svc.cluster.local:3000
- NATS monitor: http://dd-nats.messaging.svc.cluster.local:8222
- NATS metrics: http://dd-nats.messaging.svc.cluster.local:7777/metrics
- OTel collector metrics exporter: http://dd-otel-collector.observability.svc.cluster.local:8889/metrics
- Runtime services: dd-remote-web-home:8080, dd-remote-rest-api:8082,
  dd-dev-server-api:8080, dd-gleamlang-server:8081.

Suggested checks:
- Query Prometheus for failing or missing scrape targets, high 5xx rates,
  restarted pods, unavailable deployments, and runtime-specific error metrics.
- Query Loki for recent error logs from dd-remote-gateway, dd-dev-server-api,
  dd-remote-rest-api, dd-remote-web-home, dd-idle-reaper, dd-otel-collector,
  dd-prometheus, dd-loki, dd-grafana, dd-nats, and dd-gleamlang-server.
- Check NATS /varz and /connz for resource pressure or client churn.
- Inspect the manifests under remote/argocd and remote/k8s for mismatches
  between live behavior and declared GitOps state.

Working rules:
- Prefer read-only telemetry calls first. If curl is unavailable, use Node's
  built-in fetch from a small node -e script.
- Do not print secrets, tokens, private keys, or full environment dumps.
- Do not run SQL writes. Do not delete cloud resources.
- Keep changes narrow: code, manifests, tests, and docs under remote/ are the
  expected surface.
- If there is no actionable issue, do not edit files; finish with a concise
  no-change report.
- If there is an actionable issue, patch it, run the most relevant remote/tests
  command, and summarize the PR-ready change.

The remote-dev server will commit changed files, push the branch, and open or
reuse a GitHub PR against dev after you finish.
"#;

#[derive(Clone)]
struct SweepJob {
    url: String,
    auth_secret: String,
    interval_seconds: u64,
    dry_run: bool,
}

#[derive(Clone)]
struct ClusterDoctorJob {
    task_url: String,
    server_auth_secret: String,
    interval_seconds: u64,
    run_on_start: bool,
    thread_id: Option<String>,
    thread_title: String,
    provider: Option<String>,
    user_id: Option<String>,
}

#[derive(Clone)]
struct NatsWatchJob {
    nats_url: String,
    task_subject: String,
    event_subject: String,
    rest_api_url: String,
    server_auth_secret: String,
    gleam_broadcast_url: String,
    gleam_broadcast_secret: String,
    active_interval_seconds: u64,
    idle_interval_seconds: u64,
}

#[derive(Clone)]
struct RuntimeFloorJob {
    nats_url: String,
    task_subject: String,
    task_stream: String,
    task_consumer: String,
    task_ack_wait_seconds: u64,
    task_max_ack_pending: i64,
    task_max_deliver: i64,
    container_pool_url: String,
    server_auth_secret: String,
    interval_seconds: u64,
    k8s_namespace: String,
    queue_consumer_deployment: String,
    min_queue_consumer_replicas: i64,
    min_queue_consumer_ready: i64,
}

#[derive(Clone)]
struct WorkerImageBuildJob {
    repo_dir: String,
    repo_url: String,
    repo_ref: String,
    image: String,
    nerdctl: String,
    deploy_key: String,
    nats_url: String,
    event_subject: String,
    timezone: Tz,
    hour: u32,
    minute: u32,
    run_on_start: bool,
}

#[derive(Clone)]
struct K8sRuntimeWatchJob {
    nats_url: String,
    event_subject: String,
    namespaces: Vec<String>,
    label_selector: Option<String>,
    resync_interval_seconds: u64,
    watch_timeout_seconds: u64,
    retry_interval_seconds: u64,
}

#[derive(Clone)]
struct BrowserJobReapJob {
    nerdctl: String,
    namespace: String,
    label: String,
    deadline_label: String,
    interval_seconds: u64,
    grace_seconds: u64,
    nats_url: String,
    event_subject: String,
}

type QueueTaskMessage = AgentTaskQueueMessage;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeFloorPoolsResponse {
    pools: Vec<RuntimeFloorPoolSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeFloorPoolSummary {
    slug: String,
    min_warm: usize,
    idle_containers: usize,
    active_containers: usize,
    unhealthy_containers: usize,
}

fn is_shadow_task(task: &QueueTaskMessage) -> bool {
    task.shadow.unwrap_or(false)
        || task
            .message_kind
            .as_deref()
            .is_some_and(|kind| kind == "task.shadow")
}

#[derive(Debug, Deserialize)]
struct K8sWatchEvent {
    #[serde(rename = "type")]
    event_type: String,
    object: Value,
}

#[derive(Clone, Copy)]
enum K8sWatchResource {
    Deployment,
    Pod,
}

impl K8sWatchResource {
    fn kind(self) -> &'static str {
        match self {
            Self::Deployment => "Deployment",
            Self::Pod => "Pod",
        }
    }

    fn path(self, namespace: &str) -> String {
        match self {
            Self::Deployment => {
                format!("/apis/apps/v1/namespaces/{namespace}/deployments")
            }
            Self::Pod => format!("/api/v1/namespaces/{namespace}/pods"),
        }
    }
}

fn env_string(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_u64(name: &str, default_value: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default_value)
}

fn env_i64(name: &str, default_value: i64) -> i64 {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default_value)
}

fn env_bool(name: &str, default_value: bool) -> bool {
    env::var(name)
        .ok()
        .map(|v| match v.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default_value,
        })
        .unwrap_or(default_value)
}

fn env_csv(name: &str, default_value: &str) -> Vec<String> {
    env::var(name)
        .unwrap_or_else(|_| default_value.to_string())
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn sweep_job_from_env() -> Option<SweepJob> {
    let url = env_string("REAPER_SWEEP_URL");
    let auth_secret = env_string("REAPER_SECRET");
    if url.is_none() || auth_secret.is_none() {
        println!("idle sweep disabled: REAPER_SWEEP_URL or REAPER_SECRET missing");
        return None;
    }

    Some(SweepJob {
        url: url.expect("checked above"),
        auth_secret: auth_secret.expect("checked above"),
        interval_seconds: env_u64("REAPER_INTERVAL_SECONDS", 60),
        dry_run: env_bool("REAPER_DRY_RUN", false),
    })
}

fn cluster_doctor_job_from_env() -> Option<ClusterDoctorJob> {
    if !env_bool("CLUSTER_DOCTOR_ENABLED", false) {
        println!("cluster doctor disabled: CLUSTER_DOCTOR_ENABLED is false");
        return None;
    }

    let server_auth_secret = env_string("CLUSTER_DOCTOR_SERVER_AUTH_SECRET")
        .or_else(|| env_string("SERVER_AUTH_SECRET"));
    if server_auth_secret.is_none() {
        println!(
            "cluster doctor disabled: CLUSTER_DOCTOR_SERVER_AUTH_SECRET or SERVER_AUTH_SECRET missing"
        );
        return None;
    }

    Some(ClusterDoctorJob {
        task_url: env_string("CLUSTER_DOCTOR_TASK_URL").unwrap_or_else(|| {
            "http://dd-dev-server-api.default.svc.cluster.local:8080/tasks".to_string()
        }),
        server_auth_secret: server_auth_secret.expect("checked above"),
        interval_seconds: env_u64("CLUSTER_DOCTOR_INTERVAL_SECONDS", 90 * 60),
        run_on_start: env_bool("CLUSTER_DOCTOR_RUN_ON_START", false),
        thread_id: env_string("CLUSTER_DOCTOR_THREAD_ID"),
        thread_title: env_string("CLUSTER_DOCTOR_THREAD_TITLE")
            .unwrap_or_else(|| "cluster telemetry doctor".to_string()),
        provider: env_string("CLUSTER_DOCTOR_PROVIDER"),
        user_id: env_string("CLUSTER_DOCTOR_USER_ID"),
    })
}

fn server_auth_secret_from_env() -> Option<String> {
    env_string("NATS_WATCH_SERVER_AUTH_SECRET")
        .or_else(|| env_string("CLUSTER_DOCTOR_SERVER_AUTH_SECRET"))
        .or_else(|| env_string("SERVER_AUTH_SECRET"))
}

fn nats_watch_job_from_env() -> Option<NatsWatchJob> {
    if !env_bool("NATS_WATCH_ENABLED", false) {
        println!("nats watchdog disabled: NATS_WATCH_ENABLED is false");
        return None;
    }

    let server_auth_secret = server_auth_secret_from_env();
    if server_auth_secret.is_none() {
        println!(
            "nats watchdog disabled: NATS_WATCH_SERVER_AUTH_SECRET, CLUSTER_DOCTOR_SERVER_AUTH_SECRET, or SERVER_AUTH_SECRET missing"
        );
        return None;
    }
    let gleam_broadcast_secret = env_string("NATS_WATCH_GLEAM_BROADCAST_SECRET");
    if gleam_broadcast_secret.is_none() {
        println!("nats watchdog disabled: NATS_WATCH_GLEAM_BROADCAST_SECRET missing");
        return None;
    }

    Some(NatsWatchJob {
        nats_url: env_string("NATS_URL")
            .unwrap_or_else(|| "nats://dd-nats.messaging.svc.cluster.local:4222".to_string()),
        task_subject: env_string("NATS_WATCH_TASK_SUBJECT")
            .unwrap_or_else(|| THREAD_TASKS_WILDCARD.to_string()),
        event_subject: env_string("NATS_WATCH_EVENT_SUBJECT")
            .unwrap_or_else(|| RUNTIME_EVENTS_SUBJECT.to_string()),
        rest_api_url: env_string("NATS_WATCH_REST_API_URL").unwrap_or_else(|| {
            "http://dd-remote-rest-api.default.svc.cluster.local:8082".to_string()
        }),
        server_auth_secret: server_auth_secret.expect("checked above"),
        gleam_broadcast_url: env_string("NATS_WATCH_GLEAM_BROADCAST_URL").unwrap_or_else(|| {
            "http://dd-gleamlang-server.default.svc.cluster.local:8081/broadcast".to_string()
        }),
        gleam_broadcast_secret: gleam_broadcast_secret.expect("checked above"),
        active_interval_seconds: env_u64("NATS_WATCH_ACTIVE_INTERVAL_SECONDS", 5),
        idle_interval_seconds: env_u64("NATS_WATCH_IDLE_INTERVAL_SECONDS", 15),
    })
}

fn runtime_floor_job_from_env() -> Option<RuntimeFloorJob> {
    if !env_bool("RUNTIME_FLOOR_ENABLED", false) {
        println!("runtime floor disabled: RUNTIME_FLOOR_ENABLED is false");
        return None;
    }

    let server_auth_secret = server_auth_secret_from_env();
    if server_auth_secret.is_none() {
        println!(
            "runtime floor disabled: NATS_WATCH_SERVER_AUTH_SECRET, CLUSTER_DOCTOR_SERVER_AUTH_SECRET, or SERVER_AUTH_SECRET missing"
        );
        return None;
    }

    Some(RuntimeFloorJob {
        nats_url: env_string("NATS_URL")
            .unwrap_or_else(|| "nats://dd-nats.messaging.svc.cluster.local:4222".to_string()),
        task_subject: env_string("RUNTIME_FLOOR_NATS_TASK_SUBJECT")
            .or_else(|| env_string("NATS_WATCH_TASK_SUBJECT"))
            .unwrap_or_else(|| THREAD_TASKS_WILDCARD.to_string()),
        task_stream: env_string("RUNTIME_FLOOR_NATS_TASK_STREAM")
            .unwrap_or_else(|| DD_REMOTE_TASKS_STREAM_NAME.to_string()),
        task_consumer: env_string("RUNTIME_FLOOR_NATS_TASK_CONSUMER")
            .unwrap_or_else(|| THREAD_PREPARER_QUEUE_GROUP.to_string()),
        task_ack_wait_seconds: env_u64("RUNTIME_FLOOR_NATS_TASK_ACK_WAIT_SECONDS", 600),
        task_max_ack_pending: env_i64("RUNTIME_FLOOR_NATS_TASK_MAX_ACK_PENDING", 256),
        task_max_deliver: env_i64("RUNTIME_FLOOR_NATS_TASK_MAX_DELIVER", 5),
        container_pool_url: env_string("RUNTIME_FLOOR_CONTAINER_POOL_URL").unwrap_or_else(|| {
            "http://dd-container-pool.default.svc.cluster.local:8102".to_string()
        }),
        server_auth_secret: server_auth_secret.expect("checked above"),
        interval_seconds: env_u64("RUNTIME_FLOOR_INTERVAL_SECONDS", 20),
        k8s_namespace: env_string("RUNTIME_FLOOR_K8S_NAMESPACE")
            .unwrap_or_else(|| "default".to_string()),
        queue_consumer_deployment: env_string("RUNTIME_FLOOR_QUEUE_CONSUMER_DEPLOYMENT")
            .unwrap_or_else(|| "dd-remote-queue-consumer".to_string()),
        min_queue_consumer_replicas: env_i64("RUNTIME_FLOOR_QUEUE_CONSUMER_MIN_REPLICAS", 1),
        min_queue_consumer_ready: env_i64("RUNTIME_FLOOR_QUEUE_CONSUMER_MIN_READY", 1),
    })
}

fn worker_image_build_job_from_env() -> Option<WorkerImageBuildJob> {
    if !env_bool("WORKER_IMAGE_BUILD_ENABLED", false) {
        println!("worker image build disabled: WORKER_IMAGE_BUILD_ENABLED is false");
        return None;
    }

    let deploy_key =
        env_string("WORKER_IMAGE_BUILD_GITHUB_DEPLOY_KEY").or_else(|| env_string("GH_DEPLOY_KEY"));
    if deploy_key.is_none() {
        println!("worker image build disabled: WORKER_IMAGE_BUILD_GITHUB_DEPLOY_KEY or GH_DEPLOY_KEY missing");
        return None;
    }
    let Some(repo_url) = env_string("WORKER_IMAGE_BUILD_REPO_URL") else {
        println!("worker image build disabled: WORKER_IMAGE_BUILD_REPO_URL missing");
        return None;
    };

    let timezone_name =
        env_string("WORKER_IMAGE_BUILD_TIMEZONE").unwrap_or_else(|| "America/New_York".to_string());
    let timezone = timezone_name.parse::<Tz>().unwrap_or_else(|_| {
        eprintln!(
            "invalid WORKER_IMAGE_BUILD_TIMEZONE={timezone_name}; falling back to America/New_York"
        );
        chrono_tz::America::New_York
    });

    Some(WorkerImageBuildJob {
        repo_dir: env_string("WORKER_IMAGE_BUILD_REPO_DIR")
            .unwrap_or_else(|| "/opt/dd-next-1".to_string()),
        repo_url,
        repo_ref: env_string("WORKER_IMAGE_BUILD_REF").unwrap_or_else(|| "dev".to_string()),
        image: env_string("WORKER_IMAGE_BUILD_IMAGE")
            .unwrap_or_else(|| "docker.io/library/dd-dev-server:dev".to_string()),
        nerdctl: env_string("WORKER_IMAGE_BUILD_NERDCTL")
            .unwrap_or_else(|| "/usr/local/bin/nerdctl".to_string()),
        deploy_key: deploy_key.expect("checked above"),
        nats_url: env_string("NATS_URL")
            .unwrap_or_else(|| "nats://dd-nats.messaging.svc.cluster.local:4222".to_string()),
        event_subject: env_string("WORKER_IMAGE_BUILD_EVENT_SUBJECT")
            .unwrap_or_else(|| RUNTIME_EVENTS_SUBJECT.to_string()),
        timezone,
        hour: env_u64("WORKER_IMAGE_BUILD_HOUR", 4).min(23) as u32,
        minute: env_u64("WORKER_IMAGE_BUILD_MINUTE", 0).min(59) as u32,
        run_on_start: env_bool("WORKER_IMAGE_BUILD_RUN_ON_START", false),
    })
}

fn k8s_runtime_watch_job_from_env() -> Option<K8sRuntimeWatchJob> {
    if !env_bool("K8S_RUNTIME_WATCH_ENABLED", false) {
        println!("k8s runtime watch disabled: K8S_RUNTIME_WATCH_ENABLED is false");
        return None;
    }

    let namespaces = env_csv("K8S_RUNTIME_WATCH_NAMESPACES", "default,vpn");
    if namespaces.is_empty() {
        println!("k8s runtime watch disabled: no namespaces configured");
        return None;
    }

    let resync_interval_seconds = env_u64("K8S_RUNTIME_WATCH_RESYNC_SECONDS", 200);
    Some(K8sRuntimeWatchJob {
        nats_url: env_string("NATS_URL")
            .unwrap_or_else(|| "nats://dd-nats.messaging.svc.cluster.local:4222".to_string()),
        event_subject: env_string("K8S_RUNTIME_WATCH_EVENT_SUBJECT")
            .unwrap_or_else(|| RUNTIME_EVENTS_SUBJECT.to_string()),
        namespaces,
        label_selector: env_string("K8S_RUNTIME_WATCH_LABEL_SELECTOR"),
        resync_interval_seconds,
        watch_timeout_seconds: env_u64(
            "K8S_RUNTIME_WATCH_TIMEOUT_SECONDS",
            resync_interval_seconds,
        ),
        retry_interval_seconds: env_u64("K8S_RUNTIME_WATCH_RETRY_SECONDS", 5),
    })
}

fn browser_job_reap_job_from_env() -> Option<BrowserJobReapJob> {
    if !env_bool("BROWSER_JOB_REAP_ENABLED", false) {
        println!("browser job reaper disabled: BROWSER_JOB_REAP_ENABLED is false");
        return None;
    }

    Some(BrowserJobReapJob {
        nerdctl: env_string("BROWSER_JOB_REAP_NERDCTL").unwrap_or_else(|| "/usr/local/bin/nerdctl".to_string()),
        namespace: env_string("BROWSER_JOB_REAP_NAMESPACE").unwrap_or_else(|| "dd-browser-jobs".to_string()),
        label: env_string("BROWSER_JOB_REAP_LABEL")
            .unwrap_or_else(|| "dd.browser-job.managed=true".to_string()),
        deadline_label: env_string("BROWSER_JOB_REAP_DEADLINE_LABEL")
            .unwrap_or_else(|| "dd.browser-job.deadline-ms".to_string()),
        interval_seconds: env_u64("BROWSER_JOB_REAP_INTERVAL_SECONDS", 60),
        // Backstop only: the spawner kills its own overruns at the 9-minute
        // deadline, so the reaper waits an extra grace before force-removing in
        // case the spawner pod itself is gone.
        grace_seconds: env_u64("BROWSER_JOB_REAP_GRACE_SECONDS", 60),
        nats_url: env_string("NATS_URL")
            .unwrap_or_else(|| "nats://dd-nats.messaging.svc.cluster.local:4222".to_string()),
        event_subject: env_string("BROWSER_JOB_REAP_EVENT_SUBJECT")
            .unwrap_or_else(|| RUNTIME_EVENTS_SUBJECT.to_string()),
    })
}

fn parse_label_value(labels: &str, key: &str) -> Option<String> {
    labels
        .split(',')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, value)| value.trim().to_string())
}

async fn publish_browser_job_reap_event(job: &BrowserJobReapJob, reaped: &[String]) {
    let Ok(nats) = async_nats::connect(job.nats_url.clone()).await else {
        eprintln!("browser job reaper could not publish event: nats connect failed");
        return;
    };
    let payload = json!({
        "type": "browser-job-reap",
        "scope": "admin",
        "source": "dd-idle-reaper",
        "namespace": job.namespace,
        "reapedCount": reaped.len(),
        "reaped": reaped,
        "atMs": now_ms(),
    })
    .to_string();
    if let Err(error) = nats.publish(job.event_subject.clone(), payload.into()).await {
        eprintln!("browser job reap event publish failed: {error}");
    }
}

async fn run_browser_job_reap_once(job: &BrowserJobReapJob) {
    // List managed (running + stopped) browser-job containers with their labels.
    let mut list = Command::new(&job.nerdctl);
    list.arg("-n")
        .arg(&job.namespace)
        .arg("ps")
        .arg("-a")
        .arg("--no-trunc")
        .arg("--filter")
        .arg(format!("label={}", job.label))
        .arg("--format")
        .arg("{{.Names}}|{{.Labels}}");
    let output = match list.output().await {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            eprintln!(
                "browser job reaper ps failed: {}",
                truncate_for_log(&output.stderr).trim()
            );
            return;
        }
        Err(error) => {
            eprintln!("browser job reaper ps could not start: {error}");
            return;
        }
    };

    let now = now_ms();
    let grace_ms = (job.grace_seconds as u128) * 1000;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut targets: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (name, labels) = line.split_once('|').unwrap_or((line, ""));
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        // Reap when past deadline + grace, or when a managed container has no
        // parseable deadline label at all (malformed/leaked → not ours-shaped).
        let expired = match parse_label_value(labels, &job.deadline_label).and_then(|v| v.parse::<u128>().ok()) {
            Some(deadline_ms) => now > deadline_ms + grace_ms,
            None => true,
        };
        if expired {
            targets.push(name.to_string());
        }
    }

    if targets.is_empty() {
        return;
    }

    let mut reaped: Vec<String> = Vec::new();
    for name in &targets {
        let mut remove = Command::new(&job.nerdctl);
        remove
            .arg("-n")
            .arg(&job.namespace)
            .arg("rm")
            .arg("-f")
            .arg(name);
        match remove.output().await {
            Ok(output) if output.status.success() => {
                println!("browser job reaper removed expired container {name}");
                reaped.push(name.clone());
            }
            Ok(output) => eprintln!(
                "browser job reaper failed to remove {name}: {}",
                truncate_for_log(&output.stderr).trim()
            ),
            Err(error) => eprintln!("browser job reaper rm could not start for {name}: {error}"),
        }
    }

    if !reaped.is_empty() {
        publish_browser_job_reap_event(job, &reaped).await;
    }
}

async fn run_browser_job_reap_loop(job: BrowserJobReapJob) {
    println!(
        "browser job reaper starting: namespace={} label={} interval={}s grace={}s",
        job.namespace, job.label, job.interval_seconds, job.grace_seconds
    );
    loop {
        run_browser_job_reap_once(&job).await;
        sleep(Duration::from_secs(job.interval_seconds)).await;
    }
}

fn sweep_url(job: &SweepJob) -> String {
    if !job.dry_run {
        return job.url.clone();
    }

    if job.url.contains('?') {
        format!("{}&dryRun=1", job.url)
    } else {
        format!("{}?dryRun=1", job.url)
    }
}

async fn run_sweep_once(client: &Client, job: &SweepJob) {
    let url = sweep_url(job);
    match client
        .post(&url)
        .header("x-reaper-auth", &job.auth_secret)
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| String::from("<body unreadable>"));
            if status.is_success() {
                println!("sweep ok status={} body={}", status, body);
            } else {
                eprintln!("sweep failed status={} body={}", status, body);
            }
        }
        Err(err) => {
            eprintln!("sweep request error: {}", err);
        }
    }
}

async fn run_sweep_loop(client: Client, job: SweepJob) {
    println!(
        "idle sweep loop starting: interval={}s dryRun={} url={}",
        job.interval_seconds,
        job.dry_run,
        sweep_url(&job)
    );

    loop {
        run_sweep_once(&client, &job).await;
        sleep(Duration::from_secs(job.interval_seconds)).await;
    }
}

async fn run_cluster_doctor_once(client: &Client, job: &ClusterDoctorJob) {
    let mut body = json!({
        "prompt": CLUSTER_DOCTOR_PROMPT,
        "threadTitle": job.thread_title,
    });

    if let Some(thread_id) = &job.thread_id {
        body["threadId"] = json!(thread_id);
    }
    if let Some(provider) = &job.provider {
        body["provider"] = json!(provider);
    }
    if let Some(user_id) = &job.user_id {
        body["userId"] = json!(user_id);
    }

    match client
        .post(&job.task_url)
        .header("x-server-auth", &job.server_auth_secret)
        .json(&body)
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| String::from("<body unreadable>"));
            if status.is_success() {
                println!("cluster doctor dispatched status={} body={}", status, body);
            } else {
                eprintln!(
                    "cluster doctor dispatch failed status={} body={}",
                    status, body
                );
            }
        }
        Err(err) => {
            eprintln!("cluster doctor dispatch request error: {}", err);
        }
    }
}

async fn run_cluster_doctor_loop(client: Client, job: ClusterDoctorJob) {
    println!(
        "cluster doctor loop starting: interval={}s runOnStart={} taskUrl={} provider={}",
        job.interval_seconds,
        job.run_on_start,
        job.task_url,
        job.provider.as_deref().unwrap_or("default")
    );

    if !job.run_on_start {
        sleep(Duration::from_secs(job.interval_seconds)).await;
    }

    loop {
        run_cluster_doctor_once(&client, &job).await;
        sleep(Duration::from_secs(job.interval_seconds)).await;
    }
}

fn truncate_for_log(value: &[u8]) -> String {
    let text = String::from_utf8_lossy(value);
    text.chars().take(4_000).collect::<String>()
}

async fn run_command(mut command: Command, label: &str) -> Result<(), String> {
    let output = command
        .output()
        .await
        .map_err(|error| format!("{label} failed to start: {error}"))?;
    let stdout = truncate_for_log(&output.stdout);
    let stderr = truncate_for_log(&output.stderr);
    if !stdout.trim().is_empty() {
        println!("{label} stdout: {}", stdout.trim());
    }
    if !stderr.trim().is_empty() {
        eprintln!("{label} stderr: {}", stderr.trim());
    }
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("{label} exited with {}", output.status))
    }
}

async fn publish_worker_image_build_event(job: &WorkerImageBuildJob, status: &str, message: &str) {
    let Ok(nats) = async_nats::connect(job.nats_url.clone()).await else {
        eprintln!("worker image build could not publish event: nats connect failed");
        return;
    };
    let payload = json!({
        "type": "worker-image-build",
        "source": "dd-idle-reaper",
        "status": status,
        "image": job.image,
        "repoRef": job.repo_ref,
        "message": message,
    })
    .to_string();
    if let Err(error) = nats
        .publish(job.event_subject.clone(), payload.into())
        .await
    {
        eprintln!("worker image build event publish failed: {error}");
    }
}

async fn run_worker_image_build_once(job: &WorkerImageBuildJob) -> Result<(), String> {
    let key_path = format!("/tmp/dd-worker-image-build-{}.key", job.repo_ref);
    fs::write(&key_path, &job.deploy_key)
        .await
        .map_err(|error| format!("failed to write deploy key: {error}"))?;
    fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
        .await
        .map_err(|error| format!("failed to chmod deploy key: {error}"))?;
    let git_ssh_command = format!(
        "ssh -F /dev/null -i {key_path} -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/tmp/dd-worker-image-known-hosts"
    );

    let mut fetch = Command::new("git");
    fetch
        .current_dir(&job.repo_dir)
        .env("GIT_SSH_COMMAND", &git_ssh_command)
        .arg("fetch")
        .arg("origin")
        .arg(&job.repo_ref);
    run_command(fetch, "worker-image git fetch").await?;

    let mut merge = Command::new("git");
    merge
        .current_dir(&job.repo_dir)
        .env("GIT_SSH_COMMAND", &git_ssh_command)
        .arg("merge")
        .arg("--ff-only")
        .arg(format!("origin/{}", job.repo_ref));
    run_command(merge, "worker-image git merge").await?;

    let mut build = Command::new(&job.nerdctl);
    build
        .current_dir(&job.repo_dir)
        .env("DOCKER_BUILDKIT", "1")
        .arg("-n")
        .arg("k8s.io")
        .arg("build")
        .arg("--progress=plain")
        .arg("--build-arg")
        .arg(format!("DD_REPO_URL={}", job.repo_url))
        .arg("--build-arg")
        .arg(format!("DD_REPO_REF={}", job.repo_ref))
        .arg("--build-arg")
        .arg(format!("DD_REPO_CACHE_BUST={}", Utc::now().timestamp()))
        .arg("--secret")
        .arg(format!("id=github_deploy_key,src={key_path}"))
        .arg("-t")
        .arg(&job.image)
        .arg("remote/deployments/dev-server");
    run_command(build, "worker-image nerdctl build").await?;
    Ok(())
}

fn next_worker_image_build_delay(job: &WorkerImageBuildJob) -> Duration {
    let now_utc = Utc::now();
    let now_local = now_utc.with_timezone(&job.timezone);
    let mut date = now_local.date_naive();
    let mut target = job
        .timezone
        .from_local_datetime(
            &date
                .and_hms_opt(job.hour, job.minute, 0)
                .expect("valid time"),
        )
        .earliest()
        .unwrap_or_else(|| now_local + chrono::Duration::hours(24));
    if target <= now_local {
        date += chrono::Duration::days(1);
        target = job
            .timezone
            .from_local_datetime(
                &date
                    .and_hms_opt(job.hour, job.minute, 0)
                    .expect("valid time"),
            )
            .earliest()
            .unwrap_or_else(|| now_local + chrono::Duration::hours(24));
    }
    target
        .with_timezone(&Utc)
        .signed_duration_since(now_utc)
        .to_std()
        .unwrap_or_else(|_| Duration::from_secs(60))
}

async fn run_worker_image_build_loop(job: WorkerImageBuildJob) {
    println!(
        "worker image build loop starting: image={} ref={} schedule={:02}:{:02} {:?} runOnStart={}",
        job.image, job.repo_ref, job.hour, job.minute, job.timezone, job.run_on_start
    );
    if job.run_on_start {
        match run_worker_image_build_once(&job).await {
            Ok(()) => {
                println!("worker image build succeeded on start");
                publish_worker_image_build_event(
                    &job,
                    "ok",
                    "worker image build succeeded on start",
                )
                .await;
            }
            Err(error) => {
                eprintln!("worker image build failed on start: {error}");
                publish_worker_image_build_event(&job, "error", &error).await;
            }
        }
    }
    loop {
        let delay = next_worker_image_build_delay(&job);
        println!("worker image build sleeping for {}s", delay.as_secs());
        sleep(delay).await;
        match run_worker_image_build_once(&job).await {
            Ok(()) => {
                println!("worker image build succeeded");
                publish_worker_image_build_event(&job, "ok", "worker image build succeeded").await;
            }
            Err(error) => {
                eprintln!("worker image build failed: {error}");
                publish_worker_image_build_event(&job, "error", &error).await;
            }
        }
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis()
}

fn json_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |cursor, segment| cursor.get(*segment))
}

fn json_at_string(value: &Value, path: &[&str]) -> Option<String> {
    json_at(value, path)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .filter(|text| !text.is_empty())
}

fn json_at_i64(value: &Value, path: &[&str]) -> Option<i64> {
    json_at(value, path).and_then(Value::as_i64)
}

fn k8s_object_name(object: &Value) -> String {
    json_at_string(object, &["metadata", "name"]).unwrap_or_else(|| "unknown".to_string())
}

fn k8s_object_namespace(object: &Value) -> String {
    json_at_string(object, &["metadata", "namespace"]).unwrap_or_else(|| "default".to_string())
}

fn k8s_object_resource_version(object: &Value) -> String {
    json_at_string(object, &["metadata", "resourceVersion"]).unwrap_or_default()
}

fn k8s_object_key(resource: K8sWatchResource, object: &Value) -> String {
    format!(
        "{}:{}/{}",
        resource.kind(),
        k8s_object_namespace(object),
        k8s_object_name(object)
    )
}

fn k8s_container_state_label(container: &Value) -> String {
    let state = container
        .get("state")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if state.contains_key("running") {
        "running".to_string()
    } else if let Some(waiting) = state.get("waiting") {
        json_at_string(waiting, &["reason"]).unwrap_or_else(|| "waiting".to_string())
    } else if let Some(terminated) = state.get("terminated") {
        json_at_string(terminated, &["reason"]).unwrap_or_else(|| "terminated".to_string())
    } else {
        "unknown".to_string()
    }
}

fn summarize_k8s_deployment(deployment: &Value) -> Value {
    let conditions = json_at(deployment, &["status", "conditions"])
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|condition| {
                    json!({
                        "type": json_at_string(condition, &["type"]),
                        "status": json_at_string(condition, &["status"]),
                        "reason": json_at_string(condition, &["reason"]),
                        "message": json_at_string(condition, &["message"]),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "name": json_at_string(deployment, &["metadata", "name"]),
        "namespace": json_at_string(deployment, &["metadata", "namespace"]),
        "uid": json_at_string(deployment, &["metadata", "uid"]),
        "resourceVersion": json_at_string(deployment, &["metadata", "resourceVersion"]),
        "generation": json_at_i64(deployment, &["metadata", "generation"]),
        "observedGeneration": json_at_i64(deployment, &["status", "observedGeneration"]),
        "desiredReplicas": json_at_i64(deployment, &["spec", "replicas"]).unwrap_or(0),
        "replicas": json_at_i64(deployment, &["status", "replicas"]).unwrap_or(0),
        "readyReplicas": json_at_i64(deployment, &["status", "readyReplicas"]).unwrap_or(0),
        "availableReplicas": json_at_i64(deployment, &["status", "availableReplicas"]).unwrap_or(0),
        "updatedReplicas": json_at_i64(deployment, &["status", "updatedReplicas"]).unwrap_or(0),
        "unavailableReplicas": json_at_i64(deployment, &["status", "unavailableReplicas"]).unwrap_or(0),
        "conditions": conditions,
    })
}

fn summarize_k8s_pod(pod: &Value) -> Value {
    let containers = json_at(pod, &["status", "containerStatuses"])
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total_containers = containers.len();
    let ready_containers = containers
        .iter()
        .filter(|container| container.get("ready").and_then(Value::as_bool) == Some(true))
        .count();
    let restart_count = containers
        .iter()
        .map(|container| json_at_i64(container, &["restartCount"]).unwrap_or(0))
        .sum::<i64>();
    let container_summaries = containers
        .iter()
        .map(|container| {
            json!({
                "name": json_at_string(container, &["name"]),
                "ready": container.get("ready").and_then(Value::as_bool).unwrap_or(false),
                "restartCount": json_at_i64(container, &["restartCount"]).unwrap_or(0),
                "state": k8s_container_state_label(container),
            })
        })
        .collect::<Vec<_>>();

    json!({
        "name": json_at_string(pod, &["metadata", "name"]),
        "namespace": json_at_string(pod, &["metadata", "namespace"]),
        "uid": json_at_string(pod, &["metadata", "uid"]),
        "resourceVersion": json_at_string(pod, &["metadata", "resourceVersion"]),
        "phase": json_at_string(pod, &["status", "phase"]),
        "podIp": json_at_string(pod, &["status", "podIP"]),
        "hostIp": json_at_string(pod, &["status", "hostIP"]),
        "nodeName": json_at_string(pod, &["spec", "nodeName"]),
        "startTime": json_at_string(pod, &["status", "startTime"]),
        "deletionTimestamp": json_at_string(pod, &["metadata", "deletionTimestamp"]),
        "readyContainers": ready_containers,
        "totalContainers": total_containers,
        "restartCount": restart_count,
        "containers": container_summaries,
    })
}

fn summarize_k8s_object(resource: K8sWatchResource, object: &Value) -> Value {
    match resource {
        K8sWatchResource::Deployment => summarize_k8s_deployment(object),
        K8sWatchResource::Pod => summarize_k8s_pod(object),
    }
}

async fn k8s_runtime_client(timeout_seconds: u64) -> Result<(Client, String, String), String> {
    let host = env_string("KUBERNETES_SERVICE_HOST")
        .unwrap_or_else(|| "kubernetes.default.svc".to_string());
    let port = env_string("KUBERNETES_SERVICE_PORT").unwrap_or_else(|| "443".to_string());
    let token = if let Some(token) = env_string("K8S_SA_TOKEN") {
        token
    } else {
        fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/token")
            .await
            .map_err(|error| format!("failed to read service account token: {error}"))?
    };

    let mut builder = Client::builder().timeout(Duration::from_secs(timeout_seconds));
    if let Ok(ca_bytes) = fs::read("/var/run/secrets/kubernetes.io/serviceaccount/ca.crt").await {
        if let Ok(ca) = Certificate::from_pem(&ca_bytes) {
            builder = builder.add_root_certificate(ca);
        }
    }

    let client = builder
        .build()
        .map_err(|error| format!("failed to build k8s client: {error}"))?;
    Ok((client, format!("https://{host}:{port}"), token))
}

async fn k8s_get_list(
    client: &Client,
    base_url: &str,
    token: &str,
    path: String,
    label_selector: Option<&str>,
) -> Result<Value, String> {
    let mut request = client
        .get(format!("{base_url}{path}"))
        .bearer_auth(token.trim())
        .header(reqwest::header::ACCEPT, "application/json");
    if let Some(label_selector) = label_selector {
        request = request.query(&[("labelSelector", label_selector)]);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("k8s list request failed: {error}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "k8s list failed status={} body={}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    serde_json::from_str::<Value>(&body).map_err(|error| format!("k8s list invalid json: {error}"))
}

async fn k8s_get_object(
    client: &Client,
    base_url: &str,
    token: &str,
    path: String,
) -> Result<Value, String> {
    let response = client
        .get(format!("{base_url}{path}"))
        .bearer_auth(token.trim())
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| format!("k8s get request failed: {error}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "k8s get failed status={} body={}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    serde_json::from_str::<Value>(&body).map_err(|error| format!("k8s get invalid json: {error}"))
}

async fn k8s_patch_merge(
    client: &Client,
    base_url: &str,
    token: &str,
    path: String,
    body: Value,
) -> Result<Value, String> {
    let response = client
        .patch(format!("{base_url}{path}"))
        .bearer_auth(token.trim())
        .header(reqwest::header::ACCEPT, "application/json")
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/merge-patch+json",
        )
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("k8s patch request failed: {error}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "k8s patch failed status={} body={}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }
    serde_json::from_str::<Value>(&body).map_err(|error| format!("k8s patch invalid json: {error}"))
}

async fn publish_k8s_runtime_event(
    nats: &async_nats::Client,
    subject: &str,
    event_type: &str,
    resource: K8sWatchResource,
    object: &Value,
    summary: Value,
) {
    let namespace = k8s_object_namespace(object);
    let name = k8s_object_name(object);
    let resource_version = k8s_object_resource_version(object);
    let payload = json!({
        "type": "k8s-runtime-event",
        "scope": "admin",
        "source": "dd-idle-reaper",
        "eventId": format!("{}:{namespace}:{name}:{resource_version}:{event_type}", resource.kind()),
        "eventType": event_type,
        "kind": resource.kind(),
        "namespace": namespace,
        "name": name,
        "resourceVersion": resource_version,
        "atMs": now_ms(),
        "summary": summary,
    });
    match serde_json::to_vec(&payload) {
        Ok(body) => {
            if let Err(error) = nats.publish(subject.to_string(), body.into()).await {
                eprintln!("k8s runtime watch nats publish failed: {error}");
            }
        }
        Err(error) => eprintln!("k8s runtime watch payload encode failed: {error}"),
    }
}

async fn apply_k8s_runtime_object(
    nats: &async_nats::Client,
    job: &K8sRuntimeWatchJob,
    cache: &Arc<Mutex<HashMap<String, String>>>,
    resource: K8sWatchResource,
    event_type: &str,
    object: &Value,
) {
    let key = k8s_object_key(resource, object);
    let summary = summarize_k8s_object(resource, object);
    if event_type == "DELETED" {
        let mut cache = cache.lock().await;
        if cache.remove(&key).is_some() {
            drop(cache);
            publish_k8s_runtime_event(
                nats,
                &job.event_subject,
                event_type,
                resource,
                object,
                summary,
            )
            .await;
        }
        return;
    }

    let fingerprint = serde_json::to_string(&summary).unwrap_or_default();
    let mut cache = cache.lock().await;
    if cache.get(&key) == Some(&fingerprint) {
        return;
    }
    cache.insert(key, fingerprint);
    drop(cache);
    publish_k8s_runtime_event(
        nats,
        &job.event_subject,
        event_type,
        resource,
        object,
        summary,
    )
    .await;
}

async fn resync_k8s_runtime_resource(
    client: &Client,
    nats: &async_nats::Client,
    base_url: &str,
    token: &str,
    job: &K8sRuntimeWatchJob,
    cache: &Arc<Mutex<HashMap<String, String>>>,
    namespace: &str,
    resource: K8sWatchResource,
) -> Result<String, String> {
    let list = k8s_get_list(
        client,
        base_url,
        token,
        resource.path(namespace),
        job.label_selector.as_deref(),
    )
    .await?;
    let resource_version =
        json_at_string(&list, &["metadata", "resourceVersion"]).unwrap_or_default();
    let prefix = format!("{}:{namespace}/", resource.kind());
    let mut seen = HashSet::new();
    if let Some(items) = json_at(&list, &["items"]).and_then(Value::as_array) {
        for object in items {
            seen.insert(k8s_object_key(resource, object));
            apply_k8s_runtime_object(nats, job, cache, resource, "SYNC", object).await;
        }
    }

    let stale_keys = {
        let cache = cache.lock().await;
        cache
            .keys()
            .filter(|key| key.starts_with(&prefix) && !seen.contains(*key))
            .cloned()
            .collect::<Vec<_>>()
    };
    for key in stale_keys {
        let Some(name) = key.split_once('/').map(|(_, name)| name.to_string()) else {
            continue;
        };
        let object = json!({
            "metadata": {
                "namespace": namespace,
                "name": name,
                "resourceVersion": resource_version,
            }
        });
        apply_k8s_runtime_object(nats, job, cache, resource, "RESYNC_DELETED", &object).await;
    }

    Ok(resource_version)
}

async fn watch_k8s_runtime_resource(
    client: &Client,
    nats: &async_nats::Client,
    base_url: &str,
    token: &str,
    job: &K8sRuntimeWatchJob,
    cache: &Arc<Mutex<HashMap<String, String>>>,
    namespace: &str,
    resource: K8sWatchResource,
    resource_version: &mut String,
) -> Result<(), String> {
    let timeout_seconds = job.watch_timeout_seconds.to_string();
    let mut params = vec![
        ("watch", "true".to_string()),
        ("allowWatchBookmarks", "true".to_string()),
        ("timeoutSeconds", timeout_seconds),
    ];
    if !resource_version.is_empty() {
        params.push(("resourceVersion", resource_version.clone()));
    }
    if let Some(label_selector) = job.label_selector.as_deref() {
        params.push(("labelSelector", label_selector.to_string()));
    }

    let response = client
        .get(format!("{base_url}{}", resource.path(namespace)))
        .bearer_auth(token.trim())
        .header(reqwest::header::ACCEPT, "application/json")
        .query(&params)
        .send()
        .await
        .map_err(|error| format!("k8s watch request failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "k8s watch failed status={} body={}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("k8s watch stream failed: {error}"))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(newline) = buffer.find('\n') {
            let rest = buffer.split_off(newline + 1);
            let line = buffer.trim().to_string();
            buffer = rest;
            if line.is_empty() {
                continue;
            }
            let event = serde_json::from_str::<K8sWatchEvent>(&line)
                .map_err(|error| format!("k8s watch event invalid json: {error}"))?;
            if let Some(next_resource_version) =
                json_at_string(&event.object, &["metadata", "resourceVersion"])
            {
                *resource_version = next_resource_version;
            }
            if event.event_type == "BOOKMARK" {
                continue;
            }
            apply_k8s_runtime_object(nats, job, cache, resource, &event.event_type, &event.object)
                .await;
        }
    }
    Ok(())
}

async fn run_k8s_runtime_resource_loop(
    job: K8sRuntimeWatchJob,
    namespace: String,
    resource: K8sWatchResource,
) {
    let client_timeout = job.watch_timeout_seconds + job.retry_interval_seconds + 30;
    let (client, base_url, token) = match k8s_runtime_client(client_timeout).await {
        Ok(parts) => parts,
        Err(error) => {
            eprintln!("k8s runtime watch disabled for namespace={namespace}: {error}");
            return;
        }
    };
    let cache = Arc::new(Mutex::new(HashMap::new()));

    loop {
        match async_nats::connect(job.nats_url.clone()).await {
            Ok(nats) => {
                println!(
                    "k8s runtime watch connected: namespace={} resource={} subject={}",
                    namespace,
                    resource.kind(),
                    job.event_subject
                );
                let mut resource_version = String::new();
                loop {
                    match resync_k8s_runtime_resource(
                        &client, &nats, &base_url, &token, &job, &cache, &namespace, resource,
                    )
                    .await
                    {
                        Ok(next_resource_version) => resource_version = next_resource_version,
                        Err(error) => eprintln!(
                            "k8s runtime resync failed namespace={} resource={}: {}",
                            namespace,
                            resource.kind(),
                            error
                        ),
                    }

                    if let Err(error) = watch_k8s_runtime_resource(
                        &client,
                        &nats,
                        &base_url,
                        &token,
                        &job,
                        &cache,
                        &namespace,
                        resource,
                        &mut resource_version,
                    )
                    .await
                    {
                        eprintln!(
                            "k8s runtime watch failed namespace={} resource={}: {}",
                            namespace,
                            resource.kind(),
                            error
                        );
                        sleep(Duration::from_secs(job.retry_interval_seconds)).await;
                    }
                }
            }
            Err(error) => {
                eprintln!("k8s runtime watch nats connect failed: {error}");
                sleep(Duration::from_secs(job.retry_interval_seconds)).await;
            }
        }
    }
}

async fn run_k8s_runtime_watch_loop(job: K8sRuntimeWatchJob) {
    println!(
        "k8s runtime watch starting: namespaces={} subject={} resync={}s watchTimeout={}s",
        job.namespaces.join(","),
        job.event_subject,
        job.resync_interval_seconds,
        job.watch_timeout_seconds
    );
    for namespace in job.namespaces.clone() {
        tokio::spawn(run_k8s_runtime_resource_loop(
            job.clone(),
            namespace.clone(),
            K8sWatchResource::Deployment,
        ));
        tokio::spawn(run_k8s_runtime_resource_loop(
            job.clone(),
            namespace,
            K8sWatchResource::Pod,
        ));
    }
    loop {
        sleep(Duration::from_secs(3600)).await;
    }
}

async fn prepare_thread_from_nats(client: &Client, job: &NatsWatchJob, task: &QueueTaskMessage) {
    let base = job.rest_api_url.trim_end_matches('/');
    let url = format!("{base}/api/agents/threads/{}/prepare", task.thread_id);
    match client
        .post(url)
        .header("X-Agent-Auth", &job.server_auth_secret)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {
            println!(
                "nats watchdog prepared thread={} task={} shadow={} direct_dispatch={}",
                task.thread_id,
                task.task_id,
                task.shadow.unwrap_or(false),
                task.direct_dispatch.unwrap_or(false)
            );
        }
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            eprintln!(
                "nats watchdog prepare failed thread={} task={} status={} body={}",
                task.thread_id,
                task.task_id,
                status,
                body.chars().take(500).collect::<String>()
            );
        }
        Err(error) => {
            eprintln!(
                "nats watchdog prepare request failed thread={} task={} error={}",
                task.thread_id, task.task_id, error
            );
        }
    }
}

async fn broadcast_event_from_nats(client: &Client, job: &NatsWatchJob, payload: &[u8]) {
    match client
        .post(&job.gleam_broadcast_url)
        .header("content-type", "application/json")
        .header("x-dd-internal-auth", &job.gleam_broadcast_secret)
        .body(payload.to_vec())
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {
            println!("nats watchdog bridged task event to gleam websocket fanout");
        }
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            eprintln!(
                "nats watchdog gleam broadcast failed status={} body={}",
                status,
                body.chars().take(500).collect::<String>()
            );
        }
        Err(error) => {
            eprintln!("nats watchdog gleam broadcast request failed: {}", error);
        }
    }
}

async fn run_nats_watch_loop(client: Client, job: NatsWatchJob) {
    println!(
        "nats watchdog starting: taskSubject={} eventSubject={} active={}s idle={}s natsUrl={}",
        job.task_subject,
        job.event_subject,
        job.active_interval_seconds,
        job.idle_interval_seconds,
        job.nats_url
    );

    loop {
        match async_nats::connect(job.nats_url.clone()).await {
            Ok(nats) => {
                let task_subscription = nats.subscribe(job.task_subject.clone()).await;
                let event_subscription = nats.subscribe(job.event_subject.clone()).await;
                match (task_subscription, event_subscription) {
                    (Ok(mut task_subscription), Ok(mut event_subscription)) => {
                        let mut last_window_had_message = false;
                        'connected: loop {
                            let wait_seconds = if last_window_had_message {
                                job.active_interval_seconds
                            } else {
                                job.idle_interval_seconds
                            };
                            let window_sleep = sleep(Duration::from_secs(wait_seconds));
                            tokio::pin!(window_sleep);
                            let mut window_had_message = false;

                            loop {
                                tokio::select! {
                                    maybe_message = task_subscription.next() => {
                                        let Some(message) = maybe_message else {
                                            eprintln!("nats watchdog task subscription ended");
                                            break 'connected;
                                        };
                                        window_had_message = true;
                                        match serde_json::from_slice::<QueueTaskMessage>(&message.payload) {
                                            Ok(task) if is_shadow_task(&task) => prepare_thread_from_nats(&client, &job, &task).await,
                                            Ok(task) => println!(
                                                "nats watchdog ignored queued task thread={} task={} kind={} shadow={} direct_dispatch={}",
                                                task.thread_id,
                                                task.task_id,
                                                task.message_kind.as_deref().unwrap_or("unknown"),
                                                task.shadow.unwrap_or(false),
                                                task.direct_dispatch.unwrap_or(false)
                                            ),
                                            Err(error) => eprintln!("nats watchdog invalid task message: {}", error),
                                        }
                                    }
                                    maybe_message = event_subscription.next() => {
                                        let Some(message) = maybe_message else {
                                            eprintln!("nats watchdog event subscription ended");
                                            break 'connected;
                                        };
                                        window_had_message = true;
                                        broadcast_event_from_nats(&client, &job, &message.payload).await;
                                    }
                                    _ = &mut window_sleep => {
                                        break;
                                    }
                                }
                            }

                            last_window_had_message = window_had_message;
                        }
                    }
                    (Err(task_error), Err(event_error)) => {
                        eprintln!(
                            "nats watchdog subscribe failed: task={} event={}",
                            task_error, event_error
                        );
                    }
                    (Err(error), _) => eprintln!("nats watchdog task subscribe failed: {}", error),
                    (_, Err(error)) => eprintln!("nats watchdog event subscribe failed: {}", error),
                }
            }
            Err(error) => {
                eprintln!("nats watchdog connect failed: {}", error);
            }
        }

        sleep(Duration::from_secs(job.idle_interval_seconds)).await;
    }
}

async fn ensure_runtime_floor_nats(job: &RuntimeFloorJob) -> Result<async_nats::Client, String> {
    let client = async_nats::connect(job.nats_url.clone())
        .await
        .map_err(|error| format!("runtime floor nats connect failed: {error}"))?;
    let jetstream = async_nats::jetstream::new(client.clone());
    let stream = jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: job.task_stream.clone(),
            subjects: vec![job.task_subject.clone()],
            retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
            max_age: Duration::from_secs(60 * 60 * 24 * 14),
            max_message_size: 8 * 1024 * 1024,
            ..Default::default()
        })
        .await
        .map_err(|error| format!("runtime floor stream ensure failed: {error}"))?;

    stream
        .get_or_create_consumer::<async_nats::jetstream::consumer::pull::Config>(
            &job.task_consumer,
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(job.task_consumer.clone()),
                filter_subject: job.task_subject.clone(),
                ack_wait: Duration::from_secs(job.task_ack_wait_seconds),
                max_ack_pending: job.task_max_ack_pending,
                max_deliver: job.task_max_deliver,
                ..Default::default()
            },
        )
        .await
        .map_err(|error| format!("runtime floor consumer ensure failed: {error}"))?;

    Ok(client)
}

async fn publish_runtime_floor_event(
    nats: Option<&async_nats::Client>,
    job: &RuntimeFloorJob,
    status: &str,
    message: &str,
    details: Value,
) {
    let Some(nats) = nats else {
        return;
    };
    let payload = json!({
        "type": "runtime-floor",
        "scope": "admin",
        "source": "dd-idle-reaper",
        "status": status,
        "message": message,
        "queueConsumerDeployment": &job.queue_consumer_deployment,
        "queueConsumerNamespace": &job.k8s_namespace,
        "taskStream": &job.task_stream,
        "taskConsumer": &job.task_consumer,
        "atMs": now_ms(),
        "details": details,
    });
    match serde_json::to_vec(&payload) {
        Ok(body) => {
            if let Err(error) = nats.publish(job.event_subject(), body.into()).await {
                eprintln!("runtime floor event publish failed: {error}");
            }
        }
        Err(error) => eprintln!("runtime floor event encode failed: {error}"),
    }
}

impl RuntimeFloorJob {
    fn event_subject(&self) -> String {
        env_string("RUNTIME_FLOOR_EVENT_SUBJECT")
            .or_else(|| env_string("NATS_WATCH_EVENT_SUBJECT"))
            .unwrap_or_else(|| RUNTIME_EVENTS_SUBJECT.to_string())
    }
}

async fn reconcile_queue_consumer_floor(job: &RuntimeFloorJob) -> Result<Value, String> {
    let timeout_seconds = job.interval_seconds.min(20).max(5);
    let (client, base_url, token) = k8s_runtime_client(timeout_seconds).await?;
    let deployment_path = format!(
        "/apis/apps/v1/namespaces/{}/deployments/{}",
        job.k8s_namespace, job.queue_consumer_deployment
    );
    let deployment = k8s_get_object(&client, &base_url, &token, deployment_path).await?;
    let desired = json_at_i64(&deployment, &["spec", "replicas"]).unwrap_or(0);
    let ready = json_at_i64(&deployment, &["status", "readyReplicas"]).unwrap_or(0);
    let available = json_at_i64(&deployment, &["status", "availableReplicas"]).unwrap_or(0);
    let updated = json_at_i64(&deployment, &["status", "updatedReplicas"]).unwrap_or(0);
    let mut scaled = false;

    if desired < job.min_queue_consumer_replicas {
        let scale_path = format!(
            "/apis/apps/v1/namespaces/{}/deployments/{}/scale",
            job.k8s_namespace, job.queue_consumer_deployment
        );
        k8s_patch_merge(
            &client,
            &base_url,
            &token,
            scale_path,
            json!({ "spec": { "replicas": job.min_queue_consumer_replicas } }),
        )
        .await?;
        scaled = true;
    }

    Ok(json!({
        "ok": ready >= job.min_queue_consumer_ready,
        "scaled": scaled,
        "desiredReplicas": desired.max(job.min_queue_consumer_replicas),
        "readyReplicas": ready,
        "availableReplicas": available,
        "updatedReplicas": updated,
        "minReady": job.min_queue_consumer_ready,
    }))
}

async fn reconcile_container_pool_floor(
    http: &Client,
    job: &RuntimeFloorJob,
) -> Result<Value, String> {
    let base = job.container_pool_url.trim_end_matches('/');
    let response = http
        .get(format!("{base}/pools"))
        .header("X-Server-Auth", &job.server_auth_secret)
        .send()
        .await
        .map_err(|error| format!("container pool list failed: {error}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "container pool list failed status={} body={}",
            status,
            body.chars().take(500).collect::<String>()
        ));
    }

    let pools = serde_json::from_str::<RuntimeFloorPoolsResponse>(&body)
        .map_err(|error| format!("container pool list invalid json: {error}"))?
        .pools;
    let mut warm_requests = Vec::new();
    let mut errors = Vec::new();
    let mut ready_pools = 0usize;
    for pool in pools.iter().filter(|pool| pool.min_warm > 0) {
        if pool.idle_containers >= pool.min_warm {
            ready_pools += 1;
            continue;
        }
        let warm_response = http
            .post(format!("{base}/pools/{}/warm", pool.slug))
            .header("X-Server-Auth", &job.server_auth_secret)
            .send()
            .await;
        match warm_response {
            Ok(response) if response.status().is_success() => {
                warm_requests.push(json!({
                    "pool": &pool.slug,
                    "idleContainers": pool.idle_containers,
                    "minWarm": pool.min_warm,
                    "activeContainers": pool.active_containers,
                    "unhealthyContainers": pool.unhealthy_containers,
                }));
            }
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                errors.push(json!({
                    "pool": &pool.slug,
                    "status": status.as_u16(),
                    "body": body.chars().take(500).collect::<String>(),
                }));
            }
            Err(error) => {
                errors.push(json!({
                    "pool": &pool.slug,
                    "error": error.to_string(),
                }));
            }
        }
    }

    Ok(json!({
        "ok": errors.is_empty(),
        "readyPools": ready_pools,
        "configuredWarmPools": pools.iter().filter(|pool| pool.min_warm > 0).count(),
        "warmRequests": warm_requests,
        "errors": errors,
    }))
}

async fn run_runtime_floor_once(http: &Client, job: &RuntimeFloorJob) {
    let nats = match ensure_runtime_floor_nats(job).await {
        Ok(client) => Some(client),
        Err(error) => {
            eprintln!("{error}");
            None
        }
    };

    match reconcile_queue_consumer_floor(job).await {
        Ok(summary) => {
            if summary.get("ok").and_then(Value::as_bool) != Some(true) {
                eprintln!("runtime floor queue consumer below ready floor: {summary}");
                publish_runtime_floor_event(
                    nats.as_ref(),
                    job,
                    "warning",
                    "queue consumer is below its ready floor",
                    summary,
                )
                .await;
            }
        }
        Err(error) => {
            eprintln!("runtime floor queue consumer reconcile failed: {error}");
            publish_runtime_floor_event(
                nats.as_ref(),
                job,
                "error",
                "queue consumer reconcile failed",
                json!({ "error": error }),
            )
            .await;
        }
    }

    match reconcile_container_pool_floor(http, job).await {
        Ok(summary) => {
            let has_errors = summary
                .get("errors")
                .and_then(Value::as_array)
                .is_some_and(|errors| !errors.is_empty());
            let has_warm_requests = summary
                .get("warmRequests")
                .and_then(Value::as_array)
                .is_some_and(|requests| !requests.is_empty());
            if has_errors || has_warm_requests {
                publish_runtime_floor_event(
                    nats.as_ref(),
                    job,
                    if has_errors { "warning" } else { "ok" },
                    "container pool warm floor reconciled",
                    summary,
                )
                .await;
            }
        }
        Err(error) => {
            eprintln!("runtime floor container pool reconcile failed: {error}");
            publish_runtime_floor_event(
                nats.as_ref(),
                job,
                "error",
                "container pool reconcile failed",
                json!({ "error": error }),
            )
            .await;
        }
    }
}

async fn run_runtime_floor_loop(client: Client, job: RuntimeFloorJob) {
    println!(
        "runtime floor starting: interval={}s stream={} consumer={} queueDeployment={}/{} containerPool={}",
        job.interval_seconds,
        job.task_stream,
        job.task_consumer,
        job.k8s_namespace,
        job.queue_consumer_deployment,
        job.container_pool_url
    );

    loop {
        run_runtime_floor_once(&client, &job).await;
        sleep(Duration::from_secs(job.interval_seconds)).await;
    }
}

#[tokio::main]
async fn main() {
    let timeout_seconds = env_u64("REAPER_TIMEOUT_SECONDS", 25);
    let client = Client::builder()
        .timeout(Duration::from_secs(timeout_seconds))
        .build()
        .expect("failed to construct reqwest client");

    let sweep_job = sweep_job_from_env();
    let cluster_doctor_job = cluster_doctor_job_from_env();
    let nats_watch_job = nats_watch_job_from_env();
    let runtime_floor_job = runtime_floor_job_from_env();
    let worker_image_build_job = worker_image_build_job_from_env();
    let k8s_runtime_watch_job = k8s_runtime_watch_job_from_env();
    let browser_job_reap_job = browser_job_reap_job_from_env();

    println!("idle-reaper starting: timeout={}s", timeout_seconds);

    let mut enabled_jobs = 0;
    if let Some(sweep) = sweep_job {
        enabled_jobs += 1;
        tokio::spawn(run_sweep_loop(client.clone(), sweep));
    }
    if let Some(doctor) = cluster_doctor_job {
        enabled_jobs += 1;
        tokio::spawn(run_cluster_doctor_loop(client.clone(), doctor));
    }
    if let Some(nats_watch) = nats_watch_job {
        enabled_jobs += 1;
        tokio::spawn(run_nats_watch_loop(client.clone(), nats_watch));
    }
    if let Some(runtime_floor) = runtime_floor_job {
        enabled_jobs += 1;
        tokio::spawn(run_runtime_floor_loop(client.clone(), runtime_floor));
    }
    if let Some(worker_image_build) = worker_image_build_job {
        enabled_jobs += 1;
        tokio::spawn(run_worker_image_build_loop(worker_image_build));
    }
    if let Some(k8s_runtime_watch) = k8s_runtime_watch_job {
        enabled_jobs += 1;
        tokio::spawn(run_k8s_runtime_watch_loop(k8s_runtime_watch));
    }
    if let Some(browser_job_reap) = browser_job_reap_job {
        enabled_jobs += 1;
        tokio::spawn(run_browser_job_reap_loop(browser_job_reap));
    }

    if enabled_jobs == 0 {
        loop {
            println!("idle-reaper has no enabled jobs; sleeping");
            sleep(Duration::from_secs(300)).await;
        }
    }

    loop {
        sleep(Duration::from_secs(3600)).await;
    }
}
