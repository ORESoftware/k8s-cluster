use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::shared::*;
use crate::state::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ForecastRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) schema_version: Option<String>,
    pub(crate) horizon_months: Option<u32>,
    pub(crate) confidence_level: Option<f64>,
    pub(crate) scenario: Option<String>,
    pub(crate) series: Option<Vec<MarketSeries>>,
    pub(crate) macro_context: Option<MacroContext>,
    pub(crate) macro_fiscal_context: Option<MacroFiscalContext>,
    pub(crate) venture_capital_context: Option<VentureCapitalContext>,
    pub(crate) theory_weights: Option<TheoryWeights>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IngestRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) replace: Option<bool>,
    pub(crate) series: Vec<MarketSeries>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MarketSeries {
    pub(crate) instrument_id: String,
    pub(crate) display_name: Option<String>,
    pub(crate) asset_class: String,
    pub(crate) currency: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) observations: Vec<MarketObservation>,
    pub(crate) features: Option<AssetFeatures>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MarketObservation {
    pub(crate) date: String,
    pub(crate) price: f64,
    pub(crate) volume: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AssetFeatures {
    pub(crate) beta: Option<f64>,
    pub(crate) duration: Option<f64>,
    pub(crate) carry: Option<f64>,
    pub(crate) convenience_yield: Option<f64>,
    pub(crate) storage_cost: Option<f64>,
    pub(crate) supply_growth: Option<f64>,
    pub(crate) demand_growth: Option<f64>,
    pub(crate) inventory_ratio: Option<f64>,
    pub(crate) valuation_gap: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MacroContext {
    pub(crate) policy_rate: Option<f64>,
    pub(crate) foreign_policy_rate: Option<f64>,
    pub(crate) inflation: Option<f64>,
    pub(crate) foreign_inflation: Option<f64>,
    pub(crate) expected_inflation: Option<f64>,
    pub(crate) money_supply_growth: Option<f64>,
    pub(crate) real_growth: Option<f64>,
    pub(crate) output_gap: Option<f64>,
    pub(crate) unemployment_gap: Option<f64>,
    pub(crate) risk_free_rate: Option<f64>,
    pub(crate) market_return: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MacroFiscalContext {
    pub(crate) country: Option<String>,
    pub(crate) period: Option<String>,
    pub(crate) gdp: Option<f64>,
    pub(crate) gdp_growth: Option<f64>,
    pub(crate) national_debt: Option<f64>,
    pub(crate) debt_to_gdp: Option<f64>,
    pub(crate) deficit: Option<f64>,
    pub(crate) deficit_to_gdp: Option<f64>,
    pub(crate) receipts: Option<f64>,
    pub(crate) outlays: Option<f64>,
    pub(crate) borrowing: Option<f64>,
    pub(crate) net_interest_outlays: Option<f64>,
    pub(crate) labor_force_participation: Option<f64>,
    pub(crate) prime_age_participation: Option<f64>,
    pub(crate) unemployment_rate: Option<f64>,
    pub(crate) payroll_growth: Option<f64>,
    pub(crate) wage_growth: Option<f64>,
    pub(crate) productivity_growth: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VentureCapitalContext {
    pub(crate) period: Option<String>,
    pub(crate) deals: Vec<VentureCapitalDealSignal>,
    pub(crate) sector_flows: Vec<VentureSectorFlow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VentureCapitalDealSignal {
    pub(crate) firm: String,
    pub(crate) company: String,
    pub(crate) sector: String,
    pub(crate) stage: String,
    pub(crate) amount: f64,
    pub(crate) currency: Option<String>,
    pub(crate) country: Option<String>,
    pub(crate) announced_at: Option<String>,
    pub(crate) confidence: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VentureSectorFlow {
    pub(crate) sector: String,
    pub(crate) deal_count: u32,
    pub(crate) invested_capital: f64,
    pub(crate) yoy_growth: f64,
    pub(crate) dry_powder: Option<f64>,
    pub(crate) exit_liquidity: Option<f64>,
    pub(crate) confidence: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TheoryWeights {
    pub(crate) data: Option<f64>,
    pub(crate) macro_theory: Option<f64>,
    pub(crate) momentum: Option<f64>,
    pub(crate) mean_reversion: Option<f64>,
    pub(crate) carry: Option<f64>,
    pub(crate) valuation: Option<f64>,
    pub(crate) jump_stress: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApiPullRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) source_id: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) parser: Option<SourceParser>,
    pub(crate) instrument_id: Option<String>,
    pub(crate) display_name: Option<String>,
    pub(crate) asset_class: Option<String>,
    pub(crate) currency: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) root_pointer: Option<String>,
    pub(crate) date_field: Option<String>,
    pub(crate) price_field: Option<String>,
    pub(crate) volume_field: Option<String>,
    pub(crate) date_index: Option<usize>,
    pub(crate) price_index: Option<usize>,
    pub(crate) volume_index: Option<usize>,
    pub(crate) auth_header_env: Option<String>,
    pub(crate) auth_header_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SourceParser {
    JsonRecords,
    JsonTupleArray,
    CsvRecords,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApiPullResponse {
    pub(crate) ok: bool,
    pub(crate) request_id: String,
    pub(crate) source_id: Option<String>,
    pub(crate) source: String,
    pub(crate) parser: Option<SourceParser>,
    pub(crate) url_host: String,
    pub(crate) http_status: u16,
    pub(crate) bytes: usize,
    pub(crate) stored_points: usize,
    pub(crate) instrument_id: Option<String>,
    pub(crate) quality: Option<SourceQualityReport>,
    pub(crate) warnings: Vec<String>,
    pub(crate) fetched_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceQualityReport {
    pub(crate) parser: SourceParser,
    pub(crate) observed_points: usize,
    pub(crate) dropped_points: usize,
    pub(crate) first_date: Option<String>,
    pub(crate) last_date: Option<String>,
    pub(crate) min_price: Option<f64>,
    pub(crate) max_price: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ForecastResponse {
    pub(crate) ok: bool,
    pub(crate) request_id: String,
    pub(crate) schema_version: &'static str,
    pub(crate) history_years: u32,
    pub(crate) horizon_months: u32,
    pub(crate) confidence_level: f64,
    pub(crate) scenario: String,
    pub(crate) generated_at_ms: u128,
    pub(crate) des_engine: Value,
    pub(crate) equations: Vec<EquationDescriptor>,
    pub(crate) projections: Vec<Projection>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Projection {
    pub(crate) instrument_id: String,
    pub(crate) display_name: String,
    pub(crate) asset_class: String,
    pub(crate) currency: String,
    pub(crate) last_price: f64,
    pub(crate) annualized_drift: f64,
    pub(crate) annualized_volatility: f64,
    pub(crate) expected_return_18m: f64,
    pub(crate) signal: String,
    pub(crate) rationale: Vec<String>,
    pub(crate) components: Vec<ModelComponent>,
    pub(crate) points: Vec<ForecastPoint>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ForecastPoint {
    pub(crate) month: u32,
    pub(crate) label: String,
    pub(crate) expected: f64,
    pub(crate) lower: f64,
    pub(crate) upper: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelComponent {
    pub(crate) name: String,
    pub(crate) value: f64,
    pub(crate) weight: f64,
    pub(crate) equation: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EquationDescriptor {
    pub(crate) name: &'static str,
    pub(crate) family: &'static str,
    pub(crate) equation: &'static str,
    pub(crate) use_case: &'static str,
    pub(crate) caveat: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceDescriptor {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) asset_classes: &'static [&'static str],
    pub(crate) auth: &'static str,
    pub(crate) notes: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicSourceTemplate {
    pub(crate) id: &'static str,
    pub(crate) provider: &'static str,
    pub(crate) name: &'static str,
    pub(crate) asset_class: &'static str,
    pub(crate) instrument_id: &'static str,
    pub(crate) display_name: &'static str,
    pub(crate) currency: &'static str,
    pub(crate) source: &'static str,
    pub(crate) url: &'static str,
    pub(crate) host: &'static str,
    pub(crate) parser: SourceParser,
    pub(crate) root_pointer: Option<&'static str>,
    pub(crate) date_field: Option<&'static str>,
    pub(crate) price_field: Option<&'static str>,
    pub(crate) volume_field: Option<&'static str>,
    pub(crate) date_index: Option<usize>,
    pub(crate) price_index: Option<usize>,
    pub(crate) volume_index: Option<usize>,
    pub(crate) cadence: &'static str,
    pub(crate) documentation_url: &'static str,
    pub(crate) notes: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SentimentCredentialStatus {
    pub(crate) x_bearer_token: bool,
    pub(crate) x_api_key: bool,
    pub(crate) x_api_secret: bool,
    pub(crate) x_access_token: bool,
    pub(crate) x_access_token_secret: bool,
    pub(crate) reddit_client_id: bool,
    pub(crate) reddit_client_secret: bool,
    pub(crate) reddit_user_agent: bool,
    pub(crate) news_api_key: bool,
    pub(crate) stocktwits_token: bool,
    pub(crate) gdelt_api_key: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MarketDataCredentialStatus {
    pub(crate) fred_api_key: bool,
    pub(crate) bea_api_key: bool,
    pub(crate) bls_api_key: bool,
    pub(crate) treasury_api_key: bool,
    pub(crate) census_api_key: bool,
    pub(crate) eia_api_key: bool,
    pub(crate) coingecko_api_key: bool,
    pub(crate) sec_api_key: bool,
    pub(crate) crunchbase_api_key: bool,
    pub(crate) pitchbook_api_key: bool,
    pub(crate) cb_insights_api_key: bool,
    pub(crate) dealroom_api_key: bool,
    pub(crate) preqin_api_key: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PipelineIntegrationStatus {
    pub(crate) spark_pipeline_url_configured: bool,
    pub(crate) spark_pipeline_auth_configured: bool,
    pub(crate) spark_pipeline_submit_enabled: bool,
    pub(crate) spark_pipeline_url: Option<String>,
    pub(crate) spark_pipeline_auth_env: String,
    pub(crate) spark_master_url: String,
    pub(crate) airflow_api_url_configured: bool,
    pub(crate) airflow_api_url: Option<String>,
    pub(crate) databricks_host_configured: bool,
    pub(crate) databricks_token_configured: bool,
    pub(crate) data_lake_uri: String,
    pub(crate) pipeline_intent_subject: String,
    pub(crate) nats_configured: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IntegrationDependencyStatus {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) configured: bool,
    pub(crate) required_for_core_readiness: bool,
    pub(crate) mode: String,
    pub(crate) details: Value,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SentimentAnalyzeRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) schema_version: Option<String>,
    pub(crate) query: Option<String>,
    pub(crate) instrument_ids: Option<Vec<String>>,
    pub(crate) documents: Vec<SentimentDocument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SentimentDocument {
    pub(crate) source: String,
    pub(crate) text: String,
    pub(crate) url: Option<String>,
    pub(crate) author: Option<String>,
    pub(crate) published_at: Option<String>,
    pub(crate) weight: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SentimentAnalyzeResponse {
    pub(crate) ok: bool,
    pub(crate) request_id: String,
    pub(crate) schema_version: &'static str,
    pub(crate) query: Option<String>,
    pub(crate) document_count: usize,
    pub(crate) average_sentiment: f64,
    pub(crate) confidence: f64,
    pub(crate) source_scores: Vec<SentimentSourceScore>,
    pub(crate) top_terms: Vec<String>,
    pub(crate) credential_status: SentimentCredentialStatus,
    pub(crate) generated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SentimentSourceScore {
    pub(crate) source: String,
    pub(crate) document_count: usize,
    pub(crate) average_sentiment: f64,
    pub(crate) confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SentimentSignalContext {
    pub(crate) average_sentiment: Option<f64>,
    pub(crate) instrument_scores: Option<BTreeMap<String, f64>>,
    pub(crate) sector_scores: Option<BTreeMap<String, f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecommendationRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) schema_version: Option<String>,
    pub(crate) horizon_months: Option<u32>,
    pub(crate) company_limit: Option<usize>,
    pub(crate) commodity_limit: Option<usize>,
    pub(crate) scenario: Option<String>,
    pub(crate) series: Option<Vec<MarketSeries>>,
    pub(crate) macro_context: Option<MacroContext>,
    pub(crate) macro_fiscal_context: Option<MacroFiscalContext>,
    pub(crate) venture_capital_context: Option<VentureCapitalContext>,
    pub(crate) sentiment_context: Option<SentimentSignalContext>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecommendationsResponse {
    pub(crate) ok: bool,
    pub(crate) request_id: String,
    pub(crate) schema_version: &'static str,
    pub(crate) horizon_months: u32,
    pub(crate) scenario: String,
    pub(crate) generated_at_ms: u128,
    pub(crate) macro_fiscal_context: MacroFiscalContext,
    pub(crate) venture_capital_context: VentureCapitalContext,
    pub(crate) data_credential_status: MarketDataCredentialStatus,
    pub(crate) company_buys: Vec<CompanyRecommendation>,
    pub(crate) company_dumps: Vec<CompanyRecommendation>,
    pub(crate) commodity_buys: Vec<CommodityRecommendation>,
    pub(crate) commodity_sells_or_dumps: Vec<CommodityRecommendation>,
    pub(crate) methodology: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanyRecommendation {
    pub(crate) rank: usize,
    pub(crate) ticker: String,
    pub(crate) company: String,
    pub(crate) sector: String,
    pub(crate) stage: String,
    pub(crate) action: String,
    pub(crate) score: f64,
    pub(crate) expected_return_18m: f64,
    pub(crate) confidence: f64,
    pub(crate) reasons: Vec<String>,
    pub(crate) components: Vec<RecommendationComponent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommodityRecommendation {
    pub(crate) rank: usize,
    pub(crate) instrument_id: String,
    pub(crate) commodity: String,
    pub(crate) commodity_class: String,
    pub(crate) action: String,
    pub(crate) score: f64,
    pub(crate) expected_return_18m: f64,
    pub(crate) confidence: f64,
    pub(crate) reasons: Vec<String>,
    pub(crate) components: Vec<RecommendationComponent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecommendationComponent {
    pub(crate) name: String,
    pub(crate) value: f64,
    pub(crate) weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PipelinePlanRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) schema_version: Option<String>,
    pub(crate) scenario: Option<String>,
    pub(crate) data_lake_uri: Option<String>,
    pub(crate) include_recommendations: Option<bool>,
    pub(crate) publish_to_nats: Option<bool>,
    pub(crate) job_kinds: Option<Vec<String>>,
    pub(crate) recommendation_request: Option<RecommendationRequest>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PipelinePlanResponse {
    pub(crate) ok: bool,
    pub(crate) request_id: String,
    pub(crate) schema_version: &'static str,
    pub(crate) generated_at_ms: u128,
    pub(crate) pipeline_status: PipelineIntegrationStatus,
    pub(crate) recommendation_summary: Value,
    pub(crate) job_intents: Vec<PipelineJobIntent>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PipelineJobIntent {
    pub(crate) id: String,
    pub(crate) engine: String,
    pub(crate) target: String,
    pub(crate) kind: String,
    pub(crate) endpoint: Option<String>,
    pub(crate) auth_required: bool,
    pub(crate) submit_eligible: bool,
    pub(crate) params: Value,
    pub(crate) notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PipelineSubmitResponse {
    pub(crate) ok: bool,
    pub(crate) request_id: String,
    pub(crate) schema_version: &'static str,
    pub(crate) generated_at_ms: u128,
    pub(crate) plan: PipelinePlanResponse,
    pub(crate) submitted_jobs: Vec<PipelineSubmittedJob>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PipelineSubmittedJob {
    pub(crate) intent_id: String,
    pub(crate) target: String,
    pub(crate) http_status: Option<u16>,
    pub(crate) accepted: bool,
    pub(crate) response: Option<Value>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CompanyCandidate {
    pub(crate) ticker: &'static str,
    pub(crate) company: &'static str,
    pub(crate) sector: &'static str,
    pub(crate) stage: &'static str,
    pub(crate) beta: f64,
    pub(crate) profitability: f64,
    pub(crate) growth: f64,
    pub(crate) balance_sheet: f64,
    pub(crate) valuation_gap: f64,
    pub(crate) momentum: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct CommodityCandidate {
    pub(crate) instrument_id: &'static str,
    pub(crate) commodity: &'static str,
    pub(crate) commodity_class: &'static str,
    pub(crate) supply_tightness: f64,
    pub(crate) demand_growth: f64,
    pub(crate) inventory_pressure: f64,
    pub(crate) carry: f64,
    pub(crate) geopolitical_risk: f64,
    pub(crate) valuation_gap: f64,
    pub(crate) volatility: f64,
}

pub(crate) struct SeriesStats {
    pub(crate) last_price: f64,
    pub(crate) volatility_per_period: f64,
    pub(crate) periods_per_year: f64,
    pub(crate) data_drift: f64,
    pub(crate) momentum: f64,
    pub(crate) mean_reversion: f64,
}

pub(crate) struct TheoryPrior {
    pub(crate) drift: f64,
    pub(crate) carry: f64,
    pub(crate) valuation: f64,
    pub(crate) jump_stress: f64,
    pub(crate) rationale: Vec<String>,
}

pub(crate) struct NormalizedWeights {
    pub(crate) data: f64,
    pub(crate) macro_theory: f64,
    pub(crate) momentum: f64,
    pub(crate) mean_reversion: f64,
    pub(crate) carry: f64,
    pub(crate) valuation: f64,
    pub(crate) jump_stress: f64,
}

pub(crate) enum AuthFailure {
    MissingSecret,
    Unauthorized,
}

pub(crate) fn validate_series(series: &[MarketSeries]) -> Result<(), String> {
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

pub(crate) fn validate_optional_number(
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

pub(crate) fn validate_macro_context(context: Option<&MacroContext>) -> Result<(), String> {
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

pub(crate) fn validate_macro_fiscal_context(context: Option<&MacroFiscalContext>) -> Result<(), String> {
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

pub(crate) fn validate_venture_capital_context(context: Option<&VentureCapitalContext>) -> Result<(), String> {
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

pub(crate) fn validate_sentiment_context(context: Option<&SentimentSignalContext>) -> Result<(), String> {
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

pub(crate) fn validate_sentiment_score_map(
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
