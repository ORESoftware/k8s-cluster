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

#[cfg(all(feature = "axum-07", feature = "axum-08"))]
compile_error!("features axum-07 and axum-08 are mutually exclusive");
#[cfg(not(any(feature = "axum-07", feature = "axum-08")))]
compile_error!("enable exactly one of axum-07 or axum-08");
#[cfg(all(feature = "openapi", not(feature = "axum-08")))]
compile_error!("feature openapi requires axum-08");

#[cfg(feature = "axum-07")]
extern crate axum07 as axum;
#[cfg(feature = "axum-08")]
extern crate axum08 as axum;

use std::collections::HashMap;
use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use dd_shared_interfaces::{
    RuntimeConfigApplyReason, RuntimeConfigApplyRequest, RuntimeConfigApplyResponse,
    RuntimeConfigEnv, RuntimeConfigRegisterRequest,
};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
#[cfg(feature = "openapi")]
use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
#[cfg(feature = "openapi")]
use utoipa::openapi::{Components, OpenApi};
#[cfg(feature = "openapi")]
use utoipa_axum::{router::OpenApiRouter, routes};

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

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RuntimeConfigSnapshotResponse {
    service: Option<String>,
    scope: Option<String>,
    env: Option<String>,
    snapshot_version: i64,
    applied_at: Option<String>,
    entries: HashMap<String, Value>,
    last_push_id: Option<String>,
    last_reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RuntimeConfigResetResponse {
    ok: bool,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RuntimeConfigErrorResponse {
    ok: bool,
    error: String,
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
#[cfg(not(feature = "openapi"))]
pub fn router() -> Router {
    Router::new()
        .route(SNAPSHOT_ROUTE_PATH, get(handle_get))
        .route(APPLY_ROUTE_PATH, post(handle_apply))
        .route(RESET_ROUTE_PATH, post(handle_reset))
        .with_state(global_store().clone())
}

/// Returns the executable router together with the OpenAPI fragment collected
/// from the exact same route declarations. Hosts merge both results into their
/// own runtime router and service contract.
#[cfg(feature = "openapi")]
pub fn router_and_openapi() -> (Router, OpenApi) {
    router_and_openapi_with_store(global_store().clone())
}

#[cfg(feature = "openapi")]
pub fn router_and_openapi_with_store(store: RuntimeConfigStore) -> (Router, OpenApi) {
    let (router, mut openapi) = OpenApiRouter::new()
        .routes(routes!(handle_get))
        .routes(routes!(handle_apply))
        .routes(routes!(handle_reset))
        .split_for_parts();

    let components = openapi.components.get_or_insert_with(Components::new);
    components.add_security_scheme(
        "runtime_config_server_auth",
        SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
            "x-server-auth",
            "Shared server-to-server secret configured through RUNTIME_CONFIG_SERVER_SECRET.",
        ))),
    );

    (router.with_state(store), openapi)
}

