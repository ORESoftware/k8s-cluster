//! NATS MQ integration.
//!
//! Publishing (always on when NATS connects):
//! - `dd.remote.build_server.events`  — redacted job lifecycle events
//! - `dd.remote.build_server.results` — terminal results
//! - `dd.remote.build_server.images`  — registry webhook relays
//! - `dd.remote.events.critical`      — alert-worthy failures (Observability
//!   Contract: compact, redacted, dd.log.v1-compatible)
//!
//! Intake (opt-in via BUILD_SERVER_NATS_INTAKE_ENABLED): a durable JetStream
//! pull consumer on the WorkQueue stream DD_REMOTE_BUILD_JOBS over
//! `dd.remote.build_server.requests`, so CI (GitHub Actions or other
//! services) can enqueue build-server.v1 jobs without holding an HTTP
//! connection, with at-least-once delivery. Dedupe: Nats-Msg-Id +
//! webhook_deliveries/idempotency guards in the submit path.
//!
//! Subjects come from the generated dd-nats-subject-defs constants (schema:
//! remote/libs/nats/subject-defs/schema/build-server.schema.json), each
//! overridable by env.

use async_nats::jetstream::{
    self,
    consumer::pull,
    stream::{Config as StreamConfig, RetentionPolicy, StorageType},
};
use futures_util::StreamExt;
use serde_json::json;
use std::{sync::atomic::Ordering, time::Duration};

use crate::{now_ms, AppState, BuildJobRecord, BuildStatus, SERVICE_NAME};

pub async fn connect(url: &str) -> Result<async_nats::Client, String> {
    async_nats::ConnectOptions::new()
        .name(SERVICE_NAME)
        .retry_on_initial_connect()
        .ping_interval(Duration::from_secs(15))
        .connection_timeout(Duration::from_secs(10))
        .connect(url)
        .await
        .map_err(|error| format!("failed to connect to NATS at {url}: {error}"))
}

fn status_label(status: &BuildStatus) -> &'static str {
    match status {
        BuildStatus::Queued => "queued",
        BuildStatus::Running => "running",
        BuildStatus::Succeeded => "succeeded",
        BuildStatus::Failed => "failed",
    }
}

/// Compact, redacted lifecycle event. Only identifiers and coarse metadata —
/// never build args, logs, or secrets.
fn lifecycle_payload(job: &BuildJobRecord) -> serde_json::Value {
    json!({
        "schemaVersion": "build-server.event.v1",
        "service": SERVICE_NAME,
        "jobId": job.id,
        "status": status_label(&job.status),
        "jobKind": job.request.job_kind.as_deref().unwrap_or("build-image"),
        "source": job.source,
        "executor": job.executor,
        "repoUrl": job.request.repo_url,
        "gitRef": job.request.git_ref,
        "image": job.request.image,
        "fencingToken": job.fencing_token,
        "error": job.error,
        "tsMs": now_ms() as u64,
    })
}

async fn publish(state: &AppState, subject: &str, payload: serde_json::Value) {
    let Some(nats) = state.nats.as_ref() else {
        return;
    };
    let bytes = payload.to_string().into_bytes();
    match nats.publish(subject.to_string(), bytes.into()).await {
        Ok(()) => {
            state.counters.nats_published.fetch_add(1, Ordering::Relaxed);
        }
        Err(error) => {
            state
                .counters
                .nats_publish_failures
                .fetch_add(1, Ordering::Relaxed);
            tracing::warn!("failed to publish NATS event to {subject}: {error}");
        }
    }
}

pub async fn publish_lifecycle(state: &AppState, job: &BuildJobRecord) {
    let payload = lifecycle_payload(job);
    publish(state, &state.config.nats_event_subject, payload.clone()).await;
    match job.status {
        BuildStatus::Succeeded | BuildStatus::Failed => {
            publish(state, &state.config.nats_result_subject, payload.clone()).await;
        }
        _ => {}
    }
    // Alert-worthy operational failure → compact critical event
    // (Observability Contract, AGENTS.md).
    if matches!(job.status, BuildStatus::Failed) {
        let critical = json!({
            "schemaVersion": "dd.log.v1",
            "level": "error",
            "service": SERVICE_NAME,
            "message": format!(
                "build job {} failed for {} ({})",
                job.id,
                job.request.image,
                job.error.as_deref().unwrap_or("unknown error"),
            ),
            "jobId": job.id,
            "tsMs": now_ms() as u64,
        });
        publish(state, &state.config.nats_critical_subject, critical).await;
    }
}

pub async fn publish_image_event(state: &AppState, payload: serde_json::Value) {
    publish(state, &state.config.nats_image_subject, payload).await;
}

/// Durable JetStream request intake. Runs until process shutdown; connection
/// problems back off and retry rather than crashing the server.
pub async fn run_request_intake(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        return;
    };
    let context = jetstream::new(nats);
    loop {
        match intake_once(&state, &context).await {
            Ok(()) => {}
            Err(error) => {
                tracing::warn!("NATS build-request intake error: {error}; retrying in 10s");
            }
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

async fn intake_once(state: &AppState, context: &jetstream::Context) -> Result<(), String> {
    let subject = state.config.nats_request_subject.clone();
    let stream = context
        .get_or_create_stream(StreamConfig {
            name: dd_nats_subject_defs::DD_REMOTE_BUILD_JOBS_STREAM_NAME.to_string(),
            subjects: vec![subject.clone()],
            retention: RetentionPolicy::WorkQueue,
            storage: StorageType::File,
            max_age: Duration::from_secs(14 * 24 * 3600),
            ..Default::default()
        })
        .await
        .map_err(|error| format!("failed to ensure build-jobs stream: {error}"))?;

    let consumer = stream
        .get_or_create_consumer(
            dd_nats_subject_defs::BUILD_SERVER_REQUESTS_QUEUE_GROUP,
            pull::Config {
                durable_name: Some(
                    dd_nats_subject_defs::BUILD_SERVER_REQUESTS_QUEUE_GROUP.to_string(),
                ),
                filter_subject: subject,
                ack_wait: Duration::from_secs(120),
                max_ack_pending: 16,
                max_deliver: 3,
                ..Default::default()
            },
        )
        .await
        .map_err(|error| format!("failed to ensure build-jobs consumer: {error}"))?;

    let mut messages = consumer
        .messages()
        .await
        .map_err(|error| format!("failed to open build-jobs message stream: {error}"))?;

    while let Some(message) = messages.next().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => return Err(format!("build-jobs message stream failed: {error}")),
        };
        let outcome = crate::submit_from_nats(state, &message.payload).await;
        let ack = match outcome {
            // Accepted (or a duplicate we already handled): done with this message.
            Ok(()) => message.ack().await,
            // Invalid payloads can never succeed — terminate delivery.
            Err(crate::NatsSubmitError::Invalid(reason)) => {
                tracing::warn!("rejecting NATS build request: {reason}");
                message
                    .ack_with(jetstream::AckKind::Term)
                    .await
            }
            // Transient (queue full, DB down): NAK with delay for redelivery.
            Err(crate::NatsSubmitError::Transient(reason)) => {
                tracing::warn!("deferring NATS build request: {reason}");
                message
                    .ack_with(jetstream::AckKind::Nak(Some(Duration::from_secs(15))))
                    .await
            }
        };
        if let Err(error) = ack {
            tracing::warn!("failed to ack NATS build request: {error}");
        }
    }
    Ok(())
}
