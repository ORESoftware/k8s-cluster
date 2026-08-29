use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    sync::{Arc, RwLock},
};

use dd_nats_subject_defs::{
    ECONOMICS_FORECAST_REQUESTS_SUBJECT, ECONOMICS_FORECAST_RESULTS_SUBJECT,
    ECONOMICS_MARKET_EVENTS_SUBJECT, ECONOMICS_SERVER_QUEUE_GROUP,
    PUBLIC_DATA_PIPELINE_JOBS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};

use crate::metrics::*;
use crate::shared::*;
use crate::types::*;

pub(crate) const SERVICE_NAME: &str = "dd-economics-server";
pub(crate) const SCHEMA_VERSION: &str = "economics.forecast.v1";
pub(crate) const DEFAULT_HISTORY_YEARS: u32 = 15;
pub(crate) const DEFAULT_PROJECTION_MONTHS: u32 = 18;
pub(crate) const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_NATS_PAYLOAD_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_SOURCE_FETCH_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_SERIES: usize = 160;
pub(crate) const MAX_OBSERVATIONS_PER_SERIES: usize = 8_000;
pub(crate) const MAX_TOKEN_LEN: usize = 128;
pub(crate) const MAX_URL_LEN: usize = 2_048;
pub(crate) const MAX_JSON_POINTER_LEN: usize = 256;
pub(crate) const MAX_SENTIMENT_DOCUMENTS: usize = 512;
pub(crate) const MAX_SENTIMENT_TEXT_BYTES: usize = 4_096;
pub(crate) const MAX_SENTIMENT_CONTEXT_SCORES: usize = 512;
pub(crate) const MAX_VC_DEALS: usize = 256;
pub(crate) const MAX_VC_SECTOR_FLOWS: usize = 128;
pub(crate) const MAX_PIPELINE_JOB_INTENTS: usize = 12;
// Subject + queue-group names come from the shared @dd/nats-subject-defs lib
// (schema/economics.schema.json) so producers/consumers across languages cannot drift.
pub(crate) const ECONOMICS_FORECAST_REQUEST_SUBJECT: &str = ECONOMICS_FORECAST_REQUESTS_SUBJECT;
pub(crate) const ECONOMICS_FORECAST_RESULT_SUBJECT: &str = ECONOMICS_FORECAST_RESULTS_SUBJECT;
pub(crate) const ECONOMICS_MARKET_EVENT_SUBJECT: &str = ECONOMICS_MARKET_EVENTS_SUBJECT;
pub(crate) const DEFAULT_SPARK_PIPELINE_URL: &str =
    "http://dd-spark-pipeline-server.ai-ml.svc.cluster.local:8085";
pub(crate) const DEFAULT_SPARK_MASTER_URL: &str = "spark://spark-master.big-data.svc.cluster.local:7077";
pub(crate) const DEFAULT_AIRFLOW_API_URL: &str = "http://airflow.big-data.svc.cluster.local:8080";
pub(crate) const DEFAULT_DATA_LAKE_URI: &str = "s3a://dd-economics/market-signals";
pub(crate) const ECONOMICS_QUEUE_GROUP: &str = ECONOMICS_SERVER_QUEUE_GROUP;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) metrics: Arc<Metrics>,
    pub(crate) nats: Option<async_nats::Client>,
    pub(crate) http: reqwest::Client,
    pub(crate) series_store: Arc<RwLock<BTreeMap<String, MarketSeries>>>,
}

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) server_auth_secret: Option<String>,
    pub(crate) allow_unauthenticated: bool,
    pub(crate) allow_private_source_urls: bool,
    pub(crate) allowed_source_hosts: Vec<String>,
    pub(crate) allowed_source_auth_envs: Vec<String>,
    pub(crate) sentiment_credentials: SentimentCredentialStatus,
    pub(crate) market_data_credentials: MarketDataCredentialStatus,
    pub(crate) history_years: u32,
    pub(crate) projection_months: u32,
    pub(crate) confidence_level: f64,
    pub(crate) request_subject: String,
    pub(crate) queue_group: String,
    pub(crate) result_subject: String,
    pub(crate) market_event_subject: String,
    pub(crate) runtime_event_subject: String,
    pub(crate) pipeline_intent_subject: String,
    pub(crate) spark_pipeline_url: Option<String>,
    pub(crate) spark_pipeline_auth_env: String,
    pub(crate) spark_master_url: String,
    pub(crate) airflow_api_url: Option<String>,
    pub(crate) databricks_host: Option<String>,
    pub(crate) data_lake_uri: String,
    pub(crate) allow_pipeline_submit: bool,
    pub(crate) allow_external_pipeline_urls: bool,
}

pub(crate) fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

pub(crate) fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn env_list(key: &str) -> Vec<String> {
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

pub(crate) fn default_source_auth_envs() -> Vec<String> {
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

pub(crate) fn configured_source_auth_envs() -> Vec<String> {
    let mut allowed = default_source_auth_envs()
        .into_iter()
        .collect::<BTreeSet<_>>();
    for env_name in env_list("ECONOMICS_ALLOWED_SOURCE_AUTH_ENVS") {
        allowed.insert(env_name);
    }
    allowed.into_iter().collect()
}

pub(crate) fn env_bool(key: &str, fallback: bool) -> bool {
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

pub(crate) fn env_u32(key: &str, fallback: u32) -> u32 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

pub(crate) fn env_f64(key: &str, fallback: f64) -> f64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(fallback)
}

pub(crate) fn config_from_env() -> Config {
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

pub(crate) fn sentiment_credentials_from_env() -> SentimentCredentialStatus {
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

pub(crate) fn market_data_credentials_from_env() -> MarketDataCredentialStatus {
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
