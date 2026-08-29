use std::{env, error::Error, sync::atomic::Ordering, time::Duration};

use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::ingest::{process_ingest_request, process_scrape_request, process_webhook};
use crate::state::{env_value, AppState, MAX_NATS_PAYLOAD_BYTES, SERVICE_NAME};
use crate::types::{IngestRequest, ScrapeRequest, WebhookIngestRequest};
use crate::util::now_ms;

pub(crate) async fn publish_json(state: &AppState, subject: &str, value: &Value) {
    let Some(nats) = state.nats.as_ref() else {
        return;
    };
    match serde_json::to_vec(value) {
        Ok(payload) => {
            if nats
                .publish(subject.to_string(), payload.into())
                .await
                .is_ok()
            {
                state
                    .metrics
                    .nats_published_total
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            tracing::error!("public-data failed to encode nats payload: {error}");
        }
    }
}

pub(crate) async fn publish_runtime_event(state: &AppState, event_type: &str, attrs: Value) {
    publish_json(
        state,
        &state.config.runtime_event_subject,
        &json!({
            "type": event_type,
            "source": SERVICE_NAME,
            "atMs": now_ms(),
            "attributes": attrs
        }),
    )
    .await;
}

pub(crate) async fn run_nats_loop(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        tracing::info!("public-data nats loop disabled: NATS_URL is not configured");
        return;
    };
    tracing::info!(
        "public-data nats loop starting: subject={} queueGroup={} resultSubject={}",
        state.config.ingest_request_subject,
        state.config.queue_group,
        state.config.ingest_result_subject
    );
    // Bound in-flight handlers. Each message can trigger a scrape (outbound HTTP)
    // or ingest (DB writes); previously every one was `tokio::spawn`ed with no
    // ceiling, so a burst could spawn unbounded concurrent scrapes and exhaust
    // the pod. Acquiring a permit before spawning also backpressures the
    // subscription instead of piling work on. Kept modest since scrapes are heavy.
    let max_concurrency = env::var("PUBLIC_DATA_NATS_MAX_CONCURRENCY")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(32);
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(max_concurrency));
    // Durable JetStream consumer instead of core queue_subscribe. Core NATS is
    // at-most-once: a crash between receiving a message and finishing the scrape
    // or ingest silently dropped the work. A JetStream WorkQueue durable pull
    // consumer redelivers unacked messages, and we ack ONLY after the handler
    // succeeds. A long scrape is kept alive with AckKind::Progress heartbeats so
    // it isn't redelivered mid-flight; a message that exhausts max_deliver is
    // Term'd and copied to the dead-letter subject so it can't linger in the
    // WorkQueue stream until max_age.
    let jetstream = async_nats::jetstream::new(nats.clone());
    let stream_name = env_value("PUBLIC_DATA_INGEST_STREAM", "DD_PUBLIC_DATA_INGEST");
    let consumer_name = env_value("PUBLIC_DATA_INGEST_CONSUMER", &state.config.queue_group);
    let ack_wait = Duration::from_secs(
        env::var("PUBLIC_DATA_NATS_ACK_WAIT_SECONDS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(120),
    );
    let max_deliver: i64 = env::var("PUBLIC_DATA_NATS_MAX_DELIVER")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(5);
    let nak_delay = Duration::from_secs(15);
    let dlq_subject = env_value(
        "PUBLIC_DATA_NATS_DLQ_SUBJECT",
        "dd.remote.public_data.ingest.deadletter",
    );
    let ack_progress_every = (ack_wait / 3).max(Duration::from_secs(5));

    loop {
        let consumer = match build_ingest_consumer(
            &jetstream,
            &stream_name,
            &state.config.ingest_request_subject,
            &consumer_name,
            ack_wait,
            max_deliver,
        )
        .await
        {
            Ok(consumer) => consumer,
            Err(error) => {
                tracing::error!("public-data jetstream consumer setup failed: {error}; retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let mut messages = match consumer.messages().await {
            Ok(messages) => messages,
            Err(error) => {
                tracing::error!("public-data jetstream messages() failed: {error}; retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        while let Some(next) = messages.next().await {
            let message = match next {
                Ok(message) => message,
                Err(error) => {
                    tracing::error!("public-data jetstream fetch failed: {error}; re-subscribing");
                    break;
                }
            };
            state
                .metrics
                .nats_messages_total
                .fetch_add(1, Ordering::Relaxed);
            if message.payload.len() > MAX_NATS_PAYLOAD_BYTES {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                tracing::error!(
                    "public-data rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                    message.payload.len()
                );
                // An oversize payload will never get smaller; ack to drop it.
                let _ = message.ack().await;
                continue;
            }
            // Backpressure point: wait for a concurrency permit before spawning.
            let permit = match semaphore.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => break,
            };
            let task_state = state.clone();
            let dlq_nats = nats.clone();
            let dlq = dlq_subject.clone();
            tokio::spawn(async move {
                let _permit = permit; // released when this handler finishes
                let payload = message.payload.to_vec();
                // Process under an ack-progress heartbeat so a long scrape keeps
                // the ack deadline alive instead of being redelivered mid-flight.
                let outcome = run_with_ack_progress(
                    &message,
                    ack_progress_every,
                    process_ingest_payload(&task_state, &payload),
                )
                .await;
                match outcome {
                    Ok(()) => {
                        if let Err(error) = message.ack().await {
                            tracing::error!("public-data ack failed: {error}");
                        }
                    }
                    Err(error) => {
                        task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                        tracing::error!("public-data nats request failed: {error}");
                        let delivered = message.info().map(|info| info.delivered).unwrap_or(0);
                        if max_deliver > 0 && delivered >= max_deliver {
                            // Exhausted: preserve on the dead-letter subject, then
                            // Term so it leaves the WorkQueue stream immediately.
                            let _ = dlq_nats.publish(dlq.clone(), payload.into()).await;
                            let _ = dlq_nats.flush().await;
                            let _ = message
                                .ack_with(async_nats::jetstream::AckKind::Term)
                                .await;
                        } else {
                            let _ = message
                                .ack_with(async_nats::jetstream::AckKind::Nak(Some(nak_delay)))
                                .await;
                        }
                    }
                }
            });
        }
        tracing::error!(
            "public-data jetstream message stream ended (subject={}); re-subscribing in 5s",
            state.config.ingest_request_subject
        );
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Ensure the ingest WorkQueue stream and its durable pull consumer exist. The
/// service creates them itself (like dd-remote-queue-consumer) rather than
/// relying on a declarative CRD, so a fresh cluster boots without manual setup.
pub(crate) async fn build_ingest_consumer(
    jetstream: &async_nats::jetstream::Context,
    stream_name: &str,
    subject: &str,
    consumer_name: &str,
    ack_wait: Duration,
    max_deliver: i64,
) -> Result<async_nats::jetstream::consumer::PullConsumer, Box<dyn Error + Send + Sync>> {
    let stream = jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: stream_name.to_string(),
            subjects: vec![subject.to_string()],
            retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
            max_age: Duration::from_secs(60 * 60 * 24 * 7),
            max_message_size: MAX_NATS_PAYLOAD_BYTES as i32,
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
                max_deliver,
                ..Default::default()
            },
        )
        .await?;
    Ok(consumer)
}

/// Await `fut` while extending the JetStream ack deadline for `message` with
/// AckKind::Progress every `interval`, so a legitimately long scrape/ingest is
/// not mistaken for a stalled delivery and redelivered.
pub(crate) async fn run_with_ack_progress<F, T>(
    message: &async_nats::jetstream::Message,
    interval: Duration,
    fut: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    let mut fut = std::pin::pin!(fut);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await; // consume the immediate first tick
    loop {
        tokio::select! {
            biased;
            out = &mut fut => return out,
            _ = ticker.tick() => {
                let _ = message
                    .ack_with(async_nats::jetstream::AckKind::Progress)
                    .await;
            }
        }
    }
}

/// Parse and dispatch one ingest message, returning Ok only when the work
/// actually completed (so the caller acks) or Err to trigger redelivery.
pub(crate) async fn process_ingest_payload(state: &AppState, payload: &[u8]) -> Result<(), String> {
    let value: Value = serde_json::from_slice(payload).map_err(|error| error.to_string())?;
    if value.get("scrape").is_some() {
        let request = serde_json::from_value::<ScrapeRequest>(value["scrape"].clone())
            .map_err(|error| error.to_string())?;
        process_scrape_request(state, request)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    } else if value.get("url").is_some() && value.get("records").is_none() {
        let request =
            serde_json::from_value::<ScrapeRequest>(value).map_err(|error| error.to_string())?;
        process_scrape_request(state, request)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    } else if value.get("webhook").is_some() {
        let request = serde_json::from_value::<WebhookIngestRequest>(value["webhook"].clone())
            .map_err(|error| error.to_string())?;
        process_webhook(state, request)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    } else {
        let request =
            serde_json::from_value::<IngestRequest>(value).map_err(|error| error.to_string())?;
        process_ingest_request(state, request)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}
