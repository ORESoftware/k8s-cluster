use std::{sync::atomic::Ordering, time::Duration};

use dd_nats_subject_defs::ESCROW_SOLANA_VALIDATE_QUEUE_GROUP;
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::config::{MAX_NATS_PAYLOAD_BYTES, SCHEMA_VERSION, SERVICE_NAME};
use crate::logging::{log_error, log_info, log_warn, structured_log_record};
use crate::state::AppState;
use crate::types::EscrowIntentRequest;
use crate::util::now_ms;
use crate::validation::{request_id, validate_escrow_intent};

pub(crate) async fn publish_validation_result(state: &AppState, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let Ok(encoded) = serde_json::to_vec(&payload) else {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        log_error(
            "escrow-validation-result-serialize-failed",
            "Escrow validation result could not be serialized for NATS.",
            json!({}),
        );
        return;
    };
    match nats
        .publish(state.result_subject.clone(), encoded.into())
        .await
    {
        Ok(()) => {
            state
                .metrics
                .nats_results_published_total
                .fetch_add(1, Ordering::Relaxed);
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            state
                .metrics
                .nats_publish_errors_total
                .fetch_add(1, Ordering::Relaxed);
            publish_runtime_critical_event(
                state,
                "escrow-validation-result-publish-failed",
                "Escrow validation result NATS publish failed.",
                json!({ "subject": state.result_subject, "error": error.to_string() }),
            )
            .await;
        }
    }
}

pub(crate) async fn publish_escrow_event(state: &AppState, event_type: &str, request_id: &str, ok: bool) {
    let Some(nats) = &state.nats else {
        return;
    };
    let payload = json!({
        "type": event_type,
        "source": SERVICE_NAME,
        "requestId": request_id,
        "ok": ok,
        "chain": "solana",
        "schemaVersion": SCHEMA_VERSION,
        "atMs": now_ms(),
    });
    match nats
        .publish(state.event_subject.clone(), payload.to_string().into())
        .await
    {
        Ok(()) => {
            state
                .metrics
                .nats_events_published_total
                .fetch_add(1, Ordering::Relaxed);
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            state
                .metrics
                .nats_publish_errors_total
                .fetch_add(1, Ordering::Relaxed);
            log_warn(
                "escrow-event-publish-failed",
                "Escrow lifecycle event NATS publish failed.",
                json!({
                    "subject": state.event_subject,
                    "eventType": event_type,
                    "requestId": request_id,
                    "error": error.to_string(),
                }),
            );
        }
    }
}

pub(crate) async fn publish_runtime_critical_event(
    state: &AppState,
    event_name: &str,
    body: &str,
    attributes: Value,
) {
    log_error(event_name, body, attributes.clone());
    let Some(nats) = &state.nats else {
        return;
    };
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
    match serde_json::to_vec(&payload) {
        Ok(encoded) => match nats
            .publish(state.critical_event_subject.clone(), encoded.into())
            .await
        {
            Ok(()) => {
                state
                    .metrics
                    .nats_critical_events_published_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => {
                state
                    .metrics
                    .nats_publish_errors_total
                    .fetch_add(1, Ordering::Relaxed);
                log_error(
                    "escrow-critical-event-publish-failed",
                    "Escrow service critical event NATS publish failed.",
                    json!({
                        "subject": state.critical_event_subject,
                        "eventName": event_name,
                        "error": error.to_string(),
                    }),
                );
            }
        },
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            log_error(
                "escrow-critical-event-serialize-failed",
                "Escrow service critical event payload serialization failed.",
                json!({
                    "eventName": event_name,
                    "error": error.to_string(),
                }),
            );
        }
    }
}

pub(crate) async fn run_nats_loop(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        log_info(
            "escrow-nats-loop-disabled",
            "Escrow validation NATS loop is disabled because NATS_URL is not configured.",
            json!({}),
        );
        return;
    };
    log_info(
        "escrow-nats-loop-starting",
        "Escrow validation NATS loop is starting.",
        json!({
            "subject": state.validate_subject,
            "queueGroup": ESCROW_SOLANA_VALIDATE_QUEUE_GROUP,
            "resultSubject": state.result_subject,
            "eventSubject": state.event_subject,
            "criticalEventSubject": state.critical_event_subject,
        }),
    );
    loop {
    let mut subscription = match nats
        .queue_subscribe(
            state.validate_subject.clone(),
            ESCROW_SOLANA_VALIDATE_QUEUE_GROUP.to_string(),
        )
        .await
    {
        Ok(subscription) => subscription,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            publish_runtime_critical_event(
                &state,
                "escrow-nats-subscribe-failed",
                "Escrow service could not subscribe to validation requests.",
                json!({ "error": error.to_string() }),
            )
            .await;
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
    };
    while let Some(message) = subscription.next().await {
        state
            .metrics
            .nats_messages_total
            .fetch_add(1, Ordering::Relaxed);
        let payload = message.payload.to_vec();
        if payload.len() > MAX_NATS_PAYLOAD_BYTES {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            state
                .metrics
                .nats_payload_rejected_total
                .fetch_add(1, Ordering::Relaxed);
            publish_runtime_critical_event(
                &state,
                "escrow-nats-payload-too-large",
                "Escrow service rejected an oversized NATS validation request.",
                json!({
                    "payloadBytes": payload.len(),
                    "maxPayloadBytes": MAX_NATS_PAYLOAD_BYTES,
                }),
            )
            .await;
            continue;
        }
        match serde_json::from_slice::<EscrowIntentRequest>(&payload) {
            Ok(request) => {
                state
                    .metrics
                    .validations_total
                    .fetch_add(1, Ordering::Relaxed);
                let request_id = request_id(request.request_id.as_ref(), "escrow-validation");
                let result = match validate_escrow_intent(
                    &request,
                    &state.default_cluster,
                    &state.allowed_program_ids,
                ) {
                    Ok(response) => {
                        json!({
                            "messageKind": "solana.escrow.validation.result",
                            "source": SERVICE_NAME,
                            "result": response
                        })
                    }
                    Err(errors) => {
                        state
                            .metrics
                            .validation_errors_total
                            .fetch_add(1, Ordering::Relaxed);
                        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                        json!({
                            "messageKind": "solana.escrow.validation.result",
                            "source": SERVICE_NAME,
                            "result": {
                                "ok": false,
                                "requestId": request_id,
                                "errors": errors,
                                "generatedAtMs": now_ms()
                            }
                        })
                    }
                };
                let ok = result
                    .pointer("/result/ok")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                publish_validation_result(&state, result).await;
                publish_escrow_event(&state, "solana.escrow.validation", &request_id, ok).await;
            }
            Err(error) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                state
                    .metrics
                    .nats_payload_rejected_total
                    .fetch_add(1, Ordering::Relaxed);
                publish_runtime_critical_event(
                    &state,
                    "escrow-nats-payload-invalid",
                    "Escrow service rejected an invalid NATS validation request.",
                    json!({ "error": error.to_string() }),
                )
                .await;
            }
        }
    }
    log_warn(
        "escrow-nats-subscription-ended",
        "Escrow validation NATS subscription ended; re-subscribing in 5s.",
        json!({
            "subject": state.validate_subject,
            "queueGroup": ESCROW_SOLANA_VALIDATE_QUEUE_GROUP,
        }),
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
