use std::{
    collections::{BTreeMap, BTreeSet},
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
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::sleep;

const SCHEMA_VERSION: &str = "trading.decision.v1";
const SERVICE_NAME: &str = "dd-trading-server";
const MAX_HTTP_BODY_BYTES: usize = 512 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 512 * 1024;
const MAX_SYMBOL_LEN: usize = 32;
const MAX_REQUEST_ID_LEN: usize = 128;
const MAX_LABEL_LEN: usize = 96;
const MAX_WEB_SIGNALS: usize = 128;
const MAX_FEATURES: usize = 128;
const MAX_PRICE_POINTS: usize = 512;
const MAX_SIGNAL_AGE_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    platform_config: Arc<RwLock<TradingPlatformConfig>>,
    nats: Option<async_nats::Client>,
    metrics: Arc<Metrics>,
}

#[derive(Clone)]
struct Config {
    trading_mode: String,
    live_orders_enabled: bool,
    server_auth_secret: Option<String>,
    allow_unauthenticated: bool,
    database_url: Option<String>,
    app_config_scope: String,
    app_config_key: String,
    config_refresh: Duration,
    scraper_base_url: String,
    ml_base_url: String,
    mdp_base_url: String,
    signal_subject: String,
    queue_group: String,
    decision_subject: String,
    order_intent_subject: String,
    event_subject: String,
    default_limits: RiskLimits,
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    decisions_total: AtomicU64,
    order_intents_total: AtomicU64,
    blocked_orders_total: AtomicU64,
    auth_failures_total: AtomicU64,
    errors_total: AtomicU64,
    nats_messages_total: AtomicU64,
    nats_published_total: AtomicU64,
    config_refresh_total: AtomicU64,
    config_refresh_failures_total: AtomicU64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TradingPlatformConfig {
    platforms: Vec<TradingPlatform>,
    default_platform: Option<String>,
    last_config_refresh_ms: Option<u128>,
    last_config_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TradingPlatform {
    slug: String,
    display_name: String,
    provider: String,
    status: String,
    supports_paper: bool,
    supports_live: bool,
    asset_classes: Vec<String>,
    order_types: Vec<String>,
    base_urls: BTreeMap<String, String>,
    credential_secret: String,
    credential_keys: Vec<String>,
    account_ref_key: Option<String>,
    labels: Vec<String>,
    meta_data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DecisionRequest {
    request_id: Option<String>,
    schema_version: Option<String>,
    symbol: String,
    venue: Option<String>,
    target_platform: Option<String>,
    strategy: Option<String>,
    horizon: Option<String>,
    portfolio: Option<PortfolioSnapshot>,
    market: Option<MarketSnapshot>,
    web_signals: Option<Vec<WebSignal>>,
    ml_features: Option<Vec<ModelFeature>>,
    mdp_policy: Option<MdpPolicyHint>,
    constraints: Option<RiskLimits>,
    dry_run: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortfolioSnapshot {
    cash: Option<f64>,
    equity: Option<f64>,
    current_position: Option<f64>,
    average_entry_price: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarketSnapshot {
    last_price: Option<f64>,
    bid: Option<f64>,
    ask: Option<f64>,
    day_volume: Option<f64>,
    realized_volatility: Option<f64>,
    prices: Option<Vec<f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebSignal {
    source: Option<String>,
    url: Option<String>,
    title: Option<String>,
    sentiment: f64,
    confidence: Option<f64>,
    relevance: Option<f64>,
    age_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelFeature {
    name: String,
    value: f64,
    weight: Option<f64>,
    higher_is_better: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MdpPolicyHint {
    action: String,
    confidence: Option<f64>,
    value: Option<f64>,
    risk: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RiskLimits {
    max_order_notional: Option<f64>,
    max_position_notional: Option<f64>,
    max_symbol_exposure_pct: Option<f64>,
    min_confidence: Option<f64>,
    max_risk_score: Option<f64>,
    allow_short: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DecisionResponse {
    ok: bool,
    request_id: String,
    schema_version: &'static str,
    symbol: String,
    venue: Option<String>,
    strategy: String,
    horizon: String,
    mode: String,
    recommended_action: String,
    final_action: String,
    confidence: f64,
    risk_score: f64,
    raw_score: f64,
    execution_status: String,
    components: Vec<ScoreComponent>,
    safety_checks: Vec<SafetyCheck>,
    order_intent: Option<OrderIntent>,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScoreComponent {
    name: String,
    score: f64,
    weight: f64,
    reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SafetyCheck {
    name: String,
    ok: bool,
    severity: String,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OrderIntent {
    request_id: String,
    symbol: String,
    platform: String,
    platform_display_name: String,
    credential_secret: String,
    credential_keys: Vec<String>,
    side: String,
    order_type: String,
    quantity: f64,
    notional: f64,
    reference_price: f64,
    mode: String,
    dry_run: bool,
    intent_only: bool,
    subject: String,
    generated_at_ms: u128,
}

struct CandidateOrderContext<'a> {
    request_id: &'a str,
    symbol: &'a str,
    platform: Option<&'a TradingPlatform>,
    action: &'a str,
    price: Option<f64>,
    limits: &'a RiskLimits,
    request: &'a DecisionRequest,
    config: &'a Config,
    confidence: f64,
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

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| optional_env(key))
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

fn env_f64(key: &str, fallback: f64) -> f64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(fallback)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn bounded(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max)
}

fn normalized_mode(input: &str) -> String {
    match input.trim().to_ascii_lowercase().as_str() {
        "paper" => "paper".to_string(),
        "live" => "live".to_string(),
        _ => "disabled".to_string(),
    }
}

fn default_limits() -> RiskLimits {
    RiskLimits {
        max_order_notional: Some(env_f64("TRADING_MAX_ORDER_NOTIONAL", 5_000.0)),
        max_position_notional: Some(env_f64("TRADING_MAX_POSITION_NOTIONAL", 25_000.0)),
        max_symbol_exposure_pct: Some(env_f64("TRADING_MAX_SYMBOL_EXPOSURE_PCT", 0.20)),
        min_confidence: Some(env_f64("TRADING_MIN_CONFIDENCE", 0.55)),
        max_risk_score: Some(env_f64("TRADING_MAX_RISK_SCORE", 0.72)),
        allow_short: Some(env_bool("TRADING_ALLOW_SHORT", false)),
    }
}

fn config_from_env() -> Config {
    Config {
        trading_mode: normalized_mode(&env_value("TRADING_MODE", "paper")),
        live_orders_enabled: env_bool("TRADING_ALLOW_LIVE_ORDERS", false),
        server_auth_secret: optional_env("SERVER_AUTH_SECRET")
            .or_else(|| optional_env("TRADING_SERVER_AUTH_SECRET")),
        allow_unauthenticated: env_bool("TRADING_ALLOW_UNAUTHENTICATED", false),
        database_url: first_env(&[
            "TRADING_DATABASE_URL",
            "AGENT_TASKS_RDS_DATABASE_URL",
            "RDS_DATABASE_URL",
            "DATABASE_URL",
        ]),
        app_config_scope: env_value("TRADING_APP_CONFIG_SCOPE", "default"),
        app_config_key: env_value("TRADING_APP_CONFIG_KEY", "trading.platforms.v1"),
        // The 30s default is now a belt-and-braces fallback: the primary
        // refresh trigger is the WAL-gateway CDC stream subscription set
        // up in `main()`, which lands sub-second on `app_config` writes.
        // Operators with CDC fully wired can comfortably raise this to
        // 5-15 minutes via TRADING_CONFIG_REFRESH_SECONDS.
        config_refresh: Duration::from_secs(env_u64("TRADING_CONFIG_REFRESH_SECONDS", 30)),
        scraper_base_url: env_value(
            "SCRAPER_BASE_URL",
            "http://dd-web-scraper.default.svc.cluster.local:8097",
        ),
        ml_base_url: env_value(
            "ML_PIPELINE_BASE_URL",
            "http://dd-ai-ml-pipeline.ai-ml.svc.cluster.local:8099",
        ),
        mdp_base_url: env_value(
            "MDP_OPTIMIZER_BASE_URL",
            "http://dd-mdp-optimizer.default.svc.cluster.local:8096",
        ),
        signal_subject: env_value("TRADING_SIGNAL_SUBJECT", "dd.remote.trading.signals"),
        queue_group: env_value("TRADING_QUEUE_GROUP", "dd-trading-server"),
        decision_subject: env_value("TRADING_DECISION_SUBJECT", "dd.remote.trading.decisions"),
        order_intent_subject: env_value(
            "TRADING_ORDER_INTENT_SUBJECT",
            "dd.remote.trading.order_intents",
        ),
        event_subject: env_value("TRADING_EVENT_SUBJECT", "dd.remote.events"),
        default_limits: default_limits(),
    }
}

fn request_id(input: Option<&String>, prefix: &str) -> String {
    input
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(prefix)
        .chars()
        .take(MAX_REQUEST_ID_LEN)
        .collect()
}

fn normalize_symbol(input: &str) -> Result<String, String> {
    let symbol = input.trim().to_ascii_uppercase();
    if symbol.is_empty() {
        return Err("symbol must not be empty".to_string());
    }
    if symbol.len() > MAX_SYMBOL_LEN {
        return Err(format!("symbol must be at most {MAX_SYMBOL_LEN} bytes"));
    }
    if !symbol
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '/' | ':'))
    {
        return Err(
            "symbol may contain only ASCII letters, numbers, '.', '-', '_', '/', ':'".to_string(),
        );
    }
    Ok(symbol)
}

fn validate_label(value: &str, label: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if trimmed.len() > MAX_LABEL_LEN {
        return Err(format!("{label} must be at most {MAX_LABEL_LEN} bytes"));
    }
    Ok(())
}

fn validate_credential_key(value: &str, label: &str) -> Result<(), String> {
    validate_label(value, label)?;
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(format!("{label} must not be empty"));
    };
    if !first.is_ascii_uppercase() {
        return Err(format!("{label} must start with an uppercase ASCII letter"));
    }
    if !std::iter::once(first)
        .chain(chars)
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
    {
        return Err(format!(
            "{label} may contain only uppercase ASCII letters, numbers, and '_'"
        ));
    }
    Ok(())
}

fn validate_local_or_https_url(value: &str, label: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if trimmed.len() > 512 {
        return Err(format!("{label} must be at most 512 bytes"));
    }
    if trimmed.chars().any(|ch| ch.is_control()) {
        return Err(format!("{label} must not contain control characters"));
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("https://")
        || lower.starts_with("http://localhost")
        || lower.starts_with("http://127.0.0.1")
        || lower.starts_with("http://[::1]")
    {
        Ok(())
    } else {
        Err(format!("{label} must be https or a localhost URL"))
    }
}

fn safe_slug(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 120
        && bytes[0].is_ascii_lowercase()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn json_string_field(value: &Value, camel_key: &str, snake_key: &str) -> Option<String> {
    value
        .get(camel_key)
        .or_else(|| value.get(snake_key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_bool_field(value: &Value, camel_key: &str, snake_key: &str, fallback: bool) -> bool {
    value
        .get(camel_key)
        .or_else(|| value.get(snake_key))
        .and_then(Value::as_bool)
        .unwrap_or(fallback)
}

fn json_string_vec(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty() && item.len() <= MAX_LABEL_LEN)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn json_string_map(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| {
                    let value = value.as_str()?.trim();
                    if key.len() <= MAX_LABEL_LEN && !value.is_empty() && value.len() <= 512 {
                        Some((key.to_string(), value.to_string()))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn platform_from_json(value: &Value) -> Result<TradingPlatform, String> {
    let slug = json_string_field(value, "slug", "slug")
        .ok_or_else(|| "trading platform config is missing slug".to_string())?;
    if !safe_slug(&slug) {
        return Err(format!("invalid trading platform slug: {slug}"));
    }
    let display_name =
        json_string_field(value, "displayName", "display_name").unwrap_or_else(|| slug.clone());
    let provider = json_string_field(value, "provider", "provider").unwrap_or_else(|| slug.clone());
    validate_label(&display_name, "platform.displayName")?;
    validate_label(&provider, "platform.provider")?;
    let status =
        json_string_field(value, "status", "status").unwrap_or_else(|| "active".to_string());
    if !matches!(status.as_str(), "active" | "paused" | "archived") {
        return Err(format!(
            "trading platform {slug} has invalid status {status}"
        ));
    }
    let credential_secret = json_string_field(value, "credentialSecret", "credential_secret")
        .unwrap_or_else(|| "dd-trading-broker-secrets".to_string());
    validate_label(&credential_secret, "platform.credentialSecret")?;
    let credential_keys = json_string_vec(
        value
            .get("credentialKeys")
            .or_else(|| value.get("credential_keys")),
    );
    if credential_keys.is_empty() {
        return Err(format!(
            "trading platform {slug} must list at least one credential key"
        ));
    }
    for key in &credential_keys {
        validate_credential_key(key, &format!("platform.{slug}.credentialKeys"))?;
    }
    let account_ref_key = json_string_field(value, "accountRefKey", "account_ref_key");
    if let Some(key) = account_ref_key.as_ref() {
        validate_credential_key(key, &format!("platform.{slug}.accountRefKey"))?;
    }
    let base_urls = json_string_map(value.get("baseUrls").or_else(|| value.get("base_urls")));
    for (mode, url) in &base_urls {
        if !safe_slug(mode) {
            return Err(format!(
                "trading platform {slug} baseUrls key must be a safe slug: {mode}"
            ));
        }
        validate_local_or_https_url(url, &format!("platform.{slug}.baseUrls.{mode}"))?;
    }
    Ok(TradingPlatform {
        slug,
        display_name,
        provider,
        status,
        supports_paper: json_bool_field(value, "supportsPaper", "supports_paper", true),
        supports_live: json_bool_field(value, "supportsLive", "supports_live", false),
        asset_classes: json_string_vec(
            value
                .get("assetClasses")
                .or_else(|| value.get("asset_classes")),
        ),
        order_types: json_string_vec(value.get("orderTypes").or_else(|| value.get("order_types"))),
        base_urls,
        credential_secret,
        credential_keys,
        account_ref_key,
        labels: json_string_vec(value.get("labels")),
        meta_data: value
            .get("metaData")
            .or_else(|| value.get("meta_data"))
            .cloned()
            .unwrap_or_else(|| json!({})),
    })
}

fn platform_config_from_app_config_value(value: Value) -> Result<TradingPlatformConfig, String> {
    let platforms = value
        .get("platforms")
        .and_then(Value::as_array)
        .ok_or_else(|| "trading app_config value must contain a platforms array".to_string())?;
    let platforms = platforms
        .iter()
        .map(platform_from_json)
        .collect::<Result<Vec<_>, _>>()?;
    if platforms.is_empty() {
        return Err("trading app_config platforms array must not be empty".to_string());
    }
    let mut seen_slugs = BTreeSet::new();
    for platform in &platforms {
        if !seen_slugs.insert(platform.slug.as_str()) {
            return Err(format!(
                "trading app_config contains duplicate platform slug {}",
                platform.slug
            ));
        }
    }
    let default_platform = json_string_field(&value, "defaultPlatform", "default_platform");
    if let Some(default_platform) = default_platform.as_ref() {
        if !seen_slugs.contains(default_platform.as_str()) {
            return Err(format!(
                "trading app_config defaultPlatform {default_platform} is not defined"
            ));
        }
    }
    Ok(TradingPlatformConfig {
        platforms,
        default_platform,
        last_config_refresh_ms: Some(now_ms()),
        last_config_error: None,
    })
}

fn default_platform_config() -> TradingPlatformConfig {
    let value = json!({
        "defaultPlatform": "interactive-brokers",
        "platforms": [
            {
                "slug": "interactive-brokers",
                "displayName": "Interactive Brokers",
                "provider": "interactive-brokers",
                "status": "active",
                "supportsPaper": true,
                "supportsLive": true,
                "assetClasses": ["equities", "options", "futures", "forex", "bonds", "funds"],
                "orderTypes": ["market", "limit", "stop", "stop_limit"],
                "baseUrls": { "paper": "https://localhost:5000/v1/api", "live": "https://localhost:5000/v1/api" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["IBKR_GATEWAY_URL", "IBKR_ACCOUNT_ID"],
                "accountRefKey": "IBKR_ACCOUNT_ID",
                "labels": ["brokerage", "multi-asset"]
            },
            {
                "slug": "alpaca",
                "displayName": "Alpaca",
                "provider": "alpaca",
                "status": "active",
                "supportsPaper": true,
                "supportsLive": true,
                "assetClasses": ["equities", "options", "crypto"],
                "orderTypes": ["market", "limit", "stop", "stop_limit"],
                "baseUrls": { "paper": "https://paper-api.alpaca.markets", "live": "https://api.alpaca.markets" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["ALPACA_API_KEY_ID", "ALPACA_API_SECRET_KEY"],
                "labels": ["brokerage", "paper-first"]
            },
            {
                "slug": "tradier",
                "displayName": "Tradier",
                "provider": "tradier",
                "status": "active",
                "supportsPaper": true,
                "supportsLive": true,
                "assetClasses": ["equities", "options"],
                "orderTypes": ["market", "limit", "stop", "stop_limit"],
                "baseUrls": { "paper": "https://sandbox.tradier.com/v1", "live": "https://api.tradier.com/v1" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["TRADIER_ACCESS_TOKEN", "TRADIER_ACCOUNT_ID"],
                "accountRefKey": "TRADIER_ACCOUNT_ID",
                "labels": ["brokerage", "options"]
            },
            {
                "slug": "coinbase-advanced-trade",
                "displayName": "Coinbase Advanced Trade",
                "provider": "coinbase",
                "status": "active",
                "supportsPaper": false,
                "supportsLive": true,
                "assetClasses": ["crypto"],
                "orderTypes": ["market", "limit", "stop_limit"],
                "baseUrls": { "live": "https://api.coinbase.com/api/v3/brokerage" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["COINBASE_API_KEY", "COINBASE_API_SECRET"],
                "labels": ["crypto"]
            },
            {
                "slug": "kraken",
                "displayName": "Kraken",
                "provider": "kraken",
                "status": "active",
                "supportsPaper": false,
                "supportsLive": true,
                "assetClasses": ["crypto"],
                "orderTypes": ["market", "limit", "stop_loss", "take_profit"],
                "baseUrls": { "live": "https://api.kraken.com" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["KRAKEN_API_KEY", "KRAKEN_API_SECRET"],
                "labels": ["crypto"]
            },
            {
                "slug": "gemini",
                "displayName": "Gemini",
                "provider": "gemini",
                "status": "active",
                "supportsPaper": true,
                "supportsLive": true,
                "assetClasses": ["crypto"],
                "orderTypes": ["market", "limit"],
                "baseUrls": { "paper": "https://api.sandbox.gemini.com", "live": "https://api.gemini.com" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["GEMINI_API_KEY", "GEMINI_API_SECRET"],
                "labels": ["crypto", "paper-first"]
            },
            {
                "slug": "binance-us",
                "displayName": "Binance.US",
                "provider": "binance-us",
                "status": "active",
                "supportsPaper": false,
                "supportsLive": true,
                "assetClasses": ["crypto"],
                "orderTypes": ["market", "limit", "stop_limit"],
                "baseUrls": { "live": "https://api.binance.us" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["BINANCE_US_API_KEY", "BINANCE_US_API_SECRET"],
                "labels": ["crypto"]
            },
            {
                "slug": "polymarket",
                "displayName": "Polymarket",
                "provider": "polymarket",
                "status": "paused",
                "supportsPaper": false,
                "supportsLive": true,
                "assetClasses": ["prediction-markets", "crypto"],
                "orderTypes": ["market", "limit"],
                "baseUrls": { "live": "https://clob.polymarket.com" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["POLYMARKET_PRIVATE_KEY", "POLYMARKET_FUNDER_ADDRESS"],
                "labels": ["prediction-market", "crypto"]
            },
            {
                "slug": "factmachine",
                "displayName": "FactMachine",
                "provider": "factmachine",
                "status": "paused",
                "supportsPaper": false,
                "supportsLive": false,
                "assetClasses": ["prediction-markets", "data"],
                "orderTypes": [],
                "baseUrls": {},
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["FACTMACHINE_API_KEY", "FACTMACHINE_BASE_URL"],
                "labels": ["prediction-market", "research", "placeholder"],
                "metaData": { "endpointStatus": "not-configured" }
            }
        ]
    });
    platform_config_from_app_config_value(value).unwrap_or_else(|error| TradingPlatformConfig {
        platforms: Vec::new(),
        default_platform: None,
        last_config_refresh_ms: None,
        last_config_error: Some(error),
    })
}

fn platform_snapshot(state: &AppState) -> TradingPlatformConfig {
    state
        .platform_config
        .read()
        .map(|config| config.clone())
        .unwrap_or_else(|_| TradingPlatformConfig {
            platforms: Vec::new(),
            default_platform: None,
            last_config_refresh_ms: None,
            last_config_error: Some("trading platform config lock is poisoned".to_string()),
        })
}

fn row_value(row: &tokio_postgres::Row, column: &str, fallback: Value) -> Value {
    row.try_get::<_, Value>(column).unwrap_or(fallback)
}

async fn connect_postgres(config: &Config) -> Result<tokio_postgres::Client, String> {
    let database_url = config
        .database_url
        .as_deref()
        .ok_or_else(|| "trading database URL is not configured".to_string())?;
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
    let (client, connection) = tokio_postgres::connect(database_url, tls)
        .await
        .map_err(|error| error.to_string())?;

    tokio::spawn(async move {
        if let Err(error) = connection.await {
            eprintln!("trading server postgres connection error: {error}");
        }
    });
    Ok(client)
}

async fn fetch_platform_config_from_app_config(
    client: &tokio_postgres::Client,
    config: &Config,
) -> Result<Option<TradingPlatformConfig>, String> {
    let rows = client
        .query(
            r#"
            select value
            from app_config
            where scope = $1
              and key = $2
              and status = 'active'
              and is_soft_deleted = false
            order by updated_at desc
            limit 1
            "#,
            &[&config.app_config_scope, &config.app_config_key],
        )
        .await
        .map_err(|error| error.to_string())?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    platform_config_from_app_config_value(row_value(row, "value", json!({}))).map(Some)
}

async fn fetch_platform_config(config: &Config) -> Result<TradingPlatformConfig, String> {
    let Some(_) = config.database_url.as_ref() else {
        return Ok(default_platform_config());
    };
    let client = connect_postgres(config).await?;
    fetch_platform_config_from_app_config(&client, config)
        .await?
        .ok_or_else(|| {
            format!(
                "missing active app_config row scope={} key={}",
                config.app_config_scope, config.app_config_key
            )
        })
}

fn store_platform_config(state: &AppState, next: TradingPlatformConfig) -> Result<(), String> {
    let mut current = state
        .platform_config
        .write()
        .map_err(|_| "trading platform config lock is poisoned".to_string())?;
    *current = next;
    Ok(())
}

async fn refresh_platform_config(state: &AppState) -> Result<(), String> {
    let next = fetch_platform_config(&state.config).await?;
    store_platform_config(state, next)?;
    state
        .metrics
        .config_refresh_total
        .fetch_add(1, Ordering::Relaxed);
    Ok(())
}

async fn record_config_error(state: &AppState, error: String) {
    state
        .metrics
        .config_refresh_failures_total
        .fetch_add(1, Ordering::Relaxed);
    if let Ok(mut config) = state.platform_config.write() {
        config.last_config_error = Some(error);
    }
}

async fn run_config_refresh_loop(state: AppState) {
    loop {
        sleep(state.config.config_refresh).await;
        if let Err(error) = refresh_platform_config(&state).await {
            eprintln!("trading platform config refresh failed: {error}");
            record_config_error(&state, error).await;
        }
    }
}

/// Subscribe to the WAL gateway's CDC stream and refresh the trading
/// platform config the instant `app_config` changes are committed.
///
/// We deliberately keep the wider poll-based refresh loop alive: CDC can
/// drop messages if JetStream is down or the consumer is far enough
/// behind that the broker has aged messages out of the stream. The poll
/// loop is the catch-up path.
///
/// The handler filters down to the specific scope+key tuple this server
/// cares about (`trading.platforms.v1` by default) so unrelated
/// `app_config` rows don't trigger refreshes — saves a Postgres query
/// per noisy write.
async fn run_cdc_refresh_subscription(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        println!("trading server cdc subscription disabled: no NATS_URL configured");
        return;
    };
    let jetstream = async_nats::jetstream::new(nats);
    let scope = state.config.app_config_scope.clone();
    let key = state.config.app_config_key.clone();
    let label = format!(
        "dd-trading-server-app-config-{}",
        sanitize_for_durable_name(&format!("{scope}.{key}"))
    );
    let trigger_state = state.clone();
    let builder = dd_wal_consumer::Subscription::builder()
        .stream(env_value("TRADING_CDC_STREAM", "CDC"))
        .durable_name(label.clone())
        .filter_subject("cdc.public.app_config.>");
    let start = builder
        .start(&jetstream, move |change: dd_wal_consumer::RowChange| {
            let scope = scope.clone();
            let key = key.clone();
            let task_state = trigger_state.clone();
            async move {
                // The gateway sends every row in `app_config`. Skip rows
                // for other scopes/keys entirely; this is what makes the
                // CDC path cheap even when the table is busy with other
                // services' configs.
                let row_scope = change
                    .column("scope")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let row_key = change
                    .column("key")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if row_scope.as_deref() != Some(&scope) || row_key.as_deref() != Some(&key) {
                    return;
                }
                if let Err(error) = refresh_platform_config(&task_state).await {
                    eprintln!(
                        "trading platform CDC-driven refresh failed (scope={scope} key={key}): \
                         {error}"
                    );
                    record_config_error(&task_state, error).await;
                }
            }
        })
        .await;
    match start {
        Ok(_join) => {
            println!(
                "trading server cdc subscription started: durable={label} \
                 subject=cdc.public.app_config.>"
            );
        }
        Err(error) => {
            eprintln!(
                "trading server cdc subscription failed to start ({error}); \
                 falling back to poll-only refresh"
            );
        }
    }
}

fn sanitize_for_durable_name(input: &str) -> String {
    input
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn finite_optional(value: Option<f64>, label: &str) -> Result<Option<f64>, String> {
    match value {
        Some(value) if value.is_finite() => Ok(Some(value)),
        Some(_) => Err(format!("{label} must be finite")),
        None => Ok(None),
    }
}

fn finite_positive_optional(value: Option<f64>, label: &str) -> Result<Option<f64>, String> {
    match finite_optional(value, label)? {
        Some(value) if value > 0.0 => Ok(Some(value)),
        Some(_) => Err(format!("{label} must be positive")),
        None => Ok(None),
    }
}

fn finite_nonnegative_optional(value: Option<f64>, label: &str) -> Result<Option<f64>, String> {
    match finite_optional(value, label)? {
        Some(value) if value >= 0.0 => Ok(Some(value)),
        Some(_) => Err(format!("{label} must be non-negative")),
        None => Ok(None),
    }
}

fn finite_range_optional(
    value: Option<f64>,
    label: &str,
    min: f64,
    max: f64,
) -> Result<Option<f64>, String> {
    match finite_optional(value, label)? {
        Some(value) if value >= min && value <= max => Ok(Some(value)),
        Some(_) => Err(format!("{label} must be between {min:.2} and {max:.2}")),
        None => Ok(None),
    }
}

fn conservative_cap(default: Option<f64>, override_value: Option<f64>) -> Option<f64> {
    match (default, override_value) {
        (Some(default), Some(override_value)) => Some(default.min(override_value)),
        (Some(default), None) => Some(default),
        (None, Some(override_value)) => Some(override_value),
        (None, None) => None,
    }
}

fn conservative_floor(default: Option<f64>, override_value: Option<f64>) -> Option<f64> {
    match (default, override_value) {
        (Some(default), Some(override_value)) => Some(default.max(override_value)),
        (Some(default), None) => Some(default),
        (None, Some(override_value)) => Some(override_value),
        (None, None) => None,
    }
}

fn merge_limits(defaults: &RiskLimits, overrides: Option<RiskLimits>) -> RiskLimits {
    let Some(overrides) = overrides else {
        return defaults.clone();
    };
    RiskLimits {
        max_order_notional: conservative_cap(
            defaults.max_order_notional,
            overrides.max_order_notional,
        ),
        max_position_notional: conservative_cap(
            defaults.max_position_notional,
            overrides.max_position_notional,
        ),
        max_symbol_exposure_pct: conservative_cap(
            defaults.max_symbol_exposure_pct,
            overrides.max_symbol_exposure_pct,
        ),
        min_confidence: conservative_floor(defaults.min_confidence, overrides.min_confidence),
        max_risk_score: conservative_cap(defaults.max_risk_score, overrides.max_risk_score),
        allow_short: match (defaults.allow_short, overrides.allow_short) {
            (Some(default), Some(override_value)) => Some(default && override_value),
            (Some(default), None) => Some(default),
            (None, Some(override_value)) => Some(override_value),
            (None, None) => None,
        },
    }
}

fn validate_request(request: &DecisionRequest, limits: &RiskLimits) -> Result<Vec<String>, String> {
    if let Some(schema_version) = request.schema_version.as_ref() {
        if schema_version != SCHEMA_VERSION {
            return Err(format!(
                "schemaVersion must be {SCHEMA_VERSION}, got {schema_version}"
            ));
        }
    }

    if let Some(strategy) = request.strategy.as_ref() {
        validate_label(strategy, "strategy")?;
    }
    if let Some(horizon) = request.horizon.as_ref() {
        validate_label(horizon, "horizon")?;
    }
    if let Some(venue) = request.venue.as_ref() {
        validate_label(venue, "venue")?;
    }
    if let Some(target_platform) = request.target_platform.as_ref() {
        if !safe_slug(target_platform) {
            return Err(format!(
                "targetPlatform must be a safe platform slug: {target_platform}"
            ));
        }
    }

    if let Some(portfolio) = request.portfolio.as_ref() {
        finite_positive_optional(portfolio.cash, "portfolio.cash")?;
        finite_positive_optional(portfolio.equity, "portfolio.equity")?;
        finite_optional(portfolio.current_position, "portfolio.currentPosition")?;
        finite_positive_optional(portfolio.average_entry_price, "portfolio.averageEntryPrice")?;
    }

    if let Some(market) = request.market.as_ref() {
        finite_positive_optional(market.last_price, "market.lastPrice")?;
        finite_positive_optional(market.bid, "market.bid")?;
        finite_positive_optional(market.ask, "market.ask")?;
        if let (Some(bid), Some(ask)) = (market.bid, market.ask) {
            if bid > ask {
                return Err("market.bid must be less than or equal to market.ask".to_string());
            }
        }
        finite_positive_optional(market.day_volume, "market.dayVolume")?;
        finite_nonnegative_optional(market.realized_volatility, "market.realizedVolatility")?;
        if let Some(prices) = market.prices.as_ref() {
            if prices.len() > MAX_PRICE_POINTS {
                return Err(format!(
                    "market.prices can include at most {MAX_PRICE_POINTS} points"
                ));
            }
            for value in prices {
                if !value.is_finite() || *value <= 0.0 {
                    return Err("market.prices entries must be finite positive numbers".to_string());
                }
            }
        }
    }

    if let Some(signals) = request.web_signals.as_ref() {
        if signals.len() > MAX_WEB_SIGNALS {
            return Err(format!(
                "webSignals can include at most {MAX_WEB_SIGNALS} entries"
            ));
        }
        for signal in signals {
            if !signal.sentiment.is_finite() || signal.sentiment < -1.0 || signal.sentiment > 1.0 {
                return Err("webSignals sentiment must be between -1.00 and 1.00".to_string());
            }
            finite_range_optional(signal.confidence, "webSignals.confidence", 0.0, 1.0)?;
            finite_range_optional(signal.relevance, "webSignals.relevance", 0.0, 1.0)?;
        }
    }

    if let Some(features) = request.ml_features.as_ref() {
        if features.len() > MAX_FEATURES {
            return Err(format!(
                "mlFeatures can include at most {MAX_FEATURES} entries"
            ));
        }
        for feature in features {
            validate_label(&feature.name, "mlFeatures.name")?;
            if !feature.value.is_finite() || feature.value < -1.0 || feature.value > 1.0 {
                return Err("mlFeatures.value must be between -1.00 and 1.00".to_string());
            }
            finite_range_optional(feature.weight, "mlFeatures.weight", 0.0, 10.0)?;
        }
    }

    if let Some(policy) = request.mdp_policy.as_ref() {
        validate_label(&policy.action, "mdpPolicy.action")?;
        finite_range_optional(policy.confidence, "mdpPolicy.confidence", 0.0, 1.0)?;
        finite_optional(policy.value, "mdpPolicy.value")?;
        finite_range_optional(policy.risk, "mdpPolicy.risk", 0.0, 1.0)?;
    }

    finite_positive_optional(limits.max_order_notional, "constraints.maxOrderNotional")?;
    finite_positive_optional(
        limits.max_position_notional,
        "constraints.maxPositionNotional",
    )?;
    finite_range_optional(
        limits.max_symbol_exposure_pct,
        "constraints.maxSymbolExposurePct",
        0.0,
        1.0,
    )?;
    finite_range_optional(limits.min_confidence, "constraints.minConfidence", 0.0, 1.0)?;
    finite_range_optional(limits.max_risk_score, "constraints.maxRiskScore", 0.0, 1.0)?;

    let mut warnings = Vec::new();
    if request.market.is_none() {
        warnings.push("market snapshot missing; price safety gate will block orders".to_string());
    }
    if request
        .web_signals
        .as_ref()
        .map(Vec::is_empty)
        .unwrap_or(true)
        && request
            .ml_features
            .as_ref()
            .map(Vec::is_empty)
            .unwrap_or(true)
        && request.mdp_policy.is_none()
    {
        warnings.push(
            "no web, ML, or MDP signals supplied; decision will bias toward hold".to_string(),
        );
    }

    Ok(warnings)
}

fn score_web_signals(signals: Option<&[WebSignal]>) -> Option<(f64, String)> {
    let signals = signals?;
    if signals.is_empty() {
        return None;
    }
    let mut weighted = 0.0;
    let mut weight_sum = 0.0;
    for signal in signals {
        let confidence = bounded(signal.confidence.unwrap_or(0.5), 0.0, 1.0);
        let relevance = bounded(signal.relevance.unwrap_or(0.5), 0.0, 1.0);
        let freshness = match signal.age_ms {
            Some(age) if age > MAX_SIGNAL_AGE_MS => 0.25,
            Some(age) => 1.0 - (age as f64 / MAX_SIGNAL_AGE_MS as f64 * 0.35),
            None => 0.75,
        };
        let weight = confidence * relevance * bounded(freshness, 0.25, 1.0);
        weighted += bounded(signal.sentiment, -1.0, 1.0) * weight;
        weight_sum += weight;
    }
    if weight_sum <= 0.0 {
        return None;
    }
    Some((
        bounded(weighted / weight_sum, -1.0, 1.0),
        format!(
            "{} web signals weighted by confidence, relevance, and freshness",
            signals.len()
        ),
    ))
}

fn score_ml_features(features: Option<&[ModelFeature]>) -> Option<(f64, String)> {
    let features = features?;
    if features.is_empty() {
        return None;
    }
    let mut weighted = 0.0;
    let mut weight_sum = 0.0;
    for feature in features {
        let direction = if feature.higher_is_better.unwrap_or(true) {
            1.0
        } else {
            -1.0
        };
        let weight = bounded(feature.weight.unwrap_or(1.0), 0.0, 10.0);
        weighted += bounded(feature.value, -1.0, 1.0) * direction * weight;
        weight_sum += weight;
    }
    if weight_sum <= 0.0 {
        return None;
    }
    Some((
        bounded(weighted / weight_sum, -1.0, 1.0),
        format!(
            "{} AI/ML features normalized into a directional score",
            features.len()
        ),
    ))
}

fn score_market_momentum(market: Option<&MarketSnapshot>) -> Option<(f64, String)> {
    let market = market?;
    let prices = market.prices.as_ref()?;
    if prices.len() < 2 {
        return None;
    }
    let first = *prices.first()?;
    let last = market
        .last_price
        .unwrap_or_else(|| *prices.last().unwrap_or(&first));
    if first <= 0.0 || last <= 0.0 {
        return None;
    }
    let change = (last - first) / first;
    Some((
        bounded(change * 5.0, -1.0, 1.0),
        format!("recent price path changed by {:.2}%", change * 100.0),
    ))
}

fn score_mdp_policy(policy: Option<&MdpPolicyHint>) -> Option<(f64, f64, f64, String)> {
    let policy = policy?;
    let action_score = match policy.action.trim().to_ascii_lowercase().as_str() {
        "buy" | "long" | "increase" | "risk-on" => 1.0,
        "sell" | "short" | "reduce" | "risk-off" => -1.0,
        _ => 0.0,
    };
    let confidence = bounded(policy.confidence.unwrap_or(0.5), 0.0, 1.0);
    let risk = bounded(policy.risk.unwrap_or(1.0 - confidence), 0.0, 1.0);
    Some((
        action_score * confidence,
        confidence,
        risk,
        format!(
            "MDP/POMDP policy hint action={} confidence={confidence:.2}",
            policy.action
        ),
    ))
}

fn effective_price(market: Option<&MarketSnapshot>, action: &str) -> Option<f64> {
    let market = market?;
    match action {
        "buy" => market.ask.or(market.last_price).or(market.bid),
        "sell" => market.bid.or(market.last_price).or(market.ask),
        _ => market.last_price.or(market.bid).or(market.ask),
    }
}

fn mode_supported(platform: &TradingPlatform, mode: &str) -> bool {
    match mode {
        "paper" => platform.supports_paper,
        "live" => platform.supports_live,
        _ => false,
    }
}

fn platform_available_for_action(platform: &TradingPlatform, mode: &str) -> bool {
    platform.status == "active" && mode_supported(platform, mode)
}

fn select_platform(
    platforms: &[TradingPlatform],
    default_platform: Option<&str>,
    requested: Option<&str>,
    mode: &str,
) -> Option<TradingPlatform> {
    if let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) {
        return platforms
            .iter()
            .find(|platform| platform.slug == requested)
            .cloned();
    }
    if let Some(default_platform) = default_platform {
        if let Some(platform) = platforms.iter().find(|platform| {
            platform.slug == default_platform && platform_available_for_action(platform, mode)
        }) {
            return Some(platform.clone());
        }
    }
    platforms
        .iter()
        .find(|platform| platform_available_for_action(platform, mode))
        .cloned()
        .or_else(|| {
            platforms
                .iter()
                .find(|platform| platform.status == "active")
                .cloned()
        })
}

fn normalize_action(raw_score: f64) -> String {
    if raw_score >= 0.20 {
        "buy".to_string()
    } else if raw_score <= -0.20 {
        "sell".to_string()
    } else {
        "hold".to_string()
    }
}

fn safety_check(name: &str, ok: bool, severity: &str, message: String) -> SafetyCheck {
    SafetyCheck {
        name: name.to_string(),
        ok,
        severity: severity.to_string(),
        message,
    }
}

fn build_candidate_order(context: CandidateOrderContext<'_>) -> Option<OrderIntent> {
    let CandidateOrderContext {
        request_id,
        symbol,
        platform,
        action,
        price,
        limits,
        request,
        config,
        confidence,
    } = context;
    if action == "hold" {
        return None;
    }
    let platform = platform?;
    let reference_price = price?;
    if reference_price <= 0.0 {
        return None;
    }
    let max_notional = limits.max_order_notional.unwrap_or(5_000.0);
    let cash_cap = request
        .portfolio
        .as_ref()
        .and_then(|portfolio| portfolio.cash)
        .map(|cash| cash.max(0.0))
        .unwrap_or(max_notional);
    let buy_cap = if action == "buy" {
        max_notional.min(cash_cap)
    } else {
        max_notional
    };
    let notional = (buy_cap * bounded(confidence, 0.10, 1.0)).max(0.0);
    if notional <= 0.0 {
        return None;
    }
    let quantity = notional / reference_price;
    Some(OrderIntent {
        request_id: request_id.to_string(),
        symbol: symbol.to_string(),
        platform: platform.slug.clone(),
        platform_display_name: platform.display_name.clone(),
        credential_secret: platform.credential_secret.clone(),
        credential_keys: platform.credential_keys.clone(),
        side: action.to_string(),
        order_type: "limit".to_string(),
        quantity,
        notional,
        reference_price,
        mode: config.trading_mode.clone(),
        dry_run: request.dry_run.unwrap_or(config.trading_mode != "live"),
        intent_only: true,
        subject: config.order_intent_subject.clone(),
        generated_at_ms: now_ms(),
    })
}

fn exposure_check(
    request: &DecisionRequest,
    action: &str,
    order: Option<&OrderIntent>,
    limits: &RiskLimits,
) -> (bool, String) {
    let Some(order) = order else {
        return (true, "no order intent, exposure unchanged".to_string());
    };
    let Some(portfolio) = request.portfolio.as_ref() else {
        return (
            true,
            "portfolio snapshot missing; using order notional gate only".to_string(),
        );
    };
    let position = portfolio.current_position.unwrap_or(0.0);
    let current_price = order.reference_price;
    let next_position = if action == "buy" {
        position + order.quantity
    } else {
        position - order.quantity
    };
    let next_notional = (next_position * current_price).abs();
    if let Some(max_position_notional) = limits.max_position_notional {
        if next_notional > max_position_notional {
            return (
                false,
                format!(
                    "projected symbol notional {next_notional:.2} exceeds maxPositionNotional {max_position_notional:.2}"
                ),
            );
        }
    }
    if let (Some(equity), Some(max_exposure)) = (portfolio.equity, limits.max_symbol_exposure_pct) {
        if equity > 0.0 && next_notional / equity > max_exposure {
            return (
                false,
                format!(
                    "projected symbol exposure {:.2}% exceeds maxSymbolExposurePct {:.2}%",
                    (next_notional / equity) * 100.0,
                    max_exposure * 100.0
                ),
            );
        }
    }
    (
        true,
        "projected exposure is inside configured limits".to_string(),
    )
}

fn evaluate_decision(
    config: &Config,
    platform_config: &TradingPlatformConfig,
    request: DecisionRequest,
) -> Result<DecisionResponse, String> {
    let symbol = normalize_symbol(&request.symbol)?;
    let request_id = request_id(
        request.request_id.as_ref(),
        &format!("trading-{}", now_ms()),
    );
    let limits = merge_limits(&config.default_limits, request.constraints.clone());
    let mut warnings = validate_request(&request, &limits)?;

    let mut components = Vec::new();
    let mut weighted_score = 0.0;
    let mut component_weight = 0.0;
    let mut mdp_confidence = 0.0;
    let mut mdp_risk = 0.0;

    if let Some((score, reason)) = score_web_signals(request.web_signals.as_deref()) {
        let weight = 0.34;
        weighted_score += score * weight;
        component_weight += weight;
        components.push(ScoreComponent {
            name: "webSignals".to_string(),
            score,
            weight,
            reason,
        });
    }

    if let Some((score, reason)) = score_ml_features(request.ml_features.as_deref()) {
        let weight = 0.26;
        weighted_score += score * weight;
        component_weight += weight;
        components.push(ScoreComponent {
            name: "mlFeatures".to_string(),
            score,
            weight,
            reason,
        });
    }

    if let Some((score, reason)) = score_market_momentum(request.market.as_ref()) {
        let weight = 0.18;
        weighted_score += score * weight;
        component_weight += weight;
        components.push(ScoreComponent {
            name: "marketMomentum".to_string(),
            score,
            weight,
            reason,
        });
    }

    if let Some((score, confidence, risk, reason)) = score_mdp_policy(request.mdp_policy.as_ref()) {
        let weight = 0.22;
        weighted_score += score * weight;
        component_weight += weight;
        mdp_confidence = confidence;
        mdp_risk = risk;
        components.push(ScoreComponent {
            name: "mdpPolicy".to_string(),
            score,
            weight,
            reason,
        });
    }

    let raw_score = if component_weight > 0.0 {
        bounded(weighted_score / component_weight, -1.0, 1.0)
    } else {
        0.0
    };
    let coverage = bounded(component_weight, 0.0, 1.0);
    let confidence = bounded(
        raw_score.abs() * 0.45 + coverage * 0.35 + mdp_confidence * 0.20,
        0.0,
        1.0,
    );
    let market_risk = request
        .market
        .as_ref()
        .and_then(|market| market.realized_volatility)
        .map(|volatility| bounded(volatility, 0.0, 1.0))
        .unwrap_or(0.35);
    let risk_score = bounded(
        (market_risk * 0.45) + (mdp_risk * 0.35) + ((1.0 - confidence) * 0.20),
        0.0,
        1.0,
    );
    let recommended_action = normalize_action(raw_score);
    let price = effective_price(request.market.as_ref(), &recommended_action);
    let selected_platform = select_platform(
        &platform_config.platforms,
        platform_config.default_platform.as_deref(),
        request.target_platform.as_deref(),
        &config.trading_mode,
    );
    let candidate_order = build_candidate_order(CandidateOrderContext {
        request_id: &request_id,
        symbol: &symbol,
        platform: selected_platform.as_ref(),
        action: &recommended_action,
        price,
        limits: &limits,
        request: &request,
        config,
        confidence,
    });

    let min_confidence = limits.min_confidence.unwrap_or(0.55);
    let max_risk = limits.max_risk_score.unwrap_or(0.72);
    let allow_short = limits.allow_short.unwrap_or(false);

    let mut safety_checks = Vec::new();
    safety_checks.push(safety_check(
        "platformConfigured",
        recommended_action == "hold" || selected_platform.is_some(),
        "blocker",
        request
            .target_platform
            .as_ref()
            .map(|platform| format!("requested trading platform {platform}"))
            .unwrap_or_else(|| "at least one active trading platform is configured".to_string()),
    ));
    safety_checks.push(safety_check(
        "platformModeSupported",
        recommended_action == "hold"
            || selected_platform
                .as_ref()
                .map(|platform| platform_available_for_action(platform, &config.trading_mode))
                .unwrap_or(false),
        "blocker",
        selected_platform
            .as_ref()
            .map(|platform| {
                format!(
                    "{} supports paper={} live={} status={}",
                    platform.slug, platform.supports_paper, platform.supports_live, platform.status
                )
            })
            .unwrap_or_else(|| "no selected platform can support this mode".to_string()),
    ));
    safety_checks.push(safety_check(
        "modeAllowsIntent",
        config.trading_mode != "disabled" || recommended_action == "hold",
        "blocker",
        format!("TRADING_MODE is {}", config.trading_mode),
    ));
    safety_checks.push(safety_check(
        "liveOrderGate",
        config.trading_mode != "live" || config.live_orders_enabled,
        "blocker",
        "live mode requires TRADING_ALLOW_LIVE_ORDERS=true".to_string(),
    ));
    safety_checks.push(safety_check(
        "confidenceFloor",
        recommended_action == "hold" || confidence >= min_confidence,
        "blocker",
        format!("confidence {confidence:.2} vs minConfidence {min_confidence:.2}"),
    ));
    safety_checks.push(safety_check(
        "riskCeiling",
        recommended_action == "hold" || risk_score <= max_risk,
        "blocker",
        format!("riskScore {risk_score:.2} vs maxRiskScore {max_risk:.2}"),
    ));
    safety_checks.push(safety_check(
        "referencePrice",
        recommended_action == "hold" || price.is_some(),
        "blocker",
        "buy/sell decisions require bid, ask, or lastPrice".to_string(),
    ));
    let current_position = request
        .portfolio
        .as_ref()
        .and_then(|portfolio| portfolio.current_position)
        .unwrap_or(0.0);
    let sell_quantity = candidate_order
        .as_ref()
        .map(|order| order.quantity)
        .unwrap_or(0.0);
    safety_checks.push(safety_check(
        "shortingPolicy",
        recommended_action != "sell" || allow_short || current_position >= sell_quantity,
        "blocker",
        "sell intent requires enough existing long position or allowShort=true".to_string(),
    ));
    if let Some(order) = candidate_order.as_ref() {
        let max_notional = limits.max_order_notional.unwrap_or(5_000.0);
        safety_checks.push(safety_check(
            "orderNotional",
            order.notional <= max_notional,
            "blocker",
            format!(
                "order notional {:.2} vs maxOrderNotional {:.2}",
                order.notional, max_notional
            ),
        ));
    }
    let (exposure_ok, exposure_message) = exposure_check(
        &request,
        &recommended_action,
        candidate_order.as_ref(),
        &limits,
    );
    safety_checks.push(safety_check(
        "symbolExposure",
        exposure_ok,
        "blocker",
        exposure_message,
    ));

    let blocked = safety_checks
        .iter()
        .any(|check| !check.ok && check.severity == "blocker");
    let final_action = if blocked {
        if recommended_action != "hold" {
            warnings.push(format!(
                "recommended {recommended_action} was converted to hold by safety gates"
            ));
        }
        "hold".to_string()
    } else {
        recommended_action.clone()
    };
    let order_intent = if final_action == recommended_action {
        candidate_order
    } else {
        None
    };
    let execution_status = match (
        recommended_action.as_str(),
        final_action.as_str(),
        config.trading_mode.as_str(),
    ) {
        ("hold", _, _) => "no_order",
        (_, "hold", _) => "blocked_by_safety_gate",
        (_, _, "paper") => "paper_intent_ready",
        (_, _, "live") => "live_intent_ready",
        _ => "disabled",
    }
    .to_string();

    Ok(DecisionResponse {
        ok: true,
        request_id,
        schema_version: SCHEMA_VERSION,
        symbol,
        venue: request.venue,
        strategy: request
            .strategy
            .unwrap_or_else(|| "www-mdp-risk-gated".to_string()),
        horizon: request.horizon.unwrap_or_else(|| "intraday".to_string()),
        mode: config.trading_mode.clone(),
        recommended_action,
        final_action,
        confidence,
        risk_score,
        raw_score,
        execution_status,
        components,
        safety_checks,
        order_intent,
        warnings,
        generated_at_ms: now_ms(),
    })
}

fn constant_time_equals(candidate: &str, expected: &str) -> bool {
    let candidate = candidate.as_bytes();
    let expected = expected.as_bytes();
    if candidate.len() != expected.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in candidate.iter().zip(expected.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

fn request_is_authorized(headers: &HeaderMap, secret: &str) -> bool {
    ["x-server-auth", "auth", "x-trading-server-auth"]
        .iter()
        .filter_map(|name| headers.get(*name))
        .filter_map(|value| value.to_str().ok())
        .any(|value| constant_time_equals(value, secret))
}

fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), AuthFailure> {
    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        if state.config.allow_unauthenticated {
            return Ok(());
        }
        state
            .metrics
            .auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(AuthFailure::MissingSecret);
    };

    if request_is_authorized(headers, secret) {
        Ok(())
    } else {
        state
            .metrics
            .auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        Err(AuthFailure::Unauthorized)
    }
}

fn auth_failure_response(failure: AuthFailure) -> Response {
    match failure {
        AuthFailure::MissingSecret => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "ok": false,
                "error": "SERVER_AUTH_SECRET is not configured"
            })),
        )
            .into_response(),
        AuthFailure::Unauthorized => (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "ok": false,
                "error": "unauthorized",
                "errMessage": "missing required trading server auth header"
            })),
        )
            .into_response(),
    }
}

fn public_platform_descriptors(platforms: &[TradingPlatform]) -> Vec<Value> {
    platforms
        .iter()
        .map(|platform| {
            json!({
                "slug": &platform.slug,
                "displayName": &platform.display_name,
                "provider": &platform.provider,
                "status": &platform.status,
                "supportsPaper": platform.supports_paper,
                "supportsLive": platform.supports_live,
                "assetClasses": &platform.asset_classes,
                "orderTypes": &platform.order_types,
                "baseUrls": &platform.base_urls,
                "labels": &platform.labels,
                "metaData": &platform.meta_data
            })
        })
        .collect()
}

fn service_descriptor(state: &AppState) -> serde_json::Value {
    let platforms = platform_snapshot(state);
    let public_platforms = public_platform_descriptors(&platforms.platforms);
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "mode": &state.config.trading_mode,
        "liveOrdersEnabled": state.config.live_orders_enabled,
        "authRequired": state.config.server_auth_secret.is_some(),
        "endpoints": {
            "schema": "GET /schema",
            "example": "GET /example",
            "decide": "POST /decide",
            "healthz": "GET /healthz",
            "readyz": "GET /readyz",
            "metrics": "GET /metrics"
        },
        "upstreams": {
            "scraper": &state.config.scraper_base_url,
            "aiMlPipeline": &state.config.ml_base_url,
            "mdpOptimizer": &state.config.mdp_base_url
        },
        "nats": {
            "signalSubject": &state.config.signal_subject,
            "queueGroup": &state.config.queue_group,
            "decisionSubject": &state.config.decision_subject,
            "orderIntentSubject": &state.config.order_intent_subject,
            "eventSubject": &state.config.event_subject
        },
        "appConfig": {
            "postgresConfigured": state.config.database_url.is_some(),
            "scope": &state.config.app_config_scope,
            "key": &state.config.app_config_key,
            "refreshSeconds": state.config.config_refresh.as_secs(),
            "defaultPlatform": platforms.default_platform.as_deref(),
            "lastConfigRefreshMs": platforms.last_config_refresh_ms,
            "lastConfigError": platforms.last_config_error.as_deref()
        },
        "tradingPlatforms": public_platforms,
        "safety": {
            "liveTradingRequires": "TRADING_MODE=live and TRADING_ALLOW_LIVE_ORDERS=true",
            "executor": "not implemented; this service emits order intents only",
            "defaultLimits": &state.config.default_limits
        },
        "atMs": now_ms()
    })
}

fn schema_descriptor() -> serde_json::Value {
    json!({
        "schemaVersion": SCHEMA_VERSION,
        "request": {
            "symbol": "required ticker, pair, or instrument id",
            "targetPlatform": "optional platform slug from app_config trading.platforms.v1",
            "market": "bid/ask/lastPrice, realizedVolatility, and optional recent prices",
            "webSignals": "scraper-derived sentiment signals in [-1, 1]",
            "mlFeatures": "AI/ML feature values normalized to [-1, 1]",
            "mdpPolicy": "optional MDP/POMDP action hint: buy, sell, hold",
            "constraints": "per-request risk overrides for notional, confidence, risk, and shorting"
        },
        "response": {
            "recommendedAction": "raw buy/sell/hold recommendation",
            "finalAction": "risk-gated action",
            "orderIntent": "intent-only paper/live order payload when safety gates pass"
        }
    })
}

fn example_request() -> DecisionRequest {
    DecisionRequest {
        request_id: Some("example-trading-decision".to_string()),
        schema_version: Some(SCHEMA_VERSION.to_string()),
        symbol: "AAPL".to_string(),
        venue: Some("NASDAQ".to_string()),
        target_platform: Some("interactive-brokers".to_string()),
        strategy: Some("www-mdp-risk-gated".to_string()),
        horizon: Some("intraday".to_string()),
        portfolio: Some(PortfolioSnapshot {
            cash: Some(50_000.0),
            equity: Some(100_000.0),
            current_position: Some(20.0),
            average_entry_price: Some(185.0),
        }),
        market: Some(MarketSnapshot {
            last_price: Some(192.40),
            bid: Some(192.35),
            ask: Some(192.45),
            day_volume: Some(45_000_000.0),
            realized_volatility: Some(0.24),
            prices: Some(vec![188.10, 189.30, 190.20, 192.40]),
        }),
        web_signals: Some(vec![WebSignal {
            source: Some("dd-web-scraper".to_string()),
            url: Some("https://example.invalid/market-note".to_string()),
            title: Some("supply chain sentiment improving".to_string()),
            sentiment: 0.62,
            confidence: Some(0.74),
            relevance: Some(0.82),
            age_ms: Some(900_000),
        }]),
        ml_features: Some(vec![
            ModelFeature {
                name: "newsMomentum".to_string(),
                value: 0.58,
                weight: Some(1.2),
                higher_is_better: Some(true),
            },
            ModelFeature {
                name: "drawdownRisk".to_string(),
                value: 0.18,
                weight: Some(0.8),
                higher_is_better: Some(false),
            },
        ]),
        mdp_policy: Some(MdpPolicyHint {
            action: "buy".to_string(),
            confidence: Some(0.68),
            value: Some(1.8),
            risk: Some(0.31),
        }),
        constraints: Some(RiskLimits {
            max_order_notional: Some(2_500.0),
            max_position_notional: Some(20_000.0),
            max_symbol_exposure_pct: Some(0.18),
            min_confidence: Some(0.50),
            max_risk_score: Some(0.70),
            allow_short: Some(false),
        }),
        dry_run: Some(true),
    }
}

async fn publish_decision(state: &AppState, response: &DecisionResponse) {
    let Some(nats) = &state.nats else {
        return;
    };

    let decision_payload = match serde_json::to_vec(&json!({
        "messageKind": "trading.decision.result",
        "source": SERVICE_NAME,
        "result": response,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("trading server failed to encode decision: {error}");
            return;
        }
    };
    match nats
        .publish(
            state.config.decision_subject.clone(),
            decision_payload.clone().into(),
        )
        .await
    {
        Ok(_) => {
            state
                .metrics
                .nats_published_total
                .fetch_add(1, Ordering::Relaxed);
        }
        Err(error) => eprintln!("trading server failed to publish decision: {error}"),
    }

    if let Some(order_intent) = response.order_intent.as_ref() {
        let order_payload = match serde_json::to_vec(&json!({
            "messageKind": "trading.order_intent",
            "source": SERVICE_NAME,
            "intent": order_intent,
        })) {
            Ok(payload) => payload,
            Err(error) => {
                eprintln!("trading server failed to encode order intent: {error}");
                return;
            }
        };
        match nats
            .publish(
                state.config.order_intent_subject.clone(),
                order_payload.clone().into(),
            )
            .await
        {
            Ok(_) => {
                state
                    .metrics
                    .nats_published_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => eprintln!("trading server failed to publish order intent: {error}"),
        }
    }

    let _ = nats
        .publish(
            state.config.event_subject.clone(),
            json!({
                "type": "trading.decision",
                "source": SERVICE_NAME,
                "requestId": &response.request_id,
                "symbol": &response.symbol,
                "recommendedAction": &response.recommended_action,
                "finalAction": &response.final_action,
                "confidence": response.confidence,
                "riskScore": response.risk_score,
                "mode": &response.mode,
                "atMs": now_ms()
            })
            .to_string()
            .into(),
        )
        .await;
}

async fn root(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(service_descriptor(&state))
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let platforms = platform_snapshot(&state);
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "mode": &state.config.trading_mode,
        "liveOrdersEnabled": state.config.live_orders_enabled,
        "platformCount": platforms.platforms.len(),
        "lastConfigRefreshMs": platforms.last_config_refresh_ms,
        "lastConfigError": platforms.last_config_error,
        "atMs": now_ms(),
    }))
}

async fn readyz(State(state): State<AppState>) -> Response {
    let platforms = platform_snapshot(&state);
    let ready = !platforms.platforms.is_empty() && platforms.last_config_error.is_none();
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
            "mode": &state.config.trading_mode,
            "platformCount": platforms.platforms.len(),
            "lastConfigRefreshMs": platforms.last_config_refresh_ms,
            "lastConfigError": platforms.last_config_error,
            "atMs": now_ms(),
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

async fn metrics(State(state): State<AppState>) -> Response {
    let body = format!(
        "# HELP dd_trading_server_http_requests_total HTTP requests observed by the trading service.\n\
         # TYPE dd_trading_server_http_requests_total counter\n\
         dd_trading_server_http_requests_total {}\n\
         # HELP dd_trading_server_decisions_total Trading decisions evaluated.\n\
         # TYPE dd_trading_server_decisions_total counter\n\
         dd_trading_server_decisions_total {}\n\
         # HELP dd_trading_server_order_intents_total Order intents produced after safety gates.\n\
         # TYPE dd_trading_server_order_intents_total counter\n\
         dd_trading_server_order_intents_total {}\n\
         # HELP dd_trading_server_blocked_orders_total Recommendations converted to hold by safety gates.\n\
         # TYPE dd_trading_server_blocked_orders_total counter\n\
         dd_trading_server_blocked_orders_total {}\n\
         # HELP dd_trading_server_auth_failures_total Rejected HTTP requests with missing or invalid auth.\n\
         # TYPE dd_trading_server_auth_failures_total counter\n\
         dd_trading_server_auth_failures_total {}\n\
         # HELP dd_trading_server_errors_total Decision or message errors.\n\
         # TYPE dd_trading_server_errors_total counter\n\
         dd_trading_server_errors_total {}\n\
         # HELP dd_trading_server_nats_messages_total NATS signal messages received.\n\
         # TYPE dd_trading_server_nats_messages_total counter\n\
         dd_trading_server_nats_messages_total {}\n\
         # HELP dd_trading_server_nats_published_total NATS decision/order messages published.\n\
         # TYPE dd_trading_server_nats_published_total counter\n\
         dd_trading_server_nats_published_total {}\n\
         # HELP dd_trading_server_config_refresh_total Successful trading platform config refreshes.\n\
         # TYPE dd_trading_server_config_refresh_total counter\n\
         dd_trading_server_config_refresh_total {}\n\
         # HELP dd_trading_server_config_refresh_failures_total Failed trading platform config refreshes.\n\
         # TYPE dd_trading_server_config_refresh_failures_total counter\n\
         dd_trading_server_config_refresh_failures_total {}\n",
        state.metrics.http_requests_total.load(Ordering::Relaxed),
        state.metrics.decisions_total.load(Ordering::Relaxed),
        state.metrics.order_intents_total.load(Ordering::Relaxed),
        state.metrics.blocked_orders_total.load(Ordering::Relaxed),
        state.metrics.auth_failures_total.load(Ordering::Relaxed),
        state.metrics.errors_total.load(Ordering::Relaxed),
        state.metrics.nats_messages_total.load(Ordering::Relaxed),
        state.metrics.nats_published_total.load(Ordering::Relaxed),
        state.metrics.config_refresh_total.load(Ordering::Relaxed),
        state
            .metrics
            .config_refresh_failures_total
            .load(Ordering::Relaxed),
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

async fn decide_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DecisionRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(failure);
    }

    let platforms = platform_snapshot(&state);
    match evaluate_decision(&state.config, &platforms, request) {
        Ok(response) => {
            state
                .metrics
                .decisions_total
                .fetch_add(1, Ordering::Relaxed);
            if response.order_intent.is_some() {
                state
                    .metrics
                    .order_intents_total
                    .fetch_add(1, Ordering::Relaxed);
            } else if response.recommended_action != response.final_action {
                state
                    .metrics
                    .blocked_orders_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            publish_decision(&state, &response).await;
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

async fn run_nats_loop(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        println!("trading server nats loop disabled: NATS_URL is not configured");
        return;
    };
    println!(
        "trading server nats loop starting: subject={} queueGroup={} decisionSubject={}",
        state.config.signal_subject, state.config.queue_group, state.config.decision_subject
    );
    let mut subscription = match nats
        .queue_subscribe(
            state.config.signal_subject.clone(),
            state.config.queue_group.clone(),
        )
        .await
    {
        Ok(subscription) => subscription,
        Err(error) => {
            eprintln!("trading server nats subscribe failed: {error}");
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
                "trading server rejected oversize nats signal: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                payload.len()
            );
            continue;
        }
        let task_state = state.clone();
        tokio::spawn(async move {
            match serde_json::from_slice::<DecisionRequest>(&payload) {
                Ok(request) => {
                    let platforms = platform_snapshot(&task_state);
                    match evaluate_decision(&task_state.config, &platforms, request) {
                        Ok(response) => {
                            task_state
                                .metrics
                                .decisions_total
                                .fetch_add(1, Ordering::Relaxed);
                            if response.order_intent.is_some() {
                                task_state
                                    .metrics
                                    .order_intents_total
                                    .fetch_add(1, Ordering::Relaxed);
                            } else if response.recommended_action != response.final_action {
                                task_state
                                    .metrics
                                    .blocked_orders_total
                                    .fetch_add(1, Ordering::Relaxed);
                            }
                            publish_decision(&task_state, &response).await;
                        }
                        Err(error) => {
                            task_state
                                .metrics
                                .errors_total
                                .fetch_add(1, Ordering::Relaxed);
                            eprintln!("trading server failed nats decision: {error}");
                        }
                    }
                }
                Err(error) => {
                    task_state
                        .metrics
                        .errors_total
                        .fetch_add(1, Ordering::Relaxed);
                    eprintln!("trading server invalid nats signal: {error}");
                }
            }
        });
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8103").parse::<u16>()?;
    let nats = match env::var("NATS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(url) => Some(async_nats::connect(url).await?),
        None => None,
    };
    let state = AppState {
        config: Arc::new(config_from_env()),
        platform_config: Arc::new(RwLock::new(default_platform_config())),
        nats,
        metrics: Arc::new(Metrics::default()),
    };
    if let Err(error) = refresh_platform_config(&state).await {
        eprintln!("trading platform initial config refresh failed: {error}");
        record_config_error(&state, error).await;
    }
    tokio::spawn(run_config_refresh_loop(state.clone()));
    tokio::spawn(run_nats_loop(state.clone()));
    tokio::spawn(run_cdc_refresh_subscription(state.clone()));

    let app = Router::new()
        .route("/", get(root))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/schema", get(schema))
        .route("/example", get(example))
        .route("/decide", post(decide_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state);

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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(mode: &str) -> Config {
        Config {
            trading_mode: normalized_mode(mode),
            live_orders_enabled: false,
            server_auth_secret: Some("secret".to_string()),
            allow_unauthenticated: false,
            database_url: None,
            app_config_scope: "default".to_string(),
            app_config_key: "trading.platforms.v1".to_string(),
            config_refresh: Duration::from_secs(30),
            scraper_base_url: "http://scraper".to_string(),
            ml_base_url: "http://ml".to_string(),
            mdp_base_url: "http://mdp".to_string(),
            signal_subject: "dd.remote.trading.signals".to_string(),
            queue_group: "dd-trading-server".to_string(),
            decision_subject: "dd.remote.trading.decisions".to_string(),
            order_intent_subject: "dd.remote.trading.order_intents".to_string(),
            event_subject: "dd.remote.events".to_string(),
            default_limits: RiskLimits {
                max_order_notional: Some(1_000.0),
                max_position_notional: Some(10_000.0),
                max_symbol_exposure_pct: Some(0.50),
                min_confidence: Some(0.40),
                max_risk_score: Some(0.80),
                allow_short: Some(false),
            },
        }
    }

    fn positive_request() -> DecisionRequest {
        DecisionRequest {
            request_id: Some("unit-buy".to_string()),
            schema_version: Some(SCHEMA_VERSION.to_string()),
            symbol: "aapl".to_string(),
            venue: Some("NASDAQ".to_string()),
            target_platform: Some("interactive-brokers".to_string()),
            strategy: None,
            horizon: None,
            portfolio: Some(PortfolioSnapshot {
                cash: Some(25_000.0),
                equity: Some(50_000.0),
                current_position: Some(10.0),
                average_entry_price: Some(100.0),
            }),
            market: Some(MarketSnapshot {
                last_price: Some(110.0),
                bid: Some(109.9),
                ask: Some(110.1),
                day_volume: Some(1_000_000.0),
                realized_volatility: Some(0.20),
                prices: Some(vec![100.0, 104.0, 110.0]),
            }),
            web_signals: Some(vec![WebSignal {
                source: Some("scraper".to_string()),
                url: None,
                title: None,
                sentiment: 0.8,
                confidence: Some(0.9),
                relevance: Some(0.9),
                age_ms: Some(60_000),
            }]),
            ml_features: Some(vec![ModelFeature {
                name: "newsMomentum".to_string(),
                value: 0.7,
                weight: Some(1.0),
                higher_is_better: Some(true),
            }]),
            mdp_policy: Some(MdpPolicyHint {
                action: "buy".to_string(),
                confidence: Some(0.75),
                value: Some(2.0),
                risk: Some(0.2),
            }),
            constraints: None,
            dry_run: Some(true),
        }
    }

    #[test]
    fn positive_signals_create_paper_buy_intent() {
        let platforms = default_platform_config();
        let response = evaluate_decision(&test_config("paper"), &platforms, positive_request())
            .expect("decision ok");

        assert_eq!(response.symbol, "AAPL");
        assert_eq!(response.recommended_action, "buy");
        assert_eq!(response.final_action, "buy");
        assert_eq!(response.execution_status, "paper_intent_ready");
        assert!(response.confidence >= 0.40);
        assert!(response.order_intent.is_some());
        assert_eq!(
            response.order_intent.as_ref().unwrap().platform,
            "interactive-brokers"
        );
    }

    #[test]
    fn disabled_mode_blocks_buy_intent() {
        let platforms = default_platform_config();
        let response = evaluate_decision(&test_config("disabled"), &platforms, positive_request())
            .expect("decision ok");

        assert_eq!(response.recommended_action, "buy");
        assert_eq!(response.final_action, "hold");
        assert_eq!(response.execution_status, "blocked_by_safety_gate");
        assert!(response.order_intent.is_none());
        assert!(response
            .safety_checks
            .iter()
            .any(|check| check.name == "modeAllowsIntent" && !check.ok));
    }

    #[test]
    fn high_risk_signal_is_converted_to_hold() {
        let mut request = positive_request();
        request.market.as_mut().unwrap().realized_volatility = Some(0.95);
        request.mdp_policy.as_mut().unwrap().risk = Some(0.95);
        request.constraints = Some(RiskLimits {
            max_order_notional: Some(1_000.0),
            max_position_notional: Some(10_000.0),
            max_symbol_exposure_pct: Some(0.50),
            min_confidence: Some(0.40),
            max_risk_score: Some(0.70),
            allow_short: Some(false),
        });

        let platforms = default_platform_config();
        let response =
            evaluate_decision(&test_config("paper"), &platforms, request).expect("decision ok");

        assert_eq!(response.recommended_action, "buy");
        assert_eq!(response.final_action, "hold");
        assert!(response
            .safety_checks
            .iter()
            .any(|check| check.name == "riskCeiling" && !check.ok));
    }

    #[test]
    fn shorting_requires_existing_position_or_override() {
        let mut request = positive_request();
        request.web_signals.as_mut().unwrap()[0].sentiment = -0.9;
        request.ml_features.as_mut().unwrap()[0].value = -0.8;
        request.mdp_policy.as_mut().unwrap().action = "sell".to_string();
        request.portfolio.as_mut().unwrap().current_position = Some(0.0);

        let platforms = default_platform_config();
        let response =
            evaluate_decision(&test_config("paper"), &platforms, request).expect("decision ok");

        assert_eq!(response.recommended_action, "sell");
        assert_eq!(response.final_action, "hold");
        assert!(response
            .safety_checks
            .iter()
            .any(|check| check.name == "shortingPolicy" && !check.ok));
    }

    #[test]
    fn request_constraints_cannot_loosen_server_defaults() {
        let mut request = positive_request();
        request.constraints = Some(RiskLimits {
            max_order_notional: Some(100_000.0),
            max_position_notional: Some(1_000_000.0),
            max_symbol_exposure_pct: Some(1.0),
            min_confidence: Some(0.0),
            max_risk_score: Some(1.0),
            allow_short: Some(true),
        });

        let platforms = default_platform_config();
        let response =
            evaluate_decision(&test_config("paper"), &platforms, request).expect("decision ok");

        let intent = response.order_intent.expect("paper order intent");
        assert!(intent.notional <= 1_000.0);
    }

    #[test]
    fn request_constraints_cannot_enable_shorting_when_server_disallows() {
        let mut request = positive_request();
        request.web_signals.as_mut().unwrap()[0].sentiment = -0.9;
        request.ml_features.as_mut().unwrap()[0].value = -0.8;
        request.mdp_policy.as_mut().unwrap().action = "sell".to_string();
        request.portfolio.as_mut().unwrap().current_position = Some(0.0);
        request.constraints = Some(RiskLimits {
            max_order_notional: Some(100_000.0),
            max_position_notional: Some(1_000_000.0),
            max_symbol_exposure_pct: Some(1.0),
            min_confidence: Some(0.0),
            max_risk_score: Some(1.0),
            allow_short: Some(true),
        });

        let platforms = default_platform_config();
        let response =
            evaluate_decision(&test_config("paper"), &platforms, request).expect("decision ok");

        assert_eq!(response.recommended_action, "sell");
        assert_eq!(response.final_action, "hold");
        assert!(response
            .safety_checks
            .iter()
            .any(|check| check.name == "shortingPolicy" && !check.ok));
    }

    #[test]
    fn invalid_market_and_signal_inputs_are_rejected() {
        let platforms = default_platform_config();

        let mut bad_sentiment = positive_request();
        bad_sentiment.web_signals.as_mut().unwrap()[0].sentiment = 1.5;
        let error = evaluate_decision(&test_config("paper"), &platforms, bad_sentiment)
            .expect_err("out of range sentiment should fail validation");
        assert!(error.contains("webSignals sentiment"));

        let mut crossed_market = positive_request();
        crossed_market.market.as_mut().unwrap().bid = Some(111.0);
        crossed_market.market.as_mut().unwrap().ask = Some(110.0);
        let error = evaluate_decision(&test_config("paper"), &platforms, crossed_market)
            .expect_err("crossed bid/ask should fail validation");
        assert!(error.contains("market.bid"));
    }

    #[test]
    fn platform_config_rejects_duplicate_slugs_and_invalid_defaults() {
        let platform = json!({
            "slug": "dup-platform",
            "displayName": "Dup Platform",
            "provider": "dup",
            "status": "active",
            "supportsPaper": true,
            "supportsLive": false,
            "assetClasses": ["equities"],
            "orderTypes": ["limit"],
            "baseUrls": { "paper": "https://example.com" },
            "credentialKeys": ["DUP_API_KEY"]
        });
        let duplicate = platform_config_from_app_config_value(json!({
            "defaultPlatform": "dup-platform",
            "platforms": [platform.clone(), platform]
        }))
        .expect_err("duplicate platform slugs should fail");
        assert!(duplicate.contains("duplicate platform slug"));

        let missing_default = platform_config_from_app_config_value(json!({
            "defaultPlatform": "missing-platform",
            "platforms": [{
                "slug": "real-platform",
                "displayName": "Real Platform",
                "provider": "real",
                "status": "active",
                "supportsPaper": true,
                "supportsLive": false,
                "assetClasses": ["equities"],
                "orderTypes": ["limit"],
                "baseUrls": { "paper": "https://example.com" },
                "credentialKeys": ["REAL_API_KEY"]
            }]
        }))
        .expect_err("missing default should fail");
        assert!(missing_default.contains("defaultPlatform"));
    }

    #[test]
    fn service_descriptor_redacts_credential_references() {
        let state = AppState {
            config: Arc::new(test_config("paper")),
            platform_config: Arc::new(RwLock::new(default_platform_config())),
            nats: None,
            metrics: Arc::new(Metrics::default()),
        };
        let descriptor = service_descriptor(&state);
        let descriptor_text = descriptor.to_string();

        assert!(descriptor["tradingPlatforms"].is_array());
        assert!(!descriptor_text.contains("credentialKeys"));
        assert!(!descriptor_text.contains("credentialSecret"));
        assert!(!descriptor_text.contains("IBKR_ACCOUNT_ID"));
    }
}
