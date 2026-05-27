// dd-runtime-config — central runtime-config control plane.
//
// Storage:    Redis (one logical namespace per env: dd:rc:{env}:...).
// Cadence:    every RUNTIME_CONFIG_PUSH_INTERVAL_SECONDS (default 300s = 5 min)
//             the cron loop reads the current snapshot from Redis and POSTs
//             it as the body to every registered subscriber's apply URL.
// On demand:  the admin UI's "Push now" button triggers a POST /push/{env}.
// Auth:       all mutating endpoints (POST/PUT/DELETE on entries, subscribers,
//             push) require X-Server-Auth: $RUNTIME_CONFIG_SERVER_SECRET.
//             Subscribers receive the same header value on the apply payload.
//
// Shared payload shapes (RuntimeConfigEntry, RuntimeConfigSnapshot,
// RuntimeConfigApplyRequest, ...) come from remote/libs/interfaces/shared so
// every consumer language stays byte-compatible.

use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use dd_shared_interfaces::{
    RuntimeConfigApplyReason, RuntimeConfigApplyRequest, RuntimeConfigApplyResponse,
    RuntimeConfigEntry, RuntimeConfigEnv, RuntimeConfigRegisterRequest, RuntimeConfigSnapshot,
    RuntimeConfigSubscriber, RuntimeConfigUpsertRequest,
};
use maud::{html, Markup, PreEscaped, DOCTYPE};
use once_cell::sync::Lazy;
use prometheus::{
    register_int_counter_vec, register_int_gauge_vec, Encoder, IntCounterVec, IntGaugeVec,
    TextEncoder,
};
use redis::aio::MultiplexedConnection;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio::time::{sleep, Instant};

// ---------- Constants ----------

const ENV_REDIS_URL: &str = "RUNTIME_CONFIG_REDIS_URL";
const ENV_REDIS_URL_FALLBACK: &str = "REDIS_URL";
const ENV_REDIS_PREFIX: &str = "RUNTIME_CONFIG_REDIS_PREFIX";
const ENV_PUSH_INTERVAL: &str = "RUNTIME_CONFIG_PUSH_INTERVAL_SECONDS";
const ENV_PUSH_TIMEOUT: &str = "RUNTIME_CONFIG_PUSH_TIMEOUT_SECONDS";
const ENV_SERVER_SECRET: &str = "RUNTIME_CONFIG_SERVER_SECRET";
const ENV_ADMIN_SECRET: &str = "RUNTIME_CONFIG_ADMIN_SECRET";
const ENV_ALLOW_UNAUTHENTICATED: &str = "RUNTIME_CONFIG_ALLOW_UNAUTHENTICATED";
const ENV_ALLOW_EXTERNAL_SUBSCRIBERS: &str = "RUNTIME_CONFIG_ALLOW_EXTERNAL_SUBSCRIBERS";
const ENV_BIND_PORT: &str = "PORT";
const ENV_BIND_HOST: &str = "HOST";

const DEFAULT_PREFIX: &str = "dd:rc";
const DEFAULT_PUSH_INTERVAL_SECS: u64 = 300;
const DEFAULT_PUSH_TIMEOUT_SECS: u64 = 10;
const APPLY_ROUTE_PATH: &str = "/internal/update-runtime-config";

// ---------- Metrics ----------

static PUSH_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "dd_runtime_config_push_total",
        "Total subscriber push attempts by env, subscriber, and result",
        &["env", "subscriber", "result"]
    )
    .expect("register dd_runtime_config_push_total")
});

static ENTRY_COUNT: Lazy<IntGaugeVec> = Lazy::new(|| {
    register_int_gauge_vec!(
        "dd_runtime_config_entries",
        "Current entry count by env (and by env+scope for non-default scopes)",
        &["env", "scope"]
    )
    .expect("register dd_runtime_config_entries")
});

static SUBSCRIBER_COUNT: Lazy<IntGaugeVec> = Lazy::new(|| {
    register_int_gauge_vec!(
        "dd_runtime_config_subscribers",
        "Current subscriber count by env",
        &["env"]
    )
    .expect("register dd_runtime_config_subscribers")
});

// ---------- State ----------

#[derive(Clone)]
struct AppState {
    redis: redis::Client,
    redis_conn: Arc<Mutex<Option<MultiplexedConnection>>>,
    http: reqwest::Client,
    prefix: String,
    server_secret: Option<String>,
    admin_secret: Option<String>,
    allow_unauthenticated: bool,
    allow_external_subscribers: bool,
    push_timeout: Duration,
}

impl AppState {
    async fn connection(&self) -> Result<MultiplexedConnection, ServiceError> {
        let mut guard = self.redis_conn.lock().await;
        if let Some(conn) = guard.as_ref() {
            return Ok(conn.clone());
        }
        let conn = self
            .redis
            .get_multiplexed_async_connection()
            .await
            .map_err(|error| ServiceError::Internal(format!("redis connection failed: {error}")))?;
        *guard = Some(conn.clone());
        Ok(conn)
    }
}

// ---------- Errors ----------

#[derive(Debug)]
enum ServiceError {
    BadRequest(String),
    Unauthorized,
    NotFound(String),
    Unavailable(String),
    Internal(String),
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ServiceError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ServiceError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
            ServiceError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ServiceError::Unavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg),
            ServiceError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        let body = Json(json!({ "ok": false, "error": message }));
        (status, body).into_response()
    }
}

// ---------- Redis key helpers ----------

fn env_token(env: &RuntimeConfigEnv) -> &'static str {
    match env {
        RuntimeConfigEnv::Stage => "stage",
        RuntimeConfigEnv::Prod => "prod",
    }
}

