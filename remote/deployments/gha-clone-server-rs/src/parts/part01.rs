// Signed GitHub `workflow_run` failure bridge.
//
// This service does **not** implement the GitHub Actions execution protocol.
// Official GitHub Actions Runner Controller (ARC) runners preserve workflow
// semantics. The bridge only receives signed failure events and performs one
// operator-reviewed action from a static rule: dispatch a named fallback
// workflow to ARC, or submit an allowlisted profile to `dd-build-server`.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    env,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

const SERVICE_NAME: &str = "gha-clone-server";
const DEFAULT_BIND: &str = "0.0.0.0:8117";
const DEFAULT_BUILD_SERVER_URL: &str = "http://dd-build-server.default.svc.cluster.local:8100";
const DEFAULT_RULES_PATH: &str = "/etc/gha-clone/rules.json";
const MAX_WEBHOOK_BYTES: usize = 1024 * 1024;
const DEFAULT_DELIVERY_CACHE_SIZE: usize = 10_000;
const GITHUB_API_VERSION: &str = "2026-03-10";
const FAILURE_CONCLUSIONS: &[&str] = &[
    "action_required",
    "cancelled",
    "failure",
    "startup_failure",
    "timed_out",
];

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    http: reqwest::Client,
    deliveries: Arc<Mutex<DeliveryCache>>,
    counters: Arc<Counters>,
}

struct Config {
    bind: SocketAddr,
    webhook_secret: String,
    github_token: Option<String>,
    build_server_url: String,
    build_server_auth: Option<String>,
    dry_run: bool,
    rules: Vec<Rule>,
    delivery_cache_size: usize,
}

#[derive(Debug, Default)]
struct Counters {
    received: AtomicU64,
    rejected: AtomicU64,
    ignored: AtomicU64,
    duplicates: AtomicU64,
    dispatched: AtomicU64,
    failed: AtomicU64,
}

#[derive(Debug)]
struct DeliveryCache {
    max_entries: usize,
    seen: HashSet<String>,
    order: VecDeque<String>,
}

impl DeliveryCache {
    fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(1),
            seen: HashSet::new(),
            order: VecDeque::new(),
        }
    }

    fn insert(&mut self, delivery: &str) -> bool {
        if self.seen.contains(delivery) {
            return false;
        }
        self.seen.insert(delivery.to_string());
        self.order.push_back(delivery.to_string());
        while self.order.len() > self.max_entries {
            if let Some(oldest) = self.order.pop_front() {
                self.seen.remove(&oldest);
            }
        }
        true
    }

    fn remove(&mut self, delivery: &str) {
        self.seen.remove(delivery);
        self.order.retain(|value| value != delivery);
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Rule {
    repo: String,
    workflow: String,
    #[serde(default)]
    branches: Vec<String>,
    #[serde(default = "default_source_events")]
    source_events: Vec<String>,
    #[serde(default = "default_conclusions")]
    conclusions: Vec<String>,
    action: RuleAction,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum RuleAction {
    WorkflowDispatch {
        workflow_file: String,
        workflow_name: String,
        dispatch_ref: String,
        runner: String,
        #[serde(default)]
        extra_inputs: HashMap<String, String>,
    },
    BuildServerProfile {
        profile: String,
        #[serde(default)]
        executor: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
struct Repository {
    full_name: String,
}

#[derive(Debug, Deserialize)]
struct WorkflowRunEvent {
    action: String,
    repository: Repository,
    workflow_run: WorkflowRun,
}

#[derive(Debug, Deserialize)]
struct WorkflowRun {
    id: u64,
    name: String,
    event: String,
    head_branch: Option<String>,
    head_sha: String,
    conclusion: Option<String>,
    #[serde(default)]
    run_attempt: u64,
    #[serde(default)]
    head_repository: Option<Repository>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Receipt {
    accepted: bool,
    action: &'static str,
    delivery: String,
    repository: String,
    source_run_id: u64,
    dry_run: bool,
}

fn default_source_events() -> Vec<String> {
    vec!["push".to_string()]
}

fn default_conclusions() -> Vec<String> {
    FAILURE_CONCLUSIONS
        .iter()
        .map(|value| (*value).to_string())
        .collect()
}

impl Config {
    async fn from_env() -> Result<Self, String> {
        let bind = env::var("GHA_CLONE_BIND")
            .unwrap_or_else(|_| DEFAULT_BIND.to_string())
            .parse::<SocketAddr>()
            .map_err(|error| format!("invalid GHA_CLONE_BIND: {error}"))?;
        let webhook_secret = required_env("GHA_CLONE_GITHUB_WEBHOOK_SECRET")?;
        if webhook_secret.as_bytes().len() < 32 {
            return Err("GHA_CLONE_GITHUB_WEBHOOK_SECRET must be at least 32 bytes".to_string());
        }
        let dry_run = parse_bool_env("GHA_CLONE_DRY_RUN", false)?;
        let github_token = optional_env("GHA_CLONE_GITHUB_TOKEN");
        let build_server_auth = optional_env("GHA_CLONE_BUILD_SERVER_AUTH");
        let build_server_url = env::var("GHA_CLONE_BUILD_SERVER_URL")
            .unwrap_or_else(|_| DEFAULT_BUILD_SERVER_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        validate_http_url(&build_server_url, "GHA_CLONE_BUILD_SERVER_URL")?;
        let delivery_cache_size = env::var("GHA_CLONE_DELIVERY_CACHE_SIZE")
            .ok()
            .map(|raw| raw.parse::<usize>())
            .transpose()
            .map_err(|error| format!("invalid GHA_CLONE_DELIVERY_CACHE_SIZE: {error}"))?
            .unwrap_or(DEFAULT_DELIVERY_CACHE_SIZE)
            .clamp(100, 100_000);

        let rules_raw = if let Ok(raw) = env::var("GHA_CLONE_RULES") {
            raw
        } else {
            let path = env::var("GHA_CLONE_RULES_PATH")
                .unwrap_or_else(|_| DEFAULT_RULES_PATH.to_string());
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|error| format!("failed to read rules from {path}: {error}"))?
        };
        let rules = parse_rules(&rules_raw)?;

        if !dry_run {
            for rule in &rules {
                match &rule.action {
                    RuleAction::WorkflowDispatch { .. } if github_token.is_none() => {
                        return Err(
                            "GHA_CLONE_GITHUB_TOKEN is required by workflowDispatch rules"
                                .to_string(),
                        );
                    }
                    RuleAction::BuildServerProfile { .. } if build_server_auth.is_none() => {
                        return Err(
                            "GHA_CLONE_BUILD_SERVER_AUTH is required by buildServerProfile rules"
                                .to_string(),
                        );
                    }
                    _ => {}
                }
            }
        }

        Ok(Self {
            bind,
            webhook_secret,
            github_token,
            build_server_url,
            build_server_auth,
            dry_run,
            rules,
            delivery_cache_size,
        })
    }
}

fn required_env(name: &str) -> Result<String, String> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{name} is required"))
}
