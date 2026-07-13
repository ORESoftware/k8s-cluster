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
    /// Server-rendered HTMX operator console under `/app`.
    pub app_ui_enabled: bool,
    /// External path prefix the console is reached through (e.g. `/usacc`
    /// behind the gateway, empty for direct access). Every link, form
    /// action, and HTMX target the console renders is prefixed with this so
    /// the same binary works directly and behind the path-stripping gateway.
    pub app_base_path: String,
    /// Optional `Authorization: Bearer` token gating every `/app` request.
    /// When unset the console is open and intended for trusted networks
    /// (front it with the gateway auth, as the JSON API already is).
    pub app_ui_bearer: Option<String>,
    /// Extra origins allowed to issue console writes (CSRF allow-list). The
    /// request `Host` is always treated as same-origin.
    pub app_ui_allowed_origins: Vec<String>,
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
            app_ui_enabled: env_bool("USACC_APP_UI_ENABLED", true),
            app_base_path: normalize_base_path(&env_value("USACC_APP_BASE_PATH", "")),
            app_ui_bearer: first_env(&["USACC_APP_UI_BEARER"]),
            app_ui_allowed_origins: split_origins(&env_value("USACC_APP_UI_ALLOWED_ORIGINS", "")),
        }
    }
}

/// Trim a base path to the `/foo` shape: no trailing slash, a single
/// leading slash, empty stays empty.
fn normalize_base_path(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn split_origins(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
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
