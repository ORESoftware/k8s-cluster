use std::{
    collections::BTreeMap,
    env,
    error::Error,
    net::SocketAddr,
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
use dd_nats_subject_defs::RUNTIME_EVENTS_SUBJECT;
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
const MAX_SENTIMENT_DOCUMENTS: usize = 512;
const MAX_SENTIMENT_TEXT_BYTES: usize = 4_096;
const ECONOMICS_FORECAST_REQUEST_SUBJECT: &str = "dd.remote.economics.forecast.requests";
const ECONOMICS_FORECAST_RESULT_SUBJECT: &str = "dd.remote.economics.forecast.results";
const ECONOMICS_MARKET_EVENT_SUBJECT: &str = "dd.remote.economics.market.events";
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
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    forecasts_total: AtomicU64,
    ingest_requests_total: AtomicU64,
    source_pull_total: AtomicU64,
    sentiment_requests_total: AtomicU64,
    recommendation_requests_total: AtomicU64,
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
    url: String,
    instrument_id: Option<String>,
    display_name: Option<String>,
    asset_class: Option<String>,
    currency: Option<String>,
    source: Option<String>,
    root_pointer: Option<String>,
    date_field: Option<String>,
    price_field: Option<String>,
    volume_field: Option<String>,
    auth_header_env: Option<String>,
    auth_header_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiPullResponse {
    ok: bool,
    request_id: String,
    source: String,
    url_host: String,
    http_status: u16,
    bytes: usize,
    stored_points: usize,
    instrument_id: Option<String>,
    warnings: Vec<String>,
    fetched_at_ms: u128,
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
    sec_api_key: bool,
    crunchbase_api_key: bool,
    pitchbook_api_key: bool,
    cb_insights_api_key: bool,
    dealroom_api_key: bool,
    preqin_api_key: bool,
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
            "POST",
            "/sources/pull",
            "Fetch JSON market history from an approved API URL.",
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
            auth: "public/private",
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
        "macro": {
            "indicators": "GET /macro/indicators reports built-in fiscal/labor sample context and public/private credential placeholders."
        },
        "recommendations": {
            "route": "POST /recommendations",
            "companies": "Returns top 20 invest candidates and top 20 dump/hedge candidates from the model universe.",
            "commodities": "Returns top 30 buy candidates and top 30 sell-or-dump candidates from major tradable commodities."
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
            "pullSource": "POST /sources/pull",
            "sentimentSources": "GET /sentiment/sources",
            "sentimentAnalyze": "POST /sentiment/analyze",
            "macroIndicators": "GET /macro/indicators",
            "vcInvestment": "GET /vc/investment",
            "recommendations": "POST /recommendations",
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
            "runtimeEventSubject": state.config.runtime_event_subject
        },
        "desEngine": des_surface_descriptor(),
        "sentiment": {
            "credentialStatus": &state.config.sentiment_credentials,
            "sourcesRoute": "GET /sentiment/sources",
            "analyzeRoute": "POST /sentiment/analyze"
        },
        "marketData": {
            "credentialStatus": &state.config.market_data_credentials,
            "macroRoute": "GET /macro/indicators",
            "vcRoute": "GET /vc/investment",
            "recommendationsRoute": "POST /recommendations"
        },
        "equationCount": equation_catalog().len(),
        "sourceCount": source_catalog().len(),
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
        for (index, point) in item.observations.iter().enumerate() {
            clean_token(&point.date, "observation.date")?;
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

fn generate_recommendations(
    config: &Config,
    request: RecommendationRequest,
) -> Result<RecommendationsResponse, String> {
    if let Some(schema) = request.schema_version.as_deref() {
        if schema != SCHEMA_VERSION {
            return Err(format!("schemaVersion must be {SCHEMA_VERSION}"));
        }
    }

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
            eprintln!("economics server failed to encode forecast result: {error}");
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
        println!("economics server nats loop disabled: NATS_URL is not configured");
        return;
    };
    println!(
        "economics server nats loop starting: subject={} queueGroup={} resultSubject={}",
        state.config.request_subject, state.config.queue_group, state.config.result_subject
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
            eprintln!("economics server nats subscribe failed: {error}");
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
            eprintln!(
                "economics server rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                payload.len()
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
                        eprintln!("economics server nats forecast failed: {error}");
                    }
                },
                Err(error) => {
                    task_state
                        .metrics
                        .errors_total
                        .fetch_add(1, Ordering::Relaxed);
                    eprintln!("economics server invalid nats forecast request: {error}");
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
        "pullRoute": "POST /sources/pull",
        "ingestRoute": "POST /ingest",
        "sentimentSourcesRoute": "GET /sentiment/sources",
        "macroIndicatorsRoute": "GET /macro/indicators",
        "vcInvestmentRoute": "GET /vc/investment",
        "recommendationsRoute": "POST /recommendations"
    }))
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
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    }
}

