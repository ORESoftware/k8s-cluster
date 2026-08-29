use std::sync::atomic::Ordering;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::{
    dispatch::{dispatch_to_pool, pool_id_from_selector},
    lifecycle::reconcile_pool,
    types::{
        AppState, ContainerStatus, DispatchRequest, HealthResponse, PoolConfig, PoolSummary,
        PoolsResponse, WarmContainer, SERVICE_NAME,
    },
    util::{duration_millis_u64, now_ms},
};

fn request_is_authorized(headers: &HeaderMap, secret: &str) -> bool {
    headers
        .get("x-server-auth")
        .or_else(|| headers.get("x-container-pool-auth"))
        .or_else(|| headers.get("x-agent-auth"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == secret)
}

fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        return Err(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "SERVER_AUTH_SECRET is not configured",
        ));
    };
    if request_is_authorized(headers, secret) {
        Ok(())
    } else {
        Err(json_error(StatusCode::UNAUTHORIZED, "unauthorized"))
    }
}

fn json_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": message }))).into_response()
}

fn pool_summary(config: &PoolConfig, containers: Vec<WarmContainer>) -> PoolSummary {
    let idle_containers = containers
        .iter()
        .filter(|container| container.status == ContainerStatus::Idle)
        .count();
    let busy_containers = containers
        .iter()
        .filter(|container| container.status == ContainerStatus::Busy)
        .count();
    let unhealthy_containers = containers
        .iter()
        .filter(|container| container.status == ContainerStatus::Unhealthy)
        .count();
    PoolSummary {
        id: config.id.clone(),
        slug: config.slug.clone(),
        display_name: config.display_name.clone(),
        image: config.image.clone(),
        request_path: config.request_path.clone(),
        health_path: config.health_path.clone(),
        container_port: config.container_port,
        min_warm: config.min_warm,
        max_warm: config.max_warm,
        max_concurrency_per_container: config.max_concurrency_per_container,
        request_timeout_ms: duration_millis_u64(config.request_timeout),
        idle_ttl_seconds: config.idle_ttl.as_secs(),
        nats_subject: config.nats_subject.clone(),
        env_keys: config.env.keys().cloned().collect(),
        mounts: config
            .mounts
            .iter()
            .map(|mount| {
                format!(
                    "{}:{}:{}",
                    mount.source,
                    mount.target,
                    if mount.read_only { "ro" } else { "rw" }
                )
            })
            .collect(),
        labels: config.labels.clone(),
        active_containers: containers.len(),
        idle_containers,
        busy_containers,
        unhealthy_containers,
        containers,
    }
}

async fn pool_summaries(state: &AppState) -> Vec<PoolSummary> {
    let registry = state.registry.lock().await;
    let mut pools = registry
        .configs
        .values()
        .map(|config| {
            let mut containers = registry
                .containers
                .values()
                .filter(|container| container.pool_id == config.id)
                .cloned()
                .collect::<Vec<_>>();
            containers.sort_by(|a, b| a.name.cmp(&b.name));
            pool_summary(config, containers)
        })
        .collect::<Vec<_>>();
    pools.sort_by(|a, b| a.slug.cmp(&b.slug));
    pools
}

pub(crate) async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let registry = state.registry.lock().await;
    Json(HealthResponse {
        ok: true,
        service: SERVICE_NAME,
        postgres_configured: state.config.database_url.is_some(),
        nats_configured: state.nats.is_some(),
        auth_configured: state.config.server_auth_secret.is_some(),
        pool_count: registry.configs.len(),
        warm_container_count: registry.containers.len(),
        last_config_refresh_ms: registry.last_config_refresh_ms,
        last_config_error: registry.last_config_error.clone(),
    })
}

async fn container_pool_ready(state: &AppState) -> bool {
    let config_ready = {
        let registry = state.registry.lock().await;
        registry.last_config_refresh_ms.is_some() && registry.last_config_error.is_none()
    };
    let nats_ready = state.nats.as_ref().is_some_and(|client| {
        matches!(
            client.connection_state(),
            async_nats::connection::State::Connected
        )
    });
    config_ready
        && nats_ready
        && state.config.server_auth_secret.is_some()
        && std::path::Path::new(&state.config.engine_bin).exists()
}

pub(crate) async fn readyz(State(state): State<AppState>) -> Response {
    let ready = container_pool_ready(&state).await;
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "ok": ready,
            "service": SERVICE_NAME,
            "dependenciesReady": ready,
        })),
    )
        .into_response()
}

