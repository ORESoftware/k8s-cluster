use crate::platforms::{
    platform_available_for_action, select_platform, TradingPlatform, TradingPlatformConfig,
};
use crate::state::{Config, MAX_SIGNAL_AGE_MS, SCHEMA_VERSION};
use crate::types::{
    DecisionRequest, DecisionResponse, MarketSnapshot, MdpPolicyHint, ModelFeature, OrderIntent,
    RiskLimits, SafetyCheck, ScoreComponent, WebSignal,
};
use crate::util::{bounded, now_ms};
use crate::validation::{merge_limits, normalize_symbol, request_id, validate_request};

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

pub(crate) fn evaluate_decision(
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
        "tradingNotHalted",
        !config.halted,
        "blocker",
        if config.halted {
            "TRADING_HALT kill switch is engaged; all orders forced to hold".to_string()
        } else {
            "trading kill switch is disengaged".to_string()
        },
    ));
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
    // Freshness gate: only enforced when the caller timestamps the snapshot.
    // A future asOfMs (clock skew) saturates to age 0 and passes.
    let price_age_ms = request
        .market
        .as_ref()
        .and_then(|market| market.as_of_ms)
        .map(|as_of| now_ms().saturating_sub(u128::from(as_of)));
    let max_price_age_ms = config.max_price_age.as_millis();
    let market_data_fresh = price_age_ms
        .map(|age| age <= max_price_age_ms)
        .unwrap_or(true);
    safety_checks.push(safety_check(
        "marketDataFresh",
        recommended_action == "hold" || market_data_fresh,
        "blocker",
        match price_age_ms {
            Some(age) => format!("market data age {age}ms vs maxPriceAgeMs {max_price_age_ms}"),
            None => "market snapshot has no asOfMs; freshness gate not enforced".to_string(),
        },
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

/// Strip credential references (the k8s secret name + key names) from a
/// decision. These are reconnaissance-useful and are deliberately kept off the
/// public service descriptor; the HTTP `/decide` response and the broad
/// `decisions` telemetry subject must match that posture. Only the executor —
/// which consumes the dedicated `order_intents` subject — needs them, and it
/// can equally resolve them from the platform slug via app_config.
pub(crate) fn without_intent_credentials(response: &DecisionResponse) -> DecisionResponse {
    let mut redacted = response.clone();
    if let Some(intent) = redacted.order_intent.as_mut() {
        intent.credential_secret = String::new();
        intent.credential_keys = Vec::new();
    }
    redacted
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use std::time::Duration;

    use dd_nats_subject_defs::{
        RUNTIME_EVENTS_SUBJECT, TRADING_DECISIONS_SUBJECT, TRADING_ORDER_INTENTS_SUBJECT,
        TRADING_SIGNALS_QUEUE_GROUP, TRADING_SIGNALS_SUBJECT,
    };

    use crate::platforms::default_platform_config;
    use crate::state::normalized_mode;
    use crate::types::PortfolioSnapshot;

    pub(crate) fn test_config(mode: &str) -> Config {
        Config {
            trading_mode: normalized_mode(mode),
            live_orders_enabled: false,
            halted: false,
            max_inflight: 256,
            max_price_age: Duration::from_millis(300_000),
            server_auth_secret: Some("secret".to_string()),
            allow_unauthenticated: false,
            database_url: None,
            app_config_scope: "default".to_string(),
            app_config_key: "trading.platforms.v1".to_string(),
            config_refresh: Duration::from_secs(30),
            scraper_base_url: "http://scraper".to_string(),
            ml_base_url: "http://ml".to_string(),
            mdp_base_url: "http://mdp".to_string(),
            signal_subject: TRADING_SIGNALS_SUBJECT.to_string(),
            queue_group: TRADING_SIGNALS_QUEUE_GROUP.to_string(),
            decision_subject: TRADING_DECISIONS_SUBJECT.to_string(),
            order_intent_subject: TRADING_ORDER_INTENTS_SUBJECT.to_string(),
            event_subject: RUNTIME_EVENTS_SUBJECT.to_string(),
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
            as_of_ms: None,
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
    fn halt_kill_switch_forces_hold() {
        let mut config = test_config("paper");
        config.halted = true;
        let platforms = default_platform_config();
        let response =
            evaluate_decision(&config, &platforms, positive_request()).expect("decision ok");

        assert_eq!(response.recommended_action, "buy");
        assert_eq!(response.final_action, "hold");
        assert_eq!(response.execution_status, "blocked_by_safety_gate");
        assert!(response.order_intent.is_none());
        assert!(response
            .safety_checks
            .iter()
            .any(|check| check.name == "tradingNotHalted" && !check.ok));
    }

    #[test]
    fn zero_cash_account_is_accepted_not_rejected() {
        let mut request = positive_request();
        request.portfolio.as_mut().unwrap().cash = Some(0.0);
        request.portfolio.as_mut().unwrap().equity = Some(0.0);
        let platforms = default_platform_config();
        // Previously this returned a validation error ("must be positive");
        // a flat account is valid input and should evaluate to a decision.
        let response =
            evaluate_decision(&test_config("paper"), &platforms, request).expect("decision ok");
        // With zero cash the buy cap collapses, so no buy intent is emitted.
        assert!(response.order_intent.is_none());
    }

    #[test]
    fn credential_references_are_redacted_from_public_view() {
        let platforms = default_platform_config();
        let response = evaluate_decision(&test_config("paper"), &platforms, positive_request())
            .expect("decision ok");
        // The internal decision keeps credential refs for the executor channel.
        let full_intent = response.order_intent.as_ref().expect("order intent");
        assert!(!full_intent.credential_secret.is_empty());
        assert!(!full_intent.credential_keys.is_empty());

        // The public view (HTTP response + decisions subject) must not.
        let public = without_intent_credentials(&response);
        let public_intent = public.order_intent.as_ref().expect("order intent");
        assert!(public_intent.credential_secret.is_empty());
        assert!(public_intent.credential_keys.is_empty());
        // Non-sensitive fields are preserved.
        assert_eq!(public_intent.platform, full_intent.platform);
        assert_eq!(public_intent.notional, full_intent.notional);
        let public_text = serde_json::to_string(&public).expect("serialize");
        assert!(!public_text.contains("IBKR_ACCOUNT_ID"));
        assert!(!public_text.contains("dd-trading-broker-secrets"));
    }

    #[test]
    fn stale_market_data_forces_hold() {
        let mut request = positive_request();
        // Timestamp the snapshot far in the past relative to the 300s default.
        request.market.as_mut().unwrap().as_of_ms = Some(1);
        let platforms = default_platform_config();
        let response =
            evaluate_decision(&test_config("paper"), &platforms, request).expect("decision ok");

        assert_eq!(response.recommended_action, "buy");
        assert_eq!(response.final_action, "hold");
        assert!(response.order_intent.is_none());
        assert!(response
            .safety_checks
            .iter()
            .any(|check| check.name == "marketDataFresh" && !check.ok));
    }

    #[test]
    fn fresh_market_data_passes_freshness_gate() {
        let mut request = positive_request();
        request.market.as_mut().unwrap().as_of_ms = Some(now_ms() as u64);
        let platforms = default_platform_config();
        let response =
            evaluate_decision(&test_config("paper"), &platforms, request).expect("decision ok");

        assert_eq!(response.final_action, "buy");
        assert!(response
            .safety_checks
            .iter()
            .any(|check| check.name == "marketDataFresh" && check.ok));
    }
}
