use std::{env, sync::atomic::Ordering, time::Duration};

use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::forecast::*;
use crate::shared::*;
use crate::state::*;
use crate::types::*;

pub(crate) async fn publish_forecast(state: &AppState, response: &ForecastResponse) {
    let Some(nats) = state.nats.as_ref() else {
        return;
    };
    let payload = match serde_json::to_vec(&json!({
        "messageKind": "economics.forecast.result",
        "source": SERVICE_NAME,
        "result": response,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            emit_log(
                "ERROR",
                "economics.forecast.result.encode.error",
                "failed to encode economics forecast result",
                json!({
                    "error": error_summary(&error.to_string()),
                    "requestId": &response.request_id
                }),
            );
            return;
        }
    };
    if nats
        .publish(state.config.result_subject.clone(), payload.into())
        .await
        .is_ok()
    {
        state
            .metrics
            .nats_published_total
            .fetch_add(1, Ordering::Relaxed);
    }
    let _ = nats
        .publish(
            state.config.runtime_event_subject.clone(),
            json!({
                "type": "economics.forecast",
                "source": SERVICE_NAME,
                "requestId": response.request_id,
                "projectionCount": response.projections.len(),
                "scenario": response.scenario,
                "atMs": now_ms()
            })
            .to_string()
            .into(),
        )
        .await;
}

pub(crate) async fn publish_market_event(state: &AppState, event: Value) {
    let Some(nats) = state.nats.as_ref() else {
        return;
    };
    let _ = nats
        .publish(
            state.config.market_event_subject.clone(),
            event.to_string().into(),
        )
        .await;
}

pub(crate) async fn run_nats_loop(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        emit_log(
            "INFO",
            "economics.nats.loop.disabled",
            "economics NATS loop disabled",
            json!({
                "reason": "NATS_URL is not configured"
            }),
        );
        return;
    };
    emit_log(
        "INFO",
        "economics.nats.loop.start",
        "economics NATS loop starting",
        json!({
            "requestSubject": state.config.request_subject,
            "queueGroup": state.config.queue_group,
            "resultSubject": state.config.result_subject
        }),
    );
    // Bound in-flight forecast handlers. Previously every message was
    // tokio::spawn'ed with no ceiling, so a burst could spawn unbounded
    // concurrent forecasts; acquiring a permit before spawning also backpressures
    // the subscription instead of piling work on.
    let max_concurrency = env::var("ECONOMICS_NATS_MAX_CONCURRENCY")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(64);
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(max_concurrency));
    loop {
        let mut subscription = match nats
            .queue_subscribe(
                state.config.request_subject.clone(),
                state.config.queue_group.clone(),
            )
            .await
        {
            Ok(subscription) => subscription,
            Err(error) => {
                emit_log(
                    "ERROR",
                    "economics.nats.subscribe.error",
                    "economics NATS subscribe failed; retrying in 5s",
                    json!({
                        "error": error_summary(&error.to_string()),
                        "requestSubject": state.config.request_subject,
                        "queueGroup": state.config.queue_group
                    }),
                );
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
                emit_log(
                    "WARN",
                    "economics.nats.request.oversize",
                    "economics NATS forecast request rejected because payload is too large",
                    json!({
                        "bytes": payload.len(),
                        "maxBytes": MAX_NATS_PAYLOAD_BYTES
                    }),
                );
                continue;
            }
            // Backpressure point: wait for a permit before spawning more work.
            let permit = match semaphore.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => break,
            };
            let task_state = state.clone();
            tokio::spawn(async move {
                let _permit = permit; // released when this handler finishes
                match serde_json::from_slice::<ForecastRequest>(&payload) {
                    Ok(request) => match forecast_from_request(&task_state, request) {
                        Ok(response) => {
                            task_state
                                .metrics
                                .forecasts_total
                                .fetch_add(1, Ordering::Relaxed);
                            publish_forecast(&task_state, &response).await;
                        }
                        Err(error) => {
                            task_state
                                .metrics
                                .errors_total
                                .fetch_add(1, Ordering::Relaxed);
                            emit_log(
                                "ERROR",
                                "economics.nats.forecast.error",
                                "economics NATS forecast failed",
                                json!({
                                    "error": error_summary(&error)
                                }),
                            );
                        }
                    },
                    Err(error) => {
                        task_state
                            .metrics
                            .errors_total
                            .fetch_add(1, Ordering::Relaxed);
                        emit_log(
                            "WARN",
                            "economics.nats.request.invalid",
                            "economics NATS forecast request was invalid JSON",
                            json!({
                                "error": error_summary(&error.to_string())
                            }),
                        );
                    }
                }
            });
        }
        emit_log(
            "WARN",
            "economics.nats.subscription.ended",
            "economics NATS subscription ended; re-subscribing in 5s",
            json!({
                "requestSubject": state.config.request_subject,
                "queueGroup": state.config.queue_group
            }),
        );
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
