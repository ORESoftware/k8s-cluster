use std::sync::atomic::Ordering;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::catalog::*;
use crate::dashboard::*;
use crate::forecast::*;
use crate::nats::*;
use crate::pipeline::*;
use crate::recommendations::*;
use crate::sentiment::*;
use crate::shared::*;
use crate::sources::*;
use crate::state::*;
use crate::types::*;

pub(crate) async fn root() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

pub(crate) async fn descriptor(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(service_descriptor(&state))
}

pub(crate) async fn dashboard_json(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    match dashboard_payload(&state) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let stored_series = state
        .series_store
        .read()
        .map(|store| store.len())
        .unwrap_or(0);
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "storedSeries": stored_series,
        "historyYears": state.config.history_years,
        "projectionMonths": state.config.projection_months,
        "atMs": now_ms()
    }))
}

pub(crate) async fn readyz(State(state): State<AppState>) -> Response {
    let ready = state.config.allow_unauthenticated || state.config.server_auth_secret.is_some();
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
            "authConfigured": state.config.server_auth_secret.is_some(),
            "allowUnauthenticated": state.config.allow_unauthenticated,
            "atMs": now_ms()
        })),
    )
        .into_response()
}

pub(crate) async fn schema() -> impl IntoResponse {
    Json(schema_descriptor())
}

pub(crate) async fn example() -> impl IntoResponse {
    Json(example_request())
}

pub(crate) async fn equations() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "equations": equation_catalog(),
        "desEngine": des_surface_descriptor()
    }))
}

pub(crate) async fn sources() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "sources": source_catalog(),
        "publicSourcesRoute": "GET /sources/public",
        "publicSourceTemplateCount": public_source_templates().len(),
        "pullRoute": "POST /sources/pull",
        "ingestRoute": "POST /ingest",
        "sentimentSourcesRoute": "GET /sentiment/sources",
        "macroIndicatorsRoute": "GET /macro/indicators",
        "vcInvestmentRoute": "GET /vc/investment",
        "recommendationsRoute": "POST /recommendations",
        "pipelineCatalogRoute": "GET /pipelines/catalog",
        "pipelinePlanRoute": "POST /pipelines/plan",
        "pipelineSubmitRoute": "POST /pipelines/submit",
        "integrationHealthRoute": "GET /integrations/health",
        "auditHardeningRoute": "GET /audit/hardening"
    }))
}

pub(crate) async fn public_sources(State(state): State<AppState>) -> impl IntoResponse {
    Json(public_source_catalog_payload(&state.config))
}

pub(crate) async fn sentiment_sources(State(state): State<AppState>) -> impl IntoResponse {
    Json(sentiment_source_catalog(
        &state.config.sentiment_credentials,
    ))
}

pub(crate) async fn macro_indicators(State(state): State<AppState>) -> impl IntoResponse {
    Json(macro_indicator_payload(&state.config))
}

pub(crate) async fn vc_investment(State(state): State<AppState>) -> impl IntoResponse {
    Json(vc_investment_payload(&state.config))
}

pub(crate) async fn pipeline_catalog(State(state): State<AppState>) -> impl IntoResponse {
    Json(pipeline_catalog_payload(&state))
}

pub(crate) async fn integrations_health(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .integration_health_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(integration_health_payload(&state))
}

pub(crate) async fn hardening_audit(State(state): State<AppState>) -> impl IntoResponse {
    Json(hardening_audit_payload(&state))
}

pub(crate) async fn observability(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .observability_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(observability_payload(&state))
}

pub(crate) async fn des_engine_descriptor() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "surface": des_surface_descriptor(),
        "serviceDescriptor": des_service_descriptor()
    }))
}

pub(crate) async fn sentiment_analyze_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SentimentAnalyzeRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state
        .metrics
        .sentiment_requests_total
        .fetch_add(1, Ordering::Relaxed);
    match analyze_sentiment(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn recommendations_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<RecommendationRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state
        .metrics
        .recommendation_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if request.series.as_ref().map(Vec::is_empty).unwrap_or(true) {
        request.series = Some(snapshot_series_or_sample(&state));
    }
    match generate_recommendations(&state.config, request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn pipeline_plan_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PipelinePlanRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state
        .metrics
        .pipeline_plan_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let publish_to_nats = request.publish_to_nats.unwrap_or(true);
    match pipeline_plan_from_request(&state, request) {
        Ok(plan) => {
            if publish_to_nats {
                publish_pipeline_plan(&state, &plan).await;
            }
            Json(plan).into_response()
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn pipeline_submit_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PipelinePlanRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state
        .metrics
        .pipeline_submit_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let publish_to_nats = request.publish_to_nats.unwrap_or(true);
    let plan = match pipeline_plan_from_request(&state, request) {
        Ok(plan) => plan,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            state
                .metrics
                .pipeline_submit_failure_total
                .fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response();
        }
    };
    if publish_to_nats {
        publish_pipeline_plan(&state, &plan).await;
    }
    match submit_pipeline_plan(&state, &plan).await {
        Ok(submitted_jobs) => {
            let ok = submitted_jobs.iter().all(|job| job.accepted);
            Json(PipelineSubmitResponse {
                ok,
                request_id: plan.request_id.clone(),
                schema_version: SCHEMA_VERSION,
                generated_at_ms: now_ms(),
                plan,
                submitted_jobs,
                warnings: vec![
                    "only spark-pipeline-server intents are submitted; Airflow and Databricks remain plan-only".to_string(),
                ],
            })
            .into_response()
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            state
                .metrics
                .pipeline_submit_failure_total
                .fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error, "plan": plan })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn forecast_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ForecastRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    match forecast_from_request(&state, request) {
        Ok(response) => {
            state
                .metrics
                .forecasts_total
                .fetch_add(1, Ordering::Relaxed);
            publish_forecast(&state, &response).await;
            Json(response).into_response()
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn ingest_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<IngestRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state
        .metrics
        .ingest_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(error) = validate_series(&request.series) {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": error })),
        )
            .into_response();
    }
    let replace = request.replace.unwrap_or(false);
    let ingest_request_id = request_id(request.request_id.as_ref(), "ingest");
    let stored = {
        let mut store = match state.series_store.write() {
            Ok(store) => store,
            Err(_) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "ok": false, "error": "series store lock poisoned" })),
                )
                    .into_response();
            }
        };
        if replace {
            store.clear();
        }
        for series in request.series {
            store.insert(series.instrument_id.clone(), series);
        }
        store.len()
    };
    publish_market_event(
        &state,
        json!({
            "type": "economics.ingest",
            "source": SERVICE_NAME,
            "requestId": &ingest_request_id,
            "storedSeries": stored,
            "replace": replace,
            "atMs": now_ms()
        }),
    )
    .await;
    Json(json!({
        "ok": true,
        "requestId": &ingest_request_id,
        "storedSeries": stored,
        "replace": replace,
        "atMs": now_ms()
    }))
    .into_response()
}

pub(crate) async fn pull_source_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ApiPullRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    state
        .metrics
        .source_pull_total
        .fetch_add(1, Ordering::Relaxed);
    match pull_source(&state, request).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            state
                .metrics
                .source_pull_failure_total
                .fetch_add(1, Ordering::Relaxed);
            emit_log(
                "WARN",
                "economics.source_pull.error",
                "economics source pull failed",
                json!({
                    "error": error_summary(&error)
                }),
            );
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn api_docs_html() -> Html<&'static str> {
    Html(include_str!("../generated/api-docs.html"))
}

pub(crate) async fn api_docs_json() -> impl IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}
