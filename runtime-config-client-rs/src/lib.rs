// Receiver helper for the dd-runtime-config control plane.
//
// This crate is intentionally tiny and dependency-light so every dd service
// can adopt it with two lines: one in Cargo.toml, one in main.rs.
//
// Exposed surface mounted under the service's own axum Router via
// `.merge(runtime_config_client::router())`:
//
//   GET  /internal/runtime-config         — what this process currently has
//   POST /internal/update-runtime-config  — accept a new snapshot (PUSH path)
//   POST /internal/runtime-config/reset   — drop all runtime overrides
//
// Mutating routes require `X-Server-Auth: $RUNTIME_CONFIG_SERVER_SECRET`.
// Local unauthenticated development must opt in explicitly with
// `RUNTIME_CONFIG_ALLOW_UNAUTHENTICATED=true`.
//
// Hosts that want to register with the control plane should spawn
// `tokio::spawn(register_with_control_plane())` once during startup.

use std::collections::HashMap;
use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;

const ENV_SERVICE_NAME: &str = "RUNTIME_CONFIG_SERVICE_NAME";
const ENV_SCOPE: &str = "RUNTIME_CONFIG_SCOPE";
const ENV_ENV: &str = "RUNTIME_CONFIG_ENV";
const ENV_REGISTER_URL: &str = "RUNTIME_CONFIG_REGISTER_URL";
const ENV_APPLY_URL: &str = "RUNTIME_CONFIG_APPLY_URL";
const ENV_SERVER_SECRET: &str = "RUNTIME_CONFIG_SERVER_SECRET";
const ENV_ALLOW_UNAUTHENTICATED: &str = "RUNTIME_CONFIG_ALLOW_UNAUTHENTICATED";
const REGISTER_BACKOFF_SECS: u64 = 15;
const REGISTER_MAX_BACKOFF_SECS: u64 = 300;

/// Canonical apply route path. Hosts that want to advertise it in generated
/// API docs should reference this constant.
pub const APPLY_ROUTE_PATH: &str = "/internal/update-runtime-config";
pub const SNAPSHOT_ROUTE_PATH: &str = "/internal/runtime-config";
pub const RESET_ROUTE_PATH: &str = "/internal/runtime-config/reset";

#[derive(Default)]
struct RuntimeConfigState {
    snapshot_version: i64,
    applied_at: Option<String>,
    entries: HashMap<String, Value>,
    last_push_id: Option<String>,
    last_reason: Option<String>,
}

#[derive(Clone)]
pub struct RuntimeConfigStore {
    inner: Arc<RwLock<RuntimeConfigState>>,
    server_secret: Option<String>,
    allow_unauthenticated: bool,
}

impl Default for RuntimeConfigStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeConfigStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(RuntimeConfigState::default())),
            server_secret: read_env(ENV_SERVER_SECRET),
            allow_unauthenticated: read_bool_env(ENV_ALLOW_UNAUTHENTICATED),
        }
    }

    pub async fn get(&self, key: &str) -> Option<Value> {
        self.inner.read().await.entries.get(key).cloned()
    }

    pub async fn snapshot_version(&self) -> i64 {
        self.inner.read().await.snapshot_version
    }
}

/// Process-wide singleton so hosts can read live config without having to
/// thread the store through their own state. Initialised on first access by
/// `router()`.
fn global_store() -> &'static RuntimeConfigStore {
    static STORE: OnceLock<RuntimeConfigStore> = OnceLock::new();
    STORE.get_or_init(RuntimeConfigStore::new)
}

/// Look up a currently-applied entry value, if any. Returns `None` when the
/// key hasn't been pushed yet.
pub async fn get_entry(key: &str) -> Option<Value> {
    global_store().get(key).await
}

/// Snapshot version (sum of every entry's version) currently held by this
/// process. Zero until the first push lands.
pub async fn snapshot_version() -> i64 {
    global_store().snapshot_version().await
}

/// Returns the configured push-auth secret, if any. Hosts can use this to
/// align their own pre-handler with the helper's expectations.
pub fn server_secret() -> Option<String> {
    read_env(ENV_SERVER_SECRET)
}

/// Returns an axum Router with the three /internal/runtime-config* routes
/// mounted. Merge it into the host service's Router.
pub fn router() -> Router {
    Router::new()
        .route(SNAPSHOT_ROUTE_PATH, get(handle_get))
        .route(APPLY_ROUTE_PATH, post(handle_apply))
        .route(RESET_ROUTE_PATH, post(handle_reset))
        .with_state(global_store().clone())
}

fn read_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

fn read_bool_env(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in a.iter().zip(b.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

fn require_server_auth(store: &RuntimeConfigStore, headers: &HeaderMap) -> Result<(), Response> {
    let Some(expected) = store.server_secret.as_ref() else {
        if store.allow_unauthenticated {
            return Ok(());
        }
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "ok": false,
                "error": "runtime config auth is not configured"
            })),
        )
            .into_response());
    };
    let provided = headers
        .get("x-server-auth")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if provided.is_empty() || !constant_time_eq(provided, expected.as_str()) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "ok": false, "error": "unauthorized" })),
        )
            .into_response());
    }
    Ok(())
}

