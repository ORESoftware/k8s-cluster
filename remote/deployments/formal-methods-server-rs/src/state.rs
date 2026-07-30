use std::{
    collections::{HashMap, HashSet},
    env,
    path::PathBuf,
    sync::{atomic::AtomicU64, Arc},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::sync::{RwLock, Semaphore};

use crate::types::JobRecord;

// ---------------------------------------------------------------------------
// configuration & shared state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) http: reqwest::Client,
    pub(crate) jobs: Arc<RwLock<HashMap<String, JobRecord>>>,
    pub(crate) semaphore: Arc<Semaphore>,
    pub(crate) counters: Arc<Counters>,
}

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) work_root: PathBuf,
    pub(crate) git_bin: String,
    pub(crate) z3_bin: String,
    pub(crate) allowed_repo_prefixes: Vec<String>,
    pub(crate) allowed_extensions: HashSet<String>,
    pub(crate) job_timeout: Duration,
    pub(crate) z3_timeout: Duration,
    pub(crate) max_log_bytes: u64,
    pub(crate) max_jobs: usize,
    pub(crate) max_files: usize,
    pub(crate) max_file_bytes: u64,
    pub(crate) max_findings_per_job: usize,
    pub(crate) max_inline_source_bytes: usize,
    pub(crate) server_auth_secret: Option<String>,
    pub(crate) github_webhook_secret: Option<String>,
    pub(crate) github_api_token: Option<String>,
    pub(crate) github_api_base: String,
    pub(crate) pr_diff_only: bool,
    pub(crate) pr_comment_enabled: bool,
    pub(crate) pr_comment_max_rows: usize,
    pub(crate) pr_base_fetch_depth: u64,
}

#[derive(Default)]
pub(crate) struct Counters {
    pub(crate) submitted: AtomicU64,
    pub(crate) running: AtomicU64,
    pub(crate) succeeded: AtomicU64,
    pub(crate) failed: AtomicU64,
    pub(crate) rejected: AtomicU64,
    pub(crate) findings_total: AtomicU64,
    pub(crate) z3_calls: AtomicU64,
    pub(crate) z3_failures: AtomicU64,
    pub(crate) webhooks_received: AtomicU64,
    pub(crate) webhooks_rejected: AtomicU64,
    pub(crate) pr_jobs_queued: AtomicU64,
    pub(crate) pr_comments_posted: AtomicU64,
    pub(crate) pr_comments_failed: AtomicU64,
}

// ---------------------------------------------------------------------------
// env / helpers
// ---------------------------------------------------------------------------

pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

pub(crate) fn env_value(key: &str, fallback: &str) -> String {
    first_env(&[key]).unwrap_or_else(|| fallback.to_string())
}

pub(crate) fn env_u64(key: &str, fallback: u64) -> u64 {
    first_env(&[key])
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

pub(crate) fn env_usize(key: &str, fallback: usize) -> usize {
    first_env(&[key])
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_extensions(value: &str) -> HashSet<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|ext| ext.trim_start_matches('.').to_ascii_lowercase())
        .collect()
}

fn resolve_bin(name: &str) -> String {
    if name.contains('/') {
        return name.to_string();
    }
    let Ok(path) = env::var("PATH") else {
        return name.to_string();
    };
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = PathBuf::from(dir).join(name);
        if candidate.is_file() {
            return candidate.to_string_lossy().to_string();
        }
    }
    name.to_string()
}

fn env_bool(key: &str, fallback: bool) -> bool {
    first_env(&[key])
        .map(|v| {
            matches!(
                v.as_str(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(fallback)
}

pub(crate) fn config_from_env() -> Config {
    let github_api_token = first_env(&["GITHUB_API_TOKEN", "GITHUB_TOKEN"]);
    Config {
        work_root: PathBuf::from(env_value(
            "FORMAL_METHODS_WORK_ROOT",
            "/var/lib/dd-formal-methods-server/jobs",
        )),
        git_bin: resolve_bin(&env_value("FORMAL_METHODS_GIT_BIN", "git")),
        z3_bin: resolve_bin(&env_value("FORMAL_METHODS_Z3_BIN", "z3")),
        allowed_repo_prefixes: parse_csv(&env_value(
            "FORMAL_METHODS_ALLOWED_REPO_PREFIXES",
            "https://github.com/,git@github.com:,ssh://git@github.com/",
        )),
        allowed_extensions: parse_extensions(&env_value(
            "FORMAL_METHODS_ALLOWED_EXTENSIONS",
            "rs,go,ts,tsx,js,jsx,mjs,cjs,py,java,kt,scala,c,h,cc,cpp,hpp,cs,swift,gleam,ex,exs,erl,ml,mli,lua,sh,bash,dart,rb,r",
        )),
        job_timeout: Duration::from_secs(env_u64("FORMAL_METHODS_JOB_TIMEOUT_SECONDS", 900)),
        z3_timeout: Duration::from_secs(env_u64("FORMAL_METHODS_Z3_TIMEOUT_SECONDS", 5)),
        max_log_bytes: env_u64("FORMAL_METHODS_MAX_LOG_BYTES", 4 * 1024 * 1024),
        max_jobs: env_usize("FORMAL_METHODS_MAX_JOBS", 200),
        max_files: env_usize("FORMAL_METHODS_MAX_FILES", 5_000),
        max_file_bytes: env_u64("FORMAL_METHODS_MAX_FILE_BYTES", 512 * 1024),
        max_findings_per_job: env_usize("FORMAL_METHODS_MAX_FINDINGS_PER_JOB", 5_000),
        max_inline_source_bytes: env_usize("FORMAL_METHODS_MAX_INLINE_SOURCE_BYTES", 256 * 1024),
        server_auth_secret: first_env(&["FORMAL_METHODS_AUTH_SECRET", "SERVER_AUTH_SECRET"]),
        github_webhook_secret: first_env(&[
            "FORMAL_METHODS_GITHUB_WEBHOOK_SECRET",
            "GITHUB_WEBHOOK_SECRET",
        ]),
        pr_comment_enabled: env_bool(
            "FORMAL_METHODS_PR_COMMENT_ENABLED",
            github_api_token.is_some(),
        ),
        github_api_token,
        github_api_base: env_value("FORMAL_METHODS_GITHUB_API_BASE", "https://api.github.com"),
        pr_diff_only: env_bool("FORMAL_METHODS_PR_DIFF_ONLY", true),
        pr_comment_max_rows: env_usize("FORMAL_METHODS_PR_COMMENT_MAX_ROWS", 25),
        pr_base_fetch_depth: env_u64("FORMAL_METHODS_PR_BASE_FETCH_DEPTH", 200),
    }
}
