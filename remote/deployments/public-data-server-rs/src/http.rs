use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::Ordering,
};

use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::analysis::{build_analysis_result, publish_analysis};
use crate::auth::{auth_failure_response, require_auth, require_webhook_auth};
use crate::briefs::markdown_brief;
use crate::catalog::{example_payload, schema_payload, service_descriptor, source_catalog};
use crate::grants::grant_matches_from_records;
use crate::ingest::{process_ingest_request, process_scrape_request, process_webhook};
use crate::pipeline::create_pipeline_job;
use crate::state::{AppState, SERVICE_NAME};
use crate::store::{filter_records, records_snapshot, store_analysis};
use crate::types::{
    AnalysisRequest, GrantMatchRequest, IngestRequest, PipelineRequest, ScrapeRequest, UiScrapeForm,
    WebhookIngestRequest, WhitePaperRequest,
};
use crate::ui::{
    render_ui_notice, render_ui_recent_records, render_ui_scrape_result, render_ui_shell,
    render_ui_sources, render_ui_summary, ui_auth_failure_response,
};
use crate::util::request_id;

pub(crate) async fn ui_dashboard(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return ui_auth_failure_response(&state, failure);
    }
    Html(render_ui_shell(&state).into_string()).into_response()
}

pub(crate) async fn ui_summary_fragment(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return ui_auth_failure_response(&state, failure);
    }
    Html(render_ui_summary(&state).into_string()).into_response()
}

pub(crate) async fn ui_sources_fragment(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return ui_auth_failure_response(&state, failure);
    }
    Html(render_ui_sources().into_string()).into_response()
}

pub(crate) async fn ui_recent_records_fragment(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return ui_auth_failure_response(&state, failure);
    }
    Html(render_ui_recent_records(&state).into_string()).into_response()
}

pub(crate) async fn ui_scrape_action(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<UiScrapeForm>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return ui_auth_failure_response(&state, failure);
    }
    let request = match form.into_scrape_request() {
        Ok(request) => request,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return (
                StatusCode::BAD_REQUEST,
                Html(render_ui_notice("Scrape rejected", &error, true).into_string()),
            )
                .into_response();
        }
    };
    match process_scrape_request(&state, request).await {
        Ok(value) => Html(render_ui_scrape_result(&value).into_string()).into_response(),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::BAD_REQUEST,
                Html(render_ui_notice("Scrape failed", &error, true).into_string()),
            )
                .into_response()
        }
    }
}

pub(crate) async fn descriptor(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(service_descriptor(&state))
}

pub(crate) async fn sources(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({ "ok": true, "sources": source_catalog() }))
}

pub(crate) async fn schema(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(schema_payload())
}

pub(crate) async fn example(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({ "ok": true, "example": example_payload() }))
}

