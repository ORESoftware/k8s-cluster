use crate::state::{
    MAX_FEATURES, MAX_LABEL_LEN, MAX_PRICE_POINTS, MAX_REQUEST_ID_LEN, MAX_SYMBOL_LEN,
    MAX_WEB_SIGNALS, SCHEMA_VERSION,
};
use crate::types::{DecisionRequest, RiskLimits};

pub(crate) fn request_id(input: Option<&String>, prefix: &str) -> String {
    // Drop control characters so a caller-supplied id can't inject newlines
    // into logs or break downstream NATS/JSON consumers, then length-cap.
    let sanitized: String = input
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(prefix)
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_REQUEST_ID_LEN)
        .collect();
    if sanitized.is_empty() {
        prefix.chars().take(MAX_REQUEST_ID_LEN).collect()
    } else {
        sanitized
    }
}

pub(crate) fn normalize_symbol(input: &str) -> Result<String, String> {
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

pub(crate) fn validate_label(value: &str, label: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if trimmed.len() > MAX_LABEL_LEN {
        return Err(format!("{label} must be at most {MAX_LABEL_LEN} bytes"));
    }
    Ok(())
}

pub(crate) fn validate_credential_key(value: &str, label: &str) -> Result<(), String> {
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

pub(crate) fn validate_local_or_https_url(value: &str, label: &str) -> Result<(), String> {
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
    if lower.starts_with("https://") {
        return Ok(());
    }
    // Plaintext http is only allowed for an actual loopback host. Match the
    // host exactly: the authority must be `localhost`, `127.0.0.1`, or
    // `[::1]` terminated by a port (`:`), a path (`/`), or end-of-string.
    // This rejects look-alikes like `http://localhost.evil.com` that a
    // naive `starts_with` would let through.
    for host in ["localhost", "127.0.0.1", "[::1]"] {
        let prefix = format!("http://{host}");
        if let Some(rest) = lower.strip_prefix(&prefix) {
            if rest.is_empty() || rest.starts_with('/') || rest.starts_with(':') {
                return Ok(());
            }
        }
    }
    Err(format!("{label} must be https or a loopback http URL"))
}

pub(crate) fn safe_slug(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 120
        && bytes[0].is_ascii_lowercase()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
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

pub(crate) fn merge_limits(defaults: &RiskLimits, overrides: Option<RiskLimits>) -> RiskLimits {
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

pub(crate) fn validate_request(request: &DecisionRequest, limits: &RiskLimits) -> Result<Vec<String>, String> {
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
        // Cash and equity may legitimately be zero (a flat or freshly
        // funded account); only reject negative/non-finite values. A
        // zero cash balance still blocks buys via the cash cap downstream.
        finite_nonnegative_optional(portfolio.cash, "portfolio.cash")?;
        finite_nonnegative_optional(portfolio.equity, "portfolio.equity")?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_url_validation_rejects_lookalike_hosts() {
        assert!(validate_local_or_https_url("https://api.example.com", "u").is_ok());
        assert!(validate_local_or_https_url("http://localhost:5000/v1", "u").is_ok());
        assert!(validate_local_or_https_url("http://127.0.0.1/api", "u").is_ok());
        assert!(validate_local_or_https_url("http://[::1]:8080", "u").is_ok());
        // Look-alike authorities that a naive starts_with would accept.
        assert!(validate_local_or_https_url("http://localhost.evil.com", "u").is_err());
        assert!(validate_local_or_https_url("http://127.0.0.1.evil.com", "u").is_err());
        assert!(validate_local_or_https_url("http://example.com", "u").is_err());
    }

    #[test]
    fn request_id_strips_control_characters() {
        let dirty = "abc\ndef\tghi".to_string();
        let cleaned = request_id(Some(&dirty), "fallback");
        assert_eq!(cleaned, "abcdefghi");
        assert!(!cleaned.chars().any(|c| c.is_control()));
    }
}
