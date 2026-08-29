use std::time::Duration;

use dd_nats_subject_defs::cdc_table_filter_subject;
use serde::Serialize;
use serde_json::{json, Value};

use crate::db::persist_agent_event_to_postgres;
use crate::shared::{
    cdc_stream_name, env_u64, nats_event_subject, nats_git_repos_changes_subject,
    nats_lambda_functions_subject, nats_task_stream_name, nats_task_stream_subject, nats_url,
    now_ms, rest_status_gleam_broadcast_secret, rest_status_gleam_broadcast_url,
    rest_status_rust_broadcast_secret, rest_status_rust_broadcast_url,
};
use crate::types::AgentEventIngestRequest;

pub(crate) fn task_event_payload(
    thread_id: &str,
    task_id: &str,
    seq: impl Serialize,
    message_id: &str,
    event: &Value,
) -> Value {
    json!({
        "type": "task-event",
        "messageId": message_id,
        "threadId": thread_id,
        "taskId": task_id,
        "seq": seq,
        "event": event,
        "emittedAt": now_ms()
    })
}

pub(crate) fn task_event_message_id(task_id: &str, seq: i32, event: &Value) -> String {
    task_event_message_id_i64(task_id, seq as i64, event)
}

pub(crate) fn task_event_message_id_i64(task_id: &str, seq: i64, event: &Value) -> String {
    event
        .get("stage")
        .and_then(Value::as_str)
        .filter(|stage| !stage.trim().is_empty())
        .map(|stage| format!("{task_id}:{stage}"))
        .unwrap_or_else(|| format!("{task_id}:event:{seq}"))
}

pub(crate) fn cdc_column_string(change: &dd_wal_consumer::RowChange, name: &str) -> Option<String> {
    change
        .column(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}

pub(crate) fn cdc_column_i64(change: &dd_wal_consumer::RowChange, name: &str) -> Option<i64> {
    change.column(name).and_then(Value::as_i64)
}

pub(crate) fn task_event_payload_from_agent_event_change(
    change: &dd_wal_consumer::RowChange,
) -> Option<Value> {
    if matches!(change.op, dd_wal_consumer::ChangeOp::Delete) {
        return None;
    }
    let task_id = cdc_column_string(change, "task_id")?;
    let event = change
        .column("payload")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({ "kind": "cdc", "source": "wal-gateway" }));
    let thread_id = cdc_column_string(change, "thread_id").or_else(|| {
        event
            .get("threadId")
            .and_then(Value::as_str)
            .map(str::to_string)
    })?;
    let seq = cdc_column_i64(change, "seq").unwrap_or_default();
    let message_id = task_event_message_id_i64(&task_id, seq, &event);
    Some(task_event_payload(
        &thread_id,
        &task_id,
        seq,
        &message_id,
        &event,
    ))
}

pub(crate) async fn post_task_event_to_websocket_fanout(
    client: &reqwest::Client,
    name: &str,
    url: &str,
    secret: &str,
    payload: &Value,
) -> Result<(), String> {
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-dd-internal-auth", secret)
        .json(payload)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Err(format!(
            "{name} websocket fanout failed with {status}: {}",
            body.chars().take(300).collect::<String>()
        ))
    }
}

pub(crate) async fn publish_task_event_to_websocket_fanout(
    thread_id: &str,
    task_id: &str,
    seq: impl Serialize,
    message_id: &str,
    event: &Value,
) {
    let payload = task_event_payload(thread_id, task_id, seq, message_id, event);
    let timeout_ms = env_u64("REST_STATUS_WS_FANOUT_TIMEOUT_MS", 900);
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::error!("failed to build websocket fanout client: {error}");
            return;
        }
    };

    if let Some(secret) = rest_status_gleam_broadcast_secret() {
        if let Err(error) = post_task_event_to_websocket_fanout(
            &client,
            "gleam",
            &rest_status_gleam_broadcast_url(),
            &secret,
            &payload,
        )
        .await
        {
            tracing::error!("failed to publish task event to gleam websocket fanout: {error}");
        }
    }

    if let Some(secret) = rest_status_rust_broadcast_secret() {
        if let Err(error) = post_task_event_to_websocket_fanout(
            &client,
            "rust",
            &rest_status_rust_broadcast_url(),
            &secret,
            &payload,
        )
        .await
        {
            tracing::error!("failed to publish task event to rust websocket fanout: {error}");
        }
    }
}

