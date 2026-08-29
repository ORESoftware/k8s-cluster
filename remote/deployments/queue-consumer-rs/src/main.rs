use std::{
    collections::hash_map::DefaultHasher,
    collections::HashSet,
    env,
    error::Error,
    fmt::Write as _,
    fs,
    hash::{Hash, Hasher},
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use dd_nats_subject_defs::{
    DD_REMOTE_CRITICAL_EVENTS_STREAM_NAME, DD_REMOTE_TASKS_DLQ_STREAM_NAME,
    DD_REMOTE_TASKS_STREAM_NAME, RUNTIME_CRITICAL_EVENTS_QUEUE_GROUP,
    RUNTIME_CRITICAL_EVENTS_SUBJECT, RUNTIME_EVENTS_SUBJECT, THREAD_PREPARER_QUEUE_GROUP,
    THREAD_TASKS_DEAD_LETTER_SUBJECT, THREAD_TASKS_WILDCARD,
};
use dd_shared_interfaces::AgentTaskQueueMessage;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
static READY: AtomicBool = AtomicBool::new(false);
static MESSAGES_RECEIVED: AtomicU64 = AtomicU64::new(0);
static FETCH_ERRORS: AtomicU64 = AtomicU64::new(0);
static INVALID_MESSAGES: AtomicU64 = AtomicU64::new(0);
static DUPLICATE_MESSAGES: AtomicU64 = AtomicU64::new(0);
static ACK_PROGRESS_FAILURES: AtomicU64 = AtomicU64::new(0);
static HANDOFF_SUCCESSES: AtomicU64 = AtomicU64::new(0);
static HANDOFF_FAILURES: AtomicU64 = AtomicU64::new(0);
static DEAD_LETTERED: AtomicU64 = AtomicU64::new(0);
static DLQ_DUPLICATES: AtomicU64 = AtomicU64::new(0);
static DLQ_PUBLISH_FAILURES: AtomicU64 = AtomicU64::new(0);
static DLQ_PUBLISH_EXHAUSTED: AtomicU64 = AtomicU64::new(0);
static DLQ_SOURCE_MESSAGES_PRESERVED: AtomicU64 = AtomicU64::new(0);

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

/// Interval between `AckKind::Progress` heartbeats sent while a worker handoff
/// is in flight. A third of the ack-wait window (floored at 5s) so several
/// heartbeats land inside each ack deadline even if one is delayed by a slow
/// broker round-trip.
fn ack_progress_interval(ack_wait: Duration) -> Duration {
    (ack_wait / 3).max(Duration::from_secs(5))
}

/// Whether JetStream has delivered a message its last permitted time, so a
/// further failure must terminate + dead-letter it rather than Nak for an
/// (impossible) redelivery. `max_deliver <= 0` means unlimited redelivery, so
/// a message is never final in that mode.
fn is_final_delivery(delivered: i64, max_deliver: i64) -> bool {
    max_deliver > 0 && delivered >= max_deliver
}

/// Await `handoff` while periodically extending the JetStream ack deadline for
/// `message` with `AckKind::Progress`.
///
/// A worker handoff can chain two HTTP calls (prepare + dispatch), each bounded
/// only by `QUEUE_CONSUMER_HTTP_TIMEOUT_SECONDS` (default 420s), which can far
/// exceed `ack_wait` (default 120s). Without a heartbeat, JetStream treats the
/// still-running delivery as stalled once `ack_wait` elapses and redelivers it
/// to another replica — dispatching the same task twice, because the
/// duplicate-suppression receipt is only written *after* the handoff succeeds.
/// Sending `Progress` on an interval keeps the deadline alive until the handoff
/// resolves, so redelivery only happens on a genuine stall or crash.
async fn run_handoff_with_ack_progress<F, T>(
    message: &async_nats::jetstream::Message,
    interval: Duration,
    handoff: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    let mut handoff = std::pin::pin!(handoff);
    let mut ticker = tokio::time::interval(interval);
    // If a heartbeat is delayed, resume the cadence from "now" rather than
    // firing a burst of catch-up ticks.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await; // The first tick completes immediately; skip it.
    loop {
        tokio::select! {
            biased;
            output = &mut handoff => return output,
            _ = ticker.tick() => {
                if let Err(error) = message
                    .ack_with(async_nats::jetstream::AckKind::Progress)
                    .await
                {
                    ACK_PROGRESS_FAILURES.fetch_add(1, Ordering::Relaxed);
                    // A single missed heartbeat is not fatal: the handoff keeps
                    // running and the next tick retries. Only if enough are
                    // missed to blow the ack deadline does JetStream redeliver,
                    // which the receipt/idempotency layer still guards against.
                    log_warn(
                        "queue-task-ack-progress-failed",
                        "Queue consumer could not send an ack progress heartbeat.",
                        json!({ "error": error.to_string() }),
                    );
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct DlqPublishPolicy {
    attempts: u32,
    retry_base: Duration,
    publish_timeout: Duration,
}

#[derive(Debug)]
struct DlqPublishAck {
    stream: String,
    sequence: u64,
    duplicate: bool,
}

#[derive(Debug)]
struct DeadLetterSource {
    stream: String,
    consumer: String,
    original_subject: String,
    stream_sequence: u64,
    consumer_sequence: u64,
    delivered: i64,
}

fn configured_dlq_publish_policy() -> DlqPublishPolicy {
    DlqPublishPolicy {
        attempts: env_u64("NATS_TASK_DLQ_PUBLISH_ATTEMPTS", 5).clamp(1, 20) as u32,
        retry_base: Duration::from_millis(
            env_u64("NATS_TASK_DLQ_RETRY_BASE_MS", 250).clamp(10, 5_000),
        ),
        publish_timeout: Duration::from_millis(
            env_u64("NATS_TASK_DLQ_PUBLISH_TIMEOUT_MS", 5_000).clamp(100, 30_000),
        ),
    }
}

fn dlq_publish_backoff(attempt: u32, base: Duration) -> Duration {
    let exponent = attempt.saturating_sub(1).min(10);
    let multiplier = 1u128 << exponent;
    let millis = base.as_millis().saturating_mul(multiplier).min(30_000) as u64;
    Duration::from_millis(millis)
}

fn stable_payload_hash(payload: &[u8]) -> u64 {
    // FNV-1a is stable across processes and Rust releases, unlike DefaultHasher.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in payload {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn dead_letter_source(
    message: &async_nats::jetstream::Message,
    fallback_stream: &str,
) -> DeadLetterSource {
    let info = message.info().ok();
    DeadLetterSource {
        stream: info
            .as_ref()
            .map(|info| info.stream.to_string())
            .unwrap_or_else(|| fallback_stream.to_string()),
        consumer: info
            .as_ref()
            .map(|info| info.consumer.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        original_subject: message.subject.to_string(),
        stream_sequence: info.as_ref().map(|info| info.stream_sequence).unwrap_or(0),
        consumer_sequence: info
            .as_ref()
            .map(|info| info.consumer_sequence)
            .unwrap_or(0),
        delivered: info.as_ref().map(|info| info.delivered).unwrap_or(0),
    }
}

fn dead_letter_message_id(source: &DeadLetterSource, payload: &[u8]) -> String {
    let identity = if source.stream_sequence > 0 {
        source.stream_sequence
    } else {
        stable_payload_hash(payload)
    };
    format!("{}:{identity}:dd-dead-letter-v2", source.stream)
}

fn dead_letter_payload(
    message: &async_nats::jetstream::Message,
    source: &DeadLetterSource,
    task: Option<&QueueTaskMessage>,
    reason: &str,
    error_text: &str,
    max_deliver: i64,
) -> Value {
    let original_payload = serde_json::from_slice::<Value>(&message.payload).unwrap_or_else(|_| {
        json!({
            "encoding": "utf8-lossy",
            "data": String::from_utf8_lossy(&message.payload).to_string(),
            "payloadBytes": message.payload.len(),
        })
    });
    let task_metadata = task
        .map(|task| {
            json!({
                "threadId": &task.thread_id,
                "taskId": &task.task_id,
                "messageKind": &task.message_kind,
                "shadow": task.shadow.unwrap_or(false),
                "directDispatch": task.direct_dispatch.unwrap_or(false),
            })
        })
        .unwrap_or(Value::Null);
    json!({
        "type": "dead-letter",
        "schema": "dd.dead_letter.v2",
        "sourceService": SERVICE_NAME,
        "reason": reason,
        "source": {
            "stream": &source.stream,
            "consumer": &source.consumer,
            "subject": &source.original_subject,
            "streamSequence": source.stream_sequence,
            "consumerSequence": source.consumer_sequence,
            "deliveries": source.delivered,
            "maxDeliver": max_deliver,
        },
        "task": task_metadata,
        "originalPayload": original_payload,
        "error": error_text.chars().take(4_096).collect::<String>(),
        "emittedAtMs": now_ms(),
    })
}

async fn publish_dead_letter_with_retry(
    message: &async_nats::jetstream::Message,
    nats: &async_nats::Client,
    dlq_subject: &str,
    source: &DeadLetterSource,
    payload: Vec<u8>,
    policy: DlqPublishPolicy,
) -> Result<DlqPublishAck, String> {
    let jetstream = async_nats::jetstream::new(nats.clone());
    let message_id = dead_letter_message_id(source, &message.payload);
    let mut last_error = "dead-letter publish was not attempted".to_string();

    for attempt in 1..=policy.attempts {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert("Nats-Msg-Id", message_id.as_str());
        headers.insert("X-DD-DLQ-Schema", "dd.dead_letter.v2");
        headers.insert("X-DD-DLQ-Source-Stream", source.stream.as_str());
        headers.insert(
            "X-DD-DLQ-Source-Sequence",
            source.stream_sequence.to_string(),
        );
        let result = tokio::time::timeout(policy.publish_timeout, async {
            let ack = jetstream
                .publish_with_headers(dlq_subject.to_string(), headers, payload.clone().into())
                .await
                .map_err(|error| error.to_string())?;
            ack.await.map_err(|error| error.to_string())
        })
        .await;

        match result {
            Ok(Ok(ack)) => {
                return Ok(DlqPublishAck {
                    stream: ack.stream,
                    sequence: ack.sequence,
                    duplicate: ack.duplicate,
                });
            }
            Ok(Err(error)) => last_error = error,
            Err(_) => {
                last_error = format!(
                    "dead-letter publish attempt timed out after {}ms",
                    policy.publish_timeout.as_millis()
                )
            }
        }

        DLQ_PUBLISH_FAILURES.fetch_add(1, Ordering::Relaxed);
        log_warn(
            "dead-letter-publish-attempt-failed",
            "Queue consumer could not durably publish a task to the dead-letter stream.",
            json!({
                "dlqSubject": dlq_subject,
                "sourceStream": &source.stream,
                "sourceSequence": source.stream_sequence,
                "attempt": attempt,
                "maxAttempts": policy.attempts,
                "error": &last_error,
            }),
        );

        if attempt < policy.attempts {
            // Keep the source delivery alive while retrying the durable side of
            // the transfer, then apply bounded exponential backoff.
            if let Err(error) = message
                .ack_with(async_nats::jetstream::AckKind::Progress)
                .await
            {
                ACK_PROGRESS_FAILURES.fetch_add(1, Ordering::Relaxed);
                log_warn(
                    "dead-letter-ack-progress-failed",
                    "Queue consumer could not extend the source ack deadline during DLQ retry.",
                    json!({ "error": error.to_string(), "attempt": attempt }),
                );
            }
            tokio::time::sleep(dlq_publish_backoff(attempt, policy.retry_base)).await;
        }
    }

    Err(last_error)
}

async fn preserve_source_after_dlq_failure(
    message: &async_nats::jetstream::Message,
    delivered: i64,
    max_deliver: i64,
    nak_delay: Duration,
) {
    let ack_kind = if is_final_delivery(delivered, max_deliver) {
        // At MaxDeliver, another Nak cannot produce a normal redelivery. Keep
        // the message un-terminated and refresh its ack deadline so the source
        // stream remains the recovery record until an operator repairs the DLQ.
        async_nats::jetstream::AckKind::Progress
    } else {
        async_nats::jetstream::AckKind::Nak(Some(nak_delay))
    };
    if let Err(error) = message.ack_with(ack_kind).await {
        log_warn(
            "dead-letter-source-preserve-ack-failed",
            "Queue consumer could not signal source preservation after DLQ failure.",
            json!({
                "delivered": delivered,
                "maxDeliver": max_deliver,
                "error": error.to_string(),
            }),
        );
    }
}

#[allow(clippy::too_many_arguments)]
async fn move_to_dead_letter(
    message: &async_nats::jetstream::Message,
    nats: &async_nats::Client,
    critical_subject: &str,
    dlq_subject: &str,
    stream_name: &str,
    task: Option<&QueueTaskMessage>,
    max_deliver: i64,
    nak_delay: Duration,
    reason: &str,
    error_text: &str,
    policy: DlqPublishPolicy,
) -> bool {
    let source = dead_letter_source(message, stream_name);
    let payload = dead_letter_payload(message, &source, task, reason, error_text, max_deliver);
    let encoded = match serde_json::to_vec(&payload) {
        Ok(encoded) => encoded,
        Err(error) => {
            DLQ_PUBLISH_EXHAUSTED.fetch_add(1, Ordering::Relaxed);
            DLQ_SOURCE_MESSAGES_PRESERVED.fetch_add(1, Ordering::Relaxed);
            emit_runtime_critical_event(
                nats,
                critical_subject,
                "dead-letter-serialize-failed-source-preserved",
                "Queue consumer could not serialize a dead-letter envelope; the source message was preserved.",
                json!({
                    "sourceStream": &source.stream,
                    "sourceSequence": source.stream_sequence,
                    "reason": reason,
                    "error": error.to_string(),
                }),
            )
            .await;
            preserve_source_after_dlq_failure(message, source.delivered, max_deliver, nak_delay)
                .await;
            return false;
        }
    };

    match publish_dead_letter_with_retry(message, nats, dlq_subject, &source, encoded, policy).await
    {
        Ok(ack) => {
            DEAD_LETTERED.fetch_add(1, Ordering::Relaxed);
            if ack.duplicate {
                DLQ_DUPLICATES.fetch_add(1, Ordering::Relaxed);
            }
            emit_runtime_critical_event(
                nats,
                critical_subject,
                "queue-task-dead-lettered",
                "Queue consumer durably moved a source message to the dead-letter stream.",
                json!({
                    "threadId": task.map(|task| task.thread_id.as_str()),
                    "taskId": task.map(|task| task.task_id.as_str()),
                    "reason": reason,
                    "dlqSubject": dlq_subject,
                    "dlqStream": &ack.stream,
                    "dlqSequence": ack.sequence,
                    "duplicate": ack.duplicate,
                    "sourceStream": &source.stream,
                    "sourceSequence": source.stream_sequence,
                    "deliveries": source.delivered,
                    "maxDeliver": max_deliver,
                    "error": error_text,
                }),
            )
            .await;

            // The ordering is the durability invariant: only Term the source
            // after the DLQ server ACK has completed. If Term fails, a later
            // redelivery republishes with the same Nats-Msg-Id and receives a
            // duplicate ACK instead of creating another DLQ record.
            if let Err(error) = message.ack_with(async_nats::jetstream::AckKind::Term).await {
                emit_runtime_critical_event(
                    nats,
                    critical_subject,
                    "queue-task-term-failed-after-dlq-ack",
                    "Queue consumer durably published the DLQ record but could not terminate the source message.",
                    json!({
                        "sourceStream": &source.stream,
                        "sourceSequence": source.stream_sequence,
                        "dlqStream": &ack.stream,
                        "dlqSequence": ack.sequence,
                        "error": error.to_string(),
                    }),
                )
                .await;
            }
            true
        }
        Err(error) => {
            DLQ_PUBLISH_EXHAUSTED.fetch_add(1, Ordering::Relaxed);
            DLQ_SOURCE_MESSAGES_PRESERVED.fetch_add(1, Ordering::Relaxed);
            emit_runtime_critical_event(
                nats,
                critical_subject,
                "dead-letter-publish-exhausted-source-preserved",
                "All durable DLQ publish attempts failed; the source message was not terminated.",
                json!({
                    "threadId": task.map(|task| task.thread_id.as_str()),
                    "taskId": task.map(|task| task.task_id.as_str()),
                    "reason": reason,
                    "dlqSubject": dlq_subject,
                    "sourceStream": &source.stream,
                    "sourceSequence": source.stream_sequence,
                    "deliveries": source.delivered,
                    "maxDeliver": max_deliver,
                    "attempts": policy.attempts,
                    "error": &error,
                }),
            )
            .await;
            preserve_source_after_dlq_failure(message, source.delivered, max_deliver, nak_delay)
                .await;
            false
        }
    }
}

/// After a handoff failure, either negatively-acknowledge the message for
/// another delivery attempt or, at MaxDeliver, durably transfer it to the DLQ.
/// The source message is terminated only after the DLQ publish ACK succeeds.
#[allow(clippy::too_many_arguments)]
async fn nak_or_dead_letter(
    message: &async_nats::jetstream::Message,
    nats: &async_nats::Client,
    critical_subject: &str,
    dlq_subject: &str,
    stream_name: &str,
    task: &QueueTaskMessage,
    max_deliver: i64,
    nak_delay: Duration,
    error_text: &str,
    policy: DlqPublishPolicy,
) {
    let delivered = message.info().map(|info| info.delivered).unwrap_or(0);
    if !is_final_delivery(delivered, max_deliver) {
        if let Err(nak_error) = message
            .ack_with(async_nats::jetstream::AckKind::Nak(Some(nak_delay)))
            .await
        {
            emit_runtime_critical_event(
                nats,
                critical_subject,
                "queue-task-negative-ack-failed",
                "Queue consumer could not NAK a failed task message.",
                json!({
                    "threadId": &task.thread_id,
                    "taskId": &task.task_id,
                    "nakDelaySeconds": nak_delay.as_secs(),
                    "delivered": delivered,
                    "error": nak_error.to_string(),
                }),
            )
            .await;
        }
        return;
    }

    move_to_dead_letter(
        message,
        nats,
        critical_subject,
        dlq_subject,
        stream_name,
        Some(task),
        max_deliver,
        nak_delay,
        "handoff-failed-after-max-deliver",
        error_text,
        policy,
    )
    .await;
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

fn render_metrics() -> String {
    let mut output = String::new();
    let metrics = [
        (
            "dd_queue_consumer_messages_received_total",
            MESSAGES_RECEIVED.load(Ordering::Relaxed),
        ),
        (
            "dd_queue_consumer_fetch_errors_total",
            FETCH_ERRORS.load(Ordering::Relaxed),
        ),
        (
            "dd_queue_consumer_invalid_messages_total",
            INVALID_MESSAGES.load(Ordering::Relaxed),
        ),
        (
            "dd_queue_consumer_duplicate_messages_total",
            DUPLICATE_MESSAGES.load(Ordering::Relaxed),
        ),
        (
            "dd_queue_consumer_ack_progress_failures_total",
            ACK_PROGRESS_FAILURES.load(Ordering::Relaxed),
        ),
        (
            "dd_queue_consumer_handoff_successes_total",
            HANDOFF_SUCCESSES.load(Ordering::Relaxed),
        ),
        (
            "dd_queue_consumer_handoff_failures_total",
            HANDOFF_FAILURES.load(Ordering::Relaxed),
        ),
        (
            "dd_queue_consumer_dead_lettered_total",
            DEAD_LETTERED.load(Ordering::Relaxed),
        ),
        (
            "dd_queue_consumer_dlq_duplicates_total",
            DLQ_DUPLICATES.load(Ordering::Relaxed),
        ),
        (
            "dd_queue_consumer_dlq_publish_failures_total",
            DLQ_PUBLISH_FAILURES.load(Ordering::Relaxed),
        ),
        (
            "dd_queue_consumer_dlq_publish_exhausted_total",
            DLQ_PUBLISH_EXHAUSTED.load(Ordering::Relaxed),
        ),
        (
            "dd_queue_consumer_dlq_source_messages_preserved_total",
            DLQ_SOURCE_MESSAGES_PRESERVED.load(Ordering::Relaxed),
        ),
    ];
    for (name, value) in metrics {
        let _ = writeln!(output, "# TYPE {name} counter\n{name} {value}");
    }
    let ready = u8::from(READY.load(Ordering::Relaxed));
    let _ = writeln!(
        output,
        "# TYPE dd_queue_consumer_ready gauge\ndd_queue_consumer_ready {ready}"
    );
    output
}

async fn serve_metrics(addr: String) -> Result<(), Box<dyn Error + Send + Sync>> {
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    log_info(
        "metrics-server-started",
        "Queue consumer health and Prometheus endpoint started.",
        json!({ "address": addr }),
    );
    loop {
        let (mut socket, _) = listener.accept().await?;
        tokio::spawn(async move {
            let mut request = [0u8; 4096];
            let bytes_read = match socket.read(&mut request).await {
                Ok(bytes_read) => bytes_read,
                Err(_) => return,
            };
            let request = String::from_utf8_lossy(&request[..bytes_read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            let (status, content_type, body) = match path {
                "/metrics" => ("200 OK", "text/plain; version=0.0.4", render_metrics()),
                "/healthz" => ("200 OK", "text/plain", "ok\n".to_string()),
                "/readyz" if READY.load(Ordering::Relaxed) => {
                    ("200 OK", "text/plain", "ready\n".to_string())
                }
                "/readyz" => (
                    "503 Service Unavailable",
                    "text/plain",
                    "not ready\n".to_string(),
                ),
                _ => ("404 Not Found", "text/plain", "not found\n".to_string()),
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        });
    }
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

async fn ensure_dead_letter_stream(
    client: async_nats::Client,
    stream_name: &str,
    subject: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let jetstream = async_nats::jetstream::new(client);
    jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: stream_name.to_string(),
            subjects: vec![subject.to_string()],
            retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
            storage: async_nats::jetstream::stream::StorageType::File,
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_message_size: 8 * 1024 * 1024,
            ..Default::default()
        })
        .await?;
    Ok(())
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
    let metrics_addr = env_value("QUEUE_CONSUMER_METRICS_ADDR", "0.0.0.0:8120");
    let metrics_addr_for_task = metrics_addr.clone();
    tokio::spawn(async move {
        if let Err(error) = serve_metrics(metrics_addr_for_task).await {
            log_error(
                "metrics-server-failed",
                "Queue consumer health and Prometheus endpoint stopped.",
                json!({ "error": error.to_string() }),
            );
        }
    });

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
    let dlq_subject = env_value("NATS_TASK_DLQ_SUBJECT", THREAD_TASKS_DEAD_LETTER_SUBJECT);
    let dlq_stream_name = env_value("NATS_TASK_DLQ_STREAM", DD_REMOTE_TASKS_DLQ_STREAM_NAME);
    let dlq_publish_policy = configured_dlq_publish_policy();
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
            "natsEndpointConfigured": !nats_url.is_empty(),
            "stream": &stream_name,
            "subject": &subject,
            "eventSubject": &event_subject,
            "criticalSubject": &critical_subject,
            "criticalStream": &critical_stream_name,
            "criticalConsumer": &critical_consumer_name,
            "criticalLoggerEnabled": critical_logger_enabled,
            "consumer": &consumer_name,
            "dlqSubject": &dlq_subject,
            "dlqStream": &dlq_stream_name,
            "dlqPublishAttempts": dlq_publish_policy.attempts,
            "dlqRetryBaseMs": dlq_publish_policy.retry_base.as_millis(),
            "dlqPublishTimeoutMs": dlq_publish_policy.publish_timeout.as_millis(),
            "restApiUrl": &rest_api_url,
            "containerPoolUrl": &container_pool_url,
            "httpTimeoutSeconds": http_timeout_seconds,
            "fallbackRestDispatch": fallback_rest_dispatch,
            "receiptsDir": &receipts_dir,
            "metricsAddr": &metrics_addr,
        }),
    );
    let nats_client = connect_nats(&nats_url).await?;
    ensure_dead_letter_stream(nats_client.clone(), &dlq_stream_name, &dlq_subject).await?;
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
    let ack_progress_every = ack_progress_interval(Duration::from_secs(ack_wait_seconds));
    READY.store(true, Ordering::Relaxed);

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
                FETCH_ERRORS.fetch_add(1, Ordering::Relaxed);
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
        MESSAGES_RECEIVED.fetch_add(1, Ordering::Relaxed);
        let task = match serde_json::from_slice::<QueueTaskMessage>(&message.payload) {
            Ok(task) => task,
            Err(error) => {
                INVALID_MESSAGES.fetch_add(1, Ordering::Relaxed);
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
                move_to_dead_letter(
                    &message,
                    &nats_client,
                    &critical_subject,
                    &dlq_subject,
                    &stream_name,
                    None,
                    max_deliver,
                    Duration::from_secs(nak_delay_seconds),
                    "invalid-json-payload",
                    &error.to_string(),
                    dlq_publish_policy,
                )
                .await;
                continue;
            }
        };
        if let Err(validation_error) = validate_task_identifiers(&task) {
            INVALID_MESSAGES.fetch_add(1, Ordering::Relaxed);
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
            // Unsafe identifiers are poison, but they are still evidence. Move
            // them to the DLQ instead of silently acknowledging and discarding.
            move_to_dead_letter(
                &message,
                &nats_client,
                &critical_subject,
                &dlq_subject,
                &stream_name,
                Some(&task),
                max_deliver,
                Duration::from_secs(nak_delay_seconds),
                "unsafe-task-identifiers",
                &validation_error,
                dlq_publish_policy,
            )
            .await;
            continue;
        }
        if has_task_receipt(&mut receipts, &receipts_dir, &task.task_id) {
            DUPLICATE_MESSAGES.fetch_add(1, Ordering::Relaxed);
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
        // Run the handoff under an ack-progress heartbeat so a legitimately long
        // prepare+dispatch is not mistaken for a stalled delivery and redelivered
        // (which would dispatch the task twice). See run_handoff_with_ack_progress.
        let handoff = async {
            if direct_dispatch {
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
                            match dispatch_to_deterministic_worker(
                                &http,
                                &rest_api_url,
                                &secret,
                                &task,
                            )
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
            }
        };
        let result = run_handoff_with_ack_progress(&message, ack_progress_every, handoff).await;
        if let Err(error) = result {
            HANDOFF_FAILURES.fetch_add(1, Ordering::Relaxed);
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
            nak_or_dead_letter(
                &message,
                &nats_client,
                &critical_subject,
                &dlq_subject,
                &stream_name,
                &task,
                max_deliver,
                Duration::from_secs(nak_delay_seconds),
                &error_text,
                dlq_publish_policy,
            )
            .await;
            continue;
        }
        HANDOFF_SUCCESSES.fetch_add(1, Ordering::Relaxed);
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

    READY.store(false, Ordering::Relaxed);
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
    fn ack_progress_interval_is_a_third_of_ack_wait_with_floor() {
        // A comfortable margin: three heartbeats per ack-wait window.
        assert_eq!(
            ack_progress_interval(Duration::from_secs(120)),
            Duration::from_secs(40)
        );
        assert_eq!(
            ack_progress_interval(Duration::from_secs(30)),
            Duration::from_secs(10)
        );
        // Never heartbeats faster than the 5s floor even for tiny ack windows,
        // so a misconfigured short ack_wait can't spin the broker.
        assert_eq!(
            ack_progress_interval(Duration::from_secs(9)),
            Duration::from_secs(5)
        );
        assert_eq!(
            ack_progress_interval(Duration::from_secs(1)),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn is_final_delivery_matches_max_deliver() {
        // Not yet exhausted → Nak for another attempt.
        assert!(!is_final_delivery(1, 5));
        assert!(!is_final_delivery(4, 5));
        // Reached or passed the limit → Term + dead-letter.
        assert!(is_final_delivery(5, 5));
        assert!(is_final_delivery(6, 5));
        // max_deliver <= 0 means unlimited redelivery, so never final.
        assert!(!is_final_delivery(1_000, 0));
        assert!(!is_final_delivery(1_000, -1));
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

    #[test]
    fn prometheus_exposition_includes_queue_and_readiness_metrics() {
        let metrics = render_metrics();
        assert!(metrics.contains("# TYPE dd_queue_consumer_messages_received_total counter"));
        assert!(metrics.contains("# TYPE dd_queue_consumer_dead_lettered_total counter"));
        assert!(metrics.contains("# TYPE dd_queue_consumer_ready gauge"));
    }

    // ---- test helpers ---------------------------------------------------

    /// Deserialize a task envelope from JSON, exercising the same serde path the
    /// consumer uses on the wire. Panics on invalid input so routing/validation
    /// tests can read cleanly.
    fn parse_task(value: Value) -> QueueTaskMessage {
        serde_json::from_value::<QueueTaskMessage>(value).expect("task should parse")
    }

    /// A per-test scratch directory under the crate's build tree. Uses the
    /// compile-time manifest dir (no runtime getenv) plus a process-global
    /// counter, so it is unique across parallel tests and never races the
    /// env-mutating tests below.
    fn unique_receipts_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("qc-receipt-tests")
            .join(format!("{tag}-{}-{n}", std::process::id()))
    }

    // Serializes every test that reads or writes process environment. The env
    // parsers below use fixed or shared keys and `set_var`/`remove_var` mutate
    // global state, so concurrent access from other test threads must be
    // excluded. Recovers a poisoned lock so one failing env test can't cascade.
    static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_GUARD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // ---- message parsing / validation ----------------------------------

    #[test]
    fn task_message_requires_thread_and_task_ids() {
        // Minimal valid envelope: only the two required ids; optionals stay None.
        let task = parse_task(json!({"threadId": "th", "taskId": "tk"}));
        assert_eq!(task.thread_id, "th");
        assert_eq!(task.task_id, "tk");
        assert!(task.message_kind.is_none());
        assert!(task.shadow.is_none());

        // Missing either required id is a hard parse error (not a silent default).
        assert!(serde_json::from_value::<QueueTaskMessage>(json!({"threadId": "th"})).is_err());
        assert!(serde_json::from_value::<QueueTaskMessage>(json!({"taskId": "tk"})).is_err());

        // Empty, truncated, non-JSON, and non-object bodies are all rejected.
        assert!(serde_json::from_slice::<QueueTaskMessage>(b"").is_err());
        assert!(serde_json::from_slice::<QueueTaskMessage>(b"not json").is_err());
        assert!(serde_json::from_slice::<QueueTaskMessage>(b"{").is_err());
        assert!(serde_json::from_value::<QueueTaskMessage>(json!([])).is_err());
        assert!(serde_json::from_value::<QueueTaskMessage>(json!("string")).is_err());
        assert!(serde_json::from_value::<QueueTaskMessage>(json!(123)).is_err());
        assert!(serde_json::from_value::<QueueTaskMessage>(json!(null)).is_err());
    }

    #[test]
    fn task_message_maps_camelcase_and_ignores_unknown_fields() {
        let task = parse_task(json!({
            "threadId": "th",
            "taskId": "tk",
            "messageKind": "task.dispatch",
            "directDispatch": true,
            "dispatchMode": "container-pool",
            "containerPoolDispatch": false,
            "baseBranch": "main",
            "threadTitle": "My Thread",
            "contextMode": "selected",
            "contextIds": ["a", "b"],
            "createdAtMs": 1_700_000_000_000_i64,
            // A field this consumer does not model must be ignored, not fatal:
            // producers may emit newer envelope keys during a rolling upgrade.
            "somethingBrandNew": {"nested": 1}
        }));
        assert_eq!(task.message_kind.as_deref(), Some("task.dispatch"));
        assert_eq!(task.direct_dispatch, Some(true));
        assert_eq!(task.dispatch_mode.as_deref(), Some("container-pool"));
        assert_eq!(task.container_pool_dispatch, Some(false));
        assert_eq!(task.base_branch.as_deref(), Some("main"));
        assert_eq!(task.thread_title.as_deref(), Some("My Thread"));
        assert_eq!(task.context_mode.as_deref(), Some("selected"));
        assert_eq!(
            task.context_ids,
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(task.created_at_ms, Some(1_700_000_000_000));
    }

    #[test]
    fn task_message_rejects_wrong_field_types() {
        // Required ids must be strings.
        assert!(
            serde_json::from_value::<QueueTaskMessage>(json!({"threadId": 1, "taskId": "tk"}))
                .is_err()
        );
        // A boolean flag given as a string is a type error, not a coercion.
        assert!(serde_json::from_value::<QueueTaskMessage>(
            json!({"threadId": "th", "taskId": "tk", "shadow": "yes"})
        )
        .is_err());
        // contextIds must be an array of strings, not a comma-joined string.
        assert!(serde_json::from_value::<QueueTaskMessage>(
            json!({"threadId": "th", "taskId": "tk", "contextIds": "a,b"})
        )
        .is_err());
    }

    #[test]
    fn validate_task_identifiers_checks_both_ids() {
        let ok = parse_task(json!({"threadId": "th-1", "taskId": "tk-1"}));
        assert!(validate_task_identifiers(&ok).is_ok());

        // An unsafe threadId is rejected and the error names the field.
        let bad_thread = parse_task(json!({"threadId": "../etc", "taskId": "tk-1"}));
        let err = validate_task_identifiers(&bad_thread).unwrap_err();
        assert!(err.contains("threadId"), "unexpected error: {err}");

        // A valid threadId but an unsafe taskId must still be rejected: proves
        // the taskId is validated too, not just the first field.
        let bad_task = parse_task(json!({"threadId": "th-1", "taskId": "a/b"}));
        let err = validate_task_identifiers(&bad_task).unwrap_err();
        assert!(err.contains("taskId"), "unexpected error: {err}");
    }

    // ---- routing / subject-to-handler mapping --------------------------

    #[test]
    fn is_shadow_task_uses_flag_or_message_kind() {
        // The explicit shadow flag alone marks a shadow task.
        assert!(is_shadow_task(&parse_task(
            json!({"threadId": "t", "taskId": "k", "shadow": true})
        )));
        // messageKind == "task.shadow" marks it even when the flag is false/absent.
        assert!(is_shadow_task(&parse_task(
            json!({"threadId": "t", "taskId": "k", "shadow": false, "messageKind": "task.shadow"})
        )));
        assert!(is_shadow_task(&parse_task(
            json!({"threadId": "t", "taskId": "k", "messageKind": "task.shadow"})
        )));
        // Neither signal present → not a shadow task.
        assert!(!is_shadow_task(&parse_task(
            json!({"threadId": "t", "taskId": "k"})
        )));
        assert!(!is_shadow_task(&parse_task(
            json!({"threadId": "t", "taskId": "k", "shadow": false, "messageKind": "task.dispatch"})
        )));
    }

    #[test]
    fn container_pool_dispatch_mode_matches_only_pool_aliases() {
        for mode in ["queued-pool", "nats-pool", "container-pool", "pool"] {
            assert!(
                is_container_pool_dispatch_mode(mode),
                "{mode} should be a pool alias"
            );
        }
        // Non-pool modes and case variants are not pool aliases (match is exact).
        for mode in [
            "queued",
            "nats",
            "async",
            "direct",
            "",
            "POOL",
            "Container-Pool",
        ] {
            assert!(
                !is_container_pool_dispatch_mode(mode),
                "{mode} should not be a pool alias"
            );
        }
    }

    #[test]
    fn should_dispatch_to_container_pool_flag_overrides_mode() {
        // The explicit containerPoolDispatch flag wins in both directions,
        // regardless of dispatchMode.
        assert!(should_dispatch_to_container_pool(&parse_task(
            json!({"threadId": "t", "taskId": "k", "containerPoolDispatch": true, "dispatchMode": "queued"})
        )));
        assert!(!should_dispatch_to_container_pool(&parse_task(
            json!({"threadId": "t", "taskId": "k", "containerPoolDispatch": false, "dispatchMode": "container-pool"})
        )));
        // With no explicit flag, the trimmed dispatchMode decides.
        assert!(should_dispatch_to_container_pool(&parse_task(
            json!({"threadId": "t", "taskId": "k", "dispatchMode": "  container-pool  "})
        )));
        assert!(!should_dispatch_to_container_pool(&parse_task(
            json!({"threadId": "t", "taskId": "k", "dispatchMode": "queued"})
        )));
        // A blank or absent dispatchMode is not a pool dispatch.
        assert!(!should_dispatch_to_container_pool(&parse_task(
            json!({"threadId": "t", "taskId": "k", "dispatchMode": "   "})
        )));
        assert!(!should_dispatch_to_container_pool(&parse_task(
            json!({"threadId": "t", "taskId": "k"})
        )));
    }

    #[test]
    fn repo_pool_slug_matches_readme_affinity_example() {
        // The readme documents nodejs-chat-claude-live-mutex-dev for this shape.
        assert_eq!(
            repo_pool_slug("https://github.com/ORG/live-mutex.git", "dev"),
            "nodejs-chat-claude-live-mutex-dev"
        );
        // SCP-style git URL: last '/'-or-':'-delimited segment, .git stripped,
        // and both repo and branch are slug-sanitized (lowercased, '/'→'-').
        assert_eq!(
            repo_pool_slug("git@github.com:Org/My_Repo.git", "feature/New"),
            "nodejs-chat-claude-my-repo-feature-new"
        );
        // A bare repo name with no host or .git suffix.
        assert_eq!(
            repo_pool_slug("simplerepo", "main"),
            "nodejs-chat-claude-simplerepo-main"
        );
    }

    #[test]
    fn sanitize_slug_part_lowercases_collapses_and_caps() {
        assert_eq!(sanitize_slug_part("Hello World"), "hello-world");
        // A run of non-alphanumerics collapses to a single dash.
        assert_eq!(sanitize_slug_part("a__b!!c"), "a-b-c");
        // Leading and trailing dashes are trimmed off.
        assert_eq!(sanitize_slug_part("--Lead--Trail--"), "lead-trail");
        assert_eq!(sanitize_slug_part(""), "");
        // The result is capped at 80 characters.
        assert_eq!(sanitize_slug_part(&"a".repeat(200)).len(), 80);
    }

    // ---- config: env defaults / overrides ------------------------------

    #[test]
    fn env_value_trims_and_falls_back_on_empty_or_unset() {
        let _guard = env_lock();
        let key = "DD_QC_TEST_ENV_VALUE";
        std::env::remove_var(key);
        assert_eq!(env_value(key, "fb"), "fb"); // unset → fallback
        std::env::set_var(key, "  hello  ");
        assert_eq!(env_value(key, "fb"), "hello"); // surrounding whitespace trimmed
        std::env::set_var(key, "   ");
        assert_eq!(env_value(key, "fb"), "fb"); // whitespace-only → empty → fallback
        std::env::set_var(key, "");
        assert_eq!(env_value(key, "fb"), "fb"); // empty → fallback
        std::env::remove_var(key);
    }

    #[test]
    fn env_i64_accepts_only_positive_integers() {
        let _guard = env_lock();
        let key = "DD_QC_TEST_ENV_I64";
        std::env::remove_var(key);
        assert_eq!(env_i64(key, 7), 7); // unset → fallback
        std::env::set_var(key, " 42 ");
        assert_eq!(env_i64(key, 7), 42); // trimmed then parsed
        std::env::set_var(key, "0");
        assert_eq!(env_i64(key, 7), 7); // zero rejected (must be > 0)
        std::env::set_var(key, "-5");
        assert_eq!(env_i64(key, 7), 7); // negative rejected
        std::env::set_var(key, "notanumber");
        assert_eq!(env_i64(key, 7), 7); // non-numeric rejected
        std::env::remove_var(key);
    }

    #[test]
    fn env_u64_accepts_only_positive_integers() {
        let _guard = env_lock();
        let key = "DD_QC_TEST_ENV_U64";
        std::env::remove_var(key);
        assert_eq!(env_u64(key, 9), 9); // unset → fallback
        std::env::set_var(key, " 120 ");
        assert_eq!(env_u64(key, 9), 120); // trimmed then parsed
        std::env::set_var(key, "0");
        assert_eq!(env_u64(key, 9), 9); // zero rejected (must be > 0)
        std::env::set_var(key, "-1");
        assert_eq!(env_u64(key, 9), 9); // negative fails u64 parse
        std::env::set_var(key, "abc");
        assert_eq!(env_u64(key, 9), 9); // non-numeric rejected
        std::env::remove_var(key);
    }

    #[test]
    fn env_bool_matches_truthy_tokens_case_insensitively() {
        let _guard = env_lock();
        let key = "DD_QC_TEST_ENV_BOOL";
        std::env::remove_var(key);
        assert!(env_bool(key, true)); // unset → fallback (true)
        assert!(!env_bool(key, false)); // unset → fallback (false)
        for truthy in ["1", "true", "TRUE", " yes ", "On"] {
            std::env::set_var(key, truthy);
            assert!(env_bool(key, false), "{truthy:?} should be truthy");
        }
        // Any present-but-non-truthy value is false even when the fallback is true.
        for falsy in ["0", "false", "no", "off", "2", ""] {
            std::env::set_var(key, falsy);
            assert!(
                !env_bool(key, true),
                "{falsy:?} should override fallback to false"
            );
        }
        std::env::remove_var(key);
    }

    #[test]
    fn server_auth_secret_precedence_and_default() {
        let _guard = env_lock();
        let primary = "REMOTE_DEV_SERVER_SECRET";
        let secondary = "SERVER_AUTH_SECRET";
        std::env::remove_var(primary);
        std::env::remove_var(secondary);
        // Both unset → the compiled-in default.
        assert_eq!(server_auth_secret(), DEFAULT_SERVER_SECRET);
        // Secondary key is used when the primary is unset.
        std::env::set_var(secondary, "from-secondary");
        assert_eq!(server_auth_secret(), "from-secondary");
        // Primary takes precedence over the secondary, and is trimmed.
        std::env::set_var(primary, "  from-primary  ");
        assert_eq!(server_auth_secret(), "from-primary");
        std::env::remove_var(primary);
        std::env::remove_var(secondary);
    }

    #[test]
    fn optional_env_returns_none_for_unset_or_blank() {
        let _guard = env_lock();
        let key = "DD_QC_TEST_OPTIONAL_ENV";
        std::env::remove_var(key);
        assert_eq!(optional_env(key), None); // unset → None
        std::env::set_var(key, "   ");
        assert_eq!(optional_env(key), None); // blank → None
        std::env::set_var(key, "  value  ");
        assert_eq!(optional_env(key), Some("value".to_string())); // trimmed
        std::env::remove_var(key);
    }

    // ---- dedup / idempotency receipts ----------------------------------

    #[test]
    fn receipt_round_trip_persists_and_detects_by_task_id() {
        let dir = unique_receipts_dir("roundtrip");
        let _ = fs::remove_dir_all(&dir);
        let base = dir.to_str().unwrap();
        let task = parse_task(json!({
            "threadId": "th-1",
            "taskId": "tk-1",
            "messageKind": "task.dispatch"
        }));

        let mut receipts = HashSet::new();
        // Absent before any write.
        assert!(!has_task_receipt(&mut receipts, base, "tk-1"));

        write_task_receipt(base, &task).expect("receipt should persist");

        // A brand-new in-memory cache still detects the durable on-disk receipt,
        // and the disk hit warms the in-memory fast path.
        let mut fresh = HashSet::new();
        assert!(has_task_receipt(&mut fresh, base, "tk-1"));
        assert!(fresh.contains("tk-1"));
        // A different task id is not suppressed.
        assert!(!has_task_receipt(&mut fresh, base, "tk-2"));

        // The write was atomic: exactly the final .json exists, with no
        // half-written .tmp-* file left behind.
        let entries: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "unexpected receipt dir contents: {entries:?}"
        );
        assert!(
            entries[0].ends_with(".json"),
            "not a .json receipt: {entries:?}"
        );
        assert!(
            !entries[0].contains(".tmp-"),
            "leftover tmp file: {entries:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn has_task_receipt_rejects_untrusted_files_and_uses_cache() {
        let dir = unique_receipts_dir("untrusted");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let base = dir.to_str().unwrap();

        // The in-memory cache short-circuits even with no file on disk.
        let mut receipts = HashSet::new();
        record_receipt(&mut receipts, "cached-id");
        assert!(has_task_receipt(&mut receipts, base, "cached-id"));

        let mut cold = HashSet::new();
        // A receipt whose recorded taskId does not match must not suppress work
        // (guards the sanitized-filename collision case).
        fs::write(
            receipt_path(base, "real-id"),
            serde_json::to_vec(&json!({"taskId": "someone-else"})).unwrap(),
        )
        .unwrap();
        assert!(!has_task_receipt(&mut cold, base, "real-id"));
        // A corrupt (non-JSON) receipt file is likewise not trusted.
        fs::write(receipt_path(base, "corrupt-id"), b"not json {{{").unwrap();
        assert!(!has_task_receipt(&mut cold, base, "corrupt-id"));
        // A receipt whose taskId matches IS trusted, and warms the cache.
        fs::write(
            receipt_path(base, "match-id"),
            serde_json::to_vec(&json!({"taskId": "match-id"})).unwrap(),
        )
        .unwrap();
        assert!(has_task_receipt(&mut cold, base, "match-id"));
        assert!(cold.contains("match-id"));

        let _ = fs::remove_dir_all(&dir);
    }

    // ---- log schema / critical-event / status shaping ------------------

    #[test]
    fn severity_number_maps_known_levels_and_defaults() {
        assert_eq!(severity_number("FATAL"), 24);
        assert_eq!(severity_number("ERROR"), 17);
        assert_eq!(severity_number("WARN"), 13);
        assert_eq!(severity_number("INFO"), 9);
        assert_eq!(severity_number("DEBUG"), 5);
        // Unknown or wrong-case labels fall through to the minimum severity.
        assert_eq!(severity_number("TRACE"), 1);
        assert_eq!(severity_number("error"), 1);
        assert_eq!(severity_number(""), 1);
    }

    #[test]
    fn structured_log_record_uses_log_schema_and_service_identity() {
        let record = structured_log_record("WARN", "some-event", "a body", json!({"k": "v"}));
        assert_eq!(record["schema"], LOG_SCHEMA);
        assert_eq!(record["severity_text"], "WARN");
        assert_eq!(record["severity_number"], severity_number("WARN"));
        assert_eq!(record["resource_service_name"], SERVICE_NAME);
        assert_eq!(record["resource_service_namespace"], SERVICE_NAMESPACE);
        assert_eq!(record["scope_name"], LOG_SCOPE);
        assert_eq!(record["event_name"], "some-event");
        assert_eq!(record["body"], "a body");
        assert_eq!(record["attributes"]["k"], "v");
        // The timestamp is emitted as a stringified unix-nano stamp.
        assert!(record["time_unix_nano"].is_string());
    }

    #[test]
    fn compact_critical_event_attributes_extracts_and_falls_back() {
        // Direct extraction from top-level fields and log.attributes.
        let payload = json!({
            "schema": "dd.log.v1",
            "type": "runtime-critical-event",
            "source": "svc-a",
            "eventName": "boom",
            "severity": "ERROR",
            "log": {"attributes": {"threadId": "th-A", "taskId": "tk-A"}}
        });
        let attrs = compact_critical_event_attributes("crit.subject", 42, &payload);
        assert_eq!(attrs["criticalSubject"], "crit.subject");
        assert_eq!(attrs["payloadBytes"], 42);
        assert_eq!(attrs["upstreamSchema"], "dd.log.v1");
        assert_eq!(attrs["upstreamType"], "runtime-critical-event");
        assert_eq!(attrs["upstreamSource"], "svc-a");
        assert_eq!(attrs["upstreamEventName"], "boom");
        assert_eq!(attrs["upstreamSeverity"], "ERROR");
        assert_eq!(attrs["threadId"], "th-A");
        assert_eq!(attrs["taskId"], "tk-A");

        // Fallbacks: source/eventName/severity read from the nested log record,
        // and ids fall back to the dd.request.* attribute keys.
        let fallback = json!({
            "log": {
                "resource_service_name": "svc-b",
                "event_name": "nested-event",
                "severity_text": "WARN",
                "attributes": {
                    "dd.request.thread_id": "th-B",
                    "dd.request.task_id": "tk-B"
                }
            }
        });
        let attrs = compact_critical_event_attributes("s", 0, &fallback);
        assert_eq!(attrs["upstreamSource"], "svc-b");
        assert_eq!(attrs["upstreamEventName"], "nested-event");
        assert_eq!(attrs["upstreamSeverity"], "WARN");
        assert_eq!(attrs["threadId"], "th-B");
        assert_eq!(attrs["taskId"], "tk-B");

        // Final fallback: ids read from the payload root when no log exists, and
        // absent optional fields serialize as JSON null.
        let root_ids = json!({"threadId": "th-C", "taskId": "tk-C"});
        let attrs = compact_critical_event_attributes("s", 0, &root_ids);
        assert_eq!(attrs["threadId"], "th-C");
        assert_eq!(attrs["taskId"], "tk-C");
        assert!(attrs["upstreamSchema"].is_null());
    }

    #[test]
    fn queue_status_event_shape_and_flag_defaults() {
        let task = parse_task(json!({
            "threadId": "th",
            "taskId": "tk",
            "messageKind": "task.dispatch"
        }));
        let event = queue_status_event(&task, "queue-received", "ok", "hello", json!({"a": 1}));
        assert_eq!(event["kind"], "status");
        assert_eq!(event["source"], "dd-remote-queue-consumer");
        assert_eq!(event["stage"], "queue-received");
        assert_eq!(event["status"], "ok");
        assert_eq!(event["message"], "hello");
        assert_eq!(event["messageKind"], "task.dispatch");
        // shadow/directDispatch default to false when the envelope omits them.
        assert_eq!(event["shadow"], false);
        assert_eq!(event["directDispatch"], false);
        assert_eq!(event["details"]["a"], 1);
        assert!(event["atMs"].is_number());
    }

    #[test]
    fn task_message_id_combines_task_id_and_stage() {
        let task = parse_task(json!({"threadId": "th", "taskId": "tk-9"}));
        assert_eq!(task_message_id(&task, "prepare"), "tk-9:prepare");
        assert_eq!(task_message_id(&task, "dispatch"), "tk-9:dispatch");
    }

    #[test]
    fn render_metrics_exposes_every_counter_and_ready_gauge() {
        let metrics = render_metrics();
        for name in [
            "dd_queue_consumer_messages_received_total",
            "dd_queue_consumer_fetch_errors_total",
            "dd_queue_consumer_invalid_messages_total",
            "dd_queue_consumer_duplicate_messages_total",
            "dd_queue_consumer_ack_progress_failures_total",
            "dd_queue_consumer_handoff_successes_total",
            "dd_queue_consumer_handoff_failures_total",
            "dd_queue_consumer_dead_lettered_total",
            "dd_queue_consumer_dlq_duplicates_total",
            "dd_queue_consumer_dlq_publish_failures_total",
            "dd_queue_consumer_dlq_publish_exhausted_total",
            "dd_queue_consumer_dlq_source_messages_preserved_total",
        ] {
            assert!(
                metrics.contains(&format!("# TYPE {name} counter")),
                "missing TYPE line for {name}"
            );
            // Each counter also emits a "name <value>" sample line.
            assert!(
                metrics
                    .lines()
                    .any(|line| line.starts_with(&format!("{name} "))),
                "missing sample line for {name}"
            );
        }
        // The readiness gauge is present and its value is a 0/1 flag.
        assert!(metrics.contains("# TYPE dd_queue_consumer_ready gauge"));
        let ready_line = metrics
            .lines()
            .find(|line| line.starts_with("dd_queue_consumer_ready "))
            .expect("ready gauge sample line");
        let value = ready_line.rsplit(' ').next().unwrap();
        assert!(
            value == "0" || value == "1",
            "ready gauge value was {value:?}"
        );
    }

    #[test]
    fn dlq_backoff_is_bounded_and_message_id_is_stable() {
        let base = Duration::from_millis(100);
        assert_eq!(dlq_publish_backoff(1, base), Duration::from_millis(100));
        assert_eq!(dlq_publish_backoff(2, base), Duration::from_millis(200));
        assert_eq!(dlq_publish_backoff(5, base), Duration::from_millis(1_600));
        assert_eq!(dlq_publish_backoff(100, base), Duration::from_secs(30));

        let source = DeadLetterSource {
            stream: "SOURCE".to_string(),
            consumer: "worker".to_string(),
            original_subject: "tasks.one".to_string(),
            stream_sequence: 42,
            consumer_sequence: 7,
            delivered: 5,
        };
        assert_eq!(
            dead_letter_message_id(&source, b"payload-a"),
            dead_letter_message_id(&source, b"payload-b"),
            "stream sequence is the durable source identity"
        );
        let fallback = DeadLetterSource {
            stream_sequence: 0,
            ..source
        };
        assert_ne!(
            dead_letter_message_id(&fallback, b"payload-a"),
            dead_letter_message_id(&fallback, b"payload-b")
        );
    }

    #[test]
    fn configured_dlq_policy_clamps_retry_limits() {
        let _guard = env_lock();
        std::env::set_var("NATS_TASK_DLQ_PUBLISH_ATTEMPTS", "999");
        std::env::set_var("NATS_TASK_DLQ_RETRY_BASE_MS", "1");
        std::env::set_var("NATS_TASK_DLQ_PUBLISH_TIMEOUT_MS", "999999");
        let policy = configured_dlq_publish_policy();
        assert_eq!(policy.attempts, 20);
        assert_eq!(policy.retry_base, Duration::from_millis(10));
        assert_eq!(policy.publish_timeout, Duration::from_millis(30_000));
        std::env::remove_var("NATS_TASK_DLQ_PUBLISH_ATTEMPTS");
        std::env::remove_var("NATS_TASK_DLQ_RETRY_BASE_MS");
        std::env::remove_var("NATS_TASK_DLQ_PUBLISH_TIMEOUT_MS");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_dlq_failure_preserves_source_then_recovers_idempotently() {
        let Ok(nats_url) = std::env::var("NATS_DLQ_E2E_URL") else {
            return;
        };
        let client = async_nats::connect(&nats_url)
            .await
            .expect("connect to live NATS test server");
        let jetstream = async_nats::jetstream::new(client.clone());
        let suffix = format!("{}-{}", std::process::id(), now_unix_nano());
        let source_stream_name = format!("DLQ_SOURCE_{}", suffix.replace('-', "_"));
        let dlq_stream_name = format!("DLQ_TARGET_{}", suffix.replace('-', "_"));
        let source_subject = format!("e2e.source.{suffix}");
        let dlq_subject = format!("e2e.dlq.{suffix}");
        let consumer_name = format!("worker-{}", suffix);

        let source_stream = jetstream
            .create_stream(async_nats::jetstream::stream::Config {
                name: source_stream_name.clone(),
                subjects: vec![source_subject.clone()],
                retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
                storage: async_nats::jetstream::stream::StorageType::Memory,
                ..Default::default()
            })
            .await
            .expect("create source stream");
        let consumer = source_stream
            .create_consumer(async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(consumer_name.clone()),
                filter_subject: source_subject.clone(),
                ack_wait: Duration::from_secs(2),
                max_deliver: 1,
                ..Default::default()
            })
            .await
            .expect("create source consumer");
        let task_json = json!({
            "threadId": "thread-e2e",
            "taskId": "task-e2e",
            "messageKind": "task.dispatch"
        });
        let payload = serde_json::to_vec(&task_json).unwrap();
        jetstream
            .publish(source_subject.clone(), payload.clone().into())
            .await
            .expect("start source publish")
            .await
            .expect("source publish ack");

        let mut messages = consumer.messages().await.expect("open source consumer");
        let message = tokio::time::timeout(Duration::from_secs(5), messages.next())
            .await
            .expect("source delivery timeout")
            .expect("source stream ended")
            .expect("source delivery error");
        let task = serde_json::from_slice::<QueueTaskMessage>(&message.payload).unwrap();
        let policy = DlqPublishPolicy {
            attempts: 2,
            retry_base: Duration::from_millis(10),
            publish_timeout: Duration::from_millis(500),
        };

        // No DLQ stream exists. The transfer must fail without Terminating the
        // source message—the historical implementation lost it here.
        nak_or_dead_letter(
            &message,
            &client,
            "e2e.critical",
            &dlq_subject,
            &source_stream_name,
            &task,
            1,
            Duration::from_millis(10),
            "forced handoff failure",
            policy,
        )
        .await;
        let source_info = source_stream
            .get_info()
            .await
            .expect("source info after failure");
        assert_eq!(
            source_info.state.messages, 1,
            "source message was lost on DLQ failure"
        );

        jetstream
            .create_stream(async_nats::jetstream::stream::Config {
                name: dlq_stream_name.clone(),
                subjects: vec![dlq_subject.clone()],
                retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
                storage: async_nats::jetstream::stream::StorageType::Memory,
                ..Default::default()
            })
            .await
            .expect("create repaired DLQ stream");

        // Retry the same source delivery after repair. Durable ACK precedes Term.
        nak_or_dead_letter(
            &message,
            &client,
            "e2e.critical",
            &dlq_subject,
            &source_stream_name,
            &task,
            1,
            Duration::from_millis(10),
            "forced handoff failure",
            policy,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let source_info = source_stream
            .get_info()
            .await
            .expect("source info after recovery");
        let dlq_stream = jetstream
            .get_stream(&dlq_stream_name)
            .await
            .expect("get repaired DLQ stream");
        let dlq_info = dlq_stream.get_info().await.expect("DLQ info");
        assert_eq!(
            source_info.state.messages, 0,
            "source was not Terminated after DLQ ACK"
        );
        assert_eq!(
            dlq_info.state.messages, 1,
            "DLQ did not receive exactly one record"
        );

        // Re-running the transfer simulates an ACK/Term ambiguity. Nats-Msg-Id
        // must make the DLQ write idempotent and keep the target at one record.
        let source = dead_letter_source(&message, &source_stream_name);
        let encoded = serde_json::to_vec(&dead_letter_payload(
            &message,
            &source,
            Some(&task),
            "handoff-failed-after-max-deliver",
            "forced handoff failure",
            1,
        ))
        .unwrap();
        let duplicate_ack = publish_dead_letter_with_retry(
            &message,
            &client,
            &dlq_subject,
            &source,
            encoded,
            policy,
        )
        .await
        .expect("duplicate DLQ publish ack");
        assert!(
            duplicate_ack.duplicate,
            "server did not de-duplicate DLQ retry"
        );
        let dlq_info = dlq_stream
            .get_info()
            .await
            .expect("DLQ info after duplicate");
        assert_eq!(dlq_info.state.messages, 1);

        let _ = jetstream.delete_stream(&source_stream_name).await;
        let _ = jetstream.delete_stream(&dlq_stream_name).await;
    }
}