fn parse_env(value: &str) -> Result<RuntimeConfigEnv, ServiceError> {
    match value {
        "stage" => Ok(RuntimeConfigEnv::Stage),
        "prod" => Ok(RuntimeConfigEnv::Prod),
        other => Err(ServiceError::BadRequest(format!(
            "unknown env '{other}'; expected 'stage' or 'prod'"
        ))),
    }
}

fn entry_key(prefix: &str, env: &RuntimeConfigEnv, scope: &str, key: &str) -> String {
    format!("{prefix}:{env}:entry:{scope}:{key}", env = env_token(env))
}

fn entry_index_key(prefix: &str, env: &RuntimeConfigEnv) -> String {
    format!("{prefix}:{env}:entry-index", env = env_token(env))
}

fn generation_key(prefix: &str, env: &RuntimeConfigEnv) -> String {
    format!("{prefix}:{env}:generation", env = env_token(env))
}

fn subscriber_key(prefix: &str, env: &RuntimeConfigEnv, name: &str) -> String {
    format!("{prefix}:{env}:subs:{name}", env = env_token(env))
}

fn subscriber_index_key(prefix: &str, env: &RuntimeConfigEnv) -> String {
    format!("{prefix}:{env}:subs-index", env = env_token(env))
}

fn entry_index_member(scope: &str, key: &str) -> String {
    format!("{scope}\u{1f}{key}")
}

fn parse_entry_index_member(member: &str) -> Option<(String, String)> {
    let mut parts = member.splitn(2, '\u{1f}');
    let scope = parts.next()?.to_string();
    let key = parts.next()?.to_string();
    Some((scope, key))
}

// ---------- Validation ----------

fn validate_scope(value: &str) -> Result<(), ServiceError> {
    if value.is_empty() || value.len() > 120 {
        return Err(ServiceError::BadRequest(
            "scope must be 1-120 chars".to_string(),
        ));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':' | '*'))
    {
        return Err(ServiceError::BadRequest(
            "scope contains unsupported characters".to_string(),
        ));
    }
    Ok(())
}

fn validate_key(value: &str) -> Result<(), ServiceError> {
    if value.is_empty() || value.len() > 200 {
        return Err(ServiceError::BadRequest(
            "key must be 1-200 chars".to_string(),
        ));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':'))
    {
        return Err(ServiceError::BadRequest(
            "key contains unsupported characters".to_string(),
        ));
    }
    Ok(())
}

fn validate_subscriber_name(value: &str) -> Result<(), ServiceError> {
    if value.is_empty() || value.len() > 120 {
        return Err(ServiceError::BadRequest(
            "subscriber name must be 1-120 chars".to_string(),
        ));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(ServiceError::BadRequest(
            "subscriber name contains unsupported characters".to_string(),
        ));
    }
    Ok(())
}

fn validate_apply_url(state: &AppState, value: &str) -> Result<(), ServiceError> {
    let parsed = reqwest::Url::parse(value)
        .map_err(|_| ServiceError::BadRequest("applyUrl must be a valid URL".to_string()))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(ServiceError::BadRequest(
                "applyUrl must use http or https".to_string(),
            ));
        }
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err(ServiceError::BadRequest(
            "applyUrl must not include credentials".to_string(),
        ));
    }
    if parsed.path() != APPLY_ROUTE_PATH {
        return Err(ServiceError::BadRequest(format!(
            "applyUrl path must be {APPLY_ROUTE_PATH}"
        )));
    }
    if state.allow_external_subscribers {
        return Ok(());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| ServiceError::BadRequest("applyUrl must include a host".to_string()))?;
    if host.ends_with(".svc.cluster.local") || matches!(host, "localhost" | "127.0.0.1" | "::1") {
        return Ok(());
    }
    Err(ServiceError::BadRequest(
        "applyUrl host must be a Kubernetes service DNS name; set RUNTIME_CONFIG_ALLOW_EXTERNAL_SUBSCRIBERS=true only for local tests".to_string(),
    ))
}

// ---------- Auth ----------

fn require_server_auth(state: &AppState, headers: &HeaderMap) -> Result<(), ServiceError> {
    let Some(expected) = state.server_secret.as_ref() else {
        if state.allow_unauthenticated {
            return Ok(());
        }
        return Err(ServiceError::Unavailable(
            "runtime config auth is not configured".to_string(),
        ));
    };
    let provided = headers
        .get("x-server-auth")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if provided.is_empty() || !constant_time_eq(provided, expected.as_str()) {
        return Err(ServiceError::Unauthorized);
    }
    Ok(())
}

fn require_admin_auth(state: &AppState, headers: &HeaderMap) -> Result<(), ServiceError> {
    // The admin UI sits behind the gateway operator cookie too; this is a
    // belt-and-braces secondary check that the request really came from a
    // trusted source (either the gateway or an operator running curl).
    if state.admin_secret.is_none() && state.server_secret.is_none() && state.allow_unauthenticated
    {
        return Ok(());
    }
    let provided = headers
        .get("x-admin-auth")
        .or_else(|| headers.get("x-server-auth"))
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let admin_ok = state
        .admin_secret
        .as_ref()
        .is_some_and(|secret| constant_time_eq(provided, secret.as_str()));
    let server_ok = state
        .server_secret
        .as_ref()
        .is_some_and(|secret| constant_time_eq(provided, secret.as_str()));
    if admin_ok || server_ok {
        Ok(())
    } else {
        Err(ServiceError::Unauthorized)
    }
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

// ---------- Time helpers ----------

fn iso_now() -> String {
    // tokio's time can't format ISO 8601 on its own; use chrono-free formatting
    // via the std SystemTime + a hand-rolled formatter to keep the dep tree
    // minimal. Output: "2026-05-21T19:30:00.000Z".
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    format_iso(secs, millis)
}

fn format_iso(secs: i64, millis: u32) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = (rem / 3600) as u32;
    let minute = ((rem % 3600) / 60) as u32;
    let second = (rem % 60) as u32;
    let (year, month, day) = days_to_date(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn days_to_date(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 {
        z / 146_097
    } else {
        (z - 146_096) / 146_097
    };
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m as u32, d as u32)
}

fn fresh_push_id() -> String {
    // Lightweight UUIDv4-like id: 32 hex chars from a 128-bit random pool.
    let mut bytes = [0u8; 16];
    for byte in bytes.iter_mut() {
        *byte = (fastrand() & 0xff) as u8;
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:04x}{:08x}",
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        u16::from_be_bytes([bytes[4], bytes[5]]),
        u16::from_be_bytes([bytes[6], bytes[7]]),
        u16::from_be_bytes([bytes[8], bytes[9]]),
        u16::from_be_bytes([bytes[10], bytes[11]]),
        u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
    )
}

