use std::{
    env,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use dd_nats_subject_defs::{
    thread_tasks_subject, DD_REMOTE_TASKS_STREAM_NAME, GIT_REPOS_CHANGES_SUBJECT,
    LAMBDAS_FUNCTIONS_SUBJECT, ORCHESTRATOR_WAKEUP_SUBJECT, RUNTIME_EVENTS_SUBJECT,
    THREAD_TASKS_WILDCARD,
};
use serde_json::{json, Value};

use crate::types::{
    AgentsDataConfig, AgentsQuery, ContextQuery, DispatchTaskRequest, ThreadRepoConfig,
};

pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

pub(crate) fn now_label() -> String {
    now_ms().to_string()
}

pub(crate) fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

pub(crate) fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

pub(crate) fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

pub(crate) fn postgres_database_url() -> Option<String> {
    first_env(&[
        "AGENT_TASKS_RDS_DATABASE_URL",
        "RDS_DATABASE_URL",
        "AGENT_TASKS_DATABASE_URL",
        "DATABASE_URL",
    ])
}

pub(crate) fn agent_tasks_admin_user_id() -> Option<String> {
    first_env(&["AGENT_TASKS_ADMIN_USER_ID", "REMOTE_DEV_ADMIN_USER_ID"])
}

pub(crate) fn data_config() -> AgentsDataConfig {
    let rds_configured = first_env(&["AGENT_TASKS_RDS_DATABASE_URL", "RDS_DATABASE_URL"]).is_some();
    let postgres_configured = postgres_database_url().is_some();
    let supabase_configured = first_env(&["SUPABASE_URL", "NEXT_PUBLIC_SUPABASE_URL"]).is_some()
        && first_env(&["SUPABASE_SERVICE_ROLE_KEY", "SUPABASE_KEY"]).is_some();
    let nats_url = first_env(&["NATS_URL"])
        .unwrap_or_else(|| "nats://dd-nats.messaging.svc.cluster.local:4222".to_string());

    AgentsDataConfig {
        rds_configured,
        postgres_configured,
        supabase_configured,
        nats_configured: first_env(&["NATS_URL"]).is_some(),
        nats_url,
        postgres_plan:
            "This REST API is the database boundary. Point AGENT_TASKS_RDS_DATABASE_URL at RDS today, then swap to an in-cluster Postgres service later."
                .to_string(),
    }
}

pub(crate) fn limit_from_query(query: &AgentsQuery) -> i64 {
    query.limit.unwrap_or(50).clamp(1, 200)
}

pub(crate) fn context_limit_from_query(query: &ContextQuery) -> i64 {
    query.limit.unwrap_or(20).clamp(1, 100)
}

pub(crate) fn context_candidate_limit(value: Option<i64>) -> i64 {
    value.unwrap_or(10).clamp(1, 10)
}

pub(crate) fn normalize_context_project_id(value: Option<&str>) -> Result<String, String> {
    let project_id = value.unwrap_or("default").trim();
    if project_id.is_empty() {
        return Ok("default".to_string());
    }
    if project_id.len() > 120 {
        return Err("projectId must be 120 characters or fewer".to_string());
    }
    if !project_id
        .chars()
        .all(|item| item.is_ascii_alphanumeric() || matches!(item, '.' | '_' | ':' | '/' | '-'))
    {
        return Err("projectId contains unsupported characters".to_string());
    }
    Ok(project_id.to_string())
}

pub(crate) fn normalize_context_mode(value: Option<&str>, selected_count: usize) -> String {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some("none") | Some("zero") | Some("off") => "none".to_string(),
        Some("selected") => "selected".to_string(),
        Some("auto") => "auto".to_string(),
        _ if selected_count > 0 => "selected".to_string(),
        _ => "none".to_string(),
    }
}

pub(crate) fn event_limit_from_query(query: &ContextQuery) -> i64 {
    query.limit.unwrap_or(100).clamp(1, 500)
}

pub(crate) fn public_data_source_error(source: &str) -> String {
    format!("{source} source unavailable; check remote REST API server logs")
}

pub(crate) fn public_thread_worker_proxy_error(action: &str) -> String {
    format!("thread worker {action} failed; check remote REST API server logs")
}

pub(crate) fn normalize_repo_url(value: &str) -> Result<String, String> {
    let repo = value.trim();
    if repo.is_empty() {
        return Err("repo is required".to_string());
    }
    if repo.len() > 2048 {
        return Err("repo must be 2048 characters or fewer".to_string());
    }
    if !(repo.starts_with("git@") || repo.starts_with("ssh://") || repo.starts_with("https://")) {
        return Err("repo must start with git@, ssh://, or https://".to_string());
    }
    Ok(repo.to_string())
}

