use std::{
    sync::{
        atomic::AtomicU64,
        Arc, RwLock,
    },
    time::Duration,
};

use dd_nats_subject_defs::{
    RUNTIME_EVENTS_SUBJECT, TRADING_DECISIONS_SUBJECT, TRADING_ORDER_INTENTS_SUBJECT,
    TRADING_SIGNALS_QUEUE_GROUP, TRADING_SIGNALS_SUBJECT,
};

use crate::platforms::TradingPlatformConfig;
use crate::types::RiskLimits;
use crate::util::{env_bool, env_f64, env_u64, env_value, first_env, optional_env};

pub(crate) const SCHEMA_VERSION: &str = "trading.decision.v1";
pub(crate) const SERVICE_NAME: &str = "dd-trading-server";
pub(crate) const MAX_HTTP_BODY_BYTES: usize = 512 * 1024;
pub(crate) const MAX_NATS_PAYLOAD_BYTES: usize = 512 * 1024;
pub(crate) const MAX_SYMBOL_LEN: usize = 32;
pub(crate) const MAX_REQUEST_ID_LEN: usize = 128;
pub(crate) const MAX_LABEL_LEN: usize = 96;
pub(crate) const MAX_WEB_SIGNALS: usize = 128;
pub(crate) const MAX_FEATURES: usize = 128;
pub(crate) const MAX_PRICE_POINTS: usize = 512;
pub(crate) const MAX_SIGNAL_AGE_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) platform_config: Arc<RwLock<TradingPlatformConfig>>,
    pub(crate) nats: Option<async_nats::Client>,
    pub(crate) metrics: Arc<Metrics>,
    // Bounds the number of decisions evaluated concurrently off the NATS
    // signal stream. Without this a burst of signals would spawn an
    // unbounded number of tasks and exhaust memory/CPU.
    pub(crate) inflight: Arc<tokio::sync::Semaphore>,
}

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) trading_mode: String,
    pub(crate) live_orders_enabled: bool,
    // Global kill switch. When true every decision is forced to `hold`
    // regardless of signals or mode. Intended as an operator-flippable
    // circuit breaker that takes effect on the next config refresh.
    pub(crate) halted: bool,
    pub(crate) max_inflight: usize,
    // Maximum age of a timestamped market snapshot before the freshness gate
    // forces a hold. Only enforced when the request supplies `market.asOfMs`.
    pub(crate) max_price_age: Duration,
    pub(crate) server_auth_secret: Option<String>,
    pub(crate) allow_unauthenticated: bool,
    pub(crate) database_url: Option<String>,
    pub(crate) app_config_scope: String,
    pub(crate) app_config_key: String,
    pub(crate) config_refresh: Duration,
    pub(crate) scraper_base_url: String,
    pub(crate) ml_base_url: String,
    pub(crate) mdp_base_url: String,
    pub(crate) signal_subject: String,
    pub(crate) queue_group: String,
    pub(crate) decision_subject: String,
    pub(crate) order_intent_subject: String,
    pub(crate) event_subject: String,
    pub(crate) default_limits: RiskLimits,
}

#[derive(Default)]
pub(crate) struct Metrics {
    pub(crate) http_requests_total: AtomicU64,
    pub(crate) decisions_total: AtomicU64,
    pub(crate) order_intents_total: AtomicU64,
    pub(crate) blocked_orders_total: AtomicU64,
    pub(crate) auth_failures_total: AtomicU64,
    pub(crate) errors_total: AtomicU64,
    pub(crate) nats_messages_total: AtomicU64,
    pub(crate) nats_published_total: AtomicU64,
    pub(crate) config_refresh_total: AtomicU64,
    pub(crate) config_refresh_failures_total: AtomicU64,
}

pub(crate) fn normalized_mode(input: &str) -> String {
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

pub(crate) fn config_from_env() -> Config {
    Config {
        trading_mode: normalized_mode(&env_value("TRADING_MODE", "paper")),
        live_orders_enabled: env_bool("TRADING_ALLOW_LIVE_ORDERS", false),
        halted: env_bool("TRADING_HALT", false),
        // Cap concurrent decision evaluations from the NATS stream.
        max_inflight: env_u64("TRADING_MAX_INFLIGHT", 256).clamp(1, 4096) as usize,
        max_price_age: Duration::from_millis(env_u64("TRADING_MAX_PRICE_AGE_MS", 300_000)),
        server_auth_secret: optional_env("SERVER_AUTH_SECRET")
            .or_else(|| optional_env("TRADING_SERVER_AUTH_SECRET")),
        allow_unauthenticated: env_bool("TRADING_ALLOW_UNAUTHENTICATED", false),
        database_url: first_env(&[
            "TRADING_DATABASE_URL",
            "RDS_DATABASE_URL",
            "AGENT_TASKS_RDS_DATABASE_URL",
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
        signal_subject: env_value("TRADING_SIGNAL_SUBJECT", TRADING_SIGNALS_SUBJECT),
        queue_group: env_value("TRADING_QUEUE_GROUP", TRADING_SIGNALS_QUEUE_GROUP),
        decision_subject: env_value("TRADING_DECISION_SUBJECT", TRADING_DECISIONS_SUBJECT),
        order_intent_subject: env_value(
            "TRADING_ORDER_INTENT_SUBJECT",
            TRADING_ORDER_INTENTS_SUBJECT,
        ),
        event_subject: env_value("TRADING_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
        default_limits: default_limits(),
    }
}
