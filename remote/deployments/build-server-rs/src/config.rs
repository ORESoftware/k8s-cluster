use std::{collections::HashSet, env, path::PathBuf, time::Duration};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};

use crate::{fiducia, gh_secrets, profiles, webhooks};

pub(crate) mod profile_policy;

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) work_root: PathBuf,
    pub(crate) git_bin: String,
    /// Precomputed Basic authorization header for trusted private GitHub clones.
    /// Never serialized or written to command logs.
    pub(crate) git_http_auth_header: Option<String>,
    pub(crate) nerdctl_bin: String,
    pub(crate) kubectl_bin: String,
    pub(crate) tar_bin: String,
    pub(crate) containerd_namespace: String,
    pub(crate) allowed_repo_prefixes: Vec<String>,
    pub(crate) allowed_image_prefixes: Vec<String>,
    pub(crate) allowed_namespaces: HashSet<String>,
    pub(crate) allowed_profiles: HashSet<String>,
    /// Compiled profile repository policy. Plain entries are organization/repo
    /// prefixes; `exact:` entries are startup-validated repository-to-profile
    /// bindings emitted by config::profile_policy.
    pub(crate) allowed_profile_repo_prefixes: Vec<String>,
    pub(crate) profile_cpus: String,
    pub(crate) profile_memory: String,
    pub(crate) profile_pids_limit: String,
    pub(crate) deploy_enabled: bool,
    pub(crate) push_enabled: bool,
    pub(crate) ecr_login_enabled: bool,
    pub(crate) aws_region: String,
    pub(crate) job_timeout: Duration,
    /// Overall wall-clock deadline for one job (all commands together);
    /// job_timeout still bounds each individual command.
    pub(crate) job_deadline: Duration,
    pub(crate) max_log_bytes: u64,
    pub(crate) max_jobs: usize,
    /// Reject new submissions once this many jobs are queued (backpressure —
    /// authenticated callers must not be able to grow memory unboundedly).
    pub(crate) max_queued: usize,
    /// Keep cloned repos on disk after the job finishes (default: remove;
    /// build logs are always kept).
    pub(crate) keep_workdirs: bool,
    pub(crate) server_auth_secret: Option<String>,

    // --- Postgres (own database dd_build_server; see src/db.rs) ---
    pub(crate) database_url: Option<String>,

    // --- fiducia.cloud coordination (see src/fiducia.rs) ---
    pub(crate) fiducia_url: String,
    pub(crate) fiducia_api_key: Option<String>,
    pub(crate) coordination_enabled: bool,
    pub(crate) coordination_required: bool,
    pub(crate) lock_ttl: Duration,
    pub(crate) lock_wait_budget: Duration,
    pub(crate) lock_retry_interval: Duration,
    pub(crate) idempotency_lease: Duration,
    pub(crate) idempotency_retention: Duration,

    // --- NATS MQ (see src/events.rs) ---
    pub(crate) nats_url: String,
    pub(crate) nats_enabled: bool,
    pub(crate) nats_intake_enabled: bool,
    pub(crate) nats_event_subject: String,
    pub(crate) nats_result_subject: String,
    pub(crate) nats_image_subject: String,
    pub(crate) nats_request_subject: String,
    pub(crate) nats_critical_subject: String,

    // --- Webhooks (see src/webhooks.rs) ---
    pub(crate) github_webhook_secret: Option<String>,
    pub(crate) registry_webhook_secret: Option<String>,
    pub(crate) webhook_rules: Vec<webhooks::WebhookRule>,

    // --- GitHub Actions secret sync (see src/gh_secrets.rs) ---
    pub(crate) gh_sync_enabled: bool,
    pub(crate) gh_sync_token: Option<String>,
    pub(crate) gh_sync_rules: Vec<gh_secrets::SyncRule>,
    pub(crate) gh_sync_interval: Duration,

    // --- gleam-lambda-runner executor (see src/lambda_exec.rs) ---
    pub(crate) lambda_executor_enabled: bool,
    pub(crate) lambda_url: String,
    pub(crate) lambda_function_id: Option<String>,
    pub(crate) lambda_auth_secret: Option<String>,
}

