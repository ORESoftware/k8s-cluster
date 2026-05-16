use std::{env, os::unix::fs::PermissionsExt, time::Duration};

use chrono::{TimeZone, Utc};
use chrono_tz::Tz;
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tokio::{fs, process::Command, time::sleep};

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueTaskMessage {
    thread_id: String,
    task_id: String,
    shadow: Option<bool>,
    direct_dispatch: Option<bool>,
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
            .unwrap_or_else(|| "dd.remote.thread.*.tasks".to_string()),
        event_subject: env_string("NATS_WATCH_EVENT_SUBJECT")
            .unwrap_or_else(|| "dd.remote.events".to_string()),
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
        repo_url: env_string("WORKER_IMAGE_BUILD_REPO_URL")
            .unwrap_or_else(|| "git@github.com:ORESoftware/k8s-cluster.git".to_string()),
        repo_ref: env_string("WORKER_IMAGE_BUILD_REF").unwrap_or_else(|| "dev".to_string()),
        image: env_string("WORKER_IMAGE_BUILD_IMAGE")
            .unwrap_or_else(|| "docker.io/library/dd-dev-server:dev".to_string()),
        nerdctl: env_string("WORKER_IMAGE_BUILD_NERDCTL")
            .unwrap_or_else(|| "/usr/local/bin/nerdctl".to_string()),
        deploy_key: deploy_key.expect("checked above"),
        nats_url: env_string("NATS_URL")
            .unwrap_or_else(|| "nats://dd-nats.messaging.svc.cluster.local:4222".to_string()),
        event_subject: env_string("WORKER_IMAGE_BUILD_EVENT_SUBJECT")
            .unwrap_or_else(|| "dd.remote.events".to_string()),
        timezone,
        hour: env_u64("WORKER_IMAGE_BUILD_HOUR", 4).min(23) as u32,
        minute: env_u64("WORKER_IMAGE_BUILD_MINUTE", 0).min(59) as u32,
        run_on_start: env_bool("WORKER_IMAGE_BUILD_RUN_ON_START", false),
    })
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
        .arg("remote/dev-server");
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
                                            Ok(task) => prepare_thread_from_nats(&client, &job, &task).await,
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
    let worker_image_build_job = worker_image_build_job_from_env();

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
    if let Some(worker_image_build) = worker_image_build_job {
        enabled_jobs += 1;
        tokio::spawn(run_worker_image_build_loop(worker_image_build));
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