pub(crate) async fn publish_thread_runtime_event_to_nats(
    thread_id: &str,
    task_id: Option<&str>,
    action: &str,
    status: &str,
    message: &str,
) -> Result<(), String> {
    let event_task_id = task_id.unwrap_or(thread_id);
    let now = now_ms();
    let payload = json!({
        "type": "task-event",
        "threadId": thread_id,
        "taskId": event_task_id,
        "seq": now,
        "event": {
            "kind": "thread-runtime",
            "action": action,
            "status": status,
            "message": message,
            "atMs": now
        }
    });
    publish_task_event_to_websocket_fanout(
        thread_id,
        event_task_id,
        now,
        &format!("{event_task_id}:thread-runtime:{action}:{status}"),
        &payload["event"],
    )
    .await;
    let body = serde_json::to_vec(&payload).map_err(|error| error.to_string())?;
    let client = async_nats::connect(nats_url())
        .await
        .map_err(|error| error.to_string())?;
    client
        .publish(nats_event_subject(), body.into())
        .await
        .map_err(|error| error.to_string())?;
    client.flush().await.map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) async fn persist_task_status_event(
    thread_id: Option<&str>,
    task_id: &str,
    seq: i32,
    status: &str,
    message: &str,
    mut event: Value,
) -> Result<Value, String> {
    let Some(event_object) = event.as_object_mut() else {
        return Err("status event payload must be a JSON object".to_string());
    };
    event_object.insert("kind".to_string(), json!("status"));
    event_object.insert("status".to_string(), json!(status));
    event_object.insert("message".to_string(), json!(message));
    event_object.insert("atMs".to_string(), json!(now_ms()));
    let request = AgentEventIngestRequest {
        task_id: task_id.to_string(),
        thread_id: thread_id.map(str::to_string),
        seq,
        event,
    };
    persist_agent_event_to_postgres(&request, "status").await?;
    Ok(request.event)
}