pub(crate) fn first_env(keys: &[&str]) -> Option<String> {
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

pub(crate) fn env_bool(key: &str, fallback: bool) -> bool {
    first_env(&[key])
        .map(|value| {
            matches!(
                value.as_str(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(fallback)
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

pub(crate) fn parse_namespaces(value: &str) -> HashSet<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

pub(crate) fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// Inline JSON env var, or a file path env var (mounted ConfigMap), or None.
pub(crate) fn env_or_file(inline_key: &str, path_key: &str) -> Option<String> {
    if let Some(inline) = first_env(&[inline_key]) {
        return Some(inline);
    }
    let path = first_env(&[path_key])?;
    match std::fs::read_to_string(&path) {
        Ok(contents) => Some(contents),
        Err(error) => {
            tracing::error!("failed to read {path_key}={path}: {error}");
            None
        }
    }
}

pub(crate) fn config_from_env() -> Config {
    let webhook_rules = env_or_file(
        "BUILD_SERVER_WEBHOOK_RULES",
        "BUILD_SERVER_WEBHOOK_RULES_PATH",
    )
    .map(|raw| match webhooks::parse_rules(&raw) {
        Ok(rules) => rules,
        Err(error) => {
            tracing::error!("ignoring webhook rules: {error}");
            Vec::new()
        }
    })
    .unwrap_or_default();

    let gh_sync_policy = gh_secrets::SyncPolicy {
        allowed_owners: parse_csv(&env_value("BUILD_SERVER_GH_SYNC_ALLOWED_OWNERS", "")),
        allowed_env: parse_csv(&env_value("BUILD_SERVER_GH_SYNC_ALLOWED_ENV", "")),
    };
    let gh_sync_rules = env_or_file(
        "BUILD_SERVER_GH_SYNC_RULES",
        "BUILD_SERVER_GH_SYNC_RULES_PATH",
    )
    .map(|raw| match gh_secrets::parse_rules(&raw, &gh_sync_policy) {
        Ok(rules) => rules,
        Err(error) => {
            tracing::error!("ignoring gh secret sync rules: {error}");
            Vec::new()
        }
    })
    .unwrap_or_default();

    let coordination_enabled = env_bool("BUILD_SERVER_COORDINATION_ENABLED", false);
    let github_token = first_env(&["BUILD_SERVER_GIT_TOKEN", "GH_PAT"]);
    let git_http_auth_header = github_token.as_deref().map(|token| {
        format!(
            "AUTHORIZATION: basic {}",
            BASE64.encode(format!("x-access-token:{token}"))
        )
    });
    let fiducia_url = env_value(
        "FIDUCIA_LOCK_URL",
        "http://fiducia-load-balance.fiducia.svc.cluster.local:8088",
    );
    let coordination_enabled = if coordination_enabled {
        match fiducia::validate_lock_url(&fiducia_url) {
            Ok(()) => true,
            Err(error) => {
                tracing::error!("disabling fiducia coordination: {error}");
                false
            }
        }
    } else {
        false
    };

    let allowed_profiles = parse_namespaces(&env_value(
        "BUILD_SERVER_ALLOWED_PROFILES",
        &profiles::names().collect::<Vec<_>>().join(","),
    ));
    let allowed_profile_repo_prefixes = profile_policy::compile_rules(
        parse_csv(&env_value(
            "BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES",
            "https://github.com/ORESoftware/,https://github.com/sonus-auris/,git@github.com:ORESoftware/,git@github.com:sonus-auris/",
        )),
        first_env(&["BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON"]).as_deref(),
        &allowed_profiles,
    )
    .unwrap_or_else(|error| {
        panic!("invalid build-server profile repository policy: {error}")
    });

    Config {
        work_root: PathBuf::from(env_value(
            "BUILD_SERVER_WORK_ROOT",
            "/var/lib/dd-build-server/jobs",
        )),
        git_bin: env_value("BUILD_SERVER_GIT_BIN", "git"),
        git_http_auth_header,
        nerdctl_bin: env_value("BUILD_SERVER_NERDCTL_BIN", "/usr/local/bin/nerdctl"),
        kubectl_bin: env_value("BUILD_SERVER_KUBECTL_BIN", "/usr/bin/kubectl"),
        tar_bin: env_value("BUILD_SERVER_TAR_BIN", "/bin/tar"),
        containerd_namespace: env_value("BUILD_SERVER_CONTAINERD_NAMESPACE", "k8s.io"),
        allowed_repo_prefixes: parse_csv(&env_value("BUILD_SERVER_ALLOWED_REPO_PREFIXES", "")),
        allowed_image_prefixes: parse_csv(&env_value("BUILD_SERVER_ALLOWED_IMAGE_PREFIXES", "")),
        allowed_namespaces: parse_namespaces(&env_value(
            "BUILD_SERVER_ALLOWED_NAMESPACES",
            "default",
        )),
        allowed_profiles,
        allowed_profile_repo_prefixes,
        profile_cpus: env_value("BUILD_SERVER_PROFILE_CPUS", "4"),
        profile_memory: env_value("BUILD_SERVER_PROFILE_MEMORY", "8g"),
        profile_pids_limit: env_value("BUILD_SERVER_PROFILE_PIDS_LIMIT", "2048"),
        deploy_enabled: env_bool("BUILD_SERVER_DEPLOY_ENABLED", true),
        push_enabled: env_bool("BUILD_SERVER_PUSH_ENABLED", false),
        ecr_login_enabled: env_bool("BUILD_SERVER_ECR_LOGIN_ENABLED", true),
        aws_region: first_env(&["AWS_REGION", "AWS_DEFAULT_REGION"])
            .unwrap_or_else(|| "us-east-1".to_string()),
        job_timeout: Duration::from_secs(env_u64("BUILD_SERVER_JOB_TIMEOUT_SECONDS", 1_800)),
        job_deadline: Duration::from_secs(env_u64("BUILD_SERVER_JOB_DEADLINE_SECONDS", 3_600)),
        max_log_bytes: env_u64("BUILD_SERVER_MAX_LOG_BYTES", 4 * 1024 * 1024),
        max_jobs: env_usize("BUILD_SERVER_MAX_JOBS", 200),
        max_queued: env_usize("BUILD_SERVER_MAX_QUEUED", 32),
        keep_workdirs: env_bool("BUILD_SERVER_KEEP_WORKDIRS", false),
        server_auth_secret: first_env(&["BUILD_SERVER_AUTH_SECRET", "SERVER_AUTH_SECRET"]),

        database_url: first_env(&["BUILD_SERVER_DATABASE_URL", "DATABASE_URL"]),

        fiducia_url,
        fiducia_api_key: first_env(&["FIDUCIA_API_KEY"]),
        coordination_enabled,
        coordination_required: env_bool("BUILD_SERVER_COORDINATION_REQUIRED", false),
        lock_ttl: Duration::from_millis(env_u64("BUILD_SERVER_LOCK_TTL_MS", 3_900_000)),
        lock_wait_budget: Duration::from_millis(env_u64("BUILD_SERVER_LOCK_WAIT_MS", 120_000)),
        lock_retry_interval: Duration::from_millis(env_u64("BUILD_SERVER_LOCK_RETRY_MS", 3_000)),
        idempotency_lease: Duration::from_millis(env_u64(
            "BUILD_SERVER_IDEMPOTENCY_LEASE_MS",
            300_000,
        )),
        idempotency_retention: Duration::from_millis(env_u64(
            "BUILD_SERVER_IDEMPOTENCY_RETENTION_MS",
            7 * 24 * 3_600_000,
        )),

        nats_url: env_value(
            "NATS_URL",
            "nats://dd-nats.messaging.svc.cluster.local:4222",
        ),
        nats_enabled: env_bool("BUILD_SERVER_NATS_ENABLED", true),
        nats_intake_enabled: env_bool("BUILD_SERVER_NATS_INTAKE_ENABLED", false),
        nats_event_subject: env_value(
            "BUILD_SERVER_NATS_EVENT_SUBJECT",
            dd_nats_subject_defs::BUILD_SERVER_EVENTS_SUBJECT,
        ),
        nats_result_subject: env_value(
            "BUILD_SERVER_NATS_RESULT_SUBJECT",
            dd_nats_subject_defs::BUILD_SERVER_RESULTS_SUBJECT,
        ),
        nats_image_subject: env_value(
            "BUILD_SERVER_NATS_IMAGE_SUBJECT",
            dd_nats_subject_defs::BUILD_SERVER_IMAGES_SUBJECT,
        ),
        nats_request_subject: env_value(
            "BUILD_SERVER_NATS_REQUEST_SUBJECT",
            dd_nats_subject_defs::BUILD_SERVER_REQUESTS_SUBJECT,
        ),
        nats_critical_subject: env_value(
            "NATS_CRITICAL_EVENT_SUBJECT",
            dd_nats_subject_defs::RUNTIME_CRITICAL_EVENTS_SUBJECT,
        ),

        github_webhook_secret: first_env(&["BUILD_SERVER_GITHUB_WEBHOOK_SECRET"]),
        registry_webhook_secret: first_env(&["BUILD_SERVER_REGISTRY_WEBHOOK_SECRET"]),
        webhook_rules,

        gh_sync_enabled: env_bool("BUILD_SERVER_GH_SYNC_ENABLED", false),
        gh_sync_token: first_env(&["GH_SECRETS_SYNC_TOKEN", "GH_PAT", "GITHUB_TOKEN"]),
        gh_sync_rules,
        gh_sync_interval: Duration::from_secs(
            first_env(&["BUILD_SERVER_GH_SYNC_INTERVAL_SECONDS"])
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0),
        ),

        lambda_executor_enabled: env_bool("BUILD_SERVER_LAMBDA_ENABLED", false),
        lambda_url: env_value(
            "BUILD_SERVER_LAMBDA_URL",
            "http://dd-gleam-lambda-runner.default.svc.cluster.local:8083",
        ),
        lambda_function_id: first_env(&["BUILD_SERVER_LAMBDA_FUNCTION_ID"]),
        lambda_auth_secret: first_env(&["BUILD_SERVER_LAMBDA_AUTH_SECRET"]),
    }
}
