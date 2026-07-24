use std::collections::HashMap;

use axum::{
    extract::{Path, Query},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::db::{
    fetch_agent_breadcrumb_tail_from_postgres, fetch_agent_events_from_postgres,
    fetch_agents_snapshot, fetch_known_git_repos_from_postgres,
    fetch_thread_context_from_postgres, persist_agent_breadcrumb_to_postgres,
    persist_agent_event_to_postgres, persist_feedback_event_to_postgres,
    upsert_known_git_repo_to_postgres,
};
use crate::events::{publish_task_event_to_websocket_fanout, task_event_message_id};
use crate::metrics::record_request;
use crate::shared::{
    authorized_internal_request, context_limit_from_query, event_limit_from_query, first_env,
    image_builder_role, json_string, limit_from_query, now_ms, postgres_database_url,
    public_data_source_error, service_name, unauthorized_response,
};
use crate::state::runtime_thread_context;
use crate::types::{
    AgentBreadcrumbIngestRequest, AgentBreadcrumbTailResponse, AgentEventIngestRequest,
    AgentFeedbackRequest, AgentTaskEventsResponse, AgentsQuery, ContextQuery, HealthResponse,
    KnownGitRepoRequest, KnownGitReposResponse, ThreadContextResponse,
};

pub(crate) async fn healthz() -> impl IntoResponse {
    record_request("GET", "/healthz", StatusCode::OK);
    Json(HealthResponse {
        ok: true,
        service: service_name().to_string(),
        mode: if image_builder_role() {
            "internal-image-builder".to_string()
        } else {
            "database-boundary".to_string()
        },
    })
}

pub(crate) async fn agents_tasks(Query(query): Query<AgentsQuery>) -> impl IntoResponse {
    record_request("GET", "/api/agents/tasks", StatusCode::OK);
    Json(fetch_agents_snapshot(limit_from_query(&query)).await)
}

pub(crate) async fn known_git_repos(Query(query): Query<AgentsQuery>) -> impl IntoResponse {
    record_request("GET", "/api/agents/git-repos", StatusCode::OK);
    if postgres_database_url().is_none() {
        return Json(KnownGitReposResponse {
            ok: false,
            source: "postgres".to_string(),
            generated_at_ms: now_ms(),
            repos: Vec::new(),
            errors: vec!["postgres database URL is not configured".to_string()],
        });
    }

    match fetch_known_git_repos_from_postgres(limit_from_query(&query)).await {
        Ok(repos) => Json(KnownGitReposResponse {
            ok: true,
            source: "postgres".to_string(),
            generated_at_ms: now_ms(),
            repos,
            errors: Vec::new(),
        }),
        Err(error) => Json(KnownGitReposResponse {
            ok: false,
            source: "postgres".to_string(),
            generated_at_ms: now_ms(),
            repos: Vec::new(),
            errors: vec![public_data_source_error("postgres"), error],
        }),
    }
}

pub(crate) async fn save_known_git_repo(Json(request): Json<KnownGitRepoRequest>) -> Response {
    record_request("POST", "/api/agents/git-repos", StatusCode::OK);
    if postgres_database_url().is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(KnownGitReposResponse {
                ok: false,
                source: "postgres".to_string(),
                generated_at_ms: now_ms(),
                repos: Vec::new(),
                errors: vec!["postgres database URL is not configured".to_string()],
            }),
        )
            .into_response();
    }

    match upsert_known_git_repo_to_postgres(
        &request.repo_url,
        request.display_name.as_deref(),
        request.provider.as_deref(),
        request.default_branch.as_deref(),
    )
    .await
    {
        Ok(repo) => Json(KnownGitReposResponse {
            ok: true,
            source: "postgres".to_string(),
            generated_at_ms: now_ms(),
            repos: vec![repo],
            errors: Vec::new(),
        })
        .into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(KnownGitReposResponse {
                ok: false,
                source: "postgres".to_string(),
                generated_at_ms: now_ms(),
                repos: Vec::new(),
                errors: vec![error],
            }),
        )
            .into_response(),
    }
}