fn fastrand() -> u64 {
    use std::cell::Cell;
    thread_local! {
        static STATE: Cell<u64> = Cell::new(seed());
    }
    fn seed() -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0xbad_5eed);
        (now ^ 0x9E37_79B9_7F4A_7C15).wrapping_add(0xDEAD_BEEF)
    }
    STATE.with(|cell| {
        let mut x = cell.get();
        if x == 0 {
            x = 0xdead_beef_dead_beef;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        cell.set(x);
        x
    })
}

// ---------- Redis I/O ----------

async fn load_entries(
    state: &AppState,
    env: &RuntimeConfigEnv,
    scope_filter: Option<&str>,
) -> Result<Vec<RuntimeConfigEntry>, ServiceError> {
    let mut conn = state.connection().await?;
    let index = entry_index_key(&state.prefix, env);
    let members: Vec<String> = conn.smembers(&index).await.map_err(|error| {
        ServiceError::Internal(format!("redis SMEMBERS {index} failed: {error}"))
    })?;
    let mut entries = Vec::new();
    for member in members {
        let Some((scope, key)) = parse_entry_index_member(&member) else {
            continue;
        };
        if let Some(filter) = scope_filter {
            if filter != "*" && scope != filter && scope != "*" {
                continue;
            }
        }
        let entry_key = entry_key(&state.prefix, env, &scope, &key);
        let raw: Option<String> = conn.get(&entry_key).await.map_err(|error| {
            ServiceError::Internal(format!("redis GET {entry_key} failed: {error}"))
        })?;
        let Some(raw) = raw else { continue };
        match serde_json::from_str::<RuntimeConfigEntry>(&raw) {
            Ok(entry) => entries.push(entry),
            Err(error) => {
                eprintln!("[dd-runtime-config] dropping malformed entry {entry_key}: {error}");
            }
        }
    }
    entries.sort_by(|a, b| {
        (a.scope.as_str(), a.key.as_str()).cmp(&(b.scope.as_str(), b.key.as_str()))
    });
    Ok(entries)
}

async fn load_generation(
    state: &AppState,
    env: &RuntimeConfigEnv,
) -> Result<Option<i64>, ServiceError> {
    let mut conn = state.connection().await?;
    let key = generation_key(&state.prefix, env);
    conn.get(&key)
        .await
        .map_err(|error| ServiceError::Internal(format!("redis GET {key} failed: {error}")))
}

async fn bump_generation(state: &AppState, env: &RuntimeConfigEnv) -> Result<i64, ServiceError> {
    let mut conn = state.connection().await?;
    let key = generation_key(&state.prefix, env);
    conn.incr(&key, 1)
        .await
        .map_err(|error| ServiceError::Internal(format!("redis INCR {key} failed: {error}")))
}

async fn build_snapshot(
    state: &AppState,
    env: &RuntimeConfigEnv,
    scope_filter: Option<&str>,
) -> Result<RuntimeConfigSnapshot, ServiceError> {
    let entries = load_entries(state, env, scope_filter).await?;
    let snapshot_version = load_generation(state, env)
        .await?
        .unwrap_or_else(|| entries.iter().map(|entry| entry.version).sum());
    let scope = scope_filter.unwrap_or("*").to_string();
    Ok(RuntimeConfigSnapshot {
        env: env.clone(),
        scope,
        generated_at: iso_now(),
        snapshot_version,
        entries,
    })
}

async fn upsert_entry(
    state: &AppState,
    request: RuntimeConfigUpsertRequest,
    reason: Option<&str>,
    actor: Option<&str>,
) -> Result<RuntimeConfigEntry, ServiceError> {
    validate_scope(&request.scope)?;
    validate_key(&request.key)?;
    let mut conn = state.connection().await?;
    let entry_key = entry_key(&state.prefix, &request.env, &request.scope, &request.key);
    let index_key = entry_index_key(&state.prefix, &request.env);

    let prior: Option<String> = conn.get(&entry_key).await.map_err(|error| {
        ServiceError::Internal(format!("redis GET {entry_key} failed: {error}"))
    })?;
    let prior_version: i64 = prior
        .as_deref()
        .and_then(|raw| serde_json::from_str::<RuntimeConfigEntry>(raw).ok())
        .map(|entry| entry.version)
        .unwrap_or(0);

    let mut meta = serde_json::Map::new();
    if let Some(reason) = reason {
        if !reason.is_empty() {
            meta.insert("reason".to_string(), Value::String(reason.to_string()));
        }
    }
    if let Some(actor) = actor {
        if !actor.is_empty() {
            meta.insert("actor".to_string(), Value::String(actor.to_string()));
        }
    }

    let entry = RuntimeConfigEntry {
        env: request.env.clone(),
        scope: request.scope.clone(),
        key: request.key.clone(),
        value: Some(request.value.unwrap_or(Value::Null)),
        version: prior_version + 1,
        updated_at: iso_now(),
        labels: request.labels.clone(),
        meta: if meta.is_empty() {
            None
        } else {
            Some(Value::Object(meta))
        },
    };
    let serialised = serde_json::to_string(&entry)
        .map_err(|error| ServiceError::Internal(format!("entry serialisation failed: {error}")))?;
    let _: () = conn.set(&entry_key, serialised).await.map_err(|error| {
        ServiceError::Internal(format!("redis SET {entry_key} failed: {error}"))
    })?;
    let _: () = conn
        .sadd(&index_key, entry_index_member(&entry.scope, &entry.key))
        .await
        .map_err(|error| {
            ServiceError::Internal(format!("redis SADD {index_key} failed: {error}"))
        })?;
    bump_generation(state, &entry.env).await?;
    Ok(entry)
}

