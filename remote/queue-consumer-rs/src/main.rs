use std::{
    collections::HashSet,
    env,
    error::Error,
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueTaskMessage {
    message_kind: Option<String>,
    thread_id: String,
    task_id: String,
    provider: Option<String>,
    repo: Option<String>,
    base_branch: Option<String>,
    prompt: Option<String>,
    thread_title: Option<String>,
    shadow: Option<bool>,
    direct_dispatch: Option<bool>,
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

fn server_auth_secret() -> String {
    env::var("REMOTE_DEV_SERVER_SECRET")
        .or_else(|_| env::var("SERVER_AUTH_SECRET"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "dd-k8s-home".to_string())
}

fn env_bool(key: &str, fallback: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(fallback)
}

fn receipt_path(base_dir: &str, task_id: &str) -> PathBuf {
    let safe_task_id = task_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect::<String>();
    PathBuf::from(base_dir).join(format!("{safe_task_id}.json"))
}

fn has_task_receipt(receipts: &mut HashSet<String>, base_dir: &str, task_id: &str) -> bool {
    if receipts.contains(task_id) {
        return true;
    }
    if receipt_path(base_dir, task_id).exists() {
        receipts.insert(task_id.to_string());
        return true;
    }
    false
}

fn write_task_receipt(
    base_dir: &str,
    task: &QueueTaskMessage,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    fs::create_dir_all(base_dir)?;
    fs::write(
        receipt_path(base_dir, &task.task_id),
        serde_json::to_vec_pretty(&serde_json::json!({
            "threadId": &task.thread_id,
            "taskId": &task.task_id,
            "messageKind": &task.message_kind,
            "shadow": task.shadow.unwrap_or(false),
            "directDispatch": task.direct_dispatch.unwrap_or(false),
        }))?,
    )?;
    Ok(())
}

fn is_shadow_task(task: &QueueTaskMessage) -> bool {
    task.shadow.unwrap_or(false)
        || task
            .message_kind
            .as_deref()
            .is_some_and(|kind| kind == "task.shadow")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn nats_event_subject() -> &'static str {
    "dd.remote.events"
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
        eprintln!(
            "queue status event persist failed: thread={} task={} stage={stage} error={error}",
            task.thread_id, task.task_id
        );
    }
    if let Err(error) = publish_queue_status_event(nats, task, seq, stage, &event).await {
        eprintln!(
            "queue status event nats publish failed: thread={} task={} stage={stage} error={error}",
            task.thread_id, task.task_id
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
            "path": "/tasks",
            "payload": {
                "taskId": &task.task_id,
                "threadId": &task.thread_id,
                "repo": repo,
                "baseBranch": base_branch,
                "prompt": prompt,
                "provider": &task.provider,
                "threadTitle": &task.thread_title,
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

async fn dispatch_to_rest_api(
    http: &reqwest::Client,
    rest_api_url: &str,
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
    let base = rest_api_url.trim_end_matches('/');
    let url = format!("{base}/api/agents/threads/{}/tasks", task.thread_id);
    let response = http
        .post(url)
        .header("X-Agent-Auth", secret)
        .json(&serde_json::json!({
            "taskId": &task.task_id,
            "threadId": &task.thread_id,
            "repo": repo,
            "baseBranch": base_branch,
            "prompt": prompt,
            "provider": &task.provider,
            "threadTitle": &task.thread_title,
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

async fn build_jetstream_consumer(
    client: async_nats::Client,
    stream_name: &str,
    subject: &str,
    consumer_name: &str,
    ack_wait: Duration,
    max_ack_pending: i64,
    max_deliver: i64,
) -> Result<async_nats::jetstream::consumer::PullConsumer, Box<dyn Error + Send + Sync>> {
    let jetstream = async_nats::jetstream::new(client);
    let stream = jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: stream_name.to_string(),
            subjects: vec![subject.to_string()],
            retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
            max_age: Duration::from_secs(60 * 60 * 24 * 14),
            max_message_size: 8 * 1024 * 1024,
            ..Default::default()
        })
        .await?;

    let consumer = stream
        .get_or_create_consumer::<async_nats::jetstream::consumer::pull::Config>(
            consumer_name,
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(consumer_name.to_string()),
                filter_subject: subject.to_string(),
                ack_wait,
                max_ack_pending,
                max_deliver,
                ..Default::default()
            },
        )
        .await?;

    Ok(consumer)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let nats_url = env_value(
        "NATS_URL",
        "nats://dd-nats.messaging.svc.cluster.local:4222",
    );
    let subject = env_value("NATS_TASK_SUBJECT", "dd.remote.thread.*.tasks");
    let queue_group = env_value("NATS_QUEUE_GROUP", "dd-remote-thread-preparer");
    let stream_name = env_value("NATS_TASK_STREAM", "DD_REMOTE_TASKS");
    let consumer_name = env_value("NATS_TASK_CONSUMER", &queue_group);
    let ack_wait_seconds = env_u64("NATS_TASK_ACK_WAIT_SECONDS", 120);
    let max_ack_pending = env_i64("NATS_TASK_MAX_ACK_PENDING", 256);
    let max_deliver = env_i64("NATS_TASK_MAX_DELIVER", 5);
    let nak_delay_seconds = env_u64("NATS_TASK_NAK_DELAY_SECONDS", 15);
    let rest_api_url = env_value(
        "REMOTE_REST_API_URL",
        "http://dd-remote-rest-api.default.svc.cluster.local:8082",
    );
    let container_pool_url = env_value(
        "CONTAINER_POOL_BASE_URL",
        "http://dd-container-pool.default.svc.cluster.local:8102",
    );
    let fallback_rest_dispatch = env_bool("QUEUE_CONSUMER_FALLBACK_REST_DISPATCH", true);
    let receipts_dir = env_value(
        "QUEUE_CONSUMER_RECEIPTS_DIR",
        "/tmp/dd-remote-queue-consumer/tasks",
    );
    let secret = server_auth_secret();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;

    println!(
        "dd-remote-queue-consumer starting: nats_url={nats_url} stream={stream_name} subject={subject} consumer={consumer_name} rest_api_url={rest_api_url} container_pool_url={container_pool_url} receipts_dir={receipts_dir}"
    );
    let nats_client = async_nats::connect(nats_url).await?;
    let consumer = build_jetstream_consumer(
        nats_client.clone(),
        &stream_name,
        &subject,
        &consumer_name,
        Duration::from_secs(ack_wait_seconds),
        max_ack_pending,
        max_deliver,
    )
    .await?;
    let mut messages = consumer.messages().await?;
    let mut receipts = HashSet::new();

    while let Some(message) = messages.next().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                eprintln!("jetstream message fetch failed: {error}");
                continue;
            }
        };
        let task = match serde_json::from_slice::<QueueTaskMessage>(&message.payload) {
            Ok(task) => task,
            Err(error) => {
                eprintln!("invalid queue task message: {error}");
                if let Err(ack_error) = message.ack().await {
                    eprintln!("invalid queue task ack failed: {ack_error}");
                }
                continue;
            }
        };
        if has_task_receipt(&mut receipts, &receipts_dir, &task.task_id) {
            println!(
                "queue task skipped duplicate: thread={} task={}",
                task.thread_id, task.task_id
            );
            if let Err(error) = message.ack().await {
                eprintln!("duplicate queue task ack failed: {error}");
            }
            continue;
        }
        let shadow = is_shadow_task(&task);
        println!(
            "queue task received: thread={} task={} kind={} shadow={} direct_dispatch={}",
            task.thread_id,
            task.task_id,
            task.message_kind.as_deref().unwrap_or("unknown"),
            shadow,
            task.direct_dispatch.unwrap_or(false),
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
        let result = if shadow || direct_dispatch {
            let (stage, status, message) = if shadow {
                (
                    "shadow-prepare",
                    "preparing shadow worker",
                    "Shadow handoff is waking the UUID-bound thread worker.",
                )
            } else {
                (
                    "direct-dispatch-prepare",
                    "preparing direct-dispatch worker",
                    "Direct REST dispatch is executing the task; queue consumer is warming the UUID-bound worker only.",
                )
            };
            emit_queue_status_event(
                &http,
                &nats_client,
                &rest_api_url,
                &secret,
                &task,
                -930,
                stage,
                status,
                message,
                json!({ "directDispatch": direct_dispatch }),
            )
            .await;
            prepare_thread(&http, &rest_api_url, &secret, &task.thread_id).await
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
                    eprintln!(
                        "queue task container pool dispatch failed: thread={} task={} error={pool_error}",
                        task.thread_id, task.task_id
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
                        "Container-pool dispatch failed before accepting the queued task.",
                        json!({ "poolSlug": &pool, "affinityKey": &task.thread_id, "error": pool_error.to_string() }),
                    )
                    .await;
                    if task.direct_dispatch.unwrap_or(false) {
                        emit_queue_status_event(
                            &http,
                            &nats_client,
                            &rest_api_url,
                            &secret,
                            &task,
                            -910,
                            "rest-fallback-skipped",
                            "REST fallback skipped",
                            "Queue consumer skipped duplicate REST fallback because the REST API is already handling the synchronous worker dispatch.",
                            json!({}),
                        )
                        .await;
                        Ok(())
                    } else if fallback_rest_dispatch {
                        eprintln!(
                            "queue task falling back to rest dispatch: thread={} task={}",
                            task.thread_id, task.task_id
                        );
                        let fallback =
                            dispatch_to_rest_api(&http, &rest_api_url, &secret, &task).await;
                        if fallback.is_ok() {
                            emit_queue_status_event(
                                &http,
                                &nats_client,
                                &rest_api_url,
                                &secret,
                                &task,
                                -910,
                                "rest-fallback-accepted",
                                "REST fallback accepted",
                                "Direct REST fallback accepted the task and is waking the UUID-bound worker.",
                                json!({}),
                            )
                            .await;
                        }
                        fallback
                    } else {
                        Err(pool_error)
                    }
                }
            }
        };
        if let Err(error) = result {
            eprintln!(
                "queue task handoff failed: thread={} task={} error={error}",
                task.thread_id, task.task_id
            );
            emit_queue_status_event(
                &http,
                &nats_client,
                &rest_api_url,
                &secret,
                &task,
                -910,
                "queue-handoff-failed",
                "queue handoff failed",
                "Queue consumer could not hand the task to container-pool or fallback dispatch.",
                json!({ "error": error.to_string() }),
            )
            .await;
            if let Err(nak_error) = message
                .ack_with(async_nats::jetstream::AckKind::Nak(Some(
                    Duration::from_secs(nak_delay_seconds),
                )))
                .await
            {
                eprintln!("queue task negative ack failed: {nak_error}");
            }
            continue;
        }
        receipts.insert(task.task_id.clone());
        if let Err(error) = write_task_receipt(&receipts_dir, &task) {
            eprintln!(
                "queue task receipt write failed: thread={} task={} error={error}",
                task.thread_id, task.task_id
            );
        }
        if let Err(error) = message.ack().await {
            eprintln!(
                "queue task ack failed: thread={} task={} error={error}",
                task.thread_id, task.task_id
            );
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