pub(crate) async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let registry = state.registry.lock().await;
    let warm = registry.containers.len();
    let idle = registry
        .containers
        .values()
        .filter(|container| container.status == ContainerStatus::Idle)
        .count();
    let busy = registry
        .containers
        .values()
        .filter(|container| container.status == ContainerStatus::Busy)
        .count();
    let unhealthy = registry
        .containers
        .values()
        .filter(|container| container.status == ContainerStatus::Unhealthy)
        .count();
    let mut body = format!(
        "# HELP dd_container_pool_http_requests_total HTTP requests observed by dd-container-pool.\n\
         # TYPE dd_container_pool_http_requests_total counter\n\
         dd_container_pool_http_requests_total {}\n\
         # HELP dd_container_pool_dispatch_total Successful container pool dispatches.\n\
         # TYPE dd_container_pool_dispatch_total counter\n\
         dd_container_pool_dispatch_total {}\n\
         # HELP dd_container_pool_dispatch_failures_total Failed container pool dispatches.\n\
         # TYPE dd_container_pool_dispatch_failures_total counter\n\
         dd_container_pool_dispatch_failures_total {}\n\
         # HELP dd_container_pool_nats_messages_total NATS messages received by the pool service.\n\
         # TYPE dd_container_pool_nats_messages_total counter\n\
         dd_container_pool_nats_messages_total {}\n\
         # HELP dd_container_pool_nats_failures_total NATS dispatch failures.\n\
         # TYPE dd_container_pool_nats_failures_total counter\n\
         dd_container_pool_nats_failures_total {}\n\
         # HELP dd_container_pool_containers_started_total Warm containers started.\n\
         # TYPE dd_container_pool_containers_started_total counter\n\
         dd_container_pool_containers_started_total {}\n\
         # HELP dd_container_pool_containers_removed_total Warm containers removed.\n\
         # TYPE dd_container_pool_containers_removed_total counter\n\
         dd_container_pool_containers_removed_total {}\n\
         # HELP dd_container_pool_containers_unhealthy_total Warm containers retired as unhealthy.\n\
         # TYPE dd_container_pool_containers_unhealthy_total counter\n\
         dd_container_pool_containers_unhealthy_total {}\n\
         # HELP dd_container_pool_config_refresh_total Successful config refreshes.\n\
         # TYPE dd_container_pool_config_refresh_total counter\n\
         dd_container_pool_config_refresh_total {}\n\
         # HELP dd_container_pool_config_refresh_failures_total Failed config refreshes.\n\
         # TYPE dd_container_pool_config_refresh_failures_total counter\n\
         dd_container_pool_config_refresh_failures_total {}\n\
         # HELP dd_container_pool_container_health_checks_total Container health checks attempted.\n\
         # TYPE dd_container_pool_container_health_checks_total counter\n\
         dd_container_pool_container_health_checks_total {}\n\
         # HELP dd_container_pool_container_health_check_failures_total Container health checks failed.\n\
         # TYPE dd_container_pool_container_health_check_failures_total counter\n\
         dd_container_pool_container_health_check_failures_total {}\n\
         # HELP dd_container_pool_warm_containers Current known warm containers.\n\
         # TYPE dd_container_pool_warm_containers gauge\n\
         dd_container_pool_warm_containers {}\n\
         dd_container_pool_idle_containers {}\n\
         dd_container_pool_busy_containers {}\n\
         dd_container_pool_unhealthy_containers {}\n",
        state.metrics.http_requests_total.load(Ordering::Relaxed),
        state.metrics.dispatch_total.load(Ordering::Relaxed),
        state.metrics.dispatch_failures_total.load(Ordering::Relaxed),
        state.metrics.nats_messages_total.load(Ordering::Relaxed),
        state.metrics.nats_failures_total.load(Ordering::Relaxed),
        state.metrics.containers_started_total.load(Ordering::Relaxed),
        state.metrics.containers_removed_total.load(Ordering::Relaxed),
        state
            .metrics
            .containers_unhealthy_total
            .load(Ordering::Relaxed),
        state.metrics.config_refresh_total.load(Ordering::Relaxed),
        state
            .metrics
            .config_refresh_failures_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .container_health_checks_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .container_health_check_failures_total
            .load(Ordering::Relaxed),
        warm,
        idle,
        busy,
        unhealthy
    );
    drop(registry);
    body.push_str(&format!(
        "# HELP dd_container_pool_dependencies_ready Whether config, NATS, auth, and the container engine are available.\n\
         # TYPE dd_container_pool_dependencies_ready gauge\n\
         dd_container_pool_dependencies_ready {}\n",
        u8::from(container_pool_ready(&state).await)
    ));
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
}

pub(crate) async fn list_pools(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    Json(PoolsResponse {
        ok: true,
        generated_at_ms: now_ms(),
        pools: pool_summaries(&state).await,
    })
    .into_response()
}

pub(crate) async fn get_pool(
    State(state): State<AppState>,
    Path(pool): Path<String>,
    headers: HeaderMap,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let summaries = pool_summaries(&state).await;
    if let Some(summary) = summaries
        .into_iter()
        .find(|summary| summary.id == pool || summary.slug == pool)
    {
        Json(json!({ "ok": true, "pool": summary })).into_response()
    } else {
        json_error(StatusCode::NOT_FOUND, "unknown container pool")
    }
}

pub(crate) async fn warm_pool(
    State(state): State<AppState>,
    Path(pool): Path<String>,
    headers: HeaderMap,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let pool_id = {
        let registry = state.registry.lock().await;
        match pool_id_from_selector(&registry, &pool) {
            Some(pool_id) => pool_id,
            None => return json_error(StatusCode::NOT_FOUND, "unknown container pool"),
        }
    };
    match reconcile_pool(&state, &pool_id).await {
        Ok(()) => Json(json!({ "ok": true, "pool": pool, "pools": pool_summaries(&state).await }))
            .into_response(),
        Err(error) => json_error(StatusCode::BAD_GATEWAY, &error),
    }
}

pub(crate) async fn dispatch_pool(
    State(state): State<AppState>,
    Path(pool): Path<String>,
    headers: HeaderMap,
    Json(request): Json<DispatchRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    match dispatch_to_pool(&state, &pool, request).await {
        Ok(response) => {
            let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::OK);
            (status, Json(response)).into_response()
        }
        Err(error) => json_error(StatusCode::BAD_GATEWAY, &error),
    }
}

pub(crate) async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

pub(crate) async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}