pub(crate) async fn agent_task_events(
    Path(task_id): Path<String>,
    Query(query): Query<ContextQuery>,
) -> impl IntoResponse {
    record_request("GET", "/api/agents/tasks/:taskId/events", StatusCode::OK);
    let limit = event_limit_from_query(&query);
    if postgres_database_url().is_some() {
        match fetch_agent_events_from_postgres(&task_id, limit).await {
            Ok(events) => {
                return Json(AgentTaskEventsResponse {
                    ok: true,
                    source: "postgres".to_string(),
                    task_id,
                    generated_at_ms: now_ms(),
                    events,
                    errors: Vec::new(),
                });
            }
            Err(error) => {
                return Json(AgentTaskEventsResponse {
                    ok: false,
                    source: "runtime-memory".to_string(),
                    task_id,
                    generated_at_ms: now_ms(),
                    events: Vec::new(),
                    errors: vec![public_data_source_error("postgres events"), error],
                });
            }
        }
    }

    Json(AgentTaskEventsResponse {
        ok: false,
        source: "runtime-memory".to_string(),
        task_id,
        generated_at_ms: now_ms(),
        events: Vec::new(),
        errors: vec![
            "postgres database URL is not configured; task events are unavailable".to_string(),
        ],
    })
}

pub(crate) async fn agent_task_feedback(
    Path(task_id): Path<String>,
    Json(request): Json<AgentFeedbackRequest>,
) -> Response {
    record_request("POST", "/api/agents/tasks/:taskId/feedback", StatusCode::OK);
    let vote = request.vote.trim().to_lowercase();
    if vote != "up" && vote != "down" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "feedback vote must be up or down" })),
        )
            .into_response();
    }
    if postgres_database_url().is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "postgres database URL is not configured; feedback is unavailable"
            })),
        )
            .into_response();
    }

    match persist_feedback_event_to_postgres(&task_id, &request).await {
        Ok(event) => Json(json!({
            "ok": true,
            "source": "postgres",
            "taskId": task_id,
            "event": event
        }))
        .into_response(),
        Err(error) => {
            tracing::error!("agent feedback persist failed: {error}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": public_data_source_error("postgres feedback") })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn thread_context(
    Path(thread_id): Path<String>,
    Query(query): Query<ContextQuery>,
) -> impl IntoResponse {
    record_request(
        "GET",
        "/api/agents/threads/:threadId/context",
        StatusCode::OK,
    );
    let limit = context_limit_from_query(&query);
    if postgres_database_url().is_some() {
        match fetch_thread_context_from_postgres(&thread_id, limit).await {
            Ok(tasks) => {
                return Json(ThreadContextResponse {
                    ok: true,
                    source: "postgres".to_string(),
                    thread_id,
                    generated_at_ms: now_ms(),
                    tasks,
                    errors: Vec::new(),
                });
            }
            Err(error) => {
                return Json(runtime_thread_context(
                    &thread_id,
                    limit,
                    vec![public_data_source_error("postgres"), error],
                ));
            }
        }
    }

    Json(runtime_thread_context(
        &thread_id,
        limit,
        vec!["postgres database URL is not configured; showing runtime memory only".to_string()],
    ))
}

pub(crate) async fn ingest_agent_event(
    headers: HeaderMap,
    Json(request): Json<AgentEventIngestRequest>,
) -> Response {
    record_request("POST", "/api/agents/events", StatusCode::OK);
    if !authorized_internal_request(&headers) {
        return unauthorized_response();
    }
    let Some(event_kind) = json_string(&request.event, "kind") else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "event.kind is required" })),
        )
            .into_response();
    };
    match persist_agent_event_to_postgres(&request, &event_kind).await {
        Ok(()) => {
            if let Some(thread_id) = request.thread_id.as_deref() {
                publish_task_event_to_websocket_fanout(
                    thread_id,
                    &request.task_id,
                    request.seq,
                    &task_event_message_id(&request.task_id, request.seq, &request.event),
                    &request.event,
                )
                .await;
            }
            Json(json!({ "ok": true })).into_response()
        }
        Err(error) => {
            tracing::error!("agent event ingest failed: {error}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": public_data_source_error("postgres event ingest") })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn ingest_agent_breadcrumb(
    headers: HeaderMap,
    Path(thread_id): Path<String>,
    Json(mut request): Json<AgentBreadcrumbIngestRequest>,
) -> Response {
    record_request(
        "POST",
        "/api/agents/threads/:threadId/breadcrumbs",
        StatusCode::OK,
    );
    if !authorized_internal_request(&headers) {
        return unauthorized_response();
    }
    if request.thread_id.is_empty() {
        request.thread_id = thread_id.clone();
    } else if request.thread_id != thread_id {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "thread_id in body does not match :threadId path" })),
        )
            .into_response();
    }
    if request.kind.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "kind is required" })),
        )
            .into_response();
    }
    match persist_agent_breadcrumb_to_postgres(&request).await {
        Ok(row) => Json(json!({ "ok": true, "breadcrumb": row })).into_response(),
        Err(error) => {
            tracing::error!("agent breadcrumb ingest failed: {error}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": public_data_source_error("postgres breadcrumb ingest"),
                })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn agent_thread_breadcrumb_tail(
    Path(thread_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    record_request(
        "GET",
        "/api/agents/threads/:threadId/breadcrumbs/tail",
        StatusCode::OK,
    );
    let limit = query
        .get("limit")
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.min(500))
        .unwrap_or(100);
    let exclude_task_id = query
        .get("excludeTaskId")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    if postgres_database_url().is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "ok": false,
                "error": "postgres database URL is not configured",
            })),
        )
            .into_response();
    }
    match fetch_agent_breadcrumb_tail_from_postgres(&thread_id, limit, exclude_task_id.as_deref())
        .await
    {
        Ok(items) => Json(AgentBreadcrumbTailResponse {
            thread_id,
            items,
            source: "postgres",
            excluded_task_id: exclude_task_id,
            limit,
        })
        .into_response(),
        Err(error) => {
            tracing::error!("agent breadcrumb tail fetch failed: {error}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "ok": false,
                    "error": public_data_source_error("postgres breadcrumb tail"),
                })),
            )
                .into_response()
        }
    }
}

