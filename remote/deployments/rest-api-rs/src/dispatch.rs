use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use dd_shared_interfaces::AgentTaskQueueMessage;
use serde_json::{json, Value};

use crate::context::fetch_selected_agent_context_from_postgres;
use crate::db::{
    fetch_existing_task_dispatch_from_postgres, fetch_thread_repo_config_from_postgres,
    persist_runtime_task_to_postgres,
};
use crate::events::{
    jetstream_publish_task, persist_task_status_event, publish_task_event_to_nats,
    publish_task_event_to_websocket_fanout,
};
use crate::metrics::record_request;
use crate::shared::{
    first_env, json_string, missing_worker_auth_secret_message, nats_task_subject, nats_url,
    nats_wakeup_subject, normalize_context_mode, normalized_repo_config, now_ms,
    postgres_database_url, public_data_source_error, public_thread_worker_proxy_error,
    worker_auth_secret,
};
use crate::state::remember_runtime_task;
use crate::threads::{ensure_thread_worker, thread_worker_url, wait_thread_worker_ready};
use crate::types::DispatchTaskRequest;

pub(crate) async fn publish_task_dispatch_to_nats(
    request: &DispatchTaskRequest,
    branch: Option<&str>,
) -> Result<(), String> {
    publish_task_to_nats(request, branch, "task.dispatch", false).await
}

pub(crate) fn default_dispatch_mode() -> String {
    first_env(&[
        "REST_API_DEFAULT_DISPATCH_MODE",
        "REMOTE_REST_DEFAULT_DISPATCH_MODE",
    ])
    .unwrap_or_else(|| "queued".to_string())
}

pub(crate) fn dispatch_mode_value(request: &DispatchTaskRequest) -> String {
    request
        .dispatch_mode
        .as_deref()
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(default_dispatch_mode)
}

pub(crate) fn is_queued_dispatch_mode(mode: &str) -> bool {
    matches!(
        mode,
        "queued" | "nats" | "async" | "queued-pool" | "nats-pool" | "container-pool" | "pool"
    )
}

