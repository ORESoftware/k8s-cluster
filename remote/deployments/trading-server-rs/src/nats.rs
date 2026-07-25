use std::{sync::atomic::Ordering, time::Duration};

use futures_util::StreamExt;
use serde_json::json;
use tokio::time::sleep;

use crate::decision::{evaluate_decision, without_intent_credentials};
use crate::platforms::platform_snapshot;
use crate::state::{AppState, MAX_NATS_PAYLOAD_BYTES, SERVICE_NAME};
use crate::types::{DecisionRequest, DecisionResponse};
use crate::util::{env_bool, now_ms, optional_env};

/// Build a hardened NATS client from `NATS_URL` plus optional auth/TLS env.
///
/// The previous code called `async_nats::connect(url)` directly, which has no
/// client name, no auth, and aborts startup if the broker is briefly
/// unreachable. For a trading control-plane service we want: a stable client
/// name for server-side observability, optional credentials/token/nkey, a TLS
/// requirement toggle, and resilience to an initial-connect blip.
///
/// Returns `Ok(None)` when `NATS_URL` is unset (NATS features stay disabled),
/// `Err` only on a hard misconfiguration (e.g. unreadable creds file).
pub(crate) async fn connect_nats() -> Result<Option<async_nats::Client>, String> {
    let Some(url) = optional_env("NATS_URL") else {
        return Ok(None);
    };

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

    let client = options
        .connect(url)
        .await
        .map_err(|error| format!("failed to connect to NATS: {error}"))?;
    Ok(Some(client))
}

pub(crate) async fn publish_decision(state: &AppState, response: &DecisionResponse) {
    let Some(nats) = &state.nats else {
        return;
    };

    // The decisions subject is bridged into telemetry/websocket fanout, so it
    // gets the credential-free view; the full intent (with credential refs)
    // goes only to the order_intents subject below.
    let public_response = without_intent_credentials(response);
    let decision_payload = match serde_json::to_vec(&json!({
        "messageKind": "trading.decision.result",
        "source": SERVICE_NAME,
        "result": &public_response,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::error!("trading server failed to encode decision: {error}");
            return;
        }
    };
    match nats
        .publish(
            state.config.decision_subject.clone(),
            decision_payload.clone().into(),
        )
        .await
    {
        Ok(_) => {
            state
                .metrics
                .nats_published_total
                .fetch_add(1, Ordering::Relaxed);
        }
        Err(error) => tracing::error!("trading server failed to publish decision: {error}"),
    }

    if let Some(order_intent) = response.order_intent.as_ref() {
        let order_payload = match serde_json::to_vec(&json!({
            "messageKind": "trading.order_intent",
            "source": SERVICE_NAME,
            "intent": order_intent,
        })) {
            Ok(payload) => payload,
            Err(error) => {
                tracing::error!("trading server failed to encode order intent: {error}");
                return;
            }
        };
        match nats
            .publish(
                state.config.order_intent_subject.clone(),
                order_payload.clone().into(),
            )
            .await
        {
            Ok(_) => {
                state
                    .metrics
                    .nats_published_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => tracing::error!("trading server failed to publish order intent: {error}"),
        }
    }

    let _ = nats
        .publish(
            state.config.event_subject.clone(),
            json!({
                "type": "trading.decision",
                "source": SERVICE_NAME,
                "requestId": &response.request_id,
                "symbol": &response.symbol,
                "recommendedAction": &response.recommended_action,
                "finalAction": &response.final_action,
                "confidence": response.confidence,
                "riskScore": response.risk_score,
                "mode": &response.mode,
                "atMs": now_ms()
            })
            .to_string()
            .into(),
        )
        .await;
}

pub(crate) async fn run_nats_loop(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        tracing::info!("trading server nats loop disabled: NATS_URL is not configured");
        return;
    };
    tracing::info!(
        "trading server nats loop starting: subject={} queueGroup={} decisionSubject={}",
        state.config.signal_subject, state.config.queue_group, state.config.decision_subject
    );
    'outer: loop {
        let mut subscription = match nats
            .queue_subscribe(
                state.config.signal_subject.clone(),
                state.config.queue_group.clone(),
            )
            .await
        {
            Ok(subscription) => subscription,
            Err(error) => {
                tracing::error!("trading server nats subscribe failed: {error}; retrying in 5s");
                sleep(Duration::from_secs(5)).await;
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
                tracing::error!(
                    "trading server rejected oversize nats signal: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                    payload.len()
                );
                continue;
            }
            // Backpressure: block the receive loop until an inflight slot is
            // free rather than spawning unbounded tasks under a signal flood.
            let permit = match state.inflight.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => break 'outer, // semaphore closed; shutting down
            };
            let task_state = state.clone();
            tokio::spawn(async move {
                let _permit = permit;
                match serde_json::from_slice::<DecisionRequest>(&payload) {
                    Ok(request) => {
                        let platforms = platform_snapshot(&task_state);
                        match evaluate_decision(&task_state.config, &platforms, request) {
                            Ok(response) => {
                                task_state
                                    .metrics
                                    .decisions_total
                                    .fetch_add(1, Ordering::Relaxed);
                                if response.order_intent.is_some() {
                                    task_state
                                        .metrics
                                        .order_intents_total
                                        .fetch_add(1, Ordering::Relaxed);
                                } else if response.recommended_action != response.final_action {
                                    task_state
                                        .metrics
                                        .blocked_orders_total
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                                publish_decision(&task_state, &response).await;
                            }
                            Err(error) => {
                                task_state
                                    .metrics
                                    .errors_total
                                    .fetch_add(1, Ordering::Relaxed);
                                tracing::error!("trading server failed nats decision: {error}");
                            }
                        }
                    }
                    Err(error) => {
                        task_state
                            .metrics
                            .errors_total
                            .fetch_add(1, Ordering::Relaxed);
                        tracing::error!("trading server invalid nats signal: {error}");
                    }
                }
            });
        }
        tracing::error!(
            "trading server nats signal subscription ended; re-subscribing in 5s: subject={} queueGroup={}",
            state.config.signal_subject, state.config.queue_group
        );
        sleep(Duration::from_secs(5)).await;
    }
}