async fn delete_entry(
    state: &AppState,
    env: &RuntimeConfigEnv,
    scope: &str,
    key: &str,
) -> Result<bool, ServiceError> {
    validate_scope(scope)?;
    validate_key(key)?;
    let mut conn = state.connection().await?;
    let entry_key = entry_key(&state.prefix, env, scope, key);
    let index_key = entry_index_key(&state.prefix, env);
    let removed: i64 = conn.del(&entry_key).await.map_err(|error| {
        ServiceError::Internal(format!("redis DEL {entry_key} failed: {error}"))
    })?;
    let _: i64 = conn
        .srem(&index_key, entry_index_member(scope, key))
        .await
        .map_err(|error| {
            ServiceError::Internal(format!("redis SREM {index_key} failed: {error}"))
        })?;
    if removed > 0 {
        bump_generation(state, env).await?;
    }
    Ok(removed > 0)
}

async fn load_subscribers(
    state: &AppState,
    env: &RuntimeConfigEnv,
) -> Result<Vec<RuntimeConfigSubscriber>, ServiceError> {
    let mut conn = state.connection().await?;
    let index_key = subscriber_index_key(&state.prefix, env);
    let names: Vec<String> = conn.smembers(&index_key).await.map_err(|error| {
        ServiceError::Internal(format!("redis SMEMBERS {index_key} failed: {error}"))
    })?;
    let mut subs = Vec::new();
    for name in names {
        let key = subscriber_key(&state.prefix, env, &name);
        let raw: Option<String> = conn
            .get(&key)
            .await
            .map_err(|error| ServiceError::Internal(format!("redis GET {key} failed: {error}")))?;
        if let Some(raw) = raw {
            if let Ok(sub) = serde_json::from_str::<RuntimeConfigSubscriber>(&raw) {
                subs.push(sub);
            }
        }
    }
    subs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(subs)
}

async fn store_subscriber(
    state: &AppState,
    subscriber: &RuntimeConfigSubscriber,
) -> Result<(), ServiceError> {
    let mut conn = state.connection().await?;
    let key = subscriber_key(&state.prefix, &subscriber.env, &subscriber.name);
    let index_key = subscriber_index_key(&state.prefix, &subscriber.env);
    let serialised = serde_json::to_string(subscriber).map_err(|error| {
        ServiceError::Internal(format!("subscriber serialisation failed: {error}"))
    })?;
    let _: () = conn
        .set(&key, serialised)
        .await
        .map_err(|error| ServiceError::Internal(format!("redis SET {key} failed: {error}")))?;
    let _: () = conn
        .sadd(&index_key, subscriber.name.clone())
        .await
        .map_err(|error| {
            ServiceError::Internal(format!("redis SADD {index_key} failed: {error}"))
        })?;
    Ok(())
}

async fn delete_subscriber(
    state: &AppState,
    env: &RuntimeConfigEnv,
    name: &str,
) -> Result<bool, ServiceError> {
    validate_subscriber_name(name)?;
    let mut conn = state.connection().await?;
    let key = subscriber_key(&state.prefix, env, name);
    let index_key = subscriber_index_key(&state.prefix, env);
    let removed: i64 = conn
        .del(&key)
        .await
        .map_err(|error| ServiceError::Internal(format!("redis DEL {key} failed: {error}")))?;
    let _: i64 = conn.srem(&index_key, name).await.map_err(|error| {
        ServiceError::Internal(format!("redis SREM {index_key} failed: {error}"))
    })?;
    Ok(removed > 0)
}

// ---------- Push ----------

#[derive(Debug, Serialize)]
struct PushOutcome {
    env: String,
    subscriber: String,
    ok: bool,
    status: Option<u16>,
    error: Option<String>,
    applied_version: Option<i64>,
}

async fn push_to_env(
    state: &AppState,
    env: &RuntimeConfigEnv,
    reason: RuntimeConfigApplyReason,
) -> Vec<PushOutcome> {
    let subs = match load_subscribers(state, env).await {
        Ok(list) => list,
        Err(error) => {
            eprintln!(
                "[dd-runtime-config] failed to load subscribers for {}: {error:?}",
                env_token(env)
            );
            return Vec::new();
        }
    };
    SUBSCRIBER_COUNT
        .with_label_values(&[env_token(env)])
        .set(subs.len() as i64);

    let mut outcomes = Vec::new();
    for subscriber in subs {
        let snapshot = match build_snapshot(state, env, Some(&subscriber.scope)).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                outcomes.push(PushOutcome {
                    env: env_token(env).to_string(),
                    subscriber: subscriber.name.clone(),
                    ok: false,
                    status: None,
                    error: Some(format!("snapshot build failed: {error:?}")),
                    applied_version: None,
                });
                continue;
            }
        };
        let request = RuntimeConfigApplyRequest {
            push_id: fresh_push_id(),
            reason: reason.clone(),
            snapshot,
        };
        let outcome = push_one(state, env, &subscriber, &request).await;
        outcomes.push(outcome);
    }
    outcomes
}

