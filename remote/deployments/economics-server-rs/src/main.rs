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
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dd_nats_subject_defs::{PUBLIC_DATA_PIPELINE_JOBS_SUBJECT, RUNTIME_EVENTS_SUBJECT};
use des_engine::service::{EndpointKind, ServiceBuilder, ServiceInfo};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const SERVICE_NAME: &str = "dd-economics-server";
const SCHEMA_VERSION: &str = "economics.forecast.v1";
const DEFAULT_HISTORY_YEARS: u32 = 15;
const DEFAULT_PROJECTION_MONTHS: u32 = 18;
const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 1024 * 1024;
const MAX_SOURCE_FETCH_BYTES: usize = 2 * 1024 * 1024;
const MAX_SERIES: usize = 160;
const MAX_OBSERVATIONS_PER_SERIES: usize = 8_000;
const MAX_TOKEN_LEN: usize = 128;
const MAX_URL_LEN: usize = 2_048;
const MAX_JSON_POINTER_LEN: usize = 256;
const MAX_SENTIMENT_DOCUMENTS: usize = 512;
const MAX_SENTIMENT_TEXT_BYTES: usize = 4_096;
const MAX_SENTIMENT_CONTEXT_SCORES: usize = 512;
const MAX_VC_DEALS: usize = 256;
const MAX_VC_SECTOR_FLOWS: usize = 128;
const MAX_PIPELINE_JOB_INTENTS: usize = 12;
const ECONOMICS_FORECAST_REQUEST_SUBJECT: &str = "dd.remote.economics.forecast.requests";
const ECONOMICS_FORECAST_RESULT_SUBJECT: &str = "dd.remote.economics.forecast.results";
const ECONOMICS_MARKET_EVENT_SUBJECT: &str = "dd.remote.economics.market.events";
const DEFAULT_SPARK_PIPELINE_URL: &str =
    "http://dd-spark-pipeline-server.ai-ml.svc.cluster.local:8085";
const DEFAULT_SPARK_MASTER_URL: &str = "spark://spark-master.big-data.svc.cluster.local:7077";
const DEFAULT_AIRFLOW_API_URL: &str = "http://airflow.big-data.svc.cluster.local:8080";
const DEFAULT_DATA_LAKE_URI: &str = "s3a://dd-economics/market-signals";
const ECONOMICS_QUEUE_GROUP: &str = "dd-economics-server";

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    metrics: Arc<Metrics>,
    nats: Option<async_nats::Client>,
    http: reqwest::Client,
    series_store: Arc<RwLock<BTreeMap<String, MarketSeries>>>,
}