#[cfg(feature = "openapi")]
pub fn router() -> Router {
    router_and_openapi().0
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

fn runtime_config_env_from_label(value: &str) -> RuntimeConfigEnv {
    match value.trim().to_ascii_lowercase().as_str() {
        "prod" => RuntimeConfigEnv::Prod,
        _ => RuntimeConfigEnv::Stage,
    }
}

fn apply_reason_label(reason: &RuntimeConfigApplyReason) -> &'static str {
    match reason {
        RuntimeConfigApplyReason::Cron => "cron",
        RuntimeConfigApplyReason::Admin => "admin",
        RuntimeConfigApplyReason::Register => "register",
        RuntimeConfigApplyReason::Manual => "manual",
        RuntimeConfigApplyReason::Initial => "initial",
    }
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

fn error_response(status: StatusCode, error: &str) -> Response {
    (
        status,
        Json(RuntimeConfigErrorResponse {
            ok: false,
            error: error.to_string(),
        }),
    )
        .into_response()
}

fn require_server_auth(store: &RuntimeConfigStore, headers: &HeaderMap) -> Result<(), Response> {
    let Some(expected) = store.server_secret.as_ref() else {
        if store.allow_unauthenticated {
            return Ok(());
        }
        return Err(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "runtime config auth is not configured",
        ));
    };
    let provided = headers
        .get("x-server-auth")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if provided.is_empty() || !constant_time_eq(provided, expected.as_str()) {
        return Err(error_response(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok(())
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/internal/runtime-config",
        operation_id = "getRuntimeConfigSnapshot",
        tag = "runtime-config",
        security(("runtime_config_server_auth" = [])),
        responses(
            (status = 200, description = "Current in-process runtime configuration snapshot", body = RuntimeConfigSnapshotResponse),
            (status = 401, description = "Missing or invalid server authentication", body = RuntimeConfigErrorResponse)
        )
    )
)]
async fn handle_get(State(store): State<RuntimeConfigStore>, headers: HeaderMap) -> Response {
    // The snapshot lists every pushed entry value, so once a push secret is
    // configured the read side must present it too. Without a configured
    // secret the route stays open (local development) — only the mutating
    // routes hard-fail when auth is unconfigured.
    if store.server_secret.is_some() {
        if let Err(response) = require_server_auth(&store, &headers) {
            return response;
        }
    }
    let state = store.inner.read().await;
    Json(RuntimeConfigSnapshotResponse {
        service: read_env(ENV_SERVICE_NAME),
        scope: read_env(ENV_SCOPE),
        env: read_env(ENV_ENV),
        snapshot_version: state.snapshot_version,
        applied_at: state.applied_at.clone(),
        entries: state.entries.clone(),
        last_push_id: state.last_push_id.clone(),
        last_reason: state.last_reason.clone(),
    })
    .into_response()
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/internal/update-runtime-config",
        operation_id = "applyRuntimeConfigSnapshot",
        tag = "runtime-config",
        security(("runtime_config_server_auth" = [])),
        request_body = RuntimeConfigApplyRequest,
        responses(
            (status = 200, description = "Snapshot applied or acknowledged as stale", body = RuntimeConfigApplyResponse),
            (status = 401, description = "Missing or invalid server authentication", body = RuntimeConfigErrorResponse),
            (status = 503, description = "Runtime-config authentication is not configured", body = RuntimeConfigErrorResponse)
        )
    )
)]
async fn handle_apply(
    State(store): State<RuntimeConfigStore>,
    headers: HeaderMap,
    Json(body): Json<RuntimeConfigApplyRequest>,
) -> Response {
    if let Err(response) = require_server_auth(&store, &headers) {
        return response;
    }
    let new_version = body.snapshot.snapshot_version;
    let mut entries: HashMap<String, Value> = HashMap::new();
    for entry in body.snapshot.entries {
        entries.insert(entry.key, entry.value.unwrap_or(Value::Null));
    }
    let applied_at = iso_now();
    let previous_version;
    {
        let mut state = store.inner.write().await;
        previous_version = state.snapshot_version;
        if new_version < previous_version {
            return Json(RuntimeConfigApplyResponse {
                ok: true,
                service: read_env(ENV_SERVICE_NAME).unwrap_or_else(|| "unknown".to_string()),
                applied_at: state.applied_at.clone().unwrap_or_else(iso_now),
                applied_version: previous_version,
                previous_version: Some(previous_version),
                stale: Some(true),
                ignored_version: Some(new_version),
                errors: None,
            })
            .into_response();
        }
        state.snapshot_version = new_version;
        state.applied_at = Some(applied_at.clone());
        state.entries = entries;
        state.last_push_id = Some(body.push_id.clone());
        state.last_reason = Some(apply_reason_label(&body.reason).to_string());
    }
    Json(RuntimeConfigApplyResponse {
        ok: true,
        service: read_env(ENV_SERVICE_NAME).unwrap_or_else(|| "unknown".to_string()),
        applied_at,
        applied_version: new_version,
        previous_version: Some(previous_version),
        stale: None,
        ignored_version: None,
        errors: None,
    })
    .into_response()
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/internal/runtime-config/reset",
        operation_id = "resetRuntimeConfigSnapshot",
        tag = "runtime-config",
        security(("runtime_config_server_auth" = [])),
        responses(
            (status = 200, description = "Runtime overrides cleared", body = RuntimeConfigResetResponse),
            (status = 401, description = "Missing or invalid server authentication", body = RuntimeConfigErrorResponse),
            (status = 503, description = "Runtime-config authentication is not configured", body = RuntimeConfigErrorResponse)
        )
    )
)]
async fn handle_reset(State(store): State<RuntimeConfigStore>, headers: HeaderMap) -> Response {
    if let Err(response) = require_server_auth(&store, &headers) {
        return response;
    }
    let mut state = store.inner.write().await;
    *state = RuntimeConfigState::default();
    Json(RuntimeConfigResetResponse { ok: true }).into_response()
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

    // Never follow redirects: this POST carries the X-Server-Auth secret, and
    // reqwest only strips the standard Authorization/Cookie headers on a
    // cross-origin redirect — a custom header like X-Server-Auth would be
    // forwarded to the redirect target, leaking the control-plane secret.
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!("[runtime-config] failed to build http client: {error}");
            return;
        }
    };

    let body = RuntimeConfigRegisterRequest {
        env: runtime_config_env_from_label(&env_label),
        name: service_name,
        scope,
        apply_url,
        labels: None,
    };
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

#[cfg(all(test, feature = "openapi"))]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_store() -> RuntimeConfigStore {
        RuntimeConfigStore {
            inner: Arc::new(RwLock::new(RuntimeConfigState::default())),
            server_secret: None,
            allow_unauthenticated: true,
        }
    }

    #[test]
    fn openapi_fragment_matches_runtime_route_inventory() {
        let (_, openapi) = router_and_openapi_with_store(test_store());
        let value = serde_json::to_value(openapi).expect("serialize OpenAPI");
        let paths = value["paths"].as_object().expect("OpenAPI paths");
        assert_eq!(paths.len(), route_paths().len());
        for (path, method) in route_paths() {
            assert!(paths.contains_key(path), "missing OpenAPI path {path}");
            assert!(
                paths[path]
                    .as_object()
                    .expect("path item")
                    .contains_key(&method.to_ascii_lowercase()),
                "missing OpenAPI operation {method} {path}",
            );
        }
        assert_eq!(
            value["components"]["securitySchemes"]["runtime_config_server_auth"]["name"],
            "x-server-auth",
        );
    }

    #[tokio::test]
    async fn executable_router_serves_all_declared_routes() {
        let (router, _) = router_and_openapi_with_store(test_store());

        let snapshot = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(SNAPSHOT_ROUTE_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("snapshot response");
        assert_eq!(snapshot.status(), StatusCode::OK);
        let snapshot_body = to_bytes(snapshot.into_body(), 1024 * 1024)
            .await
            .expect("snapshot body");
        let snapshot_json: Value = serde_json::from_slice(&snapshot_body).expect("snapshot JSON");
        assert_eq!(snapshot_json["snapshotVersion"], 0);

        let reset = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(RESET_ROUTE_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("reset response");
        assert_eq!(reset.status(), StatusCode::OK);
    }
}