async fn push_one(
    state: &AppState,
    env: &RuntimeConfigEnv,
    subscriber: &RuntimeConfigSubscriber,
    request: &RuntimeConfigApplyRequest,
) -> PushOutcome {
    let env_label = env_token(env).to_string();
    let mut req_builder = state
        .http
        .post(&subscriber.apply_url)
        .timeout(state.push_timeout)
        .json(request);
    if let Some(secret) = state.server_secret.as_ref() {
        req_builder = req_builder.header("x-server-auth", secret.as_str());
    }
    let response = match req_builder.send().await {
        Ok(response) => response,
        Err(error) => {
            PUSH_TOTAL
                .with_label_values(&[&env_label, &subscriber.name, "transport_error"])
                .inc();
            record_push_result(state, subscriber, false, Some(error.to_string()), None).await;
            return PushOutcome {
                env: env_label,
                subscriber: subscriber.name.clone(),
                ok: false,
                status: None,
                error: Some(error.to_string()),
                applied_version: None,
            };
        }
    };
    let status = response.status();
    let result_label = if status.is_success() {
        "ok"
    } else {
        "http_error"
    };
    PUSH_TOTAL
        .with_label_values(&[&env_label, &subscriber.name, result_label])
        .inc();

    let applied_version = response
        .json::<RuntimeConfigApplyResponse>()
        .await
        .ok()
        .map(|payload| payload.applied_version);
    let ok = status.is_success();
    let error = if ok {
        None
    } else {
        Some(format!("http {}", status.as_u16()))
    };
    record_push_result(state, subscriber, ok, error.clone(), applied_version).await;
    PushOutcome {
        env: env_label,
        subscriber: subscriber.name.clone(),
        ok,
        status: Some(status.as_u16()),
        error,
        applied_version,
    }
}

async fn record_push_result(
    state: &AppState,
    subscriber: &RuntimeConfigSubscriber,
    ok: bool,
    error: Option<String>,
    applied_version: Option<i64>,
) {
    let mut updated = subscriber.clone();
    updated.last_push_at = Some(iso_now());
    updated.last_push_ok = Some(ok);
    updated.last_push_error = if ok { None } else { error };
    if let Some(version) = applied_version {
        updated.last_applied_version = Some(version);
    }
    if let Err(error) = store_subscriber(state, &updated).await {
        eprintln!(
            "[dd-runtime-config] failed to persist subscriber result for {}: {error:?}",
            subscriber.name
        );
    }
}

// ---------- Cron ----------

async fn run_push_loop(state: AppState, interval: Duration) {
    println!(
        "[dd-runtime-config] push loop starting, interval={}s",
        interval.as_secs()
    );
    loop {
        let started = Instant::now();
        for env in [RuntimeConfigEnv::Stage, RuntimeConfigEnv::Prod] {
            let outcomes = push_to_env(&state, &env, RuntimeConfigApplyReason::Cron).await;
            let ok = outcomes.iter().filter(|outcome| outcome.ok).count();
            let total = outcomes.len();
            if total > 0 {
                println!(
                    "[dd-runtime-config] cron push env={} ok={}/{}",
                    env_token(&env),
                    ok,
                    total
                );
            }
            if let Ok(entries) = load_entries(&state, &env, None).await {
                let mut by_scope: HashMap<String, i64> = HashMap::new();
                for entry in &entries {
                    *by_scope.entry(entry.scope.clone()).or_insert(0) += 1;
                }
                ENTRY_COUNT
                    .with_label_values(&[env_token(&env), "*"])
                    .set(entries.len() as i64);
                for (scope, count) in by_scope {
                    ENTRY_COUNT
                        .with_label_values(&[env_token(&env), scope.as_str()])
                        .set(count);
                }
            }
        }
        let elapsed = started.elapsed();
        if elapsed < interval {
            sleep(interval - elapsed).await;
        }
    }
}

// ---------- HTTP handlers ----------

#[derive(Deserialize)]
struct EntriesQuery {
    scope: Option<String>,
}

async fn healthz() -> impl IntoResponse {
    Json(json!({ "ok": true }))
}

async fn metrics() -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder
        .encode(&metric_families, &mut buffer)
        .expect("failed to encode metrics");
    (
        [(header::CONTENT_TYPE, encoder.format_type().to_string())],
        buffer,
    )
}

async fn list_entries(
    State(state): State<AppState>,
    Path(env_label): Path<String>,
    Query(query): Query<EntriesQuery>,
) -> Result<Json<RuntimeConfigSnapshot>, ServiceError> {
    let env = parse_env(&env_label)?;
    let snapshot = build_snapshot(&state, &env, query.scope.as_deref()).await?;
    Ok(Json(snapshot))
}

async fn get_entry(
    State(state): State<AppState>,
    Path((env_label, scope, key)): Path<(String, String, String)>,
) -> Result<Json<RuntimeConfigEntry>, ServiceError> {
    let env = parse_env(&env_label)?;
    validate_scope(&scope)?;
    validate_key(&key)?;
    let mut conn = state.connection().await?;
    let entry_key = entry_key(&state.prefix, &env, &scope, &key);
    let raw: Option<String> = conn.get(&entry_key).await.map_err(|error| {
        ServiceError::Internal(format!("redis GET {entry_key} failed: {error}"))
    })?;
    let Some(raw) = raw else {
        return Err(ServiceError::NotFound(format!(
            "no entry for env={env_label} scope={scope} key={key}"
        )));
    };
    let entry: RuntimeConfigEntry = serde_json::from_str(&raw)
        .map_err(|error| ServiceError::Internal(format!("entry parse failed: {error}")))?;
    Ok(Json(entry))
}

