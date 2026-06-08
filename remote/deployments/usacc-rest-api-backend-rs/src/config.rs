use std::{env, time::Duration};

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_url: Option<String>,
    pub auth_secret: Option<String>,
    pub auth_required: bool,
    pub contract_service_url: String,
    pub request_timeout: Duration,
    pub max_page_limit: i64,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            host: env_value("HOST", "0.0.0.0"),
            port: env_u16("PORT", 8121),
            database_url: first_env(&[
                "USACC_DATABASE_URL",
                "RDS_DATABASE_URL",
                "DATABASE_URL",
                "AGENT_TASKS_RDS_DATABASE_URL",
            ]),
            auth_secret: first_env(&["USACC_API_AUTH_SECRET", "SERVER_AUTH_SECRET"]),
            auth_required: env_bool("USACC_API_AUTH_REQUIRED", true),
            contract_service_url: env_value(
                "USACC_CONTRACT_SERVICE_URL",
                "http://dd-contract-service.default.svc.cluster.local:8101",
            ),
            request_timeout: Duration::from_secs(env_u64("USACC_REQUEST_TIMEOUT_SECONDS", 20)),
            max_page_limit: env_i64("USACC_MAX_PAGE_LIMIT", 250).clamp(1, 1000),
        }
    }
}

pub fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
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

fn env_u16(key: &str, fallback: u16) -> u16 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn env_i64(key: &str, fallback: i64) -> i64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}