async fn handle_get(State(store): State<RuntimeConfigStore>) -> impl IntoResponse {
    let state = store.inner.read().await;
    Json(json!({
        "service": read_env(ENV_SERVICE_NAME),
        "scope": read_env(ENV_SCOPE),
        "env": read_env(ENV_ENV),
        "snapshotVersion": state.snapshot_version,
        "appliedAt": state.applied_at,
        "entries": state.entries,
        "lastPushId": state.last_push_id,
        "lastReason": state.last_reason,
    }))
}

#[derive(Deserialize)]
struct ApplyEntryShape {
    key: Option<String>,
    value: Option<Value>,
}

#[derive(Deserialize)]
struct ApplySnapshotShape {
    #[serde(rename = "snapshotVersion", default)]
    snapshot_version: Option<i64>,
    #[serde(default)]
    entries: Vec<ApplyEntryShape>,
}

#[derive(Deserialize)]
struct ApplyRequestShape {
    #[serde(rename = "pushId", default)]
    push_id: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    snapshot: Option<ApplySnapshotShape>,
}

async fn handle_apply(
    State(store): State<RuntimeConfigStore>,
    headers: HeaderMap,
    Json(body): Json<ApplyRequestShape>,
) -> Response {
    if let Err(response) = require_server_auth(&store, &headers) {
        return response;
    }
    let Some(snapshot) = body.snapshot else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "snapshot is required" })),
        )
            .into_response();
    };
    let new_version = snapshot.snapshot_version.unwrap_or(0);
    let mut entries: HashMap<String, Value> = HashMap::new();
    for entry in snapshot.entries {
        let Some(key) = entry.key else { continue };
        entries.insert(key, entry.value.unwrap_or(Value::Null));
    }
    let applied_at = iso_now();
    let previous_version;
    {
        let mut state = store.inner.write().await;
        previous_version = state.snapshot_version;
        if new_version < previous_version {
            return Json(json!({
                "ok": true,
                "service": read_env(ENV_SERVICE_NAME).unwrap_or_else(|| "unknown".to_string()),
                "appliedAt": state.applied_at,
                "appliedVersion": previous_version,
                "previousVersion": previous_version,
                "stale": true,
                "ignoredVersion": new_version,
            }))
            .into_response();
        }
        state.snapshot_version = new_version;
        state.applied_at = Some(applied_at.clone());
        state.entries = entries;
        state.last_push_id = body.push_id.clone();
        state.last_reason = body.reason.clone();
    }
    Json(json!({
        "ok": true,
        "service": read_env(ENV_SERVICE_NAME).unwrap_or_else(|| "unknown".to_string()),
        "appliedAt": applied_at,
        "appliedVersion": new_version,
        "previousVersion": previous_version,
    }))
    .into_response()
}

async fn handle_reset(State(store): State<RuntimeConfigStore>, headers: HeaderMap) -> Response {
    if let Err(response) = require_server_auth(&store, &headers) {
        return response;
    }
    let mut state = store.inner.write().await;
    *state = RuntimeConfigState::default();
    Json(json!({ "ok": true })).into_response()
}

/// Register this process with the control plane in the background. Safe to
/// call from tokio::spawn during process startup; retries with exponential
/// backoff (capped at 5 min) until success.
pub async fn register_with_control_plane() {
    let Some(register_url) = read_env(ENV_REGISTER_URL) else {
        eprintln!(
            "[runtime-config] {} not set; skipping registration",
            ENV_REGISTER_URL
        );
        return;
    };
    let Some(apply_url) = read_env(ENV_APPLY_URL) else {
        eprintln!(
            "[runtime-config] {} not set; skipping registration",
            ENV_APPLY_URL
        );
        return;
    };
    let Some(service_name) = read_env(ENV_SERVICE_NAME) else {
        eprintln!(
            "[runtime-config] {} not set; skipping registration",
            ENV_SERVICE_NAME
        );
        return;
    };
    let env_label = read_env(ENV_ENV).unwrap_or_else(|| "stage".to_string());
    let scope = read_env(ENV_SCOPE).unwrap_or_else(|| service_name.clone());

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!("[runtime-config] failed to build http client: {error}");
            return;
        }
    };

    let body = json!({
        "env": env_label,
        "name": service_name,
        "scope": scope,
        "applyUrl": apply_url,
    });
    let secret = read_env(ENV_SERVER_SECRET);

    let mut delay = Duration::from_secs(REGISTER_BACKOFF_SECS);
    loop {
        let mut request = client.post(&register_url).json(&body);
        if let Some(secret) = secret.as_ref() {
            request = request.header("x-server-auth", secret.as_str());
        }
        match request.send().await {
            Ok(response) if response.status().is_success() => {
                println!("[runtime-config] registered with control plane at {register_url}");
                return;
            }
            Ok(response) => {
                eprintln!(
                    "[runtime-config] register returned HTTP {}; retrying in {}s",
                    response.status(),
                    delay.as_secs()
                );
            }
            Err(error) => {
                eprintln!(
                    "[runtime-config] register transport error: {error}; retrying in {}s",
                    delay.as_secs()
                );
            }
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(REGISTER_MAX_BACKOFF_SECS));
    }
}

fn iso_now() -> String {
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

/// Convenience: returns the three internal route paths so docs / discovery
/// tools can render them without duplicating the strings.
pub fn route_paths() -> [(&'static str, &'static str); 3] {
    [
        (SNAPSHOT_ROUTE_PATH, "GET"),
        (APPLY_ROUTE_PATH, "POST"),
        (RESET_ROUTE_PATH, "POST"),
    ]
}
