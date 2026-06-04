//! Environment-variable backed configuration.
//!
//! Keep this file free of business logic: just parsing and validation.

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: String,
    pub port: u16,

    /// HMAC-SHA256 secret used to validate `X-Hub-Signature-256` on incoming
    /// GitHub webhooks. Required: rejecting unsigned traffic is a hard rule.
    pub github_webhook_secret: String,

    /// Optional GitHub token. When `Some`, the service can clone private
    /// repos and POST check-run results back to the PR. When `None`, the
    /// service still accepts webhooks and runs analysis but treats outbound
    /// reporting as a no-op (logged only).
    pub github_token: Option<String>,
    pub github_api_base_url: String,

    /// Directory under which transient PR checkouts are placed.
    pub workdir_root: PathBuf,

    /// Path to the Cargo manifest to check on the PR head commit. Relative
    /// to the cloned worktree root.
    pub contract_manifest_path: PathBuf,

    /// Specific cargo package to test (passed as `-p`). When `None`, the
    /// whole workspace is exercised.
    pub cargo_test_package: Option<String>,

    /// Comma-separated cargo features.
    pub cargo_test_features: Option<String>,

    pub max_concurrent_analyses: usize,
    pub analyzer_timeout: Duration,

    // -------------------------------------------------------------------
    // Per-step enable flags. Each defaults to `true` for the baseline
    // (cargo + proptest), `true` for Kani/Verus/dReal (the analyzers self-
    // skip when their tool is absent), and `false` for Certora.
    // -------------------------------------------------------------------
    pub enable_cargo_check: bool,
    pub enable_cargo_test: bool,
    pub enable_proptest: bool,
    pub enable_kani: bool,
    pub enable_verus: bool,
    pub enable_dreal: bool,
    pub enable_certora: bool,

    /// Integration test target name for proptest, relative to the package
    /// in `cargo_test_package` (default `proptest_props`).
    pub proptest_test_target: String,

    /// Verus proof crate directory, relative to the cloned PR worktree.
    pub verus_proof_crate_dir: PathBuf,

    /// Directory containing dReal SMT-LIB queries, relative to the cloned
    /// PR worktree.
    pub dreal_queries_dir: PathBuf,
    pub dreal_precision: f64,

    /// Directory containing Certora run configurations, relative to the
    /// cloned PR worktree.
    pub certora_conf_dir: PathBuf,

    // -------------------------------------------------------------------
    // Webhook hardening knobs.
    // -------------------------------------------------------------------
    /// Allowlist of `owner/repo` slugs that this service is willing to
    /// analyze. Empty list means "allow everything" (suitable for dev /
    /// self-hosted single-repo deployments). A single `*` entry is the
    /// same as empty.
    pub allowed_repos: Vec<String>,

    /// File-path prefixes (relative to repo root) that, when touched by a
    /// PR, trigger an analysis run. Empty list means "run on every PR".
    pub path_prefixes: Vec<String>,

    /// How many `X-GitHub-Delivery` IDs to remember at once.
    pub delivery_dedupe_capacity: usize,

    /// How long to remember a delivery ID for dedupe purposes.
    pub delivery_dedupe_ttl: Duration,

    /// Maximum number of changed-file entries we will fetch from GitHub
    /// for the path-filter decision. Beyond this we conservatively assume
    /// the PR is in scope and run the pipeline. Each page is 100 entries.
    pub max_pr_files_pages: usize,

    /// GitHub commit-status `context` string posted by this service. This
    /// is the name that branch-protection requires-check rules match on,
    /// so it must stay stable across deployments of the same logical
    /// pipeline.
    pub status_context: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bind_addr = env_or("BIND_ADDR", "0.0.0.0");
        let port: u16 = env_or("PORT", "3010")
            .parse()
            .context("PORT must be a u16")?;

        let github_webhook_secret = env::var("GITHUB_WEBHOOK_SECRET")
            .map_err(|_| anyhow!("GITHUB_WEBHOOK_SECRET is required"))?;
        if github_webhook_secret.trim().is_empty() {
            return Err(anyhow!("GITHUB_WEBHOOK_SECRET must not be empty"));
        }

        let github_token = env::var("GITHUB_TOKEN")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let github_api_base_url = env_or("GITHUB_API_BASE_URL", "https://api.github.com");

        let workdir_root: PathBuf = env_or("WORKDIR_ROOT", ".work").into();
        let contract_manifest_path: PathBuf =
            env_or("CONTRACT_MANIFEST_PATH", "packages/contract/Cargo.toml").into();
        let cargo_test_package = env::var("CARGO_TEST_PACKAGE")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let cargo_test_features = env::var("CARGO_TEST_FEATURES")
            .ok()
            .filter(|s| !s.trim().is_empty());

        let max_concurrent_analyses: usize = env_or("MAX_CONCURRENT_ANALYSES", "2")
            .parse()
            .context("MAX_CONCURRENT_ANALYSES must be a non-negative integer")?;
        let max_concurrent_analyses = max_concurrent_analyses.max(1);

        let timeout_secs: u64 = env_or("ANALYZER_TIMEOUT_SECS", "900")
            .parse()
            .context("ANALYZER_TIMEOUT_SECS must be a non-negative integer")?;
        let analyzer_timeout = Duration::from_secs(timeout_secs);

        let enable_cargo_check = bool_env("FORMAL_METHODS_CARGO_CHECK_ENABLED", true);
        let enable_cargo_test = bool_env("FORMAL_METHODS_CARGO_TEST_ENABLED", true);
        let enable_proptest = bool_env("FORMAL_METHODS_PROPTEST_ENABLED", true);
        let enable_kani = bool_env("FORMAL_METHODS_KANI_ENABLED", true);
        let enable_verus = bool_env("FORMAL_METHODS_VERUS_ENABLED", true);
        let enable_dreal = bool_env("FORMAL_METHODS_DREAL_ENABLED", true);
        let enable_certora = bool_env("FORMAL_METHODS_CERTORA_ENABLED", false);

        let proptest_test_target = env_or("PROPTEST_TEST_TARGET", "proptest_props");

        let verus_proof_crate_dir: PathBuf = env_or(
            "VERUS_PROOF_CRATE_DIR",
            "packages/contract/om-core/proofs/verus",
        )
        .into();
        let dreal_queries_dir: PathBuf = env_or(
            "DREAL_QUERIES_DIR",
            "packages/contract/om-core/proofs/dreal",
        )
        .into();
        let dreal_precision: f64 = env_or("DREAL_PRECISION", "0.001")
            .parse()
            .context("DREAL_PRECISION must be a positive float")?;
        let certora_conf_dir: PathBuf = env_or(
            "CERTORA_CONF_DIR",
            "packages/contract/om-core/proofs/certora/conf",
        )
        .into();

        let allowed_repos = parse_csv_env("FORMAL_METHODS_ALLOWED_REPOS");
        let path_prefixes = parse_csv_env("FORMAL_METHODS_PATH_PREFIXES");

        let delivery_dedupe_capacity: usize = env_or("DELIVERY_DEDUPE_CAPACITY", "1024")
            .parse()
            .context("DELIVERY_DEDUPE_CAPACITY must be a non-negative integer")?;
        let delivery_dedupe_capacity = delivery_dedupe_capacity.max(1);

        let dedupe_ttl_secs: u64 = env_or("DELIVERY_DEDUPE_TTL_SECS", "3600")
            .parse()
            .context("DELIVERY_DEDUPE_TTL_SECS must be a non-negative integer")?;
        let delivery_dedupe_ttl = Duration::from_secs(dedupe_ttl_secs);

        let max_pr_files_pages: usize = env_or("MAX_PR_FILES_PAGES", "3")
            .parse()
            .context("MAX_PR_FILES_PAGES must be a non-negative integer")?;
        let max_pr_files_pages = max_pr_files_pages.max(1);

        let status_context = env_or("STATUS_CONTEXT", "formal-methods/analysis");

        Ok(Self {
            bind_addr,
            port,
            github_webhook_secret,
            github_token,
            github_api_base_url,
            workdir_root,
            contract_manifest_path,
            cargo_test_package,
            cargo_test_features,
            max_concurrent_analyses,
            analyzer_timeout,
            enable_cargo_check,
            enable_cargo_test,
            enable_proptest,
            enable_kani,
            enable_verus,
            enable_dreal,
            enable_certora,
            proptest_test_target,
            verus_proof_crate_dir,
            dreal_queries_dir,
            dreal_precision,
            certora_conf_dir,
            allowed_repos,
            path_prefixes,
            delivery_dedupe_capacity,
            delivery_dedupe_ttl,
            max_pr_files_pages,
            status_context,
        })
    }
}