async fn upsert_entry_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RuntimeConfigUpsertRequest>,
) -> Result<Json<RuntimeConfigEntry>, ServiceError> {
    require_server_auth(&state, &headers)?;
    let reason = body.reason.clone();
    let actor = headers
        .get("x-actor")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());
    let env = body.env.clone();
    let entry = upsert_entry(&state, body, reason.as_deref(), actor.as_deref()).await?;
    tokio::spawn({
        let state = state.clone();
        async move {
            push_to_env(&state, &env, RuntimeConfigApplyReason::Manual).await;
        }
    });
    Ok(Json(entry))
}

async fn delete_entry_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((env_label, scope, key)): Path<(String, String, String)>,
) -> Result<Json<Value>, ServiceError> {
    require_server_auth(&state, &headers)?;
    let env = parse_env(&env_label)?;
    let removed = delete_entry(&state, &env, &scope, &key).await?;
    if removed {
        tokio::spawn({
            let state = state.clone();
            async move {
                push_to_env(&state, &env, RuntimeConfigApplyReason::Manual).await;
            }
        });
    }
    Ok(Json(json!({ "ok": true, "removed": removed })))
}

async fn list_subscribers(
    State(state): State<AppState>,
    Path(env_label): Path<String>,
) -> Result<Json<Value>, ServiceError> {
    let env = parse_env(&env_label)?;
    let subs = load_subscribers(&state, &env).await?;
    Ok(Json(
        json!({ "env": env_label, "subscribers": subs, "count": subs.len() }),
    ))
}

async fn register_subscriber(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RuntimeConfigRegisterRequest>,
) -> Result<Json<RuntimeConfigSubscriber>, ServiceError> {
    require_server_auth(&state, &headers)?;
    validate_subscriber_name(&body.name)?;
    validate_scope(&body.scope)?;
    validate_apply_url(&state, &body.apply_url)?;
    let subscriber = RuntimeConfigSubscriber {
        env: body.env.clone(),
        name: body.name.clone(),
        scope: body.scope.clone(),
        apply_url: body.apply_url.clone(),
        registered_at: Some(iso_now()),
        last_push_at: None,
        last_push_ok: None,
        last_push_error: None,
        last_applied_version: None,
        labels: body.labels.clone(),
    };
    store_subscriber(&state, &subscriber).await?;
    // Fire an initial push so the new subscriber starts at the current snapshot.
    let push_state = state.clone();
    let env = subscriber.env.clone();
    tokio::spawn(async move {
        push_to_env(&push_state, &env, RuntimeConfigApplyReason::Register).await;
    });
    Ok(Json(subscriber))
}

async fn delete_subscriber_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((env_label, name)): Path<(String, String)>,
) -> Result<Json<Value>, ServiceError> {
    require_server_auth(&state, &headers)?;
    let env = parse_env(&env_label)?;
    let removed = delete_subscriber(&state, &env, &name).await?;
    Ok(Json(json!({ "ok": true, "removed": removed })))
}

async fn push_now(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(env_label): Path<String>,
) -> Result<Json<Value>, ServiceError> {
    require_server_auth(&state, &headers)?;
    let env = parse_env(&env_label)?;
    let outcomes = push_to_env(&state, &env, RuntimeConfigApplyReason::Admin).await;
    Ok(Json(
        json!({ "ok": true, "env": env_label, "outcomes": outcomes }),
    ))
}

// ---------- Admin UI ----------

async fn admin_root() -> impl IntoResponse {
    axum::response::Redirect::to("/admin?env=stage")
}

#[derive(Deserialize)]
struct AdminQuery {
    env: Option<String>,
}

async fn admin_page(
    State(state): State<AppState>,
    Query(query): Query<AdminQuery>,
) -> Result<Html<String>, ServiceError> {
    let env_label = query.env.unwrap_or_else(|| "stage".to_string());
    let env = parse_env(&env_label)?;
    let entries = load_entries(&state, &env, None).await?;
    let subscribers = load_subscribers(&state, &env).await?;
    Ok(Html(
        render_admin_page(&env, &entries, &subscribers).into_string(),
    ))
}

