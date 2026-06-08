use std::{env, path::PathBuf, time::Duration};

use crate::util::normalize_key;

pub const DEFAULT_PORT: u16 = 8118;
pub const SERVICE_NAME: &str = "dd-compliance-rs";
pub const SCHEMA_VERSION: &str = "compliance.audit.v1";

#[derive(Clone, Debug)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub work_root: PathBuf,
    pub server_auth_secret: Option<String>,
    pub allow_unauthenticated: bool,
    pub allow_external_fetch: bool,
    pub allow_repo_clone: bool,
    pub allow_private_targets: bool,
    pub allowed_repo_prefixes: Vec<String>,
    pub allowed_file_extensions: Vec<String>,
    pub git_bin: String,
    pub job_timeout: Duration,
    pub max_jobs: usize,
    pub max_http_body_bytes: usize,
    pub max_artifact_bytes: usize,
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub max_findings_per_job: usize,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            host: env_value("HOST", "0.0.0.0"),
            port: env_u16("PORT", DEFAULT_PORT),
            work_root: PathBuf::from(env_value(
                "COMPLIANCE_WORK_ROOT",
                "/var/lib/dd-compliance-rs/jobs",
            )),
            server_auth_secret: optional_env("COMPLIANCE_SERVER_AUTH_SECRET")
                .or_else(|| optional_env("SERVER_AUTH_SECRET")),
            allow_unauthenticated: env_bool("COMPLIANCE_ALLOW_UNAUTHENTICATED", false),
            allow_external_fetch: env_bool("COMPLIANCE_ALLOW_EXTERNAL_FETCH", false),
            allow_repo_clone: env_bool("COMPLIANCE_ALLOW_REPO_CLONE", false),
            allow_private_targets: env_bool("COMPLIANCE_ALLOW_PRIVATE_TARGETS", false),
            allowed_repo_prefixes: csv_env("COMPLIANCE_ALLOWED_REPO_PREFIXES"),
            allowed_file_extensions: csv_env_or(
                "COMPLIANCE_ALLOWED_FILE_EXTENSIONS",
                "rs,go,ts,tsx,js,jsx,mjs,cjs,py,java,kt,scala,c,h,cc,cpp,hpp,cs,swift,gleam,erl,ex,exs,tf,yaml,yml,json,toml,md,sh,bash,dockerfile",
            )
            .into_iter()
            .map(|ext| normalize_key(ext.trim_start_matches('.')))
            .collect(),
            git_bin: env_value("COMPLIANCE_GIT_BIN", "git"),
            job_timeout: Duration::from_secs(env_u64("COMPLIANCE_JOB_TIMEOUT_SECONDS", 900)),
            max_jobs: env_usize("COMPLIANCE_MAX_JOBS", 200),
            max_http_body_bytes: env_usize("COMPLIANCE_MAX_HTTP_BODY_BYTES", 1024 * 1024),
            max_artifact_bytes: env_usize("COMPLIANCE_MAX_ARTIFACT_BYTES", 512 * 1024),
            max_files: env_usize("COMPLIANCE_MAX_FILES", 1200),
            max_file_bytes: env_u64("COMPLIANCE_MAX_FILE_BYTES", 256 * 1024),
            max_findings_per_job: env_usize("COMPLIANCE_MAX_FINDINGS_PER_JOB", 5000),
        }
    }
}

pub fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_value(key: &str, fallback: &str) -> String {
    optional_env(key).unwrap_or_else(|| fallback.to_string())
}

fn env_bool(key: &str, fallback: bool) -> bool {
    optional_env(key)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(fallback)
}

fn env_u16(key: &str, fallback: u16) -> u16 {
    optional_env(key)
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    optional_env(key)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn env_usize(key: &str, fallback: usize) -> usize {
    optional_env(key)
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn csv_env(key: &str) -> Vec<String> {
    optional_env(key)
        .map(|value| parse_csv(&value))
        .unwrap_or_default()
}

fn csv_env_or(key: &str, fallback: &str) -> Vec<String> {
    optional_env(key)
        .map(|value| parse_csv(&value))
        .unwrap_or_else(|| parse_csv(fallback))
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}
