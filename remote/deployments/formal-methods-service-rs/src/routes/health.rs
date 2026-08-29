//! Liveness / readiness endpoints.

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::state::AppState;

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct HealthResponse {
    pub status: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ReadyRepoAllowlist {
    pub allow_all: bool,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ReadyPathFilter {
    pub active: bool,
    pub prefixes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ReadyDeliveryDedupe {
    pub tracked: usize,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ReadyResponse {
    pub status: String,
    pub analyzers: Vec<String>,
    pub github_token_configured: bool,
    pub repo_allowlist: ReadyRepoAllowlist,
    pub path_filter: ReadyPathFilter,
    pub delivery_dedupe: ReadyDeliveryDedupe,
}

#[utoipa::path(
    get,
    path = "/health",
    operation_id = "getFormalMethodsHealth",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Process is alive", body = HealthResponse))
)]
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
    })
}

#[utoipa::path(
    get,
    path = "/ready",
    operation_id = "getFormalMethodsReadiness",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Current analyzer and ingress readiness state", body = ReadyResponse))
)]
pub async fn ready(State(state): State<AppState>) -> Json<ReadyResponse> {
    let analyzers = state
        .pipeline
        .analyzer_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    let dedupe_len = state.delivery_dedupe.lock().map(|g| g.len()).unwrap_or(0);

    Json(ReadyResponse {
        status: "ok".to_string(),
        analyzers,
        github_token_configured: state.github.has_token(),
        repo_allowlist: ReadyRepoAllowlist {
            allow_all: state.repo_allowlist.allow_all(),
        },
        path_filter: ReadyPathFilter {
            active: !state.path_filter.is_empty(),
            prefixes: state.path_filter.prefixes().to_vec(),
        },
        delivery_dedupe: ReadyDeliveryDedupe {
            tracked: dedupe_len,
        },
    })
}

#[utoipa::path(
    get,
    path = "/metrics",
    operation_id = "getFormalMethodsPrometheusMetrics",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Prometheus text exposition", body = String, content_type = "text/plain"))
)]
pub async fn metrics(State(state): State<AppState>) -> Response {
    let dedupe_len = state.delivery_dedupe.lock().map(|g| g.len()).unwrap_or(0);
    let path_filter_active = u8::from(!state.path_filter.is_empty());
    let repo_allowlist_allow_all = u8::from(state.repo_allowlist.allow_all());
    let github_token_configured = u8::from(state.github.has_token());
    let body = format!(
        concat!(
            "# HELP dd_formal_methods_service_build_info Formal methods service build metadata.\n",
            "# TYPE dd_formal_methods_service_build_info gauge\n",
            "dd_formal_methods_service_build_info{{service=\"dd-formal-methods-service\"}} 1\n",
            "# HELP dd_formal_methods_service_max_concurrent_analyses Configured analysis concurrency.\n",
            "# TYPE dd_formal_methods_service_max_concurrent_analyses gauge\n",
            "dd_formal_methods_service_max_concurrent_analyses {}\n",
            "# HELP dd_formal_methods_service_available_analysis_permits Available analysis concurrency permits.\n",
            "# TYPE dd_formal_methods_service_available_analysis_permits gauge\n",
            "dd_formal_methods_service_available_analysis_permits {}\n",
            "# HELP dd_formal_methods_service_delivery_dedupe_tracked Delivery IDs tracked for webhook dedupe.\n",
            "# TYPE dd_formal_methods_service_delivery_dedupe_tracked gauge\n",
            "dd_formal_methods_service_delivery_dedupe_tracked {}\n",
            "# HELP dd_formal_methods_service_github_token_configured GitHub token configured state.\n",
            "# TYPE dd_formal_methods_service_github_token_configured gauge\n",
            "dd_formal_methods_service_github_token_configured {}\n",
            "# HELP dd_formal_methods_service_repo_allowlist_allow_all Repo allowlist allow-all state.\n",
            "# TYPE dd_formal_methods_service_repo_allowlist_allow_all gauge\n",
            "dd_formal_methods_service_repo_allowlist_allow_all {}\n",
            "# HELP dd_formal_methods_service_path_filter_active Path filter active state.\n",
            "# TYPE dd_formal_methods_service_path_filter_active gauge\n",
            "dd_formal_methods_service_path_filter_active {}\n"
        ),
        state.config.max_concurrent_analyses,
        state.analysis_semaphore.available_permits(),
        dedupe_len,
        github_token_configured,
        repo_allowlist_allow_all,
        path_filter_active
    );

    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}