pub(crate) async fn datasets(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
    let mut summaries: BTreeMap<String, Value> = BTreeMap::new();
    for record in &store.records {
        let entry = summaries
            .entry(record.dataset_id.clone())
            .or_insert_with(|| {
                json!({
                    "datasetId": record.dataset_id,
                    "sources": [],
                    "tags": [],
                    "recordCount": 0,
                    "grantCount": 0,
                    "metricNames": []
                })
            });
        entry["recordCount"] = json!(entry["recordCount"].as_u64().unwrap_or(0) + 1);
        if record.grant.is_some() {
            entry["grantCount"] = json!(entry["grantCount"].as_u64().unwrap_or(0) + 1);
        }
    }
    for (dataset_id, entry) in summaries.iter_mut() {
        let dataset_records = store
            .records
            .iter()
            .filter(|record| &record.dataset_id == dataset_id)
            .collect::<Vec<_>>();
        let sources = dataset_records
            .iter()
            .map(|record| record.source.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let tags = dataset_records
            .iter()
            .flat_map(|record| record.tags.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let metric_names = dataset_records
            .iter()
            .flat_map(|record| record.metrics.keys().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        entry["sources"] = json!(sources);
        entry["tags"] = json!(tags);
        entry["metricNames"] = json!(metric_names);
    }
    Json(json!({
        "ok": true,
        "datasets": summaries.into_values().collect::<Vec<_>>(),
        "recordCount": store.records.len(),
        "webhookReceiptCount": store.webhook_receipts.len(),
        "analysisCount": store.analyses.len(),
        "pipelineJobCount": store.pipeline_jobs.len()
    }))
    .into_response()
}

pub(crate) async fn jobs(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
    Json(json!({ "ok": true, "jobs": store.pipeline_jobs.clone() })).into_response()
}

pub(crate) async fn webhook_ingest_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<WebhookIngestRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_webhook_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    match process_webhook(&state, request).await {
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
    match process_ingest_request(&state, request).await {
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

pub(crate) async fn scrape_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ScrapeRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    match process_scrape_request(&state, request).await {
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

pub(crate) async fn grant_match_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<GrantMatchRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let records = filter_records(&records_snapshot(&state), &request.dataset_ids, &None);
    let matches = grant_matches_from_records(&records, &request);
    state
        .metrics
        .grant_match_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let result = build_analysis_result(
        "grant-match",
        request_id(request.request_id.as_ref(), "grant-match"),
        records,
        None,
        matches.clone(),
        None,
    );
    store_analysis(&state, result.clone());
    publish_analysis(&state, &result).await;
    Json(json!({ "ok": true, "matches": matches, "analysis": result })).into_response()
}

pub(crate) async fn trends_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AnalysisRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let mut records = filter_records(
        &records_snapshot(&state),
        &request.dataset_ids,
        &request.tags,
    );
    records.truncate(request.limit.unwrap_or(2_000).min(10_000));
    let result = build_analysis_result(
        "trends",
        request_id(request.request_id.as_ref(), "trends"),
        records,
        request.metrics,
        Vec::new(),
        None,
    );
    state
        .metrics
        .trend_requests_total
        .fetch_add(1, Ordering::Relaxed);
    store_analysis(&state, result.clone());
    publish_analysis(&state, &result).await;
    Json(json!({ "ok": true, "analysis": result })).into_response()
}

pub(crate) async fn correlations_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AnalysisRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let mut records = filter_records(
        &records_snapshot(&state),
        &request.dataset_ids,
        &request.tags,
    );
    records.truncate(request.limit.unwrap_or(2_000).min(10_000));
    let result = build_analysis_result(
        "correlations",
        request_id(request.request_id.as_ref(), "correlations"),
        records,
        request.metrics,
        Vec::new(),
        None,
    );
    state
        .metrics
        .correlation_requests_total
        .fetch_add(1, Ordering::Relaxed);
    store_analysis(&state, result.clone());
    publish_analysis(&state, &result).await;
    Json(json!({ "ok": true, "analysis": result })).into_response()
}

pub(crate) async fn white_paper_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<WhitePaperRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let tags = request.focus_areas.clone();
    let mut records = filter_records(&records_snapshot(&state), &request.dataset_ids, &tags);
    records.truncate(request.limit.unwrap_or(1_000).min(5_000));
    let grant_request = GrantMatchRequest {
        request_id: request.request_id.clone(),
        applicant_profile: request.research_question.clone(),
        focus_areas: request.focus_areas.clone().unwrap_or_default(),
        dataset_ids: request.dataset_ids.clone(),
        min_amount: None,
        limit: Some(20),
    };
    let grants = if request.include_grants.unwrap_or(true) {
        grant_matches_from_records(&records, &grant_request)
    } else {
        Vec::new()
    };
    let mut result = build_analysis_result(
        "white-paper-brief",
        request_id(request.request_id.as_ref(), "white-paper"),
        records.clone(),
        None,
        grants,
        None,
    );
    result.markdown = Some(markdown_brief(&request, &result, records.len()));
    state
        .metrics
        .white_paper_briefs_total
        .fetch_add(1, Ordering::Relaxed);
    store_analysis(&state, result.clone());
    publish_analysis(&state, &result).await;
    Json(json!({ "ok": true, "brief": result })).into_response()
}

pub(crate) async fn pipeline_jobs_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PipelineRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    match create_pipeline_job(&state, request).await {
        Ok(job) => Json(json!({ "ok": true, "job": job })).into_response(),
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
    let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "recordCount": store.records.len(),
        "webhookReceiptCount": store.webhook_receipts.len(),
        "analysisCount": store.analyses.len(),
        "pipelineJobCount": store.pipeline_jobs.len()
    }))
}