/// Parses a comma-separated env var into a trimmed, deduped, non-empty list.
/// Missing or empty env vars produce an empty `Vec`.
fn parse_csv_env(key: &str) -> Vec<String> {
    let Ok(raw) = env::var(key) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for piece in raw.split(',') {
        let trimmed = piece.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lowered = trimmed.to_string();
        if seen.insert(lowered.clone()) {
            out.push(lowered);
        }
    }
    out
}

/// Parses a bool-shaped env var. Accepts `true`/`false`/`1`/`0`/`yes`/`no`,
/// case-insensitive. Missing or empty falls back to `default`.
fn bool_env(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            match v.as_str() {
                "" => default,
                "1" | "true" | "yes" | "y" | "on" => true,
                "0" | "false" | "no" | "n" | "off" => false,
                _ => default,
            }
        }
        Err(_) => default,
    }
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    // Serialize all env-mutating tests in this module. `cargo test` runs
    // tests on multiple threads inside one process, so without this lock the
    // tests interfere via the shared process environment.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env<F: FnOnce()>(pairs: &[(&str, Option<&str>)], f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // Snapshot every env var we plan to touch so we can restore after the
        // test, even if it panics.
        let all_keys: Vec<&str> = pairs
            .iter()
            .map(|(k, _)| *k)
            .chain([
                "GITHUB_WEBHOOK_SECRET",
                "GITHUB_TOKEN",
                "BIND_ADDR",
                "PORT",
                "MAX_CONCURRENT_ANALYSES",
                "ANALYZER_TIMEOUT_SECS",
                "GITHUB_API_BASE_URL",
                "WORKDIR_ROOT",
                "CONTRACT_MANIFEST_PATH",
                "CARGO_TEST_PACKAGE",
                "CARGO_TEST_FEATURES",
                "FORMAL_METHODS_ALLOWED_REPOS",
                "FORMAL_METHODS_PATH_PREFIXES",
                "DELIVERY_DEDUPE_CAPACITY",
                "DELIVERY_DEDUPE_TTL_SECS",
                "MAX_PR_FILES_PAGES",
                "STATUS_CONTEXT",
            ])
            .collect();
        let saved: Vec<(String, Option<String>)> = all_keys
            .iter()
            .map(|k| (k.to_string(), env::var(k).ok()))
            .collect();

        // Wipe everything we care about, then apply only what the test asked
        // for. This makes each test fully self-contained.
        for k in &all_keys {
            env::remove_var(k);
        }
        for (k, v) in pairs {
            if let Some(val) = v {
                env::set_var(k, val);
            }
        }

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));

        for (k, v) in saved {
            match v {
                Some(val) => env::set_var(&k, val),
                None => env::remove_var(&k),
            }
        }
        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    #[test]
    fn missing_secret_errors() {
        with_env(&[("GITHUB_WEBHOOK_SECRET", None)], || {
            let err = Config::from_env().unwrap_err();
            assert!(err.to_string().contains("GITHUB_WEBHOOK_SECRET"));
        });
    }

    #[test]
    fn empty_secret_errors() {
        with_env(&[("GITHUB_WEBHOOK_SECRET", Some(""))], || {
            let err = Config::from_env().unwrap_err();
            assert!(err.to_string().contains("must not be empty"));
        });
    }

    #[test]
    fn defaults_when_secret_present() {
        with_env(
            &[
                ("GITHUB_WEBHOOK_SECRET", Some("shh")),
                ("GITHUB_TOKEN", None),
                ("BIND_ADDR", None),
                ("PORT", None),
                ("MAX_CONCURRENT_ANALYSES", None),
                ("ANALYZER_TIMEOUT_SECS", None),
            ],
            || {
                let cfg = Config::from_env().expect("config");
                assert_eq!(cfg.bind_addr, "0.0.0.0");
                assert_eq!(cfg.port, 3010);
                assert!(cfg.github_token.is_none());
                assert_eq!(cfg.max_concurrent_analyses, 2);
                assert_eq!(cfg.analyzer_timeout, Duration::from_secs(900));
            },
        );
    }

    #[test]
    fn max_concurrent_clamped_to_one() {
        with_env(
            &[
                ("GITHUB_WEBHOOK_SECRET", Some("shh")),
                ("MAX_CONCURRENT_ANALYSES", Some("0")),
            ],
            || {
                let cfg = Config::from_env().unwrap();
                assert_eq!(cfg.max_concurrent_analyses, 1);
            },
        );
    }

    #[test]
    fn parses_csv_lists_trimming_and_dedup() {
        with_env(
            &[
                ("GITHUB_WEBHOOK_SECRET", Some("shh")),
                (
                    "FORMAL_METHODS_ALLOWED_REPOS",
                    Some(" acme/widgets , acme/widgets ,  acme/sandbox "),
                ),
                (
                    "FORMAL_METHODS_PATH_PREFIXES",
                    Some("packages/contract/,, packages/math/, packages/contract/"),
                ),
            ],
            || {
                let cfg = Config::from_env().unwrap();
                assert_eq!(
                    cfg.allowed_repos,
                    vec![
                        "acme/widgets".to_string(),
                        "acme/sandbox".to_string(),
                    ]
                );
                assert_eq!(
                    cfg.path_prefixes,
                    vec![
                        "packages/contract/".to_string(),
                        "packages/math/".to_string(),
                    ]
                );
            },
        );
    }

    #[test]
    fn empty_csv_lists_yield_empty_vecs() {
        with_env(
            &[
                ("GITHUB_WEBHOOK_SECRET", Some("shh")),
                ("FORMAL_METHODS_ALLOWED_REPOS", None),
                ("FORMAL_METHODS_PATH_PREFIXES", Some("")),
            ],
            || {
                let cfg = Config::from_env().unwrap();
                assert!(cfg.allowed_repos.is_empty());
                assert!(cfg.path_prefixes.is_empty());
            },
        );
    }

    #[test]
    fn dedupe_capacity_is_clamped_to_one() {
        with_env(
            &[
                ("GITHUB_WEBHOOK_SECRET", Some("shh")),
                ("DELIVERY_DEDUPE_CAPACITY", Some("0")),
            ],
            || {
                let cfg = Config::from_env().unwrap();
                assert_eq!(cfg.delivery_dedupe_capacity, 1);
            },
        );
    }
}