fn render_admin_page(
    env: &RuntimeConfigEnv,
    entries: &[RuntimeConfigEntry],
    subscribers: &[RuntimeConfigSubscriber],
) -> Markup {
    let env_label = env_token(env);
    let other_env = match env {
        RuntimeConfigEnv::Stage => "prod",
        RuntimeConfigEnv::Prod => "stage",
    };
    html! {
        (DOCTYPE)
        html {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "dd-runtime-config — " (env_label) }
                style { (PreEscaped(ADMIN_CSS)) }
            }
            body {
                header.topbar {
                    h1 { "dd-runtime-config" }
                    nav.envs {
                        a.env-pill.env-current[true] href={ "?env=" (env_label) } { (env_label) }
                        a.env-pill href={ "?env=" (other_env) } { (other_env) }
                    }
                    form method="post" action={ "/admin/push/" (env_label) } class="push-now" {
                        button type="submit" { "Push now to all " (env_label) " subscribers" }
                    }
                }
                section.entries {
                    h2 { "Entries (" (entries.len()) ")" }
                    table {
                        thead {
                            tr {
                                th { "scope" }
                                th { "key" }
                                th { "value" }
                                th { "version" }
                                th { "updated" }
                                th {}
                            }
                        }
                        tbody {
                            @for entry in entries {
                                tr {
                                    td.scope { (entry.scope) }
                                    td.key { (entry.key) }
                                    td.value {
                                        pre { (entry.value.as_ref().map(|value| serde_json::to_string_pretty(value).unwrap_or_default()).unwrap_or_default()) }
                                    }
                                    td.version { "v" (entry.version) }
                                    td.updated { (entry.updated_at) }
                                    td.actions {
                                        form method="post" action={ "/entries/" (env_label) "/" (entry.scope) "/" (entry.key) "/delete" } {
                                            button type="submit" class="danger" { "delete" }
                                        }
                                    }
                                }
                            }
                            @if entries.is_empty() {
                                tr { td colspan="6" class="empty" { "no entries yet — add one below" } }
                            }
                        }
                    }
                }

                section.upsert {
                    h2 { "Add or update an entry" }
                    form method="post" action={ "/admin/upsert" } class="upsert-form" {
                        input type="hidden" name="env" value=(env_label);
                        label {
                            span { "scope" }
                            input type="text" name="scope" placeholder="dd-remote-web-home" required;
                        }
                        label {
                            span { "key" }
                            input type="text" name="key" placeholder="HOME_PAGE_BANNER_TEXT" required;
                        }
                        label.value {
                            span { "value (JSON; bare strings auto-quoted)" }
                            textarea name="value" rows="6" placeholder="{ \"enabled\": true, \"text\": \"hello\" }" {}
                        }
                        label {
                            span { "reason (optional)" }
                            input type="text" name="reason" placeholder="why this change";
                        }
                        button type="submit" { "save and push" }
                    }
                }

                section.subscribers {
                    h2 { "Subscribers (" (subscribers.len()) ")" }
                    table {
                        thead {
                            tr {
                                th { "name" }
                                th { "scope" }
                                th { "applyUrl" }
                                th { "last push" }
                                th { "last status" }
                                th {}
                            }
                        }
                        tbody {
                            @for sub in subscribers {
                                tr {
                                    td.name { (sub.name) }
                                    td.scope { (sub.scope) }
                                    td.apply-url { code { (sub.apply_url) } }
                                    td.last-push { (sub.last_push_at.clone().unwrap_or_else(|| "—".to_string())) }
                                    td.last-status {
                                        @if let Some(ok) = sub.last_push_ok {
                                            @if ok { span.ok { "ok v" (sub.last_applied_version.unwrap_or(0)) } }
                                            @else { span.err { (sub.last_push_error.clone().unwrap_or_else(|| "error".to_string())) } }
                                        } @else {
                                            span.pending { "pending" }
                                        }
                                    }
                                    td.actions {
                                        form method="post" action={ "/subscribers/" (env_label) "/" (sub.name) "/delete" } {
                                            button type="submit" class="danger" { "delete" }
                                        }
                                    }
                                }
                            }
                            @if subscribers.is_empty() {
                                tr { td colspan="6" class="empty" { "no subscribers yet — services register at boot via POST /subscribers" } }
                            }
                        }
                    }
                }
            }
        }
    }
}

