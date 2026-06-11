//! Process configuration, read once at boot from the environment.
//!
//! Provider API keys are *not* enumerated here — they're read on demand by
//! [`Config::provider_key`] so adding a new provider never requires touching
//! this struct, and an unset key simply means "don't register that provider".

use std::net::SocketAddr;

pub struct Config {
    pub addr: SocketAddr,
    pub log_format_json: bool,
    /// Optional bearer token gating the JSON API. When unset, the API is open
    /// to anything that can reach the listener (an upstream gateway may still
    /// authenticate). When set, callers must send `Authorization: Bearer ...`.
    pub api_auth_bearer: Option<String>,
    pub qdrant_url: String,
    pub qdrant_api_key: Option<String>,
    /// Outbound request timeout to provider APIs and Qdrant.
    pub request_timeout_secs: u64,
    /// Max number of cost-bearing requests (embed/index/search) in flight at
    /// once. Excess requests are shed with 503 rather than queued, so a flood
    /// can't exhaust file descriptors/memory or run up unbounded provider spend.
    pub max_concurrency: usize,
    /// Embedding-cache entry budget (0 disables the cache).
    pub cache_max_entries: usize,
    /// Only texts at or below this byte length are cached.
    pub cache_max_item_bytes: usize,
    pub limits: Limits,
}

/// Request-shape guardrails enforced at the API boundary. These bound memory
/// use, third-party spend, and downstream payload sizes so a single caller
/// can't run up a provider bill or OOM the pod with one giant request.
#[derive(Clone, Copy)]
pub struct Limits {
    /// Max number of texts in one embedding/index request.
    pub max_batch_size: usize,
    /// Max total characters summed across every text in a request.
    pub max_total_chars: usize,
    /// Max characters in any single text.
    pub max_item_chars: usize,
    /// Max neighbors returnable from a RAG search.
    pub max_top_k: usize,
    /// Max requested embedding dimensionality.
    pub max_dimensions: u32,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let host = env_or("EMBEDDINGS_HOST", "0.0.0.0");
        let port: u16 = env_or("EMBEDDINGS_PORT", "8090").parse()?;
        let addr: SocketAddr = format!("{host}:{port}").parse()?;

        Ok(Self {
            addr,
            log_format_json: env_or("EMBEDDINGS_LOG_FORMAT", "json") == "json",
            api_auth_bearer: non_empty(std::env::var("EMBEDDINGS_API_AUTH_BEARER").ok()),
            qdrant_url: env_or("QDRANT_URL", "http://dd-qdrant.ai-ml.svc.cluster.local:6333"),
            qdrant_api_key: non_empty(std::env::var("QDRANT_API_KEY").ok()),
            request_timeout_secs: env_or("EMBEDDINGS_REQUEST_TIMEOUT_SECS", "30").parse()?,
            max_concurrency: env_or("EMBEDDINGS_MAX_CONCURRENCY", "32").parse()?,
            cache_max_entries: env_or("EMBEDDINGS_CACHE_MAX_ENTRIES", "50000").parse()?,
            cache_max_item_bytes: env_or("EMBEDDINGS_CACHE_MAX_ITEM_BYTES", "8192").parse()?,
            limits: Limits {
                max_batch_size: env_or("EMBEDDINGS_MAX_BATCH_SIZE", "256").parse()?,
                max_total_chars: env_or("EMBEDDINGS_MAX_TOTAL_CHARS", "1000000").parse()?,
                max_item_chars: env_or("EMBEDDINGS_MAX_ITEM_CHARS", "100000").parse()?,
                max_top_k: env_or("EMBEDDINGS_MAX_TOP_K", "100").parse()?,
                max_dimensions: env_or("EMBEDDINGS_MAX_DIMENSIONS", "8192").parse()?,
            },
        })
    }

    /// Read a provider API key by env var name, treating blank as absent. This
    /// is what makes the roster opt-in: only providers whose key is populated
    /// in the secret get registered.
    pub fn provider_key(&self, env: &str) -> Option<String> {
        non_empty(std::env::var(env).ok())
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.trim().is_empty())
}