pub(crate) async fn publish_task_event_to_nats(
    client: &async_nats::Client,
    thread_id: &str,
    task_id: &str,
    seq: i32,
    message_id: &str,
    event: &Value,
) -> Result<(), String> {
    let payload = task_event_payload(thread_id, task_id, seq, message_id, event);
    let body = serde_json::to_vec(&payload).map_err(|error| error.to_string())?;
    client
        .publish(nats_event_subject(), body.into())
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) async fn ensure_nats_task_stream(jetstream: &async_nats::jetstream::Context) -> Result<(), String> {
    jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: nats_task_stream_name(),
            subjects: vec![nats_task_stream_subject()],
            retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
            max_age: Duration::from_secs(60 * 60 * 24 * 14),
            max_message_size: 8 * 1024 * 1024,
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) async fn jetstream_publish_task(
    client: async_nats::Client,
    subject: String,
    payload: Vec<u8>,
) -> Result<(), String> {
    let jetstream = async_nats::jetstream::new(client);
    ensure_nats_task_stream(&jetstream).await?;
    jetstream
        .publish(subject, payload.into())
        .await
        .map_err(|error| error.to_string())?
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Run the WAL-gateway CDC fan-out subscriptions. We turn row changes on
/// `lambda_functions` and `known_git_repos` into NATS messages on the same
/// subjects the REST handlers already publish to, so downstream consumers
/// (e.g. `gleam-lambda-runner`) see every change regardless of whether the
/// row was written through this service or via direct SQL / another service.
///
/// Why we keep the direct publish too: the REST handler still publishes
/// immediately so the originating client gets sub-100ms feedback. The CDC
/// path is the catch-net for everything else. Duplicate publishes are
/// harmless — the consumer treats lambda updates as idempotent.
pub(crate) async fn run_cdc_fanout_subscriptions() {
    let nats = match async_nats::connect(nats_url()).await {
        Ok(client) => client,
        Err(error) => {
            tracing::error!("dd-remote-rest-api cdc fanout disabled: nats connect failed: {error}");
            return;
        }
    };
    let jetstream = async_nats::jetstream::new(nats.clone());
    let stream = cdc_stream_name();

    // lambda_functions → dd.remote.lambdas.functions
    {
        let nats_for_handler = nats.clone();
        let durable = "dd-remote-rest-api-lambdas".to_string();
        let result = dd_wal_consumer::Subscription::builder()
            .stream(stream.clone())
            .durable_name(durable.clone())
            .filter_subject(cdc_table_filter_subject(
                "cdc",
                "public",
                "lambda_functions",
            ))
            .start(&jetstream, move |change: dd_wal_consumer::RowChange| {
                let nats = nats_for_handler.clone();
                async move {
                    let payload = json!({
                        "version": 1,
                        "messageKind": "lambda.function.updated",
                        "source": "wal-gateway",
                        "action": change.op.as_str(),
                        "functionId": change.column("id").cloned(),
                        "slug": change.column("slug").cloned(),
                        "status": change.column("status").cloned(),
                        "lsn": change.lsn,
                        "tsMs": change.ts_ms,
                    });
                    let bytes = match serde_json::to_vec(&payload) {
                        Ok(b) => b,
                        Err(error) => {
                            tracing::error!("cdc lambda fanout encode failed: {error}");
                            return;
                        }
                    };
                    if let Err(error) = nats
                        .publish(nats_lambda_functions_subject(), bytes.into())
                        .await
                    {
                        tracing::error!("cdc lambda fanout publish failed: {error}");
                    }
                }
            })
            .await;
        match result {
            Ok(_) => tracing::info!(
                "rest-api cdc subscription started: durable={durable} \
                 subject=cdc.public.lambda_functions.> -> {}",
                nats_lambda_functions_subject()
            ),
            Err(error) => {
                tracing::error!("rest-api cdc lambda subscription failed to start: {error}")
            }
        }
    }

    // known_git_repos → dd.remote.git-repos.changes
    {
        let nats_for_handler = nats.clone();
        let durable = "dd-remote-rest-api-git-repos".to_string();
        let result = dd_wal_consumer::Subscription::builder()
            .stream(stream.clone())
            .durable_name(durable.clone())
            .filter_subject(cdc_table_filter_subject("cdc", "public", "known_git_repos"))
            .start(&jetstream, move |change: dd_wal_consumer::RowChange| {
                let nats = nats_for_handler.clone();
                async move {
                    let payload = json!({
                        "version": 1,
                        "messageKind": "git-repo.changed",
                        "source": "wal-gateway",
                        "action": change.op.as_str(),
                        "repoId": change.column("id").cloned(),
                        "repoUrl": change.column("repo_url").cloned(),
                        "status": change.column("status").cloned(),
                        "lsn": change.lsn,
                        "tsMs": change.ts_ms,
                    });
                    let bytes = match serde_json::to_vec(&payload) {
                        Ok(b) => b,
                        Err(error) => {
                            tracing::error!("cdc git-repo fanout encode failed: {error}");
                            return;
                        }
                    };
                    if let Err(error) = nats
                        .publish(nats_git_repos_changes_subject(), bytes.into())
                        .await
                    {
                        tracing::error!("cdc git-repo fanout publish failed: {error}");
                    }
                }
            })
            .await;
        match result {
            Ok(_) => tracing::info!(
                "rest-api cdc subscription started: durable={durable} \
                 subject=cdc.public.known_git_repos.> -> {}",
                nats_git_repos_changes_subject()
            ),
            Err(error) => {
                tracing::error!("rest-api cdc git-repo subscription failed to start: {error}")
            }
        }
    }

    // agent_remote_dev_events → dd.remote.events
    //
    // This is the WAL-derived catch-net for runtime task status. The normal
    // ingest path still direct-fans out to the websocket services for latency,
    // but any event committed to Postgres is also replayed through the same
    // NATS subject consumed by the Gleam and Rust websocket fanout paths.
    {
        let nats_for_handler = nats.clone();
        let durable = "dd-remote-rest-api-agent-events".to_string();
        let result = dd_wal_consumer::Subscription::builder()
            .stream(stream.clone())
            .durable_name(durable.clone())
            .filter_subject(cdc_table_filter_subject(
                "cdc",
                "public",
                "agent_remote_dev_events",
            ))
            .start(&jetstream, move |change: dd_wal_consumer::RowChange| {
                let nats = nats_for_handler.clone();
                async move {
                    let Some(payload) = task_event_payload_from_agent_event_change(&change) else {
                        if !matches!(change.op, dd_wal_consumer::ChangeOp::Delete) {
                            tracing::error!(
                                "cdc agent-event fanout skipped malformed row: lsn={}",
                                change.lsn
                            );
                        }
                        return;
                    };
                    let bytes = match serde_json::to_vec(&payload) {
                        Ok(b) => b,
                        Err(error) => {
                            tracing::error!("cdc agent-event fanout encode failed: {error}");
                            return;
                        }
                    };
                    if let Err(error) = nats.publish(nats_event_subject(), bytes.into()).await {
                        tracing::error!("cdc agent-event fanout publish failed: {error}");
                    }
                }
            })
            .await;
        match result {
            Ok(_) => tracing::info!(
                "rest-api cdc subscription started: durable={durable} \
                 subject=cdc.public.agent_remote_dev_events.> -> {}",
                nats_event_subject()
            ),
            Err(error) => {
                tracing::error!("rest-api cdc agent-event subscription failed to start: {error}")
            }
        }
    }
}

#[cfg(test)]
mod cdc_tests {
    use super::*;

    #[test]
    fn agent_event_cdc_row_builds_websocket_task_event() {
        let thread_id = "11111111-1111-1111-1111-111111111111";
        let task_id = "22222222-2222-2222-2222-222222222222";
        let change = dd_wal_consumer::RowChange {
            schema_version: dd_wal_consumer::SCHEMA_VERSION.to_string(),
            schema: "public".to_string(),
            table: "agent_remote_dev_events".to_string(),
            op: dd_wal_consumer::ChangeOp::Insert,
            lsn: "0/1A3B5C0".to_string(),
            xid: Some(123),
            ts_ms: 1_736_000_000_000,
            source_timestamp: None,
            primary_key: vec!["id".to_string()],
            row: json!({
                "id": 42,
                "task_id": task_id,
                "thread_id": thread_id,
                "seq": 7,
                "event_kind": "status",
                "payload": {
                    "kind": "status",
                    "stage": "worker-ready",
                    "message": "ready"
                }
            }),
            previous_row: None,
        };

        let payload =
            task_event_payload_from_agent_event_change(&change).expect("payload from cdc row");

        assert_eq!(
            payload.get("type").and_then(Value::as_str),
            Some("task-event")
        );
        assert_eq!(
            payload.get("threadId").and_then(Value::as_str),
            Some(thread_id)
        );
        assert_eq!(payload.get("taskId").and_then(Value::as_str), Some(task_id));
        assert_eq!(payload.get("seq").and_then(Value::as_i64), Some(7));
        assert_eq!(
            payload.get("messageId").and_then(Value::as_str),
            Some("22222222-2222-2222-2222-222222222222:worker-ready")
        );
        assert_eq!(
            payload
                .get("event")
                .and_then(|event| event.get("stage"))
                .and_then(Value::as_str),
            Some("worker-ready")
        );
    }

    #[test]
    fn agent_event_cdc_delete_is_not_fanned_out() {
        let change = dd_wal_consumer::RowChange {
            schema_version: dd_wal_consumer::SCHEMA_VERSION.to_string(),
            schema: "public".to_string(),
            table: "agent_remote_dev_events".to_string(),
            op: dd_wal_consumer::ChangeOp::Delete,
            lsn: "0/1A3B5C1".to_string(),
            xid: Some(124),
            ts_ms: 1_736_000_000_001,
            source_timestamp: None,
            primary_key: vec!["id".to_string()],
            row: json!({
                "id": 42,
                "task_id": "22222222-2222-2222-2222-222222222222",
                "thread_id": "11111111-1111-1111-1111-111111111111",
                "seq": 7,
                "payload": { "kind": "status" }
            }),
            previous_row: None,
        };

        assert!(task_event_payload_from_agent_event_change(&change).is_none());
    }
}
