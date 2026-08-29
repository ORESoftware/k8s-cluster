use std::sync::atomic::Ordering;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::decision::{evaluate_decision, without_intent_credentials};
use crate::nats::publish_decision;
use crate::platforms::{platform_snapshot, TradingPlatform};
use crate::state::{AppState, SCHEMA_VERSION, SERVICE_NAME};
use crate::types::{
    DecisionRequest, MarketSnapshot, MdpPolicyHint, ModelFeature, PortfolioSnapshot, RiskLimits,
    WebSignal,
};
use crate::util::now_ms;

enum AuthFailure {
    MissingSecret,
    Unauthorized,
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
        "halted": state.config.halted,
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
            "killSwitch": "set TRADING_HALT=true to force every decision to hold",
            "halted": state.config.halted,
            "maxInflightDecisions": state.config.max_inflight,
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
            "market": "bid/ask/lastPrice, realizedVolatility, optional recent prices, and optional asOfMs (epoch-ms snapshot time; drives the marketDataFresh gate)",
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
            as_of_ms: Some(now_ms() as u64),
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

pub(crate) async fn root(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(service_descriptor(&state))
}

pub(crate) async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let platforms = platform_snapshot(&state);
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "mode": &state.config.trading_mode,
        "liveOrdersEnabled": state.config.live_orders_enabled,
        "halted": state.config.halted,
        "platformCount": platforms.platforms.len(),
        "lastConfigRefreshMs": platforms.last_config_refresh_ms,
        "lastConfigError": platforms.last_config_error,
        "atMs": now_ms(),
    }))
}

pub(crate) async fn readyz(State(state): State<AppState>) -> Response {
    let platforms = platform_snapshot(&state);
    // Readiness keys off whether we hold a usable platform catalog, NOT off
    // the most recent refresh result. A transient RDS/CDC hiccup sets
    // `last_config_error` while the last-good config stays cached; gating
    // readiness on that error would pull *every* replica out of rotation on
    // a shared-DB blip and take the whole decision service down. The error
    // is surfaced as advisory data and tracked via
    // dd_trading_server_config_refresh_failures_total for alerting instead.
    let ready = !platforms.platforms.is_empty();
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
            "halted": state.config.halted,
            "platformCount": platforms.platforms.len(),
            "lastConfigRefreshMs": platforms.last_config_refresh_ms,
            "lastConfigError": platforms.last_config_error,
            "atMs": now_ms(),
        })),
    )
        .into_response()
}

pub(crate) async fn schema() -> impl IntoResponse {
    Json(schema_descriptor())
}

pub(crate) async fn example() -> impl IntoResponse {
    Json(example_request())
}

pub(crate) async fn metrics(State(state): State<AppState>) -> Response {
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

pub(crate) async fn decide_http(
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
            // Don't return credential references to the HTTP caller; they ride
            // only on the executor's order_intents subject.
            Json(without_intent_credentials(&response)).into_response()
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

pub(crate) async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

pub(crate) async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, RwLock};

    use crate::decision::tests::test_config;
    use crate::platforms::default_platform_config;
    use crate::state::Metrics;

    #[test]
    fn service_descriptor_redacts_credential_references() {
        let state = AppState {
            config: Arc::new(test_config("paper")),
            platform_config: Arc::new(RwLock::new(default_platform_config())),
            nats: None,
            metrics: Arc::new(Metrics::default()),
            inflight: Arc::new(tokio::sync::Semaphore::new(8)),
        };
        let descriptor = service_descriptor(&state);
        let descriptor_text = descriptor.to_string();

        assert!(descriptor["tradingPlatforms"].is_array());
        assert!(!descriptor_text.contains("credentialKeys"));
        assert!(!descriptor_text.contains("credentialSecret"));
        assert!(!descriptor_text.contains("IBKR_ACCOUNT_ID"));
    }
}
