use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{json, Value};

use crate::state::{AppState, MAX_LABEL_LEN};
use crate::util::now_ms;
use crate::validation::{
    safe_slug, validate_credential_key, validate_label, validate_local_or_https_url,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TradingPlatformConfig {
    pub(crate) platforms: Vec<TradingPlatform>,
    pub(crate) default_platform: Option<String>,
    pub(crate) last_config_refresh_ms: Option<u128>,
    pub(crate) last_config_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TradingPlatform {
    pub(crate) slug: String,
    pub(crate) display_name: String,
    pub(crate) provider: String,
    pub(crate) status: String,
    pub(crate) supports_paper: bool,
    pub(crate) supports_live: bool,
    pub(crate) asset_classes: Vec<String>,
    pub(crate) order_types: Vec<String>,
    pub(crate) base_urls: BTreeMap<String, String>,
    pub(crate) credential_secret: String,
    pub(crate) credential_keys: Vec<String>,
    pub(crate) account_ref_key: Option<String>,
    pub(crate) labels: Vec<String>,
    pub(crate) meta_data: Value,
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

pub(crate) fn platform_config_from_app_config_value(value: Value) -> Result<TradingPlatformConfig, String> {
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

pub(crate) fn default_platform_config() -> TradingPlatformConfig {
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
                "slug": "tradestation",
                "displayName": "TradeStation",
                "provider": "tradestation",
                "status": "active",
                "supportsPaper": true,
                "supportsLive": true,
                "assetClasses": ["equities", "options", "futures", "commodities"],
                "orderTypes": ["market", "limit", "stop", "stop_limit"],
                "baseUrls": { "paper": "https://sim-api.tradestation.com/v3", "live": "https://api.tradestation.com/v3" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["TRADESTATION_API_KEY", "TRADESTATION_API_SECRET"],
                "accountRefKey": "TRADESTATION_ACCOUNT_ID",
                "labels": ["brokerage", "futures", "commodities"]
            },
            {
                "slug": "tradovate",
                "displayName": "Tradovate",
                "provider": "tradovate",
                "status": "active",
                "supportsPaper": true,
                "supportsLive": true,
                "assetClasses": ["futures", "commodities"],
                "orderTypes": ["market", "limit", "stop", "stop_limit"],
                "baseUrls": { "paper": "https://demo.tradovateapi.com/v1", "live": "https://live.tradovateapi.com/v1" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["TRADOVATE_API_KEY", "TRADOVATE_API_SECRET"],
                "accountRefKey": "TRADOVATE_ACCOUNT_ID",
                "labels": ["futures", "commodities", "paper-first"]
            },
            {
                "slug": "ironbeam",
                "displayName": "Ironbeam",
                "provider": "ironbeam",
                "status": "active",
                "supportsPaper": true,
                "supportsLive": true,
                "assetClasses": ["futures", "commodities"],
                "orderTypes": ["market", "limit", "stop", "stop_limit"],
                "baseUrls": { "paper": "https://demo.ironbeamapi.com/v2", "live": "https://live.ironbeamapi.com/v2" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["IRONBEAM_API_KEY", "IRONBEAM_API_SECRET"],
                "accountRefKey": "IRONBEAM_ACCOUNT_ID",
                "labels": ["futures", "commodities"]
            },
            {
                "slug": "oanda",
                "displayName": "OANDA",
                "provider": "oanda",
                "status": "active",
                "supportsPaper": true,
                "supportsLive": true,
                "assetClasses": ["forex", "commodities", "metals", "indices"],
                "orderTypes": ["market", "limit", "stop", "trailing_stop"],
                "baseUrls": { "paper": "https://api-fxpractice.oanda.com/v3", "live": "https://api-fxtrade.oanda.com/v3" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["OANDA_API_TOKEN"],
                "accountRefKey": "OANDA_ACCOUNT_ID",
                "labels": ["forex", "commodities", "metals", "paper-first"]
            },
            {
                "slug": "saxo",
                "displayName": "Saxo Bank",
                "provider": "saxo",
                "status": "active",
                "supportsPaper": true,
                "supportsLive": true,
                "assetClasses": ["futures", "commodities", "forex", "equities", "options"],
                "orderTypes": ["market", "limit", "stop", "stop_limit"],
                "baseUrls": { "paper": "https://gateway.saxobank.com/sim/openapi", "live": "https://gateway.saxobank.com/openapi" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["SAXO_APP_KEY", "SAXO_APP_SECRET"],
                "accountRefKey": "SAXO_ACCOUNT_KEY",
                "labels": ["brokerage", "multi-asset", "commodities"]
            },
            {
                "slug": "ig",
                "displayName": "IG",
                "provider": "ig",
                "status": "active",
                "supportsPaper": true,
                "supportsLive": true,
                "assetClasses": ["commodities", "indices", "forex", "metals"],
                "orderTypes": ["market", "limit", "stop"],
                "baseUrls": { "paper": "https://demo-api.ig.com/gateway/deal", "live": "https://api.ig.com/gateway/deal" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["IG_API_KEY", "IG_API_SECRET"],
                "accountRefKey": "IG_ACCOUNT_ID",
                "labels": ["cfd", "commodities", "paper-first"]
            },
            {
                "slug": "cqg",
                "displayName": "CQG",
                "provider": "cqg",
                "status": "active",
                "supportsPaper": true,
                "supportsLive": true,
                "assetClasses": ["futures", "commodities", "metals", "energy", "agriculture"],
                "orderTypes": ["market", "limit", "stop", "stop_limit"],
                "baseUrls": { "paper": "https://localhost:2845", "live": "https://localhost:2845" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["CQG_API_KEY", "CQG_API_SECRET"],
                "accountRefKey": "CQG_ACCOUNT_ID",
                "labels": ["futures", "commodities", "gateway"],
                "metaData": { "connector": "cqg-webapi-gateway", "notes": "Routed through an in-cluster CQG WebAPI gateway; base URL is the loopback gateway, not a public endpoint." }
            },
            {
                "slug": "amp-futures",
                "displayName": "AMP Futures",
                "provider": "amp-futures",
                "status": "active",
                "supportsPaper": true,
                "supportsLive": true,
                "assetClasses": ["futures", "commodities", "energy", "metals", "agriculture"],
                "orderTypes": ["market", "limit", "stop", "stop_limit"],
                "baseUrls": { "paper": "https://localhost:2846", "live": "https://localhost:2846" },
                "credentialSecret": "dd-trading-broker-secrets",
                "credentialKeys": ["AMP_FUTURES_API_KEY", "AMP_FUTURES_API_SECRET"],
                "accountRefKey": "AMP_FUTURES_ACCOUNT_ID",
                "labels": ["futures", "commodities", "gateway"],
                "metaData": { "connector": "cqg-or-rithmic-gateway", "notes": "AMP routes orders via CQG/Rithmic; base URL is an in-cluster loopback gateway." }
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

pub(crate) fn platform_snapshot(state: &AppState) -> TradingPlatformConfig {
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

fn mode_supported(platform: &TradingPlatform, mode: &str) -> bool {
    match mode {
        "paper" => platform.supports_paper,
        "live" => platform.supports_live,
        _ => false,
    }
}

pub(crate) fn platform_available_for_action(platform: &TradingPlatform, mode: &str) -> bool {
    platform.status == "active" && mode_supported(platform, mode)
}

pub(crate) fn select_platform(
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn catalog_ships_at_least_fifteen_active_platforms() {
        let platforms = default_platform_config();
        let active = platforms
            .platforms
            .iter()
            .filter(|platform| platform.status == "active")
            .count();
        assert!(
            active >= 15,
            "expected >= 15 active platforms, found {active}"
        );
        // Commodity/futures coverage must be present, not just equities/crypto.
        let commodity_capable = platforms
            .platforms
            .iter()
            .filter(|platform| {
                platform.status == "active"
                    && platform
                        .asset_classes
                        .iter()
                        .any(|class| class == "commodities" || class == "futures")
            })
            .count();
        assert!(
            commodity_capable >= 8,
            "expected >= 8 active commodity/futures venues, found {commodity_capable}"
        );
    }
}
