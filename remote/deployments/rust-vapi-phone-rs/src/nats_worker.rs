//! JetStream work-queue consumer for vapi phone tasks.
//!
//! Enabled only when `VAPI_NATS_URL` is set. The worker provisions the
//! `DD_VAPI_TASKS` stream + durable pull consumer `dd-vapi-phone-worker`
//! (KEDA's nats-jetstream scaler reads that consumer's lag to scale this
//! deployment — see `remote/argocd/dd-next-runtime/dd-rust-vapi-phone.scaledobject.yaml`),
//! then processes tasks published through dd-nats-bridge on `dd.vapi.tasks.>`.
//!
//! Task shapes (JSON):
//!   { "type": "outbound-call", "number": "+15551234567", "assistant_id": "..."? }
//!   { "type": "setup-refresh" }
//!
//! Ack policy: malformed or permanently-invalid tasks are acked and dropped
//! (poison messages must not wedge the queue); transient Vapi/API failures are
//! nak'd for redelivery, bounded by the consumer's max_deliver.

use crate::{
    ensure_phone_number, env_u64, env_value, normalize_e164, upsert_assistant,
    validate_vapi_path_id, vapi_request, AppState,
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::time::Duration;

pub struct NatsWorkerConfig {
    pub url: String,
    pub stream: String,
    pub subject: String,
    pub consumer: String,
    pub ack_wait: Duration,
    pub max_deliver: i64,
}

impl NatsWorkerConfig {
    /// None (worker disabled) unless VAPI_NATS_URL is set.
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("VAPI_NATS_URL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())?;
        Some(Self {
            url,
            stream: env_value("VAPI_NATS_STREAM", "DD_VAPI_TASKS"),
            subject: env_value("VAPI_NATS_SUBJECT", "dd.vapi.tasks.>"),
            consumer: env_value("VAPI_NATS_CONSUMER", "dd-vapi-phone-worker"),
            ack_wait: Duration::from_secs(env_u64("VAPI_NATS_ACK_WAIT_SECONDS", 300)),
            max_deliver: env_u64("VAPI_NATS_MAX_DELIVER", 5) as i64,
        })
    }
}

/// The parsed, validated intent of a queue message.
#[derive(Debug, PartialEq)]
enum TaskAction {
    OutboundCall {
        number: String,
        assistant_id: Option<String>,
    },
    SetupRefresh,
}

/// Parse + validate a task payload. Any Err here is permanent: the message
/// will never become valid on redelivery, so the caller acks and drops it.
fn parse_task(payload: &[u8]) -> Result<TaskAction, String> {
    let value: Value =
        serde_json::from_slice(payload).map_err(|e| format!("task is not valid JSON: {e}"))?;
    let task_type = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or("task is missing string field 'type'")?;
    match task_type {
        "setup-refresh" => Ok(TaskAction::SetupRefresh),
        "outbound-call" => {
            let raw_number = value
                .get("number")
                .and_then(Value::as_str)
                .ok_or("outbound-call task is missing string field 'number'")?;
            let number = normalize_e164(raw_number)?;
            let assistant_id = match value.get("assistant_id").and_then(Value::as_str) {
                Some(id) => {
                    validate_vapi_path_id("assistant_id", id)?;
                    Some(id.to_string())
                }
                None => None,
            };
            Ok(TaskAction::OutboundCall {
                number,
                assistant_id,
            })
        }
        other => Err(format!("unknown task type '{other}'")),
    }
}

/// Outcome of processing one task: acked-and-done vs nak-for-redelivery.
enum ProcessError {
    /// Retrying cannot help (bad task, missing static config) — ack + drop.
    Permanent(String),
    /// Upstream hiccup (Vapi API, network) — nak so JetStream redelivers.
    Transient(String),
}

async fn process_task(state: &AppState, action: TaskAction) -> Result<Value, ProcessError> {
    match action {
        TaskAction::SetupRefresh => {
            let assistant = upsert_assistant(state)
                .await
                .map_err(|e| ProcessError::Transient(e.message.clone()))?;
            let assistant_id = assistant
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ProcessError::Transient("Vapi did not return an assistant id".to_string())
                })?;
            let number = ensure_phone_number(state, assistant_id)
                .await
                .map_err(|e| ProcessError::Transient(e.message.clone()))?;
            Ok(json!({ "assistantId": assistant_id, "phoneNumberId": number.get("id") }))
        }
        TaskAction::OutboundCall {
            number,
            assistant_id,
        } => {
            let assistant_id = assistant_id
                .or_else(|| state.config.assistant_id.clone())
                .ok_or_else(|| {
                    ProcessError::Permanent(
                        "no assistant_id in task and VAPI_ASSISTANT_ID is not configured"
                            .to_string(),
                    )
                })?;
            let phone_number_id = state.config.phone_number_id.clone().ok_or_else(|| {
                ProcessError::Permanent(
                    "VAPI_PHONE_NUMBER_ID is not configured; cannot place outbound calls"
                        .to_string(),
                )
            })?;
            let body = json!({
                "assistantId": assistant_id,
                "phoneNumberId": phone_number_id,
                "customer": { "number": number },
            });
            let call = vapi_request(state, reqwest::Method::POST, "/call", Some(&body))
                .await
                .map_err(|e| ProcessError::Transient(e.message.clone()))?;
            Ok(json!({ "callId": call.get("id") }))
        }
    }
}

