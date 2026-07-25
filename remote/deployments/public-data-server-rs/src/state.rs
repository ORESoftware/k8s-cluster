use std::{
    env,
    sync::{atomic::AtomicU64, Arc, RwLock},
};

use dd_nats_subject_defs::{
    PUBLIC_DATA_ANALYSIS_RESULTS_SUBJECT, PUBLIC_DATA_INGEST_REQUESTS_QUEUE_GROUP,
    PUBLIC_DATA_INGEST_REQUESTS_SUBJECT, PUBLIC_DATA_INGEST_RESULTS_SUBJECT,
    PUBLIC_DATA_PIPELINE_JOBS_SUBJECT, PUBLIC_DATA_WEBHOOK_EVENTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};

use crate::types::{AnalysisResult, DataRecord, PipelineJob, WebhookReceipt};

pub(crate) const SERVICE_NAME: &str = "dd-public-data-server";
pub(crate) const SCHEMA_VERSION: &str = "public_data.ingest.v1";
pub(crate) const MAX_HTTP_BODY_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_NATS_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_RECORDS_PER_REQUEST: usize = 512;
pub(crate) const MAX_RECORD_STORE: usize = 10_000;
pub(crate) const MAX_RECEIPT_STORE: usize = 2_000;
pub(crate) const MAX_ANALYSIS_STORE: usize = 1_000;
pub(crate) const MAX_PIPELINE_JOBS: usize = 2_000;
pub(crate) const MAX_TEXT_LEN: usize = 4_096;
pub(crate) const MAX_LONG_TEXT_LEN: usize = 24_000;
pub(crate) const MAX_TOKEN_LEN: usize = 160;
pub(crate) const MAX_TAGS: usize = 64;
pub(crate) const MAX_GRAPH_POINTS: usize = 256;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) metrics: Arc<Metrics>,
    pub(crate) nats: Option<async_nats::Client>,
    pub(crate) http: reqwest::Client,
    pub(crate) store: Arc<RwLock<PublicDataStore>>,
}

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) server_auth_secret: Option<String>,
    pub(crate) webhook_secret: Option<String>,
    pub(crate) allow_unauthenticated: bool,
    pub(crate) allow_unauthenticated_webhooks: bool,
    pub(crate) scraper_base_url: String,
    pub(crate) scraper_auth_secret: Option<String>,
    pub(crate) ingest_request_subject: String,
    pub(crate) ingest_result_subject: String,
    pub(crate) webhook_event_subject: String,
    pub(crate) pipeline_job_subject: String,
    pub(crate) analysis_result_subject: String,
    pub(crate) runtime_event_subject: String,
    pub(crate) queue_group: String,
}

#[derive(Default)]
pub(crate) struct Metrics {
    pub(crate) http_requests_total: AtomicU64,
    pub(crate) webhook_receipts_total: AtomicU64,
    pub(crate) records_ingested_total: AtomicU64,
    pub(crate) scrape_requests_total: AtomicU64,
    pub(crate) grant_match_requests_total: AtomicU64,
    pub(crate) trend_requests_total: AtomicU64,
    pub(crate) correlation_requests_total: AtomicU64,
    pub(crate) white_paper_briefs_total: AtomicU64,
    pub(crate) pipeline_jobs_total: AtomicU64,
    pub(crate) auth_failures_total: AtomicU64,
    pub(crate) errors_total: AtomicU64,
    pub(crate) nats_messages_total: AtomicU64,
    pub(crate) nats_published_total: AtomicU64,
}

#[derive(Default)]
pub(crate) struct PublicDataStore {
    pub(crate) records: Vec<DataRecord>,
    pub(crate) webhook_receipts: Vec<WebhookReceipt>,
    pub(crate) analyses: Vec<AnalysisResult>,
    pub(crate) pipeline_jobs: Vec<PipelineJob>,
}

pub(crate) fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

pub(crate) fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn env_bool(key: &str, fallback: bool) -> bool {
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

pub(crate) fn config_from_env() -> Config {
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