const ADMIN_CSS: &str = r#"
body { background: #0d1117; color: #e6edf3; font-family: ui-sans-serif, system-ui, sans-serif; margin: 0; padding: 0 24px 48px; }
.topbar { display: flex; gap: 16px; align-items: center; padding: 16px 0; border-bottom: 1px solid #21262d; flex-wrap: wrap; }
h1 { font-size: 18px; margin: 0; }
.envs { display: flex; gap: 8px; }
.env-pill { padding: 6px 12px; border-radius: 999px; background: #161b22; color: #c9d1d9; text-decoration: none; border: 1px solid #30363d; font-size: 13px; }
.env-pill.env-current { background: #1f6feb; color: #fff; border-color: #1f6feb; }
.push-now button { background: #238636; color: white; border: none; padding: 8px 14px; border-radius: 6px; font-weight: 600; cursor: pointer; }
.push-now button:hover { background: #2ea043; }
section { margin-top: 24px; }
h2 { font-size: 14px; text-transform: uppercase; letter-spacing: 0.05em; color: #8b949e; margin-bottom: 12px; }
table { width: 100%; border-collapse: collapse; background: #0d1117; border: 1px solid #21262d; border-radius: 8px; overflow: hidden; }
thead { background: #161b22; }
th, td { text-align: left; padding: 8px 12px; border-bottom: 1px solid #21262d; vertical-align: top; font-size: 13px; }
td.value pre { background: #161b22; padding: 8px; border-radius: 4px; max-height: 200px; overflow: auto; margin: 0; font-size: 12px; }
td.actions form { margin: 0; }
button.danger { background: transparent; color: #f85149; border: 1px solid #f85149; padding: 4px 10px; border-radius: 4px; cursor: pointer; font-size: 12px; }
button.danger:hover { background: #f85149; color: #0d1117; }
td.empty { text-align: center; color: #8b949e; padding: 16px; }
.upsert-form { display: grid; grid-template-columns: repeat(2, 1fr); gap: 12px; max-width: 720px; }
.upsert-form label { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: #8b949e; }
.upsert-form label.value { grid-column: 1 / -1; }
.upsert-form input, .upsert-form textarea { background: #161b22; border: 1px solid #30363d; border-radius: 4px; color: #e6edf3; padding: 8px; font-family: ui-monospace, monospace; font-size: 13px; }
.upsert-form button { grid-column: 1 / -1; background: #1f6feb; color: white; border: none; padding: 10px; border-radius: 6px; cursor: pointer; font-weight: 600; }
.upsert-form button:hover { background: #388bfd; }
span.ok { color: #3fb950; }
span.err { color: #f85149; }
span.pending { color: #8b949e; }
code { background: #161b22; padding: 2px 6px; border-radius: 3px; font-size: 12px; }
"#;

// ---------- HTML form bridges (so the admin page works without JS) ----------

#[derive(Deserialize)]
struct AdminUpsertForm {
    env: String,
    scope: String,
    key: String,
    value: Option<String>,
    reason: Option<String>,
}

async fn admin_upsert(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Form(form): axum::extract::Form<AdminUpsertForm>,
) -> Result<Response, ServiceError> {
    require_admin_auth(&state, &headers)?;
    let env = parse_env(&form.env)?;
    let raw_value = form.value.unwrap_or_default();
    let parsed_value = parse_user_value(&raw_value)?;
    let upsert = RuntimeConfigUpsertRequest {
        env: env.clone(),
        scope: form.scope.clone(),
        key: form.key.clone(),
        value: Some(parsed_value),
        labels: None,
        reason: form.reason.clone(),
    };
    let actor = headers
        .get("x-actor")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());
    upsert_entry(&state, upsert, form.reason.as_deref(), actor.as_deref()).await?;
    tokio::spawn({
        let state = state.clone();
        async move {
            push_to_env(&state, &env, RuntimeConfigApplyReason::Admin).await;
        }
    });
    Ok(axum::response::Redirect::to(&format!("/admin?env={}", form.env)).into_response())
}

fn parse_user_value(raw: &str) -> Result<Value, ServiceError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Value::Null);
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => Ok(value),
        Err(_) => Ok(Value::String(trimmed.to_string())),
    }
}

async fn admin_delete_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((env_label, scope, key)): Path<(String, String, String)>,
) -> Result<Response, ServiceError> {
    require_admin_auth(&state, &headers)?;
    let env = parse_env(&env_label)?;
    delete_entry(&state, &env, &scope, &key).await?;
    tokio::spawn({
        let state = state.clone();
        let env = env.clone();
        async move {
            push_to_env(&state, &env, RuntimeConfigApplyReason::Admin).await;
        }
    });
    Ok(axum::response::Redirect::to(&format!("/admin?env={env_label}")).into_response())
}

async fn admin_delete_subscriber(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((env_label, name)): Path<(String, String)>,
) -> Result<Response, ServiceError> {
    require_admin_auth(&state, &headers)?;
    let env = parse_env(&env_label)?;
    delete_subscriber(&state, &env, &name).await?;
    Ok(axum::response::Redirect::to(&format!("/admin?env={env_label}")).into_response())
}

async fn admin_push_now(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(env_label): Path<String>,
) -> Result<Response, ServiceError> {
    require_admin_auth(&state, &headers)?;
    let env = parse_env(&env_label)?;
    push_to_env(&state, &env, RuntimeConfigApplyReason::Admin).await;
    Ok(axum::response::Redirect::to(&format!("/admin?env={env_label}")).into_response())
}

async fn api_docs_html() -> Html<&'static str> {
    Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

// ---------- Router ----------

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        // JSON API: entries
        .route(
            "/entries/:env",
            get(list_entries).post(upsert_entry_handler),
        )
        .route(
            "/entries/:env/:scope/:key",
            get(get_entry).delete(delete_entry_handler),
        )
        // JSON API: subscribers
        .route("/subscribers/:env", get(list_subscribers))
        .route("/subscribers", post(register_subscriber))
        .route("/subscribers/:env/:name", delete(delete_subscriber_handler))
        // JSON API: snapshot pull (short-lived consumers; rest-api proxies this)
        .route("/snapshot/:env", get(list_entries))
        // JSON push (curl/dev-loop friendly); HTML form posts to /admin/push/:env below.
        .route("/push/:env", post(push_now))
        // Admin UI (HTML form posts, redirects back to /admin?env=...)
        .route("/", get(admin_root))
        .route("/admin", get(admin_page))
        .route("/admin/upsert", post(admin_upsert))
        .route("/admin/push/:env", post(admin_push_now))
        .route("/entries/:env/:scope/:key/delete", post(admin_delete_entry))
        .route(
            "/subscribers/:env/:name/delete",
            post(admin_delete_subscriber),
        )
        .with_state(state)
}

// ---------- main ----------

fn read_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

fn read_bool_env(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

#[tokio::main]
async fn main() {
    let redis_url = read_env(ENV_REDIS_URL)
        .or_else(|| read_env(ENV_REDIS_URL_FALLBACK))
        .unwrap_or_else(|| "redis://dd-redis-cache.default.svc.cluster.local:6379".to_string());
    let prefix = read_env(ENV_REDIS_PREFIX).unwrap_or_else(|| DEFAULT_PREFIX.to_string());
    let push_interval = Duration::from_secs(
        read_env(ENV_PUSH_INTERVAL)
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_PUSH_INTERVAL_SECS),
    );
    let push_timeout = Duration::from_secs(
        read_env(ENV_PUSH_TIMEOUT)
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_PUSH_TIMEOUT_SECS),
    );

    let redis_client = redis::Client::open(redis_url.clone())
        .unwrap_or_else(|error| panic!("invalid redis url {redis_url}: {error}"));
    let http = reqwest::Client::builder()
        .timeout(push_timeout)
        .build()
        .expect("failed to build reqwest client");

    let state = AppState {
        redis: redis_client,
        redis_conn: Arc::new(Mutex::new(None)),
        http,
        prefix,
        server_secret: read_env(ENV_SERVER_SECRET),
        admin_secret: read_env(ENV_ADMIN_SECRET),
        allow_unauthenticated: read_bool_env(ENV_ALLOW_UNAUTHENTICATED),
        allow_external_subscribers: read_bool_env(ENV_ALLOW_EXTERNAL_SUBSCRIBERS),
        push_timeout,
    };

    tokio::spawn(run_push_loop(state.clone(), push_interval));

    let host = read_env(ENV_BIND_HOST).unwrap_or_else(|| "0.0.0.0".to_string());
    let port = read_env(ENV_BIND_PORT)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8110);
    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("invalid runtime-config bind address");
    println!("[dd-runtime-config] listening on http://{address}");

    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("failed to bind runtime-config listener");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("runtime-config server crashed");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
            let _ = sigterm.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
