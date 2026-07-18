use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, Form, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dd_nats_subject_defs::{
    PUBLIC_DATA_ANALYSIS_RESULTS_SUBJECT, PUBLIC_DATA_INGEST_REQUESTS_QUEUE_GROUP,
    PUBLIC_DATA_INGEST_REQUESTS_SUBJECT, PUBLIC_DATA_INGEST_RESULTS_SUBJECT,
    PUBLIC_DATA_PIPELINE_JOBS_SUBJECT, PUBLIC_DATA_WEBHOOK_EVENTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};
use futures_util::StreamExt;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const SERVICE_NAME: &str = "dd-public-data-server";
const SCHEMA_VERSION: &str = "public_data.ingest.v1";
const MAX_HTTP_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;
const MAX_RECORDS_PER_REQUEST: usize = 512;
const MAX_RECORD_STORE: usize = 10_000;
const MAX_RECEIPT_STORE: usize = 2_000;
const MAX_ANALYSIS_STORE: usize = 1_000;
const MAX_PIPELINE_JOBS: usize = 2_000;
const MAX_TEXT_LEN: usize = 4_096;
const MAX_LONG_TEXT_LEN: usize = 24_000;
const MAX_TOKEN_LEN: usize = 160;
const MAX_TAGS: usize = 64;
const MAX_GRAPH_POINTS: usize = 256;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    metrics: Arc<Metrics>,
    nats: Option<async_nats::Client>,
    http: reqwest::Client,
    store: Arc<RwLock<PublicDataStore>>,
}

#[derive(Clone)]
struct Config {
    server_auth_secret: Option<String>,
    webhook_secret: Option<String>,
    allow_unauthenticated: bool,
    allow_unauthenticated_webhooks: bool,
    scraper_base_url: String,
    scraper_auth_secret: Option<String>,
    ingest_request_subject: String,
    ingest_result_subject: String,
    webhook_event_subject: String,
    pipeline_job_subject: String,
    analysis_result_subject: String,
    runtime_event_subject: String,
    queue_group: String,
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    webhook_receipts_total: AtomicU64,
    records_ingested_total: AtomicU64,
    scrape_requests_total: AtomicU64,
    grant_match_requests_total: AtomicU64,
    trend_requests_total: AtomicU64,
    correlation_requests_total: AtomicU64,
    white_paper_briefs_total: AtomicU64,
    pipeline_jobs_total: AtomicU64,
    auth_failures_total: AtomicU64,
    errors_total: AtomicU64,
    nats_messages_total: AtomicU64,
    nats_published_total: AtomicU64,
}