pub(crate) fn is_container_pool_dispatch_mode(mode: &str) -> bool {
    matches!(
        mode,
        "queued-pool" | "nats-pool" | "container-pool" | "pool"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DispatchPath {
    NatsQueue { container_pool: bool },
    DirectWorker,
}

pub(crate) fn dispatch_path_for_mode(mode: &str) -> DispatchPath {
    if is_queued_dispatch_mode(mode) {
        DispatchPath::NatsQueue {
            container_pool: is_container_pool_dispatch_mode(mode),
        }
    } else {
        DispatchPath::DirectWorker
    }
}

pub(crate) async fn publish_task_to_nats(
    request: &DispatchTaskRequest,
    branch: Option<&str>,
    message_kind: &'static str,
    shadow: bool,
) -> Result<(), String> {
    let repo_config = normalized_repo_config(request)?;
    let dispatch_mode = dispatch_mode_value(request);
    let container_pool_dispatch = is_container_pool_dispatch_mode(&dispatch_mode);
    let message = AgentTaskQueueMessage {
        version: Some(1),
        message_kind: Some(message_kind.to_string()),
        task_kind: Some("agent.prompt".to_string()),
        shadow: Some(shadow),
        direct_dispatch: Some(false),
        dispatch_mode: Some(dispatch_mode),
        container_pool_dispatch: Some(container_pool_dispatch),
        thread_id: request.thread_id.clone(),
        task_id: request.task_id.clone(),
        provider: request.provider.clone(),
        repo: Some(repo_config.repo),
        base_branch: Some(repo_config.base_branch),
        feature_branch: branch.map(str::to_string),
        prompt: Some(request.prompt.clone()),
        thread_title: repo_config.thread_title.clone(),
        context_mode: Some(normalize_context_mode(
            request.context_mode.as_deref(),
            request.context_ids.as_ref().map_or(0, Vec::len),
        )),
        context_ids: request.context_ids.clone(),
        created_at_ms: Some(now_ms() as i64),
    };
    let payload = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
    let client = async_nats::connect(nats_url())
        .await
        .map_err(|error| error.to_string())?;
    let task_subject = nats_task_subject(&request.thread_id);

    jetstream_publish_task(client.clone(), task_subject, payload.clone()).await?;
    client
        .publish(nats_wakeup_subject(), payload.into())
        .await
        .map_err(|error| error.to_string())?;
    let status_event = persist_task_status_event(
        Some(&request.thread_id),
        &request.task_id,
        -950,
        "nats queued",
        "REST API published the task to JetStream and emitted the orchestrator wakeup.",
        json!({
            "source": "dd-remote-rest-api",
            "stage": "nats-published",
            "messageKind": message_kind,
            "shadow": shadow,
            "directDispatch": false,
            "dispatchMode": &message.dispatch_mode,
            "containerPoolDispatch": message.container_pool_dispatch,
            "subject": nats_task_subject(&request.thread_id),
            "wakeupSubject": nats_wakeup_subject(),
        }),
    )
    .await;
    match status_event {
        Ok(event) => {
            let message_id = format!("{}:{message_kind}:nats-published", request.task_id);
            publish_task_event_to_websocket_fanout(
                &request.thread_id,
                &request.task_id,
                -950,
                &message_id,
                &event,
            )
            .await;
            if let Err(error) = publish_task_event_to_nats(
                &client,
                &request.thread_id,
                &request.task_id,
                -950,
                &message_id,
                &event,
            )
            .await
            {
                tracing::error!("failed to publish task handoff event to nats: {error}");
            }
        }
        Err(error) => tracing::error!("failed to persist task handoff event: {error}"),
    }
    client.flush().await.map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) async fn dispatch_thread_task(
    Path(thread_id): Path<String>,
    Json(request): Json<DispatchTaskRequest>,
) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/tasks",
        StatusCode::OK,
    );
    if request.thread_id != thread_id {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "threadId path/body mismatch" })),
        )
            .into_response();
    }
    let mut repo_config = match normalized_repo_config(&request) {
        Ok(repo_config) => repo_config,
        Err(error) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
        }
    };
    if postgres_database_url().is_some() {
        match fetch_thread_repo_config_from_postgres(&thread_id).await {
            Ok(Some(stored_config)) => {
                if stored_config.repo != repo_config.repo
                    || stored_config.base_branch != repo_config.base_branch
                {
                    return (
                        StatusCode::CONFLICT,
                        Json(json!({
                            "error": "thread already exists with a different repo or baseBranch"
                        })),
                    )
                        .into_response();
                }
                repo_config = stored_config;
            }
            Ok(None) => {}
            Err(error) => {
                tracing::error!("failed to fetch thread repo config before dispatch: {error}")
            }
        }
        match fetch_existing_task_dispatch_from_postgres(&request.task_id).await {
            Ok(Some(existing)) => {
                if existing.thread_id != thread_id {
                    return (
                        StatusCode::CONFLICT,
                        Json(json!({
                            "error": "taskId already belongs to a different thread",
                            "threadId": &thread_id,
                            "existingThreadId": existing.thread_id,
                            "taskId": &request.task_id,
                        })),
                    )
                        .into_response();
                }
                if existing.prompt != request.prompt {
                    return (
                        StatusCode::CONFLICT,
                        Json(json!({
                            "error": "taskId already exists; generate a new taskId for follow-up tasks",
                            "threadId": &thread_id,
                            "taskId": &request.task_id,
                        })),
                    )
                        .into_response();
                }
            }
            Ok(None) => {}
            Err(error) => tracing::error!("failed to check existing task before dispatch: {error}"),
        }
    }

    let dispatch_mode = dispatch_mode_value(&request);
    let dispatch_path = dispatch_path_for_mode(&dispatch_mode);
    let (queued_dispatch, container_pool_dispatch) = match dispatch_path {
        DispatchPath::NatsQueue { container_pool } => (true, container_pool),
        DispatchPath::DirectWorker => (false, false),
    };
    remember_runtime_task(&request, None);
    if let Err(error) = persist_runtime_task_to_postgres(
        &request,
        None,
        if queued_dispatch { "queued" } else { "running" },
    )
    .await
    {
        tracing::error!("failed to persist remote task before worker wake: {error}");
    }
    if queued_dispatch {
        match persist_task_status_event(
            Some(&thread_id),
            &request.task_id,
            -980,
            "queued dispatch accepted",
            "REST API accepted the queued task request and is publishing it to NATS.",
            json!({
                "source": "dd-remote-rest-api",
                "stage": "queued-dispatch-accepted",
                "dispatchMode": &dispatch_mode,
                "requestedDispatchMode": &request.dispatch_mode,
                "subject": nats_task_subject(&thread_id),
            }),
        )
        .await
        {
            Ok(event) => {
                publish_task_event_to_websocket_fanout(
                    &thread_id,
                    &request.task_id,
                    -980,
                    &format!("{}:queued-dispatch-accepted", request.task_id),
                    &event,
                )
                .await;
            }
            Err(error) => {
                tracing::error!("failed to persist queued dispatch accepted event: {error}")
            }
        }
        match publish_task_dispatch_to_nats(&request, None).await {
            Ok(()) => {}
            Err(error) => {
                tracing::error!("failed to publish queued remote task to nats: {error}");
                match persist_task_status_event(
                    Some(&thread_id),
                    &request.task_id,
                    -940,
                    "nats publish failed",
                    "REST API could not publish the queued handoff to NATS.",
                    json!({
                        "source": "dd-remote-rest-api",
                        "stage": "nats-publish-failed",
                        "dispatchMode": &dispatch_mode,
                        "requestedDispatchMode": &request.dispatch_mode,
                        "subject": nats_task_subject(&thread_id),
                        "error": error,
                    }),
                )
                .await
                {
                    Ok(event) => {
                        publish_task_event_to_websocket_fanout(
                            &thread_id,
                            &request.task_id,
                            -940,
                            &format!("{}:nats-publish-failed", request.task_id),
                            &event,
                        )
                        .await;
                    }
                    Err(persist_error) => {
                        tracing::error!(
                            "failed to persist queued dispatch publish failure: {persist_error}"
                        );
                    }
                }
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "error": "failed to publish queued task to nats"
                    })),
                )
                    .into_response();
            }
        }
        return (
            StatusCode::ACCEPTED,
            Json(json!({
                "ok": true,
                "mode": dispatch_mode,
                "queued": true,
                "containerPoolDispatch": container_pool_dispatch,
                "directDispatch": false,
                "subject": nats_task_subject(&thread_id),
                "taskId": &request.task_id,
                "threadId": &thread_id,
            })),
        )
            .into_response();
    }

    let Ok((namespace, name, _results)) = ensure_thread_worker(
        &thread_id,
        &repo_config.repo,
        &repo_config.base_branch,
        repo_config.thread_title.as_deref(),
    )
    .await
    else {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "failed to create or wake thread worker" })),
        )
            .into_response();
    };
    let Some(secret) = worker_auth_secret() else {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": missing_worker_auth_secret_message() })),
        )
            .into_response();
    };
    if let Err(error) = wait_thread_worker_ready(&namespace, &name, &secret).await {
        return (StatusCode::BAD_GATEWAY, Json(json!({ "error": error }))).into_response();
    }

    let selected_context = if postgres_database_url().is_some() {
        match fetch_selected_agent_context_from_postgres(&request, &repo_config).await {
            Ok(items) => items,
            Err(error) => {
                tracing::error!("failed to fetch selected agent context: {error}");
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": public_data_source_error("postgres selected context") })),
                )
                    .into_response();
            }
        }
    } else {
        Vec::new()
    };
    let context_mode = normalize_context_mode(
        request.context_mode.as_deref(),
        request.context_ids.as_ref().map_or(0, Vec::len),
    );
    let worker_body = json!({
        "taskId": &request.task_id,
        "threadId": &request.thread_id,
        "prompt": &request.prompt,
        "provider": &request.provider,
        "threadTitle": &request.thread_title,
        "repo": &repo_config.repo,
        "baseBranch": &repo_config.base_branch,
        "contextMode": context_mode,
        "contextIds": &request.context_ids,
        "contextBlobs": selected_context,
    });
    let client = reqwest::Client::new();
    let response = client
        .post(thread_worker_url(&namespace, &name, "/tasks"))
        .header("X-Server-Auth", secret)
        .json(&worker_body)
        .send()
        .await;
    match response {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if status.is_success() {
                let branch = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|value| json_string(&value, "branch"));
                remember_runtime_task(&request, branch.clone());
                if let Err(error) =
                    persist_runtime_task_to_postgres(&request, branch.as_deref(), "running").await
                {
                    tracing::error!("failed to persist remote task to postgres: {error}");
                }
            }
            let public_status =
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            (
                public_status,
                [(header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response()
        }
        Err(error) => {
            tracing::error!("thread worker dispatch proxy failed: {error}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": public_thread_worker_proxy_error("dispatch") })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod dispatch_path_tests {
    use super::*;

    use crate::shared::constant_time_equals;

    #[test]
    fn image_builder_auth_comparison_matches_only_identical_secrets() {
        assert!(constant_time_equals("same-secret", "same-secret"));
        assert!(!constant_time_equals("same-secret", "same-secreu"));
        assert!(!constant_time_equals("same-secret", "same-secret-longer"));
    }

    #[test]
    fn queued_modes_resolve_to_nats_queue_only() {
        for mode in [
            "queued",
            "nats",
            "async",
            "queued-pool",
            "nats-pool",
            "container-pool",
            "pool",
        ] {
            assert!(matches!(
                dispatch_path_for_mode(mode),
                DispatchPath::NatsQueue { .. }
            ));
        }
    }

    #[test]
    fn container_pool_modes_are_still_nats_queue_modes() {
        for mode in ["queued-pool", "nats-pool", "container-pool", "pool"] {
            assert_eq!(
                dispatch_path_for_mode(mode),
                DispatchPath::NatsQueue {
                    container_pool: true
                }
            );
        }
    }

    #[test]
    fn plain_queued_modes_use_uuid_bound_worker_queue() {
        for mode in ["queued", "nats", "async"] {
            assert_eq!(
                dispatch_path_for_mode(mode),
                DispatchPath::NatsQueue {
                    container_pool: false
                }
            );
        }
    }

    #[test]
    fn direct_modes_skip_the_nats_queue_path() {
        for mode in ["direct", "worker", "sync"] {
            assert_eq!(dispatch_path_for_mode(mode), DispatchPath::DirectWorker);
        }
    }
}