pub(crate) fn normalize_base_branch(value: Option<&str>) -> Result<String, String> {
    let branch = value.unwrap_or("dev").trim();
    if branch.is_empty() {
        return Err("baseBranch must not be empty".to_string());
    }
    if branch.len() > 120 {
        return Err("baseBranch must be 120 characters or fewer".to_string());
    }
    if !branch
        .chars()
        .all(|item| item.is_ascii_alphanumeric() || matches!(item, '.' | '_' | '/' | '-'))
    {
        return Err("baseBranch contains unsupported characters".to_string());
    }
    Ok(branch.to_string())
}

pub(crate) fn infer_repo_display_name(repo_url: &str) -> String {
    repo_url
        .trim_end_matches(".git")
        .rsplit(['/', ':'])
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("Git repository")
        .chars()
        .take(200)
        .collect()
}

pub(crate) fn infer_repo_provider(repo_url: &str) -> String {
    if repo_url.contains("github.com") {
        "github".to_string()
    } else if repo_url.contains("gitlab.com") {
        "gitlab".to_string()
    } else if repo_url.contains("bitbucket.org") {
        "bitbucket".to_string()
    } else {
        "generic".to_string()
    }
}

pub(crate) fn normalize_repo_provider(value: Option<&str>, repo_url: &str) -> Result<String, String> {
    let provider = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| infer_repo_provider(repo_url));
    if matches!(
        provider.as_str(),
        "github" | "gitlab" | "bitbucket" | "generic"
    ) {
        Ok(provider)
    } else {
        Err("provider must be github, gitlab, bitbucket, or generic".to_string())
    }
}

pub(crate) fn normalized_repo_config(request: &DispatchTaskRequest) -> Result<ThreadRepoConfig, String> {
    Ok(ThreadRepoConfig {
        repo: normalize_repo_url(&request.repo)?,
        base_branch: normalize_base_branch(request.base_branch.as_deref())?,
        thread_title: request
            .thread_title
            .clone()
            .or_else(|| Some(request.prompt.chars().take(80).collect::<String>())),
    })
}

pub(crate) fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "unauthorized",
            "errMessage": "missing required dd header",
        })),
    )
        .into_response()
}

pub(crate) fn authorized_internal_request(headers: &HeaderMap) -> bool {
    let Some(expected) = worker_auth_secret() else {
        return false;
    };
    headers
        .get("x-agent-auth")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
}

pub(crate) fn worker_auth_secret() -> Option<String> {
    first_env(&["REMOTE_DEV_SERVER_SECRET", "SERVER_AUTH_SECRET"])
}

pub(crate) fn constant_time_equals(candidate: &str, expected: &str) -> bool {
    let candidate = candidate.as_bytes();
    let expected = expected.as_bytes();
    if candidate.len() != expected.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in candidate.iter().zip(expected.iter()) {
        difference |= left ^ right;
    }
    difference == 0
}

pub(crate) fn authorized_image_builder_request(headers: &HeaderMap) -> bool {
    let Some(expected) = worker_auth_secret() else {
        return false;
    };
    headers
        .get("x-server-auth")
        .or_else(|| headers.get("x-agent-auth"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| constant_time_equals(value, &expected))
}

pub(crate) fn image_builder_role() -> bool {
    env::var("DD_SERVICE_ROLE")
        .ok()
        .is_some_and(|value| value.eq_ignore_ascii_case("image-builder"))
}

pub(crate) fn service_name() -> &'static str {
    if image_builder_role() {
        "dd-image-builder"
    } else {
        "dd-remote-rest-api"
    }
}

pub(crate) fn missing_worker_auth_secret_message() -> &'static str {
    "REMOTE_DEV_SERVER_SECRET or SERVER_AUTH_SECRET is not set"
}

pub(crate) fn nats_url() -> String {
    first_env(&["NATS_URL"])
        .unwrap_or_else(|| "nats://dd-nats.messaging.svc.cluster.local:4222".to_string())
}

pub(crate) fn nats_task_subject(thread_id: &str) -> String {
    thread_tasks_subject(thread_id)
}

pub(crate) fn nats_task_stream_subject() -> String {
    first_env(&["NATS_TASK_SUBJECT"]).unwrap_or_else(|| THREAD_TASKS_WILDCARD.to_string())
}