// ---------- Runtime-config proxy ----------
//
// Short-lived consumers (gleam-lambda-runner child runtimes, container-pool
// images, ad-hoc cron jobs) pull their snapshot at boot from
// /api/runtime-config/snapshot/{env}?scope=...
// The rest-api forwards to the in-cluster dd-runtime-config service. We keep
// the snapshot endpoint unauthenticated through the gateway (same posture as
// the agents tasks UI fetch); secrets must not be put in runtime-config
// entries.
pub(crate) async fn runtime_config_snapshot(
    axum::extract::Path(env_label): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    if env_label != "stage" && env_label != "prod" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "env must be 'stage' or 'prod'" })),
        )
            .into_response();
    }
    let base = first_env(&["RUNTIME_CONFIG_BASE_URL"])
        .unwrap_or_else(|| "http://dd-runtime-config.default.svc.cluster.local:8110".to_string());
    let mut url = format!("{base}/snapshot/{env_label}");
    if let Some(scope) = query.get("scope") {
        url.push_str(&format!("?scope={}", urlencoding_minimal(scope)));
    }
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "ok": false,
                    "error": format!("http client init failed: {error}"),
                })),
            )
                .into_response();
        }
    };
    let response = match client.get(&url).send().await {
        Ok(response) => response,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "ok": false,
                    "error": format!("upstream runtime-config unreachable: {error}"),
                })),
            )
                .into_response();
        }
    };
    let status = response.status();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "ok": false,
                    "error": format!("upstream body read failed: {error}"),
                })),
            )
                .into_response();
        }
    };
    (
        axum::http::StatusCode::from_u16(status.as_u16()).unwrap_or(axum::http::StatusCode::OK),
        [(header::CONTENT_TYPE, "application/json".to_string())],
        bytes,
    )
        .into_response()
}

pub(crate) fn urlencoding_minimal(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~' | '*' | ':' | '/') {
            out.push(ch);
        } else {
            for byte in ch.to_string().as_bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}
