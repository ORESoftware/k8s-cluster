use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;

use crate::{
    audit::{run_audit, validate_request},
    auth::require_auth,
    config::{Config, SCHEMA_VERSION, SERVICE_NAME},
    diagrams::generate_infrastructure_diagram,
    jobs::JobStore,
    metrics::Metrics,
    models::{
        diagram_example_request, example_request, schema_example, system_report_example_request,
        vulnerability_scan_example_request, AuditRequest, DiagramRequest, SystemReportRequest,
        VulnerabilityScanRequest,
    },
    reports::{generate_system_report, scan_vulnerabilities},
    standards::{standard_by_id_or_alias, CONTROL_CATALOG, STANDARDS},
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub metrics: Arc<Metrics>,
    pub jobs: Arc<JobStore>,
    pub http: reqwest::Client,
}

pub fn router(state: AppState) -> Router {
    let body_limit = state.config.max_http_body_bytes;
    Router::new()
        .route("/", get(descriptor))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/schema", get(schema))
        .route("/example", get(example))
        .route("/diagrams/example", get(diagram_example))
        .route("/diagrams/infrastructure", post(diagram_infrastructure))
        .route("/reports/example", get(system_report_example))
        .route("/reports/system", post(system_report))
        .route(
            "/vulnerability-scan/example",
            get(vulnerability_scan_example),
        )
        .route("/vulnerability-scan", post(vulnerability_scan))
        .route("/standards", get(standards))
        .route("/standards/:standard_id", get(standard))
        .route("/controls", get(controls))
        .route("/audits", get(list_audits).post(submit_audit))
        .route("/audits/:job_id", get(get_audit))
        .route("/audit-sync", post(audit_sync))
        .layer(DefaultBodyLimit::max(body_limit))
        .with_state(state)
        .merge(dd_runtime_config_client::router())
}

async fn descriptor() -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "description": "Compliance readiness job server for artifacts, codebases, networks, and systems.",
        "routes": {
            "health": ["/healthz", "/readyz", "/metrics"],
            "catalog": ["/standards", "/standards/:standardId", "/controls"],
            "audits": ["POST /audits", "GET /audits", "GET /audits/:jobId", "POST /audit-sync"],
            "diagrams": ["GET /diagrams/example", "POST /diagrams/infrastructure"],
            "reports": ["GET /reports/example", "POST /reports/system", "POST /vulnerability-scan"],
            "docs": ["/docs/api", "/api/docs", "/api/docs.json"]
        }
    }))
}

async fn healthz(State(state): State<AppState>) -> Json<serde_json::Value> {
    let counts = state.jobs.counts().await;
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "authConfigured": state.config.server_auth_secret.is_some(),
        "allowUnauthenticated": state.config.allow_unauthenticated,
        "standards": STANDARDS.len(),
        "controls": CONTROL_CATALOG.len(),
        "jobs": {
            "queued": counts.queued,
            "running": counts.running,
            "succeeded": counts.succeeded,
            "failed": counts.failed,
            "total": counts.total()
        },
        "externalFetchEnabled": state.config.allow_external_fetch,
        "repoCloneEnabled": state.config.allow_repo_clone
    }))
}

async fn readyz(State(state): State<AppState>) -> Response {
    match state.jobs.storage_ready().await {
        Ok(()) => Json(json!({ "ok": true, "service": SERVICE_NAME, "jobStoreWritable": true }))
            .into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "ok": false,
                "service": SERVICE_NAME,
                "jobStoreWritable": false,
                "error": error.to_string()
            })),
        )
            .into_response(),
    }
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let mut body = state.metrics.render_prometheus();
    body.push_str(&state.jobs.render_prometheus().await);
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

async fn schema() -> Json<serde_json::Value> {
    Json(schema_example())
}

async fn example() -> Json<AuditRequest> {
    Json(example_request())
}

async fn diagram_example() -> Json<DiagramRequest> {
    Json(diagram_example_request())
}

async fn system_report_example() -> Json<SystemReportRequest> {
    Json(system_report_example_request())
}

async fn vulnerability_scan_example() -> Json<VulnerabilityScanRequest> {
    Json(vulnerability_scan_example_request())
}

async fn standards() -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "count": STANDARDS.len(),
        "standards": STANDARDS
    }))
}

async fn standard(Path(standard_id): Path<String>) -> Response {
    match standard_by_id_or_alias(&standard_id) {
        Some(standard) => Json(json!({ "ok": true, "standard": standard })).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": "standard not found" })),
        )
            .into_response(),
    }
}

async fn controls() -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "count": CONTROL_CATALOG.len(),
        "controls": CONTROL_CATALOG
    }))
}

async fn diagram_infrastructure(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DiagramRequest>,
) -> Response {
    state.metrics.http_requests_total.fetch_add(1);
    if let Err(response) = require_auth(&headers, &state.config, &state.metrics) {
        return response;
    }
    let report = generate_infrastructure_diagram(request).await;
    state.metrics.diagrams_generated_total.fetch_add(1);
    Json(report).into_response()
}

async fn system_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SystemReportRequest>,
) -> Response {
    state.metrics.http_requests_total.fetch_add(1);
    if let Err(response) = require_auth(&headers, &state.config, &state.metrics) {
        return response;
    }
    let report = generate_system_report(state.config.clone(), state.http.clone(), request).await;
    if report.diagram.is_some() {
        state.metrics.diagrams_generated_total.fetch_add(1);
    }
    Json(report).into_response()
}

async fn vulnerability_scan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<VulnerabilityScanRequest>,
) -> Response {
    state.metrics.http_requests_total.fetch_add(1);
    if let Err(response) = require_auth(&headers, &state.config, &state.metrics) {
        return response;
    }
    Json(scan_vulnerabilities(&state.config, request)).into_response()
}

async fn submit_audit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AuditRequest>,
) -> Response {
    state.metrics.http_requests_total.fetch_add(1);
    if let Err(response) = require_auth(&headers, &state.config, &state.metrics) {
        return response;
    }
    if let Err(error) = validate_request(&state.config, &request) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": error })),
        )
            .into_response();
    }
    match state
        .jobs
        .clone()
        .enqueue(
            state.config.clone(),
            state.http.clone(),
            state.metrics.clone(),
            request,
        )
        .await
    {
        Ok(record) => (StatusCode::ACCEPTED, Json(record)).into_response(),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    }
}

async fn audit_sync(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AuditRequest>,
) -> Response {
    state.metrics.http_requests_total.fetch_add(1);
    if let Err(response) = require_auth(&headers, &state.config, &state.metrics) {
        return response;
    }
    if let Err(error) = validate_request(&state.config, &request) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": error })),
        )
            .into_response();
    }
    match run_audit(
        state.config.clone(),
        state.http.clone(),
        request,
        "sync-audit".to_string(),
    )
    .await
    {
        Ok(report) => {
            state
                .metrics
                .standards_evaluated_total
                .fetch_add(report.standard_results.len() as u64);
            state
                .metrics
                .findings_total
                .fetch_add(report.findings.len() as u64);
            Json(report).into_response()
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    }
}

async fn list_audits(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state.metrics.http_requests_total.fetch_add(1);
    if let Err(response) = require_auth(&headers, &state.config, &state.metrics) {
        return response;
    }
    Json(state.jobs.list().await).into_response()
}

async fn get_audit(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    state.metrics.http_requests_total.fetch_add(1);
    if let Err(response) = require_auth(&headers, &state.config, &state.metrics) {
        return response;
    }
    match state.jobs.get(&job_id).await {
        Some(record) => Json(record).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": "audit job not found" })),
        )
            .into_response(),
    }
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
