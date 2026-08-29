use std::{
    env,
    sync::{atomic::AtomicU64, Arc, RwLock},
};

use crate::{types::PatentMatterPackage, MAX_MATTERS_DEFAULT};

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) metrics: Arc<Metrics>,
    pub(crate) store: Arc<RwLock<PatentStore>>,
    pub(crate) http: reqwest::Client,
    pub(crate) ai_permits: Arc<tokio::sync::Semaphore>,
}

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) server_auth_secret: Option<String>,
    pub(crate) allow_unauthenticated: bool,
    pub(crate) patent_center_url: String,
    pub(crate) max_matters: usize,
    pub(crate) anthropic_api_key: Option<String>,
    pub(crate) anthropic_base_url: String,
    pub(crate) ai_model: String,
    pub(crate) ai_max_concurrency: usize,
}

#[derive(Default)]
pub(crate) struct Metrics {
    pub(crate) http_requests_total: AtomicU64,
    pub(crate) package_requests_total: AtomicU64,
    pub(crate) readiness_requests_total: AtomicU64,
    pub(crate) search_plan_requests_total: AtomicU64,
    pub(crate) package_reviews_total: AtomicU64,
    pub(crate) claim_checks_total: AtomicU64,
    pub(crate) fee_estimates_total: AtomicU64,
    pub(crate) deadline_requests_total: AtomicU64,
    pub(crate) ai_drafts_total: AtomicU64,
    pub(crate) ai_draft_errors_total: AtomicU64,
    pub(crate) ai_throttled_total: AtomicU64,
    pub(crate) auth_failures_total: AtomicU64,
    pub(crate) errors_total: AtomicU64,
}

#[derive(Default)]
pub(crate) struct PatentStore {
    pub(crate) matters: Vec<PatentMatterPackage>,
}

fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn env_value(key: &str, fallback: &str) -> String {
    optional_env(key).unwrap_or_else(|| fallback.to_string())
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

fn env_usize(key: &str, fallback: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

pub(crate) fn config_from_env() -> Config {
    Config {
        server_auth_secret: optional_env("PATENT_FILING_SERVER_AUTH_SECRET")
            .or_else(|| optional_env("SERVER_AUTH_SECRET")),
        allow_unauthenticated: env_bool("PATENT_FILING_ALLOW_UNAUTHENTICATED", false),
        patent_center_url: env_value(
            "PATENT_FILING_CENTER_URL",
            "https://patentcenter.uspto.gov/",
        ),
        max_matters: env_usize("PATENT_FILING_MAX_MATTERS", MAX_MATTERS_DEFAULT),
        anthropic_api_key: optional_env("PATENT_FILING_ANTHROPIC_API_KEY")
            .or_else(|| optional_env("ANTHROPIC_API_KEY")),
        anthropic_base_url: env_value("PATENT_FILING_ANTHROPIC_BASE_URL", "https://api.anthropic.com"),
        ai_model: env_value("PATENT_FILING_AI_MODEL", "claude-opus-4-8"),
        ai_max_concurrency: env_usize("PATENT_FILING_AI_MAX_CONCURRENCY", 4),
    }
}

pub(crate) fn store_package(state: &AppState, package: PatentMatterPackage) {
    let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
    store.matters.insert(0, package);
    if store.matters.len() > state.config.max_matters {
        store.matters.truncate(state.config.max_matters);
    }
}

pub(crate) fn package_snapshot(state: &AppState) -> Vec<PatentMatterPackage> {
    state
        .store
        .read()
        .unwrap_or_else(|lock| lock.into_inner())
        .matters
        .clone()
}

pub(crate) fn get_package(state: &AppState, matter_id: &str) -> Option<PatentMatterPackage> {
    state
        .store
        .read()
        .unwrap_or_else(|lock| lock.into_inner())
        .matters
        .iter()
        .find(|package| package.matter_id == matter_id)
        .cloned()
}
