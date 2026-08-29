use std::{sync::atomic::Ordering, time::Duration};

use dd_nats_subject_defs::cdc_table_filter_subject;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::{
    dispatch::{dispatch_to_pool, pool_selector_from_request},
    lifecycle::reconcile_all,
    pool_config::{record_config_error, refresh_pool_configs},
    types::{AppState, DispatchRequest},
    util::{env_value, now_ms},
};

pub(crate) async fn run_config_refresh_loop(state: AppState) {
    loop {
        if let Err(error) = refresh_pool_configs(&state).await {
            tracing::error!("container pool config refresh failed: {error}");
            record_config_error(&state, error).await;
        }
        reconcile_all(&state).await;
        sleep(state.config.config_refresh).await;
    }
}

pub(crate) async fn run_reconcile_loop(state: AppState) {
    loop {
        reconcile_all(&state).await;
        sleep(state.config.reconcile_interval).await;
    }
}

/// Subscribe to the WAL gateway and refresh the pool registry whenever:
///   * the `app_config` row this server reads from changes (scope/key match), or
///   * any row in `container_pool_configs` changes.
///
/// We don't try to be surgical (partial-apply just the changed pool). The
/// existing `refresh_pool_configs` is cheap enough that a full reload is
/// the simplest correct thing — the registry mutex already serializes
/// readers against the swap.
///
/// The poll loop is still on as the fallback path. The CDC subscription
/// just shortens the perceived edit-to-effect latency from O(refresh_secs)
/// to O(WAL gateway poll interval) ≈ a few hundred ms.
pub(crate) async fn run_cdc_refresh_subscription(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        tracing::info!("container pool cdc subscription disabled: no NATS_URL configured");
        return;
    };
    let jetstream = async_nats::jetstream::new(nats);
    let app_config_scope = state.config.app_config_scope.clone();
    let app_config_key = state.config.app_config_key.clone();
    let stream_name = env_value("CONTAINER_POOL_CDC_STREAM", "CDC");

    // Subscription 1 — app_config (filtered to the row we read from).
    {
        let task_state = state.clone();
        let scope = app_config_scope.clone();
        let key = app_config_key.clone();
        let durable = format!(
            "dd-container-pool-app-config-{}",
            cdc_sanitize(&format!("{scope}.{key}"))
        );
        let app_config_filter = cdc_table_filter_subject("cdc", "public", "app_config");
        let result = dd_wal_consumer::Subscription::builder()
            .stream(stream_name.clone())
            .durable_name(durable.clone())
            .filter_subject(app_config_filter.clone())
            .start(&jetstream, move |change: dd_wal_consumer::RowChange| {
                let task_state = task_state.clone();
                let scope = scope.clone();
                let key = key.clone();
                async move {
                    let row_scope = change.column("scope").and_then(Value::as_str);
                    let row_key = change.column("key").and_then(Value::as_str);
                    if row_scope != Some(scope.as_str()) || row_key != Some(key.as_str()) {
                        return;
                    }
                    cdc_trigger_refresh(&task_state, "app_config").await;
                }
            })
            .await;
        log_cdc_subscription_result(&durable, &app_config_filter, result);
    }

    // Subscription 2 — container_pool_configs (no row filter, every change
    // touches the registry).
    {
        let task_state = state.clone();
        let durable = "dd-container-pool-table".to_string();
        let table_filter = cdc_table_filter_subject("cdc", "public", "container_pool_configs");
        let result = dd_wal_consumer::Subscription::builder()
            .stream(stream_name.clone())
            .durable_name(durable.clone())
            .filter_subject(table_filter.clone())
            .start(&jetstream, move |_change: dd_wal_consumer::RowChange| {
                let task_state = task_state.clone();
                async move {
                    cdc_trigger_refresh(&task_state, "container_pool_configs").await;
                }
            })
            .await;
        log_cdc_subscription_result(&durable, &table_filter, result);
    }
}

async fn cdc_trigger_refresh(state: &AppState, source: &str) {
    if let Err(error) = refresh_pool_configs(state).await {
        tracing::error!("container pool CDC-driven refresh failed ({source}): {error}");
        record_config_error(state, error).await;
        return;
    }
    // Trigger a reconcile too so containers actually warm/cool in line
    // with the new config without waiting for the regular reconcile tick.
    reconcile_all(state).await;
}

fn cdc_sanitize(input: &str) -> String {
    input
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn log_cdc_subscription_result(
    durable: &str,
    subject: &str,
    result: Result<tokio::task::JoinHandle<()>, dd_wal_consumer::Error>,
) {
    match result {
        Ok(_join) => {
            tracing::info!(
                "container pool cdc subscription started: durable={durable} subject={subject}"
            );
        }
        Err(error) => {
            tracing::error!(
                "container pool cdc subscription failed to start ({error}); \
                 falling back to poll-only refresh for {subject}"
            );
        }
    }
}

pub(crate) async fn run_nats_loop(state: AppState) {
    let Some(client) = state.nats.clone() else {
        return;
    };
    loop {
        let mut subscriber = match client
            .queue_subscribe(
                state.config.nats_subject.clone(),
                state.config.nats_queue_group.clone(),
            )
            .await
        {
            Ok(subscriber) => subscriber,
            Err(error) => {
                tracing::error!("container pool nats subscribe failed: {error}; retrying in 5s");
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        while let Some(message) = subscriber.next().await {
            state
                .metrics
                .nats_messages_total
                .fetch_add(1, Ordering::Relaxed);
            if message.payload.len() > state.config.nats_max_payload_bytes {
                state
                    .metrics
                    .nats_failures_total
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let request = match serde_json::from_slice::<DispatchRequest>(&message.payload) {
                Ok(request) => request,
                Err(error) => {
                    tracing::error!("container pool invalid nats request: {error}");
                    state
                        .metrics
                        .nats_failures_total
                        .fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };
            let selector = {
                let registry = state.registry.lock().await;
                pool_selector_from_request(&request, Some(message.subject.as_ref()), &registry)
            };
            let Some(selector) = selector else {
                tracing::error!("container pool nats request missing pool selector");
                state
                    .metrics
                    .nats_failures_total
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            };
            let response = match dispatch_to_pool(&state, &selector, request).await {
                Ok(response) => json!(response),
                Err(error) => {
                    state
                        .metrics
                        .nats_failures_total
                        .fetch_add(1, Ordering::Relaxed);
                    json!({ "ok": false, "error": error, "generatedAtMs": now_ms() })
                }
            };
            let Ok(payload) = serde_json::to_vec(&response) else {
                continue;
            };
            if let Some(reply) = message.reply {
                if let Err(error) = client.publish(reply, payload.into()).await {
                    tracing::error!("container pool nats reply failed: {error}");
                }
            } else if let Err(error) = client
                .publish(state.config.nats_result_subject.clone(), payload.into())
                .await
            {
                tracing::error!("container pool nats result publish failed: {error}");
            }
        }
        tracing::error!(
            "container pool nats subscription ended (subject={}); re-subscribing in 5s",
            state.config.nats_subject
        );
        sleep(Duration::from_secs(5)).await;
    }
}
