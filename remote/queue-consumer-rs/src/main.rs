use std::{collections::HashSet, env, error::Error, fs, path::PathBuf, time::Duration};

use futures_util::StreamExt;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueTaskMessage {
    thread_id: String,
    task_id: String,
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
            "threadId": task.thread_id,
            "taskId": task.task_id,
            "shadow": task.shadow.unwrap_or(false),
            "directDispatch": task.direct_dispatch.unwrap_or(false),
        }))?,
    )?;
    Ok(())
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
    let receipts_dir = env_value(
        "QUEUE_CONSUMER_RECEIPTS_DIR",
        "/tmp/dd-remote-queue-consumer/tasks",
    );
    let secret = server_auth_secret();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;

    println!(
        "dd-remote-queue-consumer starting: nats_url={nats_url} stream={stream_name} subject={subject} consumer={consumer_name} rest_api_url={rest_api_url} receipts_dir={receipts_dir}"
    );
    let client = async_nats::connect(nats_url).await?;
    let consumer = build_jetstream_consumer(
        client,
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
        println!(
            "queue task received: thread={} task={} shadow={} direct_dispatch={}",
            task.thread_id,
            task.task_id,
            task.shadow.unwrap_or(false),
            task.direct_dispatch.unwrap_or(false),
        );
        if let Err(error) = prepare_thread(&http, &rest_api_url, &secret, &task.thread_id).await {
            eprintln!(
                "queue task prepare failed: thread={} task={} error={error}",
                task.thread_id, task.task_id
            );
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
        }
    }

    Ok(())
}