async fn pull_source(state: &AppState, request: ApiPullRequest) -> Result<ApiPullResponse, String> {
    let parsed_url = reqwest::Url::parse(request.url.trim())
        .map_err(|error| format!("url is invalid: {error}"))?;
    validate_source_url(&parsed_url, state.config.allow_private_source_urls)?;
    let mut http_request = state.http.get(parsed_url.clone());
    if let Some(env_name) = request.auth_header_env.as_deref() {
        let env_name = clean_token(env_name, "authHeaderEnv")?;
        let header_value = optional_env(&env_name)
            .ok_or_else(|| format!("auth header env var {env_name} is not configured"))?;
        let header_name = request
            .auth_header_name
            .as_deref()
            .unwrap_or("authorization")
            .parse::<reqwest::header::HeaderName>()
            .map_err(|error| format!("authHeaderName is invalid: {error}"))?;
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
    if request.instrument_id.is_some()
        && request.asset_class.is_some()
        && request.date_field.is_some()
        && request.price_field.is_some()
    {
        let json_value = serde_json::from_slice::<Value>(&bytes)
            .map_err(|error| format!("source response is not JSON: {error}"))?;
        let series = series_from_json(&request, &json_value)?;
        stored_points = series.observations.len();
        instrument_id = Some(series.instrument_id.clone());
        validate_series(std::slice::from_ref(&series))?;
        let mut store = state
            .series_store
            .write()
            .map_err(|_| "series store lock poisoned".to_string())?;
        store.insert(series.instrument_id.clone(), series);
    } else {
        warnings.push(
            "response fetched but not stored; provide instrumentId, assetClass, dateField, and priceField to parse JSON series"
                .to_string(),
        );
    }
    let host = parsed_url.host_str().unwrap_or("unknown").to_string();
    let response = ApiPullResponse {
        ok: true,
        request_id,
        source: request
            .source
            .unwrap_or_else(|| "ad-hoc-api".to_string())
            .chars()
            .take(MAX_TOKEN_LEN)
            .collect(),
        url_host: host,
        http_status: status.as_u16(),
        bytes: bytes.len(),
        stored_points,
        instrument_id,
        warnings,
        fetched_at_ms: now_ms(),
    };
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

fn validate_source_url(url: &reqwest::Url, allow_private: bool) -> Result<(), String> {
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
    let private_host = host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.starts_with("172.16.")
        || host.starts_with("172.17.")
        || host.starts_with("172.18.")
        || host.starts_with("172.19.")
        || host.starts_with("172.2")
        || host.starts_with("172.30.")
        || host.starts_with("172.31.");
    if private_host && !allow_private {
        return Err(
            "private source hosts require ECONOMICS_ALLOW_PRIVATE_SOURCE_URLS=true".to_string(),
        );
    }
    if url.username() != "" || url.password().is_some() {
        return Err("source URL credentials are not allowed".to_string());
    }
    Ok(())
}

fn series_from_json(request: &ApiPullRequest, value: &Value) -> Result<MarketSeries, String> {
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
    for item in items.iter().take(MAX_OBSERVATIONS_PER_SERIES) {
        let date = field_value(item, date_field)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("dateField {date_field} missing or not a string"))?;
        let price = field_value(item, price_field)
            .and_then(number_from_value)
            .ok_or_else(|| format!("priceField {price_field} missing or not numeric"))?;
        let volume = volume_field
            .and_then(|field| field_value(item, field))
            .and_then(number_from_value);
        observations.push(MarketObservation {
            date: date.to_string(),
            price,
            volume,
        });
    }
    Ok(MarketSeries {
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
    })
}

fn field_value<'a>(value: &'a Value, field: &str) -> Option<&'a Value> {
    if field.starts_with('/') {
        value.pointer(field)
    } else {
        value.get(field)
    }
}

fn number_from_value(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<f64>().ok())
        })
        .filter(|number| number.is_finite())
}

async fn metrics(State(state): State<AppState>) -> Response {
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
         # HELP dd_economics_server_sentiment_requests_total Sentiment analysis requests accepted.\n\
         # TYPE dd_economics_server_sentiment_requests_total counter\n\
         dd_economics_server_sentiment_requests_total {}\n\
         # HELP dd_economics_server_recommendation_requests_total Recommendation requests accepted.\n\
         # TYPE dd_economics_server_recommendation_requests_total counter\n\
         dd_economics_server_recommendation_requests_total {}\n\
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
        state.metrics.sentiment_requests_total.load(Ordering::Relaxed),
        state
            .metrics
            .recommendation_requests_total
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
        .route("/sources/pull", post(pull_source_http))
        .route("/sentiment/sources", get(sentiment_sources))
        .route("/sentiment/analyze", post(sentiment_analyze_http))
        .route("/macro/indicators", get(macro_indicators))
        .route("/vc/investment", get(vc_investment))
        .route("/recommendations", post(recommendations_http))
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
    println!("{SERVICE_NAME} listening on http://{addr}");
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
            url: "https://example.com/data.json".to_string(),
            instrument_id: Some("TEST".to_string()),
            display_name: Some("Test".to_string()),
            asset_class: Some("equities".to_string()),
            currency: Some("USD".to_string()),
            source: Some("unit".to_string()),
            root_pointer: Some("/prices".to_string()),
            date_field: Some("d".to_string()),
            price_field: Some("p".to_string()),
            volume_field: Some("v".to_string()),
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
}