#[derive(Clone)]
struct Config {
    server_auth_secret: Option<String>,
    allow_unauthenticated: bool,
    allow_private_source_urls: bool,
    allowed_source_hosts: Vec<String>,
    allowed_source_auth_envs: Vec<String>,
    sentiment_credentials: SentimentCredentialStatus,
    market_data_credentials: MarketDataCredentialStatus,
    history_years: u32,
    projection_months: u32,
    confidence_level: f64,
    request_subject: String,
    queue_group: String,
    result_subject: String,
    market_event_subject: String,
    runtime_event_subject: String,
    pipeline_intent_subject: String,
    spark_pipeline_url: Option<String>,
    spark_pipeline_auth_env: String,
    spark_master_url: String,
    airflow_api_url: Option<String>,
    databricks_host: Option<String>,
    data_lake_uri: String,
    allow_pipeline_submit: bool,
    allow_external_pipeline_urls: bool,
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    forecasts_total: AtomicU64,
    ingest_requests_total: AtomicU64,
    source_pull_total: AtomicU64,
    source_pull_success_total: AtomicU64,
    source_pull_failure_total: AtomicU64,
    source_pull_bytes_total: AtomicU64,
    source_pull_stored_points_total: AtomicU64,
    source_pull_last_success_unix_seconds: AtomicU64,
    sentiment_requests_total: AtomicU64,
    recommendation_requests_total: AtomicU64,
    pipeline_plan_requests_total: AtomicU64,
    pipeline_submit_requests_total: AtomicU64,
    pipeline_publish_attempts_total: AtomicU64,
    pipeline_publish_success_total: AtomicU64,
    pipeline_publish_failure_total: AtomicU64,
    pipeline_submit_success_total: AtomicU64,
    pipeline_submit_failure_total: AtomicU64,
    integration_health_requests_total: AtomicU64,
    observability_requests_total: AtomicU64,
    auth_failures_total: AtomicU64,
    errors_total: AtomicU64,
    nats_messages_total: AtomicU64,
    nats_published_total: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForecastRequest {
    request_id: Option<String>,
    schema_version: Option<String>,
    horizon_months: Option<u32>,
    confidence_level: Option<f64>,
    scenario: Option<String>,
    series: Option<Vec<MarketSeries>>,
    macro_context: Option<MacroContext>,
    macro_fiscal_context: Option<MacroFiscalContext>,
    venture_capital_context: Option<VentureCapitalContext>,
    theory_weights: Option<TheoryWeights>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IngestRequest {
    request_id: Option<String>,
    replace: Option<bool>,
    series: Vec<MarketSeries>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarketSeries {
    instrument_id: String,
    display_name: Option<String>,
    asset_class: String,
    currency: Option<String>,
    source: Option<String>,
    observations: Vec<MarketObservation>,
    features: Option<AssetFeatures>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarketObservation {
    date: String,
    price: f64,
    volume: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct AssetFeatures {
    beta: Option<f64>,
    duration: Option<f64>,
    carry: Option<f64>,
    convenience_yield: Option<f64>,
    storage_cost: Option<f64>,
    supply_growth: Option<f64>,
    demand_growth: Option<f64>,
    inventory_ratio: Option<f64>,
    valuation_gap: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MacroContext {
    policy_rate: Option<f64>,
    foreign_policy_rate: Option<f64>,
    inflation: Option<f64>,
    foreign_inflation: Option<f64>,
    expected_inflation: Option<f64>,
    money_supply_growth: Option<f64>,
    real_growth: Option<f64>,
    output_gap: Option<f64>,
    unemployment_gap: Option<f64>,
    risk_free_rate: Option<f64>,
    market_return: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MacroFiscalContext {
    country: Option<String>,
    period: Option<String>,
    gdp: Option<f64>,
    gdp_growth: Option<f64>,
    national_debt: Option<f64>,
    debt_to_gdp: Option<f64>,
    deficit: Option<f64>,
    deficit_to_gdp: Option<f64>,
    receipts: Option<f64>,
    outlays: Option<f64>,
    borrowing: Option<f64>,
    net_interest_outlays: Option<f64>,
    labor_force_participation: Option<f64>,
    prime_age_participation: Option<f64>,
    unemployment_rate: Option<f64>,
    payroll_growth: Option<f64>,
    wage_growth: Option<f64>,
    productivity_growth: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct VentureCapitalContext {
    period: Option<String>,
    deals: Vec<VentureCapitalDealSignal>,
    sector_flows: Vec<VentureSectorFlow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VentureCapitalDealSignal {
    firm: String,
    company: String,
    sector: String,
    stage: String,
    amount: f64,
    currency: Option<String>,
    country: Option<String>,
    announced_at: Option<String>,
    confidence: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VentureSectorFlow {
    sector: String,
    deal_count: u32,
    invested_capital: f64,
    yoy_growth: f64,
    dry_powder: Option<f64>,
    exit_liquidity: Option<f64>,
    confidence: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TheoryWeights {
    data: Option<f64>,
    macro_theory: Option<f64>,
    momentum: Option<f64>,
    mean_reversion: Option<f64>,
    carry: Option<f64>,
    valuation: Option<f64>,
    jump_stress: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiPullRequest {
    request_id: Option<String>,
    source_id: Option<String>,
    url: Option<String>,
    parser: Option<SourceParser>,
    instrument_id: Option<String>,
    display_name: Option<String>,
    asset_class: Option<String>,
    currency: Option<String>,
    source: Option<String>,
    root_pointer: Option<String>,
    date_field: Option<String>,
    price_field: Option<String>,
    volume_field: Option<String>,
    date_index: Option<usize>,
    price_index: Option<usize>,
    volume_index: Option<usize>,
    auth_header_env: Option<String>,
    auth_header_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SourceParser {
    JsonRecords,
    JsonTupleArray,
    CsvRecords,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiPullResponse {
    ok: bool,
    request_id: String,
    source_id: Option<String>,
    source: String,
    parser: Option<SourceParser>,
    url_host: String,
    http_status: u16,
    bytes: usize,
    stored_points: usize,
    instrument_id: Option<String>,
    quality: Option<SourceQualityReport>,
    warnings: Vec<String>,
    fetched_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceQualityReport {
    parser: SourceParser,
    observed_points: usize,
    dropped_points: usize,
    first_date: Option<String>,
    last_date: Option<String>,
    min_price: Option<f64>,
    max_price: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ForecastResponse {
    ok: bool,
    request_id: String,
    schema_version: &'static str,
    history_years: u32,
    horizon_months: u32,
    confidence_level: f64,
    scenario: String,
    generated_at_ms: u128,
    des_engine: Value,
    equations: Vec<EquationDescriptor>,
    projections: Vec<Projection>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Projection {
    instrument_id: String,
    display_name: String,
    asset_class: String,
    currency: String,
    last_price: f64,
    annualized_drift: f64,
    annualized_volatility: f64,
    expected_return_18m: f64,
    signal: String,
    rationale: Vec<String>,
    components: Vec<ModelComponent>,
    points: Vec<ForecastPoint>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ForecastPoint {
    month: u32,
    label: String,
    expected: f64,
    lower: f64,
    upper: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelComponent {
    name: String,
    value: f64,
    weight: f64,
    equation: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EquationDescriptor {
    name: &'static str,
    family: &'static str,
    equation: &'static str,
    use_case: &'static str,
    caveat: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceDescriptor {
    id: &'static str,
    name: &'static str,
    asset_classes: &'static [&'static str],
    auth: &'static str,
    notes: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicSourceTemplate {
    id: &'static str,
    provider: &'static str,
    name: &'static str,
    asset_class: &'static str,
    instrument_id: &'static str,
    display_name: &'static str,
    currency: &'static str,
    source: &'static str,
    url: &'static str,
    host: &'static str,
    parser: SourceParser,
    root_pointer: Option<&'static str>,
    date_field: Option<&'static str>,
    price_field: Option<&'static str>,
    volume_field: Option<&'static str>,
    date_index: Option<usize>,
    price_index: Option<usize>,
    volume_index: Option<usize>,
    cadence: &'static str,
    documentation_url: &'static str,
    notes: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SentimentCredentialStatus {
    x_bearer_token: bool,
    x_api_key: bool,
    x_api_secret: bool,
    x_access_token: bool,
    x_access_token_secret: bool,
    reddit_client_id: bool,
    reddit_client_secret: bool,
    reddit_user_agent: bool,
    news_api_key: bool,
    stocktwits_token: bool,
    gdelt_api_key: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MarketDataCredentialStatus {
    fred_api_key: bool,
    bea_api_key: bool,
    bls_api_key: bool,
    treasury_api_key: bool,
    census_api_key: bool,
    eia_api_key: bool,
    coingecko_api_key: bool,
    sec_api_key: bool,
    crunchbase_api_key: bool,
    pitchbook_api_key: bool,
    cb_insights_api_key: bool,
    dealroom_api_key: bool,
    preqin_api_key: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PipelineIntegrationStatus {
    spark_pipeline_url_configured: bool,
    spark_pipeline_auth_configured: bool,
    spark_pipeline_submit_enabled: bool,
    spark_pipeline_url: Option<String>,
    spark_pipeline_auth_env: String,
    spark_master_url: String,
    airflow_api_url_configured: bool,
    airflow_api_url: Option<String>,
    databricks_host_configured: bool,
    databricks_token_configured: bool,
    data_lake_uri: String,
    pipeline_intent_subject: String,
    nats_configured: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct IntegrationDependencyStatus {
    id: String,
    kind: String,
    status: String,
    configured: bool,
    required_for_core_readiness: bool,
    mode: String,
    details: Value,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SentimentAnalyzeRequest {
    request_id: Option<String>,
    schema_version: Option<String>,
    query: Option<String>,
    instrument_ids: Option<Vec<String>>,
    documents: Vec<SentimentDocument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SentimentDocument {
    source: String,
    text: String,
    url: Option<String>,
    author: Option<String>,
    published_at: Option<String>,
    weight: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SentimentAnalyzeResponse {
    ok: bool,
    request_id: String,
    schema_version: &'static str,
    query: Option<String>,
    document_count: usize,
    average_sentiment: f64,
    confidence: f64,
    source_scores: Vec<SentimentSourceScore>,
    top_terms: Vec<String>,
    credential_status: SentimentCredentialStatus,
    generated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SentimentSourceScore {
    source: String,
    document_count: usize,
    average_sentiment: f64,
    confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SentimentSignalContext {
    average_sentiment: Option<f64>,
    instrument_scores: Option<BTreeMap<String, f64>>,
    sector_scores: Option<BTreeMap<String, f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecommendationRequest {
    request_id: Option<String>,
    schema_version: Option<String>,
    horizon_months: Option<u32>,
    company_limit: Option<usize>,
    commodity_limit: Option<usize>,
    scenario: Option<String>,
    series: Option<Vec<MarketSeries>>,
    macro_context: Option<MacroContext>,
    macro_fiscal_context: Option<MacroFiscalContext>,
    venture_capital_context: Option<VentureCapitalContext>,
    sentiment_context: Option<SentimentSignalContext>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecommendationsResponse {
    ok: bool,
    request_id: String,
    schema_version: &'static str,
    horizon_months: u32,
    scenario: String,
    generated_at_ms: u128,
    macro_fiscal_context: MacroFiscalContext,
    venture_capital_context: VentureCapitalContext,
    data_credential_status: MarketDataCredentialStatus,
    company_buys: Vec<CompanyRecommendation>,
    company_dumps: Vec<CompanyRecommendation>,
    commodity_buys: Vec<CommodityRecommendation>,
    commodity_sells_or_dumps: Vec<CommodityRecommendation>,
    methodology: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompanyRecommendation {
    rank: usize,
    ticker: String,
    company: String,
    sector: String,
    stage: String,
    action: String,
    score: f64,
    expected_return_18m: f64,
    confidence: f64,
    reasons: Vec<String>,
    components: Vec<RecommendationComponent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommodityRecommendation {
    rank: usize,
    instrument_id: String,
    commodity: String,
    commodity_class: String,
    action: String,
    score: f64,
    expected_return_18m: f64,
    confidence: f64,
    reasons: Vec<String>,
    components: Vec<RecommendationComponent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecommendationComponent {
    name: String,
    value: f64,
    weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PipelinePlanRequest {
    request_id: Option<String>,
    schema_version: Option<String>,
    scenario: Option<String>,
    data_lake_uri: Option<String>,
    include_recommendations: Option<bool>,
    publish_to_nats: Option<bool>,
    job_kinds: Option<Vec<String>>,
    recommendation_request: Option<RecommendationRequest>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PipelinePlanResponse {
    ok: bool,
    request_id: String,
    schema_version: &'static str,
    generated_at_ms: u128,
    pipeline_status: PipelineIntegrationStatus,
    recommendation_summary: Value,
    job_intents: Vec<PipelineJobIntent>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PipelineJobIntent {
    id: String,
    engine: String,
    target: String,
    kind: String,
    endpoint: Option<String>,
    auth_required: bool,
    submit_eligible: bool,
    params: Value,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PipelineSubmitResponse {
    ok: bool,
    request_id: String,
    schema_version: &'static str,
    generated_at_ms: u128,
    plan: PipelinePlanResponse,
    submitted_jobs: Vec<PipelineSubmittedJob>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PipelineSubmittedJob {
    intent_id: String,
    target: String,
    http_status: Option<u16>,
    accepted: bool,
    response: Option<Value>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct CompanyCandidate {
    ticker: &'static str,
    company: &'static str,
    sector: &'static str,
    stage: &'static str,
    beta: f64,
    profitability: f64,
    growth: f64,
    balance_sheet: f64,
    valuation_gap: f64,
    momentum: f64,
}

#[derive(Debug, Clone)]
struct CommodityCandidate {
    instrument_id: &'static str,
    commodity: &'static str,
    commodity_class: &'static str,
    supply_tightness: f64,
    demand_growth: f64,
    inventory_pressure: f64,
    carry: f64,
    geopolitical_risk: f64,
    valuation_gap: f64,
    volatility: f64,
}

struct SeriesStats {
    last_price: f64,
    volatility_per_period: f64,
    periods_per_year: f64,
    data_drift: f64,
    momentum: f64,
    mean_reversion: f64,
}

struct TheoryPrior {
    drift: f64,
    carry: f64,
    valuation: f64,
    jump_stress: f64,
    rationale: Vec<String>,
}

struct NormalizedWeights {
    data: f64,
    macro_theory: f64,
    momentum: f64,
    mean_reversion: f64,
    carry: f64,
    valuation: f64,
    jump_stress: f64,
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

fn env_list(key: &str) -> Vec<String> {
    env::var(key)
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(|item| item.trim().to_ascii_lowercase())
                .filter(|item| {
                    !item.is_empty()
                        && item.len() <= MAX_TOKEN_LEN
                        && !item.chars().any(char::is_control)
                })
                .take(64)
                .collect()
        })
        .unwrap_or_default()
}

fn default_source_auth_envs() -> Vec<String> {
    [
        "ECONOMICS_X_BEARER_TOKEN",
        "ECONOMICS_X_API_KEY",
        "ECONOMICS_X_API_SECRET",
        "ECONOMICS_X_ACCESS_TOKEN",
        "ECONOMICS_X_ACCESS_TOKEN_SECRET",
        "ECONOMICS_REDDIT_CLIENT_ID",
        "ECONOMICS_REDDIT_CLIENT_SECRET",
        "ECONOMICS_NEWS_API_KEY",
        "ECONOMICS_STOCKTWITS_TOKEN",
        "ECONOMICS_GDELT_API_KEY",
        "ECONOMICS_FRED_API_KEY",
        "ECONOMICS_BEA_API_KEY",
        "ECONOMICS_BLS_API_KEY",
        "ECONOMICS_TREASURY_API_KEY",
        "ECONOMICS_CENSUS_API_KEY",
        "ECONOMICS_EIA_API_KEY",
        "ECONOMICS_COINGECKO_API_KEY",
        "ECONOMICS_SEC_API_KEY",
        "ECONOMICS_CRUNCHBASE_API_KEY",
        "ECONOMICS_PITCHBOOK_API_KEY",
        "ECONOMICS_CB_INSIGHTS_API_KEY",
        "ECONOMICS_DEALROOM_API_KEY",
        "ECONOMICS_PREQIN_API_KEY",
        "ECONOMICS_DATABRICKS_TOKEN",
    ]
    .into_iter()
    .map(|value| value.to_ascii_lowercase())
    .collect()
}

fn configured_source_auth_envs() -> Vec<String> {
    let mut allowed = default_source_auth_envs()
        .into_iter()
        .collect::<BTreeSet<_>>();
    for env_name in env_list("ECONOMICS_ALLOWED_SOURCE_AUTH_ENVS") {
        allowed.insert(env_name);
    }
    allowed.into_iter().collect()
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

fn env_u32(key: &str, fallback: u32) -> u32 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn env_f64(key: &str, fallback: f64) -> f64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(fallback)
}

fn config_from_env() -> Config {
    Config {
        server_auth_secret: optional_env("SERVER_AUTH_SECRET")
            .or_else(|| optional_env("ECONOMICS_SERVER_AUTH_SECRET")),
        allow_unauthenticated: env_bool("ECONOMICS_ALLOW_UNAUTHENTICATED", false),
        allow_private_source_urls: env_bool("ECONOMICS_ALLOW_PRIVATE_SOURCE_URLS", false),
        allowed_source_hosts: env_list("ECONOMICS_ALLOWED_SOURCE_HOSTS"),
        allowed_source_auth_envs: configured_source_auth_envs(),
        sentiment_credentials: sentiment_credentials_from_env(),
        market_data_credentials: market_data_credentials_from_env(),
        history_years: env_u32("ECONOMICS_HISTORY_YEARS", DEFAULT_HISTORY_YEARS),
        projection_months: env_u32("ECONOMICS_PROJECTION_MONTHS", DEFAULT_PROJECTION_MONTHS),
        confidence_level: clamp(env_f64("ECONOMICS_CONFIDENCE_LEVEL", 0.90), 0.50, 0.995),
        request_subject: env_value(
            "ECONOMICS_FORECAST_REQUEST_SUBJECT",
            ECONOMICS_FORECAST_REQUEST_SUBJECT,
        ),
        queue_group: env_value("ECONOMICS_QUEUE_GROUP", ECONOMICS_QUEUE_GROUP),
        result_subject: env_value(
            "ECONOMICS_FORECAST_RESULT_SUBJECT",
            ECONOMICS_FORECAST_RESULT_SUBJECT,
        ),
        market_event_subject: env_value(
            "ECONOMICS_MARKET_EVENT_SUBJECT",
            ECONOMICS_MARKET_EVENT_SUBJECT,
        ),
        runtime_event_subject: env_value("ECONOMICS_RUNTIME_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
        pipeline_intent_subject: env_value(
            "ECONOMICS_PIPELINE_INTENT_SUBJECT",
            PUBLIC_DATA_PIPELINE_JOBS_SUBJECT,
        ),
        spark_pipeline_url: optional_env("ECONOMICS_SPARK_PIPELINE_URL")
            .or_else(|| Some(DEFAULT_SPARK_PIPELINE_URL.to_string())),
        spark_pipeline_auth_env: env_value(
            "ECONOMICS_SPARK_PIPELINE_AUTH_ENV",
            "SERVER_AUTH_SECRET",
        ),
        spark_master_url: env_value("ECONOMICS_SPARK_MASTER_URL", DEFAULT_SPARK_MASTER_URL),
        airflow_api_url: optional_env("ECONOMICS_AIRFLOW_API_URL")
            .or_else(|| Some(DEFAULT_AIRFLOW_API_URL.to_string())),
        databricks_host: optional_env("ECONOMICS_DATABRICKS_HOST"),
        data_lake_uri: env_value("ECONOMICS_DATA_LAKE_URI", DEFAULT_DATA_LAKE_URI),
        allow_pipeline_submit: env_bool("ECONOMICS_ENABLE_PIPELINE_SUBMIT", false),
        allow_external_pipeline_urls: env_bool("ECONOMICS_ALLOW_EXTERNAL_PIPELINE_URLS", false),
    }
}

fn sentiment_credentials_from_env() -> SentimentCredentialStatus {
    SentimentCredentialStatus {
        x_bearer_token: optional_env("ECONOMICS_X_BEARER_TOKEN").is_some(),
        x_api_key: optional_env("ECONOMICS_X_API_KEY").is_some(),
        x_api_secret: optional_env("ECONOMICS_X_API_SECRET").is_some(),
        x_access_token: optional_env("ECONOMICS_X_ACCESS_TOKEN").is_some(),
        x_access_token_secret: optional_env("ECONOMICS_X_ACCESS_TOKEN_SECRET").is_some(),
        reddit_client_id: optional_env("ECONOMICS_REDDIT_CLIENT_ID").is_some(),
        reddit_client_secret: optional_env("ECONOMICS_REDDIT_CLIENT_SECRET").is_some(),
        reddit_user_agent: optional_env("ECONOMICS_REDDIT_USER_AGENT").is_some(),
        news_api_key: optional_env("ECONOMICS_NEWS_API_KEY").is_some(),
        stocktwits_token: optional_env("ECONOMICS_STOCKTWITS_TOKEN").is_some(),
        gdelt_api_key: optional_env("ECONOMICS_GDELT_API_KEY").is_some(),
    }
}

fn market_data_credentials_from_env() -> MarketDataCredentialStatus {
    MarketDataCredentialStatus {
        fred_api_key: optional_env("ECONOMICS_FRED_API_KEY").is_some(),
        bea_api_key: optional_env("ECONOMICS_BEA_API_KEY").is_some(),
        bls_api_key: optional_env("ECONOMICS_BLS_API_KEY").is_some(),
        treasury_api_key: optional_env("ECONOMICS_TREASURY_API_KEY").is_some(),
        census_api_key: optional_env("ECONOMICS_CENSUS_API_KEY").is_some(),
        eia_api_key: optional_env("ECONOMICS_EIA_API_KEY").is_some(),
        coingecko_api_key: optional_env("ECONOMICS_COINGECKO_API_KEY").is_some(),
        sec_api_key: optional_env("ECONOMICS_SEC_API_KEY").is_some(),
        crunchbase_api_key: optional_env("ECONOMICS_CRUNCHBASE_API_KEY").is_some(),
        pitchbook_api_key: optional_env("ECONOMICS_PITCHBOOK_API_KEY").is_some(),
        cb_insights_api_key: optional_env("ECONOMICS_CB_INSIGHTS_API_KEY").is_some(),
        dealroom_api_key: optional_env("ECONOMICS_DEALROOM_API_KEY").is_some(),
        preqin_api_key: optional_env("ECONOMICS_PREQIN_API_KEY").is_some(),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_unix_nano_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}

fn severity_number(severity_text: &str) -> u8 {
    match severity_text {
        "TRACE" => 1,
        "DEBUG" => 5,
        "INFO" => 9,
        "WARN" => 13,
        "ERROR" => 17,
        _ => 9,
    }
}

fn telemetry_log_record(
    severity_text: &str,
    event_name: &str,
    body: &str,
    attributes: Value,
) -> Value {
    json!({
        "schema": "dd.log.v1",
        "time_unix_nano": now_unix_nano_string(),
        "severity_text": severity_text,
        "severity_number": severity_number(severity_text),
        "body": body,
        "resource_service_name": SERVICE_NAME,
        "resource_service_namespace": env_value("OTEL_SERVICE_NAMESPACE", "remote-dev"),
        "scope_name": "economics-server",
        "event_name": event_name,
        "attributes": attributes
    })
}

fn emit_log(severity_text: &str, event_name: &str, body: &str, attributes: Value) {
    let record = telemetry_log_record(severity_text, event_name, body, attributes).to_string();
    if severity_number(severity_text) >= 17 {
        eprintln!("{record}");
    } else {
        println!("{record}");
    }
}

fn error_summary(error: &str) -> String {
    error
        .chars()
        .filter(|ch| !ch.is_control())
        .take(256)
        .collect()
}

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max)
}

fn finite_or(value: Option<f64>, fallback: f64) -> f64 {
    value
        .filter(|number| number.is_finite())
        .unwrap_or(fallback)
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

fn clean_token(value: &str, label: &str) -> Result<String, String> {
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

fn clean_optional_token(value: &Option<String>, label: &str) -> Result<(), String> {
    if let Some(value) = value.as_deref() {
        clean_token(value, label)?;
    }
    Ok(())
}

fn validate_source_auth_env(config: &Config, env_name: &str) -> Result<String, String> {
    let clean = clean_token(env_name, "authHeaderEnv")?;
    let normalized = clean.to_ascii_lowercase();
    if config
        .allowed_source_auth_envs
        .iter()
        .any(|allowed| allowed == &normalized)
    {
        return Ok(clean);
    }
    Err(
        "authHeaderEnv must be listed in ECONOMICS_ALLOWED_SOURCE_AUTH_ENVS or one of the built-in ECONOMICS_* credential placeholders"
            .to_string(),
    )
}

fn validate_source_auth_header_name(name: &str) -> Result<reqwest::header::HeaderName, String> {
    let clean = clean_token(name, "authHeaderName")?;
    let lower = clean.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "host"
            | "connection"
            | "content-length"
            | "transfer-encoding"
            | "cookie"
            | "set-cookie"
            | "proxy-authorization"
            | "upgrade"
    ) {
        return Err(
            "authHeaderName cannot be a hop-by-hop, cookie, host, or payload framing header"
                .to_string(),
        );
    }
    clean
        .parse::<reqwest::header::HeaderName>()
        .map_err(|error| format!("authHeaderName is invalid: {error}"))
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let max_len = left.len().max(right.len());
    let mut diff = left.len() ^ right.len();
    for index in 0..max_len {
        let l = left.get(index).copied().unwrap_or(0);
        let r = right.get(index).copied().unwrap_or(0);
        diff |= usize::from(l ^ r);
    }
    diff == 0
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
        .or_else(|| headers.get("authorization"))
        .and_then(|value| value.to_str().ok());
    match provided {
        Some(value) if constant_time_eq(value.trim_start_matches("Bearer ").trim(), secret) => {
            Ok(())
        }
        _ => Err(AuthFailure::Unauthorized),
    }
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
    emit_log(
        "WARN",
        "economics.auth.failure",
        "economics request authentication failed",
        json!({
            "failure": message,
            "authConfigured": state.config.server_auth_secret.is_some(),
            "allowUnauthenticated": state.config.allow_unauthenticated
        }),
    );
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "ok": false, "error": message })),
    )
        .into_response()
}

fn des_surface_descriptor() -> Value {
    let surface = des_engine::sdk::surface();
    json!({
        "crate": surface.crate_name,
        "version": surface.version,
        "modules": surface.modules,
        "path": "remote/submodules/discrete-event-system.rs",
        "usage": "Forecast service embeds the DES SDK surface for acausal equations, MDP/POMDP, optimization, simulation, and service discovery."
    })
}

fn des_service_descriptor() -> Value {
    let mut builder = ServiceBuilder::new(ServiceInfo {
        name: SERVICE_NAME.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        description: "Economics dashboard and theory/data forecast service backed by des_engine."
            .to_string(),
    });
    builder
        .endpoint("GET", "/", "Dashboard shell.", EndpointKind::Service)
        .endpoint(
            "GET",
            "/dashboard.json",
            "Dashboard data and projections.",
            EndpointKind::Action,
        )
        .endpoint(
            "POST",
            "/forecast",
            "Run an economics forecast.",
            EndpointKind::Action,
        )
        .endpoint(
            "POST",
            "/ingest",
            "Ingest normalized market history.",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/sources/public",
            "Known public data source templates with parsers and documentation links.",
            EndpointKind::Service,
        )
        .endpoint(
            "POST",
            "/sources/pull",
            "Fetch sourceId templates or bounded custom market history from an approved API URL.",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/sentiment/sources",
            "Social/news sentiment provider catalog and credential status.",
            EndpointKind::Service,
        )
        .endpoint(
            "POST",
            "/sentiment/analyze",
            "Analyze supplied social/news text snippets for market sentiment.",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/macro/indicators",
            "Fiscal, GDP, debt, spending, borrowing, and labor indicator context.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/vc/investment",
            "Venture-capital firm, deal, sector-flow, and credential placeholder context.",
            EndpointKind::Service,
        )
        .endpoint(
            "POST",
            "/recommendations",
            "Rank top company and commodity buy/sell-or-dump candidates.",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/audit/hardening",
            "Runtime hardening posture, bounds, and residual-risk audit.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/pipelines/catalog",
            "Spark, Airflow, Databricks, data lake, and NATS pipeline integration catalog.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/observability",
            "Prometheus, Loki, Grafana, and explicit-only OTel telemetry posture.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/integrations/health",
            "Redacted readiness and degradation status for economics integrations.",
            EndpointKind::Service,
        )
        .endpoint(
            "POST",
            "/pipelines/plan",
            "Create redacted big-data pipeline job intents for economics refresh work.",
            EndpointKind::Action,
        )
        .endpoint(
            "POST",
            "/pipelines/submit",
            "Submit eligible job intents to the internal Spark pipeline server when enabled.",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/model/equations",
            "Equation and theory catalogue.",
            EndpointKind::Service,
        );
    serde_json::to_value(builder.build()).unwrap_or_else(|_| json!({}))
}

fn equation_catalog() -> Vec<EquationDescriptor> {
    vec![
        EquationDescriptor {
            name: "Geometric Brownian Motion",
            family: "stochastic-asset-pricing",
            equation: "dS/S = mu dt + sigma dW",
            use_case: "Baseline traded-asset projection and log-normal confidence intervals.",
            caveat: "Useful for liquid prices, but fat tails and regime changes require stress overlays.",
        },
        EquationDescriptor {
            name: "Ornstein-Uhlenbeck Mean Reversion",
            family: "stochastic-rates-spreads",
            equation: "dX = theta(m - X)dt + sigma dW",
            use_case: "Rates, spreads, valuation gaps, and commodity inventory/carry deviations.",
            caveat: "Mean level and speed are inferred from data and should be re-estimated by regime.",
        },
        EquationDescriptor {
            name: "Hotelling Rule With Carry",
            family: "commodity-economics",
            equation: "E[dP/P] ~= r + storage_cost - convenience_yield + demand_growth - supply_growth",
            use_case: "Oil, metals, and other storable commodities with inventory and convenience yield.",
            caveat: "Short-run supply shocks can dominate the smooth scarcity path.",
        },
        EquationDescriptor {
            name: "CAPM Expected Return",
            family: "asset-pricing",
            equation: "E[R_i] = R_f + beta_i(E[R_m] - R_f)",
            use_case: "Equity and risk-asset prior when market return and beta are known.",
            caveat: "Single-factor CAPM is a prior, not a complete trading model.",
        },
        EquationDescriptor {
            name: "Fisher Equation",
            family: "rates-inflation",
            equation: "i ~= r + pi_e",
            use_case: "Separates nominal rates into real-rate and expected-inflation components.",
            caveat: "Risk premia and term premia make observed yields richer than the identity.",
        },
        EquationDescriptor {
            name: "Taylor Rule",
            family: "monetary-policy",
            equation: "i = r* + pi + 0.5(pi - pi*) + 0.5 y_gap",
            use_case: "Measures policy tightness versus inflation and output-gap conditions.",
            caveat: "Central banks react to financial stability and politics outside this simple rule.",
        },
        EquationDescriptor {
            name: "Quantity Theory Growth Form",
            family: "monetary-macro",
            equation: "money_growth + velocity_growth ~= inflation + real_growth",
            use_case: "Liquidity impulse for crypto, gold, equities, and broad nominal assets.",
            caveat: "Velocity is unstable, especially around crises and payment-regime shifts.",
        },
        EquationDescriptor {
            name: "Phillips Curve",
            family: "labor-inflation",
            equation: "pi = pi_e - alpha unemployment_gap + supply_shock",
            use_case: "Inflation pressure from labor slack and supply shocks.",
            caveat: "Slope changes over time; use as a weak prior.",
        },
        EquationDescriptor {
            name: "Uncovered Interest Parity",
            family: "foreign-exchange",
            equation: "E[Delta s] ~= i_domestic - i_foreign",
            use_case: "FX drift prior from interest-rate differentials.",
            caveat: "Carry premia and funding stress often violate UIP in tradeable horizons.",
        },
        EquationDescriptor {
            name: "Purchasing Power Parity",
            family: "foreign-exchange",
            equation: "Delta s ~= inflation_domestic - inflation_foreign",
            use_case: "Long-run currency valuation anchor.",
            caveat: "Slow-moving; tariffs, terms of trade, and capital controls matter.",
        },
        EquationDescriptor {
            name: "Expectations Hypothesis Of The Term Structure",
            family: "bonds",
            equation: "long_yield ~= average expected short_rates + term_premium",
            use_case: "Bond price sensitivity to expected policy-rate paths.",
            caveat: "Term premium is time-varying and can dominate forecast errors.",
        },
        EquationDescriptor {
            name: "Supply-Demand Elasticity",
            family: "micro-commodity",
            equation: "Delta P/P ~= (Delta D/D - Delta S/S) / (epsilon_s + abs(epsilon_d))",
            use_case: "Commodity and housing pressure from demand/supply imbalance.",
            caveat: "Elasticities differ sharply by market and horizon.",
        },
        EquationDescriptor {
            name: "Solow Growth Transition",
            family: "macro-growth",
            equation: "dk/dt = s f(k) - (delta + n + g)k",
            use_case: "Slow-moving real growth anchor for macro scenarios.",
            caveat: "Not a short-term trading equation; it anchors regime assumptions.",
        },
        EquationDescriptor {
            name: "Logistic Adoption Diffusion",
            family: "market-discovery",
            equation: "dA/dt = r A(1 - A/K)",
            use_case: "Adoption curves for crypto networks, new commodities, and emerging markets.",
            caveat: "Carrying capacity K is the fragile assumption.",
        },
    ]
}

fn source_catalog() -> Vec<SourceDescriptor> {
    vec![
        SourceDescriptor {
            id: "fred",
            name: "Federal Reserve Economic Data",
            asset_classes: &["rates", "macro", "housing", "money-market", "commodities"],
            auth: "optional API key",
            notes: "Policy rates, CPI/PCE, yield curves, M2, housing, commodity benchmarks.",
        },
        SourceDescriptor {
            id: "treasury",
            name: "US Treasury FiscalData and yield feeds",
            asset_classes: &["bonds", "rates", "money-market"],
            auth: "public",
            notes: "Treasury yield curve, bills, notes, auction and debt datasets.",
        },
        SourceDescriptor {
            id: "bls-bea-census",
            name: "BLS, BEA, Census",
            asset_classes: &["macro", "labor", "real-estate", "trade"],
            auth: "public or optional key",
            notes: "Employment, CPI/PPI, GDP, income, construction, trade, and housing series.",
        },
        SourceDescriptor {
            id: "fiscal-labor",
            name: "Treasury FiscalData, BEA, BLS, CBO, OECD fiscal/labor feeds",
            asset_classes: &["macro", "fiscal", "labor", "debt", "spending", "gdp"],
            auth: "public or optional ECONOMICS_FRED_API_KEY / BEA / BLS placeholders",
            notes: "National borrowing, outlays, receipts, deficits, debt-to-GDP, GDP growth, labor participation, payrolls, wages, and productivity.",
        },
        SourceDescriptor {
            id: "vc-private-markets",
            name: "Crunchbase, PitchBook, CB Insights, Dealroom, Preqin, SEC filings",
            asset_classes: &["venture-capital", "private-markets", "equities", "securities"],
            auth: "ECONOMICS_CRUNCHBASE_API_KEY, ECONOMICS_PITCHBOOK_API_KEY, ECONOMICS_CB_INSIGHTS_API_KEY, ECONOMICS_DEALROOM_API_KEY, ECONOMICS_PREQIN_API_KEY",
            notes: "VC firm investment, deal velocity, sector flow, dry powder, late-stage marks, exit liquidity, and private-to-public market read-throughs.",
        },
        SourceDescriptor {
            id: "eia-opec",
            name: "EIA, OPEC, IEA-style energy feeds",
            asset_classes: &["oil", "energy", "commodities"],
            auth: "public or private key",
            notes: "Crude, refined products, storage, production, consumption, and flows.",
        },
        SourceDescriptor {
            id: "metals",
            name: "LBMA, CME, exchange and vendor metals feeds",
            asset_classes: &["gold", "silver", "metals", "commodities"],
            auth: "public/private",
            notes: "Spot/futures curves, vault/inventory data, lease/carry proxies.",
        },
        SourceDescriptor {
            id: "crypto",
            name: "CoinGecko, Coinbase, Kraken, Binance US",
            asset_classes: &["crypto", "fx"],
            auth: "public 365-day CoinGecko window or ECONOMICS_COINGECKO_API_KEY/private exchange keys",
            notes: "Spot, order-book, volume, market-cap, funding, and exchange metadata.",
        },
        SourceDescriptor {
            id: "x-twitter",
            name: "X / Twitter API",
            asset_classes: &[
                "sentiment",
                "equities",
                "crypto",
                "commodities",
                "forex",
                "macro",
            ],
            auth: "ECONOMICS_X_BEARER_TOKEN or OAuth 1.0a key/secret placeholders",
            notes: "Market chatter, breaking-news velocity, cashtag/hashtag momentum, influencer and source clustering.",
        },
        SourceDescriptor {
            id: "reddit",
            name: "Reddit API",
            asset_classes: &[
                "sentiment",
                "equities",
                "crypto",
                "commodities",
                "real-estate",
                "macro",
            ],
            auth: "ECONOMICS_REDDIT_CLIENT_ID, ECONOMICS_REDDIT_CLIENT_SECRET, ECONOMICS_REDDIT_USER_AGENT",
            notes: "Subreddit discussion, retail crowd attention, ticker mentions, local real-estate chatter, and topic shifts.",
        },
        SourceDescriptor {
            id: "news-social",
            name: "NewsAPI, RSS, GDELT, Stocktwits, forums",
            asset_classes: &[
                "sentiment",
                "news",
                "equities",
                "crypto",
                "commodities",
                "forex",
                "macro",
            ],
            auth: "ECONOMICS_NEWS_API_KEY, ECONOMICS_GDELT_API_KEY, ECONOMICS_STOCKTWITS_TOKEN",
            notes: "Public/private news and social streams for narrative, event, and entity-level sentiment features.",
        },
        SourceDescriptor {
            id: "equities",
            name: "Polygon, Alpaca, IEX, Nasdaq Data Link, Stooq",
            asset_classes: &["equities", "securities", "etf", "indices"],
            auth: "public/private",
            notes: "OHLCV, corporate actions, indices, sectors, and securities metadata.",
        },
        SourceDescriptor {
            id: "forex",
            name: "ECB, BIS, broker FX APIs",
            asset_classes: &["forex", "currency", "rates"],
            auth: "public/private",
            notes: "Exchange rates, effective exchange rates, forwards, and carry data.",
        },
        SourceDescriptor {
            id: "global-macro",
            name: "World Bank, IMF, OECD, WTO",
            asset_classes: &["macro", "trade", "currency", "country-risk"],
            auth: "public/private",
            notes:
                "Country macro, trade, debt, inflation, productivity, and balance-of-payments data.",
        },
        SourceDescriptor {
            id: "real-estate",
            name: "FHFA, Case-Shiller, Census, private property feeds",
            asset_classes: &["real-estate", "housing", "credit"],
            auth: "public/private",
            notes:
                "Prices, rents, permits, starts, inventory, mortgage rates, and regional supply.",
        },
    ]
}

fn public_source_templates() -> Vec<PublicSourceTemplate> {
    vec![
        PublicSourceTemplate {
            id: "treasury-debt-to-penny",
            provider: "US Treasury FiscalData",
            name: "US total public debt outstanding",
            asset_class: "debt",
            instrument_id: "US-PUBLIC-DEBT",
            display_name: "US Total Public Debt Outstanding",
            currency: "USD",
            source: "treasury-fiscaldata",
            url: "https://api.fiscaldata.treasury.gov/services/api/fiscal_service/v2/accounting/od/debt_to_penny?fields=record_date,tot_pub_debt_out_amt&filter=record_date:gte:2011-01-01&sort=record_date&page%5Bsize%5D=8000",
            host: "api.fiscaldata.treasury.gov",
            parser: SourceParser::JsonRecords,
            root_pointer: Some("/data"),
            date_field: Some("record_date"),
            price_field: Some("tot_pub_debt_out_amt"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "business-daily",
            documentation_url: "https://fiscaldata.treasury.gov/datasets/debt-to-the-penny/",
            notes: "Official Treasury borrowing series for national-debt context.",
        },
        PublicSourceTemplate {
            id: "worldbank-us-gdp-current-usd",
            provider: "World Bank Indicators API",
            name: "US GDP current USD",
            asset_class: "macro",
            instrument_id: "US-GDP-CURRENT-USD",
            display_name: "US GDP Current USD",
            currency: "USD",
            source: "worldbank",
            url: "https://api.worldbank.org/v2/country/US/indicator/NY.GDP.MKTP.CD?format=json&per_page=70",
            host: "api.worldbank.org",
            parser: SourceParser::JsonRecords,
            root_pointer: Some("/1"),
            date_field: Some("date"),
            price_field: Some("value"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "annual",
            documentation_url: "https://datahelpdesk.worldbank.org/knowledgebase/articles/889392-about-the-indicators-api-documentation",
            notes: "Public GDP anchor for macro/fiscal projections.",
        },
        PublicSourceTemplate {
            id: "worldbank-us-labor-participation",
            provider: "World Bank Indicators API",
            name: "US labor force participation rate",
            asset_class: "labor",
            instrument_id: "US-LABOR-PARTICIPATION",
            display_name: "US Labor Force Participation Rate",
            currency: "PCT",
            source: "worldbank",
            url: "https://api.worldbank.org/v2/country/US/indicator/SL.TLF.CACT.ZS?format=json&per_page=70",
            host: "api.worldbank.org",
            parser: SourceParser::JsonRecords,
            root_pointer: Some("/1"),
            date_field: Some("date"),
            price_field: Some("value"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "annual",
            documentation_url: "https://datahelpdesk.worldbank.org/knowledgebase/articles/889392-about-the-indicators-api-documentation",
            notes: "Public workforce participation proxy for labor pressure.",
        },
        PublicSourceTemplate {
            id: "coingecko-bitcoin-usd",
            provider: "CoinGecko API",
            name: "Bitcoin market chart USD",
            asset_class: "crypto",
            instrument_id: "BTC-USD",
            display_name: "Bitcoin USD",
            currency: "USD",
            source: "coingecko",
            url: "https://api.coingecko.com/api/v3/coins/bitcoin/market_chart?vs_currency=usd&days=365&interval=daily",
            host: "api.coingecko.com",
            parser: SourceParser::JsonTupleArray,
            root_pointer: Some("/prices"),
            date_field: None,
            price_field: None,
            volume_field: None,
            date_index: Some(0),
            price_index: Some(1),
            volume_index: None,
            cadence: "daily",
            documentation_url: "https://docs.coingecko.com/reference/endpoint-overview",
            notes: "Public unauthenticated crypto history is provider-limited to the past 365 days; longer windows require a provider key or private market-data feed.",
        },
        PublicSourceTemplate {
            id: "coingecko-ethereum-usd",
            provider: "CoinGecko API",
            name: "Ethereum market chart USD",
            asset_class: "crypto",
            instrument_id: "ETH-USD",
            display_name: "Ethereum USD",
            currency: "USD",
            source: "coingecko",
            url: "https://api.coingecko.com/api/v3/coins/ethereum/market_chart?vs_currency=usd&days=365&interval=daily",
            host: "api.coingecko.com",
            parser: SourceParser::JsonTupleArray,
            root_pointer: Some("/prices"),
            date_field: None,
            price_field: None,
            volume_field: None,
            date_index: Some(0),
            price_index: Some(1),
            volume_index: None,
            cadence: "daily",
            documentation_url: "https://docs.coingecko.com/reference/endpoint-overview",
            notes: "Public unauthenticated crypto history is provider-limited to the past 365 days; longer windows require a provider key or private market-data feed.",
        },
        PublicSourceTemplate {
            id: "fred-dgs10",
            provider: "Federal Reserve Economic Data",
            name: "10-year Treasury constant maturity rate",
            asset_class: "rates",
            instrument_id: "DGS10",
            display_name: "10-Year Treasury Constant Maturity Rate",
            currency: "PCT",
            source: "fred-public-csv",
            url: "https://fred.stlouisfed.org/graph/fredgraph.csv?id=DGS10&cosd=2011-01-01",
            host: "fred.stlouisfed.org",
            parser: SourceParser::CsvRecords,
            root_pointer: None,
            date_field: Some("observation_date"),
            price_field: Some("DGS10"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "business-daily",
            documentation_url: "https://fred.stlouisfed.org/series/DGS10",
            notes: "Public rate anchor for duration, discount-rate, and dollar-pressure features.",
        },
        PublicSourceTemplate {
            id: "fred-wti-oil",
            provider: "Federal Reserve Economic Data",
            name: "WTI crude oil spot price",
            asset_class: "oil",
            instrument_id: "DCOILWTICO",
            display_name: "WTI Crude Oil Spot Price",
            currency: "USD",
            source: "fred-public-csv",
            url: "https://fred.stlouisfed.org/graph/fredgraph.csv?id=DCOILWTICO&cosd=2011-01-01",
            host: "fred.stlouisfed.org",
            parser: SourceParser::CsvRecords,
            root_pointer: None,
            date_field: Some("observation_date"),
            price_field: Some("DCOILWTICO"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "business-daily",
            documentation_url: "https://fred.stlouisfed.org/series/DCOILWTICO",
            notes: "Public oil benchmark for commodity and inflation scenarios.",
        },
        PublicSourceTemplate {
            id: "fred-gold",
            provider: "Federal Reserve Economic Data",
            name: "Gold fixing price USD",
            asset_class: "gold",
            instrument_id: "GOLDAMGBD228NLBM",
            display_name: "Gold Fixing Price USD",
            currency: "USD",
            source: "fred-public-csv",
            url: "https://fred.stlouisfed.org/graph/fredgraph.csv?id=GOLDAMGBD228NLBM&cosd=2011-01-01",
            host: "fred.stlouisfed.org",
            parser: SourceParser::CsvRecords,
            root_pointer: None,
            date_field: Some("observation_date"),
            price_field: Some("GOLDAMGBD228NLBM"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "business-daily",
            documentation_url: "https://fred.stlouisfed.org/series/GOLDAMGBD228NLBM",
            notes: "Public precious-metals benchmark for real-rate and safe-haven modeling.",
        },
        PublicSourceTemplate {
            id: "fred-silver",
            provider: "Federal Reserve Economic Data",
            name: "Silver price USD",
            asset_class: "silver",
            instrument_id: "SLVPRUSD",
            display_name: "Silver Price USD",
            currency: "USD",
            source: "fred-public-csv",
            url: "https://fred.stlouisfed.org/graph/fredgraph.csv?id=SLVPRUSD&cosd=2011-01-01",
            host: "fred.stlouisfed.org",
            parser: SourceParser::CsvRecords,
            root_pointer: None,
            date_field: Some("observation_date"),
            price_field: Some("SLVPRUSD"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "business-daily",
            documentation_url: "https://fred.stlouisfed.org/series/SLVPRUSD",
            notes: "Public silver benchmark for industrial and precious-metals modeling.",
        },
        PublicSourceTemplate {
            id: "fred-sp500",
            provider: "Federal Reserve Economic Data",
            name: "S&P 500 index",
            asset_class: "equities",
            instrument_id: "SP500",
            display_name: "S&P 500 Index",
            currency: "USD",
            source: "fred-public-csv",
            url: "https://fred.stlouisfed.org/graph/fredgraph.csv?id=SP500&cosd=2011-01-01",
            host: "fred.stlouisfed.org",
            parser: SourceParser::CsvRecords,
            root_pointer: None,
            date_field: Some("observation_date"),
            price_field: Some("SP500"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "business-daily",
            documentation_url: "https://fred.stlouisfed.org/series/SP500",
            notes: "Public equity benchmark for broad market momentum and risk-premium context.",
        },
        PublicSourceTemplate {
            id: "fred-mortgage30",
            provider: "Federal Reserve Economic Data",
            name: "30-year fixed mortgage average",
            asset_class: "real-estate",
            instrument_id: "MORTGAGE30US",
            display_name: "30-Year Fixed Rate Mortgage Average",
            currency: "PCT",
            source: "fred-public-csv",
            url: "https://fred.stlouisfed.org/graph/fredgraph.csv?id=MORTGAGE30US&cosd=2011-01-01",
            host: "fred.stlouisfed.org",
            parser: SourceParser::CsvRecords,
            root_pointer: None,
            date_field: Some("observation_date"),
            price_field: Some("MORTGAGE30US"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "weekly",
            documentation_url: "https://fred.stlouisfed.org/series/MORTGAGE30US",
            notes: "Public real-estate financing pressure series.",
        },
        PublicSourceTemplate {
            id: "fred-usd-eur",
            provider: "Federal Reserve Economic Data",
            name: "US dollar to euro exchange rate",
            asset_class: "forex",
            instrument_id: "DEXUSEU",
            display_name: "US Dollar to Euro Exchange Rate",
            currency: "USD/EUR",
            source: "fred-public-csv",
            url: "https://fred.stlouisfed.org/graph/fredgraph.csv?id=DEXUSEU&cosd=2011-01-01",
            host: "fred.stlouisfed.org",
            parser: SourceParser::CsvRecords,
            root_pointer: None,
            date_field: Some("observation_date"),
            price_field: Some("DEXUSEU"),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            cadence: "business-daily",
            documentation_url: "https://fred.stlouisfed.org/series/DEXUSEU",
            notes: "Public FX benchmark for dollar-strength and carry scenarios.",
        },
    ]
}

fn public_source_template(id: &str) -> Option<PublicSourceTemplate> {
    public_source_templates()
        .into_iter()
        .find(|template| template.id == id)
}

fn public_source_ids() -> Vec<&'static str> {
    public_source_templates()
        .into_iter()
        .map(|template| template.id)
        .collect()
}

fn public_source_hosts() -> Vec<&'static str> {
    let mut hosts = public_source_templates()
        .into_iter()
        .map(|template| template.host)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    hosts.sort_unstable();
    hosts
}

fn public_source_catalog_payload(config: &Config) -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "sources": public_source_templates(),
        "pullRoute": "POST /sources/pull",
        "usage": {
            "sourceId": "Pass one of these ids to POST /sources/pull with no url to fetch and parse a known public source.",
            "adHoc": "Pass url plus instrumentId, assetClass, parser, and field/index metadata for authenticated custom API pulls."
        },
        "egressPolicy": {
            "privateUrlsAllowed": config.allow_private_source_urls,
            "allowedSourceHosts": config.allowed_source_hosts,
            "knownPublicHosts": public_source_hosts(),
            "redirectFollowing": false
        },
        "atMs": now_ms()
    })
}

fn observability_payload(state: &AppState) -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "service": SERVICE_NAME,
        "prometheus": {
            "metricsRoute": "GET /metrics",
            "contentType": "text/plain; version=0.0.4",
            "scrapePort": 8114,
            "lowCardinalityMetrics": true,
            "counters": [
                "dd_economics_server_http_requests_total",
                "dd_economics_server_forecasts_total",
                "dd_economics_server_ingest_requests_total",
                "dd_economics_server_source_pull_total",
                "dd_economics_server_source_pull_success_total",
                "dd_economics_server_source_pull_failure_total",
                "dd_economics_server_source_pull_bytes_total",
                "dd_economics_server_source_pull_stored_points_total",
                "dd_economics_server_sentiment_requests_total",
                "dd_economics_server_recommendation_requests_total",
                "dd_economics_server_pipeline_plan_requests_total",
                "dd_economics_server_pipeline_submit_requests_total",
                "dd_economics_server_pipeline_publish_attempts_total",
                "dd_economics_server_pipeline_publish_success_total",
                "dd_economics_server_pipeline_publish_failure_total",
                "dd_economics_server_pipeline_submit_success_total",
                "dd_economics_server_pipeline_submit_failure_total",
                "dd_economics_server_integration_health_requests_total",
                "dd_economics_server_observability_requests_total",
                "dd_economics_server_auth_failures_total",
                "dd_economics_server_errors_total",
                "dd_economics_server_nats_messages_total",
                "dd_economics_server_nats_published_total"
            ],
            "gauges": [
                "dd_economics_server_source_pull_last_success_unix_seconds"
            ]
        },
        "loki": {
            "collectionBoundary": "container stdout/stderr through Promtail",
            "structuredLogSchema": "dd.log.v1",
            "eventNames": [
                "economics.server.start",
                "economics.auth.failure",
                "economics.source_pull.ok",
                "economics.source_pull.error",
                "economics.nats.loop.disabled",
                "economics.nats.loop.start",
                "economics.nats.subscribe.error",
                "economics.nats.request.oversize",
                "economics.nats.forecast.error",
                "economics.nats.request.invalid",
                "economics.pipeline.plan.encode.error",
                "economics.pipeline.plan.publish.skipped",
                "economics.pipeline.plan.publish.ok",
                "economics.pipeline.plan.publish.error",
                "economics.pipeline.submit.ok",
                "economics.pipeline.submit.rejected",
                "economics.pipeline.submit.error"
            ],
            "labelGuidance": "Promtail should promote only low-cardinality fields such as schema, severity_text, service, namespace, and app labels."
        },
        "otel": {
            "mode": "explicit-only",
            "autoInstrumentation": false,
            "runtimeMonkeyPatching": false,
            "serviceName": env_value("OTEL_SERVICE_NAME", SERVICE_NAME),
            "serviceNamespace": env_value("OTEL_SERVICE_NAMESPACE", "remote-dev"),
            "resourceAttributesConfigured": optional_env("OTEL_RESOURCE_ATTRIBUTES").is_some(),
            "otlpEndpointConfigured": optional_env("OTEL_EXPORTER_OTLP_ENDPOINT").is_some(),
            "collector": "dd-otel-collector handles explicit OTLP and Prometheus scrape pipelines; this service exposes Prometheus metrics and dd.log.v1 logs without auto-instrumentation."
        },
        "grafana": {
            "dashboardUid": env_value("ECONOMICS_GRAFANA_DASHBOARD_UID", "dd-economics-server"),
            "suggestedPanels": [
                "request, error, and auth-failure rates",
                "source pull success/failure, bytes, stored points, and last success timestamp",
                "forecast/recommendation/pipeline plan/publish/submit rates",
                "integration health request rate and degraded dependency count",
                "Loki dd.log.v1 warning/error stream filtered by resource_service_name",
                "pod readiness/restarts from k8s resource exporter"
            ]
        },
        "runtime": {
            "natsConfigured": state.nats.is_some(),
            "publicSourceTemplateCount": public_source_templates().len(),
            "knownPublicSourceHosts": public_source_hosts(),
            "sourcePullAllowedHosts": state.config.allowed_source_hosts,
            "sourceAuthHeaderEnvAllowlistCount": state.config.allowed_source_auth_envs.len(),
            "integrationHealthRoute": "GET /integrations/health",
            "storedSeries": state.series_store.read().map(|store| store.len()).unwrap_or(0)
        },
        "atMs": now_ms()
    })
}

fn sentiment_source_catalog(credentials: &SentimentCredentialStatus) -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "credentialStatus": credentials,
        "providers": [
            {
                "id": "x-twitter",
                "name": "X / Twitter",
                "credentialEnv": [
                    "ECONOMICS_X_BEARER_TOKEN",
                    "ECONOMICS_X_API_KEY",
                    "ECONOMICS_X_API_SECRET",
                    "ECONOMICS_X_ACCESS_TOKEN",
                    "ECONOMICS_X_ACCESS_TOKEN_SECRET"
                ],
                "configured": credentials.x_bearer_token || (
                    credentials.x_api_key
                        && credentials.x_api_secret
                        && credentials.x_access_token
                        && credentials.x_access_token_secret
                ),
                "signals": ["cashtags", "hashtags", "source velocity", "topic drift", "breaking-news attention"]
            },
            {
                "id": "reddit",
                "name": "Reddit",
                "credentialEnv": [
                    "ECONOMICS_REDDIT_CLIENT_ID",
                    "ECONOMICS_REDDIT_CLIENT_SECRET",
                    "ECONOMICS_REDDIT_USER_AGENT"
                ],
                "configured": credentials.reddit_client_id
                    && credentials.reddit_client_secret
                    && credentials.reddit_user_agent,
                "signals": ["subreddit momentum", "ticker mentions", "retail crowd sentiment", "regional chatter"]
            },
            {
                "id": "newsapi",
                "name": "News API / private news feed",
                "credentialEnv": ["ECONOMICS_NEWS_API_KEY"],
                "configured": credentials.news_api_key,
                "signals": ["entity news tone", "event velocity", "headline surprise"]
            },
            {
                "id": "stocktwits",
                "name": "Stocktwits",
                "credentialEnv": ["ECONOMICS_STOCKTWITS_TOKEN"],
                "configured": credentials.stocktwits_token,
                "signals": ["cashtag stream sentiment", "watcher momentum", "retail alerting"]
            },
            {
                "id": "gdelt",
                "name": "GDELT / open web events",
                "credentialEnv": ["ECONOMICS_GDELT_API_KEY"],
                "configured": credentials.gdelt_api_key,
                "signals": ["global media tone", "country/event intensity", "trade and conflict narratives"]
            }
        ],
        "analyzeRoute": "POST /sentiment/analyze",
        "placeholderMode": "live provider fetchers are not implemented yet; POST supplied documents for bounded keyword sentiment scoring"
    })
}

fn schema_descriptor() -> Value {
    json!({
        "schemaVersion": SCHEMA_VERSION,
        "defaults": {
            "historyYears": DEFAULT_HISTORY_YEARS,
            "projectionMonths": DEFAULT_PROJECTION_MONTHS,
            "confidenceLevel": 0.90
        },
        "request": {
            "series": "Optional array of instrument time series. If omitted, the service uses ingested in-memory series or the built-in demonstration basket.",
            "macroContext": "Optional policy, inflation, growth, liquidity, and rate context.",
            "macroFiscalContext": "Optional country fiscal/labor context: GDP, borrowing, spending, debt, deficits, interest outlays, workforce participation, payrolls, wages, and productivity.",
            "ventureCapitalContext": "Optional private-market context with VC firm deal signals and sector flows.",
            "theoryWeights": "Optional blend weights for data, macro theory, momentum, mean reversion, carry, valuation, and jump stress.",
            "scenario": "base, oil-shock, liquidity-crunch, dollar-strength, deflation, soft-landing, or custom label."
        },
        "response": {
            "projections": "Per-instrument forecasts with monthly expected/lower/upper values.",
            "components": "Model contribution ledger showing data and equation priors.",
            "equations": "Transparent list of the accepted equation families used as priors."
        },
        "sentiment": {
            "sources": "GET /sentiment/sources reports placeholder credential env names and configured status for X/Twitter, Reddit, news, Stocktwits, and GDELT.",
            "analyze": "POST /sentiment/analyze accepts supplied social/news snippets and returns bounded placeholder sentiment scores by source."
        },
        "sources": {
            "catalog": "GET /sources reports broad provider families.",
            "publicTemplates": "GET /sources/public reports sourceId templates for known public APIs and CSV feeds.",
            "pull": "POST /sources/pull accepts authenticated sourceId pulls or bounded ad-hoc API pulls."
        },
        "macro": {
            "indicators": "GET /macro/indicators reports built-in fiscal/labor sample context and public/private credential placeholders."
        },
        "recommendations": {
            "route": "POST /recommendations",
            "companies": "Returns top 20 invest candidates and top 20 dump/hedge candidates from the model universe.",
            "commodities": "Returns top 30 buy candidates and top 30 sell-or-dump candidates from major tradable commodities."
        },
        "pipelines": {
            "catalog": "GET /pipelines/catalog reports Spark, Airflow, Databricks, data lake, and NATS integration status without returning secrets.",
            "integrations": "GET /integrations/health reports redacted ready/degraded/disabled status for auth, egress, source credentials, Spark, Airflow, Databricks, NATS, runtime-config, data lake, and DES dependencies.",
            "plan": "POST /pipelines/plan creates redacted job intents for Spark pipeline server, Spark feature builds, Airflow DAG triggers, Databricks run-now payloads, and NATS public-data pipeline events.",
            "submit": "POST /pipelines/submit submits only spark-pipeline-server intents and only when ECONOMICS_ENABLE_PIPELINE_SUBMIT=true.",
            "audit": "GET /audit/hardening reports auth, request bounds, SSRF controls, secret handling, and residual risks."
        },
        "observability": {
            "route": "GET /observability",
            "prometheus": "GET /metrics exposes low-cardinality counters and gauges.",
            "loki": "stdout/stderr emits compact dd.log.v1 JSON records for Promtail/Loki.",
            "otel": "explicit-only posture; no auto-instrumentation or runtime monkey-patching."
        }
    })
}

fn example_request() -> ForecastRequest {
    ForecastRequest {
        request_id: Some("example-economics-forecast".to_string()),
        schema_version: Some(SCHEMA_VERSION.to_string()),
        horizon_months: Some(DEFAULT_PROJECTION_MONTHS),
        confidence_level: Some(0.90),
        scenario: Some("base".to_string()),
        series: Some(sample_market_series()),
        macro_context: Some(MacroContext {
            policy_rate: Some(0.045),
            foreign_policy_rate: Some(0.025),
            inflation: Some(0.031),
            foreign_inflation: Some(0.021),
            expected_inflation: Some(0.026),
            money_supply_growth: Some(0.045),
            real_growth: Some(0.020),
            output_gap: Some(0.004),
            unemployment_gap: Some(-0.003),
            risk_free_rate: Some(0.040),
            market_return: Some(0.082),
        }),
        macro_fiscal_context: Some(default_macro_fiscal_context()),
        venture_capital_context: Some(sample_venture_capital_context()),
        theory_weights: None,
    }
}

fn service_descriptor(state: &AppState) -> Value {
    let stored_series = state
        .series_store
        .read()
        .map(|store| store.len())
        .unwrap_or(0);
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "defaults": {
            "historyYears": state.config.history_years,
            "projectionMonths": state.config.projection_months,
            "confidenceLevel": state.config.confidence_level
        },
        "authRequired": !state.config.allow_unauthenticated,
        "storedSeries": stored_series,
        "endpoints": {
            "dashboard": "GET /",
            "dashboardJson": "GET /dashboard.json",
            "forecast": "POST /forecast",
            "ingest": "POST /ingest",
            "sources": "GET /sources",
            "publicSources": "GET /sources/public",
            "pullSource": "POST /sources/pull",
            "sentimentSources": "GET /sentiment/sources",
            "sentimentAnalyze": "POST /sentiment/analyze",
            "macroIndicators": "GET /macro/indicators",
            "vcInvestment": "GET /vc/investment",
            "recommendations": "POST /recommendations",
            "auditHardening": "GET /audit/hardening",
            "pipelineCatalog": "GET /pipelines/catalog",
            "pipelinePlan": "POST /pipelines/plan",
            "pipelineSubmit": "POST /pipelines/submit",
            "observability": "GET /observability",
            "integrationHealth": "GET /integrations/health",
            "equations": "GET /model/equations",
            "schema": "GET /schema",
            "example": "GET /example",
            "desEngine": "GET /engine/des",
            "healthz": "GET /healthz",
            "readyz": "GET /readyz",
            "metrics": "GET /metrics"
        },
        "nats": {
            "requestSubject": state.config.request_subject,
            "queueGroup": state.config.queue_group,
            "resultSubject": state.config.result_subject,
            "marketEventSubject": state.config.market_event_subject,
            "runtimeEventSubject": state.config.runtime_event_subject,
            "pipelineIntentSubject": state.config.pipeline_intent_subject
        },
        "desEngine": des_surface_descriptor(),
        "sentiment": {
            "credentialStatus": &state.config.sentiment_credentials,
            "sourcesRoute": "GET /sentiment/sources",
            "analyzeRoute": "POST /sentiment/analyze"
        },
        "marketData": {
            "credentialStatus": &state.config.market_data_credentials,
            "publicSourcesRoute": "GET /sources/public",
            "macroRoute": "GET /macro/indicators",
            "vcRoute": "GET /vc/investment",
            "recommendationsRoute": "POST /recommendations"
        },
        "pipelines": {
            "status": pipeline_integration_status(state),
            "catalogRoute": "GET /pipelines/catalog",
            "planRoute": "POST /pipelines/plan",
            "submitRoute": "POST /pipelines/submit",
            "integrationHealthRoute": "GET /integrations/health",
            "auditRoute": "GET /audit/hardening"
        },
        "integrations": integration_health_payload(state),
        "observability": observability_payload(state),
        "equationCount": equation_catalog().len(),
        "sourceCount": source_catalog().len(),
        "publicSourceTemplateCount": public_source_templates().len(),
        "atMs": now_ms()
    })
}

fn validate_series(series: &[MarketSeries]) -> Result<(), String> {
    if series.is_empty() {
        return Err("series must contain at least one instrument".to_string());
    }
    if series.len() > MAX_SERIES {
        return Err(format!(
            "series must contain at most {MAX_SERIES} instruments"
        ));
    }
    for item in series {
        clean_token(&item.instrument_id, "instrumentId")?;
        clean_token(&item.asset_class, "assetClass")?;
        clean_optional_token(&item.display_name, "displayName")?;
        clean_optional_token(&item.currency, "currency")?;
        clean_optional_token(&item.source, "source")?;
        if item.observations.len() < 2 {
            return Err(format!(
                "series {} must contain at least two observations",
                item.instrument_id
            ));
        }
        if item.observations.len() > MAX_OBSERVATIONS_PER_SERIES {
            return Err(format!(
                "series {} must contain at most {MAX_OBSERVATIONS_PER_SERIES} observations",
                item.instrument_id
            ));
        }
        let mut seen_dates = BTreeSet::new();
        for (index, point) in item.observations.iter().enumerate() {
            clean_token(&point.date, "observation.date")?;
            if !seen_dates.insert(point.date.trim().to_string()) {
                return Err(format!(
                    "series {} observation {index} date is duplicated",
                    item.instrument_id
                ));
            }
            if !point.price.is_finite() || point.price <= 0.0 {
                return Err(format!(
                    "series {} observation {index} price must be finite and positive",
                    item.instrument_id
                ));
            }
            if let Some(volume) = point.volume {
                if !volume.is_finite() || volume < 0.0 {
                    return Err(format!(
                        "series {} observation {index} volume must be finite and non-negative",
                        item.instrument_id
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_optional_number(
    value: Option<f64>,
    label: &str,
    min: f64,
    max: f64,
) -> Result<(), String> {
    if let Some(value) = value {
        if !value.is_finite() || value < min || value > max {
            return Err(format!(
                "{label} must be finite and between {min} and {max}"
            ));
        }
    }
    Ok(())
}

fn validate_macro_context(context: Option<&MacroContext>) -> Result<(), String> {
    let Some(context) = context else {
        return Ok(());
    };
    validate_optional_number(context.policy_rate, "macroContext.policyRate", -0.50, 1.00)?;
    validate_optional_number(
        context.foreign_policy_rate,
        "macroContext.foreignPolicyRate",
        -0.50,
        1.00,
    )?;
    validate_optional_number(context.inflation, "macroContext.inflation", -0.50, 1.00)?;
    validate_optional_number(
        context.foreign_inflation,
        "macroContext.foreignInflation",
        -0.50,
        1.00,
    )?;
    validate_optional_number(
        context.expected_inflation,
        "macroContext.expectedInflation",
        -0.50,
        1.00,
    )?;
    validate_optional_number(
        context.money_supply_growth,
        "macroContext.moneySupplyGrowth",
        -1.00,
        2.00,
    )?;
    validate_optional_number(context.real_growth, "macroContext.realGrowth", -1.00, 2.00)?;
    validate_optional_number(context.output_gap, "macroContext.outputGap", -1.00, 1.00)?;
    validate_optional_number(
        context.unemployment_gap,
        "macroContext.unemploymentGap",
        -1.00,
        1.00,
    )?;
    validate_optional_number(
        context.risk_free_rate,
        "macroContext.riskFreeRate",
        -0.50,
        1.00,
    )?;
    validate_optional_number(
        context.market_return,
        "macroContext.marketReturn",
        -1.00,
        2.00,
    )?;
    Ok(())
}

fn validate_macro_fiscal_context(context: Option<&MacroFiscalContext>) -> Result<(), String> {
    let Some(context) = context else {
        return Ok(());
    };
    clean_optional_token(&context.country, "macroFiscalContext.country")?;
    clean_optional_token(&context.period, "macroFiscalContext.period")?;
    validate_optional_number(context.gdp, "macroFiscalContext.gdp", 1.0, 1.0e17)?;
    validate_optional_number(
        context.gdp_growth,
        "macroFiscalContext.gdpGrowth",
        -1.00,
        2.00,
    )?;
    validate_optional_number(
        context.national_debt,
        "macroFiscalContext.nationalDebt",
        0.0,
        1.0e17,
    )?;
    validate_optional_number(
        context.debt_to_gdp,
        "macroFiscalContext.debtToGdp",
        0.0,
        10.0,
    )?;
    validate_optional_number(
        context.deficit,
        "macroFiscalContext.deficit",
        -1.0e16,
        1.0e16,
    )?;
    validate_optional_number(
        context.deficit_to_gdp,
        "macroFiscalContext.deficitToGdp",
        -2.0,
        2.0,
    )?;
    validate_optional_number(context.receipts, "macroFiscalContext.receipts", 0.0, 1.0e17)?;
    validate_optional_number(context.outlays, "macroFiscalContext.outlays", 0.0, 1.0e17)?;
    validate_optional_number(
        context.borrowing,
        "macroFiscalContext.borrowing",
        0.0,
        1.0e17,
    )?;
    validate_optional_number(
        context.net_interest_outlays,
        "macroFiscalContext.netInterestOutlays",
        0.0,
        1.0e17,
    )?;
    validate_optional_number(
        context.labor_force_participation,
        "macroFiscalContext.laborForceParticipation",
        0.0,
        1.0,
    )?;
    validate_optional_number(
        context.prime_age_participation,
        "macroFiscalContext.primeAgeParticipation",
        0.0,
        1.0,
    )?;
    validate_optional_number(
        context.unemployment_rate,
        "macroFiscalContext.unemploymentRate",
        0.0,
        1.0,
    )?;
    validate_optional_number(
        context.payroll_growth,
        "macroFiscalContext.payrollGrowth",
        -1.0,
        2.0,
    )?;
    validate_optional_number(
        context.wage_growth,
        "macroFiscalContext.wageGrowth",
        -1.0,
        2.0,
    )?;
    validate_optional_number(
        context.productivity_growth,
        "macroFiscalContext.productivityGrowth",
        -1.0,
        2.0,
    )?;
    Ok(())
}

fn validate_venture_capital_context(context: Option<&VentureCapitalContext>) -> Result<(), String> {
    let Some(context) = context else {
        return Ok(());
    };
    clean_optional_token(&context.period, "ventureCapitalContext.period")?;
    if context.deals.len() > MAX_VC_DEALS {
        return Err(format!(
            "ventureCapitalContext.deals must contain at most {MAX_VC_DEALS} items"
        ));
    }
    if context.sector_flows.len() > MAX_VC_SECTOR_FLOWS {
        return Err(format!(
            "ventureCapitalContext.sectorFlows must contain at most {MAX_VC_SECTOR_FLOWS} items"
        ));
    }
    for (index, deal) in context.deals.iter().enumerate() {
        clean_token(&deal.firm, "ventureCapitalContext.deals[].firm")?;
        clean_token(&deal.company, "ventureCapitalContext.deals[].company")?;
        clean_token(&deal.sector, "ventureCapitalContext.deals[].sector")?;
        clean_token(&deal.stage, "ventureCapitalContext.deals[].stage")?;
        clean_optional_token(&deal.currency, "ventureCapitalContext.deals[].currency")?;
        clean_optional_token(&deal.country, "ventureCapitalContext.deals[].country")?;
        clean_optional_token(
            &deal.announced_at,
            "ventureCapitalContext.deals[].announcedAt",
        )?;
        if !deal.amount.is_finite() || deal.amount < 0.0 || deal.amount > 1.0e13 {
            return Err(format!(
                "ventureCapitalContext.deals[{index}].amount must be finite and between 0 and 10000000000000"
            ));
        }
        validate_optional_number(
            deal.confidence,
            "ventureCapitalContext.deals[].confidence",
            0.0,
            1.0,
        )?;
    }
    for flow in &context.sector_flows {
        clean_token(&flow.sector, "ventureCapitalContext.sectorFlows[].sector")?;
        validate_optional_number(
            Some(f64::from(flow.deal_count)),
            "ventureCapitalContext.sectorFlows[].dealCount",
            0.0,
            1_000_000.0,
        )?;
        validate_optional_number(
            Some(flow.invested_capital),
            "ventureCapitalContext.sectorFlows[].investedCapital",
            0.0,
            1.0e15,
        )?;
        validate_optional_number(
            Some(flow.yoy_growth),
            "ventureCapitalContext.sectorFlows[].yoyGrowth",
            -1.0,
            10.0,
        )?;
        validate_optional_number(
            flow.dry_powder,
            "ventureCapitalContext.sectorFlows[].dryPowder",
            0.0,
            1.0e15,
        )?;
        validate_optional_number(
            flow.exit_liquidity,
            "ventureCapitalContext.sectorFlows[].exitLiquidity",
            -1.0,
            10.0,
        )?;
        validate_optional_number(
            flow.confidence,
            "ventureCapitalContext.sectorFlows[].confidence",
            0.0,
            1.0,
        )?;
    }
    Ok(())
}

fn validate_sentiment_context(context: Option<&SentimentSignalContext>) -> Result<(), String> {
    let Some(context) = context else {
        return Ok(());
    };
    validate_optional_number(
        context.average_sentiment,
        "sentimentContext.averageSentiment",
        -1.0,
        1.0,
    )?;
    validate_sentiment_score_map(
        context.instrument_scores.as_ref(),
        "sentimentContext.instrumentScores",
    )?;
    validate_sentiment_score_map(
        context.sector_scores.as_ref(),
        "sentimentContext.sectorScores",
    )?;
    Ok(())
}

fn validate_sentiment_score_map(
    map: Option<&BTreeMap<String, f64>>,
    label: &str,
) -> Result<(), String> {
    let Some(map) = map else {
        return Ok(());
    };
    if map.len() > MAX_SENTIMENT_CONTEXT_SCORES {
        return Err(format!(
            "{label} must contain at most {MAX_SENTIMENT_CONTEXT_SCORES} scores"
        ));
    }
    for (key, value) in map {
        clean_token(key, label)?;
        validate_optional_number(Some(*value), label, -1.0, 1.0)?;
    }
    Ok(())
}

fn snapshot_series_or_sample(state: &AppState) -> Vec<MarketSeries> {
    let stored = state
        .series_store
        .read()
        .map(|store| store.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    if stored.is_empty() {
        sample_market_series()
    } else {
        stored
    }
}

fn forecast_from_request(
    state: &AppState,
    mut request: ForecastRequest,
) -> Result<ForecastResponse, String> {
    if request.series.as_ref().map(Vec::is_empty).unwrap_or(true) {
        request.series = Some(snapshot_series_or_sample(state));
    }
    generate_forecast(&state.config, request)
}

fn generate_forecast(
    config: &Config,
    request: ForecastRequest,
) -> Result<ForecastResponse, String> {
    let request_id = request_id(request.request_id.as_ref(), "economics-forecast");
    if let Some(schema) = request.schema_version.as_deref() {
        if schema != SCHEMA_VERSION {
            return Err(format!("schemaVersion must be {SCHEMA_VERSION}"));
        }
    }
    let horizon_months = request
        .horizon_months
        .unwrap_or(config.projection_months)
        .clamp(1, 120);
    let confidence_level = clamp(
        request.confidence_level.unwrap_or(config.confidence_level),
        0.50,
        0.995,
    );
    let scenario = request
        .scenario
        .unwrap_or_else(|| "base".to_string())
        .trim()
        .to_ascii_lowercase();
    let series = request
        .series
        .ok_or_else(|| "series must be provided or previously ingested".to_string())?;
    validate_macro_context(request.macro_context.as_ref())?;
    validate_macro_fiscal_context(request.macro_fiscal_context.as_ref())?;
    validate_venture_capital_context(request.venture_capital_context.as_ref())?;
    validate_series(&series)?;
    let macro_context = request.macro_context.unwrap_or_default();
    let weights = normalize_weights(request.theory_weights.as_ref());
    let mut projections = Vec::with_capacity(series.len());
    let mut warnings = Vec::new();

    for item in &series {
        let stats = series_stats(item, config.history_years)?;
        let prior = theory_prior(item, &macro_context, &scenario);
        let scenario_adjustment = scenario_adjustment(&item.asset_class, &scenario);
        let drift = weights.data * stats.data_drift
            + weights.macro_theory * prior.drift
            + weights.momentum * stats.momentum
            + weights.mean_reversion * stats.mean_reversion
            + weights.carry * prior.carry
            + weights.valuation * prior.valuation
            - weights.jump_stress * prior.jump_stress
            + scenario_adjustment;
        let class_floor = class_volatility_floor(&item.asset_class);
        let annualized_volatility = stats
            .volatility_per_period
            .mul_add(stats.periods_per_year.sqrt(), 0.0)
            .max(class_floor);
        let points = forecast_points(
            stats.last_price,
            drift,
            annualized_volatility,
            horizon_months,
            confidence_level,
        );
        let terminal = points
            .last()
            .map(|point| point.expected)
            .unwrap_or(stats.last_price);
        let expected_return_18m = terminal / stats.last_price - 1.0;
        let signal = signal_for(expected_return_18m, annualized_volatility);
        let mut rationale = prior.rationale;
        if scenario != "base" {
            rationale.push(format!(
                "scenario `{scenario}` adjustment {:.2}%",
                scenario_adjustment * 100.0
            ));
        }
        let display_name = item
            .display_name
            .clone()
            .unwrap_or_else(|| item.instrument_id.clone());
        let currency = item.currency.clone().unwrap_or_else(|| "USD".to_string());
        projections.push(Projection {
            instrument_id: item.instrument_id.clone(),
            display_name,
            asset_class: item.asset_class.clone(),
            currency,
            last_price: round4(stats.last_price),
            annualized_drift: round6(drift),
            annualized_volatility: round6(annualized_volatility),
            expected_return_18m: round6(expected_return_18m),
            signal,
            rationale,
            components: vec![
                component(
                    "dataDrift",
                    stats.data_drift,
                    weights.data,
                    "mean(log returns) annualized",
                ),
                component(
                    "macroTheory",
                    prior.drift,
                    weights.macro_theory,
                    "CAPM/Taylor/Fisher/UIP/PPP/Hotelling prior by asset class",
                ),
                component(
                    "momentum",
                    stats.momentum,
                    weights.momentum,
                    "recent log return annualized",
                ),
                component(
                    "meanReversion",
                    stats.mean_reversion,
                    weights.mean_reversion,
                    "OU-style pull toward long-run log-price mean",
                ),
                component(
                    "carry",
                    prior.carry,
                    weights.carry,
                    "carry, convenience yield, storage, duration, or rate income",
                ),
                component(
                    "valuation",
                    prior.valuation,
                    weights.valuation,
                    "valuation gap and adoption/saturation pressure",
                ),
                component(
                    "jumpStress",
                    -prior.jump_stress,
                    weights.jump_stress,
                    "fat-tail stress haircut",
                ),
            ],
            points,
        });
    }

    if series
        .iter()
        .any(|item| item.source.as_deref() == Some("built-in-sample"))
    {
        warnings.push(
            "using built-in demonstration data; ingest or pull real API series for live analysis"
                .to_string(),
        );
    }

    Ok(ForecastResponse {
        ok: true,
        request_id,
        schema_version: SCHEMA_VERSION,
        history_years: config.history_years,
        horizon_months,
        confidence_level,
        scenario,
        generated_at_ms: now_ms(),
        des_engine: des_surface_descriptor(),
        equations: equation_catalog(),
        projections,
        warnings,
    })
}

fn component(name: &str, value: f64, weight: f64, equation: &str) -> ModelComponent {
    ModelComponent {
        name: name.to_string(),
        value: round6(value),
        weight: round6(weight),
        equation: equation.to_string(),
    }
}

fn series_stats(series: &MarketSeries, history_years: u32) -> Result<SeriesStats, String> {
    let mut observations = series.observations.clone();
    observations.sort_by(|left, right| left.date.cmp(&right.date));
    let last_price = observations
        .last()
        .ok_or_else(|| "series has no observations".to_string())?
        .price;
    let mut returns = Vec::with_capacity(observations.len().saturating_sub(1));
    for pair in observations.windows(2) {
        let left = pair[0].price;
        let right = pair[1].price;
        if left <= 0.0 || right <= 0.0 {
            return Err(format!(
                "series {} contains non-positive prices",
                series.instrument_id
            ));
        }
        returns.push((right / left).ln());
    }
    let periods_per_year =
        ((returns.len() as f64) / f64::from(history_years.max(1))).clamp(4.0, 252.0);
    let mean_return = mean(&returns);
    let variance = if returns.len() > 1 {
        returns
            .iter()
            .map(|value| {
                let diff = value - mean_return;
                diff * diff
            })
            .sum::<f64>()
            / ((returns.len() - 1) as f64)
    } else {
        0.0
    };
    let recent_count = returns.len().min(periods_per_year.round() as usize).max(1);
    let recent_sum = returns
        .iter()
        .rev()
        .take(recent_count)
        .copied()
        .sum::<f64>();
    let momentum = recent_sum * periods_per_year / recent_count as f64;
    let mean_log_price = mean(
        &observations
            .iter()
            .map(|point| point.price.ln())
            .collect::<Vec<_>>(),
    );
    let mean_reversion = (mean_log_price - last_price.ln()) * 0.25;
    Ok(SeriesStats {
        last_price,
        volatility_per_period: variance.sqrt(),
        periods_per_year,
        data_drift: mean_return * periods_per_year,
        momentum,
        mean_reversion,
    })
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn normalize_weights(input: Option<&TheoryWeights>) -> NormalizedWeights {
    let raw = [
        input.and_then(|w| w.data).unwrap_or(0.42).max(0.0),
        input.and_then(|w| w.macro_theory).unwrap_or(0.28).max(0.0),
        input.and_then(|w| w.momentum).unwrap_or(0.14).max(0.0),
        input
            .and_then(|w| w.mean_reversion)
            .unwrap_or(0.08)
            .max(0.0),
        input.and_then(|w| w.carry).unwrap_or(0.04).max(0.0),
        input.and_then(|w| w.valuation).unwrap_or(0.03).max(0.0),
        input.and_then(|w| w.jump_stress).unwrap_or(0.01).max(0.0),
    ];
    let sum = raw.iter().sum::<f64>().max(f64::EPSILON);
    NormalizedWeights {
        data: raw[0] / sum,
        macro_theory: raw[1] / sum,
        momentum: raw[2] / sum,
        mean_reversion: raw[3] / sum,
        carry: raw[4] / sum,
        valuation: raw[5] / sum,
        jump_stress: raw[6] / sum,
    }
}

fn theory_prior(
    series: &MarketSeries,
    macro_context: &MacroContext,
    scenario: &str,
) -> TheoryPrior {
    let asset_class = series.asset_class.to_ascii_lowercase();
    let features = series.features.clone().unwrap_or_default();
    let policy_rate = macro_context.policy_rate.unwrap_or(0.045);
    let foreign_policy_rate = macro_context.foreign_policy_rate.unwrap_or(0.025);
    let inflation = macro_context.inflation.unwrap_or(0.030);
    let foreign_inflation = macro_context.foreign_inflation.unwrap_or(0.020);
    let expected_inflation = macro_context.expected_inflation.unwrap_or(inflation);
    let money_growth = macro_context.money_supply_growth.unwrap_or(0.045);
    let real_growth = macro_context.real_growth.unwrap_or(0.020);
    let output_gap = macro_context.output_gap.unwrap_or(0.0);
    let unemployment_gap = macro_context.unemployment_gap.unwrap_or(0.0);
    let risk_free_rate = macro_context.risk_free_rate.unwrap_or(policy_rate);
    let market_return = macro_context.market_return.unwrap_or(0.080);
    let neutral_real_rate = 0.010;
    let inflation_target = 0.020;
    let taylor_rate =
        neutral_real_rate + inflation + 0.5 * (inflation - inflation_target) + 0.5 * output_gap;
    let policy_tightness = policy_rate - taylor_rate;
    let real_rate = policy_rate - expected_inflation;
    let liquidity_impulse = (money_growth - real_growth).clamp(-0.15, 0.20);
    let phillips_pressure = (-0.4 * unemployment_gap).clamp(-0.05, 0.05);
    let beta = features.beta.unwrap_or_else(|| default_beta(&asset_class));
    let mut rationale = vec![
        format!("Taylor tightness {:.2}%", policy_tightness * 100.0),
        format!("Fisher real-rate proxy {:.2}%", real_rate * 100.0),
        format!("liquidity impulse {:.2}%", liquidity_impulse * 100.0),
    ];

    let supply_growth = features.supply_growth.unwrap_or(0.020);
    let demand_growth = features.demand_growth.unwrap_or(real_growth + output_gap);
    let storage_cost = features.storage_cost.unwrap_or(0.015);
    let convenience_yield = features.convenience_yield.unwrap_or(0.010);
    let carry = features.carry.unwrap_or(0.0);
    let duration = features.duration.unwrap_or(6.0);
    let valuation_gap = features.valuation_gap.unwrap_or(0.0);

    let (drift, carry_component, valuation_component, jump_stress) = if asset_class
        .contains("equity")
        || asset_class.contains("security")
        || asset_class.contains("index")
        || asset_class.contains("etf")
    {
        let capm = risk_free_rate + beta * (market_return - risk_free_rate);
        rationale.push(format!("CAPM prior {:.2}%", capm * 100.0));
        (
            capm - 0.8 * policy_tightness + 0.2 * liquidity_impulse + 0.2 * output_gap,
            carry,
            -0.25 * valuation_gap,
            0.10,
        )
    } else if asset_class.contains("bond") || asset_class.contains("treasury") {
        let rate_shock = policy_tightness + 0.35 * (inflation - expected_inflation);
        rationale.push(format!("duration {:.1} years", duration));
        (
            risk_free_rate - duration * rate_shock * 0.25,
            risk_free_rate,
            -0.15 * valuation_gap,
            0.04,
        )
    } else if asset_class.contains("money") || asset_class.contains("cash") {
        (policy_rate, policy_rate, 0.0, 0.01)
    } else if asset_class.contains("fx")
        || asset_class.contains("forex")
        || asset_class.contains("currency")
    {
        let uip = policy_rate - foreign_policy_rate;
        let ppp = inflation - foreign_inflation;
        rationale.push(format!(
            "UIP {:.2}% and PPP {:.2}%",
            uip * 100.0,
            ppp * 100.0
        ));
        (
            0.6 * uip + 0.4 * ppp,
            carry + uip,
            -0.1 * valuation_gap,
            0.06,
        )
    } else if asset_class.contains("gold")
        || asset_class.contains("silver")
        || asset_class.contains("precious")
    {
        let inflation_surprise = inflation - expected_inflation;
        (
            -1.2 * real_rate + 0.8 * inflation_surprise + 0.3 * liquidity_impulse,
            carry - storage_cost + convenience_yield,
            -0.20 * valuation_gap,
            0.09,
        )
    } else if asset_class.contains("crypto") {
        (
            0.10 + 1.6 * liquidity_impulse - 1.1 * real_rate + 0.3 * output_gap,
            carry,
            -0.35 * valuation_gap,
            0.25,
        )
    } else if asset_class.contains("real-estate")
        || asset_class.contains("housing")
        || asset_class.contains("property")
    {
        let rent_growth = inflation + real_growth + output_gap;
        (
            rent_growth - 2.4 * real_rate + 0.4 * liquidity_impulse,
            carry,
            -0.30 * valuation_gap,
            0.08,
        )
    } else if asset_class.contains("oil")
        || asset_class.contains("energy")
        || asset_class.contains("commodity")
        || asset_class.contains("metal")
    {
        let elasticity_pressure = (demand_growth - supply_growth) / 0.7;
        let hotelling = policy_rate + storage_cost - convenience_yield + elasticity_pressure;
        rationale.push(format!("Hotelling/carry prior {:.2}%", hotelling * 100.0));
        (
            hotelling + 0.35 * (inflation - inflation_target) + phillips_pressure,
            carry - storage_cost + convenience_yield,
            -0.15 * valuation_gap,
            0.14,
        )
    } else {
        (
            risk_free_rate
                + beta * (market_return - risk_free_rate) * 0.5
                + 0.2 * liquidity_impulse,
            carry,
            -0.20 * valuation_gap,
            0.08,
        )
    };

    let scenario_risk = if matches!(scenario, "liquidity-crunch" | "oil-shock" | "deflation") {
        jump_stress * 1.5
    } else {
        jump_stress
    };

    TheoryPrior {
        drift: clamp(drift, -0.80, 0.80),
        carry: clamp(carry_component, -0.40, 0.40),
        valuation: clamp(valuation_component, -0.40, 0.40),
        jump_stress: scenario_risk,
        rationale,
    }
}

fn default_beta(asset_class: &str) -> f64 {
    if asset_class.contains("crypto") {
        1.8
    } else if asset_class.contains("equity") || asset_class.contains("security") {
        1.0
    } else if asset_class.contains("real-estate") {
        0.7
    } else if asset_class.contains("commodity") || asset_class.contains("oil") {
        0.6
    } else if asset_class.contains("bond") || asset_class.contains("money") {
        0.1
    } else {
        0.5
    }
}

fn class_volatility_floor(asset_class: &str) -> f64 {
    let lower = asset_class.to_ascii_lowercase();
    if lower.contains("crypto") {
        0.55
    } else if lower.contains("oil") || lower.contains("energy") {
        0.32
    } else if lower.contains("equity") || lower.contains("security") || lower.contains("index") {
        0.18
    } else if lower.contains("gold") || lower.contains("silver") || lower.contains("commodity") {
        0.20
    } else if lower.contains("fx") || lower.contains("forex") || lower.contains("currency") {
        0.09
    } else if lower.contains("bond") || lower.contains("treasury") {
        0.08
    } else if lower.contains("real-estate") || lower.contains("housing") {
        0.10
    } else {
        0.12
    }
}

fn scenario_adjustment(asset_class: &str, scenario: &str) -> f64 {
    let lower = asset_class.to_ascii_lowercase();
    match scenario {
        "oil-shock" => {
            if lower.contains("oil") || lower.contains("energy") {
                0.22
            } else if lower.contains("equity") || lower.contains("real-estate") {
                -0.07
            } else if lower.contains("gold") || lower.contains("silver") {
                0.06
            } else {
                -0.02
            }
        }
        "liquidity-crunch" => {
            if lower.contains("crypto") {
                -0.28
            } else if lower.contains("equity") || lower.contains("real-estate") {
                -0.16
            } else if lower.contains("bond") || lower.contains("treasury") {
                0.04
            } else if lower.contains("gold") {
                0.05
            } else {
                -0.05
            }
        }
        "dollar-strength" => {
            if lower.contains("fx") || lower.contains("currency") {
                0.08
            } else if lower.contains("gold")
                || lower.contains("silver")
                || lower.contains("commodity")
            {
                -0.06
            } else {
                -0.02
            }
        }
        "deflation" => {
            if lower.contains("bond") || lower.contains("treasury") {
                0.08
            } else if lower.contains("money") {
                0.02
            } else {
                -0.10
            }
        }
        "soft-landing" => {
            if lower.contains("equity") || lower.contains("real-estate") {
                0.06
            } else if lower.contains("crypto") {
                0.08
            } else {
                0.02
            }
        }
        _ => 0.0,
    }
}

fn forecast_points(
    last_price: f64,
    drift: f64,
    annualized_volatility: f64,
    horizon_months: u32,
    confidence_level: f64,
) -> Vec<ForecastPoint> {
    let z = z_score(confidence_level);
    (1..=horizon_months)
        .map(|month| {
            let t = f64::from(month) / 12.0;
            let expected = last_price * (drift * t).exp();
            let center = (drift - 0.5 * annualized_volatility * annualized_volatility) * t;
            let width = z * annualized_volatility * t.sqrt();
            ForecastPoint {
                month,
                label: format!("M+{month}"),
                expected: round4(expected),
                lower: round4((last_price * (center - width).exp()).max(0.0)),
                upper: round4(last_price * (center + width).exp()),
            }
        })
        .collect()
}

fn z_score(confidence_level: f64) -> f64 {
    if confidence_level >= 0.99 {
        2.576
    } else if confidence_level >= 0.975 {
        2.241
    } else if confidence_level >= 0.95 {
        1.960
    } else if confidence_level >= 0.90 {
        1.645
    } else if confidence_level >= 0.80 {
        1.282
    } else {
        1.000
    }
}

fn signal_for(expected_return: f64, annualized_volatility: f64) -> String {
    let risk_adjusted = expected_return / annualized_volatility.max(0.05);
    if risk_adjusted > 0.65 {
        "accumulate".to_string()
    } else if risk_adjusted > 0.20 {
        "watch-uptrend".to_string()
    } else if risk_adjusted < -0.45 {
        "reduce-or-hedge".to_string()
    } else {
        "neutral".to_string()
    }
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn sample_market_series() -> Vec<MarketSeries> {
    vec![
        synthetic_series("CL=F", "WTI crude oil", "oil", 71.0, 0.015, 0.30, 0.0),
        synthetic_series("GC=F", "Gold spot proxy", "gold", 2050.0, 0.055, 0.18, 1.1),
        synthetic_series(
            "SI=F",
            "Silver spot proxy",
            "silver",
            25.0,
            0.045,
            0.28,
            2.2,
        ),
        synthetic_series("BTC-USD", "Bitcoin", "crypto", 65_000.0, 0.13, 0.65, 3.0),
        synthetic_series("SPY", "US equities", "equities", 520.0, 0.075, 0.18, 4.1),
        synthetic_series(
            "UST10Y",
            "10Y treasury price proxy",
            "bonds",
            98.0,
            0.025,
            0.08,
            5.0,
        ),
        synthetic_series(
            "USD-EUR",
            "USD/EUR FX proxy",
            "forex",
            0.92,
            0.005,
            0.09,
            5.8,
        ),
        synthetic_series(
            "CSHPI",
            "US home price proxy",
            "real-estate",
            310.0,
            0.045,
            0.10,
            6.4,
        ),
        synthetic_series(
            "CORN",
            "Corn commodity proxy",
            "commodity",
            460.0,
            0.020,
            0.24,
            7.0,
        ),
    ]
}

fn synthetic_series(
    instrument_id: &str,
    display_name: &str,
    asset_class: &str,
    terminal_price: f64,
    annual_drift: f64,
    annual_volatility: f64,
    phase: f64,
) -> MarketSeries {
    let months = (DEFAULT_HISTORY_YEARS * 12) as usize;
    let monthly_drift = annual_drift / 12.0;
    let monthly_vol = annual_volatility / 12.0_f64.sqrt();
    let mut log_price = terminal_price.ln() - monthly_drift * months as f64;
    let mut observations = Vec::with_capacity(months + 1);
    for idx in 0..=months {
        let cycle = ((idx as f64 / 9.0) + phase).sin() * monthly_vol * 0.7;
        let regime = ((idx as f64 / 37.0) + phase).cos() * monthly_vol * 0.35;
        if idx > 0 {
            log_price += monthly_drift + cycle + regime;
        }
        observations.push(MarketObservation {
            date: format!("T-{:03}M", months - idx),
            price: round4(log_price.exp()),
            volume: Some(round4(
                1_000_000.0 * (1.0 + ((idx as f64 / 5.0) + phase).sin() * 0.25),
            )),
        });
    }
    let observed_terminal = observations
        .last()
        .map(|point| point.price)
        .unwrap_or(terminal_price);
    let scale = terminal_price / observed_terminal;
    for point in &mut observations {
        point.price = round4(point.price * scale);
    }
    MarketSeries {
        instrument_id: instrument_id.to_string(),
        display_name: Some(display_name.to_string()),
        asset_class: asset_class.to_string(),
        currency: Some("USD".to_string()),
        source: Some("built-in-sample".to_string()),
        observations,
        features: Some(default_features(asset_class)),
    }
}

fn default_features(asset_class: &str) -> AssetFeatures {
    let lower = asset_class.to_ascii_lowercase();
    AssetFeatures {
        beta: Some(default_beta(&lower)),
        duration: if lower.contains("bond") {
            Some(7.0)
        } else {
            None
        },
        carry: if lower.contains("money") {
            Some(0.045)
        } else {
            Some(0.0)
        },
        convenience_yield: if lower.contains("oil") || lower.contains("commodity") {
            Some(0.018)
        } else {
            None
        },
        storage_cost: if lower.contains("oil") || lower.contains("commodity") {
            Some(0.020)
        } else {
            None
        },
        supply_growth: Some(0.020),
        demand_growth: Some(0.026),
        inventory_ratio: None,
        valuation_gap: Some(0.0),
    }
}

fn dashboard_payload(state: &AppState) -> Result<Value, String> {
    let series = snapshot_series_or_sample(state);
    let request = ForecastRequest {
        request_id: Some("dashboard".to_string()),
        schema_version: Some(SCHEMA_VERSION.to_string()),
        horizon_months: Some(state.config.projection_months),
        confidence_level: Some(state.config.confidence_level),
        scenario: Some("base".to_string()),
        series: Some(series.clone()),
        macro_context: None,
        macro_fiscal_context: Some(default_macro_fiscal_context()),
        venture_capital_context: Some(sample_venture_capital_context()),
        theory_weights: None,
    };
    let forecast = generate_forecast(&state.config, request)?;
    let recommendations = generate_recommendations(
        &state.config,
        RecommendationRequest {
            request_id: Some("dashboard-recommendations".to_string()),
            schema_version: Some(SCHEMA_VERSION.to_string()),
            horizon_months: Some(state.config.projection_months),
            company_limit: Some(20),
            commodity_limit: Some(30),
            scenario: Some("base".to_string()),
            series: Some(series.clone()),
            macro_context: None,
            macro_fiscal_context: Some(default_macro_fiscal_context()),
            venture_capital_context: Some(sample_venture_capital_context()),
            sentiment_context: None,
        },
    )?;
    Ok(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "series": series,
        "forecast": forecast,
        "recommendations": recommendations,
        "macroFiscalContext": default_macro_fiscal_context(),
        "ventureCapitalContext": sample_venture_capital_context(),
        "sources": source_catalog(),
        "equations": equation_catalog(),
        "desEngine": des_surface_descriptor(),
        "atMs": now_ms()
    }))
}

fn default_macro_fiscal_context() -> MacroFiscalContext {
    MacroFiscalContext {
        country: Some("US".to_string()),
        period: Some("built-in-demo-current".to_string()),
        gdp: Some(29_000_000_000_000.0),
        gdp_growth: Some(0.021),
        national_debt: Some(36_000_000_000_000.0),
        debt_to_gdp: Some(1.24),
        deficit: Some(1_800_000_000_000.0),
        deficit_to_gdp: Some(0.062),
        receipts: Some(5_000_000_000_000.0),
        outlays: Some(6_800_000_000_000.0),
        borrowing: Some(1_900_000_000_000.0),
        net_interest_outlays: Some(950_000_000_000.0),
        labor_force_participation: Some(0.626),
        prime_age_participation: Some(0.836),
        unemployment_rate: Some(0.040),
        payroll_growth: Some(0.014),
        wage_growth: Some(0.041),
        productivity_growth: Some(0.015),
    }
}

fn sample_venture_capital_context() -> VentureCapitalContext {
    VentureCapitalContext {
        period: Some("built-in-demo-current".to_string()),
        sector_flows: vec![
            VentureSectorFlow {
                sector: "artificial-intelligence".to_string(),
                deal_count: 640,
                invested_capital: 96_000_000_000.0,
                yoy_growth: 0.42,
                dry_powder: Some(120_000_000_000.0),
                exit_liquidity: Some(0.28),
                confidence: Some(0.70),
            },
            VentureSectorFlow {
                sector: "cybersecurity".to_string(),
                deal_count: 310,
                invested_capital: 24_000_000_000.0,
                yoy_growth: 0.18,
                dry_powder: Some(36_000_000_000.0),
                exit_liquidity: Some(0.34),
                confidence: Some(0.64),
            },
            VentureSectorFlow {
                sector: "climate-energy".to_string(),
                deal_count: 420,
                invested_capital: 38_000_000_000.0,
                yoy_growth: 0.12,
                dry_powder: Some(48_000_000_000.0),
                exit_liquidity: Some(0.22),
                confidence: Some(0.60),
            },
            VentureSectorFlow {
                sector: "biotech-healthcare".to_string(),
                deal_count: 520,
                invested_capital: 44_000_000_000.0,
                yoy_growth: 0.06,
                dry_powder: Some(70_000_000_000.0),
                exit_liquidity: Some(0.30),
                confidence: Some(0.62),
            },
            VentureSectorFlow {
                sector: "fintech".to_string(),
                deal_count: 360,
                invested_capital: 28_000_000_000.0,
                yoy_growth: -0.04,
                dry_powder: Some(54_000_000_000.0),
                exit_liquidity: Some(0.18),
                confidence: Some(0.56),
            },
            VentureSectorFlow {
                sector: "industrial-automation".to_string(),
                deal_count: 250,
                invested_capital: 21_000_000_000.0,
                yoy_growth: 0.16,
                dry_powder: Some(29_000_000_000.0),
                exit_liquidity: Some(0.26),
                confidence: Some(0.58),
            },
        ],
        deals: vec![
            VentureCapitalDealSignal {
                firm: "sample-growth-fund".to_string(),
                company: "Anthropic".to_string(),
                sector: "artificial-intelligence".to_string(),
                stage: "late-private".to_string(),
                amount: 4_000_000_000.0,
                currency: Some("USD".to_string()),
                country: Some("US".to_string()),
                announced_at: Some("demo".to_string()),
                confidence: Some(0.58),
            },
            VentureCapitalDealSignal {
                firm: "sample-infrastructure-fund".to_string(),
                company: "Databricks".to_string(),
                sector: "data-infrastructure".to_string(),
                stage: "late-private".to_string(),
                amount: 1_800_000_000.0,
                currency: Some("USD".to_string()),
                country: Some("US".to_string()),
                announced_at: Some("demo".to_string()),
                confidence: Some(0.56),
            },
            VentureCapitalDealSignal {
                firm: "sample-fintech-fund".to_string(),
                company: "Stripe".to_string(),
                sector: "fintech".to_string(),
                stage: "late-private".to_string(),
                amount: 900_000_000.0,
                currency: Some("USD".to_string()),
                country: Some("US".to_string()),
                announced_at: Some("demo".to_string()),
                confidence: Some(0.52),
            },
            VentureCapitalDealSignal {
                firm: "sample-defense-tech-fund".to_string(),
                company: "Anduril".to_string(),
                sector: "defense-industrials".to_string(),
                stage: "late-private".to_string(),
                amount: 1_500_000_000.0,
                currency: Some("USD".to_string()),
                country: Some("US".to_string()),
                announced_at: Some("demo".to_string()),
                confidence: Some(0.54),
            },
            VentureCapitalDealSignal {
                firm: "sample-energy-transition-fund".to_string(),
                company: "Commonwealth Fusion Systems".to_string(),
                sector: "climate-energy".to_string(),
                stage: "growth".to_string(),
                amount: 850_000_000.0,
                currency: Some("USD".to_string()),
                country: Some("US".to_string()),
                announced_at: Some("demo".to_string()),
                confidence: Some(0.50),
            },
            VentureCapitalDealSignal {
                firm: "sample-biotech-fund".to_string(),
                company: "Generate Biomedicines".to_string(),
                sector: "biotech-healthcare".to_string(),
                stage: "growth".to_string(),
                amount: 600_000_000.0,
                currency: Some("USD".to_string()),
                country: Some("US".to_string()),
                announced_at: Some("demo".to_string()),
                confidence: Some(0.48),
            },
        ],
    }
}

fn macro_indicator_payload(config: &Config) -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "macroFiscalContext": default_macro_fiscal_context(),
        "credentialStatus": &config.market_data_credentials,
        "providers": [
            {
                "id": "fred",
                "credentialEnv": ["ECONOMICS_FRED_API_KEY"],
                "signals": ["federal debt", "debt-to-GDP", "rates", "money supply", "labor participation"]
            },
            {
                "id": "bea",
                "credentialEnv": ["ECONOMICS_BEA_API_KEY"],
                "signals": ["GDP", "gross domestic income", "productivity-compatible national accounts"]
            },
            {
                "id": "bls",
                "credentialEnv": ["ECONOMICS_BLS_API_KEY"],
                "signals": ["labor force participation", "unemployment", "payrolls", "wages", "productivity"]
            },
            {
                "id": "treasury-fiscaldata",
                "credentialEnv": ["ECONOMICS_TREASURY_API_KEY"],
                "signals": ["receipts", "outlays", "deficits", "borrowing", "debt outstanding", "interest outlays"]
            },
            {
                "id": "census-eia",
                "credentialEnv": ["ECONOMICS_CENSUS_API_KEY", "ECONOMICS_EIA_API_KEY"],
                "signals": ["trade", "construction", "inventory", "energy supply-demand"]
            }
        ],
        "placeholderMode": "built-in sample context is returned until live provider fetchers are attached"
    })
}

fn vc_investment_payload(config: &Config) -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "ventureCapitalContext": sample_venture_capital_context(),
        "credentialStatus": &config.market_data_credentials,
        "providers": [
            {
                "id": "crunchbase",
                "credentialEnv": ["ECONOMICS_CRUNCHBASE_API_KEY"],
                "signals": ["funding rounds", "company sectors", "investor participation", "stage"]
            },
            {
                "id": "pitchbook",
                "credentialEnv": ["ECONOMICS_PITCHBOOK_API_KEY"],
                "signals": ["VC firm investment", "deal terms", "private valuations", "exit/liquidity"]
            },
            {
                "id": "cb-insights",
                "credentialEnv": ["ECONOMICS_CB_INSIGHTS_API_KEY"],
                "signals": ["sector momentum", "private market narratives", "company tracking"]
            },
            {
                "id": "dealroom-preqin",
                "credentialEnv": ["ECONOMICS_DEALROOM_API_KEY", "ECONOMICS_PREQIN_API_KEY"],
                "signals": ["global private-market flows", "dry powder", "fundraising", "late-stage marks"]
            },
            {
                "id": "sec",
                "credentialEnv": ["ECONOMICS_SEC_API_KEY"],
                "signals": ["D filings", "S-1 filings", "insider and issuer disclosures"]
            }
        ],
        "recommendationsRoute": "POST /recommendations",
        "placeholderMode": "built-in sample VC flow context is returned until live provider fetchers are attached"
    })
}

fn is_cluster_internal_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host.ends_with(".svc.cluster.local")
        || host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
}

fn validate_http_base_url(
    base: &str,
    allow_external: bool,
    label: &str,
) -> Result<reqwest::Url, String> {
    let trimmed = base.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_URL_LEN || trimmed.chars().any(char::is_control) {
        return Err(format!(
            "{label} must be non-empty, contain no control characters, and be at most {MAX_URL_LEN} bytes"
        ));
    }
    let parsed =
        reqwest::Url::parse(trimmed).map_err(|error| format!("{label} is invalid: {error}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err(format!("{label} must use http or https")),
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err(format!("{label} must not contain URL credentials"));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(format!(
            "{label} must not contain query strings or fragments"
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("{label} must include a host"))?;
    if !is_cluster_internal_host(host) && !allow_external {
        return Err(format!(
            "{label} must be cluster-local unless ECONOMICS_ALLOW_EXTERNAL_PIPELINE_URLS=true"
        ));
    }
    Ok(parsed)
}

fn validate_plan_only_http_url(base: &str, label: &str) -> Result<reqwest::Url, String> {
    validate_http_base_url(base, true, label)
}

fn integration_dependency(
    id: &str,
    kind: &str,
    status: &str,
    configured: bool,
    required_for_core_readiness: bool,
    mode: &str,
    details: Value,
    warnings: Vec<String>,
) -> IntegrationDependencyStatus {
    IntegrationDependencyStatus {
        id: id.to_string(),
        kind: kind.to_string(),
        status: status.to_string(),
        configured,
        required_for_core_readiness,
        mode: mode.to_string(),
        details,
        warnings,
    }
}

fn sentiment_credential_count(credentials: &SentimentCredentialStatus) -> usize {
    [
        credentials.x_bearer_token,
        credentials.x_api_key,
        credentials.x_api_secret,
        credentials.x_access_token,
        credentials.x_access_token_secret,
        credentials.reddit_client_id,
        credentials.reddit_client_secret,
        credentials.reddit_user_agent,
        credentials.news_api_key,
        credentials.stocktwits_token,
        credentials.gdelt_api_key,
    ]
    .into_iter()
    .filter(|configured| *configured)
    .count()
}

fn market_data_credential_count(credentials: &MarketDataCredentialStatus) -> usize {
    [
        credentials.fred_api_key,
        credentials.bea_api_key,
        credentials.bls_api_key,
        credentials.treasury_api_key,
        credentials.census_api_key,
        credentials.eia_api_key,
        credentials.coingecko_api_key,
        credentials.sec_api_key,
        credentials.crunchbase_api_key,
        credentials.pitchbook_api_key,
        credentials.cb_insights_api_key,
        credentials.dealroom_api_key,
        credentials.preqin_api_key,
    ]
    .into_iter()
    .filter(|configured| *configured)
    .count()
}

fn integration_dependencies(state: &AppState) -> Vec<IntegrationDependencyStatus> {
    let auth_ready =
        state.config.allow_unauthenticated || state.config.server_auth_secret.is_some();
    let mut dependencies = vec![integration_dependency(
        "server-auth",
        "security",
        if auth_ready { "ready" } else { "not-ready" },
        state.config.server_auth_secret.is_some(),
        true,
        if state.config.allow_unauthenticated {
            "local-unauthenticated"
        } else {
            "shared-secret"
        },
        json!({
            "acceptedHeaders": ["x-server-auth", "auth", "authorization"],
            "allowUnauthenticated": state.config.allow_unauthenticated,
            "secretConfigured": state.config.server_auth_secret.is_some()
        }),
        if state.config.allow_unauthenticated {
            vec![
                "ECONOMICS_ALLOW_UNAUTHENTICATED=true should stay limited to local development"
                    .to_string(),
            ]
        } else {
            Vec::new()
        },
    )];

    let source_warnings = [
        (
            state.config.allow_private_source_urls,
            "private/link-local source URLs are enabled",
        ),
        (
            state.config.allowed_source_hosts.is_empty(),
            "ad-hoc source host allowlist is empty",
        ),
    ]
    .into_iter()
    .filter_map(|(active, warning)| active.then(|| warning.to_string()))
    .collect::<Vec<_>>();
    dependencies.push(integration_dependency(
        "source-egress",
        "data-ingest",
        if source_warnings.is_empty() {
            "ready"
        } else {
            "degraded"
        },
        true,
        false,
        "bounded-http-pull",
        json!({
            "privateUrlsAllowed": state.config.allow_private_source_urls,
            "redirectFollowing": false,
            "allowedSourceHosts": state.config.allowed_source_hosts,
            "knownPublicHosts": public_source_hosts(),
            "maxSourceFetchBytes": MAX_SOURCE_FETCH_BYTES
        }),
        source_warnings,
    ));
    dependencies.push(integration_dependency(
        "source-auth-env-allowlist",
        "secret-boundary",
        if state.config.allowed_source_auth_envs.is_empty() {
            "degraded"
        } else {
            "ready"
        },
        !state.config.allowed_source_auth_envs.is_empty(),
        false,
        "explicit-env-allowlist",
        json!({
            "allowedEnvCount": state.config.allowed_source_auth_envs.len(),
            "allowlistEnv": "ECONOMICS_ALLOWED_SOURCE_AUTH_ENVS",
            "valuesReturned": false
        }),
        Vec::new(),
    ));

    let spark_url_status = state.config.spark_pipeline_url.as_deref().map(|url| {
        validate_http_base_url(
            url,
            state.config.allow_external_pipeline_urls,
            "spark pipeline URL",
        )
    });
    let spark_valid = spark_url_status
        .as_ref()
        .map(Result::is_ok)
        .unwrap_or(false);
    let spark_auth_configured = optional_env(&state.config.spark_pipeline_auth_env).is_some();
    let spark_status = if state.config.allow_pipeline_submit {
        if spark_valid && spark_auth_configured {
            "ready"
        } else {
            "degraded"
        }
    } else if spark_url_status.as_ref().is_some_and(Result::is_err) {
        "misconfigured"
    } else {
        "disabled"
    };
    dependencies.push(integration_dependency(
        "spark-pipeline-server",
        "big-data",
        spark_status,
        state.config.spark_pipeline_url.is_some(),
        false,
        if state.config.allow_pipeline_submit {
            "submit-enabled"
        } else {
            "plan-only"
        },
        json!({
            "urlConfigured": state.config.spark_pipeline_url.is_some(),
            "urlValid": spark_valid,
            "authEnv": state.config.spark_pipeline_auth_env,
            "authConfigured": spark_auth_configured,
            "externalUrlsAllowed": state.config.allow_external_pipeline_urls
        }),
        spark_url_status
            .and_then(Result::err)
            .map(|error| vec![error])
            .unwrap_or_default(),
    ));

    let airflow_status = state
        .config
        .airflow_api_url
        .as_deref()
        .map(|url| validate_plan_only_http_url(url, "Airflow API URL"));
    dependencies.push(integration_dependency(
        "airflow",
        "orchestrator",
        match airflow_status.as_ref() {
            Some(Ok(_)) => "plan-only",
            Some(Err(_)) => "misconfigured",
            None => "disabled",
        },
        state.config.airflow_api_url.is_some(),
        false,
        "plan-only",
        json!({
            "apiUrlConfigured": state.config.airflow_api_url.is_some(),
            "dagBlueprint": "economics_market_refresh",
            "liveSubmissionImplemented": false
        }),
        airflow_status
            .and_then(Result::err)
            .map(|error| vec![error])
            .unwrap_or_default(),
    ));

    let databricks_status = state
        .config
        .databricks_host
        .as_deref()
        .map(|url| validate_plan_only_http_url(url, "Databricks host"));
    let databricks_token_configured = optional_env("ECONOMICS_DATABRICKS_TOKEN").is_some();
    dependencies.push(integration_dependency(
        "databricks",
        "managed-big-data",
        match databricks_status.as_ref() {
            Some(Ok(_)) if databricks_token_configured => "plan-only",
            Some(Ok(_)) => "degraded",
            Some(Err(_)) => "misconfigured",
            None => "disabled",
        },
        state.config.databricks_host.is_some() || databricks_token_configured,
        false,
        "plan-only",
        json!({
            "hostConfigured": state.config.databricks_host.is_some(),
            "tokenConfigured": databricks_token_configured,
            "credentialValuesReturned": false,
            "liveSubmissionImplemented": false
        }),
        databricks_status
            .and_then(Result::err)
            .map(|error| vec![error])
            .unwrap_or_else(|| {
                if state.config.databricks_host.is_some() && !databricks_token_configured {
                    vec!["ECONOMICS_DATABRICKS_TOKEN is not configured".to_string()]
                } else {
                    Vec::new()
                }
            }),
    ));

    let data_lake_valid = validate_data_lake_uri(&state.config.data_lake_uri);
    dependencies.push(integration_dependency(
        "data-lake",
        "storage",
        if data_lake_valid.is_ok() {
            "ready"
        } else {
            "misconfigured"
        },
        true,
        false,
        "pipeline-target",
        json!({
            "uriSchemeAllowed": data_lake_valid.is_ok(),
            "allowedSchemes": ["s3", "s3a", "abfss", "gs", "file:///tmp/"]
        }),
        data_lake_valid
            .err()
            .map(|error| vec![error])
            .unwrap_or_default(),
    ));

    dependencies.push(integration_dependency(
        "nats",
        "messaging",
        if state.nats.is_some() {
            "ready"
        } else {
            "disabled"
        },
        state.nats.is_some(),
        false,
        "forecast-and-pipeline-events",
        json!({
            "forecastRequestSubject": state.config.request_subject,
            "forecastResultSubject": state.config.result_subject,
            "marketEventSubject": state.config.market_event_subject,
            "runtimeEventSubject": state.config.runtime_event_subject,
            "pipelineIntentSubject": state.config.pipeline_intent_subject
        }),
        if state.nats.is_some() {
            Vec::new()
        } else {
            vec!["NATS_URL is not configured or connection was not established".to_string()]
        },
    ));

    let sentiment_count = sentiment_credential_count(&state.config.sentiment_credentials);
    dependencies.push(integration_dependency(
        "sentiment-providers",
        "social-news",
        if sentiment_count > 0 {
            "ready"
        } else {
            "placeholder"
        },
        sentiment_count > 0,
        false,
        "document-analysis-now-live-fetchers-later",
        json!({
            "configuredCredentialCount": sentiment_count,
            "providerCatalogRoute": "GET /sentiment/sources",
            "analyzeRoute": "POST /sentiment/analyze"
        }),
        if sentiment_count > 0 {
            Vec::new()
        } else {
            vec!["live sentiment provider fetchers are placeholders; POST supplied documents for scoring".to_string()]
        },
    ));

    let market_count = market_data_credential_count(&state.config.market_data_credentials);
    dependencies.push(integration_dependency(
        "market-data-providers",
        "market-macro-private-data",
        if market_count > 0 {
            "ready"
        } else {
            "public-only"
        },
        market_count > 0,
        false,
        "source-templates-and-private-credentials",
        json!({
            "configuredCredentialCount": market_count,
            "publicSourceTemplateCount": public_source_templates().len(),
            "publicSourcesRoute": "GET /sources/public"
        }),
        Vec::new(),
    ));

    dependencies.push(integration_dependency(
        "runtime-config",
        "control-plane",
        if optional_env("RUNTIME_CONFIG_REGISTER_URL").is_some() {
            "ready"
        } else {
            "disabled"
        },
        optional_env("RUNTIME_CONFIG_REGISTER_URL").is_some(),
        false,
        "register-and-receive-updates",
        json!({
            "registerUrlConfigured": optional_env("RUNTIME_CONFIG_REGISTER_URL").is_some(),
            "applyRouteConfigured": optional_env("RUNTIME_CONFIG_APPLY_URL").is_some(),
            "scope": env_value("RUNTIME_CONFIG_SCOPE", "default"),
            "env": env_value("RUNTIME_CONFIG_ENV", "stage")
        }),
        Vec::new(),
    ));

    dependencies.push(integration_dependency(
        "des-engine",
        "math-engine",
        "ready",
        true,
        true,
        "embedded-sdk-surface",
        des_surface_descriptor(),
        Vec::new(),
    ));

    dependencies
}

fn integration_health_payload(state: &AppState) -> Value {
    let dependencies = integration_dependencies(state);
    let required_ready = dependencies
        .iter()
        .filter(|dependency| dependency.required_for_core_readiness)
        .all(|dependency| dependency.status == "ready");
    let degraded_count = dependencies
        .iter()
        .filter(|dependency| {
            matches!(
                dependency.status.as_str(),
                "degraded" | "misconfigured" | "not-ready"
            )
        })
        .count();
    let overall_status = if required_ready && degraded_count == 0 {
        "ready"
    } else if required_ready {
        "degraded"
    } else {
        "not-ready"
    };
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "service": SERVICE_NAME,
        "overallStatus": overall_status,
        "coreReady": required_ready,
        "dependencyCount": dependencies.len(),
        "degradedDependencyCount": degraded_count,
        "dependencies": dependencies,
        "telemetry": {
            "metricsRoute": "GET /metrics",
            "observabilityRoute": "GET /observability",
            "integrationHealthRequestsMetric": "dd_economics_server_integration_health_requests_total",
            "structuredLogSchema": "dd.log.v1"
        },
        "atMs": now_ms()
    })
}

fn pipeline_integration_status(state: &AppState) -> PipelineIntegrationStatus {
    PipelineIntegrationStatus {
        spark_pipeline_url_configured: state.config.spark_pipeline_url.is_some(),
        spark_pipeline_auth_configured: optional_env(&state.config.spark_pipeline_auth_env)
            .is_some(),
        spark_pipeline_submit_enabled: state.config.allow_pipeline_submit,
        spark_pipeline_url: state.config.spark_pipeline_url.clone(),
        spark_pipeline_auth_env: state.config.spark_pipeline_auth_env.clone(),
        spark_master_url: state.config.spark_master_url.clone(),
        airflow_api_url_configured: state.config.airflow_api_url.is_some(),
        airflow_api_url: state.config.airflow_api_url.clone(),
        databricks_host_configured: state.config.databricks_host.is_some(),
        databricks_token_configured: optional_env("ECONOMICS_DATABRICKS_TOKEN").is_some(),
        data_lake_uri: state.config.data_lake_uri.clone(),
        pipeline_intent_subject: state.config.pipeline_intent_subject.clone(),
        nats_configured: state.nats.is_some(),
    }
}

fn pipeline_catalog_payload(state: &AppState) -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "status": pipeline_integration_status(state),
        "engines": [
            {
                "id": "spark-pipeline-server",
                "kind": "internal-http",
                "route": "POST /v1/jobs",
                "supportedJobKinds": ["INGEST_VALIDATE_PUBLISH", "SPARK_SUBMIT"],
                "defaultUrl": DEFAULT_SPARK_PIPELINE_URL,
                "submitRoute": "POST /pipelines/submit",
                "submitGate": "ECONOMICS_ENABLE_PIPELINE_SUBMIT must be true and SERVER_AUTH_SECRET must be available"
            },
            {
                "id": "spark-standalone",
                "kind": "spark",
                "master": state.config.spark_master_url,
                "namespace": "big-data",
                "notes": "Development Spark master/worker stack from remote/argocd/big-data."
            },
            {
                "id": "airflow",
                "kind": "orchestrator",
                "apiUrl": state.config.airflow_api_url,
                "dagBlueprint": "economics_market_refresh",
                "notes": "Plan output includes a DAG trigger payload; live Airflow submission is intentionally not implemented until service credentials and API auth are designed."
            },
            {
                "id": "databricks",
                "kind": "managed-external",
                "hostConfigured": state.config.databricks_host.is_some(),
                "tokenConfigured": optional_env("ECONOMICS_DATABRICKS_TOKEN").is_some(),
                "credentialEnv": ["ECONOMICS_DATABRICKS_HOST", "ECONOMICS_DATABRICKS_TOKEN"],
                "notes": "Plan output includes Databricks Jobs API run-now payloads without exposing token values."
            },
            {
                "id": "nats-public-data-pipeline",
                "kind": "nats",
                "subject": state.config.pipeline_intent_subject,
                "defaultSubject": PUBLIC_DATA_PIPELINE_JOBS_SUBJECT,
                "notes": "Pipeline plans can be published as redacted job intents for downstream big-data workers."
            }
        ],
        "integrationHealthRoute": "GET /integrations/health",
        "planRoute": "POST /pipelines/plan",
        "auditRoute": "GET /audit/hardening"
    })
}

fn hardening_audit_payload(state: &AppState) -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "service": SERVICE_NAME,
        "auth": {
            "required": !state.config.allow_unauthenticated,
            "acceptedHeaders": ["x-server-auth", "auth", "authorization"],
            "constantTimeComparison": true,
            "allowUnauthenticated": state.config.allow_unauthenticated
        },
        "requestLimits": {
            "maxHttpBodyBytes": MAX_HTTP_BODY_BYTES,
            "maxNatsPayloadBytes": MAX_NATS_PAYLOAD_BYTES,
            "maxSeries": MAX_SERIES,
            "maxObservationsPerSeries": MAX_OBSERVATIONS_PER_SERIES,
            "maxSentimentDocuments": MAX_SENTIMENT_DOCUMENTS,
            "maxSentimentTextBytes": MAX_SENTIMENT_TEXT_BYTES,
            "maxSentimentContextScores": MAX_SENTIMENT_CONTEXT_SCORES,
            "maxVentureCapitalDeals": MAX_VC_DEALS,
            "maxVentureSectorFlows": MAX_VC_SECTOR_FLOWS,
            "maxPipelineJobIntents": MAX_PIPELINE_JOB_INTENTS
        },
        "egressPolicy": {
            "sourcePullPrivateUrlsAllowed": state.config.allow_private_source_urls,
            "sourcePullAllowedHosts": state.config.allowed_source_hosts,
            "knownPublicSourceHosts": public_source_hosts(),
            "knownPublicSourceTemplates": public_source_templates().len(),
            "sourcePullRedirectFollowing": false,
            "externalPipelineUrlsAllowed": state.config.allow_external_pipeline_urls,
            "sparkPipelineSubmitEnabled": state.config.allow_pipeline_submit,
            "sparkPipelineSubmitRequiresInternalUrl": !state.config.allow_external_pipeline_urls
        },
        "secretHandling": {
            "credentialValuesReturned": false,
            "credentialStatusOnly": true,
            "sourceAuthHeaderEnvAllowlistEnabled": true,
            "sourceAuthHeaderEnvAllowlistCount": state.config.allowed_source_auth_envs.len(),
            "sourceAuthHeaderEnvAllowlistVar": "ECONOMICS_ALLOWED_SOURCE_AUTH_ENVS",
            "sparkPipelineAuthEnv": state.config.spark_pipeline_auth_env,
            "databricksTokenEnv": "ECONOMICS_DATABRICKS_TOKEN"
        },
        "observability": {
            "prometheusMetricsRoute": "GET /metrics",
            "observabilityRoute": "GET /observability",
            "integrationHealthRoute": "GET /integrations/health",
            "structuredLogSchema": "dd.log.v1",
            "lokiCollectionBoundary": "container stdout/stderr via Promtail",
            "otelMode": "explicit-only",
            "autoInstrumentation": false,
            "runtimeMonkeyPatching": false
        },
        "bigData": pipeline_integration_status(state),
        "integrationHealth": integration_health_payload(state),
        "deploymentPosture": {
            "expectedNoServiceAccountToken": true,
            "expectedReadOnlyRootFilesystem": true,
            "expectedDroppedCapabilities": true,
            "expectedRuntimeDefaultSeccomp": true,
            "expectedBoundedWritableVolumes": true
        },
        "residualRisks": [
            "live provider connectors are placeholders until per-provider rate limits, retries, and backoff are implemented",
            "recommendation rankings are research signals, not trade execution instructions",
            "Airflow and Databricks submission remain plan-only until their auth and audit flows are explicitly designed",
            "GET /integrations/health reports integration readiness but does not perform active network probes against external providers",
            "Spark pipeline HTTP submission is disabled unless ECONOMICS_ENABLE_PIPELINE_SUBMIT=true"
        ],
        "atMs": now_ms()
    })
}

fn pipeline_plan_from_request(
    state: &AppState,
    mut request: PipelinePlanRequest,
) -> Result<PipelinePlanResponse, String> {
    if let Some(schema) = request.schema_version.as_deref() {
        if schema != SCHEMA_VERSION {
            return Err(format!("schemaVersion must be {SCHEMA_VERSION}"));
        }
    }
    let request_id = request_id(request.request_id.as_ref(), "economics-pipeline-plan");
    let scenario = request
        .scenario
        .take()
        .unwrap_or_else(|| "base".to_string())
        .trim()
        .to_ascii_lowercase();
    clean_token(&scenario, "scenario")?;
    let data_lake_uri = request
        .data_lake_uri
        .take()
        .unwrap_or_else(|| state.config.data_lake_uri.clone());
    validate_data_lake_uri(&data_lake_uri)?;
    let job_kinds = normalize_pipeline_job_kinds(request.job_kinds.as_ref())?;
    let include_recommendations = request.include_recommendations.unwrap_or(true);
    let recommendation_summary = if include_recommendations {
        let mut recommendation_request =
            request
                .recommendation_request
                .take()
                .unwrap_or_else(|| RecommendationRequest {
                    request_id: Some(format!("{request_id}-recommendations")),
                    schema_version: Some(SCHEMA_VERSION.to_string()),
                    horizon_months: Some(state.config.projection_months),
                    company_limit: Some(20),
                    commodity_limit: Some(30),
                    scenario: Some(scenario.clone()),
                    series: Some(snapshot_series_or_sample(state)),
                    macro_context: None,
                    macro_fiscal_context: Some(default_macro_fiscal_context()),
                    venture_capital_context: Some(sample_venture_capital_context()),
                    sentiment_context: None,
                });
        if recommendation_request
            .series
            .as_ref()
            .map(Vec::is_empty)
            .unwrap_or(true)
        {
            recommendation_request.series = Some(snapshot_series_or_sample(state));
        }
        let recommendations = generate_recommendations(&state.config, recommendation_request)?;
        json!({
            "requestId": recommendations.request_id,
            "companyBuyCount": recommendations.company_buys.len(),
            "companyDumpCount": recommendations.company_dumps.len(),
            "commodityBuyCount": recommendations.commodity_buys.len(),
            "commoditySellOrDumpCount": recommendations.commodity_sells_or_dumps.len(),
            "topCompanyBuys": recommendations.company_buys.iter().take(5).map(|item| json!({
                "ticker": item.ticker,
                "company": item.company,
                "score": item.score,
                "expectedReturn18m": item.expected_return_18m
            })).collect::<Vec<_>>(),
            "topCommodityBuys": recommendations.commodity_buys.iter().take(5).map(|item| json!({
                "instrumentId": item.instrument_id,
                "commodity": item.commodity,
                "score": item.score,
                "expectedReturn18m": item.expected_return_18m
            })).collect::<Vec<_>>()
        })
    } else {
        json!({ "included": false })
    };

    let mut job_intents = Vec::new();
    if job_kinds.iter().any(|kind| kind == "ingest") {
        job_intents.push(spark_ingest_intent(&request_id, &data_lake_uri));
    }
    if job_kinds.iter().any(|kind| kind == "spark-features") {
        job_intents.push(spark_feature_intent(
            &request_id,
            &scenario,
            &data_lake_uri,
            &state.config.spark_master_url,
        ));
    }
    if job_kinds.iter().any(|kind| kind == "airflow") {
        job_intents.push(airflow_dag_intent(
            &request_id,
            &scenario,
            &data_lake_uri,
            state.config.airflow_api_url.as_deref(),
        ));
    }
    if job_kinds.iter().any(|kind| kind == "databricks") {
        job_intents.push(databricks_job_intent(
            &request_id,
            &scenario,
            &data_lake_uri,
            state.config.databricks_host.as_deref(),
        ));
    }
    if job_kinds.iter().any(|kind| kind == "nats") {
        job_intents.push(nats_pipeline_intent(
            &request_id,
            &scenario,
            &data_lake_uri,
            &state.config.pipeline_intent_subject,
        ));
    }
    if job_intents.len() > MAX_PIPELINE_JOB_INTENTS {
        return Err(format!(
            "pipeline plan produced more than {MAX_PIPELINE_JOB_INTENTS} job intents"
        ));
    }

    let mut warnings = vec![
        "pipeline plans are redacted job intents; secret values are never returned".to_string(),
        "Airflow and Databricks are plan-only until their auth flows are explicitly enabled"
            .to_string(),
    ];
    if !state.config.allow_pipeline_submit {
        warnings.push(
            "Spark pipeline submission is disabled by ECONOMICS_ENABLE_PIPELINE_SUBMIT=false"
                .to_string(),
        );
    }

    Ok(PipelinePlanResponse {
        ok: true,
        request_id,
        schema_version: SCHEMA_VERSION,
        generated_at_ms: now_ms(),
        pipeline_status: pipeline_integration_status(state),
        recommendation_summary,
        job_intents,
        warnings,
    })
}

fn normalize_pipeline_job_kinds(input: Option<&Vec<String>>) -> Result<Vec<String>, String> {
    let values = input.cloned().unwrap_or_else(|| {
        vec![
            "ingest".to_string(),
            "spark-features".to_string(),
            "airflow".to_string(),
            "databricks".to_string(),
            "nats".to_string(),
        ]
    });
    if values.is_empty() {
        return Err("jobKinds must contain at least one item".to_string());
    }
    if values.len() > MAX_PIPELINE_JOB_INTENTS {
        return Err(format!(
            "jobKinds must contain at most {MAX_PIPELINE_JOB_INTENTS} items"
        ));
    }
    let mut normalized = Vec::with_capacity(values.len());
    for value in values {
        let clean = clean_token(&value, "jobKinds[]")?.to_ascii_lowercase();
        match clean.as_str() {
            "ingest" | "spark-features" | "airflow" | "databricks" | "nats" => {
                if !normalized.iter().any(|existing| existing == &clean) {
                    normalized.push(clean);
                }
            }
            _ => {
                return Err(format!(
                    "jobKinds[] value `{clean}` is not supported; use ingest, spark-features, airflow, databricks, or nats"
                ));
            }
        }
    }
    Ok(normalized)
}

fn validate_data_lake_uri(uri: &str) -> Result<(), String> {
    let trimmed = uri.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_URL_LEN || trimmed.chars().any(char::is_control) {
        return Err(format!(
            "dataLakeUri must be non-empty, contain no control characters, and be at most {MAX_URL_LEN} bytes"
        ));
    }
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("s3://")
        || lower.starts_with("s3a://")
        || lower.starts_with("abfss://")
        || lower.starts_with("gs://")
        || lower.starts_with("file:///tmp/"))
    {
        return Err("dataLakeUri must use s3, s3a, abfss, gs, or file:///tmp/".to_string());
    }
    Ok(())
}

fn spark_ingest_intent(request_id: &str, data_lake_uri: &str) -> PipelineJobIntent {
    PipelineJobIntent {
        id: format!("{request_id}-ingest-validate-publish"),
        engine: "spark-pipeline-server".to_string(),
        target: "dd-spark-pipeline-server.ai-ml.svc.cluster.local:8085".to_string(),
        kind: "INGEST_VALIDATE_PUBLISH".to_string(),
        endpoint: Some("/v1/jobs".to_string()),
        auth_required: true,
        submit_eligible: true,
        params: json!({
            "source": SERVICE_NAME,
            "dataset": "economics-market-history",
            "schemaVersion": SCHEMA_VERSION,
            "requestId": request_id,
            "dataLakeUri": data_lake_uri,
            "publicSourceIds": public_source_ids(),
            "inputRoutes": ["POST /sources/pull", "POST /ingest"],
            "qualityChecks": [
                "schema-check",
                "finite-price-volume-check",
                "duplicate-date-check",
                "asset-class-partition-check"
            ],
            "outputs": [
                format!("{data_lake_uri}/bronze/market_series"),
                format!("{data_lake_uri}/manifests/economics-market-history.json")
            ]
        }),
        notes: vec![
            "compatible with dd-spark-pipeline-server JobKind.INGEST_VALIDATE_PUBLISH".to_string(),
        ],
    }
}

fn spark_feature_intent(
    request_id: &str,
    scenario: &str,
    data_lake_uri: &str,
    spark_master_url: &str,
) -> PipelineJobIntent {
    PipelineJobIntent {
        id: format!("{request_id}-spark-feature-build"),
        engine: "spark-pipeline-server".to_string(),
        target: "dd-spark-pipeline-server.ai-ml.svc.cluster.local:8085".to_string(),
        kind: "SPARK_SUBMIT".to_string(),
        endpoint: Some("/v1/jobs".to_string()),
        auth_required: true,
        submit_eligible: true,
        params: json!({
            "source": SERVICE_NAME,
            "appName": "economics-feature-build",
            "requestId": request_id,
            "master": spark_master_url,
            "mainClass": "com.oresoftware.dd.economics.FeatureBuildJob",
            "appResource": format!("{data_lake_uri}/jobs/economics-feature-build.jar"),
            "args": [
                "--scenario", scenario,
                "--public-source-ids", public_source_ids().join(","),
                "--input", format!("{data_lake_uri}/bronze/market_series"),
                "--output", format!("{data_lake_uri}/silver/features"),
                "--recommendations", format!("{data_lake_uri}/gold/recommendations")
            ],
            "conf": {
                "spark.sql.shuffle.partitions": "96",
                "spark.sql.adaptive.enabled": "true",
                "spark.serializer": "org.apache.spark.serializer.KryoSerializer"
            }
        }),
        notes: vec![
            "placeholder Spark application contract; appResource is a data-lake artifact path, not bundled in this Rust service".to_string(),
        ],
    }
}

fn airflow_dag_intent(
    request_id: &str,
    scenario: &str,
    data_lake_uri: &str,
    airflow_api_url: Option<&str>,
) -> PipelineJobIntent {
    PipelineJobIntent {
        id: format!("{request_id}-airflow-refresh"),
        engine: "airflow".to_string(),
        target: airflow_api_url
            .unwrap_or(DEFAULT_AIRFLOW_API_URL)
            .to_string(),
        kind: "TRIGGER_DAG".to_string(),
        endpoint: Some("/api/v1/dags/economics_market_refresh/dagRuns".to_string()),
        auth_required: true,
        submit_eligible: false,
        params: json!({
            "dagRunId": format!("{request_id}-economics-market-refresh"),
            "conf": {
                "source": SERVICE_NAME,
                "schemaVersion": SCHEMA_VERSION,
                "scenario": scenario,
                "dataLakeUri": data_lake_uri,
                "publicSourceIds": public_source_ids(),
                "sparkPipelineJobKinds": ["INGEST_VALIDATE_PUBLISH", "SPARK_SUBMIT"]
            }
        }),
        notes: vec![
            "Airflow submission is plan-only until a service-account auth path is configured"
                .to_string(),
        ],
    }
}

fn databricks_job_intent(
    request_id: &str,
    scenario: &str,
    data_lake_uri: &str,
    databricks_host: Option<&str>,
) -> PipelineJobIntent {
    PipelineJobIntent {
        id: format!("{request_id}-databricks-run-now"),
        engine: "databricks".to_string(),
        target: databricks_host
            .unwrap_or("databricks-managed-workspace")
            .to_string(),
        kind: "DATABRICKS_RUN_NOW".to_string(),
        endpoint: Some("/api/2.1/jobs/run-now".to_string()),
        auth_required: true,
        submit_eligible: false,
        params: json!({
            "idempotencyToken": request_id,
            "jobName": "economics-feature-and-recommendation-refresh",
            "notebookParams": {
                "source": SERVICE_NAME,
                "schemaVersion": SCHEMA_VERSION,
                "scenario": scenario,
                "dataLakeUri": data_lake_uri,
                "publicSourceIds": public_source_ids()
            },
            "credentialEnv": ["ECONOMICS_DATABRICKS_HOST", "ECONOMICS_DATABRICKS_TOKEN"]
        }),
        notes: vec![
            "Databricks token status is exposed only as a boolean; token values are never returned"
                .to_string(),
        ],
    }
}

fn nats_pipeline_intent(
    request_id: &str,
    scenario: &str,
    data_lake_uri: &str,
    subject: &str,
) -> PipelineJobIntent {
    PipelineJobIntent {
        id: format!("{request_id}-nats-public-data-pipeline"),
        engine: "nats".to_string(),
        target: subject.to_string(),
        kind: "PUBLIC_DATA_PIPELINE_INTENT".to_string(),
        endpoint: None,
        auth_required: false,
        submit_eligible: false,
        params: json!({
            "messageKind": "economics.pipeline.intent",
            "source": SERVICE_NAME,
            "requestId": request_id,
            "schemaVersion": SCHEMA_VERSION,
            "scenario": scenario,
            "dataLakeUri": data_lake_uri,
            "publicSourceIds": public_source_ids(),
            "createdAtMs": now_ms()
        }),
        notes: vec![
            "published to dd.remote.public_data.pipeline.jobs or ECONOMICS_PIPELINE_INTENT_SUBJECT when NATS is configured".to_string(),
        ],
    }
}

async fn publish_pipeline_plan(state: &AppState, plan: &PipelinePlanResponse) {
    state
        .metrics
        .pipeline_publish_attempts_total
        .fetch_add(1, Ordering::Relaxed);
    let Some(nats) = state.nats.as_ref() else {
        state
            .metrics
            .pipeline_publish_failure_total
            .fetch_add(1, Ordering::Relaxed);
        emit_log(
            "WARN",
            "economics.pipeline.plan.publish.skipped",
            "pipeline plan publish requested but NATS is not configured",
            json!({
                "requestId": &plan.request_id,
                "subject": state.config.pipeline_intent_subject,
                "natsConfigured": false
            }),
        );
        return;
    };
    let payload = match serde_json::to_vec(&json!({
        "messageKind": "economics.pipeline.plan",
        "source": SERVICE_NAME,
        "plan": plan
    })) {
        Ok(payload) => payload,
        Err(error) => {
            state
                .metrics
                .pipeline_publish_failure_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            emit_log(
                "ERROR",
                "economics.pipeline.plan.encode.error",
                "failed to encode economics pipeline plan",
                json!({
                    "error": error_summary(&error.to_string()),
                    "requestId": &plan.request_id
                }),
            );
            return;
        }
    };
    match nats
        .publish(state.config.pipeline_intent_subject.clone(), payload.into())
        .await
    {
        Ok(()) => {
            state
                .metrics
                .nats_published_total
                .fetch_add(1, Ordering::Relaxed);
            state
                .metrics
                .pipeline_publish_success_total
                .fetch_add(1, Ordering::Relaxed);
            emit_log(
                "INFO",
                "economics.pipeline.plan.publish.ok",
                "pipeline plan published to NATS",
                json!({
                    "requestId": &plan.request_id,
                    "subject": state.config.pipeline_intent_subject,
                    "jobIntentCount": plan.job_intents.len()
                }),
            );
        }
        Err(error) => {
            state
                .metrics
                .pipeline_publish_failure_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            emit_log(
                "ERROR",
                "economics.pipeline.plan.publish.error",
                "failed to publish pipeline plan to NATS",
                json!({
                    "requestId": &plan.request_id,
                    "subject": state.config.pipeline_intent_subject,
                    "error": error_summary(&error.to_string())
                }),
            );
        }
    }
}

fn validate_pipeline_submit_url(config: &Config) -> Result<String, String> {
    let Some(base) = config.spark_pipeline_url.as_deref() else {
        return Err("ECONOMICS_SPARK_PIPELINE_URL is not configured".to_string());
    };
    validate_http_base_url(
        base,
        config.allow_external_pipeline_urls,
        "spark pipeline URL",
    )?;
    Ok(format!("{}/v1/jobs", base.trim_end_matches('/')))
}

async fn submit_pipeline_plan(
    state: &AppState,
    plan: &PipelinePlanResponse,
) -> Result<Vec<PipelineSubmittedJob>, String> {
    if !state.config.allow_pipeline_submit {
        return Err(
            "pipeline submission is disabled; set ECONOMICS_ENABLE_PIPELINE_SUBMIT=true"
                .to_string(),
        );
    }
    let submit_url = validate_pipeline_submit_url(&state.config)?;
    let auth_value = optional_env(&state.config.spark_pipeline_auth_env).ok_or_else(|| {
        format!(
            "spark pipeline auth env {} is not configured",
            state.config.spark_pipeline_auth_env
        )
    })?;
    let mut submitted = Vec::new();
    for intent in plan
        .job_intents
        .iter()
        .filter(|intent| intent.engine == "spark-pipeline-server" && intent.submit_eligible)
    {
        let payload = json!({
            "kind": intent.kind,
            "params": intent.params
        });
        let response = state
            .http
            .post(&submit_url)
            .header("x-server-auth", &auth_value)
            .json(&payload)
            .send()
            .await;
        match response {
            Ok(response) => {
                let status = response.status().as_u16();
                let accepted = (200..300).contains(&status);
                let body = response.json::<Value>().await.ok();
                if accepted {
                    state
                        .metrics
                        .pipeline_submit_success_total
                        .fetch_add(1, Ordering::Relaxed);
                    emit_log(
                        "INFO",
                        "economics.pipeline.submit.ok",
                        "pipeline job submitted to Spark pipeline server",
                        json!({
                            "requestId": &plan.request_id,
                            "intentId": &intent.id,
                            "kind": &intent.kind,
                            "httpStatus": status
                        }),
                    );
                } else {
                    state
                        .metrics
                        .pipeline_submit_failure_total
                        .fetch_add(1, Ordering::Relaxed);
                    emit_log(
                        "WARN",
                        "economics.pipeline.submit.rejected",
                        "Spark pipeline server rejected a submitted economics job",
                        json!({
                            "requestId": &plan.request_id,
                            "intentId": &intent.id,
                            "kind": &intent.kind,
                            "httpStatus": status
                        }),
                    );
                }
                submitted.push(PipelineSubmittedJob {
                    intent_id: intent.id.clone(),
                    target: submit_url.clone(),
                    http_status: Some(status),
                    accepted,
                    response: body,
                    error: None,
                });
            }
            Err(error) => {
                state
                    .metrics
                    .pipeline_submit_failure_total
                    .fetch_add(1, Ordering::Relaxed);
                emit_log(
                    "ERROR",
                    "economics.pipeline.submit.error",
                    "failed to submit economics job to Spark pipeline server",
                    json!({
                        "requestId": &plan.request_id,
                        "intentId": &intent.id,
                        "kind": &intent.kind,
                        "error": error_summary(&error.to_string())
                    }),
                );
                submitted.push(PipelineSubmittedJob {
                    intent_id: intent.id.clone(),
                    target: submit_url.clone(),
                    http_status: None,
                    accepted: false,
                    response: None,
                    error: Some(error_summary(&error.to_string())),
                });
            }
        }
    }
    Ok(submitted)
}

fn generate_recommendations(
    config: &Config,
    request: RecommendationRequest,
) -> Result<RecommendationsResponse, String> {
    if let Some(schema) = request.schema_version.as_deref() {
        if schema != SCHEMA_VERSION {
            return Err(format!("schemaVersion must be {SCHEMA_VERSION}"));
        }
    }
    validate_macro_context(request.macro_context.as_ref())?;
    validate_macro_fiscal_context(request.macro_fiscal_context.as_ref())?;
    validate_venture_capital_context(request.venture_capital_context.as_ref())?;
    validate_sentiment_context(request.sentiment_context.as_ref())?;

    let request_id = request_id(request.request_id.as_ref(), "economics-recommendations");
    let horizon_months = request
        .horizon_months
        .unwrap_or(config.projection_months)
        .clamp(1, 120);
    let company_limit = request.company_limit.unwrap_or(20).clamp(1, 20);
    let commodity_limit = request.commodity_limit.unwrap_or(30).clamp(1, 30);
    let scenario = request
        .scenario
        .unwrap_or_else(|| "base".to_string())
        .trim()
        .to_ascii_lowercase();
    let macro_context = request.macro_context.unwrap_or_default();
    let macro_fiscal_context = request
        .macro_fiscal_context
        .unwrap_or_else(default_macro_fiscal_context);
    let venture_capital_context = match request.venture_capital_context {
        Some(context) if !context.deals.is_empty() || !context.sector_flows.is_empty() => context,
        _ => sample_venture_capital_context(),
    };
    let sentiment_context = request.sentiment_context.unwrap_or_default();
    let series = request.series.unwrap_or_else(sample_market_series);
    validate_series(&series)?;
    let series_hints = series_signal_hints(config, &series)?;

    let mut company_scores = company_candidates()
        .into_iter()
        .map(|candidate| {
            score_company_candidate(
                &candidate,
                &macro_context,
                &macro_fiscal_context,
                &venture_capital_context,
                &sentiment_context,
                &series_hints,
                &scenario,
                horizon_months,
            )
        })
        .collect::<Vec<_>>();
    company_scores.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut commodity_scores = commodity_candidates()
        .into_iter()
        .map(|candidate| {
            score_commodity_candidate(
                &candidate,
                &macro_context,
                &macro_fiscal_context,
                &sentiment_context,
                &series_hints,
                &scenario,
                horizon_months,
            )
        })
        .collect::<Vec<_>>();
    commodity_scores.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let company_buys = company_scores
        .iter()
        .take(company_limit)
        .enumerate()
        .map(|(index, item)| company_with_action(item, index + 1, "invest"))
        .collect::<Vec<_>>();
    let mut company_dump_source = company_scores.clone();
    company_dump_source.sort_by(|left, right| {
        left.score
            .partial_cmp(&right.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let company_dumps = company_dump_source
        .iter()
        .take(company_limit)
        .enumerate()
        .map(|(index, item)| company_with_action(item, index + 1, "dump-or-hedge"))
        .collect::<Vec<_>>();

    let commodity_buys = commodity_scores
        .iter()
        .take(commodity_limit)
        .enumerate()
        .map(|(index, item)| commodity_with_action(item, index + 1, "buy"))
        .collect::<Vec<_>>();
    let mut commodity_sell_source = commodity_scores.clone();
    commodity_sell_source.sort_by(|left, right| {
        left.score
            .partial_cmp(&right.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let commodity_sells_or_dumps = commodity_sell_source
        .iter()
        .take(commodity_limit)
        .enumerate()
        .map(|(index, item)| commodity_with_action(item, index + 1, "sell-or-dump"))
        .collect::<Vec<_>>();

    let mut warnings = vec![
        "rankings are model signals for research workflows, not financial advice".to_string(),
        "built-in candidate universes are placeholders until live market, macro, and private-data adapters are connected".to_string(),
    ];
    if series
        .iter()
        .any(|item| item.source.as_deref() == Some("built-in-sample"))
    {
        warnings.push(
            "using built-in demonstration market series for observed signal hints".to_string(),
        );
    }

    Ok(RecommendationsResponse {
        ok: true,
        request_id,
        schema_version: SCHEMA_VERSION,
        horizon_months,
        scenario,
        generated_at_ms: now_ms(),
        macro_fiscal_context,
        venture_capital_context,
        data_credential_status: config.market_data_credentials.clone(),
        company_buys,
        company_dumps,
        commodity_buys,
        commodity_sells_or_dumps,
        methodology: vec![
            "company scores blend profitability, growth, balance-sheet strength, valuation, momentum, macro/fiscal/labor pressure, VC sector flow, sentiment, and observed series hints".to_string(),
            "commodity scores blend demand growth, supply tightness, inventory pressure, carry, geopolitical risk, valuation, inflation/real-rate/fiscal effects, sentiment, and observed series hints".to_string(),
            "theoretical priors stay transparent: CAPM/Taylor/Fisher/UIP/PPP/Hotelling/Phillips-style forces are represented through bounded model components".to_string(),
        ],
        warnings,
    })
}

fn series_signal_hints(
    config: &Config,
    series: &[MarketSeries],
) -> Result<BTreeMap<String, f64>, String> {
    let mut hints = BTreeMap::new();
    for item in series {
        let stats = series_stats(item, config.history_years)?;
        let signal = clamp(0.55 * stats.data_drift + 0.45 * stats.momentum, -0.50, 0.50);
        hints.insert(item.instrument_id.to_ascii_lowercase(), signal);
    }
    Ok(hints)
}

fn score_company_candidate(
    candidate: &CompanyCandidate,
    macro_context: &MacroContext,
    fiscal_context: &MacroFiscalContext,
    vc_context: &VentureCapitalContext,
    sentiment_context: &SentimentSignalContext,
    series_hints: &BTreeMap<String, f64>,
    scenario: &str,
    horizon_months: u32,
) -> CompanyRecommendation {
    let quality = 0.20 * candidate.profitability
        + 0.18 * candidate.growth
        + 0.14 * candidate.balance_sheet
        + 0.12 * candidate.momentum;
    let valuation = -0.16 * candidate.valuation_gap;
    let real_rate = finite_or(macro_context.policy_rate, 0.045)
        - finite_or(
            macro_context.expected_inflation,
            finite_or(macro_context.inflation, 0.030),
        );
    let rate_sensitivity = -0.16 * candidate.beta * real_rate;
    let macro_fiscal = sector_macro_adjustment(candidate.sector, macro_context, fiscal_context)
        + 0.20 * fiscal_equity_bias(fiscal_context);
    let vc_flow = vc_sector_impulse(vc_context, candidate.sector);
    let sentiment = sentiment_adjustment(sentiment_context, candidate.ticker, candidate.sector);
    let observed = series_hints
        .get(&candidate.ticker.to_ascii_lowercase())
        .copied()
        .unwrap_or(0.0)
        * 0.35;
    let scenario_component =
        scenario_company_adjustment(candidate.sector, candidate.beta, scenario, fiscal_context);
    let score = clamp(
        quality
            + valuation
            + rate_sensitivity
            + macro_fiscal
            + vc_flow
            + sentiment
            + observed
            + scenario_component,
        -1.0,
        1.0,
    );
    let horizon_scale = f64::from(horizon_months) / 18.0;
    let expected_return_18m = clamp(score * 0.46 * horizon_scale, -0.85, 0.95);
    let confidence = clamp(
        0.48 + score.abs() * 0.24 + vc_flow.abs() * 0.18 + candidate.balance_sheet.max(0.0) * 0.08,
        0.30,
        0.92,
    );
    CompanyRecommendation {
        rank: 0,
        ticker: candidate.ticker.to_string(),
        company: candidate.company.to_string(),
        sector: candidate.sector.to_string(),
        stage: candidate.stage.to_string(),
        action: "candidate".to_string(),
        score: round6(score),
        expected_return_18m: round6(expected_return_18m),
        confidence: round6(confidence),
        reasons: company_reasons(candidate, score, macro_fiscal, vc_flow, sentiment),
        components: vec![
            recommendation_component("qualityGrowthMomentum", quality, 1.0),
            recommendation_component("valuation", valuation, 1.0),
            recommendation_component("rateSensitivity", rate_sensitivity, 1.0),
            recommendation_component("macroFiscalLabor", macro_fiscal, 1.0),
            recommendation_component("ventureCapitalFlow", vc_flow, 1.0),
            recommendation_component("sentiment", sentiment, 1.0),
            recommendation_component("observedSeriesSignal", observed, 1.0),
            recommendation_component("scenario", scenario_component, 1.0),
        ],
    }
}

fn score_commodity_candidate(
    candidate: &CommodityCandidate,
    macro_context: &MacroContext,
    fiscal_context: &MacroFiscalContext,
    sentiment_context: &SentimentSignalContext,
    series_hints: &BTreeMap<String, f64>,
    scenario: &str,
    horizon_months: u32,
) -> CommodityRecommendation {
    let fundamentals = 0.22 * candidate.demand_growth
        + 0.20 * candidate.supply_tightness
        + 0.16 * candidate.inventory_pressure
        + 0.10 * candidate.carry
        + 0.08 * candidate.geopolitical_risk;
    let valuation = -0.16 * candidate.valuation_gap;
    let macro_component =
        commodity_macro_adjustment(candidate.commodity_class, macro_context, fiscal_context);
    let sentiment = sentiment_adjustment(
        sentiment_context,
        candidate.instrument_id,
        candidate.commodity_class,
    );
    let observed = series_hints
        .get(&candidate.instrument_id.to_ascii_lowercase())
        .copied()
        .unwrap_or(0.0)
        * 0.45;
    let scenario_component = scenario_commodity_adjustment(candidate.commodity_class, scenario);
    let score = clamp(
        fundamentals + valuation + macro_component + sentiment + observed + scenario_component,
        -1.0,
        1.0,
    );
    let horizon_scale = f64::from(horizon_months) / 18.0;
    let expected_return_18m = clamp(score * 0.42 * horizon_scale, -0.80, 0.90);
    let confidence = clamp(
        0.46 + score.abs() * 0.22 + candidate.volatility.max(0.0) * 0.08,
        0.28,
        0.90,
    );
    CommodityRecommendation {
        rank: 0,
        instrument_id: candidate.instrument_id.to_string(),
        commodity: candidate.commodity.to_string(),
        commodity_class: candidate.commodity_class.to_string(),
        action: "candidate".to_string(),
        score: round6(score),
        expected_return_18m: round6(expected_return_18m),
        confidence: round6(confidence),
        reasons: commodity_reasons(candidate, score, macro_component, sentiment),
        components: vec![
            recommendation_component("fundamentals", fundamentals, 1.0),
            recommendation_component("valuation", valuation, 1.0),
            recommendation_component("macroFiscalInflation", macro_component, 1.0),
            recommendation_component("sentiment", sentiment, 1.0),
            recommendation_component("observedSeriesSignal", observed, 1.0),
            recommendation_component("scenario", scenario_component, 1.0),
        ],
    }
}

fn company_with_action(
    item: &CompanyRecommendation,
    rank: usize,
    action: &str,
) -> CompanyRecommendation {
    let mut clone = item.clone();
    clone.rank = rank;
    clone.action = action.to_string();
    clone
}

fn commodity_with_action(
    item: &CommodityRecommendation,
    rank: usize,
    action: &str,
) -> CommodityRecommendation {
    let mut clone = item.clone();
    clone.rank = rank;
    clone.action = action.to_string();
    clone
}

fn recommendation_component(name: &str, value: f64, weight: f64) -> RecommendationComponent {
    RecommendationComponent {
        name: name.to_string(),
        value: round6(value),
        weight: round6(weight),
    }
}

fn company_reasons(
    candidate: &CompanyCandidate,
    score: f64,
    macro_fiscal: f64,
    vc_flow: f64,
    sentiment: f64,
) -> Vec<String> {
    let mut reasons = vec![format!(
        "{} model score {:.2} with growth {:.2}, profitability {:.2}, and valuation gap {:.2}",
        candidate.sector, score, candidate.growth, candidate.profitability, candidate.valuation_gap
    )];
    if macro_fiscal.abs() > 0.02 {
        reasons.push(format!(
            "macro/fiscal/labor contribution {:.2}",
            macro_fiscal
        ));
    }
    if vc_flow.abs() > 0.02 {
        reasons.push(format!("VC sector-flow contribution {:.2}", vc_flow));
    }
    if sentiment.abs() > 0.01 {
        reasons.push(format!("sentiment contribution {:.2}", sentiment));
    }
    if score < 0.0 {
        reasons.push(
            "negative composite score flags dump, hedge, or avoid research priority".to_string(),
        );
    } else {
        reasons
            .push("positive composite score flags invest or deeper diligence priority".to_string());
    }
    reasons
}

fn commodity_reasons(
    candidate: &CommodityCandidate,
    score: f64,
    macro_component: f64,
    sentiment: f64,
) -> Vec<String> {
    let mut reasons = vec![format!(
        "{} score {:.2} from demand {:.2}, supply tightness {:.2}, inventory pressure {:.2}",
        candidate.commodity_class,
        score,
        candidate.demand_growth,
        candidate.supply_tightness,
        candidate.inventory_pressure
    )];
    if macro_component.abs() > 0.02 {
        reasons.push(format!(
            "inflation/real-rate/fiscal contribution {:.2}",
            macro_component
        ));
    }
    if sentiment.abs() > 0.01 {
        reasons.push(format!("sentiment contribution {:.2}", sentiment));
    }
    if score < 0.0 {
        reasons.push(
            "negative composite score flags sell, dump, or avoid research priority".to_string(),
        );
    } else {
        reasons
            .push("positive composite score flags buy or accumulate research priority".to_string());
    }
    reasons
}

fn fiscal_equity_bias(fiscal_context: &MacroFiscalContext) -> f64 {
    let gdp_growth = finite_or(fiscal_context.gdp_growth, 0.021);
    let productivity = finite_or(fiscal_context.productivity_growth, 0.015);
    let payroll_growth = finite_or(fiscal_context.payroll_growth, 0.014);
    let unemployment = finite_or(fiscal_context.unemployment_rate, 0.040);
    clamp(
        1.5 * gdp_growth + productivity + payroll_growth
            - fiscal_stress(fiscal_context)
            - 0.4 * unemployment,
        -0.20,
        0.20,
    )
}

fn fiscal_stress(fiscal_context: &MacroFiscalContext) -> f64 {
    let gdp = finite_or(fiscal_context.gdp, 29_000_000_000_000.0).max(1.0);
    let deficit_to_gdp = finite_or(
        fiscal_context.deficit_to_gdp,
        finite_or(fiscal_context.deficit, 1_800_000_000_000.0) / gdp,
    );
    let debt_to_gdp = finite_or(
        fiscal_context.debt_to_gdp,
        finite_or(fiscal_context.national_debt, 36_000_000_000_000.0) / gdp,
    );
    let borrowing_to_gdp = finite_or(fiscal_context.borrowing, 1_900_000_000_000.0) / gdp;
    let interest_to_gdp = finite_or(fiscal_context.net_interest_outlays, 950_000_000_000.0) / gdp;
    clamp(
        0.55 * deficit_to_gdp
            + 0.08 * debt_to_gdp
            + 0.60 * borrowing_to_gdp
            + 1.10 * interest_to_gdp,
        0.0,
        0.35,
    )
}

fn sector_macro_adjustment(
    sector: &str,
    macro_context: &MacroContext,
    fiscal_context: &MacroFiscalContext,
) -> f64 {
    let lower = sector.to_ascii_lowercase();
    let inflation = finite_or(macro_context.inflation, 0.030);
    let expected_inflation = finite_or(macro_context.expected_inflation, inflation);
    let real_rate = finite_or(macro_context.policy_rate, 0.045) - expected_inflation;
    let gdp_growth = finite_or(fiscal_context.gdp_growth, 0.021);
    let productivity = finite_or(fiscal_context.productivity_growth, 0.015);
    let labor = finite_or(fiscal_context.labor_force_participation, 0.626) - 0.620;
    let wage_growth = finite_or(fiscal_context.wage_growth, 0.041);
    let deficit = finite_or(fiscal_context.deficit_to_gdp, 0.062);
    let stress = fiscal_stress(fiscal_context);
    let base = 1.2 * gdp_growth + 0.8 * productivity + 0.6 * labor - 0.8 * stress;
    let adjustment = if lower.contains("technology")
        || lower.contains("artificial")
        || lower.contains("software")
        || lower.contains("semiconductor")
    {
        base + 2.2 * productivity - 1.8 * real_rate
    } else if lower.contains("financial") || lower.contains("fintech") {
        base + 0.9 * real_rate - 0.8 * stress
    } else if lower.contains("energy") || lower.contains("materials") {
        base + 1.6 * inflation + 0.8 * deficit
    } else if lower.contains("industrial") || lower.contains("defense") {
        base + 0.7 * deficit + 0.8 * productivity
    } else if lower.contains("consumer") || lower.contains("retail") {
        base + 1.1 * wage_growth + 0.9 * labor - 0.7 * inflation
    } else if lower.contains("real-estate") || lower.contains("utilities") {
        base - 2.4 * real_rate - 0.5 * stress
    } else if lower.contains("health") || lower.contains("biotech") {
        0.5 * base - 0.4 * stress + 0.5 * productivity
    } else {
        base - 0.8 * real_rate
    };
    clamp(adjustment, -0.18, 0.18)
}

fn commodity_macro_adjustment(
    commodity_class: &str,
    macro_context: &MacroContext,
    fiscal_context: &MacroFiscalContext,
) -> f64 {
    let lower = commodity_class.to_ascii_lowercase();
    let inflation = finite_or(macro_context.inflation, 0.030);
    let expected_inflation = finite_or(macro_context.expected_inflation, inflation);
    let real_rate = finite_or(macro_context.policy_rate, 0.045) - expected_inflation;
    let gdp_growth = finite_or(fiscal_context.gdp_growth, 0.021);
    let productivity = finite_or(fiscal_context.productivity_growth, 0.015);
    let stress = fiscal_stress(fiscal_context);
    let labor = finite_or(fiscal_context.labor_force_participation, 0.626) - 0.620;
    let base = 1.1 * gdp_growth + 1.3 * inflation - 1.0 * real_rate + 0.35 * stress;
    let adjustment = if lower.contains("precious")
        || lower.contains("gold")
        || lower.contains("silver")
    {
        base + 2.4 * inflation - 2.2 * real_rate + 0.6 * stress
    } else if lower.contains("energy") {
        base + 1.7 * gdp_growth + 0.8 * inflation
    } else if lower.contains("industrial") || lower.contains("battery") || lower.contains("bulk") {
        base + 1.5 * gdp_growth + 1.2 * productivity
    } else if lower.contains("agriculture") || lower.contains("food") || lower.contains("livestock")
    {
        base + 1.2 * inflation + 0.5 * labor
    } else if lower.contains("carbon") || lower.contains("freight") {
        base + 1.0 * productivity + 0.8 * gdp_growth
    } else {
        base
    };
    clamp(adjustment, -0.22, 0.24)
}

fn vc_sector_impulse(context: &VentureCapitalContext, sector: &str) -> f64 {
    let mut score = 0.0;
    let mut weight = 0.0;
    for flow in &context.sector_flows {
        if sector_matches(sector, &flow.sector) {
            let confidence = finite_or(flow.confidence, 0.50).clamp(0.0, 1.0);
            let capital_score = (flow.invested_capital.max(0.0) / 10_000_000_000.0)
                .ln_1p()
                .min(3.0)
                / 3.0;
            let flow_score = 0.55 * flow.yoy_growth
                + 0.25 * capital_score
                + 0.20 * finite_or(flow.exit_liquidity, 0.20);
            score += flow_score * confidence;
            weight += confidence;
        }
    }
    for deal in &context.deals {
        if sector_matches(sector, &deal.sector) {
            let confidence = finite_or(deal.confidence, 0.45).clamp(0.0, 1.0);
            let stage_boost = if deal.stage.to_ascii_lowercase().contains("late") {
                0.08
            } else if deal.stage.to_ascii_lowercase().contains("growth") {
                0.05
            } else {
                0.02
            };
            let amount_score = (deal.amount.max(0.0) / 1_000_000_000.0).ln_1p().min(2.0) / 2.0;
            score += (0.20 * amount_score + stage_boost) * confidence;
            weight += confidence;
        }
    }
    if weight <= f64::EPSILON {
        0.0
    } else {
        clamp(score / weight, -0.18, 0.24)
    }
}

fn sector_matches(candidate_sector: &str, signal_sector: &str) -> bool {
    let candidate = candidate_sector.to_ascii_lowercase();
    let signal = signal_sector.to_ascii_lowercase();
    if candidate.contains(&signal) || signal.contains(&candidate) {
        return true;
    }
    let aliases: &[&str] = if candidate.contains("technology") || candidate.contains("software") {
        &[
            "ai",
            "artificial",
            "software",
            "data",
            "cloud",
            "cyber",
            "semiconductor",
        ]
    } else if candidate.contains("health") || candidate.contains("biotech") {
        &["biotech", "health", "pharma", "life-science"]
    } else if candidate.contains("energy") {
        &["energy", "climate", "fusion", "grid", "battery"]
    } else if candidate.contains("financial") || candidate.contains("fintech") {
        &["fintech", "payments", "banking", "financial"]
    } else if candidate.contains("industrial") || candidate.contains("defense") {
        &[
            "industrial",
            "automation",
            "defense",
            "manufacturing",
            "robotics",
        ]
    } else if candidate.contains("consumer") {
        &["consumer", "retail", "marketplace"]
    } else {
        &[]
    };
    aliases
        .iter()
        .any(|alias| candidate.contains(alias) || signal.contains(alias))
}

fn sentiment_adjustment(
    context: &SentimentSignalContext,
    instrument_id: &str,
    sector_or_class: &str,
) -> f64 {
    let average = finite_or(context.average_sentiment, 0.0);
    let instrument = lookup_context_score(context.instrument_scores.as_ref(), instrument_id)
        .unwrap_or(average * 0.5);
    let sector = lookup_context_score(context.sector_scores.as_ref(), sector_or_class)
        .unwrap_or(average * 0.5);
    clamp(
        0.07 * instrument + 0.05 * sector + 0.03 * average,
        -0.15,
        0.15,
    )
}

fn lookup_context_score(map: Option<&BTreeMap<String, f64>>, key: &str) -> Option<f64> {
    let key_lower = key.to_ascii_lowercase();
    map.and_then(|scores| {
        scores.iter().find_map(|(candidate, value)| {
            let candidate_lower = candidate.to_ascii_lowercase();
            if candidate_lower == key_lower
                || candidate_lower.contains(&key_lower)
                || key_lower.contains(&candidate_lower)
            {
                value.is_finite().then_some(*value)
            } else {
                None
            }
        })
    })
}

fn scenario_company_adjustment(
    sector: &str,
    beta: f64,
    scenario: &str,
    fiscal_context: &MacroFiscalContext,
) -> f64 {
    let lower = sector.to_ascii_lowercase();
    let adjustment = match scenario {
        "liquidity-crunch" => -0.08 * beta - 0.05 * fiscal_stress(fiscal_context),
        "oil-shock" if lower.contains("energy") => 0.10,
        "oil-shock" if lower.contains("consumer") || lower.contains("transport") => -0.08,
        "dollar-strength" if lower.contains("materials") || lower.contains("energy") => -0.05,
        "deflation" if lower.contains("utilities") || lower.contains("health") => 0.03,
        "deflation" => -0.05 * beta,
        "soft-landing" => 0.04 + 0.02 * beta,
        _ => 0.0,
    };
    clamp(adjustment, -0.14, 0.14)
}

fn scenario_commodity_adjustment(commodity_class: &str, scenario: &str) -> f64 {
    let lower = commodity_class.to_ascii_lowercase();
    let adjustment = match scenario {
        "oil-shock" if lower.contains("energy") => 0.18,
        "oil-shock" if lower.contains("agriculture") || lower.contains("food") => 0.05,
        "liquidity-crunch" => -0.06,
        "dollar-strength" if lower.contains("precious") || lower.contains("industrial") => -0.07,
        "deflation" if lower.contains("precious") => -0.04,
        "deflation" => -0.08,
        "soft-landing" if lower.contains("industrial") || lower.contains("energy") => 0.06,
        _ => 0.0,
    };
    clamp(adjustment, -0.18, 0.18)
}

fn company_candidates() -> Vec<CompanyCandidate> {
    vec![
        CompanyCandidate {
            ticker: "NVDA",
            company: "NVIDIA",
            sector: "technology-semiconductor-ai",
            stage: "public",
            beta: 1.7,
            profitability: 0.92,
            growth: 0.95,
            balance_sheet: 0.72,
            valuation_gap: 0.38,
            momentum: 0.88,
        },
        CompanyCandidate {
            ticker: "MSFT",
            company: "Microsoft",
            sector: "technology-software-cloud-ai",
            stage: "public",
            beta: 1.0,
            profitability: 0.90,
            growth: 0.62,
            balance_sheet: 0.86,
            valuation_gap: 0.20,
            momentum: 0.56,
        },
        CompanyCandidate {
            ticker: "AVGO",
            company: "Broadcom",
            sector: "technology-semiconductor-infrastructure",
            stage: "public",
            beta: 1.3,
            profitability: 0.82,
            growth: 0.62,
            balance_sheet: 0.55,
            valuation_gap: 0.18,
            momentum: 0.63,
        },
        CompanyCandidate {
            ticker: "GOOGL",
            company: "Alphabet",
            sector: "technology-advertising-ai",
            stage: "public",
            beta: 1.1,
            profitability: 0.78,
            growth: 0.48,
            balance_sheet: 0.88,
            valuation_gap: -0.05,
            momentum: 0.42,
        },
        CompanyCandidate {
            ticker: "AMZN",
            company: "Amazon",
            sector: "technology-cloud-consumer",
            stage: "public",
            beta: 1.4,
            profitability: 0.54,
            growth: 0.58,
            balance_sheet: 0.48,
            valuation_gap: 0.10,
            momentum: 0.45,
        },
        CompanyCandidate {
            ticker: "META",
            company: "Meta Platforms",
            sector: "technology-advertising-ai",
            stage: "public",
            beta: 1.2,
            profitability: 0.84,
            growth: 0.52,
            balance_sheet: 0.76,
            valuation_gap: 0.02,
            momentum: 0.48,
        },
        CompanyCandidate {
            ticker: "AMD",
            company: "Advanced Micro Devices",
            sector: "technology-semiconductor-ai",
            stage: "public",
            beta: 1.8,
            profitability: 0.44,
            growth: 0.64,
            balance_sheet: 0.50,
            valuation_gap: 0.30,
            momentum: 0.36,
        },
        CompanyCandidate {
            ticker: "AAPL",
            company: "Apple",
            sector: "technology-consumer-hardware",
            stage: "public",
            beta: 1.0,
            profitability: 0.86,
            growth: 0.22,
            balance_sheet: 0.66,
            valuation_gap: 0.16,
            momentum: 0.20,
        },
        CompanyCandidate {
            ticker: "TSLA",
            company: "Tesla",
            sector: "consumer-energy-automation",
            stage: "public",
            beta: 2.0,
            profitability: 0.34,
            growth: 0.38,
            balance_sheet: 0.42,
            valuation_gap: 0.44,
            momentum: -0.08,
        },
        CompanyCandidate {
            ticker: "ASML",
            company: "ASML",
            sector: "technology-semiconductor-equipment",
            stage: "public",
            beta: 1.2,
            profitability: 0.80,
            growth: 0.44,
            balance_sheet: 0.72,
            valuation_gap: 0.12,
            momentum: 0.34,
        },
        CompanyCandidate {
            ticker: "JPM",
            company: "JPMorgan Chase",
            sector: "financials-banking",
            stage: "public",
            beta: 1.1,
            profitability: 0.70,
            growth: 0.22,
            balance_sheet: 0.66,
            valuation_gap: -0.04,
            momentum: 0.28,
        },
        CompanyCandidate {
            ticker: "GS",
            company: "Goldman Sachs",
            sector: "financials-capital-markets",
            stage: "public",
            beta: 1.3,
            profitability: 0.58,
            growth: 0.18,
            balance_sheet: 0.48,
            valuation_gap: 0.02,
            momentum: 0.18,
        },
        CompanyCandidate {
            ticker: "V",
            company: "Visa",
            sector: "financials-payments-fintech",
            stage: "public",
            beta: 0.9,
            profitability: 0.88,
            growth: 0.36,
            balance_sheet: 0.78,
            valuation_gap: 0.12,
            momentum: 0.24,
        },
        CompanyCandidate {
            ticker: "MA",
            company: "Mastercard",
            sector: "financials-payments-fintech",
            stage: "public",
            beta: 1.0,
            profitability: 0.86,
            growth: 0.38,
            balance_sheet: 0.70,
            valuation_gap: 0.15,
            momentum: 0.25,
        },
        CompanyCandidate {
            ticker: "BRK.B",
            company: "Berkshire Hathaway",
            sector: "financials-industrials-insurance",
            stage: "public",
            beta: 0.8,
            profitability: 0.62,
            growth: 0.18,
            balance_sheet: 0.92,
            valuation_gap: -0.03,
            momentum: 0.18,
        },
        CompanyCandidate {
            ticker: "LLY",
            company: "Eli Lilly",
            sector: "healthcare-biotech-pharma",
            stage: "public",
            beta: 0.7,
            profitability: 0.76,
            growth: 0.70,
            balance_sheet: 0.58,
            valuation_gap: 0.36,
            momentum: 0.62,
        },
        CompanyCandidate {
            ticker: "UNH",
            company: "UnitedHealth Group",
            sector: "healthcare-services",
            stage: "public",
            beta: 0.7,
            profitability: 0.64,
            growth: 0.24,
            balance_sheet: 0.54,
            valuation_gap: -0.02,
            momentum: -0.12,
        },
        CompanyCandidate {
            ticker: "MRK",
            company: "Merck",
            sector: "healthcare-pharma",
            stage: "public",
            beta: 0.5,
            profitability: 0.66,
            growth: 0.20,
            balance_sheet: 0.58,
            valuation_gap: -0.05,
            momentum: 0.08,
        },
        CompanyCandidate {
            ticker: "PFE",
            company: "Pfizer",
            sector: "healthcare-pharma",
            stage: "public",
            beta: 0.6,
            profitability: 0.28,
            growth: -0.18,
            balance_sheet: 0.36,
            valuation_gap: -0.30,
            momentum: -0.34,
        },
        CompanyCandidate {
            ticker: "XOM",
            company: "Exxon Mobil",
            sector: "energy-oil-gas",
            stage: "public",
            beta: 0.9,
            profitability: 0.64,
            growth: 0.14,
            balance_sheet: 0.72,
            valuation_gap: -0.10,
            momentum: 0.15,
        },
        CompanyCandidate {
            ticker: "CVX",
            company: "Chevron",
            sector: "energy-oil-gas",
            stage: "public",
            beta: 0.9,
            profitability: 0.58,
            growth: 0.08,
            balance_sheet: 0.76,
            valuation_gap: -0.08,
            momentum: 0.06,
        },
        CompanyCandidate {
            ticker: "COP",
            company: "ConocoPhillips",
            sector: "energy-oil-gas",
            stage: "public",
            beta: 1.0,
            profitability: 0.62,
            growth: 0.16,
            balance_sheet: 0.60,
            valuation_gap: -0.02,
            momentum: 0.14,
        },
        CompanyCandidate {
            ticker: "NEE",
            company: "NextEra Energy",
            sector: "utilities-climate-energy",
            stage: "public",
            beta: 0.7,
            profitability: 0.42,
            growth: 0.18,
            balance_sheet: 0.24,
            valuation_gap: 0.08,
            momentum: -0.20,
        },
        CompanyCandidate {
            ticker: "CAT",
            company: "Caterpillar",
            sector: "industrials-machinery",
            stage: "public",
            beta: 1.1,
            profitability: 0.62,
            growth: 0.22,
            balance_sheet: 0.52,
            valuation_gap: 0.04,
            momentum: 0.20,
        },
        CompanyCandidate {
            ticker: "DE",
            company: "Deere",
            sector: "industrials-agriculture-machinery",
            stage: "public",
            beta: 1.0,
            profitability: 0.60,
            growth: 0.10,
            balance_sheet: 0.46,
            valuation_gap: -0.04,
            momentum: 0.02,
        },
        CompanyCandidate {
            ticker: "GE",
            company: "GE Aerospace",
            sector: "industrials-aerospace",
            stage: "public",
            beta: 1.1,
            profitability: 0.56,
            growth: 0.30,
            balance_sheet: 0.42,
            valuation_gap: 0.18,
            momentum: 0.46,
        },
        CompanyCandidate {
            ticker: "RTX",
            company: "RTX",
            sector: "industrials-defense",
            stage: "public",
            beta: 0.8,
            profitability: 0.46,
            growth: 0.14,
            balance_sheet: 0.36,
            valuation_gap: 0.00,
            momentum: 0.12,
        },
        CompanyCandidate {
            ticker: "LMT",
            company: "Lockheed Martin",
            sector: "industrials-defense",
            stage: "public",
            beta: 0.6,
            profitability: 0.52,
            growth: 0.08,
            balance_sheet: 0.40,
            valuation_gap: -0.05,
            momentum: 0.08,
        },
        CompanyCandidate {
            ticker: "COST",
            company: "Costco",
            sector: "consumer-retail",
            stage: "public",
            beta: 0.8,
            profitability: 0.60,
            growth: 0.26,
            balance_sheet: 0.66,
            valuation_gap: 0.28,
            momentum: 0.34,
        },
        CompanyCandidate {
            ticker: "WMT",
            company: "Walmart",
            sector: "consumer-retail",
            stage: "public",
            beta: 0.6,
            profitability: 0.48,
            growth: 0.22,
            balance_sheet: 0.58,
            valuation_gap: 0.08,
            momentum: 0.24,
        },
        CompanyCandidate {
            ticker: "HD",
            company: "Home Depot",
            sector: "consumer-housing-retail",
            stage: "public",
            beta: 1.0,
            profitability: 0.62,
            growth: 0.08,
            balance_sheet: 0.34,
            valuation_gap: 0.08,
            momentum: 0.06,
        },
        CompanyCandidate {
            ticker: "MCD",
            company: "McDonald's",
            sector: "consumer-staples-restaurants",
            stage: "public",
            beta: 0.6,
            profitability: 0.66,
            growth: 0.12,
            balance_sheet: 0.28,
            valuation_gap: 0.10,
            momentum: 0.10,
        },
        CompanyCandidate {
            ticker: "PG",
            company: "Procter & Gamble",
            sector: "consumer-staples",
            stage: "public",
            beta: 0.5,
            profitability: 0.58,
            growth: 0.08,
            balance_sheet: 0.50,
            valuation_gap: 0.05,
            momentum: 0.08,
        },
        CompanyCandidate {
            ticker: "KO",
            company: "Coca-Cola",
            sector: "consumer-staples",
            stage: "public",
            beta: 0.5,
            profitability: 0.60,
            growth: 0.10,
            balance_sheet: 0.46,
            valuation_gap: 0.04,
            momentum: 0.08,
        },
        CompanyCandidate {
            ticker: "PLD",
            company: "Prologis",
            sector: "real-estate-industrial",
            stage: "public",
            beta: 1.0,
            profitability: 0.42,
            growth: 0.14,
            balance_sheet: 0.34,
            valuation_gap: 0.06,
            momentum: -0.08,
        },
        CompanyCandidate {
            ticker: "AMT",
            company: "American Tower",
            sector: "real-estate-infrastructure",
            stage: "public",
            beta: 0.8,
            profitability: 0.44,
            growth: 0.12,
            balance_sheet: 0.28,
            valuation_gap: 0.02,
            momentum: -0.10,
        },
        CompanyCandidate {
            ticker: "COIN",
            company: "Coinbase",
            sector: "financials-crypto",
            stage: "public",
            beta: 2.4,
            profitability: 0.30,
            growth: 0.58,
            balance_sheet: 0.42,
            valuation_gap: 0.26,
            momentum: 0.40,
        },
        CompanyCandidate {
            ticker: "RBLX",
            company: "Roblox",
            sector: "technology-consumer-platform",
            stage: "public",
            beta: 1.6,
            profitability: -0.22,
            growth: 0.28,
            balance_sheet: 0.16,
            valuation_gap: 0.32,
            momentum: -0.12,
        },
        CompanyCandidate {
            ticker: "OPENAI-PRIVATE",
            company: "OpenAI",
            sector: "technology-artificial-intelligence",
            stage: "late-private",
            beta: 1.8,
            profitability: -0.10,
            growth: 0.98,
            balance_sheet: 0.34,
            valuation_gap: 0.55,
            momentum: 0.70,
        },
        CompanyCandidate {
            ticker: "ANTHROPIC-PRIVATE",
            company: "Anthropic",
            sector: "technology-artificial-intelligence",
            stage: "late-private",
            beta: 1.7,
            profitability: -0.18,
            growth: 0.92,
            balance_sheet: 0.30,
            valuation_gap: 0.46,
            momentum: 0.66,
        },
        CompanyCandidate {
            ticker: "DATABRICKS-PRIVATE",
            company: "Databricks",
            sector: "technology-data-infrastructure",
            stage: "late-private",
            beta: 1.5,
            profitability: 0.08,
            growth: 0.72,
            balance_sheet: 0.38,
            valuation_gap: 0.30,
            momentum: 0.52,
        },
        CompanyCandidate {
            ticker: "STRIPE-PRIVATE",
            company: "Stripe",
            sector: "financials-payments-fintech",
            stage: "late-private",
            beta: 1.3,
            profitability: 0.14,
            growth: 0.50,
            balance_sheet: 0.42,
            valuation_gap: 0.20,
            momentum: 0.34,
        },
        CompanyCandidate {
            ticker: "ANDURIL-PRIVATE",
            company: "Anduril",
            sector: "industrials-defense-automation",
            stage: "late-private",
            beta: 1.4,
            profitability: -0.05,
            growth: 0.68,
            balance_sheet: 0.32,
            valuation_gap: 0.24,
            momentum: 0.48,
        },
        CompanyCandidate {
            ticker: "CFS-PRIVATE",
            company: "Commonwealth Fusion Systems",
            sector: "energy-climate",
            stage: "growth-private",
            beta: 1.8,
            profitability: -0.40,
            growth: 0.84,
            balance_sheet: 0.18,
            valuation_gap: 0.42,
            momentum: 0.42,
        },
    ]
}

fn commodity_candidates() -> Vec<CommodityCandidate> {
    vec![
        CommodityCandidate {
            instrument_id: "CL=F",
            commodity: "WTI crude oil",
            commodity_class: "energy",
            supply_tightness: 0.18,
            demand_growth: 0.12,
            inventory_pressure: 0.08,
            carry: -0.02,
            geopolitical_risk: 0.38,
            valuation_gap: 0.02,
            volatility: 0.30,
        },
        CommodityCandidate {
            instrument_id: "BZ=F",
            commodity: "Brent crude oil",
            commodity_class: "energy",
            supply_tightness: 0.16,
            demand_growth: 0.12,
            inventory_pressure: 0.06,
            carry: -0.02,
            geopolitical_risk: 0.42,
            valuation_gap: 0.03,
            volatility: 0.28,
        },
        CommodityCandidate {
            instrument_id: "NG=F",
            commodity: "Natural gas",
            commodity_class: "energy",
            supply_tightness: -0.10,
            demand_growth: 0.18,
            inventory_pressure: -0.12,
            carry: -0.05,
            geopolitical_risk: 0.24,
            valuation_gap: -0.12,
            volatility: 0.58,
        },
        CommodityCandidate {
            instrument_id: "RB=F",
            commodity: "Gasoline",
            commodity_class: "energy-refined",
            supply_tightness: 0.12,
            demand_growth: 0.08,
            inventory_pressure: 0.10,
            carry: -0.04,
            geopolitical_risk: 0.22,
            valuation_gap: 0.04,
            volatility: 0.34,
        },
        CommodityCandidate {
            instrument_id: "HO=F",
            commodity: "Heating oil",
            commodity_class: "energy-refined",
            supply_tightness: 0.10,
            demand_growth: 0.06,
            inventory_pressure: 0.06,
            carry: -0.03,
            geopolitical_risk: 0.24,
            valuation_gap: 0.02,
            volatility: 0.32,
        },
        CommodityCandidate {
            instrument_id: "LNG",
            commodity: "Liquefied natural gas",
            commodity_class: "energy",
            supply_tightness: 0.14,
            demand_growth: 0.28,
            inventory_pressure: 0.08,
            carry: -0.06,
            geopolitical_risk: 0.32,
            valuation_gap: 0.08,
            volatility: 0.46,
        },
        CommodityCandidate {
            instrument_id: "U3O8",
            commodity: "Uranium",
            commodity_class: "energy",
            supply_tightness: 0.46,
            demand_growth: 0.34,
            inventory_pressure: 0.30,
            carry: 0.02,
            geopolitical_risk: 0.28,
            valuation_gap: 0.22,
            volatility: 0.42,
        },
        CommodityCandidate {
            instrument_id: "THERMAL-COAL",
            commodity: "Thermal coal",
            commodity_class: "energy",
            supply_tightness: -0.08,
            demand_growth: -0.12,
            inventory_pressure: -0.04,
            carry: -0.02,
            geopolitical_risk: 0.18,
            valuation_gap: -0.18,
            volatility: 0.30,
        },
        CommodityCandidate {
            instrument_id: "GC=F",
            commodity: "Gold",
            commodity_class: "precious-metals",
            supply_tightness: 0.10,
            demand_growth: 0.12,
            inventory_pressure: 0.08,
            carry: -0.03,
            geopolitical_risk: 0.30,
            valuation_gap: 0.10,
            volatility: 0.18,
        },
        CommodityCandidate {
            instrument_id: "SI=F",
            commodity: "Silver",
            commodity_class: "precious-industrial-metals",
            supply_tightness: 0.20,
            demand_growth: 0.22,
            inventory_pressure: 0.16,
            carry: -0.03,
            geopolitical_risk: 0.22,
            valuation_gap: 0.06,
            volatility: 0.28,
        },
        CommodityCandidate {
            instrument_id: "PL=F",
            commodity: "Platinum",
            commodity_class: "precious-industrial-metals",
            supply_tightness: 0.18,
            demand_growth: 0.06,
            inventory_pressure: 0.12,
            carry: -0.03,
            geopolitical_risk: 0.24,
            valuation_gap: -0.08,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "PA=F",
            commodity: "Palladium",
            commodity_class: "precious-industrial-metals",
            supply_tightness: -0.06,
            demand_growth: -0.12,
            inventory_pressure: -0.04,
            carry: -0.02,
            geopolitical_risk: 0.26,
            valuation_gap: -0.24,
            volatility: 0.40,
        },
        CommodityCandidate {
            instrument_id: "HG=F",
            commodity: "Copper",
            commodity_class: "industrial-metals",
            supply_tightness: 0.28,
            demand_growth: 0.30,
            inventory_pressure: 0.22,
            carry: -0.01,
            geopolitical_risk: 0.18,
            valuation_gap: 0.12,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "ALUMINUM",
            commodity: "Aluminum",
            commodity_class: "industrial-metals",
            supply_tightness: 0.06,
            demand_growth: 0.16,
            inventory_pressure: 0.04,
            carry: -0.01,
            geopolitical_risk: 0.14,
            valuation_gap: -0.02,
            volatility: 0.22,
        },
        CommodityCandidate {
            instrument_id: "NICKEL",
            commodity: "Nickel",
            commodity_class: "battery-industrial-metals",
            supply_tightness: -0.18,
            demand_growth: 0.20,
            inventory_pressure: -0.16,
            carry: -0.02,
            geopolitical_risk: 0.22,
            valuation_gap: -0.18,
            volatility: 0.38,
        },
        CommodityCandidate {
            instrument_id: "ZINC",
            commodity: "Zinc",
            commodity_class: "industrial-metals",
            supply_tightness: 0.04,
            demand_growth: 0.10,
            inventory_pressure: 0.02,
            carry: -0.01,
            geopolitical_risk: 0.12,
            valuation_gap: -0.05,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "LEAD",
            commodity: "Lead",
            commodity_class: "industrial-metals",
            supply_tightness: -0.02,
            demand_growth: 0.04,
            inventory_pressure: -0.02,
            carry: -0.01,
            geopolitical_risk: 0.10,
            valuation_gap: -0.08,
            volatility: 0.22,
        },
        CommodityCandidate {
            instrument_id: "TIN",
            commodity: "Tin",
            commodity_class: "industrial-metals",
            supply_tightness: 0.22,
            demand_growth: 0.14,
            inventory_pressure: 0.18,
            carry: -0.02,
            geopolitical_risk: 0.18,
            valuation_gap: 0.06,
            volatility: 0.32,
        },
        CommodityCandidate {
            instrument_id: "IRON-ORE",
            commodity: "Iron ore",
            commodity_class: "bulk-industrial",
            supply_tightness: -0.04,
            demand_growth: 0.06,
            inventory_pressure: -0.05,
            carry: -0.02,
            geopolitical_risk: 0.12,
            valuation_gap: -0.10,
            volatility: 0.28,
        },
        CommodityCandidate {
            instrument_id: "STEEL-HRC",
            commodity: "Hot-rolled coil steel",
            commodity_class: "bulk-industrial",
            supply_tightness: 0.02,
            demand_growth: 0.08,
            inventory_pressure: 0.00,
            carry: -0.02,
            geopolitical_risk: 0.12,
            valuation_gap: -0.06,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "LITHIUM",
            commodity: "Lithium carbonate",
            commodity_class: "battery-metals",
            supply_tightness: -0.22,
            demand_growth: 0.34,
            inventory_pressure: -0.24,
            carry: -0.03,
            geopolitical_risk: 0.20,
            valuation_gap: -0.32,
            volatility: 0.52,
        },
        CommodityCandidate {
            instrument_id: "COBALT",
            commodity: "Cobalt",
            commodity_class: "battery-metals",
            supply_tightness: -0.10,
            demand_growth: 0.16,
            inventory_pressure: -0.08,
            carry: -0.03,
            geopolitical_risk: 0.38,
            valuation_gap: -0.18,
            volatility: 0.40,
        },
        CommodityCandidate {
            instrument_id: "GRAPHITE",
            commodity: "Graphite",
            commodity_class: "battery-metals",
            supply_tightness: 0.24,
            demand_growth: 0.32,
            inventory_pressure: 0.18,
            carry: -0.02,
            geopolitical_risk: 0.34,
            valuation_gap: 0.10,
            volatility: 0.36,
        },
        CommodityCandidate {
            instrument_id: "RARE-EARTHS",
            commodity: "Rare earth basket",
            commodity_class: "battery-industrial-metals",
            supply_tightness: 0.30,
            demand_growth: 0.26,
            inventory_pressure: 0.20,
            carry: -0.03,
            geopolitical_risk: 0.42,
            valuation_gap: 0.18,
            volatility: 0.44,
        },
        CommodityCandidate {
            instrument_id: "CORN",
            commodity: "Corn",
            commodity_class: "agriculture-food",
            supply_tightness: -0.04,
            demand_growth: 0.08,
            inventory_pressure: -0.02,
            carry: -0.01,
            geopolitical_risk: 0.14,
            valuation_gap: -0.10,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "WHEAT",
            commodity: "Wheat",
            commodity_class: "agriculture-food",
            supply_tightness: 0.10,
            demand_growth: 0.08,
            inventory_pressure: 0.08,
            carry: -0.01,
            geopolitical_risk: 0.30,
            valuation_gap: -0.04,
            volatility: 0.30,
        },
        CommodityCandidate {
            instrument_id: "SOYBEANS",
            commodity: "Soybeans",
            commodity_class: "agriculture-food",
            supply_tightness: 0.02,
            demand_growth: 0.10,
            inventory_pressure: 0.00,
            carry: -0.01,
            geopolitical_risk: 0.16,
            valuation_gap: -0.06,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "SOYMEAL",
            commodity: "Soybean meal",
            commodity_class: "agriculture-food",
            supply_tightness: 0.04,
            demand_growth: 0.12,
            inventory_pressure: 0.04,
            carry: -0.01,
            geopolitical_risk: 0.14,
            valuation_gap: -0.04,
            volatility: 0.22,
        },
        CommodityCandidate {
            instrument_id: "SOYOIL",
            commodity: "Soybean oil",
            commodity_class: "agriculture-food-energy",
            supply_tightness: 0.12,
            demand_growth: 0.16,
            inventory_pressure: 0.08,
            carry: -0.01,
            geopolitical_risk: 0.14,
            valuation_gap: 0.02,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "RICE",
            commodity: "Rice",
            commodity_class: "agriculture-food",
            supply_tightness: 0.08,
            demand_growth: 0.06,
            inventory_pressure: 0.06,
            carry: 0.00,
            geopolitical_risk: 0.12,
            valuation_gap: 0.00,
            volatility: 0.18,
        },
        CommodityCandidate {
            instrument_id: "OATS",
            commodity: "Oats",
            commodity_class: "agriculture-food",
            supply_tightness: -0.02,
            demand_growth: 0.04,
            inventory_pressure: -0.03,
            carry: -0.01,
            geopolitical_risk: 0.08,
            valuation_gap: -0.08,
            volatility: 0.22,
        },
        CommodityCandidate {
            instrument_id: "COFFEE",
            commodity: "Coffee",
            commodity_class: "agriculture-softs",
            supply_tightness: 0.22,
            demand_growth: 0.12,
            inventory_pressure: 0.16,
            carry: -0.02,
            geopolitical_risk: 0.16,
            valuation_gap: 0.12,
            volatility: 0.34,
        },
        CommodityCandidate {
            instrument_id: "COCOA",
            commodity: "Cocoa",
            commodity_class: "agriculture-softs",
            supply_tightness: 0.42,
            demand_growth: 0.10,
            inventory_pressure: 0.36,
            carry: -0.03,
            geopolitical_risk: 0.24,
            valuation_gap: 0.34,
            volatility: 0.50,
        },
        CommodityCandidate {
            instrument_id: "SUGAR",
            commodity: "Sugar",
            commodity_class: "agriculture-softs",
            supply_tightness: 0.10,
            demand_growth: 0.08,
            inventory_pressure: 0.06,
            carry: -0.01,
            geopolitical_risk: 0.12,
            valuation_gap: 0.00,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "COTTON",
            commodity: "Cotton",
            commodity_class: "agriculture-softs",
            supply_tightness: -0.06,
            demand_growth: 0.04,
            inventory_pressure: -0.04,
            carry: -0.01,
            geopolitical_risk: 0.08,
            valuation_gap: -0.12,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "ORANGE-JUICE",
            commodity: "Frozen concentrated orange juice",
            commodity_class: "agriculture-softs",
            supply_tightness: 0.34,
            demand_growth: 0.02,
            inventory_pressure: 0.28,
            carry: -0.02,
            geopolitical_risk: 0.12,
            valuation_gap: 0.30,
            volatility: 0.48,
        },
        CommodityCandidate {
            instrument_id: "LIVE-CATTLE",
            commodity: "Live cattle",
            commodity_class: "livestock-food",
            supply_tightness: 0.26,
            demand_growth: 0.08,
            inventory_pressure: 0.20,
            carry: 0.00,
            geopolitical_risk: 0.08,
            valuation_gap: 0.14,
            volatility: 0.20,
        },
        CommodityCandidate {
            instrument_id: "LEAN-HOGS",
            commodity: "Lean hogs",
            commodity_class: "livestock-food",
            supply_tightness: -0.04,
            demand_growth: 0.06,
            inventory_pressure: -0.04,
            carry: 0.00,
            geopolitical_risk: 0.06,
            valuation_gap: -0.08,
            volatility: 0.28,
        },
        CommodityCandidate {
            instrument_id: "LUMBER",
            commodity: "Lumber",
            commodity_class: "housing-industrial",
            supply_tightness: -0.12,
            demand_growth: 0.02,
            inventory_pressure: -0.10,
            carry: -0.02,
            geopolitical_risk: 0.08,
            valuation_gap: -0.18,
            volatility: 0.44,
        },
        CommodityCandidate {
            instrument_id: "RUBBER",
            commodity: "Rubber",
            commodity_class: "industrial-agriculture",
            supply_tightness: 0.02,
            demand_growth: 0.08,
            inventory_pressure: 0.00,
            carry: -0.02,
            geopolitical_risk: 0.10,
            valuation_gap: -0.04,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "PALM-OIL",
            commodity: "Palm oil",
            commodity_class: "agriculture-food-energy",
            supply_tightness: 0.10,
            demand_growth: 0.14,
            inventory_pressure: 0.08,
            carry: -0.01,
            geopolitical_risk: 0.12,
            valuation_gap: 0.02,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "CANOLA",
            commodity: "Canola",
            commodity_class: "agriculture-food-energy",
            supply_tightness: 0.04,
            demand_growth: 0.10,
            inventory_pressure: 0.02,
            carry: -0.01,
            geopolitical_risk: 0.10,
            valuation_gap: -0.03,
            volatility: 0.22,
        },
        CommodityCandidate {
            instrument_id: "MILK",
            commodity: "Class III milk",
            commodity_class: "dairy-food",
            supply_tightness: -0.02,
            demand_growth: 0.04,
            inventory_pressure: -0.02,
            carry: 0.00,
            geopolitical_risk: 0.04,
            valuation_gap: -0.04,
            volatility: 0.18,
        },
        CommodityCandidate {
            instrument_id: "BUTTER",
            commodity: "Butter",
            commodity_class: "dairy-food",
            supply_tightness: 0.06,
            demand_growth: 0.04,
            inventory_pressure: 0.04,
            carry: 0.00,
            geopolitical_risk: 0.04,
            valuation_gap: 0.02,
            volatility: 0.20,
        },
        CommodityCandidate {
            instrument_id: "CARBON-EUA",
            commodity: "EU carbon allowances",
            commodity_class: "carbon",
            supply_tightness: 0.18,
            demand_growth: 0.18,
            inventory_pressure: 0.12,
            carry: 0.02,
            geopolitical_risk: 0.10,
            valuation_gap: 0.06,
            volatility: 0.30,
        },
        CommodityCandidate {
            instrument_id: "CARBON-CCA",
            commodity: "California carbon allowances",
            commodity_class: "carbon",
            supply_tightness: 0.10,
            demand_growth: 0.12,
            inventory_pressure: 0.08,
            carry: 0.02,
            geopolitical_risk: 0.06,
            valuation_gap: 0.02,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "FREIGHT-BDI",
            commodity: "Dry bulk freight",
            commodity_class: "freight-industrial",
            supply_tightness: 0.06,
            demand_growth: 0.14,
            inventory_pressure: 0.02,
            carry: -0.02,
            geopolitical_risk: 0.18,
            valuation_gap: -0.02,
            volatility: 0.46,
        },
        CommodityCandidate {
            instrument_id: "POTASH",
            commodity: "Potash",
            commodity_class: "fertilizer-agriculture",
            supply_tightness: 0.08,
            demand_growth: 0.10,
            inventory_pressure: 0.04,
            carry: -0.01,
            geopolitical_risk: 0.24,
            valuation_gap: -0.02,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "PHOSPHATE",
            commodity: "Phosphate",
            commodity_class: "fertilizer-agriculture",
            supply_tightness: 0.10,
            demand_growth: 0.08,
            inventory_pressure: 0.06,
            carry: -0.01,
            geopolitical_risk: 0.22,
            valuation_gap: 0.00,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "UREA",
            commodity: "Urea",
            commodity_class: "fertilizer-agriculture-energy",
            supply_tightness: 0.02,
            demand_growth: 0.08,
            inventory_pressure: 0.00,
            carry: -0.02,
            geopolitical_risk: 0.22,
            valuation_gap: -0.08,
            volatility: 0.30,
        },
        CommodityCandidate {
            instrument_id: "AMMONIA",
            commodity: "Ammonia",
            commodity_class: "fertilizer-energy",
            supply_tightness: 0.00,
            demand_growth: 0.08,
            inventory_pressure: -0.02,
            carry: -0.02,
            geopolitical_risk: 0.20,
            valuation_gap: -0.08,
            volatility: 0.30,
        },
        CommodityCandidate {
            instrument_id: "ETHANOL",
            commodity: "Ethanol",
            commodity_class: "energy-agriculture",
            supply_tightness: -0.04,
            demand_growth: 0.08,
            inventory_pressure: -0.04,
            carry: -0.01,
            geopolitical_risk: 0.08,
            valuation_gap: -0.10,
            volatility: 0.24,
        },
        CommodityCandidate {
            instrument_id: "METHANOL",
            commodity: "Methanol",
            commodity_class: "chemicals-energy",
            supply_tightness: -0.02,
            demand_growth: 0.10,
            inventory_pressure: -0.02,
            carry: -0.02,
            geopolitical_risk: 0.10,
            valuation_gap: -0.08,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "POLYETHYLENE",
            commodity: "Polyethylene",
            commodity_class: "chemicals-industrial",
            supply_tightness: -0.10,
            demand_growth: 0.06,
            inventory_pressure: -0.08,
            carry: -0.02,
            geopolitical_risk: 0.08,
            valuation_gap: -0.12,
            volatility: 0.20,
        },
        CommodityCandidate {
            instrument_id: "PROPANE",
            commodity: "Propane",
            commodity_class: "energy",
            supply_tightness: 0.00,
            demand_growth: 0.08,
            inventory_pressure: -0.02,
            carry: -0.02,
            geopolitical_risk: 0.14,
            valuation_gap: -0.08,
            volatility: 0.34,
        },
        CommodityCandidate {
            instrument_id: "JET-FUEL",
            commodity: "Jet fuel",
            commodity_class: "energy-refined",
            supply_tightness: 0.08,
            demand_growth: 0.14,
            inventory_pressure: 0.04,
            carry: -0.03,
            geopolitical_risk: 0.16,
            valuation_gap: 0.00,
            volatility: 0.30,
        },
        CommodityCandidate {
            instrument_id: "NAPHTHA",
            commodity: "Naphtha",
            commodity_class: "energy-chemicals",
            supply_tightness: -0.02,
            demand_growth: 0.10,
            inventory_pressure: -0.02,
            carry: -0.02,
            geopolitical_risk: 0.12,
            valuation_gap: -0.06,
            volatility: 0.26,
        },
        CommodityCandidate {
            instrument_id: "SUSTAINABLE-AVIATION-FUEL",
            commodity: "Sustainable aviation fuel credits",
            commodity_class: "energy-carbon",
            supply_tightness: 0.24,
            demand_growth: 0.34,
            inventory_pressure: 0.18,
            carry: 0.01,
            geopolitical_risk: 0.08,
            valuation_gap: 0.18,
            volatility: 0.38,
        },
        CommodityCandidate {
            instrument_id: "REC",
            commodity: "Renewable energy certificates",
            commodity_class: "carbon-energy",
            supply_tightness: 0.08,
            demand_growth: 0.16,
            inventory_pressure: 0.04,
            carry: 0.01,
            geopolitical_risk: 0.06,
            valuation_gap: 0.02,
            volatility: 0.22,
        },
        CommodityCandidate {
            instrument_id: "WATER-RIGHTS",
            commodity: "Water rights proxy",
            commodity_class: "scarcity-resource",
            supply_tightness: 0.28,
            demand_growth: 0.18,
            inventory_pressure: 0.20,
            carry: 0.00,
            geopolitical_risk: 0.10,
            valuation_gap: 0.16,
            volatility: 0.30,
        },
    ]
}

fn analyze_sentiment(
    config: &Config,
    request: SentimentAnalyzeRequest,
) -> Result<SentimentAnalyzeResponse, String> {
    if let Some(schema) = request.schema_version.as_deref() {
        if schema != SCHEMA_VERSION {
            return Err(format!("schemaVersion must be {SCHEMA_VERSION}"));
        }
    }
    if request.documents.is_empty() {
        return Err("documents must contain at least one item".to_string());
    }
    if request.documents.len() > MAX_SENTIMENT_DOCUMENTS {
        return Err(format!(
            "documents must contain at most {MAX_SENTIMENT_DOCUMENTS} items"
        ));
    }
    if let Some(instrument_ids) = request.instrument_ids.as_ref() {
        if instrument_ids.len() > MAX_SENTIMENT_CONTEXT_SCORES {
            return Err(format!(
                "instrumentIds must contain at most {MAX_SENTIMENT_CONTEXT_SCORES} items"
            ));
        }
        for instrument_id in instrument_ids {
            clean_token(instrument_id, "instrumentIds[]")?;
        }
    }

    let request_id = request_id(request.request_id.as_ref(), "sentiment-analyze");
    let mut weighted_sum = 0.0;
    let mut weight_total = 0.0;
    let mut source_totals: BTreeMap<String, (usize, f64, f64)> = BTreeMap::new();
    let mut term_counts: BTreeMap<String, usize> = BTreeMap::new();

    for document in &request.documents {
        let source = clean_token(&document.source, "documents[].source")?;
        let text = document.text.trim();
        if text.is_empty() {
            return Err("documents[].text must not be empty".to_string());
        }
        if text.len() > MAX_SENTIMENT_TEXT_BYTES {
            return Err(format!(
                "documents[].text must be at most {MAX_SENTIMENT_TEXT_BYTES} bytes"
            ));
        }
        clean_optional_token(&document.author, "documents[].author")?;
        clean_optional_token(&document.published_at, "documents[].publishedAt")?;
        if let Some(url) = document.url.as_deref() {
            if url.len() > MAX_URL_LEN || url.chars().any(char::is_control) {
                return Err(format!(
                    "documents[].url must be at most {MAX_URL_LEN} bytes and contain no control characters"
                ));
            }
        }
        let weight = document
            .weight
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(1.0)
            .min(25.0);
        let score = score_sentiment_text(text);
        weighted_sum += score * weight;
        weight_total += weight;
        let entry = source_totals.entry(source).or_insert((0, 0.0, 0.0));
        entry.0 += 1;
        entry.1 += score * weight;
        entry.2 += weight;
        collect_sentiment_terms(text, &mut term_counts);
    }

    let average_sentiment = weighted_sum / weight_total.max(f64::EPSILON);
    let source_scores = source_totals
        .into_iter()
        .map(|(source, (document_count, score_sum, source_weight))| {
            let average = score_sum / source_weight.max(f64::EPSILON);
            SentimentSourceScore {
                source,
                document_count,
                average_sentiment: round6(average),
                confidence: round6(sentiment_confidence(document_count, average)),
            }
        })
        .collect::<Vec<_>>();
    let mut terms = term_counts.into_iter().collect::<Vec<_>>();
    terms.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let top_terms = terms
        .into_iter()
        .take(16)
        .map(|(term, _)| term)
        .collect::<Vec<_>>();

    Ok(SentimentAnalyzeResponse {
        ok: true,
        request_id,
        schema_version: SCHEMA_VERSION,
        query: request.query,
        document_count: request.documents.len(),
        average_sentiment: round6(average_sentiment),
        confidence: round6(sentiment_confidence(
            request.documents.len(),
            average_sentiment,
        )),
        source_scores,
        top_terms,
        credential_status: config.sentiment_credentials.clone(),
        generated_at_ms: now_ms(),
    })
}

fn score_sentiment_text(text: &str) -> f64 {
    let lower = text.to_ascii_lowercase();
    let positive = [
        "beat",
        "bull",
        "bullish",
        "breakout",
        "growth",
        "upgrade",
        "surge",
        "rally",
        "accumulate",
        "strong",
        "resilient",
        "expansion",
        "demand",
        "profit",
        "record",
        "approval",
        "adoption",
        "inflow",
        "soft landing",
    ];
    let negative = [
        "miss",
        "bear",
        "bearish",
        "crash",
        "recession",
        "downgrade",
        "default",
        "fraud",
        "lawsuit",
        "weak",
        "shortage",
        "glut",
        "outflow",
        "layoff",
        "bankruptcy",
        "tariff",
        "war",
        "inflation shock",
        "liquidity crunch",
    ];
    let pos = positive
        .iter()
        .filter(|term| lower.contains(**term))
        .count() as f64;
    let neg = negative
        .iter()
        .filter(|term| lower.contains(**term))
        .count() as f64;
    if pos == 0.0 && neg == 0.0 {
        0.0
    } else {
        clamp((pos - neg) / (pos + neg + 1.0), -1.0, 1.0)
    }
}

fn collect_sentiment_terms(text: &str, counts: &mut BTreeMap<String, usize>) {
    for raw in text.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '$' && ch != '#') {
        let token = raw.trim().to_ascii_lowercase();
        if token.len() < 3 || token.len() > 32 {
            continue;
        }
        if matches!(
            token.as_str(),
            "the" | "and" | "for" | "this" | "that" | "with" | "from" | "market" | "price"
        ) {
            continue;
        }
        *counts.entry(token).or_insert(0) += 1;
    }
}

fn sentiment_confidence(document_count: usize, average_sentiment: f64) -> f64 {
    clamp(
        0.25 + (document_count as f64).ln_1p() / 8.0 + average_sentiment.abs() * 0.35,
        0.0,
        0.95,
    )
}

async fn publish_forecast(state: &AppState, response: &ForecastResponse) {
    let Some(nats) = state.nats.as_ref() else {
        return;
    };
    let payload = match serde_json::to_vec(&json!({
        "messageKind": "economics.forecast.result",
        "source": SERVICE_NAME,
        "result": response,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            emit_log(
                "ERROR",
                "economics.forecast.result.encode.error",
                "failed to encode economics forecast result",
                json!({
                    "error": error_summary(&error.to_string()),
                    "requestId": &response.request_id
                }),
            );
            return;
        }
    };
    if nats
        .publish(state.config.result_subject.clone(), payload.into())
        .await
        .is_ok()
    {
        state
            .metrics
            .nats_published_total
            .fetch_add(1, Ordering::Relaxed);
    }
    let _ = nats
        .publish(
            state.config.runtime_event_subject.clone(),
            json!({
                "type": "economics.forecast",
                "source": SERVICE_NAME,
                "requestId": response.request_id,
                "projectionCount": response.projections.len(),
                "scenario": response.scenario,
                "atMs": now_ms()
            })
            .to_string()
            .into(),
        )
        .await;
}

async fn publish_market_event(state: &AppState, event: Value) {
    let Some(nats) = state.nats.as_ref() else {
        return;
    };
    let _ = nats
        .publish(
            state.config.market_event_subject.clone(),
            event.to_string().into(),
        )
        .await;
}

async fn run_nats_loop(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        emit_log(
            "INFO",
            "economics.nats.loop.disabled",
            "economics NATS loop disabled",
            json!({
                "reason": "NATS_URL is not configured"
            }),
        );
        return;
    };
    emit_log(
        "INFO",
        "economics.nats.loop.start",
        "economics NATS loop starting",
        json!({
            "requestSubject": state.config.request_subject,
            "queueGroup": state.config.queue_group,
            "resultSubject": state.config.result_subject
        }),
    );
    let mut subscription = match nats
        .queue_subscribe(
            state.config.request_subject.clone(),
            state.config.queue_group.clone(),
        )
        .await
    {
        Ok(subscription) => subscription,
        Err(error) => {
            emit_log(
                "ERROR",
                "economics.nats.subscribe.error",
                "economics NATS subscribe failed",
                json!({
                    "error": error_summary(&error.to_string()),
                    "requestSubject": state.config.request_subject,
                    "queueGroup": state.config.queue_group
                }),
            );
            return;
        }
    };
    while let Some(message) = subscription.next().await {
        state
            .metrics
            .nats_messages_total
            .fetch_add(1, Ordering::Relaxed);
        let payload = message.payload.to_vec();
        if payload.len() > MAX_NATS_PAYLOAD_BYTES {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            emit_log(
                "WARN",
                "economics.nats.request.oversize",
                "economics NATS forecast request rejected because payload is too large",
                json!({
                    "bytes": payload.len(),
                    "maxBytes": MAX_NATS_PAYLOAD_BYTES
                }),
            );
            continue;
        }
        let task_state = state.clone();
        tokio::spawn(async move {
            match serde_json::from_slice::<ForecastRequest>(&payload) {
                Ok(request) => match forecast_from_request(&task_state, request) {
                    Ok(response) => {
                        task_state
                            .metrics
                            .forecasts_total
                            .fetch_add(1, Ordering::Relaxed);
                        publish_forecast(&task_state, &response).await;
                    }
                    Err(error) => {
                        task_state
                            .metrics
                            .errors_total
                            .fetch_add(1, Ordering::Relaxed);
                        emit_log(
                            "ERROR",
                            "economics.nats.forecast.error",
                            "economics NATS forecast failed",
                            json!({
                                "error": error_summary(&error)
                            }),
                        );
                    }
                },
                Err(error) => {
                    task_state
                        .metrics
                        .errors_total
                        .fetch_add(1, Ordering::Relaxed);
                    emit_log(
                        "WARN",
                        "economics.nats.request.invalid",
                        "economics NATS forecast request was invalid JSON",
                        json!({
                            "error": error_summary(&error.to_string())
                        }),
                    );
                }
            }
        });
    }
}

async fn root() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

async fn descriptor(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(service_descriptor(&state))
}

async fn dashboard_json(State(state): State<AppState>, headers: HeaderMap) -> Response {
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

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
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

async fn readyz(State(state): State<AppState>) -> Response {
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

async fn schema() -> impl IntoResponse {
    Json(schema_descriptor())
}

async fn example() -> impl IntoResponse {
    Json(example_request())
}

async fn equations() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "equations": equation_catalog(),
        "desEngine": des_surface_descriptor()
    }))
}

async fn sources() -> impl IntoResponse {
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

async fn public_sources(State(state): State<AppState>) -> impl IntoResponse {
    Json(public_source_catalog_payload(&state.config))
}

async fn sentiment_sources(State(state): State<AppState>) -> impl IntoResponse {
    Json(sentiment_source_catalog(
        &state.config.sentiment_credentials,
    ))
}

async fn macro_indicators(State(state): State<AppState>) -> impl IntoResponse {
    Json(macro_indicator_payload(&state.config))
}

async fn vc_investment(State(state): State<AppState>) -> impl IntoResponse {
    Json(vc_investment_payload(&state.config))
}

async fn pipeline_catalog(State(state): State<AppState>) -> impl IntoResponse {
    Json(pipeline_catalog_payload(&state))
}

async fn integrations_health(State(state): State<AppState>) -> impl IntoResponse {
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

async fn hardening_audit(State(state): State<AppState>) -> impl IntoResponse {
    Json(hardening_audit_payload(&state))
}

async fn observability(State(state): State<AppState>) -> impl IntoResponse {
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

async fn des_engine_descriptor() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "surface": des_surface_descriptor(),
        "serviceDescriptor": des_service_descriptor()
    }))
}

async fn sentiment_analyze_http(
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

async fn recommendations_http(
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

async fn pipeline_plan_http(
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

async fn pipeline_submit_http(
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

async fn forecast_http(
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

async fn pull_source_http(
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

async fn pull_source(state: &AppState, request: ApiPullRequest) -> Result<ApiPullResponse, String> {
    let mut request = request;
    let source_template = apply_public_source_template(&mut request)?;
    validate_api_pull_request(&request, source_template.as_ref())?;
    let url = request.url.as_deref().ok_or_else(|| {
        "url is required unless sourceId names a public source template".to_string()
    })?;
    let parsed_url =
        reqwest::Url::parse(url.trim()).map_err(|error| format!("url is invalid: {error}"))?;
    if let Some(template) = source_template.as_ref() {
        validate_public_source_url(&parsed_url, template)?;
    } else {
        validate_source_url_for_config(&parsed_url, &state.config)?;
    }
    let mut http_request = state.http.get(parsed_url.clone());
    if let Some(env_name) = request.auth_header_env.as_deref() {
        let env_name = validate_source_auth_env(&state.config, env_name)?;
        let header_value = optional_env(&env_name)
            .ok_or_else(|| format!("auth header env var {env_name} is not configured"))?;
        let header_name = validate_source_auth_header_name(
            request
                .auth_header_name
                .as_deref()
                .unwrap_or("authorization"),
        )?;
        let header_value = reqwest::header::HeaderValue::from_str(&header_value)
            .map_err(|_| "auth header value contains invalid bytes".to_string())?;
        http_request = http_request.header(header_name, header_value);
    }
    let response = http_request
        .send()
        .await
        .map_err(|error| format!("source fetch failed: {error}"))?;
    let status = response.status();
    if let Some(len) = response.content_length() {
        if len as usize > MAX_SOURCE_FETCH_BYTES {
            return Err(format!(
                "source response is too large: {len} bytes, max {MAX_SOURCE_FETCH_BYTES}"
            ));
        }
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("source body read failed: {error}"))?;
    if bytes.len() > MAX_SOURCE_FETCH_BYTES {
        return Err(format!(
            "source response is too large: {} bytes, max {MAX_SOURCE_FETCH_BYTES}",
            bytes.len()
        ));
    }
    if !status.is_success() {
        return Err(format!("source returned HTTP {status}"));
    }
    let request_id = request_id(request.request_id.as_ref(), "source-pull");
    let mut stored_points = 0usize;
    let mut warnings = Vec::new();
    let mut instrument_id = request.instrument_id.clone();
    let mut quality = None;
    let parser = request.parser;
    let should_parse = parser.is_some()
        || (request.instrument_id.is_some()
            && request.asset_class.is_some()
            && request.date_field.is_some()
            && request.price_field.is_some());
    if should_parse {
        let (series, report) = series_from_bytes(&request, &bytes)?;
        stored_points = series.observations.len();
        instrument_id = Some(series.instrument_id.clone());
        validate_series(std::slice::from_ref(&series))?;
        quality = Some(report);
        let mut store = state
            .series_store
            .write()
            .map_err(|_| "series store lock poisoned".to_string())?;
        store.insert(series.instrument_id.clone(), series);
    } else {
        warnings.push(
            "response fetched but not stored; provide sourceId or instrumentId, assetClass, parser, and field/index metadata to parse a series"
                .to_string(),
        );
    }
    let host = parsed_url.host_str().unwrap_or("unknown").to_string();
    let response = ApiPullResponse {
        ok: true,
        request_id,
        source_id: request.source_id.clone(),
        source: request
            .source
            .unwrap_or_else(|| "ad-hoc-api".to_string())
            .chars()
            .take(MAX_TOKEN_LEN)
            .collect(),
        parser,
        url_host: host,
        http_status: status.as_u16(),
        bytes: bytes.len(),
        stored_points,
        instrument_id,
        quality,
        warnings,
        fetched_at_ms: now_ms(),
    };
    state
        .metrics
        .source_pull_success_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .source_pull_bytes_total
        .fetch_add(bytes.len() as u64, Ordering::Relaxed);
    state
        .metrics
        .source_pull_stored_points_total
        .fetch_add(stored_points as u64, Ordering::Relaxed);
    state
        .metrics
        .source_pull_last_success_unix_seconds
        .store(now_unix_seconds(), Ordering::Relaxed);
    emit_log(
        "INFO",
        "economics.source_pull.ok",
        "economics source pull completed",
        json!({
            "requestId": &response.request_id,
            "sourceId": &response.source_id,
            "source": &response.source,
            "urlHost": &response.url_host,
            "httpStatus": response.http_status,
            "bytes": response.bytes,
            "storedPoints": response.stored_points,
            "instrumentId": &response.instrument_id,
            "parser": &response.parser
        }),
    );
    publish_market_event(
        state,
        json!({
            "type": "economics.source_pull",
            "source": SERVICE_NAME,
            "requestId": response.request_id,
            "urlHost": response.url_host,
            "storedPoints": response.stored_points,
            "instrumentId": response.instrument_id,
            "atMs": response.fetched_at_ms
        }),
    )
    .await;
    Ok(response)
}

fn apply_public_source_template(
    request: &mut ApiPullRequest,
) -> Result<Option<PublicSourceTemplate>, String> {
    let Some(source_id) = request.source_id.as_deref() else {
        return Ok(None);
    };
    let source_id = clean_token(source_id, "sourceId")?;
    let template = public_source_template(&source_id).ok_or_else(|| {
        format!(
            "unknown sourceId {source_id}; use GET /sources/public for supported public templates"
        )
    })?;
    if request
        .url
        .as_deref()
        .map(|url| !url.trim().is_empty())
        .unwrap_or(false)
    {
        return Err("sourceId templates do not allow url overrides".to_string());
    }
    if request.auth_header_env.is_some() || request.auth_header_name.is_some() {
        return Err(
            "sourceId templates are public and do not accept auth header overrides".to_string(),
        );
    }

    request.source_id = Some(source_id);
    request.url = Some(template.url.to_string());
    request.parser.get_or_insert(template.parser);
    request
        .instrument_id
        .get_or_insert_with(|| template.instrument_id.to_string());
    request
        .display_name
        .get_or_insert_with(|| template.display_name.to_string());
    request
        .asset_class
        .get_or_insert_with(|| template.asset_class.to_string());
    request
        .currency
        .get_or_insert_with(|| template.currency.to_string());
    request
        .source
        .get_or_insert_with(|| template.source.to_string());
    if request.root_pointer.is_none() {
        request.root_pointer = template.root_pointer.map(str::to_string);
    }
    if request.date_field.is_none() {
        request.date_field = template.date_field.map(str::to_string);
    }
    if request.price_field.is_none() {
        request.price_field = template.price_field.map(str::to_string);
    }
    if request.volume_field.is_none() {
        request.volume_field = template.volume_field.map(str::to_string);
    }
    request.date_index = request.date_index.or(template.date_index);
    request.price_index = request.price_index.or(template.price_index);
    request.volume_index = request.volume_index.or(template.volume_index);
    Ok(Some(template))
}

fn validate_api_pull_request(
    request: &ApiPullRequest,
    source_template: Option<&PublicSourceTemplate>,
) -> Result<(), String> {
    clean_optional_token(&request.source_id, "sourceId")?;
    clean_optional_token(&request.instrument_id, "instrumentId")?;
    clean_optional_token(&request.display_name, "displayName")?;
    clean_optional_token(&request.asset_class, "assetClass")?;
    clean_optional_token(&request.currency, "currency")?;
    clean_optional_token(&request.source, "source")?;
    clean_optional_token(&request.date_field, "dateField")?;
    clean_optional_token(&request.price_field, "priceField")?;
    clean_optional_token(&request.volume_field, "volumeField")?;
    clean_optional_token(&request.auth_header_env, "authHeaderEnv")?;
    clean_optional_token(&request.auth_header_name, "authHeaderName")?;
    if let Some(url) = request.url.as_deref() {
        if url.trim().is_empty() || url.len() > MAX_URL_LEN || url.chars().any(char::is_control) {
            return Err(format!(
                "url must be non-empty, contain no control characters, and be at most {MAX_URL_LEN} bytes"
            ));
        }
    }
    if let Some(pointer) = request.root_pointer.as_deref() {
        validate_json_pointer(pointer, "rootPointer")?;
    }
    for (label, index) in [
        ("dateIndex", request.date_index),
        ("priceIndex", request.price_index),
        ("volumeIndex", request.volume_index),
    ] {
        if let Some(index) = index {
            if index > 16 {
                return Err(format!("{label} must be between 0 and 16"));
            }
        }
    }
    if source_template.is_none() && request.source_id.is_some() {
        return Err("sourceId did not resolve to a public source template".to_string());
    }
    Ok(())
}

fn validate_json_pointer(pointer: &str, label: &str) -> Result<(), String> {
    let trimmed = pointer.trim();
    if trimmed.len() > MAX_JSON_POINTER_LEN || trimmed.chars().any(char::is_control) {
        return Err(format!(
            "{label} must contain no control characters and be at most {MAX_JSON_POINTER_LEN} bytes"
        ));
    }
    if !trimmed.is_empty() && !trimmed.starts_with('/') {
        return Err(format!("{label} must be a JSON pointer starting with /"));
    }
    Ok(())
}

fn validate_public_source_url(
    url: &reqwest::Url,
    template: &PublicSourceTemplate,
) -> Result<(), String> {
    validate_source_url(url, false)?;
    let host = url
        .host_str()
        .ok_or_else(|| "source URL must include a host".to_string())?
        .to_ascii_lowercase();
    if host != template.host {
        return Err(format!(
            "sourceId {} must resolve to host {}",
            template.id, template.host
        ));
    }
    Ok(())
}

fn validate_source_url_for_config(url: &reqwest::Url, config: &Config) -> Result<(), String> {
    validate_source_url(url, config.allow_private_source_urls)?;
    validate_source_host_allowlist(url, &config.allowed_source_hosts)
}

fn validate_source_url(url: &reqwest::Url, allow_private: bool) -> Result<(), String> {
    if url.as_str().len() > MAX_URL_LEN || url.as_str().chars().any(char::is_control) {
        return Err(format!(
            "source URL must contain no control characters and be at most {MAX_URL_LEN} bytes"
        ));
    }
    if url.fragment().is_some() {
        return Err("source URL fragments are not allowed".to_string());
    }
    match url.scheme() {
        "https" => {}
        "http" if allow_private => {}
        "http" => {
            return Err(
                "http source URLs require ECONOMICS_ALLOW_PRIVATE_SOURCE_URLS=true".to_string(),
            );
        }
        other => return Err(format!("unsupported source URL scheme {other}")),
    }
    let host = url
        .host_str()
        .ok_or_else(|| "source URL must include a host".to_string())?
        .to_ascii_lowercase();
    if source_host_is_private(&host) && !allow_private {
        return Err(
            "private source hosts require ECONOMICS_ALLOW_PRIVATE_SOURCE_URLS=true".to_string(),
        );
    }
    if url.port().is_some() && !allow_private {
        return Err(
            "custom source URL ports require ECONOMICS_ALLOW_PRIVATE_SOURCE_URLS=true".to_string(),
        );
    }
    if url.username() != "" || url.password().is_some() {
        return Err("source URL credentials are not allowed".to_string());
    }
    Ok(())
}

fn source_host_is_private(host: &str) -> bool {
    if matches!(
        host,
        "localhost" | "host.docker.internal" | "metadata.google.internal"
    ) || host.ends_with(".localhost")
        || host.ends_with(".local")
    {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            let [a, b, _, _] = ip.octets();
            a == 0
                || a == 10
                || a == 127
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 168)
                || a >= 224
        }
        Ok(IpAddr::V6(ip)) => {
            let first = ip.segments()[0];
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
        }
        Err(_) => false,
    }
}

fn validate_source_host_allowlist(
    url: &reqwest::Url,
    allowed_hosts: &[String],
) -> Result<(), String> {
    if allowed_hosts.is_empty() {
        return Ok(());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "source URL must include a host".to_string())?
        .to_ascii_lowercase();
    if allowed_hosts
        .iter()
        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
    {
        return Ok(());
    }
    Err(format!(
        "source host {host} is not in ECONOMICS_ALLOWED_SOURCE_HOSTS"
    ))
}

fn series_from_bytes(
    request: &ApiPullRequest,
    bytes: &[u8],
) -> Result<(MarketSeries, SourceQualityReport), String> {
    match request.parser.unwrap_or(SourceParser::JsonRecords) {
        SourceParser::JsonRecords => {
            let json_value = serde_json::from_slice::<Value>(bytes)
                .map_err(|error| format!("source response is not JSON: {error}"))?;
            series_from_json_records_with_quality(request, &json_value)
        }
        SourceParser::JsonTupleArray => {
            let json_value = serde_json::from_slice::<Value>(bytes)
                .map_err(|error| format!("source response is not JSON: {error}"))?;
            series_from_json_tuple_array(request, &json_value)
        }
        SourceParser::CsvRecords => {
            let text = std::str::from_utf8(bytes)
                .map_err(|error| format!("source response is not UTF-8 CSV: {error}"))?;
            series_from_csv_records(request, text)
        }
    }
}

#[cfg(test)]
fn series_from_json(request: &ApiPullRequest, value: &Value) -> Result<MarketSeries, String> {
    series_from_json_records_with_quality(request, value).map(|(series, _)| series)
}

fn series_from_json_records_with_quality(
    request: &ApiPullRequest,
    value: &Value,
) -> Result<(MarketSeries, SourceQualityReport), String> {
    let root = match request.root_pointer.as_deref() {
        Some(pointer) if !pointer.trim().is_empty() => value
            .pointer(pointer)
            .ok_or_else(|| format!("rootPointer {pointer} did not match JSON response"))?,
        _ => value,
    };
    let items = root
        .as_array()
        .ok_or_else(|| "selected JSON value must be an array".to_string())?;
    let date_field = request.date_field.as_deref().unwrap_or("date");
    let price_field = request.price_field.as_deref().unwrap_or("price");
    let volume_field = request.volume_field.as_deref();
    let mut observations = Vec::with_capacity(items.len().min(MAX_OBSERVATIONS_PER_SERIES));
    let mut dropped_points = 0usize;
    for item in items.iter().take(MAX_OBSERVATIONS_PER_SERIES) {
        let Some(date) = field_value(item, date_field).and_then(date_from_value) else {
            dropped_points += 1;
            continue;
        };
        let Some(price) = field_value(item, price_field).and_then(number_from_value) else {
            dropped_points += 1;
            continue;
        };
        let volume = volume_field
            .and_then(|field| field_value(item, field))
            .and_then(number_from_value);
        observations.push(MarketObservation {
            date,
            price,
            volume,
        });
    }
    build_series_with_quality(
        request,
        SourceParser::JsonRecords,
        observations,
        dropped_points,
    )
}

fn series_from_json_tuple_array(
    request: &ApiPullRequest,
    value: &Value,
) -> Result<(MarketSeries, SourceQualityReport), String> {
    let root = match request.root_pointer.as_deref() {
        Some(pointer) if !pointer.trim().is_empty() => value
            .pointer(pointer)
            .ok_or_else(|| format!("rootPointer {pointer} did not match JSON response"))?,
        _ => value,
    };
    let items = root
        .as_array()
        .ok_or_else(|| "selected JSON value must be an array".to_string())?;
    let date_index = request.date_index.unwrap_or(0);
    let price_index = request.price_index.unwrap_or(1);
    let volume_index = request.volume_index;
    let mut observations = Vec::with_capacity(items.len().min(MAX_OBSERVATIONS_PER_SERIES));
    let mut dropped_points = 0usize;
    for item in items.iter().take(MAX_OBSERVATIONS_PER_SERIES) {
        let Some(tuple) = item.as_array() else {
            dropped_points += 1;
            continue;
        };
        let Some(date) = tuple.get(date_index).and_then(date_from_value) else {
            dropped_points += 1;
            continue;
        };
        let Some(price) = tuple.get(price_index).and_then(number_from_value) else {
            dropped_points += 1;
            continue;
        };
        let volume = volume_index
            .and_then(|index| tuple.get(index))
            .and_then(number_from_value);
        observations.push(MarketObservation {
            date,
            price,
            volume,
        });
    }
    build_series_with_quality(
        request,
        SourceParser::JsonTupleArray,
        observations,
        dropped_points,
    )
}

fn series_from_csv_records(
    request: &ApiPullRequest,
    text: &str,
) -> Result<(MarketSeries, SourceQualityReport), String> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header_line = lines
        .next()
        .ok_or_else(|| "CSV response must include a header row".to_string())?;
    let headers = parse_csv_line(header_line)?;
    let date_field = request.date_field.as_deref().unwrap_or("date");
    let price_field = request.price_field.as_deref().unwrap_or("price");
    let volume_field = request.volume_field.as_deref();
    let date_index = csv_header_index(&headers, date_field)?;
    let price_index = csv_header_index(&headers, price_field)?;
    let volume_index = volume_field
        .map(|field| csv_header_index(&headers, field))
        .transpose()?;
    let mut observations = Vec::with_capacity(MAX_OBSERVATIONS_PER_SERIES.min(1024));
    let mut dropped_points = 0usize;
    for line in lines.take(MAX_OBSERVATIONS_PER_SERIES) {
        let fields = parse_csv_line(line)?;
        let Some(date) = fields
            .get(date_index)
            .and_then(|value| date_from_text(value))
        else {
            dropped_points += 1;
            continue;
        };
        let Some(price) = fields
            .get(price_index)
            .and_then(|value| number_from_text(value))
        else {
            dropped_points += 1;
            continue;
        };
        let volume = volume_index
            .and_then(|index| fields.get(index))
            .and_then(|value| number_from_text(value));
        observations.push(MarketObservation {
            date,
            price,
            volume,
        });
    }
    build_series_with_quality(
        request,
        SourceParser::CsvRecords,
        observations,
        dropped_points,
    )
}

fn build_series_with_quality(
    request: &ApiPullRequest,
    parser: SourceParser,
    mut observations: Vec<MarketObservation>,
    dropped_points: usize,
) -> Result<(MarketSeries, SourceQualityReport), String> {
    observations.sort_by(|left, right| left.date.cmp(&right.date));
    let before_dedupe = observations.len();
    observations.dedup_by(|left, right| left.date == right.date);
    let dropped_points = dropped_points + before_dedupe.saturating_sub(observations.len());
    let quality = source_quality_report(parser, &observations, dropped_points);
    let series = MarketSeries {
        instrument_id: request
            .instrument_id
            .clone()
            .ok_or_else(|| "instrumentId is required to store parsed source data".to_string())?,
        display_name: request.display_name.clone(),
        asset_class: request
            .asset_class
            .clone()
            .ok_or_else(|| "assetClass is required to store parsed source data".to_string())?,
        currency: request.currency.clone(),
        source: request
            .source
            .clone()
            .or_else(|| Some("api-pull".to_string())),
        observations,
        features: None,
    };
    Ok((series, quality))
}

fn source_quality_report(
    parser: SourceParser,
    observations: &[MarketObservation],
    dropped_points: usize,
) -> SourceQualityReport {
    let min_price = observations
        .iter()
        .map(|point| point.price)
        .reduce(f64::min)
        .map(round6);
    let max_price = observations
        .iter()
        .map(|point| point.price)
        .reduce(f64::max)
        .map(round6);
    SourceQualityReport {
        parser,
        observed_points: observations.len(),
        dropped_points,
        first_date: observations.first().map(|point| point.date.clone()),
        last_date: observations.last().map(|point| point.date.clone()),
        min_price,
        max_price,
    }
}

fn parse_csv_line(line: &str) -> Result<Vec<String>, String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(field.trim().to_string());
                field.clear();
            }
            _ => field.push(ch),
        }
    }
    if in_quotes {
        return Err("CSV row has an unterminated quoted field".to_string());
    }
    fields.push(field.trim().to_string());
    Ok(fields)
}

fn csv_header_index(headers: &[String], field: &str) -> Result<usize, String> {
    headers
        .iter()
        .position(|header| header.eq_ignore_ascii_case(field))
        .ok_or_else(|| format!("CSV field {field} was not found in header row"))
}

fn field_value<'a>(value: &'a Value, field: &str) -> Option<&'a Value> {
    if field.starts_with('/') {
        value.pointer(field)
    } else {
        value.get(field)
    }
}

fn date_from_value(value: &Value) -> Option<String> {
    value
        .as_str()
        .and_then(date_from_text)
        .or_else(|| value.as_i64().map(|number| number.to_string()))
        .or_else(|| value.as_u64().map(|number| number.to_string()))
        .or_else(|| {
            value.as_f64().and_then(|number| {
                if number.is_finite() {
                    Some(format!("{number:.0}"))
                } else {
                    None
                }
            })
        })
}

fn date_from_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed.eq_ignore_ascii_case("null") {
        None
    } else {
        Some(trimmed.chars().take(MAX_TOKEN_LEN).collect())
    }
}

fn number_from_value(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(number_from_text))
        .filter(|number| number.is_finite())
}

fn number_from_text(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed.eq_ignore_ascii_case("null")
        || trimmed.eq_ignore_ascii_case("nan")
    {
        return None;
    }
    let normalized = trimmed
        .trim_start_matches('$')
        .chars()
        .filter(|ch| *ch != ',')
        .collect::<String>();
    normalized
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
}

async fn metrics(State(state): State<AppState>) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let body = format!(
        "# HELP dd_economics_server_http_requests_total HTTP requests observed by the economics service.\n\
         # TYPE dd_economics_server_http_requests_total counter\n\
         dd_economics_server_http_requests_total {}\n\
         # HELP dd_economics_server_forecasts_total Forecasts generated.\n\
         # TYPE dd_economics_server_forecasts_total counter\n\
         dd_economics_server_forecasts_total {}\n\
         # HELP dd_economics_server_ingest_requests_total Ingest requests accepted.\n\
         # TYPE dd_economics_server_ingest_requests_total counter\n\
         dd_economics_server_ingest_requests_total {}\n\
         # HELP dd_economics_server_source_pull_total Source pull requests attempted.\n\
         # TYPE dd_economics_server_source_pull_total counter\n\
         dd_economics_server_source_pull_total {}\n\
         # HELP dd_economics_server_source_pull_success_total Source pull requests that fetched and parsed/stored or fetched successfully.\n\
         # TYPE dd_economics_server_source_pull_success_total counter\n\
         dd_economics_server_source_pull_success_total {}\n\
         # HELP dd_economics_server_source_pull_failure_total Source pull requests rejected or failed before a successful response.\n\
         # TYPE dd_economics_server_source_pull_failure_total counter\n\
         dd_economics_server_source_pull_failure_total {}\n\
         # HELP dd_economics_server_source_pull_bytes_total Total response bytes fetched by successful source pulls.\n\
         # TYPE dd_economics_server_source_pull_bytes_total counter\n\
         dd_economics_server_source_pull_bytes_total {}\n\
         # HELP dd_economics_server_source_pull_stored_points_total Total normalized observations stored by source pulls.\n\
         # TYPE dd_economics_server_source_pull_stored_points_total counter\n\
         dd_economics_server_source_pull_stored_points_total {}\n\
         # HELP dd_economics_server_source_pull_last_success_unix_seconds Unix timestamp of the latest successful source pull.\n\
         # TYPE dd_economics_server_source_pull_last_success_unix_seconds gauge\n\
         dd_economics_server_source_pull_last_success_unix_seconds {}\n\
         # HELP dd_economics_server_sentiment_requests_total Sentiment analysis requests accepted.\n\
         # TYPE dd_economics_server_sentiment_requests_total counter\n\
         dd_economics_server_sentiment_requests_total {}\n\
         # HELP dd_economics_server_recommendation_requests_total Recommendation requests accepted.\n\
         # TYPE dd_economics_server_recommendation_requests_total counter\n\
         dd_economics_server_recommendation_requests_total {}\n\
         # HELP dd_economics_server_pipeline_plan_requests_total Pipeline plan requests accepted.\n\
         # TYPE dd_economics_server_pipeline_plan_requests_total counter\n\
         dd_economics_server_pipeline_plan_requests_total {}\n\
         # HELP dd_economics_server_pipeline_submit_requests_total Pipeline submit requests accepted.\n\
         # TYPE dd_economics_server_pipeline_submit_requests_total counter\n\
         dd_economics_server_pipeline_submit_requests_total {}\n\
         # HELP dd_economics_server_pipeline_publish_attempts_total Pipeline plan NATS publish attempts requested.\n\
         # TYPE dd_economics_server_pipeline_publish_attempts_total counter\n\
         dd_economics_server_pipeline_publish_attempts_total {}\n\
         # HELP dd_economics_server_pipeline_publish_success_total Pipeline plans published to NATS successfully.\n\
         # TYPE dd_economics_server_pipeline_publish_success_total counter\n\
         dd_economics_server_pipeline_publish_success_total {}\n\
         # HELP dd_economics_server_pipeline_publish_failure_total Pipeline plan publish attempts skipped or failed.\n\
         # TYPE dd_economics_server_pipeline_publish_failure_total counter\n\
         dd_economics_server_pipeline_publish_failure_total {}\n\
         # HELP dd_economics_server_pipeline_submit_success_total Spark pipeline jobs accepted by the pipeline server.\n\
         # TYPE dd_economics_server_pipeline_submit_success_total counter\n\
         dd_economics_server_pipeline_submit_success_total {}\n\
         # HELP dd_economics_server_pipeline_submit_failure_total Spark pipeline job submits rejected or failed before submit.\n\
         # TYPE dd_economics_server_pipeline_submit_failure_total counter\n\
         dd_economics_server_pipeline_submit_failure_total {}\n\
         # HELP dd_economics_server_integration_health_requests_total Integration health requests served.\n\
         # TYPE dd_economics_server_integration_health_requests_total counter\n\
         dd_economics_server_integration_health_requests_total {}\n\
         # HELP dd_economics_server_observability_requests_total Observability descriptor requests served.\n\
         # TYPE dd_economics_server_observability_requests_total counter\n\
         dd_economics_server_observability_requests_total {}\n\
         # HELP dd_economics_server_auth_failures_total Rejected requests with missing or invalid auth.\n\
         # TYPE dd_economics_server_auth_failures_total counter\n\
         dd_economics_server_auth_failures_total {}\n\
         # HELP dd_economics_server_errors_total Forecast, ingest, source, or publish errors.\n\
         # TYPE dd_economics_server_errors_total counter\n\
         dd_economics_server_errors_total {}\n\
         # HELP dd_economics_server_nats_messages_total NATS forecast requests consumed.\n\
         # TYPE dd_economics_server_nats_messages_total counter\n\
         dd_economics_server_nats_messages_total {}\n\
         # HELP dd_economics_server_nats_published_total NATS messages published.\n\
         # TYPE dd_economics_server_nats_published_total counter\n\
         dd_economics_server_nats_published_total {}\n",
        state.metrics.http_requests_total.load(Ordering::Relaxed),
        state.metrics.forecasts_total.load(Ordering::Relaxed),
        state.metrics.ingest_requests_total.load(Ordering::Relaxed),
        state.metrics.source_pull_total.load(Ordering::Relaxed),
        state
            .metrics
            .source_pull_success_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .source_pull_failure_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .source_pull_bytes_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .source_pull_stored_points_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .source_pull_last_success_unix_seconds
            .load(Ordering::Relaxed),
        state.metrics.sentiment_requests_total.load(Ordering::Relaxed),
        state
            .metrics
            .recommendation_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .pipeline_plan_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .pipeline_submit_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .pipeline_publish_attempts_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .pipeline_publish_success_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .pipeline_publish_failure_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .pipeline_submit_success_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .pipeline_submit_failure_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .integration_health_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .observability_requests_total
            .load(Ordering::Relaxed),
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8114").parse::<u16>()?;
    let nats = match optional_env("NATS_URL") {
        Some(url) => Some(async_nats::connect(url).await?),
        None => None,
    };
    let state = AppState {
        config: Arc::new(config_from_env()),
        metrics: Arc::new(Metrics::default()),
        nats,
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("dd-economics-server/0.1 source-pull")
            .build()?,
        series_store: Arc::new(RwLock::new(BTreeMap::new())),
    };
    tokio::spawn(run_nats_loop(state.clone()));

    let app = Router::new()
        .route("/", get(root))
        .route("/descriptor", get(descriptor))
        .route("/dashboard.json", get(dashboard_json))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/schema", get(schema))
        .route("/example", get(example))
        .route("/sources", get(sources))
        .route("/sources/public", get(public_sources))
        .route("/sources/pull", post(pull_source_http))
        .route("/sentiment/sources", get(sentiment_sources))
        .route("/sentiment/analyze", post(sentiment_analyze_http))
        .route("/macro/indicators", get(macro_indicators))
        .route("/vc/investment", get(vc_investment))
        .route("/recommendations", post(recommendations_http))
        .route("/audit/hardening", get(hardening_audit))
        .route("/observability", get(observability))
        .route("/integrations/health", get(integrations_health))
        .route("/pipelines/catalog", get(pipeline_catalog))
        .route("/pipelines/plan", post(pipeline_plan_http))
        .route("/pipelines/submit", post(pipeline_submit_http))
        .route("/model/equations", get(equations))
        .route("/engine/des", get(des_engine_descriptor))
        .route("/forecast", post(forecast_http))
        .route("/ingest", post(ingest_http))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    emit_log(
        "INFO",
        "economics.server.start",
        "dd-economics-server listening",
        json!({
            "address": addr.to_string(),
            "metricsRoute": "GET /metrics",
            "observabilityRoute": "GET /observability",
            "otelMode": "explicit-only"
        }),
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    Ok(())
}

const DASHBOARD_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Economics Dashboard</title>
  <style>
    :root {
      color-scheme: dark;
      --bg: #101213;
      --panel: #171a1d;
      --panel-2: #20252a;
      --line: #303840;
      --text: #f1f4f2;
      --muted: #9ba8a2;
      --green: #50c878;
      --blue: #64a6ff;
      --gold: #e3b341;
      --red: #f26d6d;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      background: var(--bg);
      color: var(--text);
      font: 14px/1.45 Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    }
    header {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 16px;
      min-height: 64px;
      padding: 0 20px;
      border-bottom: 1px solid var(--line);
      background: #121517;
    }
    h1 { margin: 0; font-size: 18px; font-weight: 700; letter-spacing: 0; }
    .sub { color: var(--muted); font-size: 12px; }
    main {
      display: grid;
      grid-template-columns: 320px minmax(0, 1fr);
      gap: 0;
      min-height: calc(100vh - 64px);
    }
    aside {
      border-right: 1px solid var(--line);
      background: var(--panel);
      padding: 16px;
      overflow: auto;
    }
    section {
      min-width: 0;
      padding: 16px;
      overflow: auto;
    }
    .toolbar {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
      margin-bottom: 12px;
    }
    button, select {
      border: 1px solid var(--line);
      background: var(--panel-2);
      color: var(--text);
      height: 34px;
      border-radius: 6px;
      padding: 0 10px;
    }
    button { cursor: pointer; }
    .metric-grid {
      display: grid;
      grid-template-columns: repeat(4, minmax(150px, 1fr));
      gap: 10px;
      margin-bottom: 12px;
    }
    .metric, .chart, .table-wrap, .equations {
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 8px;
    }
    .metric { padding: 12px; min-height: 78px; }
    .metric strong { display: block; font-size: 20px; }
    .metric span { color: var(--muted); font-size: 12px; }
    .watchlist { display: grid; gap: 8px; }
    .watch {
      width: 100%;
      text-align: left;
      display: grid;
      grid-template-columns: 1fr auto;
      gap: 4px 8px;
      min-height: 56px;
    }
    .watch.active { border-color: var(--blue); }
    .watch small { color: var(--muted); }
    .signal { font-size: 12px; color: var(--green); }
    .signal.risk { color: var(--red); }
    .chart { padding: 12px; margin-bottom: 12px; }
    canvas { width: 100%; height: 360px; display: block; }
    .table-wrap { overflow: auto; }
    table { width: 100%; border-collapse: collapse; min-width: 760px; }
    th, td { padding: 10px 12px; border-bottom: 1px solid var(--line); text-align: left; white-space: nowrap; }
    th { color: var(--muted); font-size: 12px; font-weight: 600; }
    .equations { padding: 12px; margin-top: 12px; }
    .eq-list { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; }
    .eq {
      border: 1px solid var(--line);
      border-radius: 6px;
      padding: 10px;
      background: #14181a;
    }
    .eq code { color: var(--gold); white-space: normal; }
    @media (max-width: 900px) {
      main { grid-template-columns: 1fr; }
      aside { border-right: 0; border-bottom: 1px solid var(--line); max-height: 320px; }
      .metric-grid { grid-template-columns: repeat(2, minmax(130px, 1fr)); }
      .eq-list { grid-template-columns: 1fr; }
      canvas { height: 300px; }
    }
  </style>
</head>
<body>
  <header>
    <div>
      <h1>Economics Dashboard</h1>
      <div class="sub">15Y history model | 18M projection | DES-backed theory surface</div>
    </div>
    <div class="sub" id="status">loading</div>
  </header>
  <main>
    <aside>
      <div class="toolbar">
        <strong>Markets</strong>
        <button id="refresh">Refresh</button>
      </div>
      <div class="watchlist" id="watchlist"></div>
    </aside>
    <section>
      <div class="toolbar">
        <div>
          <strong id="selected-title">Projection</strong>
          <div class="sub" id="selected-sub"></div>
        </div>
        <select id="scenario">
          <option value="base">Base</option>
          <option value="soft-landing">Soft landing</option>
          <option value="liquidity-crunch">Liquidity crunch</option>
          <option value="oil-shock">Oil shock</option>
          <option value="dollar-strength">Dollar strength</option>
          <option value="deflation">Deflation</option>
        </select>
      </div>
      <div class="metric-grid" id="metrics"></div>
      <div class="chart"><canvas id="chart" width="1200" height="420"></canvas></div>
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Instrument</th>
              <th>Class</th>
              <th>Signal</th>
              <th>Last</th>
              <th>18M Return</th>
              <th>Drift</th>
              <th>Volatility</th>
            </tr>
          </thead>
          <tbody id="projection-rows"></tbody>
        </table>
      </div>
      <div class="equations">
        <strong>Equation Layer</strong>
        <div class="eq-list" id="equations"></div>
      </div>
    </section>
  </main>
  <script>
    const state = { data: null, selected: 0 };
    const colors = ["#64a6ff", "#50c878", "#e3b341", "#f26d6d"];

    function fmtPct(v) { return (v * 100).toFixed(2) + "%"; }
    function fmtNum(v) {
      if (Math.abs(v) >= 1000) return Number(v).toLocaleString(undefined, { maximumFractionDigits: 0 });
      return Number(v).toLocaleString(undefined, { maximumFractionDigits: 3 });
    }

    async function load() {
      const res = await fetch("dashboard.json", { headers: { "accept": "application/json" } });
      if (!res.ok) throw new Error("HTTP " + res.status);
      state.data = await res.json();
      state.selected = Math.min(state.selected, state.data.forecast.projections.length - 1);
      render();
    }

    async function applyScenario(scenario) {
      const res = await fetch("forecast", {
        method: "POST",
        headers: { "accept": "application/json", "content-type": "application/json" },
        body: JSON.stringify({
          schemaVersion: "economics.forecast.v1",
          requestId: "dashboard-" + scenario,
          scenario,
          horizonMonths: state.data.forecast.horizonMonths,
          confidenceLevel: state.data.forecast.confidenceLevel,
          series: state.data.series
        })
      });
      if (!res.ok) throw new Error("HTTP " + res.status);
      state.data.forecast = await res.json();
      state.selected = Math.min(state.selected, state.data.forecast.projections.length - 1);
      render();
    }

    function render() {
      const projections = state.data.forecast.projections;
      const selected = projections[state.selected] || projections[0];
      document.getElementById("status").textContent = new Date().toLocaleTimeString();
      document.getElementById("selected-title").textContent = selected.displayName;
      document.getElementById("selected-sub").textContent = selected.assetClass + " | " + selected.currency;
      renderWatchlist(projections);
      renderMetrics(selected, projections);
      renderTable(projections);
      renderEquations(state.data.equations.slice(0, 6));
      drawChart(selected);
    }

    function renderWatchlist(projections) {
      const list = document.getElementById("watchlist");
      list.innerHTML = "";
      projections.forEach((p, idx) => {
        const btn = document.createElement("button");
        btn.className = "watch" + (idx === state.selected ? " active" : "");
        btn.onclick = () => { state.selected = idx; render(); };
        const risk = p.signal.includes("reduce") ? " risk" : "";
        btn.innerHTML = "<span>" + p.displayName + "<br><small>" + p.instrumentId + "</small></span>" +
          "<span class=\"signal" + risk + "\">" + p.signal + "</span>";
        list.appendChild(btn);
      });
    }

    function renderMetrics(selected, projections) {
      const best = projections.slice().sort((a, b) => b.expectedReturn18m - a.expectedReturn18m)[0];
      const worst = projections.slice().sort((a, b) => a.expectedReturn18m - b.expectedReturn18m)[0];
      const metrics = [
        ["Selected 18M", fmtPct(selected.expectedReturn18m), selected.signal],
        ["Annual Drift", fmtPct(selected.annualizedDrift), "weighted theory/data"],
        ["Annual Vol", fmtPct(selected.annualizedVolatility), "interval width"],
        ["Best/Worst", best.instrumentId + " / " + worst.instrumentId, fmtPct(best.expectedReturn18m) + " / " + fmtPct(worst.expectedReturn18m)]
      ];
      document.getElementById("metrics").innerHTML = metrics.map(m =>
        "<div class=\"metric\"><span>" + m[0] + "</span><strong>" + m[1] + "</strong><span>" + m[2] + "</span></div>"
      ).join("");
    }

    function renderTable(projections) {
      document.getElementById("projection-rows").innerHTML = projections.map((p, idx) =>
        "<tr data-idx=\"" + idx + "\"><td>" + p.displayName + "</td><td>" + p.assetClass + "</td><td>" +
        p.signal + "</td><td>" + fmtNum(p.lastPrice) + "</td><td>" + fmtPct(p.expectedReturn18m) +
        "</td><td>" + fmtPct(p.annualizedDrift) + "</td><td>" + fmtPct(p.annualizedVolatility) + "</td></tr>"
      ).join("");
    }

    function renderEquations(equations) {
      document.getElementById("equations").innerHTML = equations.map(eq =>
        "<div class=\"eq\"><strong>" + eq.name + "</strong><br><code>" + eq.equation +
        "</code><div class=\"sub\">" + eq.family + "</div></div>"
      ).join("");
    }

    function drawChart(p) {
      const canvas = document.getElementById("chart");
      const ctx = canvas.getContext("2d");
      const w = canvas.width;
      const h = canvas.height;
      ctx.clearRect(0, 0, w, h);
      ctx.fillStyle = "#121517";
      ctx.fillRect(0, 0, w, h);
      const pts = p.points;
      const values = pts.flatMap(x => [x.lower, x.expected, x.upper]);
      const min = Math.min(...values) * 0.98;
      const max = Math.max(...values) * 1.02;
      const x = i => 48 + (w - 80) * (i / Math.max(1, pts.length - 1));
      const y = v => h - 36 - (h - 72) * ((v - min) / Math.max(1e-9, max - min));
      ctx.strokeStyle = "#303840";
      ctx.lineWidth = 1;
      for (let i = 0; i < 5; i++) {
        const yy = 24 + i * (h - 72) / 4;
        ctx.beginPath(); ctx.moveTo(40, yy); ctx.lineTo(w - 24, yy); ctx.stroke();
      }
      drawLine(ctx, pts.map((pt, i) => [x(i), y(pt.upper)]), colors[2], 1);
      drawLine(ctx, pts.map((pt, i) => [x(i), y(pt.lower)]), colors[3], 1);
      drawLine(ctx, pts.map((pt, i) => [x(i), y(pt.expected)]), colors[0], 3);
      ctx.fillStyle = "#9ba8a2";
      ctx.font = "14px ui-monospace, Menlo, monospace";
      ctx.fillText(fmtNum(max), 8, 28);
      ctx.fillText(fmtNum(min), 8, h - 22);
      ctx.fillStyle = "#f1f4f2";
      ctx.fillText(p.instrumentId + " expected path", 48, 24);
    }

    function drawLine(ctx, points, color, width) {
      ctx.strokeStyle = color;
      ctx.lineWidth = width;
      ctx.beginPath();
      points.forEach(([x, y], idx) => idx ? ctx.lineTo(x, y) : ctx.moveTo(x, y));
      ctx.stroke();
    }

    document.getElementById("refresh").onclick = () => load().catch(err => {
      document.getElementById("status").textContent = err.message;
    });
    document.getElementById("scenario").onchange = (event) => {
      applyScenario(event.target.value).catch(err => {
        document.getElementById("status").textContent = err.message;
      });
    };
    load().catch(err => { document.getElementById("status").textContent = err.message; });
  </script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            server_auth_secret: Some("secret".to_string()),
            allow_unauthenticated: false,
            allow_private_source_urls: false,
            allowed_source_hosts: Vec::new(),
            allowed_source_auth_envs: default_source_auth_envs(),
            sentiment_credentials: SentimentCredentialStatus {
                x_bearer_token: true,
                x_api_key: false,
                x_api_secret: false,
                x_access_token: false,
                x_access_token_secret: false,
                reddit_client_id: true,
                reddit_client_secret: true,
                reddit_user_agent: true,
                news_api_key: false,
                stocktwits_token: false,
                gdelt_api_key: false,
            },
            market_data_credentials: MarketDataCredentialStatus {
                fred_api_key: true,
                bea_api_key: true,
                bls_api_key: true,
                treasury_api_key: false,
                census_api_key: false,
                eia_api_key: false,
                coingecko_api_key: true,
                sec_api_key: false,
                crunchbase_api_key: true,
                pitchbook_api_key: false,
                cb_insights_api_key: false,
                dealroom_api_key: false,
                preqin_api_key: false,
            },
            history_years: DEFAULT_HISTORY_YEARS,
            projection_months: DEFAULT_PROJECTION_MONTHS,
            confidence_level: 0.90,
            request_subject: ECONOMICS_FORECAST_REQUEST_SUBJECT.to_string(),
            queue_group: ECONOMICS_QUEUE_GROUP.to_string(),
            result_subject: ECONOMICS_FORECAST_RESULT_SUBJECT.to_string(),
            market_event_subject: ECONOMICS_MARKET_EVENT_SUBJECT.to_string(),
            runtime_event_subject: RUNTIME_EVENTS_SUBJECT.to_string(),
            pipeline_intent_subject: PUBLIC_DATA_PIPELINE_JOBS_SUBJECT.to_string(),
            spark_pipeline_url: Some(DEFAULT_SPARK_PIPELINE_URL.to_string()),
            spark_pipeline_auth_env: "SERVER_AUTH_SECRET".to_string(),
            spark_master_url: DEFAULT_SPARK_MASTER_URL.to_string(),
            airflow_api_url: Some(DEFAULT_AIRFLOW_API_URL.to_string()),
            databricks_host: Some("https://example.cloud.databricks.com".to_string()),
            data_lake_uri: DEFAULT_DATA_LAKE_URI.to_string(),
            allow_pipeline_submit: false,
            allow_external_pipeline_urls: false,
        }
    }

    fn test_state() -> AppState {
        AppState {
            config: Arc::new(test_config()),
            metrics: Arc::new(Metrics::default()),
            nats: None,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("dd-economics-server/0.1 test-source-pull")
                .build()
                .unwrap(),
            series_store: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    fn source_id_pull_request(source_id: &str) -> ApiPullRequest {
        ApiPullRequest {
            request_id: None,
            source_id: Some(source_id.to_string()),
            url: None,
            parser: None,
            instrument_id: None,
            display_name: None,
            asset_class: None,
            currency: None,
            source: None,
            root_pointer: None,
            date_field: None,
            price_field: None,
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            auth_header_env: None,
            auth_header_name: None,
        }
    }

    #[test]
    fn forecast_uses_equation_catalog_and_projects_requested_horizon() {
        let request = example_request();
        let response = generate_forecast(&test_config(), request).expect("forecast succeeds");

        assert_eq!(response.schema_version, SCHEMA_VERSION);
        assert_eq!(response.horizon_months, DEFAULT_PROJECTION_MONTHS);
        assert!(response.equations.len() >= 10);
        assert!(response.projections.len() >= 5);
        assert!(response
            .projections
            .iter()
            .all(|projection| projection.points.len() == DEFAULT_PROJECTION_MONTHS as usize));
    }

    #[test]
    fn liquidity_crunch_penalizes_crypto_more_than_bonds() {
        let mut request = example_request();
        request.scenario = Some("liquidity-crunch".to_string());
        let response = generate_forecast(&test_config(), request).expect("forecast succeeds");
        let crypto = response
            .projections
            .iter()
            .find(|projection| projection.asset_class == "crypto")
            .expect("crypto projection");
        let bond = response
            .projections
            .iter()
            .find(|projection| projection.asset_class == "bonds")
            .expect("bond projection");

        assert!(crypto.annualized_drift < bond.annualized_drift);
    }

    #[test]
    fn invalid_series_prices_are_rejected() {
        let mut request = example_request();
        let series = request.series.as_mut().unwrap();
        series[0].observations[0].price = 0.0;

        let error = generate_forecast(&test_config(), request).expect_err("invalid price rejected");
        assert!(error.contains("price must be finite and positive"));
    }

    #[test]
    fn source_url_policy_blocks_private_hosts_by_default() {
        let url = reqwest::Url::parse("http://127.0.0.1:9000/data.json").unwrap();
        let error = validate_source_url(&url, false).expect_err("private http blocked");

        assert!(error.contains("ECONOMICS_ALLOW_PRIVATE_SOURCE_URLS"));
    }

    #[test]
    fn parses_json_series_from_pointer_fields() {
        let request = ApiPullRequest {
            request_id: None,
            source_id: None,
            url: Some("https://example.com/data.json".to_string()),
            parser: Some(SourceParser::JsonRecords),
            instrument_id: Some("TEST".to_string()),
            display_name: Some("Test".to_string()),
            asset_class: Some("equities".to_string()),
            currency: Some("USD".to_string()),
            source: Some("unit".to_string()),
            root_pointer: Some("/prices".to_string()),
            date_field: Some("d".to_string()),
            price_field: Some("p".to_string()),
            volume_field: Some("v".to_string()),
            date_index: None,
            price_index: None,
            volume_index: None,
            auth_header_env: None,
            auth_header_name: None,
        };
        let value = json!({
            "prices": [
                { "d": "2026-01", "p": "100.5", "v": 10 },
                { "d": "2026-02", "p": 102.0, "v": 11 }
            ]
        });

        let series = series_from_json(&request, &value).expect("series parsed");

        assert_eq!(series.instrument_id, "TEST");
        assert_eq!(series.observations.len(), 2);
        assert_eq!(series.observations[0].price, 100.5);
        assert_eq!(series.observations[1].volume, Some(11.0));
    }

    #[test]
    fn des_surface_is_available_for_runtime_discovery() {
        let surface = des_surface_descriptor();

        assert_eq!(surface["crate"], "des_engine");
        assert!(surface["modules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "acausal"));
    }

    #[test]
    fn sentiment_placeholder_scores_documents_and_reports_credentials() {
        let response = analyze_sentiment(
            &test_config(),
            SentimentAnalyzeRequest {
                request_id: Some("sentiment-unit".to_string()),
                schema_version: Some(SCHEMA_VERSION.to_string()),
                query: Some("$BTC oil recession".to_string()),
                instrument_ids: Some(vec!["BTC-USD".to_string(), "CL=F".to_string()]),
                documents: vec![
                    SentimentDocument {
                        source: "x-twitter".to_string(),
                        text: "$BTC bullish breakout with strong inflow and adoption".to_string(),
                        url: None,
                        author: None,
                        published_at: None,
                        weight: Some(2.0),
                    },
                    SentimentDocument {
                        source: "reddit".to_string(),
                        text: "Oil looks weak after recession and liquidity crunch chatter"
                            .to_string(),
                        url: None,
                        author: None,
                        published_at: None,
                        weight: Some(1.0),
                    },
                ],
            },
        )
        .expect("sentiment analysis succeeds");

        assert_eq!(response.request_id, "sentiment-unit");
        assert_eq!(response.document_count, 2);
        assert!(response.average_sentiment > 0.0);
        assert!(response.credential_status.x_bearer_token);
        assert!(response.credential_status.reddit_client_id);
        assert_eq!(response.source_scores.len(), 2);
        assert!(response.top_terms.iter().any(|term| term == "$btc"));
    }

    #[test]
    fn recommendations_return_company_and_commodity_rankings() {
        let response = generate_recommendations(
            &test_config(),
            RecommendationRequest {
                request_id: Some("recommendation-unit".to_string()),
                schema_version: Some(SCHEMA_VERSION.to_string()),
                horizon_months: Some(18),
                company_limit: Some(20),
                commodity_limit: Some(30),
                scenario: Some("base".to_string()),
                series: Some(sample_market_series()),
                macro_context: Some(MacroContext {
                    policy_rate: Some(0.045),
                    expected_inflation: Some(0.026),
                    inflation: Some(0.031),
                    real_growth: Some(0.020),
                    ..MacroContext::default()
                }),
                macro_fiscal_context: Some(default_macro_fiscal_context()),
                venture_capital_context: Some(sample_venture_capital_context()),
                sentiment_context: Some(SentimentSignalContext {
                    average_sentiment: Some(0.10),
                    instrument_scores: None,
                    sector_scores: None,
                }),
            },
        )
        .expect("recommendations succeed");

        assert_eq!(response.request_id, "recommendation-unit");
        assert_eq!(response.company_buys.len(), 20);
        assert_eq!(response.company_dumps.len(), 20);
        assert_eq!(response.commodity_buys.len(), 30);
        assert_eq!(response.commodity_sells_or_dumps.len(), 30);
        assert!(response.company_buys[0].score >= response.company_buys[19].score);
        assert!(response.company_dumps[0].score <= response.company_dumps[19].score);
        assert!(response
            .methodology
            .iter()
            .any(|item| item.contains("VC sector flow")));
    }

    #[test]
    fn pipeline_plan_emits_spark_airflow_databricks_and_nats_intents() {
        let state = test_state();
        let plan = pipeline_plan_from_request(
            &state,
            PipelinePlanRequest {
                request_id: Some("pipeline-unit".to_string()),
                schema_version: Some(SCHEMA_VERSION.to_string()),
                scenario: Some("soft-landing".to_string()),
                data_lake_uri: Some("s3a://dd-economics/unit".to_string()),
                include_recommendations: Some(true),
                publish_to_nats: Some(false),
                job_kinds: None,
                recommendation_request: None,
            },
        )
        .expect("pipeline plan succeeds");

        assert_eq!(plan.request_id, "pipeline-unit");
        assert_eq!(plan.job_intents.len(), 5);
        assert!(plan
            .job_intents
            .iter()
            .any(|intent| intent.engine == "spark-pipeline-server"
                && intent.kind == "INGEST_VALIDATE_PUBLISH"
                && intent.submit_eligible));
        assert!(plan
            .job_intents
            .iter()
            .any(|intent| intent.engine == "airflow" && !intent.submit_eligible));
        assert!(plan
            .job_intents
            .iter()
            .any(|intent| intent.engine == "databricks" && !intent.submit_eligible));
        assert_eq!(
            plan.pipeline_status.pipeline_intent_subject,
            PUBLIC_DATA_PIPELINE_JOBS_SUBJECT
        );
    }

    #[test]
    fn recommendation_validation_rejects_unbounded_vc_context() {
        let mut context = sample_venture_capital_context();
        context.deals[0].amount = f64::INFINITY;
        let error = generate_recommendations(
            &test_config(),
            RecommendationRequest {
                request_id: Some("bad-vc".to_string()),
                schema_version: Some(SCHEMA_VERSION.to_string()),
                horizon_months: Some(18),
                company_limit: Some(20),
                commodity_limit: Some(30),
                scenario: Some("base".to_string()),
                series: Some(sample_market_series()),
                macro_context: None,
                macro_fiscal_context: Some(default_macro_fiscal_context()),
                venture_capital_context: Some(context),
                sentiment_context: None,
            },
        )
        .expect_err("invalid vc amount rejected");

        assert!(error.contains("ventureCapitalContext.deals"));
    }

    #[test]
    fn pipeline_submit_url_rejects_external_hosts_by_default() {
        let mut config = test_config();
        config.spark_pipeline_url = Some("https://spark.example.com".to_string());
        config.allow_external_pipeline_urls = false;

        let error = validate_pipeline_submit_url(&config).expect_err("external URL rejected");

        assert!(error.contains("cluster-local"));
    }

    #[test]
    fn pipeline_submit_url_rejects_credentials_queries_and_fragments() {
        let mut config = test_config();
        config.spark_pipeline_url = Some(
            "http://user:secret@dd-spark-pipeline-server.ai-ml.svc.cluster.local:8085".to_string(),
        );
        let error = validate_pipeline_submit_url(&config).expect_err("credentials rejected");
        assert!(error.contains("credentials"));

        config.spark_pipeline_url = Some(
            "http://dd-spark-pipeline-server.ai-ml.svc.cluster.local:8085?token=secret".to_string(),
        );
        let error = validate_pipeline_submit_url(&config).expect_err("query rejected");
        assert!(error.contains("query strings"));

        config.spark_pipeline_url =
            Some("http://dd-spark-pipeline-server.ai-ml.svc.cluster.local:8085/#frag".to_string());
        let error = validate_pipeline_submit_url(&config).expect_err("fragment rejected");
        assert!(error.contains("fragments"));
    }

    #[tokio::test]
    async fn source_auth_header_env_must_be_allowed() {
        let state = test_state();
        let request = ApiPullRequest {
            request_id: Some("auth-env-unit".to_string()),
            source_id: None,
            url: Some("https://api.worldbank.org/v2/country/US".to_string()),
            parser: None,
            instrument_id: None,
            display_name: None,
            asset_class: None,
            currency: None,
            source: Some("unit".to_string()),
            root_pointer: None,
            date_field: None,
            price_field: None,
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            auth_header_env: Some("SERVER_AUTH_SECRET".to_string()),
            auth_header_name: Some("authorization".to_string()),
        };

        let error = pull_source(&state, request)
            .await
            .expect_err("non-economics auth env rejected before request");
        assert!(error.contains("authHeaderEnv"));
    }

    #[test]
    fn source_auth_header_name_blocks_transport_headers() {
        let error = validate_source_auth_header_name("host").expect_err("host header rejected");

        assert!(error.contains("hop-by-hop"));
    }

    #[test]
    fn public_source_catalog_covers_tradeable_and_macro_assets() {
        let ids = public_source_ids();

        assert!(ids.contains(&"treasury-debt-to-penny"));
        assert!(ids.contains(&"worldbank-us-gdp-current-usd"));
        assert!(ids.contains(&"coingecko-bitcoin-usd"));
        assert!(ids.contains(&"fred-wti-oil"));
        assert!(ids.contains(&"fred-gold"));
        assert!(ids.contains(&"fred-silver"));
        assert!(ids.contains(&"fred-sp500"));
        assert!(ids.contains(&"fred-mortgage30"));
        assert!(ids.contains(&"fred-usd-eur"));
        assert!(public_source_hosts().contains(&"api.fiscaldata.treasury.gov"));
    }

    #[test]
    fn source_id_template_fills_pull_metadata_and_rejects_url_override() {
        let mut request = source_id_pull_request("treasury-debt-to-penny");
        let template = apply_public_source_template(&mut request)
            .expect("source template resolves")
            .expect("template present");

        assert_eq!(template.host, "api.fiscaldata.treasury.gov");
        assert_eq!(request.instrument_id.as_deref(), Some("US-PUBLIC-DEBT"));
        assert_eq!(request.parser, Some(SourceParser::JsonRecords));
        validate_api_pull_request(&request, Some(&template)).expect("template request validates");

        let mut override_request = source_id_pull_request("treasury-debt-to-penny");
        override_request.url = Some("https://example.com/not-the-template.json".to_string());
        let error = apply_public_source_template(&mut override_request)
            .expect_err("sourceId URL override rejected");
        assert!(error.contains("url overrides"));
    }

    #[test]
    fn parses_treasury_fiscaldata_json_and_reports_quality() {
        let mut request = source_id_pull_request("treasury-debt-to-penny");
        let template = apply_public_source_template(&mut request)
            .expect("template resolves")
            .expect("template present");
        validate_api_pull_request(&request, Some(&template)).expect("request validates");
        let body = br#"{
            "data": [
                {"record_date":"2026-06-03","tot_pub_debt_out_amt":"39204974715248.65"},
                {"record_date":"2026-06-04","tot_pub_debt_out_amt":"39232150577283.87"},
                {"record_date":"2026-06-05","tot_pub_debt_out_amt":null}
            ]
        }"#;

        let (series, quality) = series_from_bytes(&request, body).expect("treasury series parsed");

        validate_series(std::slice::from_ref(&series)).expect("series validates");
        assert_eq!(series.instrument_id, "US-PUBLIC-DEBT");
        assert_eq!(series.observations.len(), 2);
        assert_eq!(quality.dropped_points, 1);
        assert_eq!(quality.first_date.as_deref(), Some("2026-06-03"));
        assert_eq!(quality.last_date.as_deref(), Some("2026-06-04"));
    }

    #[test]
    fn parses_worldbank_records_and_skips_latest_null() {
        let mut request = source_id_pull_request("worldbank-us-gdp-current-usd");
        let template = apply_public_source_template(&mut request)
            .expect("template resolves")
            .expect("template present");
        let body = br#"[
            {"page":1,"pages":1,"per_page":3,"total":3},
            [
                {"date":"2025","value":null},
                {"date":"2024","value":28750956130731.2},
                {"date":"2023","value":27292170793214.4}
            ]
        ]"#;

        let (series, quality) = series_from_bytes(&request, body).expect("worldbank series parsed");

        validate_api_pull_request(&request, Some(&template)).expect("request validates");
        validate_series(std::slice::from_ref(&series)).expect("series validates");
        assert_eq!(series.instrument_id, "US-GDP-CURRENT-USD");
        assert_eq!(series.observations[0].date, "2023");
        assert_eq!(quality.dropped_points, 1);
    }

    #[test]
    fn parses_coingecko_tuple_arrays() {
        let mut request = source_id_pull_request("coingecko-bitcoin-usd");
        let template = apply_public_source_template(&mut request)
            .expect("template resolves")
            .expect("template present");
        let body = br#"{
            "prices": [
                [1780790400000,60861.88012897632],
                [1780704000000,60921.79441516493]
            ],
            "market_caps": [],
            "total_volumes": []
        }"#;

        let (series, quality) = series_from_bytes(&request, body).expect("coingecko series parsed");

        validate_api_pull_request(&request, Some(&template)).expect("request validates");
        validate_series(std::slice::from_ref(&series)).expect("series validates");
        assert_eq!(series.instrument_id, "BTC-USD");
        assert_eq!(series.observations[0].date, "1780704000000");
        assert_eq!(quality.parser, SourceParser::JsonTupleArray);
        assert_eq!(quality.observed_points, 2);
    }

    #[test]
    fn parses_csv_records_and_drops_missing_values() {
        let request = ApiPullRequest {
            request_id: None,
            source_id: None,
            url: Some("https://example.com/dgs10.csv".to_string()),
            parser: Some(SourceParser::CsvRecords),
            instrument_id: Some("DGS10".to_string()),
            display_name: Some("10-Year Treasury".to_string()),
            asset_class: Some("rates".to_string()),
            currency: Some("PCT".to_string()),
            source: Some("unit-csv".to_string()),
            root_pointer: None,
            date_field: Some("observation_date".to_string()),
            price_field: Some("DGS10".to_string()),
            volume_field: None,
            date_index: None,
            price_index: None,
            volume_index: None,
            auth_header_env: None,
            auth_header_name: None,
        };
        let body = "observation_date,DGS10\n2026-06-01,4.45\n2026-06-02,.\n2026-06-03,4.41\n";

        let (series, quality) =
            series_from_bytes(&request, body.as_bytes()).expect("csv series parsed");

        validate_series(std::slice::from_ref(&series)).expect("series validates");
        assert_eq!(series.observations.len(), 2);
        assert_eq!(quality.dropped_points, 1);
        assert_eq!(quality.min_price, Some(4.41));
    }

    #[test]
    fn source_policy_blocks_private_redirect_targets_and_custom_ports() {
        let link_local = reqwest::Url::parse("https://169.254.169.254/latest/meta-data").unwrap();
        let error = validate_source_url(&link_local, false).expect_err("link-local blocked");
        assert!(error.contains("ECONOMICS_ALLOW_PRIVATE_SOURCE_URLS"));

        let custom_port = reqwest::Url::parse("https://api.worldbank.org:8443/v2").unwrap();
        let error = validate_source_url(&custom_port, false).expect_err("custom port blocked");
        assert!(error.contains("custom source URL ports"));

        let public_172 = reqwest::Url::parse("https://172.200.1.1/data.json").unwrap();
        validate_source_url(&public_172, false).expect("172.200/16 is not RFC1918 private");
    }

    #[test]
    fn source_host_allowlist_restricts_ad_hoc_public_pulls() {
        let allowed = vec!["api.worldbank.org".to_string()];
        let worldbank = reqwest::Url::parse("https://api.worldbank.org/v2/country/US").unwrap();
        let coingecko = reqwest::Url::parse("https://api.coingecko.com/api/v3/ping").unwrap();

        validate_source_host_allowlist(&worldbank, &allowed).expect("worldbank allowed");
        let error = validate_source_host_allowlist(&coingecko, &allowed)
            .expect_err("coingecko blocked by allowlist");
        assert!(error.contains("ECONOMICS_ALLOWED_SOURCE_HOSTS"));
    }

    #[test]
    fn duplicate_ingested_observation_dates_are_rejected() {
        let series = MarketSeries {
            instrument_id: "DUP".to_string(),
            display_name: None,
            asset_class: "equities".to_string(),
            currency: Some("USD".to_string()),
            source: Some("unit".to_string()),
            observations: vec![
                MarketObservation {
                    date: "2026-01-01".to_string(),
                    price: 100.0,
                    volume: None,
                },
                MarketObservation {
                    date: "2026-01-01".to_string(),
                    price: 101.0,
                    volume: None,
                },
            ],
            features: None,
        };

        let error = validate_series(&[series]).expect_err("duplicate date rejected");
        assert!(error.contains("duplicated"));
    }

    #[test]
    fn observability_payload_advertises_explicit_otel_and_dd_log_schema() {
        let state = test_state();
        let payload = observability_payload(&state);

        assert_eq!(payload["ok"], true);
        assert_eq!(payload["loki"]["structuredLogSchema"], "dd.log.v1");
        assert_eq!(payload["otel"]["mode"], "explicit-only");
        assert_eq!(payload["otel"]["autoInstrumentation"], false);
        assert_eq!(payload["otel"]["runtimeMonkeyPatching"], false);
        assert_eq!(payload["prometheus"]["metricsRoute"], "GET /metrics");
    }

    #[test]
    fn integration_health_payload_reports_dependency_status_without_secrets() {
        let mut config = test_config();
        config.server_auth_secret = Some("ultra-private-unit-token".to_string());
        let state = AppState {
            config: Arc::new(config),
            metrics: Arc::new(Metrics::default()),
            nats: None,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("dd-economics-server/0.1 test-source-pull")
                .build()
                .unwrap(),
            series_store: Arc::new(RwLock::new(BTreeMap::new())),
        };
        let payload = integration_health_payload(&state);
        let dependencies = payload["dependencies"]
            .as_array()
            .expect("dependencies array");

        assert_eq!(payload["ok"], true);
        assert_eq!(payload["coreReady"], true);
        assert!(dependencies
            .iter()
            .any(|dependency| dependency["id"] == "source-auth-env-allowlist"
                && dependency["status"] == "ready"));
        assert!(dependencies
            .iter()
            .any(|dependency| dependency["id"] == "spark-pipeline-server"));
        assert!(!payload.to_string().contains("ultra-private-unit-token"));
    }

    #[test]
    fn telemetry_log_record_uses_dd_log_v1_envelope() {
        let record = telemetry_log_record(
            "INFO",
            "economics.unit.test",
            "unit test log",
            json!({ "requestId": "unit" }),
        );

        assert_eq!(record["schema"], "dd.log.v1");
        assert_eq!(record["severity_text"], "INFO");
        assert_eq!(record["severity_number"], 9);
        assert_eq!(record["resource_service_name"], SERVICE_NAME);
        assert_eq!(record["event_name"], "economics.unit.test");
        assert_eq!(record["attributes"]["requestId"], "unit");
    }

    #[tokio::test]
    async fn metrics_expose_source_and_observability_counters() {
        let response = metrics(State(test_state())).await;
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("metrics body bytes");
        let body = String::from_utf8(bytes.to_vec()).expect("metrics utf8");

        assert!(body.contains("dd_economics_server_source_pull_success_total"));
        assert!(body.contains("dd_economics_server_source_pull_failure_total"));
        assert!(body.contains("dd_economics_server_source_pull_bytes_total"));
        assert!(body.contains("dd_economics_server_source_pull_stored_points_total"));
        assert!(body.contains("dd_economics_server_source_pull_last_success_unix_seconds"));
        assert!(body.contains("dd_economics_server_observability_requests_total"));
        assert!(body.contains("dd_economics_server_integration_health_requests_total"));
        assert!(body.contains("dd_economics_server_pipeline_publish_attempts_total"));
        assert!(body.contains("dd_economics_server_pipeline_publish_success_total"));
        assert!(body.contains("dd_economics_server_pipeline_publish_failure_total"));
        assert!(body.contains("dd_economics_server_pipeline_submit_success_total"));
        assert!(body.contains("dd_economics_server_pipeline_submit_failure_total"));
    }

    #[tokio::test]
    #[ignore = "uses live public APIs and should be run manually"]
    async fn public_source_templates_fetch_live_external_data_when_available() {
        let state = test_state();
        let mut successes = 0usize;
        for source_id in [
            "treasury-debt-to-penny",
            "worldbank-us-gdp-current-usd",
            "coingecko-bitcoin-usd",
        ] {
            match pull_source(&state, source_id_pull_request(source_id)).await {
                Ok(response) => {
                    assert!(response.stored_points >= 2);
                    assert!(response.quality.is_some());
                    successes += 1;
                }
                Err(error) => {
                    eprintln!("live public source {source_id} unavailable or changed: {error}");
                }
            }
        }
        if successes == 0 {
            eprintln!("no live public sources were reachable; skipping external assertions");
            return;
        }
        let stored = state.series_store.read().unwrap().len();
        assert!(stored >= successes);
    }
}