pub(crate) async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "natsConfigured": state.nats.is_some(),
        "scraperBaseUrl": state.config.scraper_base_url
    }))
}

pub(crate) async fn metrics(State(state): State<AppState>) -> Response {
    let body = format!(
        "# HELP dd_public_data_server_http_requests_total HTTP requests observed by the public-data service.\n\
         # TYPE dd_public_data_server_http_requests_total counter\n\
         dd_public_data_server_http_requests_total {}\n\
         # HELP dd_public_data_server_webhook_receipts_total Webhook receipts accepted.\n\
         # TYPE dd_public_data_server_webhook_receipts_total counter\n\
         dd_public_data_server_webhook_receipts_total {}\n\
         # HELP dd_public_data_server_records_ingested_total Normalized records ingested.\n\
         # TYPE dd_public_data_server_records_ingested_total counter\n\
         dd_public_data_server_records_ingested_total {}\n\
         # HELP dd_public_data_server_scrape_requests_total Scrape requests delegated to dd-web-scraper.\n\
         # TYPE dd_public_data_server_scrape_requests_total counter\n\
         dd_public_data_server_scrape_requests_total {}\n\
         # HELP dd_public_data_server_grant_match_requests_total Grant match requests accepted.\n\
         # TYPE dd_public_data_server_grant_match_requests_total counter\n\
         dd_public_data_server_grant_match_requests_total {}\n\
         # HELP dd_public_data_server_trend_requests_total Trend analysis requests accepted.\n\
         # TYPE dd_public_data_server_trend_requests_total counter\n\
         dd_public_data_server_trend_requests_total {}\n\
         # HELP dd_public_data_server_correlation_requests_total Correlation analysis requests accepted.\n\
         # TYPE dd_public_data_server_correlation_requests_total counter\n\
         dd_public_data_server_correlation_requests_total {}\n\
         # HELP dd_public_data_server_white_paper_briefs_total White-paper evidence briefs generated.\n\
         # TYPE dd_public_data_server_white_paper_briefs_total counter\n\
         dd_public_data_server_white_paper_briefs_total {}\n\
         # HELP dd_public_data_server_pipeline_jobs_total Pipeline job intents queued.\n\
         # TYPE dd_public_data_server_pipeline_jobs_total counter\n\
         dd_public_data_server_pipeline_jobs_total {}\n\
         # HELP dd_public_data_server_auth_failures_total Rejected requests with missing or invalid auth.\n\
         # TYPE dd_public_data_server_auth_failures_total counter\n\
         dd_public_data_server_auth_failures_total {}\n\
         # HELP dd_public_data_server_errors_total Request, scrape, analysis, or publish errors.\n\
         # TYPE dd_public_data_server_errors_total counter\n\
         dd_public_data_server_errors_total {}\n\
         # HELP dd_public_data_server_nats_messages_total NATS ingest messages consumed.\n\
         # TYPE dd_public_data_server_nats_messages_total counter\n\
         dd_public_data_server_nats_messages_total {}\n\
         # HELP dd_public_data_server_nats_published_total NATS messages published.\n\
         # TYPE dd_public_data_server_nats_published_total counter\n\
         dd_public_data_server_nats_published_total {}\n",
        state.metrics.http_requests_total.load(Ordering::Relaxed),
        state.metrics.webhook_receipts_total.load(Ordering::Relaxed),
        state.metrics.records_ingested_total.load(Ordering::Relaxed),
        state.metrics.scrape_requests_total.load(Ordering::Relaxed),
        state
            .metrics
            .grant_match_requests_total
            .load(Ordering::Relaxed),
        state.metrics.trend_requests_total.load(Ordering::Relaxed),
        state
            .metrics
            .correlation_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .white_paper_briefs_total
            .load(Ordering::Relaxed),
        state.metrics.pipeline_jobs_total.load(Ordering::Relaxed),
        state.metrics.auth_failures_total.load(Ordering::Relaxed),
        state.metrics.errors_total.load(Ordering::Relaxed),
        state.metrics.nats_messages_total.load(Ordering::Relaxed),
        state.metrics.nats_published_total.load(Ordering::Relaxed),
    );
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
        .into_response()
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
