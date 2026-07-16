use std::{
    collections::hash_map::DefaultHasher,
    collections::HashSet,
    env,
    error::Error,
    fs,
    hash::{Hash, Hasher},
    io::Write,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use dd_nats_subject_defs::{
    DD_REMOTE_CRITICAL_EVENTS_STREAM_NAME, DD_REMOTE_TASKS_STREAM_NAME,
    RUNTIME_CRITICAL_EVENTS_QUEUE_GROUP, RUNTIME_CRITICAL_EVENTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
    THREAD_PREPARER_QUEUE_GROUP, THREAD_TASKS_WILDCARD,
};
use dd_shared_interfaces::AgentTaskQueueMessage;
use futures_util::StreamExt;
use serde_json::{json, Value};

type QueueTaskMessage = AgentTaskQueueMessage;

const SERVICE_NAME: &str = "dd-remote-queue-consumer";
const SERVICE_NAMESPACE: &str = "remote-dev";
const LOG_SCHEMA: &str = "dd.log.v1";
const LOG_SCOPE: &str = "dd-remote-queue-consumer";
const DEFAULT_SERVER_SECRET: &str = "dd-k8s-home";
const MAX_IDENTIFIER_LEN: usize = 200;
// Caps the in-memory duplicate-suppression cache so a long-lived pod can't
// grow it without bound. The on-disk receipt files remain the durable check;
// this set is only a fast path, so trimming it is safe.
const MAX_RECEIPT_CACHE: usize = 50_000;

/// Reject identifiers that are empty, overlong, or carry characters that would
/// let a NATS payload steer the REST request path (`/api/agents/threads/{id}/
/// prepare`) or escape the receipts directory. Thread/task ids are UUIDs in
/// the producer, so this never rejects legitimate traffic; it only blocks
/// crafted values like `../../admin` or ids with embedded slashes/NULs.
///
/// The id is interpolated raw into REST URLs, so the check is a strict
/// allowlist rather than a denylist: only ASCII alphanumerics plus `-`, `_`,
/// and `.` are permitted. A denylist of just `/`, `\`, and control characters
/// would still let URL-significant bytes through — `?`/`#` open a query string
/// or fragment and `%` begins a percent-escape, any of which can redirect the
/// request — so those are rejected here too.
fn validate_identifier(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if value.len() > MAX_IDENTIFIER_LEN {
        return Err(format!(
            "{label} must be at most {MAX_IDENTIFIER_LEN} bytes"
        ));
    }
    if value.contains("..") {
        return Err(format!("{label} must not contain '..'"));
    }
    if let Some(bad) = value
        .chars()
        .find(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')))
    {
        return Err(format!(
            "{label} must contain only ASCII alphanumerics, '-', '_', or '.' (found {bad:?})"
        ));
    }
    Ok(())
}

fn validate_task_identifiers(task: &QueueTaskMessage) -> Result<(), String> {
    validate_identifier(&task.thread_id, "threadId")?;
    validate_identifier(&task.task_id, "taskId")?;
    Ok(())
}

/// Record a processed task id in the in-memory fast-path cache, trimming it if
/// it has grown past the cap. The durable check is the on-disk receipt.
fn record_receipt(receipts: &mut HashSet<String>, task_id: &str) {
    if receipts.len() >= MAX_RECEIPT_CACHE {
        receipts.clear();
    }
    receipts.insert(task_id.to_string());
}

/// Capped exponential backoff for a run of consecutive JetStream fetch errors.
///
/// On a persistent failure (broker down, consumer deleted) the message loops
/// would otherwise `continue` instantly, spinning the CPU, hammering NATS, and
/// flooding critical events. This spaces retries out — 250ms, 500ms, 1s, 2s,
/// 4s, then capped at 5s — while async-nats reconnects underneath. The counter
/// resets to zero on the next successful fetch.
fn fetch_error_backoff(consecutive_errors: u32) -> Duration {
    let exponent = consecutive_errors.saturating_sub(1).min(5);
    let millis = (250u64 << exponent).min(5_000);
    Duration::from_millis(millis)
}

fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn env_i64(key: &str, fallback: i64) -> i64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn env_bool(key: &str, fallback: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(fallback)
}

fn server_auth_secret() -> String {
    env::var("REMOTE_DEV_SERVER_SECRET")
        .or_else(|_| env::var("SERVER_AUTH_SECRET"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVER_SECRET.to_string())
}

fn receipt_path(base_dir: &str, task_id: &str) -> PathBuf {
    let safe_task_id = task_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect::<String>();
    // The sanitized id alone is lossy: two distinct ids can collapse to the
    // same string (e.g. `a/b` and `ab`, or any id made only of stripped
    // characters), which would make one task silently suppress the other.
    // Append a hash of the *raw* id so the filename is unique per real id
    // while staying filesystem-safe and human-greppable.
    let mut hasher = DefaultHasher::new();
    task_id.hash(&mut hasher);
    let digest = hasher.finish();
    PathBuf::from(base_dir).join(format!("{safe_task_id}-{digest:016x}.json"))
}

fn has_task_receipt(receipts: &mut HashSet<String>, base_dir: &str, task_id: &str) -> bool {
    if receipts.contains(task_id) {
        return true;
    }
    let path = receipt_path(base_dir, task_id);
    let valid_receipt = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| {
            value
                .get("taskId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|recorded_task_id| recorded_task_id == task_id);
    if valid_receipt {
        record_receipt(receipts, task_id);
        return true;
    }
    false
}

fn write_task_receipt(
    base_dir: &str,
    task: &QueueTaskMessage,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    fs::create_dir_all(base_dir)?;
    let destination = receipt_path(base_dir, &task.task_id);
    let temporary = destination.with_extension(format!(
        "json.tmp-{}-{}",
        std::process::id(),
        now_unix_nano()
    ));
    let encoded = serde_json::to_vec_pretty(&serde_json::json!({
        "threadId": &task.thread_id,
        "taskId": &task.task_id,
        "messageKind": &task.message_kind,
        "shadow": task.shadow.unwrap_or(false),
        "directDispatch": task.direct_dispatch.unwrap_or(false),
    }))?;

    // A receipt is a task-suppression decision, so it must never become visible
    // half-written. Flush a uniquely named file and atomically rename it into
    // place; readers also validate its JSON/task id before trusting it.
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    fs::rename(temporary, destination)?;
    Ok(())
}

fn is_shadow_task(task: &QueueTaskMessage) -> bool {
    task.shadow.unwrap_or(false)
        || task
            .message_kind
            .as_deref()
            .is_some_and(|kind| kind == "task.shadow")
}

fn is_container_pool_dispatch_mode(mode: &str) -> bool {
    matches!(
        mode,
        "queued-pool" | "nats-pool" | "container-pool" | "pool"
    )
}

fn should_dispatch_to_container_pool(task: &QueueTaskMessage) -> bool {
    task.container_pool_dispatch.unwrap_or_else(|| {
        task.dispatch_mode
            .as_deref()
            .map(str::trim)
            .filter(|mode| !mode.is_empty())
            .is_some_and(is_container_pool_dispatch_mode)
    })
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn now_unix_nano() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn severity_number(severity: &str) -> i32 {
    match severity {
        "FATAL" => 24,
        "ERROR" => 17,
        "WARN" => 13,
        "INFO" => 9,
        "DEBUG" => 5,
        _ => 1,
    }
}

fn structured_log_record(severity: &str, event_name: &str, body: &str, attributes: Value) -> Value {
    json!({
        "schema": LOG_SCHEMA,
        "time_unix_nano": now_unix_nano().to_string(),
        "severity_text": severity,
        "severity_number": severity_number(severity),
        "body": body,
        "resource_service_name": SERVICE_NAME,
        "resource_service_namespace": SERVICE_NAMESPACE,
        "scope_name": LOG_SCOPE,
        "event_name": event_name,
        "attributes": attributes,
    })
}

fn write_structured_log_to_stdout(severity: &str, event_name: &str, body: &str, attributes: Value) {
    let record = structured_log_record(severity, event_name, body, attributes);
    match serde_json::to_string(&record) {
        Ok(line) => tracing::info!("{line}"),
        Err(error) => tracing::info!(
            "{{\"schema\":\"{LOG_SCHEMA}\",\"severity_text\":\"ERROR\",\"body\":\"structured log serialization failed\",\"resource_service_name\":\"{SERVICE_NAME}\",\"event_name\":\"structured-log-serialize-failed\",\"attributes\":{{\"error\":\"{error}\"}}}}"
        ),
    }
}

fn write_structured_log_to_stderr(severity: &str, event_name: &str, body: &str, attributes: Value) {
    let record = structured_log_record(severity, event_name, body, attributes);
    match serde_json::to_string(&record) {
        Ok(line) => tracing::error!("{line}"),
        Err(error) => tracing::error!(
            "{{\"schema\":\"{LOG_SCHEMA}\",\"severity_text\":\"ERROR\",\"body\":\"structured log serialization failed\",\"resource_service_name\":\"{SERVICE_NAME}\",\"event_name\":\"structured-log-serialize-failed\",\"attributes\":{{\"error\":\"{error}\"}}}}"
        ),
    }
}

fn log_info(event_name: &str, body: &str, attributes: Value) {
    write_structured_log_to_stdout("INFO", event_name, body, attributes);
}

fn log_warn(event_name: &str, body: &str, attributes: Value) {
    write_structured_log_to_stderr("WARN", event_name, body, attributes);
}

fn log_error(event_name: &str, body: &str, attributes: Value) {
    write_structured_log_to_stderr("ERROR", event_name, body, attributes);
}

fn nats_event_subject() -> String {
    env_value("NATS_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT)
}

fn critical_event_subject() -> String {
    env_value(
        "NATS_CRITICAL_EVENT_SUBJECT",
        RUNTIME_CRITICAL_EVENTS_SUBJECT,
    )
}

fn critical_event_stream_name() -> String {
    env_value(
        "NATS_CRITICAL_EVENT_STREAM",
        DD_REMOTE_CRITICAL_EVENTS_STREAM_NAME,
    )
}

fn critical_event_consumer_name() -> String {
    env_value(
        "NATS_CRITICAL_EVENT_CONSUMER",
        RUNTIME_CRITICAL_EVENTS_QUEUE_GROUP,
    )
}

fn string_at<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

fn compact_critical_event_attributes(
    subject: &str,
    payload_bytes: usize,
    payload: &Value,
) -> Value {
    let log = payload.get("log").unwrap_or(&Value::Null);
    let log_attributes = log.get("attributes").unwrap_or(&Value::Null);
    json!({
        "criticalSubject": subject,
        "payloadBytes": payload_bytes,
        "upstreamSchema": string_at(payload, "/schema"),
        "upstreamType": string_at(payload, "/type"),
        "upstreamSource": string_at(payload, "/source")
            .or_else(|| string_at(log, "/resource_service_name")),
        "upstreamEventName": string_at(payload, "/eventName")
            .or_else(|| string_at(log, "/event_name")),
        "upstreamSeverity": string_at(payload, "/severity")
            .or_else(|| string_at(log, "/severity_text")),
        "threadId": string_at(log_attributes, "/threadId")
            .or_else(|| string_at(log_attributes, "/dd.request.thread_id"))
            .or_else(|| string_at(payload, "/threadId")),
        "taskId": string_at(log_attributes, "/taskId")
            .or_else(|| string_at(log_attributes, "/dd.request.task_id"))
            .or_else(|| string_at(payload, "/taskId")),
    })
}

fn task_message_id(task: &QueueTaskMessage, stage: &str) -> String {
    format!("{}:{stage}", task.task_id)
}

fn queue_status_event(
    task: &QueueTaskMessage,
    stage: &str,
    status: &str,
    message: &str,
    details: Value,
) -> Value {
    json!({
        "kind": "status",
        "status": status,
        "message": message,
        "source": "dd-remote-queue-consumer",
        "stage": stage,
        "messageKind": &task.message_kind,
        "shadow": task.shadow.unwrap_or(false),
        "directDispatch": task.direct_dispatch.unwrap_or(false),
        "details": details,
        "atMs": now_ms(),
    })
}

async fn persist_queue_status_event(
    http: &reqwest::Client,
    rest_api_url: &str,
    secret: &str,
    task: &QueueTaskMessage,
    seq: i32,
    event: &Value,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let base = rest_api_url.trim_end_matches('/');
    let url = format!("{base}/api/agents/events");
    let response = http
        .post(url)
        .header("X-Agent-Auth", secret)
        .json(&json!({
            "taskId": &task.task_id,
            "threadId": &task.thread_id,
            "seq": seq,
            "event": event,
        }))
        .send()
        .await?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    Err(format!(
        "queue status event ingest failed with {status}: {}",
        body.chars().take(500).collect::<String>()
    )
    .into())
}

async fn publish_queue_status_event(
    nats: &async_nats::Client,
    task: &QueueTaskMessage,
    seq: i32,
    stage: &str,
    event: &Value,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let payload = json!({
        "type": "task-event",
        "messageId": task_message_id(task, stage),
        "threadId": &task.thread_id,
        "taskId": &task.task_id,
        "seq": seq,
        "event": event,
        "emittedAt": now_ms(),
    });
    nats.publish(nats_event_subject(), serde_json::to_vec(&payload)?.into())
        .await?;
    nats.flush().await?;
    Ok(())
}

async fn publish_runtime_critical_event(
    nats: &async_nats::Client,
    critical_subject: &str,
    event_name: &str,
    body: &str,
    attributes: Value,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let log = structured_log_record("ERROR", event_name, body, attributes);
    let payload = json!({
        "type": "runtime-critical-event",
        "schema": "dd.runtime_critical_event.v1",
        "source": SERVICE_NAME,
        "eventName": event_name,
        "severity": "ERROR",
        "log": log,
        "emittedAtMs": now_ms(),
    });
    nats.publish(
        critical_subject.to_string(),
        serde_json::to_vec(&payload)?.into(),
    )
    .await?;
    nats.flush().await?;
    Ok(())
}

async fn emit_runtime_critical_event(
    nats: &async_nats::Client,
    critical_subject: &str,
    event_name: &str,
    body: &str,
    attributes: Value,
) {
    log_error(event_name, body, attributes.clone());
    if let Err(error) =
        publish_runtime_critical_event(nats, critical_subject, event_name, body, attributes).await
    {
        log_error(
            "critical-event-publish-failed",
            "Runtime critical event NATS publish failed.",
            json!({
                "criticalSubject": critical_subject,
                "eventName": event_name,
                "error": error.to_string(),
            }),
        );
    }
}

// Keeping the event's semantic fields explicit at call sites makes the ordered
// worker breadcrumb stream reviewable; grouping them would obscure which value
// is the persisted sequence, stage, status, or operator-facing message.
#[allow(clippy::too_many_arguments)]
async fn emit_queue_status_event(
    http: &reqwest::Client,
    nats: &async_nats::Client,
    rest_api_url: &str,
    secret: &str,
    task: &QueueTaskMessage,
    seq: i32,
    stage: &str,
    status: &str,
    message: &str,
    details: Value,
) {
    let event = queue_status_event(task, stage, status, message, details);
    if let Err(error) =
        persist_queue_status_event(http, rest_api_url, secret, task, seq, &event).await
    {
        log_warn(
            "queue-status-event-persist-failed",
            "Queue status event REST persist failed.",
            json!({
                "threadId": &task.thread_id,
                "taskId": &task.task_id,
                "stage": stage,
                "error": error.to_string(),
            }),
        );
    }
    if let Err(error) = publish_queue_status_event(nats, task, seq, stage, &event).await {
        log_warn(
            "queue-status-event-nats-publish-failed",
            "Queue status event NATS publish failed.",
            json!({
                "threadId": &task.thread_id,
                "taskId": &task.task_id,
                "stage": stage,
                "error": error.to_string(),
            }),
        );
    }
}

fn sanitize_slug_part(input: &str) -> String {
    let mut output = String::new();
    let mut last_dash = false;
    for ch in input.chars() {
        let next = if ch.is_ascii_alphanumeric() {
            Some(ch.to_ascii_lowercase())
        } else if !last_dash {
            Some('-')
        } else {
            None
        };
        if let Some(value) = next {
            last_dash = value == '-';
            output.push(value);
        }
    }
    output.trim_matches('-').chars().take(80).collect()
}

fn repo_pool_slug(repo: &str, base_branch: &str) -> String {
    let repo_name = repo
        .trim_end_matches(".git")
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("repo");
    let repo_part = sanitize_slug_part(repo_name);
    let branch_part = sanitize_slug_part(base_branch);
    format!("nodejs-chat-claude-{repo_part}-{branch_part}")
}

async fn dispatch_to_container_pool(
    http: &reqwest::Client,
    container_pool_url: &str,
    secret: &str,
    task: &QueueTaskMessage,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let repo = task
        .repo
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("queued task missing repo")?;
    let base_branch = task
        .base_branch
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("dev");
    let prompt = task
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("queued task missing prompt")?;
    let pool = repo_pool_slug(repo, base_branch);
    let base = container_pool_url.trim_end_matches('/');
    let url = format!("{base}/pools/{pool}/dispatch");
    let response = http
        .post(url)
        .header("X-Server-Auth", secret)
        .json(&serde_json::json!({
            "requestId": &task.task_id,
            "poolSlug": pool,
            "affinityKey": &task.thread_id,
            "freshAffinity": true,
            "path": "/tasks",
            "payload": {
                "taskId": &task.task_id,
                "threadId": &task.thread_id,
                "repo": repo,
                "baseBranch": base_branch,
                "prompt": prompt,
                "provider": &task.provider,
                "threadTitle": &task.thread_title,
                "contextMode": &task.context_mode,
                "contextIds": &task.context_ids,
            }
        }))
        .send()
        .await?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    Err(format!(
        "container pool dispatch failed with {status}: {}",
        body.chars().take(500).collect::<String>()
    )
    .into())
}

async fn prepare_thread(
    http: &reqwest::Client,
    rest_api_url: &str,
    secret: &str,
    thread_id: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let base = rest_api_url.trim_end_matches('/');
    let url = format!("{base}/api/agents/threads/{thread_id}/prepare");
    let response = http.post(url).header("X-Agent-Auth", secret).send().await?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }

    let body = response.text().await.unwrap_or_default();
    Err(format!(
        "prepare failed with {status}: {}",
        body.chars().take(500).collect::<String>()
    )
    .into())
}

async fn dispatch_to_rest_api(
    http: &reqwest::Client,
    rest_api_url: &str,
    secret: &str,
    task: &QueueTaskMessage,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let prompt = task
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("queued task missing prompt")?;
    let base = rest_api_url.trim_end_matches('/');
    let url = format!("{base}/api/agents/threads/{}/tasks", task.thread_id);
    let response = http
        .post(url)
        .header("X-Agent-Auth", secret)
        .json(&serde_json::json!({
            "threadId": &task.thread_id,
            "taskId": &task.task_id,
            "prompt": prompt,
            "provider": &task.provider,
            "repo": &task.repo,
            "baseBranch": &task.base_branch,
            "threadTitle": &task.thread_title,
            "contextMode": &task.context_mode,
            "contextIds": &task.context_ids,
            "dispatchMode": "direct",
        }))
        .send()
        .await?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    Err(format!(
        "rest fallback dispatch failed with {status}: {}",
        body.chars().take(500).collect::<String>()
    )
    .into())
}

async fn dispatch_to_deterministic_worker(
    http: &reqwest::Client,
    rest_api_url: &str,
    secret: &str,
    task: &QueueTaskMessage,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    prepare_thread(http, rest_api_url, secret, &task.thread_id).await?;
    dispatch_to_rest_api(http, rest_api_url, secret, task).await
}

struct JetStreamConsumerConfig<'a> {
    stream_name: &'a str,
    subject: &'a str,
    consumer_name: &'a str,
    retention: async_nats::jetstream::stream::RetentionPolicy,
    ack_wait: Duration,
    max_ack_pending: i64,
    max_deliver: i64,
}

async fn build_jetstream_consumer(
    client: async_nats::Client,
    config: JetStreamConsumerConfig<'_>,
) -> Result<async_nats::jetstream::consumer::PullConsumer, Box<dyn Error + Send + Sync>> {
    let jetstream = async_nats::jetstream::new(client);
    let stream = jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: config.stream_name.to_string(),
            subjects: vec![config.subject.to_string()],
            retention: config.retention,
            max_age: Duration::from_secs(60 * 60 * 24 * 14),
            max_message_size: 8 * 1024 * 1024,
            ..Default::default()
        })
        .await?;

    let consumer = stream
        .get_or_create_consumer::<async_nats::jetstream::consumer::pull::Config>(
            config.consumer_name,
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(config.consumer_name.to_string()),
                filter_subject: config.subject.to_string(),
                ack_wait: config.ack_wait,
                max_ack_pending: config.max_ack_pending,
                max_deliver: config.max_deliver,
                ..Default::default()
            },
        )
        .await?;

    Ok(consumer)
}

async fn run_critical_event_logger(
    client: async_nats::Client,
    stream_name: String,
    subject: String,
    consumer_name: String,
    ack_wait: Duration,
    max_ack_pending: i64,
    max_deliver: i64,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let consumer = build_jetstream_consumer(
        client,
        JetStreamConsumerConfig {
            stream_name: &stream_name,
            subject: &subject,
            consumer_name: &consumer_name,
            retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
            ack_wait,
            max_ack_pending,
            max_deliver,
        },
    )
    .await?;
    let mut messages = consumer.messages().await?;
    log_info(
        "critical-event-logger-started",
        "Critical runtime event logger started.",
        json!({
            "stream": &stream_name,
            "subject": &subject,
            "consumer": &consumer_name,
        }),
    );
    let mut consecutive_fetch_errors: u32 = 0;

    while let Some(message) = messages.next().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                consecutive_fetch_errors = consecutive_fetch_errors.saturating_add(1);
                log_error(
                    "critical-event-fetch-failed",
                    "Critical runtime event fetch failed.",
                    json!({
                        "stream": &stream_name,
                        "subject": &subject,
                        "consumer": &consumer_name,
                        "consecutiveErrors": consecutive_fetch_errors,
                        "error": error.to_string(),
                    }),
                );
                // Back off so a persistent failure can't spin this loop.
                tokio::time::sleep(fetch_error_backoff(consecutive_fetch_errors)).await;
                continue;
            }
        };
        consecutive_fetch_errors = 0;

        let message_subject = message.subject.to_string();
        match serde_json::from_slice::<Value>(&message.payload) {
            Ok(payload) => {
                let log = payload.get("log").unwrap_or(&Value::Null);
                let body = string_at(log, "/body")
                    .or_else(|| string_at(&payload, "/message"))
                    .unwrap_or("Runtime critical event received.");
                log_error(
                    "runtime-critical-event-received",
                    body,
                    compact_critical_event_attributes(
                        &message_subject,
                        message.payload.len(),
                        &payload,
                    ),
                );
            }
            Err(error) => {
                log_error(
                    "critical-event-payload-invalid",
                    "Critical runtime event payload was not valid JSON.",
                    json!({
                        "stream": &stream_name,
                        "subject": &message_subject,
                        "payloadBytes": message.payload.len(),
                        "error": error.to_string(),
                    }),
                );
            }
        }

        if let Err(error) = message.ack().await {
            log_error(
                "critical-event-ack-failed",
                "Critical runtime event acknowledgement failed.",
                json!({
                    "stream": &stream_name,
                    "subject": &subject,
                    "consumer": &consumer_name,
                    "error": error.to_string(),
                }),
            );
        }
    }

    Ok(())
}

fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Build a hardened NATS client from `nats_url` plus optional auth/TLS env.
///
/// Replaces a bare `async_nats::connect(url)` (no client name, no auth, no
/// retry) with a connection that carries a stable name for server-side
/// observability, pings, a connect timeout, retries the initial connect, and
/// supports optional auth via `NATS_CREDENTIALS_FILE`/`NATS_TOKEN`/`NATS_NKEY`
/// plus `NATS_REQUIRE_TLS=true`.
async fn connect_nats(nats_url: &str) -> Result<async_nats::Client, Box<dyn Error + Send + Sync>> {
    let mut options = async_nats::ConnectOptions::new()
        .name(SERVICE_NAME)
        .retry_on_initial_connect()
        .ping_interval(Duration::from_secs(15))
        .connection_timeout(Duration::from_secs(10));

    if env_bool("NATS_REQUIRE_TLS", false) {
        options = options.require_tls(true);
    }

    // Auth precedence: credentials file (JWT+nkey) > token > nkey seed.
    if let Some(path) = optional_env("NATS_CREDENTIALS_FILE") {
        options = options
            .credentials_file(&path)
            .await
            .map_err(|error| format!("failed to read NATS_CREDENTIALS_FILE {path}: {error}"))?;
    } else if let Some(token) = optional_env("NATS_TOKEN") {
        options = options.token(token);
    } else if let Some(seed) = optional_env("NATS_NKEY") {
        options = options.nkey(seed);
    }

    Ok(options.connect(nats_url).await?)
}