pub(crate) fn nats_task_stream_name() -> String {
    first_env(&["NATS_TASK_STREAM"]).unwrap_or_else(|| DD_REMOTE_TASKS_STREAM_NAME.to_string())
}

pub(crate) fn nats_wakeup_subject() -> &'static str {
    ORCHESTRATOR_WAKEUP_SUBJECT
}

pub(crate) fn nats_event_subject() -> &'static str {
    RUNTIME_EVENTS_SUBJECT
}

pub(crate) fn rest_status_gleam_broadcast_url() -> String {
    first_env(&["REST_STATUS_GLEAM_BROADCAST_URL", "GLEAM_BROADCAST_URL"]).unwrap_or_else(|| {
        "http://dd-gleamlang-server.default.svc.cluster.local:8081/broadcast".to_string()
    })
}

pub(crate) fn rest_status_gleam_broadcast_secret() -> Option<String> {
    first_env(&[
        "REST_STATUS_GLEAM_BROADCAST_SECRET",
        "GLEAM_BROADCAST_SECRET",
        "NATS_WATCH_GLEAM_BROADCAST_SECRET",
    ])
}

pub(crate) fn rest_status_rust_broadcast_url() -> String {
    first_env(&["REST_STATUS_RUST_BROADCAST_URL", "RUNTIME_BROADCAST_URL"]).unwrap_or_else(|| {
        "http://dd-webrtc-signaling.default.svc.cluster.local:8095/runtime/broadcast".to_string()
    })
}

pub(crate) fn rest_status_rust_broadcast_secret() -> Option<String> {
    first_env(&[
        "REST_STATUS_RUST_BROADCAST_SECRET",
        "RUNTIME_BROADCAST_SECRET",
        "REMOTE_DEV_SERVER_SECRET",
        "SERVER_AUTH_SECRET",
    ])
}

pub(crate) fn nats_lambda_functions_subject() -> &'static str {
    LAMBDAS_FUNCTIONS_SUBJECT
}

pub(crate) fn nats_git_repos_changes_subject() -> &'static str {
    GIT_REPOS_CHANGES_SUBJECT
}

pub(crate) fn cdc_stream_name() -> String {
    first_env(&["REST_API_CDC_STREAM"]).unwrap_or_else(|| "CDC".to_string())
}

pub(crate) fn json_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |cursor, segment| cursor.get(*segment))
}

pub(crate) fn json_at_string(value: &Value, path: &[&str]) -> Option<String> {
    json_at(value, path)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .filter(|text| !text.is_empty())
}

pub(crate) fn json_at_i64(value: &Value, path: &[&str]) -> Option<i64> {
    json_at(value, path).and_then(Value::as_i64)
}

pub(crate) fn row_string(row: &tokio_postgres::Row, column: &str) -> String {
    row.try_get::<_, String>(column).unwrap_or_default()
}

pub(crate) fn row_opt_string(row: &tokio_postgres::Row, column: &str) -> Option<String> {
    row.try_get::<_, Option<String>>(column)
        .ok()
        .flatten()
        .filter(|value| !value.is_empty())
}

pub(crate) fn row_i32(row: &tokio_postgres::Row, column: &str) -> i32 {
    row.try_get::<_, i32>(column).unwrap_or_default()
}

pub(crate) fn row_i64(row: &tokio_postgres::Row, column: &str) -> i64 {
    row.try_get::<_, i64>(column).unwrap_or_default()
}

pub(crate) fn row_bool(row: &tokio_postgres::Row, column: &str) -> bool {
    row.try_get::<_, bool>(column).unwrap_or_default()
}

pub(crate) fn row_value(row: &tokio_postgres::Row, column: &str, fallback: Value) -> Value {
    row.try_get::<_, Value>(column).unwrap_or(fallback)
}

pub(crate) fn looks_like_uuid(input: &str) -> bool {
    let bytes = input.as_bytes();
    if bytes.len() != 36 {
        return false;
    }

    bytes.iter().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            *byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}

pub(crate) fn json_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .filter(|text| !text.is_empty())
}

pub(crate) fn json_i32(value: &Value, key: &str) -> i32 {
    value
        .get(key)
        .and_then(Value::as_i64)
        .and_then(|number| i32::try_from(number).ok())
        .unwrap_or_default()
}

pub(crate) fn json_i64(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or_default()
}

pub(crate) fn internal_db_routes_enabled() -> bool {
    env_bool("REST_API_INTERNAL_DB_ROUTES_ENABLED", false)
        || env_bool("REST_API_ENABLE_INTERNAL_DB_ROUTES", false)
}
