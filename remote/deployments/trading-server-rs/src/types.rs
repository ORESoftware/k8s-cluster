use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecisionRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) schema_version: Option<String>,
    pub(crate) symbol: String,
    pub(crate) venue: Option<String>,
    pub(crate) target_platform: Option<String>,
    pub(crate) strategy: Option<String>,
    pub(crate) horizon: Option<String>,
    pub(crate) portfolio: Option<PortfolioSnapshot>,
    pub(crate) market: Option<MarketSnapshot>,
    pub(crate) web_signals: Option<Vec<WebSignal>>,
    pub(crate) ml_features: Option<Vec<ModelFeature>>,
    pub(crate) mdp_policy: Option<MdpPolicyHint>,
    pub(crate) constraints: Option<RiskLimits>,
    pub(crate) dry_run: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PortfolioSnapshot {
    pub(crate) cash: Option<f64>,
    pub(crate) equity: Option<f64>,
    pub(crate) current_position: Option<f64>,
    pub(crate) average_entry_price: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MarketSnapshot {
    pub(crate) last_price: Option<f64>,
    pub(crate) bid: Option<f64>,
    pub(crate) ask: Option<f64>,
    pub(crate) day_volume: Option<f64>,
    pub(crate) realized_volatility: Option<f64>,
    pub(crate) prices: Option<Vec<f64>>,
    // Epoch-ms timestamp of when this market snapshot was captured. Optional;
    // when present it drives the `marketDataFresh` safety gate so stale quotes
    // can't produce a live order intent.
    pub(crate) as_of_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebSignal {
    pub(crate) source: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) sentiment: f64,
    pub(crate) confidence: Option<f64>,
    pub(crate) relevance: Option<f64>,
    pub(crate) age_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelFeature {
    pub(crate) name: String,
    pub(crate) value: f64,
    pub(crate) weight: Option<f64>,
    pub(crate) higher_is_better: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MdpPolicyHint {
    pub(crate) action: String,
    pub(crate) confidence: Option<f64>,
    pub(crate) value: Option<f64>,
    pub(crate) risk: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RiskLimits {
    pub(crate) max_order_notional: Option<f64>,
    pub(crate) max_position_notional: Option<f64>,
    pub(crate) max_symbol_exposure_pct: Option<f64>,
    pub(crate) min_confidence: Option<f64>,
    pub(crate) max_risk_score: Option<f64>,
    pub(crate) allow_short: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecisionResponse {
    pub(crate) ok: bool,
    pub(crate) request_id: String,
    pub(crate) schema_version: &'static str,
    pub(crate) symbol: String,
    pub(crate) venue: Option<String>,
    pub(crate) strategy: String,
    pub(crate) horizon: String,
    pub(crate) mode: String,
    pub(crate) recommended_action: String,
    pub(crate) final_action: String,
    pub(crate) confidence: f64,
    pub(crate) risk_score: f64,
    pub(crate) raw_score: f64,
    pub(crate) execution_status: String,
    pub(crate) components: Vec<ScoreComponent>,
    pub(crate) safety_checks: Vec<SafetyCheck>,
    pub(crate) order_intent: Option<OrderIntent>,
    pub(crate) warnings: Vec<String>,
    pub(crate) generated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScoreComponent {
    pub(crate) name: String,
    pub(crate) score: f64,
    pub(crate) weight: f64,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafetyCheck {
    pub(crate) name: String,
    pub(crate) ok: bool,
    pub(crate) severity: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OrderIntent {
    pub(crate) request_id: String,
    pub(crate) symbol: String,
    pub(crate) platform: String,
    pub(crate) platform_display_name: String,
    pub(crate) credential_secret: String,
    pub(crate) credential_keys: Vec<String>,
    pub(crate) side: String,
    pub(crate) order_type: String,
    pub(crate) quantity: f64,
    pub(crate) notional: f64,
    pub(crate) reference_price: f64,
    pub(crate) mode: String,
    pub(crate) dry_run: bool,
    pub(crate) intent_only: bool,
    pub(crate) subject: String,
    pub(crate) generated_at_ms: u128,
}