/// Resolves when the process receives SIGTERM (Kubernetes rolling restart) or
/// SIGINT, so the message loop can stop pulling new work and exit cleanly
/// instead of being killed mid-handoff (which forces a JetStream redelivery).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(stream) => stream,
            Err(_) => return std::future::pending().await,
        };
        let mut interrupt = match signal(SignalKind::interrupt()) {
            Ok(stream) => stream,
            Err(_) => return std::future::pending().await,
        };
        tokio::select! {
            _ = terminate.recv() => {}
            _ = interrupt.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = dd_telemetry::init("dd-remote-queue-consumer");

    let nats_url = env_value(
        "NATS_URL",
        "nats://dd-nats.messaging.svc.cluster.local:4222",
    );
    let subject = env_value("NATS_TASK_SUBJECT", THREAD_TASKS_WILDCARD);
    let queue_group = env_value("NATS_QUEUE_GROUP", THREAD_PREPARER_QUEUE_GROUP);
    let stream_name = env_value("NATS_TASK_STREAM", DD_REMOTE_TASKS_STREAM_NAME);
    let consumer_name = env_value("NATS_TASK_CONSUMER", &queue_group);
    let ack_wait_seconds = env_u64("NATS_TASK_ACK_WAIT_SECONDS", 120);
    let max_ack_pending = env_i64("NATS_TASK_MAX_ACK_PENDING", 256);
    let max_deliver = env_i64("NATS_TASK_MAX_DELIVER", 5);
    let nak_delay_seconds = env_u64("NATS_TASK_NAK_DELAY_SECONDS", 15);
    let rest_api_url = env_value(
        "REMOTE_REST_API_URL",
        "http://dd-remote-rest-api.default.svc.cluster.local:8082",
    );
    let event_subject = nats_event_subject();
    let critical_subject = critical_event_subject();
    let critical_stream_name = critical_event_stream_name();
    let critical_consumer_name = critical_event_consumer_name();
    let critical_logger_enabled = env_bool("QUEUE_CONSUMER_CRITICAL_EVENT_LOGGER", true);
    let critical_ack_wait_seconds = env_u64("NATS_CRITICAL_EVENT_ACK_WAIT_SECONDS", 60);
    let critical_max_ack_pending = env_i64("NATS_CRITICAL_EVENT_MAX_ACK_PENDING", 512);
    let critical_max_deliver = env_i64("NATS_CRITICAL_EVENT_MAX_DELIVER", 5);
    let container_pool_url = env_value(
        "CONTAINER_POOL_BASE_URL",
        "http://dd-container-pool.default.svc.cluster.local:8102",
    );
    let http_timeout_seconds = env_u64("QUEUE_CONSUMER_HTTP_TIMEOUT_SECONDS", 420);
    let fallback_rest_dispatch = env_bool("QUEUE_CONSUMER_FALLBACK_REST_DISPATCH", true);
    let receipts_dir = env_value(
        "QUEUE_CONSUMER_RECEIPTS_DIR",
        "/tmp/dd-remote-queue-consumer/tasks",
    );
    let secret = server_auth_secret();
    if secret == DEFAULT_SERVER_SECRET {
        // The default secret is compiled into this binary, so anyone with the
        // image knows it. Let a hardened deploy fail closed rather than run
        // with a known-public credential on the X-Agent-Auth/X-Server-Auth
        // headers; the default stays a warning so existing dev pods are
        // unaffected.
        if env_bool("QUEUE_CONSUMER_REQUIRE_NONDEFAULT_SECRET", false) {
            log_error(
                "server-auth-secret-default-refused",
                "Refusing to start: QUEUE_CONSUMER_REQUIRE_NONDEFAULT_SECRET is set but the internal auth secret is the built-in default. Set REMOTE_DEV_SERVER_SECRET or SERVER_AUTH_SECRET.",
                json!({}),
            );
            return Err(
                "refusing to start with the built-in default internal auth secret while QUEUE_CONSUMER_REQUIRE_NONDEFAULT_SECRET is set"
                    .into(),
            );
        }
        log_warn(
            "server-auth-secret-default",
            "Using the built-in default internal auth secret; set REMOTE_DEV_SERVER_SECRET or SERVER_AUTH_SECRET.",
            json!({}),
        );
    }
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(http_timeout_seconds))
        .build()?;

    log_info(
        "queue-consumer-starting",
        "Queue consumer starting.",
        json!({
            "natsUrl": &nats_url,
            "stream": &stream_name,
            "subject": &subject,
            "eventSubject": &event_subject,
            "criticalSubject": &critical_subject,
            "criticalStream": &critical_stream_name,
            "criticalConsumer": &critical_consumer_name,
            "criticalLoggerEnabled": critical_logger_enabled,
            "consumer": &consumer_name,
            "restApiUrl": &rest_api_url,
            "containerPoolUrl": &container_pool_url,
            "httpTimeoutSeconds": http_timeout_seconds,
            "fallbackRestDispatch": fallback_rest_dispatch,
            "receiptsDir": &receipts_dir,
        }),
    );
    let nats_client = connect_nats(&nats_url).await?;
    if critical_logger_enabled {
        let critical_client = nats_client.clone();
        let critical_stream_for_task = critical_stream_name.clone();
        let critical_subject_for_task = critical_subject.clone();
        let critical_consumer_for_task = critical_consumer_name.clone();
        // Supervise the logger: if it ever returns (clean stream end or error)
        // the process must not keep running deaf to critical events. Restart it
        // with a capped backoff so a flapping broker can't hot-loop respawns.
        tokio::spawn(async move {
            let mut restart_delay = Duration::from_secs(1);
            loop {
                match run_critical_event_logger(
                    critical_client.clone(),
                    critical_stream_for_task.clone(),
                    critical_subject_for_task.clone(),
                    critical_consumer_for_task.clone(),
                    Duration::from_secs(critical_ack_wait_seconds),
                    critical_max_ack_pending,
                    critical_max_deliver,
                )
                .await
                {
                    Ok(()) => log_warn(
                        "critical-event-logger-ended",
                        "Critical runtime event logger ended; restarting.",
                        json!({
                            "stream": &critical_stream_for_task,
                            "subject": &critical_subject_for_task,
                            "consumer": &critical_consumer_for_task,
                            "restartDelaySeconds": restart_delay.as_secs(),
                        }),
                    ),
                    Err(error) => log_error(
                        "critical-event-logger-stopped",
                        "Critical runtime event logger stopped; restarting.",
                        json!({
                            "stream": &critical_stream_for_task,
                            "subject": &critical_subject_for_task,
                            "consumer": &critical_consumer_for_task,
                            "restartDelaySeconds": restart_delay.as_secs(),
                            "error": error.to_string(),
                        }),
                    ),
                }
                tokio::time::sleep(restart_delay).await;
                restart_delay = (restart_delay * 2).min(Duration::from_secs(30));
            }
        });
    } else {
        log_warn(
            "critical-event-logger-disabled",
            "Critical runtime event logger is disabled.",
            json!({
                "stream": &critical_stream_name,
                "subject": &critical_subject,
                "consumer": &critical_consumer_name,
            }),
        );
    }
    let consumer = build_jetstream_consumer(
        nats_client.clone(),
        JetStreamConsumerConfig {
            stream_name: &stream_name,
            subject: &subject,
            consumer_name: &consumer_name,
            retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
            ack_wait: Duration::from_secs(ack_wait_seconds),
            max_ack_pending,
            max_deliver,
        },
    )
    .await?;
    let mut messages = consumer.messages().await?;
    let mut receipts = HashSet::new();
    let mut shutdown = std::pin::pin!(shutdown_signal());
    let mut consecutive_fetch_errors: u32 = 0;

    loop {
        // Race the next JetStream message against a shutdown signal. A signal
        // only wins while we are idle waiting for work, so an in-flight handoff
        // (in the loop body) always runs to completion before we exit.
        let message = tokio::select! {
            biased;
            _ = &mut shutdown => {
                log_info(
                    "queue-consumer-shutdown",
                    "Received shutdown signal; stopping the queue consumer message loop.",
                    json!({ "consumer": &consumer_name }),
                );
                break;
            }
            next = messages.next() => match next {
                Some(message) => message,
                None => break,
            },
        };
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                consecutive_fetch_errors = consecutive_fetch_errors.saturating_add(1);
                emit_runtime_critical_event(
                    &nats_client,
                    &critical_subject,
                    "jetstream-message-fetch-failed",
                    "JetStream message fetch failed.",
                    json!({
                        "stream": &stream_name,
                        "subject": &subject,
                        "consumer": &consumer_name,
                        "consecutiveErrors": consecutive_fetch_errors,
                        "error": error.to_string(),
                    }),
                )
                .await;
                // Back off so a persistent failure can't spin the loop, and let
                // a shutdown signal still win during the wait.
                tokio::select! {
                    biased;
                    _ = &mut shutdown => break,
                    _ = tokio::time::sleep(fetch_error_backoff(consecutive_fetch_errors)) => {}
                }
                continue;
            }
        };
        consecutive_fetch_errors = 0;
        let task = match serde_json::from_slice::<QueueTaskMessage>(&message.payload) {
            Ok(task) => task,
            Err(error) => {
                emit_runtime_critical_event(
                    &nats_client,
                    &critical_subject,
                    "invalid-queue-task-message",
                    "Queue consumer received an invalid task payload.",
                    json!({
                        "stream": &stream_name,
                        "subject": message.subject.to_string(),
                        "payloadBytes": message.payload.len(),
                        "error": error.to_string(),
                    }),
                )
                .await;
                if let Err(ack_error) = message.ack().await {
                    emit_runtime_critical_event(
                        &nats_client,
                        &critical_subject,
                        "invalid-queue-task-ack-failed",
                        "Queue consumer could not acknowledge an invalid task payload.",
                        json!({
                            "stream": &stream_name,
                            "subject": message.subject.to_string(),
                            "error": ack_error.to_string(),
                        }),
                    )
                    .await;
                }
                continue;
            }
        };
        if let Err(validation_error) = validate_task_identifiers(&task) {
            emit_runtime_critical_event(
                &nats_client,
                &critical_subject,
                "invalid-queue-task-identifiers",
                "Queue consumer received a task with an unsafe threadId or taskId.",
                json!({
                    "stream": &stream_name,
                    "subject": message.subject.to_string(),
                    "error": &validation_error,
                }),
            )
            .await;
            // Drop the poison message: a bad id can't become valid on retry,
            // and we must not let it steer the REST path or alias a receipt.
            if let Err(ack_error) = message.ack().await {
                emit_runtime_critical_event(
                    &nats_client,
                    &critical_subject,
                    "invalid-queue-task-identifiers-ack-failed",
                    "Queue consumer could not acknowledge a task with unsafe identifiers.",
                    json!({
                        "stream": &stream_name,
                        "subject": message.subject.to_string(),
                        "error": ack_error.to_string(),
                    }),
                )
                .await;
            }
            continue;
        }
        if has_task_receipt(&mut receipts, &receipts_dir, &task.task_id) {
            log_info(
                "queue-task-skipped-duplicate",
                "Queue task skipped because a receipt already exists.",
                json!({
                    "threadId": &task.thread_id,
                    "taskId": &task.task_id,
                    "receiptsDir": &receipts_dir,
                }),
            );
            if let Err(error) = message.ack().await {
                emit_runtime_critical_event(
                    &nats_client,
                    &critical_subject,
                    "duplicate-queue-task-ack-failed",
                    "Queue consumer could not acknowledge a duplicate task message.",
                    json!({
                        "threadId": &task.thread_id,
                        "taskId": &task.task_id,
                        "error": error.to_string(),
                    }),
                )
                .await;
            }
            continue;
        }
        let shadow = is_shadow_task(&task);
        log_info(
            "queue-task-received",
            "Queue consumer received a task message.",
            json!({
                "threadId": &task.thread_id,
                "taskId": &task.task_id,
                "messageKind": task.message_kind.as_deref().unwrap_or("unknown"),
                "shadow": shadow,
                "directDispatch": task.direct_dispatch.unwrap_or(false),
            }),
        );
        emit_queue_status_event(
            &http,
            &nats_client,
            &rest_api_url,
            &secret,
            &task,
            -940,
            "queue-received",
            "queue received",
            "Queue consumer received the JetStream task message.",
            json!({ "consumer": &consumer_name, "subject": &subject }),
        )
        .await;
        let direct_dispatch = task.direct_dispatch.unwrap_or(false);
        let container_pool_dispatch = should_dispatch_to_container_pool(&task);
        let result = if direct_dispatch {
            emit_queue_status_event(
                &http,
                &nats_client,
                &rest_api_url,
                &secret,
                &task,
                -930,
                "direct-dispatch-observed",
                "direct dispatch observed",
                "Synchronous REST dispatch owns worker creation and task execution; queue consumer is recording and acknowledging the duplicate JetStream message only.",
                json!({ "directDispatch": true }),
            )
            .await;
            Ok(())
        } else if shadow {
            emit_queue_status_event(
                &http,
                &nats_client,
                &rest_api_url,
                &secret,
                &task,
                -930,
                "shadow-prepare",
                "preparing shadow worker",
                "Shadow handoff is waking the UUID-bound thread worker.",
                json!({ "directDispatch": false }),
            )
            .await;
            prepare_thread(&http, &rest_api_url, &secret, &task.thread_id).await
        } else if !container_pool_dispatch {
            emit_queue_status_event(
                &http,
                &nats_client,
                &rest_api_url,
                &secret,
                &task,
                -930,
                "deterministic-worker-dispatch",
                "dispatching to deterministic worker",
                "Queued NATS mode is preparing the UUID-bound thread worker and dispatching through REST, without container-pool.",
                json!({ "dispatchMode": &task.dispatch_mode, "containerPoolDispatch": false }),
            )
            .await;
            match dispatch_to_deterministic_worker(&http, &rest_api_url, &secret, &task).await {
                Ok(()) => {
                    emit_queue_status_event(
                        &http,
                        &nats_client,
                        &rest_api_url,
                        &secret,
                        &task,
                        -920,
                        "deterministic-worker-accepted",
                        "deterministic worker accepted",
                        "UUID-bound thread worker accepted the queued NATS task dispatch.",
                        json!({ "dispatchMode": &task.dispatch_mode, "containerPoolDispatch": false }),
                    )
                    .await;
                    Ok(())
                }
                Err(error) => {
                    emit_queue_status_event(
                        &http,
                        &nats_client,
                        &rest_api_url,
                        &secret,
                        &task,
                        -920,
                        "deterministic-worker-failed",
                        "deterministic worker failed",
                        "Queued NATS mode could not prepare or dispatch to the UUID-bound thread worker.",
                        json!({ "dispatchMode": &task.dispatch_mode, "containerPoolDispatch": false, "error": error.to_string() }),
                    )
                    .await;
                    Err(error)
                }
            }
        } else {
            let pool = task
                .repo
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|repo| repo_pool_slug(repo, task.base_branch.as_deref().unwrap_or("dev")));
            emit_queue_status_event(
                &http,
                &nats_client,
                &rest_api_url,
                &secret,
                &task,
                -930,
                "container-pool-dispatch",
                "dispatching to container pool",
                "Queue consumer is asking container-pool for a warm repo worker.",
                json!({ "poolSlug": &pool, "affinityKey": &task.thread_id }),
            )
            .await;
            match dispatch_to_container_pool(&http, &container_pool_url, &secret, &task).await {
                Ok(()) => {
                    emit_queue_status_event(
                        &http,
                        &nats_client,
                        &rest_api_url,
                        &secret,
                        &task,
                        -920,
                        "container-pool-accepted",
                        "container pool accepted",
                        "Container-pool accepted the task dispatch.",
                        json!({ "poolSlug": &pool, "affinityKey": &task.thread_id }),
                    )
                    .await;
                    Ok(())
                }
                Err(pool_error) => {
                    let pool_error_summary =
                        pool_error.to_string().chars().take(300).collect::<String>();
                    let pool_error_message =
                        format!("Container-pool dispatch failed: {pool_error_summary}");
                    log_warn(
                        "container-pool-dispatch-failed",
                        "Container-pool dispatch failed; fallback may still recover the task.",
                        json!({
                            "threadId": &task.thread_id,
                            "taskId": &task.task_id,
                            "poolSlug": &pool,
                            "error": pool_error.to_string(),
                        }),
                    );
                    emit_queue_status_event(
                        &http,
                        &nats_client,
                        &rest_api_url,
                        &secret,
                        &task,
                        -920,
                        "container-pool-failed",
                        "container pool failed",
                        &pool_error_message,
                        json!({ "poolSlug": &pool, "affinityKey": &task.thread_id, "error": pool_error.to_string() }),
                    )
                    .await;
                    if !fallback_rest_dispatch {
                        Err(pool_error)
                    } else {
                        emit_queue_status_event(
                            &http,
                            &nats_client,
                            &rest_api_url,
                            &secret,
                            &task,
                            -915,
                            "rest-fallback-dispatch",
                            "falling back to direct worker",
                            "Container-pool did not accept the task; queue consumer is preparing the deterministic worker and dispatching through REST.",
                            json!({ "poolSlug": &pool, "affinityKey": &task.thread_id }),
                        )
                        .await;
                        match dispatch_to_deterministic_worker(&http, &rest_api_url, &secret, &task)
                            .await
                        {
                            Ok(()) => {
                                emit_queue_status_event(
                                    &http,
                                    &nats_client,
                                    &rest_api_url,
                                    &secret,
                                    &task,
                                    -914,
                                    "rest-fallback-accepted",
                                    "direct worker accepted",
                                    "Deterministic worker accepted the fallback task dispatch.",
                                    json!({ "poolSlug": &pool, "affinityKey": &task.thread_id }),
                                )
                                .await;
                                Ok(())
                            }
                            Err(rest_error) => {
                                let message = format!(
                                    "REST fallback dispatch failed after pool error: {rest_error}"
                                );
                                emit_queue_status_event(
                                    &http,
                                    &nats_client,
                                    &rest_api_url,
                                    &secret,
                                    &task,
                                    -914,
                                    "rest-fallback-failed",
                                    "direct worker fallback failed",
                                    &message,
                                    json!({
                                        "poolSlug": &pool,
                                        "affinityKey": &task.thread_id,
                                        "poolError": pool_error.to_string(),
                                        "restError": rest_error.to_string(),
                                    }),
                                )
                                .await;
                                Err(rest_error)
                            }
                        }
                    }
                }
            }
        };
        if let Err(error) = result {
            if shadow {
                let error_text = error.to_string();
                emit_runtime_critical_event(
                    &nats_client,
                    &critical_subject,
                    "shadow-prepare-failed",
                    "Queue consumer could not complete shadow worker warmup.",
                    json!({
                        "threadId": &task.thread_id,
                        "taskId": &task.task_id,
                        "shadow": true,
                        "directDispatch": false,
                        "error": &error_text,
                    }),
                )
                .await;
                emit_queue_status_event(
                    &http,
                    &nats_client,
                    &rest_api_url,
                    &secret,
                    &task,
                    -910,
                    "shadow-prepare-failed",
                    "shadow prepare failed",
                    "Queue consumer could not complete the shadow worker warmup; the original task dispatch already owns execution.",
                    json!({ "error": &error_text, "shadow": true, "directDispatch": false }),
                )
                .await;
                record_receipt(&mut receipts, &task.task_id);
                if let Err(error) = write_task_receipt(&receipts_dir, &task) {
                    emit_runtime_critical_event(
                        &nats_client,
                        &critical_subject,
                        "queue-task-receipt-write-failed",
                        "Queue consumer could not write a duplicate-suppression receipt.",
                        json!({
                            "threadId": &task.thread_id,
                            "taskId": &task.task_id,
                            "receiptsDir": &receipts_dir,
                            "error": error.to_string(),
                        }),
                    )
                    .await;
                }
                if let Err(error) = message.ack().await {
                    emit_runtime_critical_event(
                        &nats_client,
                        &critical_subject,
                        "queue-task-ack-failed-after-shadow-prepare-failure",
                        "Queue consumer could not acknowledge a shadow message after recording warmup failure.",
                        json!({
                            "threadId": &task.thread_id,
                            "taskId": &task.task_id,
                            "error": error.to_string(),
                        }),
                    )
                    .await;
                } else {
                    emit_queue_status_event(
                        &http,
                        &nats_client,
                        &rest_api_url,
                        &secret,
                        &task,
                        -900,
                        "queue-acked",
                        "queue message acked",
                        "Queue consumer acknowledged the non-executing JetStream message after recording the warmup failure.",
                        json!({ "shadow": shadow, "directDispatch": direct_dispatch }),
                    )
                    .await;
                }
                continue;
            }
            let error_text = error.to_string();
            emit_runtime_critical_event(
                &nats_client,
                &critical_subject,
                "queue-task-handoff-failed",
                "Queue consumer could not hand the task to a worker.",
                json!({
                    "threadId": &task.thread_id,
                    "taskId": &task.task_id,
                    "shadow": shadow,
                    "directDispatch": direct_dispatch,
                    "error": &error_text,
                }),
            )
            .await;
            emit_queue_status_event(
                &http,
                &nats_client,
                &rest_api_url,
                &secret,
                &task,
                -910,
                "queue-handoff-failed",
                "queue handoff failed",
                "Queue consumer could not hand the task to container-pool.",
                json!({ "error": &error_text }),
            )
            .await;
            if let Err(nak_error) = message
                .ack_with(async_nats::jetstream::AckKind::Nak(Some(
                    Duration::from_secs(nak_delay_seconds),
                )))
                .await
            {
                emit_runtime_critical_event(
                    &nats_client,
                    &critical_subject,
                    "queue-task-negative-ack-failed",
                    "Queue consumer could not NAK a failed task message.",
                    json!({
                        "threadId": &task.thread_id,
                        "taskId": &task.task_id,
                        "nakDelaySeconds": nak_delay_seconds,
                        "error": nak_error.to_string(),
                    }),
                )
                .await;
            }
            continue;
        }
        emit_queue_status_event(
            &http,
            &nats_client,
            &rest_api_url,
            &secret,
            &task,
            -910,
            "queue-handoff-ok",
            "queue handoff ok",
            "Queue consumer completed the worker handoff and will acknowledge the JetStream message.",
            json!({ "directDispatch": direct_dispatch }),
        )
        .await;
        record_receipt(&mut receipts, &task.task_id);
        if let Err(error) = write_task_receipt(&receipts_dir, &task) {
            emit_runtime_critical_event(
                &nats_client,
                &critical_subject,
                "queue-task-receipt-write-failed",
                "Queue consumer could not write a duplicate-suppression receipt.",
                json!({
                    "threadId": &task.thread_id,
                    "taskId": &task.task_id,
                    "receiptsDir": &receipts_dir,
                    "error": error.to_string(),
                }),
            )
            .await;
        }
        if let Err(error) = message.ack().await {
            emit_runtime_critical_event(
                &nats_client,
                &critical_subject,
                "queue-task-ack-failed",
                "Queue consumer could not acknowledge a successfully handed-off task.",
                json!({
                    "threadId": &task.thread_id,
                    "taskId": &task.task_id,
                    "error": error.to_string(),
                }),
            )
            .await;
        } else {
            emit_queue_status_event(
                &http,
                &nats_client,
                &rest_api_url,
                &secret,
                &task,
                -900,
                "queue-acked",
                "queue message acked",
                "Queue consumer acknowledged the JetStream message.",
                json!({}),
            )
            .await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_identifier_accepts_uuids_and_rejects_path_injection() {
        assert!(validate_identifier("018f6b1e-4c2a-7b9d-9f3a-2b1c0d4e5f6a", "id").is_ok());
        assert!(validate_identifier("trading-1700000000000", "id").is_ok());

        assert!(validate_identifier("", "id").is_err());
        assert!(validate_identifier("../../admin", "id").is_err());
        assert!(validate_identifier("a/b", "id").is_err());
        assert!(validate_identifier("a\\b", "id").is_err());
        assert!(validate_identifier("a\nb", "id").is_err());
        assert!(validate_identifier("x..y", "id").is_err());
        assert!(validate_identifier(&"z".repeat(MAX_IDENTIFIER_LEN + 1), "id").is_err());

        // URL-significant characters must be rejected: they would steer the
        // REST path's query string, fragment, or percent-escaping even though
        // they are neither '/', '\\', nor control characters.
        assert!(validate_identifier("id?admin=1", "id").is_err());
        assert!(validate_identifier("id#frag", "id").is_err());
        assert!(validate_identifier("id%2e%2e", "id").is_err());
        assert!(validate_identifier("id with space", "id").is_err());
        assert!(validate_identifier("id&x=1", "id").is_err());
    }

    #[test]
    fn receipt_path_is_collision_resistant_for_distinct_ids() {
        // Two ids that sanitize to the same lossy stem must not share a file.
        let a = receipt_path("/tmp/x", "ab");
        let b = receipt_path("/tmp/x", "a/b");
        assert_ne!(a, b);
        // Same id is stable.
        assert_eq!(receipt_path("/tmp/x", "ab"), receipt_path("/tmp/x", "ab"));
        // Filenames stay filesystem-safe (sanitized stem + hex hash + .json).
        let name = receipt_path("/tmp/x", "weird/../id")
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(name.ends_with(".json"));
        assert!(!name.contains('/'));
    }

    #[test]
    fn fetch_error_backoff_grows_then_caps() {
        assert_eq!(fetch_error_backoff(0), Duration::from_millis(250));
        assert_eq!(fetch_error_backoff(1), Duration::from_millis(250));
        assert_eq!(fetch_error_backoff(2), Duration::from_millis(500));
        assert_eq!(fetch_error_backoff(3), Duration::from_millis(1_000));
        assert_eq!(fetch_error_backoff(5), Duration::from_millis(4_000));
        // Caps at 5s no matter how high the streak goes.
        assert_eq!(fetch_error_backoff(6), Duration::from_millis(5_000));
        assert_eq!(fetch_error_backoff(1_000), Duration::from_millis(5_000));
    }

    #[test]
    fn record_receipt_trims_when_capped() {
        let mut receipts = HashSet::new();
        for i in 0..MAX_RECEIPT_CACHE {
            receipts.insert(format!("seed-{i}"));
        }
        assert_eq!(receipts.len(), MAX_RECEIPT_CACHE);
        // Next insert via the capped helper trims the set instead of growing it.
        record_receipt(&mut receipts, "fresh");
        assert!(receipts.len() <= MAX_RECEIPT_CACHE);
        assert!(receipts.contains("fresh"));
    }
}