/// Run forever: (re)connect, provision stream + consumer, pull and process.
/// Spawned from main; never returns except at process shutdown.
pub async fn run(state: AppState, cfg: NatsWorkerConfig) {
    let mut backoff = Duration::from_secs(1);
    loop {
        match run_once(&state, &cfg).await {
            Ok(()) => backoff = Duration::from_secs(1),
            Err(e) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                tracing::error!("nats worker error, reconnecting in {backoff:?}: {e}");
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

async fn run_once(
    state: &AppState,
    cfg: &NatsWorkerConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = async_nats::ConnectOptions::new()
        .name("dd-rust-vapi-phone")
        .retry_on_initial_connect()
        .connect(&cfg.url)
        .await?;
    let jetstream = async_nats::jetstream::new(client);

    let stream = jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: cfg.stream.clone(),
            subjects: vec![cfg.subject.clone()],
            retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
            storage: async_nats::jetstream::stream::StorageType::File,
            max_age: Duration::from_secs(60 * 60 * 24 * 14),
            max_message_size: 1024 * 1024,
            ..Default::default()
        })
        .await?;

    let consumer = stream
        .get_or_create_consumer::<async_nats::jetstream::consumer::pull::Config>(
            &cfg.consumer,
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(cfg.consumer.clone()),
                filter_subject: cfg.subject.clone(),
                ack_wait: cfg.ack_wait,
                max_deliver: cfg.max_deliver,
                ..Default::default()
            },
        )
        .await?;

    tracing::info!(
        stream = %cfg.stream,
        subject = %cfg.subject,
        consumer = %cfg.consumer,
        "vapi nats worker started"
    );

    let mut messages = consumer.messages().await?;
    while let Some(message) = messages.next().await {
        let message = match message {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("nats message stream error: {e}");
                continue;
            }
        };

        // Stream sequence identifies a task in logs without recording the
        // caller's phone number (PII). It also makes duplicate delivery
        // visible: under work-queue retention each seq must appear once.
        let seq = message
            .info()
            .map(|info| info.stream_sequence)
            .unwrap_or_default();

        match parse_task(&message.payload) {
            Err(reason) => {
                tracing::warn!(subject = %message.subject, seq, "dropping invalid vapi task: {reason}");
                let _ = message.ack().await;
            }
            Ok(action) => match process_task(state, action).await {
                Ok(result) => {
                    tracing::info!(subject = %message.subject, seq, %result, "vapi task done");
                    let _ = message.ack().await;
                }
                Err(ProcessError::Permanent(reason)) => {
                    state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(subject = %message.subject, seq, "dropping unprocessable vapi task: {reason}");
                    let _ = message.ack().await;
                }
                Err(ProcessError::Transient(reason)) => {
                    state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(subject = %message.subject, seq, "vapi task failed, nak for redelivery: {reason}");
                    let _ = message
                        .ack_with(async_nats::jetstream::AckKind::Nak(Some(
                            Duration::from_secs(10),
                        )))
                        .await;
                }
            },
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_setup_refresh() {
        let task = parse_task(br#"{ "type": "setup-refresh" }"#).unwrap();
        assert_eq!(task, TaskAction::SetupRefresh);
    }

    #[test]
    fn parses_outbound_call() {
        let task =
            parse_task(br#"{ "type": "outbound-call", "number": " +15551234567 " }"#).unwrap();
        assert_eq!(
            task,
            TaskAction::OutboundCall {
                number: "+15551234567".to_string(),
                assistant_id: None,
            }
        );
    }

    #[test]
    fn rejects_outbound_call_without_number() {
        assert!(parse_task(br#"{ "type": "outbound-call" }"#).is_err());
        assert!(parse_task(br#"{ "type": "outbound-call", "number": "not-a-number" }"#).is_err());
    }

    #[test]
    fn rejects_unknown_and_malformed_tasks() {
        assert!(parse_task(br#"{ "type": "reboot-cluster" }"#).is_err());
        assert!(parse_task(br#"{ "number": "+15551234567" }"#).is_err());
        assert!(parse_task(b"not json").is_err());
    }

    #[test]
    fn rejects_invalid_assistant_id() {
        let raw = br#"{ "type": "outbound-call", "number": "+15551234567", "assistant_id": "../evil" }"#;
        assert!(parse_task(raw).is_err());
    }
}
