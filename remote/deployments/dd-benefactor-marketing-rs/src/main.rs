use std::{
    collections::hash_map::DefaultHasher,
    env,
    hash::{Hash, Hasher},
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, FixedOffset, Utc};
use dd_pg_defs_sea_orm::{
    benefactor_marketing_attribution_events as attribution_events,
    benefactor_marketing_automation_events as automation_events,
    benefactor_marketing_automation_workflows as automation_workflows,
    benefactor_marketing_campaign_channels as campaign_channels,
    benefactor_marketing_campaign_experiments as campaign_experiments,
    benefactor_marketing_campaigns as campaigns,
    benefactor_marketing_client_approvals as client_approvals,
    benefactor_marketing_clients as clients, benefactor_marketing_contacts as contacts,
    benefactor_marketing_content_assets as content_assets,
    benefactor_marketing_contracts as contracts,
    benefactor_marketing_conversion_events as conversion_events,
    benefactor_marketing_enrichment_jobs as enrichment_jobs,
    benefactor_marketing_integration_sync_runs as integration_sync_runs,
    benefactor_marketing_integrations as integrations, benefactor_marketing_invoices as invoices,
    benefactor_marketing_leads as leads, benefactor_marketing_meetings as meetings,
    benefactor_marketing_opportunities as opportunities,
    benefactor_marketing_outreach_enrollments as outreach_enrollments,
    benefactor_marketing_outreach_sequences as outreach_sequences,
    benefactor_marketing_outreach_steps as outreach_steps,
    benefactor_marketing_outreach_touchpoints as outreach_touchpoints,
    benefactor_marketing_project_tasks as project_tasks,
    benefactor_marketing_prospect_research_briefs as research_briefs,
    benefactor_marketing_reports as reports,
    benefactor_marketing_service_packages as service_packages,
    benefactor_marketing_team_allocations as team_allocations,
    benefactor_marketing_tickets as tickets,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection,
    DbErr, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set, Statement,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;
use tokio::sync::Mutex;
use tower_http::{limit::RequestBodyLimitLayer, trace::TraceLayer};
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use uuid::Uuid;

const SERVICE_NAME: &str = "dd-benefactor-marketing-rs";
const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
const DEFAULT_PORT: u16 = 8134;
const DEFAULT_LIMIT: u64 = 50;
const MAX_LIMIT: u64 = 200;
const DEFAULT_CACHE_TTL_SECONDS: u64 = 120;
const DEFAULT_RATE_LIMIT_PER_MINUTE: u64 = 600;
const DEFAULT_JOB_STREAM: &str = "benefactor:marketing:jobs";

type AppResult<T> = Result<T, AppError>;

#[derive(Clone)]
struct AppState {
    cfg: Arc<Config>,
    db: DatabaseConnection,
    redis: Option<redis::Client>,
    redis_connection: Arc<Mutex<Option<redis::aio::MultiplexedConnection>>>,
    metrics: Arc<Metrics>,
    started_at: Instant,
}

#[derive(Clone, Debug)]
struct Config {
    host: String,
    port: u16,
    database_url: String,
    api_auth_bearer: Option<String>,
    allow_unauthenticated: bool,
    scraper_base_url: Option<String>,
    redis_url: Option<String>,
    redis_required_for_ready: bool,
    cache_ttl_seconds: u64,
    rate_limit_per_minute: u64,
    job_stream: String,
    log_json: bool,
}

#[derive(Default)]
struct Metrics {
    mutations_total: AtomicU64,
    enrichment_jobs_total: AtomicU64,
    lead_imports_total: AtomicU64,
    auth_failures_total: AtomicU64,
    db_errors_total: AtomicU64,
    redis_errors_total: AtomicU64,
    cache_hits_total: AtomicU64,
    cache_misses_total: AtomicU64,
    cache_invalidations_total: AtomicU64,
    rate_limit_rejections_total: AtomicU64,
    redis_jobs_published_total: AtomicU64,
    integration_sync_runs_total: AtomicU64,
    outreach_touchpoints_total: AtomicU64,
    research_briefs_total: AtomicU64,
    conversion_events_total: AtomicU64,
}

#[derive(Debug, Error)]
enum AppError {
    #[error("authentication required")]
    Unauthorized,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("rate limit exceeded")]
    RateLimited,
    #[error("database operation failed")]
    Database(#[from] DbErr),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match self {
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            AppError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        if matches!(self, AppError::Database(_)) {
            warn!(error = %self, "request failed");
        }
        let body = json!({
            "error": status.canonical_reason().unwrap_or("error"),
            "message": self.to_string(),
        });
        (status, Json(body)).into_response()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateClientRequest {
    name: String,
    slug: Option<String>,
    status: Option<String>,
    industry: Option<String>,
    website_url: Option<String>,
    billing_email: Option<String>,
    owner_user_id: Option<Uuid>,
    service_package: Option<String>,
    onboarding_stage: Option<String>,
    portal_enabled: Option<bool>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateContactRequest {
    status: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    email: Option<String>,
    phone: Option<String>,
    job_title: Option<String>,
    lifecycle_role: Option<String>,
    consent_status: Option<String>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateServicePackageRequest {
    status: Option<String>,
    code: String,
    name: String,
    channel_mix: Option<Value>,
    deliverables: Option<Value>,
    monthly_budget_cents: Option<i32>,
    retainer_cents: Option<i32>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateContractRequest {
    package_id: Option<Uuid>,
    status: Option<String>,
    contract_number: Option<String>,
    starts_on: Option<String>,
    ends_on: Option<String>,
    billing_terms: Option<Value>,
    total_value_cents: Option<i32>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateInvoiceRequest {
    contract_id: Option<Uuid>,
    status: Option<String>,
    invoice_number: Option<String>,
    due_on: Option<String>,
    amount_cents: Option<i32>,
    line_items: Option<Value>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateIntegrationRequest {
    platform: String,
    status: Option<String>,
    auth_kind: Option<String>,
    external_account_id: Option<String>,
    sync_cursor: Option<String>,
    config: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateIntegrationSyncRunRequest {
    client_id: Option<Uuid>,
    sync_kind: Option<String>,
    direction: Option<String>,
    status: Option<String>,
    records_seen: Option<i32>,
    records_changed: Option<i32>,
    cursor_before: Option<String>,
    cursor_after: Option<String>,
    payload: Option<Value>,
    error_summary: Option<String>,
    started_at: Option<DateTime<FixedOffset>>,
    completed_at: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LeadImportRequest {
    client_id: Uuid,
    source_integration_id: Option<Uuid>,
    leads: Vec<LeadDraft>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LeadDraft {
    company_name: String,
    domain: Option<String>,
    contact_name: Option<String>,
    contact_email: Option<String>,
    contact_title: Option<String>,
    country_code: Option<String>,
    lead_score: Option<i32>,
    icp_fit_score: Option<i32>,
    verification_status: Option<String>,
    company_profile: Option<Value>,
    signals: Option<Value>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnrichmentJobRequest {
    job_kind: String,
    external_job_id: Option<String>,
    scraper_handoff_url: Option<String>,
    input: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScoreLeadRequest {
    lead_score: Option<i32>,
    icp_fit_score: Option<i32>,
    status: Option<String>,
    verification_status: Option<String>,
    enrichment_status: Option<String>,
    company_profile: Option<Value>,
    signals: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateCampaignRequest {
    client_id: Uuid,
    status: Option<String>,
    campaign_kind: Option<String>,
    name: String,
    objective: Option<String>,
    budget_cents: Option<i32>,
    starts_on: Option<String>,
    ends_on: Option<String>,
    target_segments: Option<Value>,
    kpis: Option<Value>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateCampaignChannelRequest {
    channel: String,
    status: Option<String>,
    external_campaign_id: Option<String>,
    strategy: Option<Value>,
    schedule: Option<Value>,
    metrics_snapshot: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateCampaignExperimentRequest {
    status: Option<String>,
    experiment_kind: String,
    hypothesis: Option<String>,
    variants: Option<Value>,
    winning_variant: Option<String>,
    result_summary: Option<Value>,
    started_at: Option<DateTime<FixedOffset>>,
    ended_at: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateOutreachSequenceRequest {
    client_id: Uuid,
    campaign_id: Option<Uuid>,
    status: Option<String>,
    channel: Option<String>,
    name: String,
    audience_filter: Option<Value>,
    cadence: Option<Value>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateOutreachStepRequest {
    status: Option<String>,
    step_order: i32,
    channel: String,
    delay_minutes: Option<i32>,
    subject: Option<String>,
    body_template: Option<String>,
    personalization_hints: Option<Value>,
    experiment_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateOutreachEnrollmentRequest {
    client_id: Uuid,
    sequence_id: Uuid,
    lead_id: Option<Uuid>,
    contact_id: Option<Uuid>,
    status: Option<String>,
    current_step_order: Option<i32>,
    enrollment_context: Option<Value>,
    last_touch_at: Option<DateTime<FixedOffset>>,
    next_touch_at: Option<DateTime<FixedOffset>>,
    outcome: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordOutreachTouchpointRequest {
    client_id: Uuid,
    sequence_id: Option<Uuid>,
    enrollment_id: Option<Uuid>,
    campaign_id: Option<Uuid>,
    lead_id: Option<Uuid>,
    contact_id: Option<Uuid>,
    channel: String,
    direction: Option<String>,
    status: Option<String>,
    subject: Option<String>,
    body_excerpt: Option<String>,
    external_message_id: Option<String>,
    occurred_at: Option<DateTime<FixedOffset>>,
    payload: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAutomationWorkflowRequest {
    client_id: Uuid,
    status: Option<String>,
    name: String,
    trigger_kind: String,
    trigger_config: Option<Value>,
    action_graph: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationEventRequest {
    client_id: Uuid,
    workflow_id: Option<Uuid>,
    lead_id: Option<Uuid>,
    event_kind: String,
    status: Option<String>,
    payload: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportSnapshotRequest {
    client_id: Uuid,
    campaign_id: Option<Uuid>,
    report_kind: Option<String>,
    status: Option<String>,
    period_start: Option<String>,
    period_end: Option<String>,
    metrics: Option<Value>,
    narrative: Option<String>,
    delivery_targets: Option<Value>,
    generated_at: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttributionEventRequest {
    client_id: Uuid,
    campaign_id: Option<Uuid>,
    lead_id: Option<Uuid>,
    event_type: String,
    source_platform: Option<String>,
    source_event_id: Option<String>,
    occurred_at: Option<DateTime<FixedOffset>>,
    value_cents: Option<i32>,
    payload: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateOpportunityRequest {
    client_id: Uuid,
    lead_id: Option<Uuid>,
    status: Option<String>,
    stage: Option<String>,
    name: String,
    amount_cents: Option<i32>,
    probability_micros: Option<i32>,
    expected_close_on: Option<String>,
    owner_user_id: Option<Uuid>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateContentAssetRequest {
    client_id: Uuid,
    campaign_id: Option<Uuid>,
    status: Option<String>,
    asset_kind: String,
    title: String,
    channel: Option<String>,
    body: Option<String>,
    asset_uri: Option<String>,
    seo_keywords: Option<Value>,
    approval_status: Option<String>,
    publish_at: Option<DateTime<FixedOffset>>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateResearchBriefRequest {
    client_id: Uuid,
    lead_id: Option<Uuid>,
    status: Option<String>,
    research_kind: Option<String>,
    source: Option<String>,
    summary: Option<String>,
    findings: Option<Value>,
    recommended_actions: Option<Value>,
    confidence_micros: Option<i32>,
    model_name: Option<String>,
    generated_at: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordConversionEventRequest {
    client_id: Uuid,
    campaign_id: Option<Uuid>,
    lead_id: Option<Uuid>,
    content_asset_id: Option<Uuid>,
    event_type: String,
    source_platform: Option<String>,
    source_event_id: Option<String>,
    session_id: Option<String>,
    visitor_key: Option<String>,
    occurred_at: Option<DateTime<FixedOffset>>,
    value_cents: Option<i32>,
    utm: Option<Value>,
    payload: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateProjectTaskRequest {
    client_id: Uuid,
    campaign_id: Option<Uuid>,
    content_asset_id: Option<Uuid>,
    status: Option<String>,
    priority: Option<String>,
    title: String,
    description: Option<String>,
    assigned_to: Option<Uuid>,
    due_on: Option<String>,
    sla_due_at: Option<DateTime<FixedOffset>>,
    time_spent_minutes: Option<i32>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateApprovalRequest {
    client_id: Uuid,
    campaign_id: Option<Uuid>,
    content_asset_id: Option<Uuid>,
    requested_by: Option<Uuid>,
    approval_kind: String,
    title: String,
    request_payload: Option<Value>,
    due_at: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DecideApprovalRequest {
    status: String,
    response_note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTicketRequest {
    client_id: Uuid,
    status: Option<String>,
    priority: Option<String>,
    subject: String,
    description: Option<String>,
    source: Option<String>,
    assigned_to: Option<Uuid>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateMeetingRequest {
    client_id: Uuid,
    lead_id: Option<Uuid>,
    opportunity_id: Option<Uuid>,
    status: Option<String>,
    meeting_kind: String,
    title: String,
    scheduled_at: DateTime<FixedOffset>,
    duration_minutes: Option<i32>,
    notes: Option<String>,
    recording_uri: Option<String>,
    transcript_summary: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTeamAllocationRequest {
    campaign_id: Option<Uuid>,
    user_id: Uuid,
    role: String,
    allocation_percent: Option<i32>,
    starts_on: Option<String>,
    ends_on: Option<String>,
    billable: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LeadImportResponse {
    imported: usize,
    leads: Vec<leads::Model>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::from_env()?;
    init_tracing(cfg.log_json);
    let addr: SocketAddr = format!("{}:{}", cfg.host, cfg.port).parse()?;
    let db = Database::connect(&cfg.database_url).await?;
    let redis = cfg
        .redis_url
        .as_deref()
        .map(redis::Client::open)
        .transpose()?;
    if redis.is_some() {
        info!("redis integration enabled for benefactor marketing runtime");
    }
    let state = AppState {
        cfg: Arc::new(cfg),
        db,
        redis,
        redis_connection: Arc::new(Mutex::new(None)),
        metrics: Arc::new(Metrics::default()),
        started_at: Instant::now(),
    };

    let app = build_router(state.clone());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "benefactor marketing backend listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    info!("benefactor marketing backend shut down cleanly");
    Ok(())
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(descriptor))
        .route("/descriptor", get(descriptor))
        .route("/capabilities", get(capabilities))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/runtime/redis", get(redis_status))
        .route(
            "/service-packages",
            get(list_service_packages).post(create_service_package),
        )
        .route("/clients", get(list_clients).post(create_client))
        .route("/clients/{client_id}/overview", get(client_dashboard))
        .route("/clients/{client_id}/dashboard", get(client_dashboard))
        .route(
            "/clients/{client_id}/lead-intelligence",
            get(client_lead_intelligence),
        )
        .route(
            "/clients/{client_id}/revenue-attribution",
            get(client_revenue_attribution),
        )
        .route(
            "/clients/{client_id}/operations",
            get(client_operations_summary),
        )
        .route(
            "/clients/{client_id}/team-allocations",
            get(list_client_team_allocations).post(create_team_allocation),
        )
        .route("/clients/{client_id}/sync-runs", get(list_client_sync_runs))
        .route(
            "/clients/{client_id}/outreach",
            get(client_outreach_summary),
        )
        .route(
            "/clients/{client_id}/outreach/sequences",
            get(list_client_outreach_sequences),
        )
        .route(
            "/clients/{client_id}/research/briefs",
            get(list_client_research_briefs),
        )
        .route(
            "/clients/{client_id}/conversion-events",
            get(list_client_conversion_events),
        )
        .route("/clients/{client_id}/contacts", post(create_contact))
        .route("/clients/{client_id}/contracts", post(create_contract))
        .route("/clients/{client_id}/invoices", post(create_invoice))
        .route(
            "/clients/{client_id}/integrations",
            post(create_integration),
        )
        .route("/clients/{client_id}/leads", get(list_client_leads))
        .route("/clients/{client_id}/campaigns", get(list_client_campaigns))
        .route(
            "/integrations/{integration_id}/sync-runs",
            post(create_integration_sync_run),
        )
        .route("/leads/import", post(import_leads))
        .route(
            "/leads/{lead_id}/enrichment-jobs",
            post(queue_enrichment_job),
        )
        .route("/leads/{lead_id}/score", post(score_lead))
        .route("/campaigns", post(create_campaign))
        .route(
            "/campaigns/{campaign_id}/channels",
            post(create_campaign_channel),
        )
        .route(
            "/campaigns/{campaign_id}/experiments",
            post(create_campaign_experiment),
        )
        .route("/outreach/sequences", post(create_outreach_sequence))
        .route(
            "/outreach/sequences/{sequence_id}/steps",
            post(create_outreach_step),
        )
        .route("/outreach/enrollments", post(create_outreach_enrollment))
        .route("/outreach/touchpoints", post(record_outreach_touchpoint))
        .route("/automation/workflows", post(create_automation_workflow))
        .route("/automation/events", post(record_automation_event))
        .route("/reports/snapshots", post(create_report_snapshot))
        .route("/attribution/events", post(record_attribution_event))
        .route("/opportunities", post(create_opportunity))
        .route("/content/assets", post(create_content_asset))
        .route("/research/briefs", post(create_research_brief))
        .route("/conversion/events", post(record_conversion_event))
        .route("/projects/tasks", post(create_project_task))
        .route("/approvals", post(create_approval))
        .route("/approvals/{approval_id}/decision", patch(decide_approval))
        .route("/tickets", post(create_ticket))
        .route("/meetings", post(create_meeting))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .layer(RequestBodyLimitLayer::new(MAX_HTTP_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

impl Config {
    fn from_env() -> anyhow::Result<Self> {
        let host = env::var("BENEFACTOR_MARKETING_HOST")
            .or_else(|_| env::var("HOST"))
            .unwrap_or_else(|_| "0.0.0.0".to_string());
        let port = env::var("BENEFACTOR_MARKETING_PORT")
            .or_else(|_| env::var("PORT"))
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);
        let database_url = env::var("BENEFACTOR_MARKETING_DATABASE_URL")
            .or_else(|_| env::var("DATABASE_URL"))
            .map_err(|_| {
                anyhow::anyhow!("BENEFACTOR_MARKETING_DATABASE_URL or DATABASE_URL must be set")
            })?;
        let api_auth_bearer = env::var("BENEFACTOR_MARKETING_API_AUTH_BEARER")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let allow_unauthenticated = env_bool("BENEFACTOR_MARKETING_ALLOW_UNAUTHENTICATED", false);
        let scraper_base_url = env::var("BENEFACTOR_MARKETING_SCRAPER_BASE_URL")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let redis_url = env::var("BENEFACTOR_MARKETING_REDIS_URL")
            .or_else(|_| env::var("REDIS_URL"))
            .ok()
            .filter(|value| !value.trim().is_empty());
        let redis_required_for_ready =
            env_bool("BENEFACTOR_MARKETING_REDIS_REQUIRED_FOR_READY", false);
        let cache_ttl_seconds = env_u64(
            "BENEFACTOR_MARKETING_CACHE_TTL_SECONDS",
            DEFAULT_CACHE_TTL_SECONDS,
        );
        let rate_limit_per_minute = env_u64(
            "BENEFACTOR_MARKETING_RATE_LIMIT_PER_MINUTE",
            DEFAULT_RATE_LIMIT_PER_MINUTE,
        );
        let job_stream = env::var("BENEFACTOR_MARKETING_JOB_STREAM")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_JOB_STREAM.to_string());
        let log_json = env::var("BENEFACTOR_MARKETING_LOG_FORMAT")
            .map(|value| value.eq_ignore_ascii_case("json"))
            .unwrap_or(false);

        Ok(Self {
            host,
            port,
            database_url,
            api_auth_bearer,
            allow_unauthenticated,
            scraper_base_url,
            redis_url,
            redis_required_for_ready,
            cache_ttl_seconds,
            rate_limit_per_minute,
            job_stream,
            log_json,
        })
    }
}

fn init_tracing(json_logs: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("dd_benefactor_marketing_rs=info,sea_orm=warn,sqlx=warn,tower_http=info")
    });
    let fmt = tracing_subscriber::fmt::layer();
    if json_logs {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt.json())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt.compact())
            .init();
    }
}

async fn descriptor() -> Json<Value> {
    Json(json!({
        "service": SERVICE_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "docs": "/docs/api",
        "health": "/healthz",
        "ready": "/readyz",
        "capabilities": "/capabilities"
    }))
}

async fn capabilities() -> Json<Value> {
    Json(json!({
        "service": SERVICE_NAME,
        "modules": [
            "clientManagement",
            "leadGeneration",
            "campaignManagement",
            "crmSync",
            "outreachSequencing",
            "marketingAutomation",
            "analyticsReporting",
            "conversionTracking",
            "salesPipeline",
            "prospectResearch",
            "contentOperations",
            "projectManagement",
            "clientCommunication",
            "agencyOperations"
        ],
        "channels": ["socialMedia", "seoAeo", "email", "linkedin", "sms", "paidAds", "content"],
        "integrations": [
            "salesforce",
            "hubspot",
            "apollo",
            "zoominfo",
            "googleAnalytics",
            "googleAds",
            "linkedinAds",
            "metaAds",
            "mailchimp",
            "sendgrid",
            "externalScraper"
        ],
        "storage": {
            "database": "postgres",
            "orm": "sea-orm via remote/libs/pg-defs/generated/rust/sea-orm",
            "tablePrefix": "benefactor_marketing_",
            "cache": "redis dashboard cache",
            "rateLimits": "redis per-actor counters",
            "jobStream": DEFAULT_JOB_STREAM
        }
    }))
}

async fn healthz() -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "timeUnix": unix_seconds()
    }))
}

async fn readyz(State(state): State<AppState>) -> Response {
    let database_ready = state
        .db
        .execute(Statement::from_string(
            DatabaseBackend::Postgres,
            "select 1".to_string(),
        ))
        .await
        .is_ok();
    let redis_configured = state.redis.is_some();
    let redis_ready = if redis_configured {
        redis_ping(&state).await
    } else {
        false
    };
    let ready = database_ready
        && (!state.cfg.redis_required_for_ready || (redis_configured && redis_ready));
    let status = if ready {
        StatusCode::OK
    } else {
        if !database_ready {
            state
                .metrics
                .db_errors_total
                .fetch_add(1, Ordering::Relaxed);
        }
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "ok": ready,
            "service": SERVICE_NAME,
            "database": if database_ready { "ready" } else { "unavailable" },
            "redis": {
                "configured": redis_configured,
                "requiredForReady": state.cfg.redis_required_for_ready,
                "status": if redis_configured {
                    if redis_ready { "ready" } else { "unavailable" }
                } else {
                    "disabled"
                }
            }
        })),
    )
        .into_response()
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let uptime = state.started_at.elapsed().as_secs();
    let body = format!(
        "# HELP benefactor_marketing_uptime_seconds Process uptime in seconds.\n\
# TYPE benefactor_marketing_uptime_seconds gauge\n\
benefactor_marketing_uptime_seconds {}\n\
# HELP benefactor_marketing_mutations_total Domain mutations accepted by the backend.\n\
# TYPE benefactor_marketing_mutations_total counter\n\
benefactor_marketing_mutations_total {}\n\
# HELP benefactor_marketing_enrichment_jobs_total Lead enrichment or scraper handoff jobs queued.\n\
# TYPE benefactor_marketing_enrichment_jobs_total counter\n\
benefactor_marketing_enrichment_jobs_total {}\n\
# HELP benefactor_marketing_lead_imports_total Lead import requests accepted.\n\
# TYPE benefactor_marketing_lead_imports_total counter\n\
benefactor_marketing_lead_imports_total {}\n\
# HELP benefactor_marketing_auth_failures_total Authentication failures.\n\
# TYPE benefactor_marketing_auth_failures_total counter\n\
benefactor_marketing_auth_failures_total {}\n\
	# HELP benefactor_marketing_db_errors_total Database readiness or query failures.\n\
	# TYPE benefactor_marketing_db_errors_total counter\n\
	benefactor_marketing_db_errors_total {}\n\
	# HELP benefactor_marketing_redis_errors_total Redis readiness, cache, rate-limit, or stream failures.\n\
	# TYPE benefactor_marketing_redis_errors_total counter\n\
	benefactor_marketing_redis_errors_total {}\n\
	# HELP benefactor_marketing_cache_hits_total Redis dashboard cache hits.\n\
	# TYPE benefactor_marketing_cache_hits_total counter\n\
	benefactor_marketing_cache_hits_total {}\n\
	# HELP benefactor_marketing_cache_misses_total Redis dashboard cache misses.\n\
	# TYPE benefactor_marketing_cache_misses_total counter\n\
	benefactor_marketing_cache_misses_total {}\n\
	# HELP benefactor_marketing_cache_invalidations_total Redis cache keys invalidated after mutations.\n\
	# TYPE benefactor_marketing_cache_invalidations_total counter\n\
	benefactor_marketing_cache_invalidations_total {}\n\
	# HELP benefactor_marketing_rate_limit_rejections_total Write requests rejected by Redis-backed rate limits.\n\
	# TYPE benefactor_marketing_rate_limit_rejections_total counter\n\
	benefactor_marketing_rate_limit_rejections_total {}\n\
		# HELP benefactor_marketing_redis_jobs_published_total Marketing job handoff events published to Redis streams.\n\
		# TYPE benefactor_marketing_redis_jobs_published_total counter\n\
		benefactor_marketing_redis_jobs_published_total {}\n\
		# HELP benefactor_marketing_integration_sync_runs_total CRM or analytics sync runs recorded.\n\
		# TYPE benefactor_marketing_integration_sync_runs_total counter\n\
		benefactor_marketing_integration_sync_runs_total {}\n\
		# HELP benefactor_marketing_outreach_touchpoints_total Outreach touchpoints recorded.\n\
		# TYPE benefactor_marketing_outreach_touchpoints_total counter\n\
		benefactor_marketing_outreach_touchpoints_total {}\n\
		# HELP benefactor_marketing_research_briefs_total Prospect research briefs created.\n\
		# TYPE benefactor_marketing_research_briefs_total counter\n\
		benefactor_marketing_research_briefs_total {}\n\
		# HELP benefactor_marketing_conversion_events_total Conversion events recorded.\n\
		# TYPE benefactor_marketing_conversion_events_total counter\n\
		benefactor_marketing_conversion_events_total {}\n",
        uptime,
        state.metrics.mutations_total.load(Ordering::Relaxed),
        state.metrics.enrichment_jobs_total.load(Ordering::Relaxed),
        state.metrics.lead_imports_total.load(Ordering::Relaxed),
        state.metrics.auth_failures_total.load(Ordering::Relaxed),
        state.metrics.db_errors_total.load(Ordering::Relaxed),
        state.metrics.redis_errors_total.load(Ordering::Relaxed),
        state.metrics.cache_hits_total.load(Ordering::Relaxed),
        state.metrics.cache_misses_total.load(Ordering::Relaxed),
        state
            .metrics
            .cache_invalidations_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .rate_limit_rejections_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .redis_jobs_published_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .integration_sync_runs_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .outreach_touchpoints_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .research_briefs_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .conversion_events_total
            .load(Ordering::Relaxed)
    );
    ([("content-type", "text/plain; version=0.0.4")], body)
}

async fn redis_status(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    let configured = state.redis.is_some();
    let ready = if configured {
        redis_ping(&state).await
    } else {
        false
    };
    Ok(Json(json!({
        "configured": configured,
        "ready": ready,
        "requiredForReady": state.cfg.redis_required_for_ready,
        "cacheTtlSeconds": state.cfg.cache_ttl_seconds,
        "rateLimitPerMinute": state.cfg.rate_limit_per_minute,
        "jobStream": state.cfg.job_stream,
        "keyPrefix": "benefactor:marketing"
    })))
}

async fn list_service_packages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    let rows = service_packages::Entity::find()
        .order_by_asc(service_packages::Column::Code)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "servicePackages": rows })))
}

async fn create_service_package(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateServicePackageRequest>,
) -> AppResult<(StatusCode, Json<service_packages::Model>)> {
    require_write_access(&state, &headers, "service-packages.create").await?;
    let model = service_packages::ActiveModel {
        status: Set(req.status.unwrap_or_else(|| "active".to_string())),
        code: Set(req.code),
        name: Set(req.name),
        channel_mix: Set(array_or_default(req.channel_mix, "channelMix")?),
        deliverables: Set(array_or_default(req.deliverables, "deliverables")?),
        monthly_budget_cents: Set(req.monthly_budget_cents.unwrap_or(0)),
        retainer_cents: Set(req.retainer_cents.unwrap_or(0)),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_mutation(&state);
    Ok((StatusCode::CREATED, Json(model)))
}

async fn list_clients(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    let rows = clients::Entity::find()
        .order_by_desc(clients::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "clients": rows })))
}

async fn create_client(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateClientRequest>,
) -> AppResult<(StatusCode, Json<clients::Model>)> {
    require_write_access(&state, &headers, "clients.create").await?;
    let slug = req.slug.unwrap_or_else(|| slugify(&req.name));
    let model = clients::ActiveModel {
        status: Set(req.status.unwrap_or_else(|| "onboarding".to_string())),
        name: Set(req.name),
        slug: Set(slug),
        industry: Set(req.industry),
        website_url: Set(req.website_url),
        billing_email: Set(req.billing_email),
        owner_user_id: Set(req.owner_user_id),
        service_package: Set(req.service_package),
        onboarding_stage: Set(req.onboarding_stage.unwrap_or_else(|| "intake".to_string())),
        portal_enabled: Set(req.portal_enabled.unwrap_or(true)),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, model.id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn client_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    let cache_key = client_dashboard_cache_key(client_id);
    if let Some(cached) = cache_get_json(&state, &cache_key).await {
        return Ok(Json(cached));
    }
    let client = clients::Entity::find_by_id(client_id)
        .one(&state.db)
        .await?
        .ok_or(AppError::NotFound("client"))?;
    let lead_count = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let campaign_count = campaigns::Entity::find()
        .filter(campaigns::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let opportunity_count = opportunities::Entity::find()
        .filter(opportunities::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let open_ticket_count = tickets_count(&state.db, client_id).await?;
    let pending_approval_count = client_approvals::Entity::find()
        .filter(client_approvals::Column::ClientId.eq(client_id))
        .filter(client_approvals::Column::Status.eq("pending"))
        .count(&state.db)
        .await?;
    let outreach_sequence_count = outreach_sequences::Entity::find()
        .filter(outreach_sequences::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let research_brief_count = research_briefs::Entity::find()
        .filter(research_briefs::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let conversion_event_count = conversion_events::Entity::find()
        .filter(conversion_events::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let recent_campaigns = campaigns::Entity::find()
        .filter(campaigns::Column::ClientId.eq(client_id))
        .order_by_desc(campaigns::Column::UpdatedAt)
        .limit(8)
        .all(&state.db)
        .await?;
    let recent_reports = reports::Entity::find()
        .filter(reports::Column::ClientId.eq(client_id))
        .order_by_desc(reports::Column::UpdatedAt)
        .limit(5)
        .all(&state.db)
        .await?;
    let open_tasks = project_tasks::Entity::find()
        .filter(project_tasks::Column::ClientId.eq(client_id))
        .filter(project_tasks::Column::Status.is_in(["todo", "in_progress", "blocked"]))
        .order_by_desc(project_tasks::Column::UpdatedAt)
        .limit(10)
        .all(&state.db)
        .await?;
    let recent_conversions = conversion_events::Entity::find()
        .filter(conversion_events::Column::ClientId.eq(client_id))
        .order_by_desc(conversion_events::Column::OccurredAt)
        .limit(8)
        .all(&state.db)
        .await?;
    let payload = json!({
        "client": client,
        "counts": {
            "leads": lead_count,
            "campaigns": campaign_count,
            "opportunities": opportunity_count,
            "openTickets": open_ticket_count,
            "pendingApprovals": pending_approval_count,
            "outreachSequences": outreach_sequence_count,
            "researchBriefs": research_brief_count,
            "conversionEvents": conversion_event_count
        },
        "recent": {
            "campaigns": recent_campaigns,
            "reports": recent_reports,
            "openTasks": open_tasks,
            "conversions": recent_conversions
        }
    });
    cache_set_json(&state, &cache_key, &payload).await;
    Ok(Json(payload))
}

async fn client_lead_intelligence(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let total_leads = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let enrichment_pending = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .filter(leads::Column::EnrichmentStatus.eq("pending"))
        .count(&state.db)
        .await?;
    let enrichment_running = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .filter(leads::Column::EnrichmentStatus.eq("running"))
        .count(&state.db)
        .await?;
    let verified_contacts = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .filter(leads::Column::VerificationStatus.eq("verified"))
        .count(&state.db)
        .await?;
    let high_fit_leads = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .filter(leads::Column::IcpFitScore.gte(80))
        .count(&state.db)
        .await?;
    let top_leads = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .order_by_desc(leads::Column::LeadScore)
        .order_by_desc(leads::Column::IcpFitScore)
        .order_by_desc(leads::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({
        "clientId": client_id,
        "counts": {
            "total": total_leads,
            "enrichmentPending": enrichment_pending,
            "enrichmentRunning": enrichment_running,
            "verifiedContacts": verified_contacts,
            "highFit": high_fit_leads
        },
        "topLeads": top_leads
    })))
}

async fn client_revenue_attribution(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let attribution_event_count = attribution_events::Entity::find()
        .filter(attribution_events::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let recent_attribution_events = attribution_events::Entity::find()
        .filter(attribution_events::Column::ClientId.eq(client_id))
        .order_by_desc(attribution_events::Column::OccurredAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    let recent_value_cents: i64 = recent_attribution_events
        .iter()
        .map(|event| i64::from(event.value_cents))
        .sum();
    let conversion_event_count = conversion_events::Entity::find()
        .filter(conversion_events::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let recent_conversion_events = conversion_events::Entity::find()
        .filter(conversion_events::Column::ClientId.eq(client_id))
        .order_by_desc(conversion_events::Column::OccurredAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    let recent_conversion_value_cents: i64 = recent_conversion_events
        .iter()
        .map(|event| i64::from(event.value_cents))
        .sum();
    let open_opportunities = opportunities::Entity::find()
        .filter(opportunities::Column::ClientId.eq(client_id))
        .filter(opportunities::Column::Status.eq("open"))
        .order_by_desc(opportunities::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    let forecast_cents: i64 = open_opportunities
        .iter()
        .map(|opportunity| {
            i64::from(opportunity.amount_cents) * i64::from(opportunity.probability_micros)
                / 1_000_000
        })
        .sum();
    Ok(Json(json!({
        "clientId": client_id,
        "attribution": {
            "eventCount": attribution_event_count,
            "recentValueCents": recent_value_cents,
            "recentEvents": recent_attribution_events
        },
        "conversions": {
            "eventCount": conversion_event_count,
            "recentValueCents": recent_conversion_value_cents,
            "recentEvents": recent_conversion_events
        },
        "pipeline": {
            "openOpportunityCount": open_opportunities.len(),
            "forecastCents": forecast_cents,
            "openOpportunities": open_opportunities
        }
    })))
}

async fn client_operations_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let now = now_fixed();
    let open_task_count = project_tasks::Entity::find()
        .filter(project_tasks::Column::ClientId.eq(client_id))
        .filter(project_tasks::Column::Status.is_in(["todo", "in_progress", "blocked"]))
        .count(&state.db)
        .await?;
    let blocked_task_count = project_tasks::Entity::find()
        .filter(project_tasks::Column::ClientId.eq(client_id))
        .filter(project_tasks::Column::Status.eq("blocked"))
        .count(&state.db)
        .await?;
    let sla_risk_task_count = project_tasks::Entity::find()
        .filter(project_tasks::Column::ClientId.eq(client_id))
        .filter(project_tasks::Column::Status.is_in(["todo", "in_progress", "blocked"]))
        .filter(project_tasks::Column::SlaDueAt.lt(now))
        .count(&state.db)
        .await?;
    let pending_approval_count = client_approvals::Entity::find()
        .filter(client_approvals::Column::ClientId.eq(client_id))
        .filter(client_approvals::Column::Status.eq("pending"))
        .count(&state.db)
        .await?;
    let open_ticket_count = tickets_count(&state.db, client_id).await?;
    let recent_tasks = project_tasks::Entity::find()
        .filter(project_tasks::Column::ClientId.eq(client_id))
        .order_by_desc(project_tasks::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    let recent_tickets = tickets::Entity::find()
        .filter(tickets::Column::ClientId.eq(client_id))
        .order_by_desc(tickets::Column::LastActivityAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({
        "clientId": client_id,
        "counts": {
            "openTasks": open_task_count,
            "blockedTasks": blocked_task_count,
            "slaRiskTasks": sla_risk_task_count,
            "pendingApprovals": pending_approval_count,
            "openTickets": open_ticket_count
        },
        "recent": {
            "tasks": recent_tasks,
            "tickets": recent_tickets
        }
    })))
}

async fn list_client_team_allocations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = team_allocations::Entity::find()
        .filter(team_allocations::Column::ClientId.eq(Some(client_id)))
        .order_by_desc(team_allocations::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "teamAllocations": rows })))
}

async fn create_team_allocation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateTeamAllocationRequest>,
) -> AppResult<(StatusCode, Json<team_allocations::Model>)> {
    require_write_access(&state, &headers, "team.allocations.create").await?;
    ensure_client(&state.db, client_id).await?;
    if let Some(campaign_id) = req.campaign_id {
        let campaign = ensure_campaign(&state.db, campaign_id).await?;
        if campaign.client_id != client_id {
            return Err(AppError::BadRequest(
                "campaignId must belong to the route client".to_string(),
            ));
        }
    }
    let model = team_allocations::ActiveModel {
        client_id: Set(Some(client_id)),
        campaign_id: Set(req.campaign_id),
        user_id: Set(req.user_id),
        role: Set(req.role),
        allocation_percent: Set(percent(req.allocation_percent.unwrap_or(100))?),
        starts_on: Set(req.starts_on),
        ends_on: Set(req.ends_on),
        billable: Set(req.billable.unwrap_or(true)),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn list_client_sync_runs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = integration_sync_runs::Entity::find()
        .filter(integration_sync_runs::Column::ClientId.eq(Some(client_id)))
        .order_by_desc(integration_sync_runs::Column::CreatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "syncRuns": rows })))
}

async fn client_outreach_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let sequence_count = outreach_sequences::Entity::find()
        .filter(outreach_sequences::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let active_sequence_count = outreach_sequences::Entity::find()
        .filter(outreach_sequences::Column::ClientId.eq(client_id))
        .filter(outreach_sequences::Column::Status.eq("active"))
        .count(&state.db)
        .await?;
    let active_enrollment_count = outreach_enrollments::Entity::find()
        .filter(outreach_enrollments::Column::ClientId.eq(client_id))
        .filter(outreach_enrollments::Column::Status.eq("active"))
        .count(&state.db)
        .await?;
    let recent_touchpoints = outreach_touchpoints::Entity::find()
        .filter(outreach_touchpoints::Column::ClientId.eq(client_id))
        .order_by_desc(outreach_touchpoints::Column::OccurredAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    let upcoming_enrollments = outreach_enrollments::Entity::find()
        .filter(outreach_enrollments::Column::ClientId.eq(client_id))
        .filter(outreach_enrollments::Column::Status.eq("active"))
        .order_by_asc(outreach_enrollments::Column::NextTouchAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({
        "clientId": client_id,
        "counts": {
            "sequences": sequence_count,
            "activeSequences": active_sequence_count,
            "activeEnrollments": active_enrollment_count
        },
        "recentTouchpoints": recent_touchpoints,
        "upcomingEnrollments": upcoming_enrollments
    })))
}

async fn list_client_outreach_sequences(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = outreach_sequences::Entity::find()
        .filter(outreach_sequences::Column::ClientId.eq(client_id))
        .order_by_desc(outreach_sequences::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "outreachSequences": rows })))
}

async fn list_client_research_briefs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = research_briefs::Entity::find()
        .filter(research_briefs::Column::ClientId.eq(client_id))
        .order_by_desc(research_briefs::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "researchBriefs": rows })))
}

async fn list_client_conversion_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = conversion_events::Entity::find()
        .filter(conversion_events::Column::ClientId.eq(client_id))
        .order_by_desc(conversion_events::Column::OccurredAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "conversionEvents": rows })))
}

async fn create_contact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateContactRequest>,
) -> AppResult<(StatusCode, Json<contacts::Model>)> {
    require_write_access(&state, &headers, "contacts.create").await?;
    ensure_client(&state.db, client_id).await?;
    let model = contacts::ActiveModel {
        client_id: Set(client_id),
        status: Set(req.status.unwrap_or_else(|| "active".to_string())),
        first_name: Set(req.first_name),
        last_name: Set(req.last_name),
        email: Set(req.email),
        phone: Set(req.phone),
        job_title: Set(req.job_title),
        lifecycle_role: Set(req.lifecycle_role.unwrap_or_else(|| "other".to_string())),
        consent_status: Set(req.consent_status.unwrap_or_else(|| "unknown".to_string())),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_contract(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateContractRequest>,
) -> AppResult<(StatusCode, Json<contracts::Model>)> {
    require_write_access(&state, &headers, "contracts.create").await?;
    ensure_client(&state.db, client_id).await?;
    let model = contracts::ActiveModel {
        client_id: Set(client_id),
        package_id: Set(req.package_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        contract_number: Set(req.contract_number),
        starts_on: Set(req.starts_on),
        ends_on: Set(req.ends_on),
        billing_terms: Set(object_or_default(req.billing_terms, "billingTerms")?),
        total_value_cents: Set(req.total_value_cents.unwrap_or(0)),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_invoice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateInvoiceRequest>,
) -> AppResult<(StatusCode, Json<invoices::Model>)> {
    require_write_access(&state, &headers, "invoices.create").await?;
    ensure_client(&state.db, client_id).await?;
    let model = invoices::ActiveModel {
        client_id: Set(client_id),
        contract_id: Set(req.contract_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        invoice_number: Set(req.invoice_number),
        due_on: Set(req.due_on),
        amount_cents: Set(req.amount_cents.unwrap_or(0)),
        line_items: Set(array_or_default(req.line_items, "lineItems")?),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_integration(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateIntegrationRequest>,
) -> AppResult<(StatusCode, Json<integrations::Model>)> {
    require_write_access(&state, &headers, "integrations.create").await?;
    ensure_client(&state.db, client_id).await?;
    let model = integrations::ActiveModel {
        client_id: Set(Some(client_id)),
        platform: Set(req.platform),
        status: Set(req.status.unwrap_or_else(|| "connected".to_string())),
        auth_kind: Set(req.auth_kind.unwrap_or_else(|| "manual".to_string())),
        external_account_id: Set(req.external_account_id),
        sync_cursor: Set(req.sync_cursor),
        config: Set(object_or_default(req.config, "config")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_integration_sync_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(integration_id): Path<Uuid>,
    Json(req): Json<CreateIntegrationSyncRunRequest>,
) -> AppResult<(StatusCode, Json<integration_sync_runs::Model>)> {
    require_write_access(&state, &headers, "integrations.sync-runs.create").await?;
    let integration = ensure_integration(&state.db, integration_id).await?;
    let client_id = req.client_id.or(integration.client_id);
    if let Some(client_id) = client_id {
        ensure_client(&state.db, client_id).await?;
    }
    let model = integration_sync_runs::ActiveModel {
        integration_id: Set(integration_id),
        client_id: Set(client_id),
        sync_kind: Set(req.sync_kind.unwrap_or_else(|| "incremental".to_string())),
        direction: Set(req.direction.unwrap_or_else(|| "import".to_string())),
        status: Set(req.status.unwrap_or_else(|| "queued".to_string())),
        records_seen: Set(non_negative(req.records_seen.unwrap_or(0), "recordsSeen")?),
        records_changed: Set(non_negative(
            req.records_changed.unwrap_or(0),
            "recordsChanged",
        )?),
        cursor_before: Set(req.cursor_before),
        cursor_after: Set(req.cursor_after),
        payload: Set(object_or_default(req.payload, "payload")?),
        error_summary: Set(req.error_summary),
        started_at: Set(req.started_at),
        completed_at: Set(req.completed_at),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;

    if model.status == "succeeded" {
        let mut active_integration: integrations::ActiveModel = integration.into();
        active_integration.last_sync_at = Set(Some(model.completed_at.unwrap_or_else(now_fixed)));
        if let Some(cursor_after) = model.cursor_after.clone() {
            active_integration.sync_cursor = Set(Some(cursor_after));
        }
        active_integration.updated_at = Set(now_fixed());
        active_integration.update(&state.db).await?;
    }

    state
        .metrics
        .integration_sync_runs_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "integration_sync_run_recorded",
        json!({
            "integrationId": integration_id,
            "clientId": client_id,
            "syncRunId": model.id,
            "syncKind": &model.sync_kind,
            "direction": &model.direction,
            "status": &model.status
        }),
    )
    .await;
    if let Some(client_id) = client_id {
        record_client_mutation(&state, client_id).await;
    } else {
        record_mutation(&state);
    }
    Ok((StatusCode::CREATED, Json(model)))
}

async fn import_leads(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LeadImportRequest>,
) -> AppResult<(StatusCode, Json<LeadImportResponse>)> {
    require_write_access(&state, &headers, "leads.import").await?;
    let client_id = req.client_id;
    let source_integration_id = req.source_integration_id;
    ensure_client(&state.db, client_id).await?;
    if req.leads.is_empty() {
        return Err(AppError::BadRequest(
            "leads must contain at least one item".to_string(),
        ));
    }
    if req.leads.len() > 500 {
        return Err(AppError::BadRequest(
            "lead import is limited to 500 records".to_string(),
        ));
    }
    let mut inserted = Vec::with_capacity(req.leads.len());
    for draft in req.leads {
        let model = leads::ActiveModel {
            client_id: Set(client_id),
            source_integration_id: Set(source_integration_id),
            status: Set("new".to_string()),
            company_name: Set(draft.company_name),
            domain: Set(draft.domain),
            contact_name: Set(draft.contact_name),
            contact_email: Set(draft.contact_email),
            contact_title: Set(draft.contact_title),
            country_code: Set(draft.country_code),
            lead_score: Set(score(draft.lead_score.unwrap_or(0))?),
            icp_fit_score: Set(score(draft.icp_fit_score.unwrap_or(0))?),
            verification_status: Set(draft
                .verification_status
                .unwrap_or_else(|| "unknown".to_string())),
            enrichment_status: Set("pending".to_string()),
            company_profile: Set(object_or_default(draft.company_profile, "companyProfile")?),
            signals: Set(array_or_default(draft.signals, "signals")?),
            meta_data: Set(object_or_default(draft.meta_data, "metaData")?),
            ..Default::default()
        }
        .insert(&state.db)
        .await?;
        inserted.push(model);
    }
    state
        .metrics
        .lead_imports_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "lead_import_batch",
        json!({
            "clientId": client_id,
            "imported": inserted.len(),
            "leadIds": inserted.iter().map(|lead| lead.id).collect::<Vec<_>>()
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((
        StatusCode::CREATED,
        Json(LeadImportResponse {
            imported: inserted.len(),
            leads: inserted,
        }),
    ))
}

async fn list_client_leads(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .order_by_desc(leads::Column::LeadScore)
        .order_by_desc(leads::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "leads": rows })))
}

async fn queue_enrichment_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(lead_id): Path<Uuid>,
    Json(req): Json<EnrichmentJobRequest>,
) -> AppResult<(StatusCode, Json<enrichment_jobs::Model>)> {
    require_write_access(&state, &headers, "leads.enrichment.queue").await?;
    let lead = leads::Entity::find_by_id(lead_id)
        .one(&state.db)
        .await?
        .ok_or(AppError::NotFound("lead"))?;
    let client_id = lead.client_id;
    let lead_id = lead.id;
    let job_id = Uuid::new_v4();
    let handoff_url = req.scraper_handoff_url.or_else(|| {
        state
            .cfg
            .scraper_base_url
            .as_ref()
            .map(|base| format!("{}/jobs/{}", base.trim_end_matches('/'), job_id))
    });
    let model = enrichment_jobs::ActiveModel {
        id: Set(job_id),
        client_id: Set(client_id),
        lead_id: Set(Some(lead_id)),
        job_kind: Set(req.job_kind),
        status: Set("queued".to_string()),
        external_job_id: Set(req.external_job_id),
        scraper_handoff_url: Set(handoff_url),
        input: Set(object_or_default(req.input, "input")?),
        result: Set(json!({})),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    let mut active_lead: leads::ActiveModel = lead.into();
    active_lead.enrichment_status = Set("running".to_string());
    active_lead.updated_at = Set(now_fixed());
    active_lead.update(&state.db).await?;
    state
        .metrics
        .enrichment_jobs_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "lead_enrichment_queued",
        json!({
            "clientId": client_id,
            "leadId": lead_id,
            "jobId": model.id,
            "jobKind": &model.job_kind,
            "scraperHandoffUrl": &model.scraper_handoff_url
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn score_lead(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(lead_id): Path<Uuid>,
    Json(req): Json<ScoreLeadRequest>,
) -> AppResult<Json<leads::Model>> {
    require_write_access(&state, &headers, "leads.score").await?;
    let lead = leads::Entity::find_by_id(lead_id)
        .one(&state.db)
        .await?
        .ok_or(AppError::NotFound("lead"))?;
    let client_id = lead.client_id;
    let mut active: leads::ActiveModel = lead.into();
    if let Some(value) = req.lead_score {
        active.lead_score = Set(score(value)?);
    }
    if let Some(value) = req.icp_fit_score {
        active.icp_fit_score = Set(score(value)?);
    }
    if let Some(value) = req.status {
        active.status = Set(value);
    }
    if let Some(value) = req.verification_status {
        active.verification_status = Set(value);
    }
    if let Some(value) = req.enrichment_status {
        active.enrichment_status = Set(value);
    }
    if let Some(value) = req.company_profile {
        active.company_profile = Set(object_or_default(Some(value), "companyProfile")?);
    }
    if let Some(value) = req.signals {
        active.signals = Set(array_or_default(Some(value), "signals")?);
    }
    active.updated_at = Set(now_fixed());
    let model = active.update(&state.db).await?;
    record_client_mutation(&state, client_id).await;
    Ok(Json(model))
}

async fn create_campaign(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateCampaignRequest>,
) -> AppResult<(StatusCode, Json<campaigns::Model>)> {
    require_write_access(&state, &headers, "campaigns.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = campaigns::ActiveModel {
        client_id: Set(client_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        campaign_kind: Set(req
            .campaign_kind
            .unwrap_or_else(|| "multi_channel".to_string())),
        name: Set(req.name),
        objective: Set(req.objective),
        budget_cents: Set(req.budget_cents.unwrap_or(0)),
        starts_on: Set(req.starts_on),
        ends_on: Set(req.ends_on),
        target_segments: Set(array_or_default(req.target_segments, "targetSegments")?),
        kpis: Set(object_or_default(req.kpis, "kpis")?),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn list_client_campaigns(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = campaigns::Entity::find()
        .filter(campaigns::Column::ClientId.eq(client_id))
        .order_by_desc(campaigns::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "campaigns": rows })))
}

async fn create_campaign_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(campaign_id): Path<Uuid>,
    Json(req): Json<CreateCampaignChannelRequest>,
) -> AppResult<(StatusCode, Json<campaign_channels::Model>)> {
    require_write_access(&state, &headers, "campaigns.channels.create").await?;
    let campaign = ensure_campaign(&state.db, campaign_id).await?;
    let model = campaign_channels::ActiveModel {
        campaign_id: Set(campaign_id),
        channel: Set(req.channel),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        external_campaign_id: Set(req.external_campaign_id),
        strategy: Set(object_or_default(req.strategy, "strategy")?),
        schedule: Set(object_or_default(req.schedule, "schedule")?),
        metrics_snapshot: Set(object_or_default(req.metrics_snapshot, "metricsSnapshot")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, campaign.client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_campaign_experiment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(campaign_id): Path<Uuid>,
    Json(req): Json<CreateCampaignExperimentRequest>,
) -> AppResult<(StatusCode, Json<campaign_experiments::Model>)> {
    require_write_access(&state, &headers, "campaigns.experiments.create").await?;
    let campaign = ensure_campaign(&state.db, campaign_id).await?;
    let model = campaign_experiments::ActiveModel {
        campaign_id: Set(campaign_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        experiment_kind: Set(req.experiment_kind),
        hypothesis: Set(req.hypothesis),
        variants: Set(array_or_default(req.variants, "variants")?),
        winning_variant: Set(req.winning_variant),
        result_summary: Set(object_or_default(req.result_summary, "resultSummary")?),
        started_at: Set(req.started_at),
        ended_at: Set(req.ended_at),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, campaign.client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_outreach_sequence(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateOutreachSequenceRequest>,
) -> AppResult<(StatusCode, Json<outreach_sequences::Model>)> {
    require_write_access(&state, &headers, "outreach.sequences.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    if let Some(campaign_id) = req.campaign_id {
        let campaign = ensure_campaign(&state.db, campaign_id).await?;
        if campaign.client_id != client_id {
            return Err(AppError::BadRequest(
                "campaignId must belong to the sequence client".to_string(),
            ));
        }
    }
    let model = outreach_sequences::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        channel: Set(req.channel.unwrap_or_else(|| "email".to_string())),
        name: Set(req.name),
        audience_filter: Set(object_or_default(req.audience_filter, "audienceFilter")?),
        cadence: Set(object_or_default(req.cadence, "cadence")?),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_outreach_step(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(sequence_id): Path<Uuid>,
    Json(req): Json<CreateOutreachStepRequest>,
) -> AppResult<(StatusCode, Json<outreach_steps::Model>)> {
    require_write_access(&state, &headers, "outreach.steps.create").await?;
    let sequence = ensure_outreach_sequence(&state.db, sequence_id).await?;
    let model = outreach_steps::ActiveModel {
        sequence_id: Set(sequence_id),
        status: Set(req.status.unwrap_or_else(|| "active".to_string())),
        step_order: Set(step_order(req.step_order)?),
        channel: Set(req.channel),
        delay_minutes: Set(non_negative(
            req.delay_minutes.unwrap_or(0),
            "delayMinutes",
        )?),
        subject: Set(req.subject),
        body_template: Set(req.body_template),
        personalization_hints: Set(array_or_default(
            req.personalization_hints,
            "personalizationHints",
        )?),
        experiment_key: Set(req.experiment_key),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, sequence.client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_outreach_enrollment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateOutreachEnrollmentRequest>,
) -> AppResult<(StatusCode, Json<outreach_enrollments::Model>)> {
    require_write_access(&state, &headers, "outreach.enrollments.create").await?;
    if req.lead_id.is_none() && req.contact_id.is_none() {
        return Err(AppError::BadRequest(
            "leadId or contactId is required".to_string(),
        ));
    }
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let sequence = ensure_outreach_sequence(&state.db, req.sequence_id).await?;
    if sequence.client_id != client_id {
        return Err(AppError::BadRequest(
            "sequenceId must belong to the enrollment client".to_string(),
        ));
    }
    ensure_optional_lead(&state.db, client_id, req.lead_id).await?;
    ensure_optional_contact(&state.db, client_id, req.contact_id).await?;
    let model = outreach_enrollments::ActiveModel {
        client_id: Set(client_id),
        sequence_id: Set(req.sequence_id),
        lead_id: Set(req.lead_id),
        contact_id: Set(req.contact_id),
        status: Set(req.status.unwrap_or_else(|| "active".to_string())),
        current_step_order: Set(step_order(req.current_step_order.unwrap_or(1))?),
        enrollment_context: Set(object_or_default(
            req.enrollment_context,
            "enrollmentContext",
        )?),
        last_touch_at: Set(req.last_touch_at),
        next_touch_at: Set(req.next_touch_at),
        outcome: Set(req.outcome),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    publish_job_event(
        &state,
        "outreach_enrollment_created",
        json!({
            "clientId": client_id,
            "sequenceId": model.sequence_id,
            "enrollmentId": model.id,
            "leadId": model.lead_id,
            "contactId": model.contact_id,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn record_outreach_touchpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RecordOutreachTouchpointRequest>,
) -> AppResult<(StatusCode, Json<outreach_touchpoints::Model>)> {
    require_write_access(&state, &headers, "outreach.touchpoints.record").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    if let Some(sequence_id) = req.sequence_id {
        let sequence = ensure_outreach_sequence(&state.db, sequence_id).await?;
        if sequence.client_id != client_id {
            return Err(AppError::BadRequest(
                "sequenceId must belong to the touchpoint client".to_string(),
            ));
        }
    }
    ensure_optional_enrollment(&state.db, client_id, req.enrollment_id).await?;
    if let Some(campaign_id) = req.campaign_id {
        let campaign = ensure_campaign(&state.db, campaign_id).await?;
        if campaign.client_id != client_id {
            return Err(AppError::BadRequest(
                "campaignId must belong to the touchpoint client".to_string(),
            ));
        }
    }
    ensure_optional_lead(&state.db, client_id, req.lead_id).await?;
    ensure_optional_contact(&state.db, client_id, req.contact_id).await?;
    let model = outreach_touchpoints::ActiveModel {
        client_id: Set(client_id),
        sequence_id: Set(req.sequence_id),
        enrollment_id: Set(req.enrollment_id),
        campaign_id: Set(req.campaign_id),
        lead_id: Set(req.lead_id),
        contact_id: Set(req.contact_id),
        channel: Set(req.channel),
        direction: Set(req.direction.unwrap_or_else(|| "outbound".to_string())),
        status: Set(req.status.unwrap_or_else(|| "planned".to_string())),
        subject: Set(req.subject),
        body_excerpt: Set(req.body_excerpt),
        external_message_id: Set(req.external_message_id),
        occurred_at: Set(req.occurred_at.unwrap_or_else(now_fixed)),
        payload: Set(object_or_default(req.payload, "payload")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state
        .metrics
        .outreach_touchpoints_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "outreach_touchpoint_recorded",
        json!({
            "clientId": client_id,
            "touchpointId": model.id,
            "sequenceId": model.sequence_id,
            "enrollmentId": model.enrollment_id,
            "leadId": model.lead_id,
            "contactId": model.contact_id,
            "channel": &model.channel,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_automation_workflow(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateAutomationWorkflowRequest>,
) -> AppResult<(StatusCode, Json<automation_workflows::Model>)> {
    require_write_access(&state, &headers, "automation.workflows.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = automation_workflows::ActiveModel {
        client_id: Set(client_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        name: Set(req.name),
        trigger_kind: Set(req.trigger_kind),
        trigger_config: Set(object_or_default(req.trigger_config, "triggerConfig")?),
        action_graph: Set(object_or_default(req.action_graph, "actionGraph")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn record_automation_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AutomationEventRequest>,
) -> AppResult<(StatusCode, Json<automation_events::Model>)> {
    require_write_access(&state, &headers, "automation.events.record").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = automation_events::ActiveModel {
        client_id: Set(client_id),
        workflow_id: Set(req.workflow_id),
        lead_id: Set(req.lead_id),
        event_kind: Set(req.event_kind),
        status: Set(req.status.unwrap_or_else(|| "received".to_string())),
        payload: Set(object_or_default(req.payload, "payload")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    publish_job_event(
        &state,
        "automation_event_recorded",
        json!({
            "clientId": client_id,
            "eventId": model.id,
            "workflowId": model.workflow_id,
            "leadId": model.lead_id,
            "eventKind": &model.event_kind,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_report_snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ReportSnapshotRequest>,
) -> AppResult<(StatusCode, Json<reports::Model>)> {
    require_write_access(&state, &headers, "reports.snapshots.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = reports::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        report_kind: Set(req.report_kind.unwrap_or_else(|| "dashboard".to_string())),
        status: Set(req.status.unwrap_or_else(|| "ready".to_string())),
        period_start: Set(req.period_start),
        period_end: Set(req.period_end),
        metrics: Set(object_or_default(req.metrics, "metrics")?),
        narrative: Set(req.narrative),
        delivery_targets: Set(array_or_default(req.delivery_targets, "deliveryTargets")?),
        generated_at: Set(req.generated_at.or_else(|| Some(now_fixed()))),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    publish_job_event(
        &state,
        "report_snapshot_ready",
        json!({
            "clientId": client_id,
            "reportId": model.id,
            "campaignId": model.campaign_id,
            "reportKind": &model.report_kind,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn record_attribution_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AttributionEventRequest>,
) -> AppResult<(StatusCode, Json<attribution_events::Model>)> {
    require_write_access(&state, &headers, "attribution.events.record").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = attribution_events::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        lead_id: Set(req.lead_id),
        event_type: Set(req.event_type),
        source_platform: Set(req.source_platform),
        source_event_id: Set(req.source_event_id),
        occurred_at: Set(req.occurred_at.unwrap_or_else(now_fixed)),
        value_cents: Set(req.value_cents.unwrap_or(0)),
        payload: Set(object_or_default(req.payload, "payload")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    publish_job_event(
        &state,
        "attribution_event_recorded",
        json!({
            "clientId": client_id,
            "eventId": model.id,
            "campaignId": model.campaign_id,
            "leadId": model.lead_id,
            "eventType": &model.event_type,
            "sourcePlatform": &model.source_platform,
            "valueCents": model.value_cents
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_opportunity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateOpportunityRequest>,
) -> AppResult<(StatusCode, Json<opportunities::Model>)> {
    require_write_access(&state, &headers, "opportunities.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = opportunities::ActiveModel {
        client_id: Set(client_id),
        lead_id: Set(req.lead_id),
        status: Set(req.status.unwrap_or_else(|| "open".to_string())),
        stage: Set(req.stage.unwrap_or_else(|| "prospecting".to_string())),
        name: Set(req.name),
        amount_cents: Set(req.amount_cents.unwrap_or(0)),
        probability_micros: Set(probability(req.probability_micros.unwrap_or(0))?),
        expected_close_on: Set(req.expected_close_on),
        owner_user_id: Set(req.owner_user_id),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_content_asset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateContentAssetRequest>,
) -> AppResult<(StatusCode, Json<content_assets::Model>)> {
    require_write_access(&state, &headers, "content.assets.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = content_assets::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        asset_kind: Set(req.asset_kind),
        title: Set(req.title),
        channel: Set(req.channel),
        body: Set(req.body),
        asset_uri: Set(req.asset_uri),
        seo_keywords: Set(array_or_default(req.seo_keywords, "seoKeywords")?),
        approval_status: Set(req.approval_status.unwrap_or_else(|| "pending".to_string())),
        publish_at: Set(req.publish_at),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_research_brief(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateResearchBriefRequest>,
) -> AppResult<(StatusCode, Json<research_briefs::Model>)> {
    require_write_access(&state, &headers, "research.briefs.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    ensure_optional_lead(&state.db, client_id, req.lead_id).await?;
    let model = research_briefs::ActiveModel {
        client_id: Set(client_id),
        lead_id: Set(req.lead_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        research_kind: Set(req
            .research_kind
            .unwrap_or_else(|| "account_research".to_string())),
        source: Set(req.source.unwrap_or_else(|| "ai_assisted".to_string())),
        summary: Set(req.summary),
        findings: Set(array_or_default(req.findings, "findings")?),
        recommended_actions: Set(array_or_default(
            req.recommended_actions,
            "recommendedActions",
        )?),
        confidence_micros: Set(probability(req.confidence_micros.unwrap_or(0))?),
        model_name: Set(req.model_name),
        generated_at: Set(req.generated_at.or_else(|| Some(now_fixed()))),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state
        .metrics
        .research_briefs_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "prospect_research_brief_created",
        json!({
            "clientId": client_id,
            "briefId": model.id,
            "leadId": model.lead_id,
            "researchKind": &model.research_kind,
            "source": &model.source,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn record_conversion_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RecordConversionEventRequest>,
) -> AppResult<(StatusCode, Json<conversion_events::Model>)> {
    require_write_access(&state, &headers, "conversion.events.record").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    if let Some(campaign_id) = req.campaign_id {
        let campaign = ensure_campaign(&state.db, campaign_id).await?;
        if campaign.client_id != client_id {
            return Err(AppError::BadRequest(
                "campaignId must belong to the conversion client".to_string(),
            ));
        }
    }
    ensure_optional_lead(&state.db, client_id, req.lead_id).await?;
    ensure_optional_content_asset(&state.db, client_id, req.content_asset_id).await?;
    let model = conversion_events::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        lead_id: Set(req.lead_id),
        content_asset_id: Set(req.content_asset_id),
        event_type: Set(req.event_type),
        source_platform: Set(req.source_platform),
        source_event_id: Set(req.source_event_id),
        session_id: Set(req.session_id),
        visitor_key: Set(req.visitor_key),
        occurred_at: Set(req.occurred_at.unwrap_or_else(now_fixed)),
        value_cents: Set(non_negative(req.value_cents.unwrap_or(0), "valueCents")?),
        utm: Set(object_or_default(req.utm, "utm")?),
        payload: Set(object_or_default(req.payload, "payload")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state
        .metrics
        .conversion_events_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "conversion_event_recorded",
        json!({
            "clientId": client_id,
            "eventId": model.id,
            "campaignId": model.campaign_id,
            "leadId": model.lead_id,
            "eventType": &model.event_type,
            "sourcePlatform": &model.source_platform,
            "valueCents": model.value_cents
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_project_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateProjectTaskRequest>,
) -> AppResult<(StatusCode, Json<project_tasks::Model>)> {
    require_write_access(&state, &headers, "projects.tasks.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = project_tasks::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        content_asset_id: Set(req.content_asset_id),
        status: Set(req.status.unwrap_or_else(|| "todo".to_string())),
        priority: Set(req.priority.unwrap_or_else(|| "normal".to_string())),
        title: Set(req.title),
        description: Set(req.description),
        assigned_to: Set(req.assigned_to),
        due_on: Set(req.due_on),
        sla_due_at: Set(req.sla_due_at),
        time_spent_minutes: Set(req.time_spent_minutes.unwrap_or(0)),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_approval(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateApprovalRequest>,
) -> AppResult<(StatusCode, Json<client_approvals::Model>)> {
    require_write_access(&state, &headers, "approvals.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = client_approvals::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        content_asset_id: Set(req.content_asset_id),
        requested_by: Set(req.requested_by),
        status: Set("pending".to_string()),
        approval_kind: Set(req.approval_kind),
        title: Set(req.title),
        request_payload: Set(object_or_default(req.request_payload, "requestPayload")?),
        due_at: Set(req.due_at),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn decide_approval(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(approval_id): Path<Uuid>,
    Json(req): Json<DecideApprovalRequest>,
) -> AppResult<Json<client_approvals::Model>> {
    require_write_access(&state, &headers, "approvals.decide").await?;
    if !["approved", "rejected", "canceled", "expired"].contains(&req.status.as_str()) {
        return Err(AppError::BadRequest(
            "approval decision status must be approved, rejected, canceled, or expired".to_string(),
        ));
    }
    let approval = client_approvals::Entity::find_by_id(approval_id)
        .one(&state.db)
        .await?
        .ok_or(AppError::NotFound("approval"))?;
    let client_id = approval.client_id;
    let mut active: client_approvals::ActiveModel = approval.into();
    active.status = Set(req.status);
    active.response_note = Set(req.response_note);
    active.decided_at = Set(Some(now_fixed()));
    active.updated_at = Set(now_fixed());
    let model = active.update(&state.db).await?;
    record_client_mutation(&state, client_id).await;
    Ok(Json(model))
}

async fn create_ticket(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateTicketRequest>,
) -> AppResult<(StatusCode, Json<tickets::Model>)> {
    require_write_access(&state, &headers, "tickets.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = tickets::ActiveModel {
        client_id: Set(client_id),
        status: Set(req.status.unwrap_or_else(|| "open".to_string())),
        priority: Set(req.priority.unwrap_or_else(|| "normal".to_string())),
        subject: Set(req.subject),
        description: Set(req.description),
        source: Set(req.source.unwrap_or_else(|| "portal".to_string())),
        assigned_to: Set(req.assigned_to),
        last_activity_at: Set(now_fixed()),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_meeting(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateMeetingRequest>,
) -> AppResult<(StatusCode, Json<meetings::Model>)> {
    require_write_access(&state, &headers, "meetings.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = meetings::ActiveModel {
        client_id: Set(client_id),
        lead_id: Set(req.lead_id),
        opportunity_id: Set(req.opportunity_id),
        status: Set(req.status.unwrap_or_else(|| "scheduled".to_string())),
        meeting_kind: Set(req.meeting_kind),
        title: Set(req.title),
        scheduled_at: Set(req.scheduled_at),
        duration_minutes: Set(req.duration_minutes.unwrap_or(30)),
        notes: Set(req.notes),
        recording_uri: Set(req.recording_uri),
        transcript_summary: Set(object_or_default(
            req.transcript_summary,
            "transcriptSummary",
        )?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
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

async fn ensure_client(db: &DatabaseConnection, client_id: Uuid) -> AppResult<clients::Model> {
    clients::Entity::find_by_id(client_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("client"))
}

async fn ensure_campaign(
    db: &DatabaseConnection,
    campaign_id: Uuid,
) -> AppResult<campaigns::Model> {
    campaigns::Entity::find_by_id(campaign_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("campaign"))
}

async fn ensure_integration(
    db: &DatabaseConnection,
    integration_id: Uuid,
) -> AppResult<integrations::Model> {
    integrations::Entity::find_by_id(integration_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("integration"))
}

async fn ensure_outreach_sequence(
    db: &DatabaseConnection,
    sequence_id: Uuid,
) -> AppResult<outreach_sequences::Model> {
    outreach_sequences::Entity::find_by_id(sequence_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("outreach sequence"))
}

async fn ensure_optional_lead(
    db: &DatabaseConnection,
    client_id: Uuid,
    lead_id: Option<Uuid>,
) -> AppResult<()> {
    let Some(lead_id) = lead_id else {
        return Ok(());
    };
    let lead = leads::Entity::find_by_id(lead_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("lead"))?;
    if lead.client_id != client_id {
        return Err(AppError::BadRequest(
            "leadId must belong to the request client".to_string(),
        ));
    }
    Ok(())
}

async fn ensure_optional_contact(
    db: &DatabaseConnection,
    client_id: Uuid,
    contact_id: Option<Uuid>,
) -> AppResult<()> {
    let Some(contact_id) = contact_id else {
        return Ok(());
    };
    let contact = contacts::Entity::find_by_id(contact_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("contact"))?;
    if contact.client_id != client_id {
        return Err(AppError::BadRequest(
            "contactId must belong to the request client".to_string(),
        ));
    }
    Ok(())
}

async fn ensure_optional_content_asset(
    db: &DatabaseConnection,
    client_id: Uuid,
    content_asset_id: Option<Uuid>,
) -> AppResult<()> {
    let Some(content_asset_id) = content_asset_id else {
        return Ok(());
    };
    let asset = content_assets::Entity::find_by_id(content_asset_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("content asset"))?;
    if asset.client_id != client_id {
        return Err(AppError::BadRequest(
            "contentAssetId must belong to the request client".to_string(),
        ));
    }
    Ok(())
}

async fn ensure_optional_enrollment(
    db: &DatabaseConnection,
    client_id: Uuid,
    enrollment_id: Option<Uuid>,
) -> AppResult<()> {
    let Some(enrollment_id) = enrollment_id else {
        return Ok(());
    };
    let enrollment = outreach_enrollments::Entity::find_by_id(enrollment_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("outreach enrollment"))?;
    if enrollment.client_id != client_id {
        return Err(AppError::BadRequest(
            "enrollmentId must belong to the request client".to_string(),
        ));
    }
    Ok(())
}

async fn tickets_count(db: &DatabaseConnection, client_id: Uuid) -> AppResult<u64> {
    Ok(tickets::Entity::find()
        .filter(tickets::Column::ClientId.eq(client_id))
        .filter(tickets::Column::Status.is_in(["open", "pending_client", "pending_agency"]))
        .count(db)
        .await?)
}

fn require_auth(state: &AppState, headers: &HeaderMap) -> AppResult<()> {
    if state.cfg.allow_unauthenticated {
        return Ok(());
    }
    let Some(expected) = state.cfg.api_auth_bearer.as_deref() else {
        state
            .metrics
            .auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(AppError::Unauthorized);
    };
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let legacy_auth = headers
        .get("Auth")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let bearer = authorization
        .strip_prefix("Bearer ")
        .map(str::trim)
        .unwrap_or(authorization);
    if bearer == expected || legacy_auth == expected {
        return Ok(());
    }
    state
        .metrics
        .auth_failures_total
        .fetch_add(1, Ordering::Relaxed);
    Err(AppError::Unauthorized)
}

async fn require_write_access(
    state: &AppState,
    headers: &HeaderMap,
    action: &'static str,
) -> AppResult<()> {
    require_auth(state, headers)?;
    enforce_rate_limit(state, headers, action).await
}

async fn enforce_rate_limit(
    state: &AppState,
    headers: &HeaderMap,
    action: &'static str,
) -> AppResult<()> {
    if state.cfg.rate_limit_per_minute == 0 || state.redis.is_none() {
        return Ok(());
    }

    let key = format!(
        "benefactor:marketing:rate:{action}:{}",
        auth_actor_hash(headers)
    );
    let Some(count) = redis_incr_with_ttl(state, &key, 60).await else {
        return Ok(());
    };
    if count > state.cfg.rate_limit_per_minute as i64 {
        state
            .metrics
            .rate_limit_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(AppError::RateLimited);
    }
    Ok(())
}

fn auth_actor_hash(headers: &HeaderMap) -> String {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let legacy_auth = headers
        .get("Auth")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let actor = if authorization.is_empty() {
        legacy_auth
    } else {
        authorization
    };
    let mut hasher = DefaultHasher::new();
    actor.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

async fn redis_ping(state: &AppState) -> bool {
    if state.redis.is_none() {
        return false;
    }
    let mut guard = state.redis_connection.lock().await;
    if !ensure_redis_connection(state, &mut *guard).await {
        return false;
    }
    let result: redis::RedisResult<String> = {
        let conn = guard.as_mut().expect("redis connection ensured");
        redis::cmd("PING").query_async(conn).await
    };
    match result {
        Ok(_) => true,
        Err(err) => {
            *guard = None;
            record_redis_error(state, "ping", &err);
            false
        }
    }
}

async fn cache_get_json(state: &AppState, key: &str) -> Option<Value> {
    if state.redis.is_none() || state.cfg.cache_ttl_seconds == 0 {
        return None;
    }
    let mut guard = state.redis_connection.lock().await;
    if !ensure_redis_connection(state, &mut *guard).await {
        return None;
    }
    let result: redis::RedisResult<Option<String>> = {
        let conn = guard.as_mut().expect("redis connection ensured");
        redis::cmd("GET").arg(key).query_async(conn).await
    };
    match result {
        Ok(Some(body)) => match serde_json::from_str(&body) {
            Ok(value) => {
                state
                    .metrics
                    .cache_hits_total
                    .fetch_add(1, Ordering::Relaxed);
                Some(value)
            }
            Err(err) => {
                warn!(error = %err, key, "redis cache payload was not valid JSON");
                state
                    .metrics
                    .cache_misses_total
                    .fetch_add(1, Ordering::Relaxed);
                None
            }
        },
        Ok(None) => {
            state
                .metrics
                .cache_misses_total
                .fetch_add(1, Ordering::Relaxed);
            None
        }
        Err(err) => {
            *guard = None;
            record_redis_error(state, "cache_get", &err);
            None
        }
    }
}

async fn cache_set_json(state: &AppState, key: &str, value: &Value) {
    if state.redis.is_none() || state.cfg.cache_ttl_seconds == 0 {
        return;
    }
    let Ok(body) = serde_json::to_string(value) else {
        return;
    };
    let mut guard = state.redis_connection.lock().await;
    if !ensure_redis_connection(state, &mut *guard).await {
        return;
    }
    let result: redis::RedisResult<()> = {
        let conn = guard.as_mut().expect("redis connection ensured");
        redis::cmd("SETEX")
            .arg(key)
            .arg(state.cfg.cache_ttl_seconds)
            .arg(body)
            .query_async(conn)
            .await
    };
    if let Err(err) = result {
        *guard = None;
        record_redis_error(state, "cache_set", &err);
    }
}

async fn cache_delete(state: &AppState, key: &str) {
    if state.redis.is_none() {
        return;
    }
    let mut guard = state.redis_connection.lock().await;
    if !ensure_redis_connection(state, &mut *guard).await {
        return;
    }
    let result: redis::RedisResult<u64> = {
        let conn = guard.as_mut().expect("redis connection ensured");
        redis::cmd("DEL").arg(key).query_async(conn).await
    };
    match result {
        Ok(deleted) if deleted > 0 => {
            state
                .metrics
                .cache_invalidations_total
                .fetch_add(deleted, Ordering::Relaxed);
        }
        Ok(_) => {}
        Err(err) => {
            *guard = None;
            record_redis_error(state, "cache_delete", &err);
        }
    }
}

async fn redis_incr_with_ttl(state: &AppState, key: &str, ttl_seconds: i64) -> Option<i64> {
    let mut guard = state.redis_connection.lock().await;
    if !ensure_redis_connection(state, &mut *guard).await {
        return None;
    }
    let result: redis::RedisResult<i64> = {
        let conn = guard.as_mut().expect("redis connection ensured");
        redis::cmd("INCR").arg(key).query_async(conn).await
    };
    match result {
        Ok(count) => {
            if count == 1 {
                let expire_result: redis::RedisResult<bool> = {
                    let conn = guard.as_mut().expect("redis connection ensured");
                    redis::cmd("EXPIRE")
                        .arg(key)
                        .arg(ttl_seconds)
                        .query_async(conn)
                        .await
                };
                if let Err(err) = expire_result {
                    record_redis_error(state, "rate_limit_expire", &err);
                }
            }
            Some(count)
        }
        Err(err) => {
            *guard = None;
            record_redis_error(state, "rate_limit_incr", &err);
            None
        }
    }
}

async fn publish_job_event(state: &AppState, event_kind: &'static str, payload: Value) {
    if state.redis.is_none() {
        return;
    }
    let Ok(payload_body) = serde_json::to_string(&payload) else {
        return;
    };
    let mut guard = state.redis_connection.lock().await;
    if !ensure_redis_connection(state, &mut *guard).await {
        return;
    }
    let result: redis::RedisResult<String> = {
        let conn = guard.as_mut().expect("redis connection ensured");
        redis::cmd("XADD")
            .arg(&state.cfg.job_stream)
            .arg("*")
            .arg("service")
            .arg(SERVICE_NAME)
            .arg("eventKind")
            .arg(event_kind)
            .arg("payload")
            .arg(payload_body)
            .query_async(conn)
            .await
    };
    match result {
        Ok(_) => {
            state
                .metrics
                .redis_jobs_published_total
                .fetch_add(1, Ordering::Relaxed);
        }
        Err(err) => {
            *guard = None;
            record_redis_error(state, "stream_publish", &err);
        }
    }
}

async fn ensure_redis_connection(
    state: &AppState,
    connection: &mut Option<redis::aio::MultiplexedConnection>,
) -> bool {
    if connection.is_some() {
        return true;
    }
    let Some(client) = state.redis.as_ref() else {
        return false;
    };
    match client.get_multiplexed_async_connection().await {
        Ok(conn) => {
            *connection = Some(conn);
            true
        }
        Err(err) => {
            record_redis_error(state, "connect", &err);
            false
        }
    }
}

fn record_redis_error(state: &AppState, context: &'static str, err: &redis::RedisError) {
    state
        .metrics
        .redis_errors_total
        .fetch_add(1, Ordering::Relaxed);
    warn!(context, error = %err, "redis operation failed");
}

fn object_or_default(value: Option<Value>, field: &str) -> AppResult<Value> {
    match value {
        Some(value) if value.is_object() => Ok(value),
        Some(_) => Err(AppError::BadRequest(format!(
            "{field} must be a JSON object"
        ))),
        None => Ok(json!({})),
    }
}

fn array_or_default(value: Option<Value>, field: &str) -> AppResult<Value> {
    match value {
        Some(value) if value.is_array() => Ok(value),
        Some(_) => Err(AppError::BadRequest(format!(
            "{field} must be a JSON array"
        ))),
        None => Ok(json!([])),
    }
}

fn limit(value: Option<u64>) -> u64 {
    value.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

fn score(value: i32) -> AppResult<i32> {
    if (0..=100).contains(&value) {
        Ok(value)
    } else {
        Err(AppError::BadRequest(
            "scores must be between 0 and 100".to_string(),
        ))
    }
}

fn probability(value: i32) -> AppResult<i32> {
    if (0..=1_000_000).contains(&value) {
        Ok(value)
    } else {
        Err(AppError::BadRequest(
            "probabilityMicros must be between 0 and 1000000".to_string(),
        ))
    }
}

fn percent(value: i32) -> AppResult<i32> {
    if (0..=100).contains(&value) {
        Ok(value)
    } else {
        Err(AppError::BadRequest(
            "allocationPercent must be between 0 and 100".to_string(),
        ))
    }
}

fn step_order(value: i32) -> AppResult<i32> {
    if (1..=100).contains(&value) {
        Ok(value)
    } else {
        Err(AppError::BadRequest(
            "step order must be between 1 and 100".to_string(),
        ))
    }
}

fn non_negative(value: i32, field: &str) -> AppResult<i32> {
    if value >= 0 {
        Ok(value)
    } else {
        Err(AppError::BadRequest(format!(
            "{field} must be non-negative"
        )))
    }
}

fn client_dashboard_cache_key(client_id: Uuid) -> String {
    format!("benefactor:marketing:client-dashboard:{client_id}")
}

fn record_mutation(state: &AppState) {
    state
        .metrics
        .mutations_total
        .fetch_add(1, Ordering::Relaxed);
}

async fn record_client_mutation(state: &AppState, client_id: Uuid) {
    record_mutation(state);
    cache_delete(state, &client_dashboard_cache_key(client_id)).await;
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn now_fixed() -> DateTime<FixedOffset> {
    Utc::now().fixed_offset()
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 220 {
            break;
        }
    }
    let mut out = out.trim_matches('-').to_string();
    if out.is_empty() {
        out = "client".to_string();
    }
    while out.len() < 3 {
        out.push('x');
    }
    out
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            error!(error = %err, "failed to install ctrl_c handler");
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                error!(error = %err, "failed to install SIGTERM handler");
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("ctrl_c received, shutting down"),
        _ = terminate => info!("SIGTERM received, shutting down"),
    }
}