#[derive(Default)]
struct PublicDataStore {
    records: Vec<DataRecord>,
    webhook_receipts: Vec<WebhookReceipt>,
    analyses: Vec<AnalysisResult>,
    pipeline_jobs: Vec<PipelineJob>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IngestRequest {
    request_id: Option<String>,
    schema_version: Option<String>,
    dataset_id: Option<String>,
    source: String,
    source_url: Option<String>,
    tags: Option<Vec<String>>,
    records: Vec<IncomingRecord>,
    pipeline: Option<PipelineOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IncomingRecord {
    record_id: Option<String>,
    dataset_id: Option<String>,
    source: Option<String>,
    source_url: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    published_at: Option<String>,
    authors: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    metrics: Option<BTreeMap<String, f64>>,
    grant: Option<GrantOpportunity>,
    raw: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DataRecord {
    record_id: String,
    dataset_id: String,
    source: String,
    source_url: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    published_at: Option<String>,
    collected_at_ms: u128,
    authors: Vec<String>,
    tags: Vec<String>,
    metrics: BTreeMap<String, f64>,
    grant: Option<GrantOpportunity>,
    raw: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantOpportunity {
    grant_id: Option<String>,
    title: String,
    agency: Option<String>,
    program: Option<String>,
    amount: Option<f64>,
    due_date: Option<String>,
    eligibility: Option<String>,
    topics: Vec<String>,
    url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScrapeRequest {
    request_id: Option<String>,
    source: String,
    url: String,
    dataset_id: Option<String>,
    strategy: Option<String>,
    render_javascript: Option<bool>,
    selector: Option<String>,
    selectors: Option<BTreeMap<String, String>>,
    include_links: Option<bool>,
    tags: Option<Vec<String>>,
    pipeline: Option<PipelineOptions>,
}

#[derive(Debug, Deserialize)]
struct UiScrapeForm {
    source: Option<String>,
    url: String,
    dataset_id: Option<String>,
    strategy: Option<String>,
    selector: Option<String>,
    tags: Option<String>,
    render_javascript: Option<String>,
    include_links: Option<String>,
    pipeline_enabled: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScraperExtraction {
    title: Option<String>,
    text: Option<String>,
    fields: Option<BTreeMap<String, String>>,
    links: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScraperResponse {
    ok: bool,
    request_id: Option<String>,
    url: Option<String>,
    final_url: Option<String>,
    status: Option<u16>,
    content_type: Option<String>,
    duration_ms: Option<u64>,
    extraction: Option<ScraperExtraction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebhookIngestRequest {
    request_id: Option<String>,
    provider: String,
    event_type: Option<String>,
    dataset_id: Option<String>,
    source_url: Option<String>,
    payload: Value,
    records: Option<Vec<IncomingRecord>>,
    pipeline: Option<PipelineOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebhookReceipt {
    receipt_id: String,
    provider: String,
    event_type: String,
    dataset_id: Option<String>,
    source_url: Option<String>,
    received_at_ms: u128,
    record_count: usize,
    payload_shape: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PipelineOptions {
    enabled: Option<bool>,
    job_type: Option<String>,
    sink: Option<String>,
    airflow_dag: Option<String>,
    spark_app: Option<String>,
    parameters: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PipelineRequest {
    request_id: Option<String>,
    job_type: Option<String>,
    dataset_ids: Option<Vec<String>>,
    analysis_ids: Option<Vec<String>>,
    sink: Option<String>,
    airflow_dag: Option<String>,
    spark_app: Option<String>,
    parameters: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PipelineJob {
    job_id: String,
    request_id: String,
    job_type: String,
    status: String,
    dataset_ids: Vec<String>,
    analysis_ids: Vec<String>,
    sink: String,
    airflow_dag: Option<String>,
    spark_app: Option<String>,
    parameters: Value,
    submitted_at_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantMatchRequest {
    request_id: Option<String>,
    applicant_profile: String,
    focus_areas: Vec<String>,
    dataset_ids: Option<Vec<String>>,
    min_amount: Option<f64>,
    limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GrantMatch {
    record_id: String,
    dataset_id: String,
    source: String,
    title: String,
    url: Option<String>,
    agency: Option<String>,
    program: Option<String>,
    amount: Option<f64>,
    due_date: Option<String>,
    score: f64,
    reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnalysisRequest {
    request_id: Option<String>,
    dataset_ids: Option<Vec<String>>,
    metrics: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnalysisResult {
    analysis_id: String,
    request_id: String,
    kind: String,
    generated_at_ms: u128,
    dataset_ids: Vec<String>,
    summary: String,
    graph: GraphData,
    trends: Vec<TrendSummary>,
    correlations: Vec<CorrelationSummary>,
    grants: Vec<GrantMatch>,
    model_notes: Vec<ModelNote>,
    markdown: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphData {
    graph_type: String,
    title: String,
    x_label: String,
    y_label: String,
    series: Vec<GraphSeries>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphSeries {
    name: String,
    points: Vec<GraphPoint>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphPoint {
    x: f64,
    y: f64,
    label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrendSummary {
    metric: String,
    count: usize,
    mean: f64,
    min: f64,
    max: f64,
    slope_per_record: f64,
    direction: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CorrelationSummary {
    left_metric: String,
    right_metric: String,
    count: usize,
    pearson: f64,
    strength: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelNote {
    name: String,
    equation: String,
    use_case: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WhitePaperRequest {
    request_id: Option<String>,
    title: Option<String>,
    research_question: String,
    dataset_ids: Option<Vec<String>>,
    focus_areas: Option<Vec<String>>,
    include_grants: Option<bool>,
    limit: Option<usize>,
}

enum AuthFailure {
    MissingSecret,
    Unauthorized,
}

fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_bool(key: &str, fallback: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(fallback)
}

fn config_from_env() -> Config {
    Config {
        server_auth_secret: optional_env("SERVER_AUTH_SECRET")
            .or_else(|| optional_env("PUBLIC_DATA_SERVER_AUTH_SECRET")),
        webhook_secret: optional_env("PUBLIC_DATA_WEBHOOK_SECRET"),
        allow_unauthenticated: env_bool("PUBLIC_DATA_ALLOW_UNAUTHENTICATED", false),
        allow_unauthenticated_webhooks: env_bool(
            "PUBLIC_DATA_ALLOW_UNAUTHENTICATED_WEBHOOKS",
            false,
        ),
        scraper_base_url: env_value(
            "PUBLIC_DATA_SCRAPER_BASE_URL",
            "http://dd-web-scraper.default.svc.cluster.local:8097",
        ),
        scraper_auth_secret: optional_env("PUBLIC_DATA_SCRAPER_AUTH_SECRET")
            .or_else(|| optional_env("SERVER_AUTH_SECRET")),
        ingest_request_subject: env_value(
            "PUBLIC_DATA_INGEST_REQUEST_SUBJECT",
            PUBLIC_DATA_INGEST_REQUESTS_SUBJECT,
        ),
        ingest_result_subject: env_value(
            "PUBLIC_DATA_INGEST_RESULT_SUBJECT",
            PUBLIC_DATA_INGEST_RESULTS_SUBJECT,
        ),
        webhook_event_subject: env_value(
            "PUBLIC_DATA_WEBHOOK_EVENT_SUBJECT",
            PUBLIC_DATA_WEBHOOK_EVENTS_SUBJECT,
        ),
        pipeline_job_subject: env_value(
            "PUBLIC_DATA_PIPELINE_JOB_SUBJECT",
            PUBLIC_DATA_PIPELINE_JOBS_SUBJECT,
        ),
        analysis_result_subject: env_value(
            "PUBLIC_DATA_ANALYSIS_RESULT_SUBJECT",
            PUBLIC_DATA_ANALYSIS_RESULTS_SUBJECT,
        ),
        runtime_event_subject: env_value(
            "PUBLIC_DATA_RUNTIME_EVENT_SUBJECT",
            RUNTIME_EVENTS_SUBJECT,
        ),
        queue_group: env_value(
            "PUBLIC_DATA_QUEUE_GROUP",
            PUBLIC_DATA_INGEST_REQUESTS_QUEUE_GROUP,
        ),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn request_id(input: Option<&String>, fallback: &str) -> String {
    input
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .chars()
        .take(MAX_TOKEN_LEN)
        .collect()
}

fn clean_text(value: Option<&String>, max_len: usize) -> Option<String> {
    value
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
        .map(|text| {
            text.chars()
                .filter(|ch| !ch.is_control())
                .take(max_len)
                .collect()
        })
}

fn clean_required(value: &str, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if trimmed.len() > MAX_TOKEN_LEN {
        return Err(format!("{label} must be at most {MAX_TOKEN_LEN} bytes"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(format!("{label} must not contain control characters"));
    }
    Ok(trimmed.to_string())
}

fn clean_tags(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        let normalized = value
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_' || *ch == ' ')
            .take(80)
            .collect::<String>();
        let normalized = normalized.split_whitespace().collect::<Vec<_>>().join("-");
        if !normalized.is_empty() && seen.insert(normalized.clone()) {
            out.push(normalized);
        }
        if out.len() >= MAX_TAGS {
            break;
        }
    }
    out
}

fn form_text(value: Option<String>, max_len: usize) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .map(|text| text.chars().take(max_len).collect())
}

fn form_csv(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

impl UiScrapeForm {
    fn into_scrape_request(self) -> Result<ScrapeRequest, String> {
        let url = self.url.trim();
        if url.is_empty() {
            return Err("url must not be empty".to_string());
        }
        if url.len() > MAX_TEXT_LEN {
            return Err(format!("url must be at most {MAX_TEXT_LEN} bytes"));
        }
        if url.chars().any(char::is_control) {
            return Err("url must not contain control characters".to_string());
        }
        let url = url.to_string();
        validate_public_url(&url)?;
        let source = form_text(self.source, MAX_TOKEN_LEN)
            .map(|value| clean_required(&value, "source"))
            .transpose()?
            .unwrap_or_else(|| "operator-scrape".to_string());
        let dataset_id = form_text(self.dataset_id, MAX_TOKEN_LEN)
            .map(|value| clean_required(&value, "datasetId"))
            .transpose()?;
        let strategy = form_text(self.strategy, MAX_TOKEN_LEN);
        let selector = form_text(self.selector, MAX_TOKEN_LEN);
        let tags = clean_tags(form_csv(self.tags));
        let pipeline = self.pipeline_enabled.as_ref().map(|_| PipelineOptions {
            enabled: Some(true),
            job_type: Some("spark-etl".to_string()),
            sink: dataset_id
                .as_ref()
                .map(|dataset| format!("minio://public-data/bronze/{dataset}")),
            airflow_dag: None,
            spark_app: Some("public-data-normalize".to_string()),
            parameters: Some(json!({ "submittedBy": "public-data-ui" })),
        });
        Ok(ScrapeRequest {
            request_id: None,
            source,
            url,
            dataset_id,
            strategy,
            render_javascript: Some(self.render_javascript.is_some()),
            selector,
            selectors: None,
            include_links: Some(self.include_links.is_some()),
            tags: if tags.is_empty() { None } else { Some(tags) },
            pipeline,
        })
    }
}

fn durable_token(prefix: &str, source: &str, suffix: &str) -> String {
    let source = source
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let source = if source.is_empty() {
        "unknown".to_string()
    } else {
        source
    };
    format!("{prefix}-{source}-{suffix}")
        .chars()
        .take(MAX_TOKEN_LEN)
        .collect()
}

fn validate_public_url(raw: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(raw).map_err(|error| format!("invalid url: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("url scheme must be http or https".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("url credentials are not allowed".to_string());
    }
    let Some(host) = url.host_str() else {
        return Err("url must include a host".to_string());
    };
    if blocked_public_data_host(host) {
        return Err("private or local targets are not allowed".to_string());
    }
    Ok(())
}

fn blocked_public_data_host(host: &str) -> bool {
    let host = host.trim().trim_matches(['[', ']']).to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(addr)) => {
            addr.is_private()
                || addr.is_loopback()
                || addr.is_link_local()
                || addr.is_broadcast()
                || addr.is_documentation()
                || addr.is_unspecified()
        }
        Ok(IpAddr::V6(addr)) => {
            addr.is_loopback()
                || addr.is_unspecified()
                || addr.is_unique_local()
                || addr.is_unicast_link_local()
                || addr.is_multicast()
        }
        Err(_) => false,
    }
}

fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), AuthFailure> {
    if state.config.allow_unauthenticated {
        return Ok(());
    }
    let Some(secret) = state.config.server_auth_secret.as_ref() else {
        return Err(AuthFailure::MissingSecret);
    };
    let provided = headers
        .get("x-server-auth")
        .or_else(|| headers.get("auth"))
        .and_then(|value| value.to_str().ok());
    match provided {
        Some(value) if value == secret => Ok(()),
        _ => Err(AuthFailure::Unauthorized),
    }
}

fn require_webhook_auth(headers: &HeaderMap, state: &AppState) -> Result<(), AuthFailure> {
    if state.config.allow_unauthenticated_webhooks {
        return Ok(());
    }
    if let Some(secret) = state.config.webhook_secret.as_ref() {
        let provided = headers
            .get("x-public-data-webhook-secret")
            .or_else(|| headers.get("x-webhook-secret"))
            .and_then(|value| value.to_str().ok());
        return match provided {
            Some(value) if value == secret => Ok(()),
            _ => Err(AuthFailure::Unauthorized),
        };
    }
    require_auth(headers, state)
}

fn auth_failure_response(state: &AppState, failure: AuthFailure) -> Response {
    state
        .metrics
        .auth_failures_total
        .fetch_add(1, Ordering::Relaxed);
    let message = match failure {
        AuthFailure::MissingSecret => "server auth secret is not configured",
        AuthFailure::Unauthorized => "unauthorized",
    };
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "ok": false, "error": message })),
    )
        .into_response()
}

fn source_catalog() -> Vec<Value> {
    vec![
        json!({
            "slug": "data-gov",
            "name": "Data.gov",
            "baseUrl": "https://data.gov",
            "kind": "government-open-data",
            "defaultStrategy": "native-fetch",
            "notes": "Catalog/API source for public US government datasets."
        }),
        json!({
            "slug": "science-gov",
            "name": "Science.gov",
            "baseUrl": "https://www.science.gov",
            "kind": "government-science-search",
            "defaultStrategy": "cheerio",
            "notes": "Federated science search and agency research discovery."
        }),
        json!({
            "slug": "pubmed",
            "name": "PubMed",
            "baseUrl": "https://pubmed.ncbi.nlm.nih.gov",
            "kind": "biomedical-literature",
            "defaultStrategy": "native-fetch",
            "notes": "Biomedical article metadata, abstracts, MeSH topics, and trend signals."
        }),
        json!({
            "slug": "state-libraries",
            "name": "State libraries",
            "baseUrl": "varies",
            "kind": "state-public-records",
            "defaultStrategy": "auto",
            "notes": "State-level archives, library catalogs, local reports, and historical collections."
        }),
        json!({
            "slug": "plos",
            "name": "PLOS",
            "baseUrl": "https://plos.org",
            "kind": "open-access-research",
            "defaultStrategy": "native-fetch",
            "notes": "Open-access research articles for evidence synthesis."
        }),
        json!({
            "slug": "propublica",
            "name": "ProPublica",
            "baseUrl": "https://www.propublica.org",
            "kind": "public-interest-investigations",
            "defaultStrategy": "cheerio",
            "notes": "Investigative datasets, nonprofit data, and public-interest reporting."
        }),
        json!({
            "slug": "cambridge-analytics",
            "name": "Cambridge analytics / Cambridge research sources",
            "baseUrl": "varies",
            "kind": "research-and-analytics",
            "defaultStrategy": "auto",
            "notes": "Placeholder catalog slot for approved Cambridge-linked public analytics/research sources."
        }),
        json!({
            "slug": "sbir",
            "name": "SBIR.gov",
            "baseUrl": "https://www.sbir.gov",
            "kind": "grant-opportunities",
            "defaultStrategy": "cheerio",
            "notes": "Small Business Innovation Research funding opportunities and award data."
        }),
        json!({
            "slug": "pew-research",
            "name": "Pew Research Center",
            "baseUrl": "https://www.pewresearch.org",
            "kind": "survey-and-social-trends",
            "defaultStrategy": "cheerio",
            "notes": "Survey reports, public opinion trends, and social-science datasets."
        }),
    ]
}

fn service_descriptor(state: &AppState) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "description": "Rust public-data ingestion, webhook, scraper-orchestration, grants, analysis, graph-data, white-paper evidence, and Spark/Airflow handoff service.",
        "scraperBaseUrl": state.config.scraper_base_url,
        "auth": {
            "operatorAuth": "X-Server-Auth or Auth",
            "webhookAuth": "X-Public-Data-Webhook-Secret when PUBLIC_DATA_WEBHOOK_SECRET is configured",
            "allowUnauthenticated": state.config.allow_unauthenticated,
            "allowUnauthenticatedWebhooks": state.config.allow_unauthenticated_webhooks
        },
        "subjects": {
            "ingestRequests": state.config.ingest_request_subject,
            "ingestResults": state.config.ingest_result_subject,
            "webhookEvents": state.config.webhook_event_subject,
            "pipelineJobs": state.config.pipeline_job_subject,
            "analysisResults": state.config.analysis_result_subject,
            "runtimeEvents": state.config.runtime_event_subject,
            "queueGroup": state.config.queue_group
        },
        "endpoints": {
            "home": "GET /",
            "descriptor": "GET /descriptor",
            "sources": "GET /sources",
            "schema": "GET /schema",
            "example": "GET /example",
            "datasets": "GET /datasets",
            "jobs": "GET /jobs",
            "webhookIngest": "POST /webhooks/ingest",
            "ingest": "POST /ingest",
            "scrape": "POST /scrape",
            "grantMatch": "POST /grants/match",
            "trends": "POST /analysis/trends",
            "correlations": "POST /analysis/correlations",
            "whitePaper": "POST /briefs/white-paper",
            "pipelineJobs": "POST /pipeline/jobs",
            "healthz": "GET /healthz",
            "readyz": "GET /readyz",
            "metrics": "GET /metrics",
            "apiDocs": "GET /docs/api"
        },
        "sources": source_catalog()
    })
}

fn schema_payload() -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "contracts": {
            "incomingRecord": {
                "recordId": "optional stable source id",
                "datasetId": "optional dataset grouping id",
                "source": "optional source override",
                "sourceUrl": "optional public URL",
                "title": "short title",
                "summary": "bounded abstract/body text",
                "publishedAt": "source timestamp string",
                "authors": ["names"],
                "tags": ["public", "science", "grant"],
                "metrics": { "numericFeature": 1.23 },
                "grant": "optional grant opportunity object",
                "raw": "bounded JSON metadata; do not include secrets"
            },
            "pipelineJob": {
                "jobType": "spark-etl | airflow-dag | correlation-analysis | white-paper-evidence",
                "datasetIds": ["dataset tokens"],
                "analysisIds": ["analysis tokens"],
                "sink": "minio://public-data/bronze or another approved downstream sink",
                "airflowDag": "optional DAG id",
                "sparkApp": "optional Spark app name",
                "parameters": {}
            }
        },
        "outputs": [
            "normalized dataset records",
            "grant matches",
            "trend summaries",
            "pairwise metric correlations",
            "graph data suitable for chart rendering",
            "white-paper evidence markdown",
            "Spark/Airflow pipeline job intents"
        ]
    })
}

fn example_payload() -> Value {
    json!({
        "ingest": {
            "source": "sbir",
            "datasetId": "sbir-energy-grants",
            "tags": ["grants", "energy", "public"],
            "records": [
                {
                    "recordId": "sbir-topic-001",
                    "title": "Grid resilience research topic",
                    "summary": "Public funding opportunity for grid analytics and resilience modeling.",
                    "sourceUrl": "https://www.sbir.gov/",
                    "metrics": { "awardAmountUsd": 250000, "phase": 1 },
                    "grant": {
                        "title": "Grid resilience research topic",
                        "agency": "DOE",
                        "program": "SBIR",
                        "amount": 250000,
                        "dueDate": "2026-09-15",
                        "eligibility": "US small businesses",
                        "topics": ["energy", "resilience", "analytics"],
                        "url": "https://www.sbir.gov/"
                    }
                }
            ],
            "pipeline": {
                "enabled": true,
                "jobType": "spark-etl",
                "sink": "minio://public-data/bronze/sbir-energy-grants",
                "airflowDag": "public_data_ingest",
                "sparkApp": "public-data-normalize"
            }
        },
        "scrape": {
            "source": "pew-research",
            "url": "https://www.pewresearch.org/",
            "strategy": "auto",
            "includeLinks": true,
            "pipeline": { "enabled": true, "jobType": "airflow-dag" }
        },
        "grantMatch": {
            "applicantProfile": "Small team building mathematical public-data models for energy, health, and civic infrastructure.",
            "focusAreas": ["energy", "AI", "public data", "research"],
            "minAmount": 50000
        }
    })
}

fn normalize_record(
    incoming: IncomingRecord,
    fallback_source: &str,
    fallback_dataset: &str,
    fallback_url: Option<&String>,
    inherited_tags: &[String],
    index: usize,
) -> Result<DataRecord, String> {
    let source = incoming
        .source
        .as_deref()
        .unwrap_or(fallback_source)
        .trim()
        .to_string();
    let source = clean_required(&source, "source")?;
    let dataset_id = incoming
        .dataset_id
        .as_deref()
        .unwrap_or(fallback_dataset)
        .trim()
        .to_string();
    let dataset_id = clean_required(&dataset_id, "datasetId")?;
    let record_id = incoming
        .record_id
        .unwrap_or_else(|| durable_token("record", &source, &format!("{}-{index}", now_ms())));
    let source_url = incoming
        .source_url
        .or_else(|| fallback_url.cloned())
        .filter(|url| validate_public_url(url).is_ok());
    let mut tags = inherited_tags.to_vec();
    tags.extend(incoming.tags.unwrap_or_default());
    if let Some(grant) = incoming.grant.as_ref() {
        tags.extend(grant.topics.iter().cloned());
        tags.push("grant".to_string());
    }
    let metrics = incoming
        .metrics
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, value)| value.is_finite())
        .map(|(key, value)| (key.chars().take(80).collect(), value))
        .collect::<BTreeMap<_, _>>();
    Ok(DataRecord {
        record_id: clean_required(&record_id, "recordId")?,
        dataset_id,
        source,
        source_url,
        title: clean_text(incoming.title.as_ref(), MAX_TEXT_LEN),
        summary: clean_text(incoming.summary.as_ref(), MAX_LONG_TEXT_LEN),
        published_at: clean_text(incoming.published_at.as_ref(), MAX_TOKEN_LEN),
        collected_at_ms: now_ms(),
        authors: incoming
            .authors
            .unwrap_or_default()
            .into_iter()
            .filter_map(|author| clean_text(Some(&author), MAX_TOKEN_LEN))
            .take(64)
            .collect(),
        tags: clean_tags(tags),
        metrics,
        grant: incoming.grant,
        raw: incoming.raw,
    })
}

fn store_records(state: &AppState, records: Vec<DataRecord>) {
    let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
    store.records.extend(records);
    if store.records.len() > MAX_RECORD_STORE {
        let overflow = store.records.len() - MAX_RECORD_STORE;
        store.records.drain(0..overflow);
    }
}

fn store_receipt(state: &AppState, receipt: WebhookReceipt) {
    let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
    store.webhook_receipts.push(receipt);
    if store.webhook_receipts.len() > MAX_RECEIPT_STORE {
        let overflow = store.webhook_receipts.len() - MAX_RECEIPT_STORE;
        store.webhook_receipts.drain(0..overflow);
    }
}

fn store_analysis(state: &AppState, result: AnalysisResult) {
    let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
    store.analyses.push(result);
    if store.analyses.len() > MAX_ANALYSIS_STORE {
        let overflow = store.analyses.len() - MAX_ANALYSIS_STORE;
        store.analyses.drain(0..overflow);
    }
}

fn store_pipeline_job(state: &AppState, job: PipelineJob) {
    let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
    store.pipeline_jobs.push(job);
    if store.pipeline_jobs.len() > MAX_PIPELINE_JOBS {
        let overflow = store.pipeline_jobs.len() - MAX_PIPELINE_JOBS;
        store.pipeline_jobs.drain(0..overflow);
    }
}

fn records_snapshot(state: &AppState) -> Vec<DataRecord> {
    state
        .store
        .read()
        .unwrap_or_else(|lock| lock.into_inner())
        .records
        .clone()
}

fn filter_records(
    records: &[DataRecord],
    dataset_ids: &Option<Vec<String>>,
    tags: &Option<Vec<String>>,
) -> Vec<DataRecord> {
    let dataset_filter = dataset_ids.as_ref().map(|values| {
        values
            .iter()
            .map(|value| value.trim().to_string())
            .collect::<BTreeSet<_>>()
    });
    let tag_filter = tags.as_ref().map(|values| {
        values
            .iter()
            .map(|value| value.trim().to_ascii_lowercase())
            .collect::<BTreeSet<_>>()
    });
    records
        .iter()
        .filter(|record| {
            dataset_filter
                .as_ref()
                .map(|filter| filter.contains(&record.dataset_id))
                .unwrap_or(true)
        })
        .filter(|record| {
            tag_filter
                .as_ref()
                .map(|filter| record.tags.iter().any(|tag| filter.contains(tag)))
                .unwrap_or(true)
        })
        .cloned()
        .collect()
}

async fn publish_json(state: &AppState, subject: &str, value: &Value) {
    let Some(nats) = state.nats.as_ref() else {
        return;
    };
    match serde_json::to_vec(value) {
        Ok(payload) => {
            if nats
                .publish(subject.to_string(), payload.into())
                .await
                .is_ok()
            {
                state
                    .metrics
                    .nats_published_total
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            tracing::error!("public-data failed to encode nats payload: {error}");
        }
    }
}

async fn publish_runtime_event(state: &AppState, event_type: &str, attrs: Value) {
    publish_json(
        state,
        &state.config.runtime_event_subject,
        &json!({
            "type": event_type,
            "source": SERVICE_NAME,
            "atMs": now_ms(),
            "attributes": attrs
        }),
    )
    .await;
}

async fn maybe_submit_pipeline_job(
    state: &AppState,
    request_id: &str,
    dataset_ids: Vec<String>,
    analysis_ids: Vec<String>,
    options: Option<PipelineOptions>,
) -> Option<PipelineJob> {
    let Some(options) = options else {
        return None;
    };
    if options.enabled == Some(false) {
        return None;
    }
    let request = PipelineRequest {
        request_id: Some(request_id.to_string()),
        job_type: options.job_type,
        dataset_ids: Some(dataset_ids),
        analysis_ids: Some(analysis_ids),
        sink: options.sink,
        airflow_dag: options.airflow_dag,
        spark_app: options.spark_app,
        parameters: options.parameters,
    };
    match create_pipeline_job(state, request).await {
        Ok(job) => Some(job),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            tracing::error!("public-data pipeline job creation failed: {error}");
            None
        }
    }
}

async fn create_pipeline_job(
    state: &AppState,
    request: PipelineRequest,
) -> Result<PipelineJob, String> {
    let request_id = request_id(request.request_id.as_ref(), "pipeline");
    let job_id = durable_token("public-data-job", &request_id, &now_ms().to_string());
    let job = PipelineJob {
        job_id,
        request_id,
        job_type: request
            .job_type
            .unwrap_or_else(|| "spark-etl".to_string())
            .chars()
            .take(MAX_TOKEN_LEN)
            .collect(),
        status: "queued".to_string(),
        dataset_ids: request.dataset_ids.unwrap_or_default(),
        analysis_ids: request.analysis_ids.unwrap_or_default(),
        sink: request
            .sink
            .unwrap_or_else(|| "minio://public-data/bronze".to_string())
            .chars()
            .take(MAX_TOKEN_LEN)
            .collect(),
        airflow_dag: request
            .airflow_dag
            .map(|value| value.chars().take(MAX_TOKEN_LEN).collect()),
        spark_app: request
            .spark_app
            .map(|value| value.chars().take(MAX_TOKEN_LEN).collect()),
        parameters: request.parameters.unwrap_or_else(|| json!({})),
        submitted_at_ms: now_ms(),
    };
    store_pipeline_job(state, job.clone());
    state
        .metrics
        .pipeline_jobs_total
        .fetch_add(1, Ordering::Relaxed);
    publish_json(
        state,
        &state.config.pipeline_job_subject,
        &json!({
            "schemaVersion": "public_data.pipeline.job.v1",
            "source": SERVICE_NAME,
            "job": job
        }),
    )
    .await;
    publish_runtime_event(
        state,
        "public_data.pipeline.job_queued",
        json!({ "jobId": job.job_id, "jobType": job.job_type }),
    )
    .await;
    Ok(job)
}

async fn process_ingest_request(state: &AppState, request: IngestRequest) -> Result<Value, String> {
    if request.records.len() > MAX_RECORDS_PER_REQUEST {
        return Err(format!(
            "records length must be at most {MAX_RECORDS_PER_REQUEST}"
        ));
    }
    let source = clean_required(&request.source, "source")?;
    if let Some(url) = request.source_url.as_ref() {
        validate_public_url(url)?;
    }
    let request_id = request_id(request.request_id.as_ref(), "ingest");
    let dataset_id = request
        .dataset_id
        .clone()
        .unwrap_or_else(|| durable_token("dataset", &source, &request_id));
    let inherited_tags = clean_tags(request.tags.unwrap_or_default());
    let mut records = Vec::new();
    for (index, incoming) in request.records.into_iter().enumerate() {
        records.push(normalize_record(
            incoming,
            &source,
            &dataset_id,
            request.source_url.as_ref(),
            &inherited_tags,
            index,
        )?);
    }
    let dataset_ids = records
        .iter()
        .map(|record| record.dataset_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let record_count = records.len();
    store_records(state, records.clone());
    state
        .metrics
        .records_ingested_total
        .fetch_add(record_count as u64, Ordering::Relaxed);
    let pipeline_job = maybe_submit_pipeline_job(
        state,
        &request_id,
        dataset_ids.clone(),
        Vec::new(),
        request.pipeline,
    )
    .await;
    let response = json!({
        "ok": true,
        "requestId": request_id,
        "schemaVersion": SCHEMA_VERSION,
        "source": source,
        "datasetIds": dataset_ids,
        "recordCount": record_count,
        "pipelineJob": pipeline_job,
        "ingestedAtMs": now_ms()
    });
    publish_json(
        state,
        &state.config.ingest_result_subject,
        &json!({
            "type": "public_data.ingest",
            "source": SERVICE_NAME,
            "result": response
        }),
    )
    .await;
    publish_runtime_event(
        state,
        "public_data.ingest",
        json!({ "recordCount": record_count, "source": source }),
    )
    .await;
    Ok(response)
}

async fn process_webhook(state: &AppState, request: WebhookIngestRequest) -> Result<Value, String> {
    let request_id = request_id(request.request_id.as_ref(), "webhook");
    let provider = clean_required(&request.provider, "provider")?;
    if let Some(url) = request.source_url.as_ref() {
        validate_public_url(url)?;
    }
    let records = request.records.unwrap_or_default();
    if records.len() > MAX_RECORDS_PER_REQUEST {
        return Err(format!(
            "records length must be at most {MAX_RECORDS_PER_REQUEST}"
        ));
    }
    let dataset_id = request
        .dataset_id
        .clone()
        .unwrap_or_else(|| durable_token("webhook-dataset", &provider, &request_id));
    let mut normalized = Vec::new();
    for (index, incoming) in records.into_iter().enumerate() {
        normalized.push(normalize_record(
            incoming,
            &provider,
            &dataset_id,
            request.source_url.as_ref(),
            &["webhook".to_string(), provider.clone()],
            index,
        )?);
    }
    let record_count = normalized.len();
    if record_count > 0 {
        store_records(state, normalized);
        state
            .metrics
            .records_ingested_total
            .fetch_add(record_count as u64, Ordering::Relaxed);
    }
    let event_type = request
        .event_type
        .unwrap_or_else(|| "provider.push".to_string());
    let receipt = WebhookReceipt {
        receipt_id: durable_token("public-data-webhook", &provider, &request_id),
        provider: provider.clone(),
        event_type: event_type.clone(),
        dataset_id: Some(dataset_id.clone()),
        source_url: request.source_url,
        received_at_ms: now_ms(),
        record_count,
        payload_shape: payload_shape(&request.payload),
    };
    store_receipt(state, receipt.clone());
    state
        .metrics
        .webhook_receipts_total
        .fetch_add(1, Ordering::Relaxed);
    let pipeline_job = maybe_submit_pipeline_job(
        state,
        &request_id,
        vec![dataset_id.clone()],
        Vec::new(),
        request.pipeline,
    )
    .await;
    let response = json!({
        "ok": true,
        "requestId": request_id,
        "receipt": receipt,
        "recordCount": record_count,
        "pipelineJob": pipeline_job
    });
    publish_json(
        state,
        &state.config.webhook_event_subject,
        &json!({
            "type": "public_data.webhook",
            "source": SERVICE_NAME,
            "receipt": response["receipt"]
        }),
    )
    .await;
    publish_json(
        state,
        &state.config.ingest_result_subject,
        &json!({
            "type": "public_data.webhook_ingest",
            "source": SERVICE_NAME,
            "result": response
        }),
    )
    .await;
    publish_runtime_event(
        state,
        "public_data.webhook",
        json!({ "provider": provider, "eventType": event_type, "recordCount": record_count }),
    )
    .await;
    Ok(response)
}

fn payload_shape(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let keys = map.keys().take(64).cloned().collect::<Vec<_>>();
            json!({ "type": "object", "keys": keys, "keyCount": map.len() })
        }
        Value::Array(values) => json!({ "type": "array", "length": values.len() }),
        Value::String(text) => json!({ "type": "string", "length": text.len() }),
        Value::Number(_) => json!({ "type": "number" }),
        Value::Bool(_) => json!({ "type": "boolean" }),
        Value::Null => json!({ "type": "null" }),
    }
}

async fn process_scrape_request(state: &AppState, request: ScrapeRequest) -> Result<Value, String> {
    validate_public_url(&request.url)?;
    let request_id = request_id(request.request_id.as_ref(), "scrape");
    let scrape_url = format!(
        "{}/scrape",
        state.config.scraper_base_url.trim_end_matches('/')
    );
    let mut body = json!({
        "requestId": request_id,
        "url": request.url.clone(),
        "strategy": request.strategy.clone().unwrap_or_else(|| "auto".to_string()),
        "renderJavaScript": request.render_javascript,
        "selector": request.selector.clone(),
        "selectors": request.selectors.clone(),
        "includeText": true,
        "includeLinks": request.include_links.unwrap_or(true),
        "maxTextChars": MAX_LONG_TEXT_LEN,
        "timeoutMs": 60000
    });
    strip_null_fields(&mut body);
    let mut builder = state.http.post(scrape_url).json(&body);
    if let Some(secret) = state.config.scraper_auth_secret.as_ref() {
        builder = builder.header("x-server-auth", secret);
    }
    let response = builder
        .send()
        .await
        .map_err(|error| format!("scraper request failed: {error}"))?;
    let status = response.status();
    let value = response
        .json::<Value>()
        .await
        .map_err(|error| format!("scraper response was not JSON: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "scraper returned {status}: {}",
            compact_json(&value)
        ));
    }
    let scraper_response: ScraperResponse = serde_json::from_value(value.clone())
        .map_err(|error| format!("scraper response shape mismatch: {error}"))?;
    let extraction = scraper_response.extraction.clone();
    let dataset_id = request
        .dataset_id
        .clone()
        .unwrap_or_else(|| durable_token("scrape-dataset", &request.source, &request_id));
    let mut metrics = BTreeMap::new();
    if let Some(extraction) = extraction.as_ref() {
        metrics.insert(
            "linkCount".to_string(),
            extraction
                .links
                .as_ref()
                .map(|links| links.len())
                .unwrap_or(0) as f64,
        );
        metrics.insert(
            "textLength".to_string(),
            extraction.text.as_ref().map(|text| text.len()).unwrap_or(0) as f64,
        );
    }
    let incoming = IncomingRecord {
        record_id: Some(durable_token("scrape-record", &request.source, &request_id)),
        dataset_id: Some(dataset_id.clone()),
        source: Some(request.source.clone()),
        source_url: scraper_response
            .final_url
            .clone()
            .or_else(|| Some(request.url.clone())),
        title: extraction.as_ref().and_then(|item| item.title.clone()),
        summary: extraction.as_ref().and_then(|item| item.text.clone()),
        published_at: None,
        authors: None,
        tags: request.tags.clone(),
        metrics: Some(metrics),
        grant: None,
        raw: Some(value.clone()),
    };
    let record = normalize_record(
        incoming,
        &request.source,
        &dataset_id,
        Some(&request.url),
        &clean_tags(vec!["scrape".to_string(), request.source.clone()]),
        0,
    )?;
    store_records(state, vec![record.clone()]);
    state
        .metrics
        .records_ingested_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .scrape_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let pipeline_job = maybe_submit_pipeline_job(
        state,
        &request_id,
        vec![dataset_id.clone()],
        Vec::new(),
        request.pipeline,
    )
    .await;
    let result = json!({
        "ok": true,
        "requestId": request_id,
        "source": request.source,
        "datasetId": dataset_id,
        "record": record,
        "scraper": scraper_response,
        "pipelineJob": pipeline_job
    });
    publish_json(
        state,
        &state.config.ingest_result_subject,
        &json!({
            "type": "public_data.scrape",
            "source": SERVICE_NAME,
            "result": result
        }),
    )
    .await;
    publish_runtime_event(
        state,
        "public_data.scrape",
        json!({ "datasetId": dataset_id }),
    )
    .await;
    Ok(result)
}

fn strip_null_fields(value: &mut Value) {
    if let Value::Object(map) = value {
        map.retain(|_, nested| !nested.is_null());
    }
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| "{}".to_string())
        .chars()
        .take(500)
        .collect()
}

fn metric_universe(records: &[DataRecord], requested: &Option<Vec<String>>) -> Vec<String> {
    if let Some(metrics) = requested {
        return metrics
            .iter()
            .filter_map(|metric| clean_text(Some(metric), 80))
            .collect();
    }
    let mut names = BTreeSet::new();
    for record in records {
        names.extend(record.metrics.keys().cloned());
    }
    names.into_iter().collect()
}

fn trend_summaries(records: &[DataRecord], requested: &Option<Vec<String>>) -> Vec<TrendSummary> {
    let mut trends = Vec::new();
    for metric in metric_universe(records, requested) {
        let values = records
            .iter()
            .filter_map(|record| record.metrics.get(&metric).copied())
            .filter(|value| value.is_finite())
            .collect::<Vec<_>>();
        if values.len() < 2 {
            continue;
        }
        let count = values.len();
        let mean = values.iter().sum::<f64>() / count as f64;
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let slope = simple_slope(&values);
        let direction = if slope.abs() < 1e-9 {
            "flat"
        } else if slope > 0.0 {
            "up"
        } else {
            "down"
        };
        trends.push(TrendSummary {
            metric,
            count,
            mean,
            min,
            max,
            slope_per_record: slope,
            direction: direction.to_string(),
        });
    }
    trends.sort_by(|left, right| {
        right
            .slope_per_record
            .abs()
            .partial_cmp(&left.slope_per_record.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    trends
}

fn simple_slope(values: &[f64]) -> f64 {
    let n = values.len() as f64;
    let mean_x = (n - 1.0) / 2.0;
    let mean_y = values.iter().sum::<f64>() / n;
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    for (index, value) in values.iter().enumerate() {
        let x = index as f64;
        numerator += (x - mean_x) * (value - mean_y);
        denominator += (x - mean_x).powi(2);
    }
    if denominator.abs() < f64::EPSILON {
        0.0
    } else {
        numerator / denominator
    }
}

fn correlation_summaries(
    records: &[DataRecord],
    requested: &Option<Vec<String>>,
) -> Vec<CorrelationSummary> {
    let metrics = metric_universe(records, requested);
    let mut out = Vec::new();
    for left_index in 0..metrics.len() {
        for right_index in (left_index + 1)..metrics.len() {
            let left = &metrics[left_index];
            let right = &metrics[right_index];
            let pairs = records
                .iter()
                .filter_map(|record| {
                    Some((
                        record.metrics.get(left).copied()?,
                        record.metrics.get(right).copied()?,
                    ))
                })
                .filter(|(a, b)| a.is_finite() && b.is_finite())
                .collect::<Vec<_>>();
            if pairs.len() < 3 {
                continue;
            }
            let pearson = pearson(&pairs);
            out.push(CorrelationSummary {
                left_metric: left.clone(),
                right_metric: right.clone(),
                count: pairs.len(),
                pearson,
                strength: correlation_strength(pearson).to_string(),
            });
        }
    }
    out.sort_by(|left, right| {
        right
            .pearson
            .abs()
            .partial_cmp(&left.pearson.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

fn pearson(pairs: &[(f64, f64)]) -> f64 {
    let n = pairs.len() as f64;
    let mean_x = pairs.iter().map(|pair| pair.0).sum::<f64>() / n;
    let mean_y = pairs.iter().map(|pair| pair.1).sum::<f64>() / n;
    let mut numerator = 0.0;
    let mut left_sum = 0.0;
    let mut right_sum = 0.0;
    for (left, right) in pairs {
        let dx = left - mean_x;
        let dy = right - mean_y;
        numerator += dx * dy;
        left_sum += dx * dx;
        right_sum += dy * dy;
    }
    let denominator = left_sum.sqrt() * right_sum.sqrt();
    if denominator.abs() < f64::EPSILON {
        0.0
    } else {
        (numerator / denominator).clamp(-1.0, 1.0)
    }
}

fn correlation_strength(value: f64) -> &'static str {
    let abs = value.abs();
    if abs >= 0.85 {
        "very-strong"
    } else if abs >= 0.65 {
        "strong"
    } else if abs >= 0.40 {
        "moderate"
    } else if abs >= 0.20 {
        "weak"
    } else {
        "very-weak"
    }
}

fn graph_from_trends(trends: &[TrendSummary], records: &[DataRecord]) -> GraphData {
    let series = trends
        .iter()
        .take(8)
        .map(|trend| {
            let points = records
                .iter()
                .filter_map(|record| {
                    Some(GraphPoint {
                        x: record.collected_at_ms as f64,
                        y: *record.metrics.get(&trend.metric)?,
                        label: record.title.clone(),
                    })
                })
                .take(MAX_GRAPH_POINTS)
                .collect::<Vec<_>>();
            GraphSeries {
                name: trend.metric.clone(),
                points,
            }
        })
        .collect();
    GraphData {
        graph_type: "line".to_string(),
        title: "Public data metric trends".to_string(),
        x_label: "collectedAtMs".to_string(),
        y_label: "metricValue".to_string(),
        series,
    }
}

fn model_notes() -> Vec<ModelNote> {
    vec![
        ModelNote {
            name: "Ordinary Least Squares Trend".to_string(),
            equation: "y_t = alpha + beta t + epsilon_t".to_string(),
            use_case: "Estimate first-pass direction and slope for normalized public metrics."
                .to_string(),
        },
        ModelNote {
            name: "Pearson Correlation".to_string(),
            equation: "rho_xy = cov(x,y) / (sigma_x sigma_y)".to_string(),
            use_case: "Identify candidate relationships for later causal review, not causal proof."
                .to_string(),
        },
        ModelNote {
            name: "Evidence-Weighted Grant Fit".to_string(),
            equation: "score = topic_overlap + source_prior + amount_fit + eligibility_fit"
                .to_string(),
            use_case:
                "Rank grant opportunities against a declared applicant profile and focus areas."
                    .to_string(),
        },
    ]
}

fn build_analysis_result(
    kind: &str,
    request_id: String,
    records: Vec<DataRecord>,
    requested_metrics: Option<Vec<String>>,
    grants: Vec<GrantMatch>,
    markdown: Option<String>,
) -> AnalysisResult {
    let dataset_ids = records
        .iter()
        .map(|record| record.dataset_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let trends = trend_summaries(&records, &requested_metrics);
    let correlations = correlation_summaries(&records, &requested_metrics);
    let summary = format!(
        "Analyzed {} records across {} datasets; {} trends and {} metric correlations qualified.",
        records.len(),
        dataset_ids.len(),
        trends.len(),
        correlations.len()
    );
    let graph = graph_from_trends(&trends, &records);
    AnalysisResult {
        analysis_id: durable_token(
            "public-data-analysis",
            kind,
            &format!("{}-{request_id}", now_ms()),
        ),
        request_id,
        kind: kind.to_string(),
        generated_at_ms: now_ms(),
        dataset_ids,
        summary,
        graph,
        trends,
        correlations,
        grants,
        model_notes: model_notes(),
        markdown,
    }
}

async fn publish_analysis(state: &AppState, result: &AnalysisResult) {
    publish_json(
        state,
        &state.config.analysis_result_subject,
        &json!({
            "schemaVersion": "public_data.analysis.v1",
            "source": SERVICE_NAME,
            "result": result
        }),
    )
    .await;
    publish_runtime_event(
        state,
        "public_data.analysis",
        json!({ "analysisId": result.analysis_id, "kind": result.kind }),
    )
    .await;
}

fn grant_matches_from_records(
    records: &[DataRecord],
    request: &GrantMatchRequest,
) -> Vec<GrantMatch> {
    let focus = clean_tags(request.focus_areas.clone());
    let profile_terms = clean_tags(
        request
            .applicant_profile
            .split_whitespace()
            .map(|value| value.to_string())
            .collect::<Vec<_>>(),
    );
    let min_amount = request.min_amount.unwrap_or(0.0);
    let mut matches = Vec::new();
    for record in records {
        let Some(grant) = record.grant.as_ref() else {
            continue;
        };
        if grant.amount.unwrap_or(0.0) < min_amount {
            continue;
        }
        let mut reasons = Vec::new();
        let mut score = 0.0;
        let mut haystack = record.tags.clone();
        haystack.extend(grant.topics.iter().cloned());
        if let Some(eligibility) = grant.eligibility.as_ref() {
            haystack.extend(
                eligibility
                    .split_whitespace()
                    .map(|value| value.to_string()),
            );
        }
        if let Some(summary) = record.summary.as_ref() {
            haystack.extend(summary.split_whitespace().map(|value| value.to_string()));
        }
        haystack.extend(
            grant
                .title
                .split_whitespace()
                .map(|value| value.to_string()),
        );
        let haystack = clean_tags(haystack);
        let focus_hits = focus
            .iter()
            .filter(|term| {
                haystack
                    .iter()
                    .any(|item| item.contains(*term) || term.contains(item))
            })
            .count();
        if focus_hits > 0 {
            score += focus_hits as f64 * 2.5;
            reasons.push(format!("{focus_hits} focus-area terms matched"));
        }
        let profile_hits = profile_terms
            .iter()
            .filter(|term| {
                haystack
                    .iter()
                    .any(|item| item.contains(*term) || term.contains(item))
            })
            .count();
        if profile_hits > 0 {
            score += profile_hits as f64 * 0.5;
            reasons.push(format!("{profile_hits} applicant-profile terms matched"));
        }
        if record.source.to_ascii_lowercase().contains("sbir")
            || grant
                .program
                .as_ref()
                .map(|program| program.to_ascii_lowercase().contains("sbir"))
                .unwrap_or(false)
        {
            score += 1.5;
            reasons.push("SBIR source/program prior".to_string());
        }
        if grant.amount.unwrap_or(0.0) > 0.0 {
            score += (grant.amount.unwrap_or(0.0).log10() / 10.0).clamp(0.0, 1.0);
            reasons.push("funding amount is specified".to_string());
        }
        if grant.due_date.is_some() {
            score += 0.4;
            reasons.push("deadline is available".to_string());
        }
        if score <= 0.0 {
            continue;
        }
        matches.push(GrantMatch {
            record_id: record.record_id.clone(),
            dataset_id: record.dataset_id.clone(),
            source: record.source.clone(),
            title: grant.title.clone(),
            url: grant.url.clone().or_else(|| record.source_url.clone()),
            agency: grant.agency.clone(),
            program: grant.program.clone(),
            amount: grant.amount,
            due_date: grant.due_date.clone(),
            score,
            reasons,
        });
    }
    matches.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    matches.truncate(request.limit.unwrap_or(20).min(100));
    matches
}

fn markdown_brief(
    request: &WhitePaperRequest,
    result: &AnalysisResult,
    record_count: usize,
) -> String {
    let title = request
        .title
        .clone()
        .unwrap_or_else(|| "Public Data Evidence Brief".to_string());
    let mut lines = vec![
        format!("# {title}"),
        String::new(),
        format!("Research question: {}", request.research_question.trim()),
        String::new(),
        format!(
            "Evidence base: {record_count} normalized records across {} datasets.",
            result.dataset_ids.len()
        ),
        String::new(),
        "## Candidate Trends".to_string(),
    ];
    if result.trends.is_empty() {
        lines.push("- No numeric trend had enough points yet.".to_string());
    } else {
        for trend in result.trends.iter().take(12) {
            lines.push(format!(
                "- `{}` is `{}` with slope {:.4}, mean {:.4}, range {:.4}..{:.4} across {} points.",
                trend.metric,
                trend.direction,
                trend.slope_per_record,
                trend.mean,
                trend.min,
                trend.max,
                trend.count
            ));
        }
    }
    lines.push(String::new());
    lines.push("## Candidate Correlations".to_string());
    if result.correlations.is_empty() {
        lines.push("- No metric pair had enough paired observations yet.".to_string());
    } else {
        for correlation in result.correlations.iter().take(12) {
            lines.push(format!(
                "- `{}` vs `{}`: Pearson {:.4} ({}, n={}).",
                correlation.left_metric,
                correlation.right_metric,
                correlation.pearson,
                correlation.strength,
                correlation.count
            ));
        }
    }
    if !result.grants.is_empty() {
        lines.push(String::new());
        lines.push("## Grant Opportunities".to_string());
        for grant in result.grants.iter().take(12) {
            lines.push(format!(
                "- `{}` score {:.2}; agency={}; program={}; amount={}.",
                grant.title,
                grant.score,
                grant
                    .agency
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                grant
                    .program
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                grant
                    .amount
                    .map(|amount| format!("{amount:.0}"))
                    .unwrap_or_else(|| "unknown".to_string())
            ));
        }
    }
    lines.push(String::new());
    lines.push("## Model Notes".to_string());
    for note in &result.model_notes {
        lines.push(format!(
            "- {}: `{}`. {}",
            note.name, note.equation, note.use_case
        ));
    }
    lines.push(String::new());
    lines.push("This brief is generated evidence for internal research review. Correlations are not causal claims until validated against domain assumptions, confounders, and source quality.".to_string());
    lines.join("\n")
}

async fn root() -> Html<String> {
    Html(render_root().into_string())
}

/// Landing page. maud compile-checks the structure and auto-escapes any dynamic
/// value, replacing the previous hand-written HTML string literal.
fn render_root() -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "dd-public-data-server" }
                style { (PreEscaped(ROOT_CSS)) }
            }
            body {
                main {
                    h1 { "dd-public-data-server" }
                    p { "Rust public-data ingestion service for webhooks, scraper orchestration, public/government source normalization, grant matching, trend/correlation graph data, white-paper evidence briefs, and Spark/Airflow pipeline job intents." }
                    div class="actions" {
                        a class="button" href="./ui" { "Operator UI" }
                        a class="link" href="./docs/api" { "API docs" }
                    }
                    div class="grid" {
                        div class="card" { strong { "Sources" } p { code { "GET /sources" } } }
                        div class="card" { strong { "Ingest" } p { code { "POST /ingest" } " and " code { "POST /webhooks/ingest" } } }
                        div class="card" { strong { "Analysis" } p { code { "POST /analysis/trends" } ", " code { "/analysis/correlations" } } }
                        div class="card" { strong { "Docs" } p { code { "GET /docs/api" } } }
                    }
                }
            }
        }
    }
}

const ROOT_CSS: &str = r#"body { margin: 0; font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: #172026; background: #f7f8fb; }
    main { max-width: 1040px; margin: 0 auto; padding: 40px 24px; }
    h1 { margin: 0 0 8px; font-size: 32px; letter-spacing: 0; }
    p { line-height: 1.5; max-width: 780px; }
    .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 12px; margin-top: 24px; }
    .card { background: white; border: 1px solid #d8dde6; border-radius: 8px; padding: 16px; }
    .actions { display: flex; flex-wrap: wrap; gap: 10px; margin-top: 20px; }
    .button { color: white; background: #1f6f70; border-radius: 6px; padding: 10px 14px; text-decoration: none; font-weight: 700; }
    .link { color: #1f5f87; font-weight: 700; }
    code { background: #eef1f5; border-radius: 4px; padding: 2px 5px; }"#;

/// The operator dashboard shell. HTMX wiring (hx-get/hx-post/hx-target/hx-swap/
/// hx-trigger/hx-disabled-elt) and the exact CSS are preserved; the initial
/// fragment bodies are embedded as already-rendered maud `Markup`.
fn render_ui_shell(state: &AppState) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Public Data Operations" }
                script src="https://unpkg.com/htmx.org@2.0.4/dist/htmx.min.js" defer {}
                style { (PreEscaped(UI_SHELL_CSS)) }
            }
            body {
                header {
                    div {
                        p class="kicker" { "Public data" }
                        h1 { "Evidence Operations" }
                        p class="muted" { "Scrapes, webhooks, grants, model evidence, and pipeline handoffs." }
                    }
                    nav aria-label="Public data navigation" {
                        a href="./" { "Home" }
                        a href="./docs/api" { "API docs" }
                        a href="./metrics" { "Metrics" }
                    }
                }
                main {
                    section id="summary" hx-get="./ui/fragments/summary" hx-trigger="load, every 15s" hx-swap="innerHTML" {
                        (render_ui_summary(state))
                    }
                    div class="grid split" {
                        section class="panel" {
                            h2 { "Scrape Intake" }
                            form hx-post="./ui/actions/scrape" hx-target="#scrape-result" hx-swap="innerHTML" hx-disabled-elt="button" {
                                label { "Source"
                                    input name="source" value="sbir" autocomplete="off";
                                }
                                label { "Public URL"
                                    input name="url" type="url" value="https://www.sbir.gov/" required;
                                }
                                label { "Dataset"
                                    input name="dataset_id" value="sbir-opportunities" autocomplete="off";
                                }
                                label { "Strategy"
                                    select name="strategy" {
                                        option value="auto" { "auto" }
                                        option value="native-fetch" { "native-fetch" }
                                        option value="cheerio" { "cheerio" }
                                        option value="browser" { "browser" }
                                    }
                                }
                                label { "Selector"
                                    input name="selector" autocomplete="off" placeholder="main";
                                }
                                label { "Tags"
                                    input name="tags" value="grants, public-data" autocomplete="off";
                                }
                                div class="checks" {
                                    label class="check" { input type="checkbox" name="include_links" checked; " Links" }
                                    label class="check" { input type="checkbox" name="render_javascript"; " JavaScript" }
                                    label class="check" { input type="checkbox" name="pipeline_enabled" checked; " Pipeline" }
                                }
                                button class="primary" type="submit" { "Run Scrape" }
                            }
                            div id="scrape-result" class="result" aria-live="polite" {}
                        }
                        section id="sources" class="panel" hx-get="./ui/fragments/sources" hx-trigger="load" hx-swap="innerHTML" {
                            (render_ui_sources())
                        }
                    }
                    section id="recent-records" class="panel" style="margin-top:16px" hx-get="./ui/fragments/recent-records" hx-trigger="load, every 20s" hx-swap="innerHTML" {
                        (render_ui_recent_records(state))
                    }
                }
            }
        }
    }
}

const UI_SHELL_CSS: &str = r#":root { color-scheme: light; --ink: #172026; --muted: #5d6975; --line: #d8dde6; --panel: #ffffff; --page: #f5f7f8; --teal: #1f6f70; --blue: #1f5f87; --amber: #9a6615; --danger: #a43d3d; }
    * { box-sizing: border-box; }
    body { margin: 0; font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: var(--ink); background: var(--page); }
    header, main { max-width: 1180px; margin: 0 auto; padding: 0 24px; }
    header { display: flex; align-items: flex-end; justify-content: space-between; gap: 20px; padding-top: 28px; padding-bottom: 18px; border-bottom: 1px solid var(--line); }
    h1, h2, h3, p { margin-top: 0; letter-spacing: 0; }
    h1 { margin-bottom: 4px; font-size: 28px; }
    h2 { font-size: 18px; margin-bottom: 14px; }
    h3 { font-size: 15px; margin-bottom: 6px; }
    p { line-height: 1.5; }
    nav { display: flex; flex-wrap: wrap; gap: 10px; justify-content: flex-end; }
    nav a, button, .button { min-height: 38px; display: inline-flex; align-items: center; justify-content: center; border-radius: 6px; border: 1px solid var(--line); padding: 8px 12px; color: var(--ink); background: #ffffff; text-decoration: none; font: inherit; font-weight: 700; cursor: pointer; }
    button.primary { color: #ffffff; background: var(--teal); border-color: var(--teal); }
    button:disabled { opacity: 0.6; cursor: wait; }
    main { padding-top: 22px; padding-bottom: 44px; }
    .kicker { margin-bottom: 4px; color: var(--teal); font-size: 12px; font-weight: 800; text-transform: uppercase; }
    .grid { display: grid; gap: 14px; }
    .stats { grid-template-columns: repeat(auto-fit, minmax(170px, 1fr)); }
    .split { grid-template-columns: minmax(300px, 420px) 1fr; align-items: start; margin-top: 16px; }
    .panel, .stat { background: var(--panel); border: 1px solid var(--line); border-radius: 8px; padding: 16px; }
    .stat strong { display: block; font-size: 26px; }
    .muted { color: var(--muted); }
    .status-row { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 10px; margin-top: 12px; }
    .badge, .pill { display: inline-flex; align-items: center; border-radius: 999px; padding: 3px 8px; font-size: 12px; font-weight: 800; background: #e9f2f2; color: #15595a; }
    .badge.warn { background: #fff1d6; color: var(--amber); }
    .badge.danger { background: #f8dddd; color: var(--danger); }
    .pill { margin: 2px 4px 2px 0; background: #eef1f5; color: #35414d; }
    form { display: grid; gap: 12px; }
    label { display: grid; gap: 6px; font-size: 13px; font-weight: 800; color: #35414d; }
    input, select { width: 100%; min-height: 38px; border: 1px solid var(--line); border-radius: 6px; padding: 8px 10px; font: inherit; background: #ffffff; color: var(--ink); }
    .checks { display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); gap: 8px; }
    .check { display: flex; align-items: center; gap: 8px; font-weight: 700; }
    .check input { width: 16px; min-height: 16px; }
    .result { min-height: 48px; }
    .notice { border: 1px solid var(--line); border-left: 4px solid var(--teal); border-radius: 8px; padding: 12px; background: #ffffff; }
    .notice.error { border-left-color: var(--danger); }
    table { width: 100%; border-collapse: collapse; font-size: 14px; }
    th, td { padding: 10px 8px; text-align: left; border-bottom: 1px solid var(--line); vertical-align: top; }
    th { color: #35414d; font-size: 12px; text-transform: uppercase; }
    code { background: #eef1f5; border-radius: 4px; padding: 2px 5px; }
    .empty { border: 1px dashed var(--line); border-radius: 8px; color: var(--muted); padding: 14px; background: #ffffff; }
    @media (max-width: 820px) { header { align-items: flex-start; flex-direction: column; } nav { justify-content: flex-start; } .split { grid-template-columns: 1fr; } }"#;

fn render_stat(label: &str, value: impl ToString, note: &str) -> Markup {
    html! {
        div class="stat" {
            span class="muted" { (label) }
            strong { (value.to_string()) }
            span class="muted" { (note) }
        }
    }
}

fn render_badge(label: &str, tone: &str) -> Markup {
    html! {
        span class={ "badge " (tone) } { (label) }
    }
}

fn render_ui_summary(state: &AppState) -> Markup {
    let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
    let dataset_count = store
        .records
        .iter()
        .map(|record| record.dataset_id.clone())
        .collect::<BTreeSet<_>>()
        .len();
    let grant_count = store
        .records
        .iter()
        .filter(|record| record.grant.is_some())
        .count();
    let last_analysis = store.analyses.last().cloned();
    let last_job = store.pipeline_jobs.last().cloned();
    let record_count = store.records.len();
    let webhook_count = store.webhook_receipts.len();
    let analysis_count = store.analyses.len();
    let job_count = store.pipeline_jobs.len();
    drop(store);

    let nats = if state.nats.is_some() {
        render_badge("NATS connected", "")
    } else {
        render_badge("NATS disabled", "warn")
    };
    let auth = if state.config.allow_unauthenticated {
        render_badge("auth relaxed", "warn")
    } else if state.config.server_auth_secret.is_some() {
        render_badge("auth configured", "")
    } else {
        render_badge("auth missing", "danger")
    };
    html! {
        div class="grid stats" {
            (render_stat("Records", record_count, "bounded in-process ledger"))
            (render_stat("Datasets", dataset_count, "normalized collections"))
            (render_stat("Grants", grant_count, "records with funding data"))
            (render_stat("Webhooks", webhook_count, "provider receipts"))
            (render_stat("Analyses", analysis_count, "trend/correlation/brief outputs"))
            (render_stat("Jobs", job_count, "pipeline intents"))
        }
        div class="status-row" {
            div class="panel" {
                h3 { "Runtime" }
                (nats) " " (auth) " "
                p class="muted" {
                    "NATS messages: " (state.metrics.nats_messages_total.load(Ordering::Relaxed))
                    "; published: " (state.metrics.nats_published_total.load(Ordering::Relaxed))
                }
            }
            div class="panel" {
                h3 { "Last Analysis" }
                @if let Some(analysis) = last_analysis {
                    div { strong { (analysis.kind) } p class="muted" { (analysis.summary) } }
                } @else {
                    div class="muted" { "No analyses yet." }
                }
            }
            div class="panel" {
                h3 { "Last Pipeline Job" }
                @if let Some(job) = last_job {
                    div { strong { (job.job_type) } p class="muted" { code { (job.job_id) } " " (job.status) } }
                } @else {
                    div class="muted" { "No pipeline jobs yet." }
                }
            }
        }
    }
}

fn source_str(source: &Value, key: &str) -> String {
    source
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn render_ui_sources() -> Markup {
    html! {
        h2 { "Source Catalog" }
        table {
            thead { tr { th { "Source" } th { "Base URL" } th { "Strategy" } th { "Notes" } } }
            tbody {
                @for source in source_catalog() {
                    tr {
                        td {
                            strong { (source_str(&source, "name")) }
                            div class="muted" { (source_str(&source, "kind")) }
                        }
                        td { (source_str(&source, "baseUrl")) }
                        td { code { (source_str(&source, "defaultStrategy")) } }
                        td { (source_str(&source, "notes")) }
                    }
                }
            }
        }
    }
}

fn render_pills(values: &[String]) -> Markup {
    html! {
        @if values.is_empty() {
            span class="muted" { "none" }
        } @else {
            @for value in values.iter().take(8) {
                span class="pill" { (value) }
            }
        }
    }
}

fn render_metric_pills(metrics: &BTreeMap<String, f64>) -> Markup {
    html! {
        @if metrics.is_empty() {
            span class="muted" { "none" }
        } @else {
            @for (key, value) in metrics.iter().take(8) {
                span class="pill" { (key) " " (format!("{value:.2}")) }
            }
        }
    }
}

fn render_ui_recent_records(state: &AppState) -> Markup {
    let store = state.store.read().unwrap_or_else(|lock| lock.into_inner());
    html! {
        h2 { "Recent Records" }
        @if store.records.is_empty() {
            div class="empty" { "No records yet." }
        } @else {
            table {
                thead { tr { th { "Record" } th { "Dataset" } th { "Source" } th { "Tags" } th { "Metrics" } } }
                tbody {
                    @for record in store.records.iter().rev().take(12) {
                        @let title = record.title.as_deref().unwrap_or(&record.record_id);
                        tr {
                            td {
                                strong { (title) }
                                div class="muted" { code { (record.record_id) } }
                            }
                            td { (record.dataset_id) }
                            td { (record.source) }
                            td { (render_pills(&record.tags)) }
                            td { (render_metric_pills(&record.metrics)) }
                        }
                    }
                }
            }
        }
    }
}

fn render_ui_notice(title: &str, detail: &str, error: bool) -> Markup {
    let class_name = if error { "notice error" } else { "notice" };
    html! {
        div class=(class_name) {
            strong { (title) }
            p class="muted" { (detail) }
        }
    }
}

fn render_ui_scrape_result(value: &Value) -> Markup {
    let request_id = value
        .get("requestId")
        .and_then(Value::as_str)
        .unwrap_or("scrape");
    let dataset_id = value
        .get("datasetId")
        .and_then(Value::as_str)
        .unwrap_or("dataset");
    let record_title = value
        .pointer("/record/title")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/record/recordId").and_then(Value::as_str))
        .unwrap_or("record");
    let status = value
        .pointer("/scraper/status")
        .and_then(Value::as_u64)
        .map(|status| status.to_string())
        .unwrap_or_else(|| "ok".to_string());
    html! {
        div class="notice" {
            strong { "Scrape accepted" }
            p class="muted" {
                code { (request_id) } " stored " code { (record_title) } " in " code { (dataset_id) } "; scraper status " (status) "."
            }
            button type="button" hx-get="./ui/fragments/summary" hx-target="#summary" hx-swap="innerHTML" { "Refresh Board" }
        }
    }
}

fn ui_auth_failure_response(state: &AppState, failure: AuthFailure) -> Response {
    state
        .metrics
        .auth_failures_total
        .fetch_add(1, Ordering::Relaxed);
    let message = match failure {
        AuthFailure::MissingSecret => "server auth secret is not configured",
        AuthFailure::Unauthorized => "unauthorized",
    };
    (
        StatusCode::UNAUTHORIZED,
        Html(render_ui_notice("Unauthorized", message, true).into_string()),
    )
        .into_response()
}

async fn ui_dashboard(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return ui_auth_failure_response(&state, failure);
    }
    Html(render_ui_shell(&state).into_string()).into_response()
}

async fn ui_summary_fragment(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return ui_auth_failure_response(&state, failure);
    }
    Html(render_ui_summary(&state).into_string()).into_response()
}

async fn ui_sources_fragment(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return ui_auth_failure_response(&state, failure);
    }
    Html(render_ui_sources().into_string()).into_response()
}

async fn ui_recent_records_fragment(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return ui_auth_failure_response(&state, failure);
    }
    Html(render_ui_recent_records(&state).into_string()).into_response()
}

async fn ui_scrape_action(
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

async fn descriptor(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(service_descriptor(&state))
}

async fn sources(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({ "ok": true, "sources": source_catalog() }))
}

async fn schema(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(schema_payload())
}

async fn example(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({ "ok": true, "example": example_payload() }))
}

async fn datasets(State(state): State<AppState>, headers: HeaderMap) -> Response {
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

async fn jobs(State(state): State<AppState>, headers: HeaderMap) -> Response {
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

async fn webhook_ingest_http(
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

async fn ingest_http(
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

async fn scrape_http(
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

async fn grant_match_http(
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

async fn trends_http(
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

async fn correlations_http(
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

async fn white_paper_http(
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

async fn pipeline_jobs_http(
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

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
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

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "natsConfigured": state.nats.is_some(),
        "scraperBaseUrl": state.config.scraper_base_url
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
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

async fn api_docs_html() -> Html<&'static str> {
    Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

async fn run_nats_loop(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        tracing::info!("public-data nats loop disabled: NATS_URL is not configured");
        return;
    };
    tracing::info!(
        "public-data nats loop starting: subject={} queueGroup={} resultSubject={}",
        state.config.ingest_request_subject,
        state.config.queue_group,
        state.config.ingest_result_subject
    );
    // Bound in-flight handlers. Each message can trigger a scrape (outbound HTTP)
    // or ingest (DB writes); previously every one was `tokio::spawn`ed with no
    // ceiling, so a burst could spawn unbounded concurrent scrapes and exhaust
    // the pod. Acquiring a permit before spawning also backpressures the
    // subscription instead of piling work on. Kept modest since scrapes are heavy.
    let max_concurrency = env::var("PUBLIC_DATA_NATS_MAX_CONCURRENCY")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(32);
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(max_concurrency));
    // Durable JetStream consumer instead of core queue_subscribe. Core NATS is
    // at-most-once: a crash between receiving a message and finishing the scrape
    // or ingest silently dropped the work. A JetStream WorkQueue durable pull
    // consumer redelivers unacked messages, and we ack ONLY after the handler
    // succeeds. A long scrape is kept alive with AckKind::Progress heartbeats so
    // it isn't redelivered mid-flight; a message that exhausts max_deliver is
    // Term'd and copied to the dead-letter subject so it can't linger in the
    // WorkQueue stream until max_age.
    let jetstream = async_nats::jetstream::new(nats.clone());
    let stream_name = env_value("PUBLIC_DATA_INGEST_STREAM", "DD_PUBLIC_DATA_INGEST");
    let consumer_name = env_value("PUBLIC_DATA_INGEST_CONSUMER", &state.config.queue_group);
    let ack_wait = Duration::from_secs(
        env::var("PUBLIC_DATA_NATS_ACK_WAIT_SECONDS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(120),
    );
    let max_deliver: i64 = env::var("PUBLIC_DATA_NATS_MAX_DELIVER")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(5);
    let nak_delay = Duration::from_secs(15);
    let dlq_subject = env_value(
        "PUBLIC_DATA_NATS_DLQ_SUBJECT",
        "dd.remote.public_data.ingest.deadletter",
    );
    let ack_progress_every = (ack_wait / 3).max(Duration::from_secs(5));

    loop {
        let consumer = match build_ingest_consumer(
            &jetstream,
            &stream_name,
            &state.config.ingest_request_subject,
            &consumer_name,
            ack_wait,
            max_deliver,
        )
        .await
        {
            Ok(consumer) => consumer,
            Err(error) => {
                tracing::error!("public-data jetstream consumer setup failed: {error}; retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let mut messages = match consumer.messages().await {
            Ok(messages) => messages,
            Err(error) => {
                tracing::error!("public-data jetstream messages() failed: {error}; retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        while let Some(next) = messages.next().await {
            let message = match next {
                Ok(message) => message,
                Err(error) => {
                    tracing::error!("public-data jetstream fetch failed: {error}; re-subscribing");
                    break;
                }
            };
            state
                .metrics
                .nats_messages_total
                .fetch_add(1, Ordering::Relaxed);
            if message.payload.len() > MAX_NATS_PAYLOAD_BYTES {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                tracing::error!(
                    "public-data rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                    message.payload.len()
                );
                // An oversize payload will never get smaller; ack to drop it.
                let _ = message.ack().await;
                continue;
            }
            // Backpressure point: wait for a concurrency permit before spawning.
            let permit = match semaphore.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => break,
            };
            let task_state = state.clone();
            let dlq_nats = nats.clone();
            let dlq = dlq_subject.clone();
            tokio::spawn(async move {
                let _permit = permit; // released when this handler finishes
                let payload = message.payload.to_vec();
                // Process under an ack-progress heartbeat so a long scrape keeps
                // the ack deadline alive instead of being redelivered mid-flight.
                let outcome = run_with_ack_progress(
                    &message,
                    ack_progress_every,
                    process_ingest_payload(&task_state, &payload),
                )
                .await;
                match outcome {
                    Ok(()) => {
                        if let Err(error) = message.ack().await {
                            tracing::error!("public-data ack failed: {error}");
                        }
                    }
                    Err(error) => {
                        task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                        tracing::error!("public-data nats request failed: {error}");
                        let delivered = message.info().map(|info| info.delivered).unwrap_or(0);
                        if max_deliver > 0 && delivered >= max_deliver {
                            // Exhausted: preserve on the dead-letter subject, then
                            // Term so it leaves the WorkQueue stream immediately.
                            let _ = dlq_nats.publish(dlq.clone(), payload.into()).await;
                            let _ = dlq_nats.flush().await;
                            let _ = message
                                .ack_with(async_nats::jetstream::AckKind::Term)
                                .await;
                        } else {
                            let _ = message
                                .ack_with(async_nats::jetstream::AckKind::Nak(Some(nak_delay)))
                                .await;
                        }
                    }
                }
            });
        }
        tracing::error!(
            "public-data jetstream message stream ended (subject={}); re-subscribing in 5s",
            state.config.ingest_request_subject
        );
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Ensure the ingest WorkQueue stream and its durable pull consumer exist. The
/// service creates them itself (like dd-remote-queue-consumer) rather than
/// relying on a declarative CRD, so a fresh cluster boots without manual setup.
async fn build_ingest_consumer(
    jetstream: &async_nats::jetstream::Context,
    stream_name: &str,
    subject: &str,
    consumer_name: &str,
    ack_wait: Duration,
    max_deliver: i64,
) -> Result<async_nats::jetstream::consumer::PullConsumer, Box<dyn Error + Send + Sync>> {
    let stream = jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: stream_name.to_string(),
            subjects: vec![subject.to_string()],
            retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
            max_age: Duration::from_secs(60 * 60 * 24 * 7),
            max_message_size: MAX_NATS_PAYLOAD_BYTES as i32,
            ..Default::default()
        })
        .await?;
    let consumer = stream
        .get_or_create_consumer::<async_nats::jetstream::consumer::pull::Config>(
            consumer_name,
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(consumer_name.to_string()),
                filter_subject: subject.to_string(),
                ack_wait,
                max_deliver,
                ..Default::default()
            },
        )
        .await?;
    Ok(consumer)
}

/// Await `fut` while extending the JetStream ack deadline for `message` with
/// AckKind::Progress every `interval`, so a legitimately long scrape/ingest is
/// not mistaken for a stalled delivery and redelivered.
async fn run_with_ack_progress<F, T>(
    message: &async_nats::jetstream::Message,
    interval: Duration,
    fut: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    let mut fut = std::pin::pin!(fut);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await; // consume the immediate first tick
    loop {
        tokio::select! {
            biased;
            out = &mut fut => return out,
            _ = ticker.tick() => {
                let _ = message
                    .ack_with(async_nats::jetstream::AckKind::Progress)
                    .await;
            }
        }
    }
}

/// Parse and dispatch one ingest message, returning Ok only when the work
/// actually completed (so the caller acks) or Err to trigger redelivery.
async fn process_ingest_payload(state: &AppState, payload: &[u8]) -> Result<(), String> {
    let value: Value = serde_json::from_slice(payload).map_err(|error| error.to_string())?;
    if value.get("scrape").is_some() {
        let request = serde_json::from_value::<ScrapeRequest>(value["scrape"].clone())
            .map_err(|error| error.to_string())?;
        process_scrape_request(state, request)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    } else if value.get("url").is_some() && value.get("records").is_none() {
        let request =
            serde_json::from_value::<ScrapeRequest>(value).map_err(|error| error.to_string())?;
        process_scrape_request(state, request)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    } else if value.get("webhook").is_some() {
        let request = serde_json::from_value::<WebhookIngestRequest>(value["webhook"].clone())
            .map_err(|error| error.to_string())?;
        process_webhook(state, request)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    } else {
        let request =
            serde_json::from_value::<IngestRequest>(value).map_err(|error| error.to_string())?;
        process_ingest_request(state, request)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = dd_telemetry::init("dd-public-data-server");

    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8115").parse::<u16>()?;
    let nats = match optional_env("NATS_URL") {
        // Degrade gracefully if the broker is down at boot: the HTTP/ingest API
        // must come up even when messaging is unavailable. async-nats serves a
        // reconnecting client, so a later recovery is picked up.
        Some(url) => match async_nats::connect(&url).await {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::error!("dd-public-data-server NATS connect failed ({url}): {error}");
                None
            }
        },
        None => None,
    };
    let state = AppState {
        config: Arc::new(config_from_env()),
        metrics: Arc::new(Metrics::default()),
        nats,
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(75))
            .build()?,
        store: Arc::new(RwLock::new(PublicDataStore::default())),
    };
    tokio::spawn(run_nats_loop(state.clone()));

    let app = Router::new()
        .route("/", get(root))
        .route("/ui", get(ui_dashboard))
        .route("/ui/fragments/summary", get(ui_summary_fragment))
        .route("/ui/fragments/sources", get(ui_sources_fragment))
        .route(
            "/ui/fragments/recent-records",
            get(ui_recent_records_fragment),
        )
        .route("/ui/actions/scrape", post(ui_scrape_action))
        .route("/descriptor", get(descriptor))
        .route("/sources", get(sources))
        .route("/schema", get(schema))
        .route("/example", get(example))
        .route("/datasets", get(datasets))
        .route("/jobs", get(jobs))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/webhooks/ingest", post(webhook_ingest_http))
        .route("/ingest", post(ingest_http))
        .route("/scrape", post(scrape_http))
        .route("/grants/match", post(grant_match_http))
        .route("/analysis/trends", post(trends_http))
        .route("/analysis/correlations", post(correlations_http))
        .route("/briefs/white-paper", post(white_paper_http))
        .route("/pipeline/jobs", post(pipeline_jobs_http))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    tracing::info!("{SERVICE_NAME} listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_record(
        record_id: &str,
        dataset_id: &str,
        source: &str,
        tags: &[&str],
        metrics: &[(&str, f64)],
        grant: Option<GrantOpportunity>,
    ) -> DataRecord {
        DataRecord {
            record_id: record_id.to_string(),
            dataset_id: dataset_id.to_string(),
            source: source.to_string(),
            source_url: Some("https://www.sbir.gov/".to_string()),
            title: Some(record_id.to_string()),
            summary: Some("energy public data analytics grant research".to_string()),
            published_at: Some("2026-01-01".to_string()),
            collected_at_ms: 1,
            authors: Vec::new(),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            metrics: metrics
                .iter()
                .map(|(name, value)| (name.to_string(), *value))
                .collect(),
            grant,
            raw: None,
        }
    }

    #[test]
    fn public_url_validation_blocks_private_local_and_credential_targets() {
        for url in [
            "http://localhost/data",
            "https://127.0.0.1/data",
            "https://10.2.3.4/data",
            "https://172.16.1.2/data",
            "https://192.168.1.2/data",
            "https://[::1]/data",
            "https://[fc00::1]/data",
            "https://user@example.gov/data",
            "ftp://data.gov/file",
        ] {
            assert!(validate_public_url(url).is_err(), "{url} should be blocked");
        }
        assert!(validate_public_url("https://www.data.gov/").is_ok());
        assert!(validate_public_url("https://pubmed.ncbi.nlm.nih.gov/").is_ok());
    }

    #[test]
    fn trend_and_correlation_summaries_detect_linear_relationships() {
        let records = vec![
            test_record(
                "r1",
                "science",
                "pubmed",
                &["science"],
                &[("citations", 1.0), ("funding", 2.0)],
                None,
            ),
            test_record(
                "r2",
                "science",
                "pubmed",
                &["science"],
                &[("citations", 2.0), ("funding", 4.0)],
                None,
            ),
            test_record(
                "r3",
                "science",
                "pubmed",
                &["science"],
                &[("citations", 3.0), ("funding", 6.0)],
                None,
            ),
        ];
        let trends = trend_summaries(&records, &Some(vec!["citations".to_string()]));
        assert_eq!(trends.len(), 1);
        assert_eq!(trends[0].direction, "up");
        assert!((trends[0].slope_per_record - 1.0).abs() < 1e-9);

        let correlations = correlation_summaries(&records, &None);
        let strongest = correlations
            .iter()
            .find(|item| {
                (item.left_metric == "citations" && item.right_metric == "funding")
                    || (item.left_metric == "funding" && item.right_metric == "citations")
            })
            .expect("expected citations/funding correlation");
        assert!((strongest.pearson - 1.0).abs() < 1e-9);
        assert_eq!(strongest.strength, "very-strong");
    }

    #[test]
    fn grant_matching_respects_focus_terms_and_minimum_amount() {
        let strong_grant = GrantOpportunity {
            grant_id: Some("sbir-energy-ai".to_string()),
            title: "Energy AI public data grant".to_string(),
            agency: Some("DOE".to_string()),
            program: Some("SBIR".to_string()),
            amount: Some(250_000.0),
            due_date: Some("2026-09-15".to_string()),
            eligibility: Some("US small business research teams".to_string()),
            topics: vec![
                "energy".to_string(),
                "ai".to_string(),
                "analytics".to_string(),
            ],
            url: Some("https://www.sbir.gov/".to_string()),
        };
        let small_grant = GrantOpportunity {
            grant_id: Some("tiny".to_string()),
            title: "Tiny archive grant".to_string(),
            agency: Some("Library".to_string()),
            program: None,
            amount: Some(1_000.0),
            due_date: None,
            eligibility: None,
            topics: vec!["archives".to_string()],
            url: None,
        };
        let records = vec![
            test_record(
                "grant-1",
                "sbir",
                "sbir",
                &["energy", "ai", "grant"],
                &[("awardAmountUsd", 250_000.0)],
                Some(strong_grant),
            ),
            test_record(
                "grant-2",
                "libraries",
                "state-libraries",
                &["archives", "grant"],
                &[("awardAmountUsd", 1_000.0)],
                Some(small_grant),
            ),
        ];
        let request = GrantMatchRequest {
            request_id: Some("match".to_string()),
            applicant_profile: "small business building AI public data models".to_string(),
            focus_areas: vec!["energy".to_string(), "ai".to_string()],
            dataset_ids: None,
            min_amount: Some(50_000.0),
            limit: Some(5),
        };
        let matches = grant_matches_from_records(&records, &request);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].record_id, "grant-1");
        assert!(matches[0].score > 0.0);
        assert!(matches[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("focus-area")));
    }

    #[test]
    fn ui_helpers_escape_html_and_build_scrape_requests() {
        // maud auto-escapes every interpolated value, so the old manual
        // html_escape helper is gone; assert the same escaping via a real
        // render path instead.
        let notice = render_ui_notice("SBIR <grant> & \"quotes\"", "detail", false).into_string();
        assert!(notice.contains("SBIR &lt;grant&gt; &amp; &quot;quotes&quot;"));
        assert!(!notice.contains("<grant>"));

        let request = UiScrapeForm {
            source: Some("SBIR".to_string()),
            url: "https://www.sbir.gov/funding/".to_string(),
            dataset_id: Some("sbir-ui".to_string()),
            strategy: Some("cheerio".to_string()),
            selector: Some("main".to_string()),
            tags: Some("grants, energy, grants".to_string()),
            render_javascript: None,
            include_links: Some("on".to_string()),
            pipeline_enabled: Some("on".to_string()),
        }
        .into_scrape_request()
        .expect("ui scrape form should be accepted");

        assert_eq!(request.source, "SBIR");
        assert_eq!(request.dataset_id.as_deref(), Some("sbir-ui"));
        assert_eq!(
            request.tags,
            Some(vec!["grants".to_string(), "energy".to_string()])
        );
        assert_eq!(request.include_links, Some(true));
        assert_eq!(request.render_javascript, Some(false));
        assert!(request.pipeline.is_some());
    }

    fn test_state() -> AppState {
        AppState {
            config: Arc::new(Config {
                server_auth_secret: Some("secret".to_string()),
                webhook_secret: None,
                allow_unauthenticated: false,
                allow_unauthenticated_webhooks: false,
                scraper_base_url: "http://127.0.0.1:9000".to_string(),
                scraper_auth_secret: None,
                ingest_request_subject: PUBLIC_DATA_INGEST_REQUESTS_SUBJECT.to_string(),
                ingest_result_subject: PUBLIC_DATA_INGEST_RESULTS_SUBJECT.to_string(),
                webhook_event_subject: PUBLIC_DATA_WEBHOOK_EVENTS_SUBJECT.to_string(),
                pipeline_job_subject: PUBLIC_DATA_PIPELINE_JOBS_SUBJECT.to_string(),
                analysis_result_subject: PUBLIC_DATA_ANALYSIS_RESULTS_SUBJECT.to_string(),
                runtime_event_subject: RUNTIME_EVENTS_SUBJECT.to_string(),
                queue_group: PUBLIC_DATA_INGEST_REQUESTS_QUEUE_GROUP.to_string(),
            }),
            metrics: Arc::new(Metrics::default()),
            nats: None,
            http: reqwest::Client::new(),
            store: Arc::new(RwLock::new(PublicDataStore::default())),
        }
    }

    #[test]
    fn root_page_renders_doctype_and_actions() {
        let html = render_root().into_string();
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<title>dd-public-data-server</title>"));
        assert!(html.contains("href=\"./ui\""));
        // CSS embedded verbatim via PreEscaped.
        assert!(html.contains("background: #1f6f70"));
    }

    #[tokio::test]
    async fn ui_shell_renders_htmx_wiring_and_css() {
        let state = test_state();
        let html = render_ui_shell(&state).into_string();
        assert!(html.starts_with("<!DOCTYPE html>"));
        // HTMX script tag and fragment/action wiring preserved verbatim.
        assert!(html.contains("src=\"https://unpkg.com/htmx.org@2.0.4/dist/htmx.min.js\""));
        assert!(html.contains("hx-get=\"./ui/fragments/summary\""));
        assert!(html.contains("hx-trigger=\"load, every 15s\""));
        assert!(html.contains("hx-post=\"./ui/actions/scrape\""));
        assert!(html.contains("hx-target=\"#scrape-result\""));
        assert!(html.contains("hx-swap=\"innerHTML\""));
        assert!(html.contains("hx-disabled-elt=\"button\""));
        assert!(html.contains("hx-trigger=\"load, every 20s\""));
        // CSS embedded verbatim via PreEscaped.
        assert!(html.contains("--teal: #1f6f70"));
    }

    #[test]
    fn scrape_result_wires_refresh_button_and_escapes_dynamic_values() {
        let value = json!({
            "requestId": "req-1",
            "datasetId": "sbir-ui",
            "record": { "title": "Grant <x> & \"y\"" },
            "scraper": { "status": 200 }
        });
        let html = render_ui_scrape_result(&value).into_string();
        assert!(html.contains("hx-get=\"./ui/fragments/summary\""));
        assert!(html.contains("hx-target=\"#summary\""));
        assert!(html.contains("hx-swap=\"innerHTML\""));
        // Dynamic values are auto-escaped and interpolated.
        assert!(html.contains("Grant &lt;x&gt; &amp; &quot;y&quot;"));
        assert!(!html.contains("Grant <x>"));
        assert!(html.contains("scraper status 200."));
    }
}
