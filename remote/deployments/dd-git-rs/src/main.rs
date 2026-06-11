//! dd-git-rs — a multi-VCS operations server.
//!
//! Speaks git, mercurial (hg), subversion (svn), and fossil. Repositories are
//! registered in Postgres (`vcs_repositories`), mirrored to a local storage
//! volume, and inspected through their native CLIs. Read operations (refs, log,
//! show) are served from the local mirror; privileged operations (register,
//! mirror, sync, remove) require the server-auth header. Ref snapshots are
//! cached in Redis and persisted to `vcs_refs`; every command run is audited in
//! `vcs_operations`.

use std::{
    collections::BTreeMap,
    env,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounterVec, IntGauge, Opts, TextEncoder};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Semaphore};
use uuid::Uuid;

mod vcs;
use vcs::{VcsError, VcsKind};

mod pg_contract {
    pub use dd_pg_defs::{VCS_OPERATIONS_TABLE, VCS_REFS_TABLE, VCS_REPOSITORIES_TABLE};
}

const SERVICE_NAME: &str = "dd-git-rs";
const DEFAULT_REDIS_URL: &str = "redis://dd-redis-cache.default.svc.cluster.local:6379/0";
const DEFAULT_STORAGE_ROOT: &str = "/var/lib/dd-git-rs/repos";
const DEFAULT_PORT: u64 = 8137;
const DEFAULT_MIRROR_TIMEOUT_SECONDS: u64 = 600;
const DEFAULT_READ_TIMEOUT_SECONDS: u64 = 120;
const DEFAULT_PROBE_TIMEOUT_SECONDS: u64 = 10;
const DEFAULT_REFS_CACHE_TTL_SECONDS: u64 = 60;
const DEFAULT_LOG_LIMIT: i64 = 50;
const MAX_LOG_LIMIT: i64 = 1000;
const MAX_LIST_LIMIT: i64 = 500;
const MIRROR_LOCK_TTL_SECONDS: usize = 1800;
const DEFAULT_MAX_CONCURRENT_OPS: usize = 4;
const DEFAULT_MAX_BODY_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_DB_CONNECTIONS: usize = 16;
const DEFAULT_MAX_REPO_BYTES: u64 = 5 * 1024 * 1024 * 1024;

static STARTED_AT: Lazy<Instant> = Lazy::new(Instant::now);
static HTTP_REQUESTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "dd_git_rs_http_requests_total",
            "HTTP requests observed by dd-git-rs.",
        ),
        &["method", "path", "status"],
    )
    .expect("failed to create dd_git_rs_http_requests_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("failed to register dd_git_rs_http_requests_total");
    counter
});
static UPTIME_SECONDS: Lazy<IntGauge> = Lazy::new(|| {
    let gauge = IntGauge::new(
        "dd_git_rs_uptime_seconds",
        "dd-git-rs process uptime in seconds.",
    )
    .expect("failed to create dd_git_rs_uptime_seconds");
    prometheus::default_registry()
        .register(Box::new(gauge.clone()))
        .expect("failed to register dd_git_rs_uptime_seconds");
    gauge
});
static VCS_OPERATIONS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "dd_git_rs_vcs_operations_total",
            "VCS operations executed by dd-git-rs.",
        ),
        &["kind", "op", "result"],
    )
    .expect("failed to create dd_git_rs_vcs_operations_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("failed to register dd_git_rs_vcs_operations_total");
    counter
});

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    redis: Option<redis::Client>,
    redis_connection: Arc<Mutex<Option<redis::aio::MultiplexedConnection>>>,
    nats: Option<async_nats::Client>,
    vcs_available: Arc<BTreeMap<&'static str, VcsAvailability>>,
    // Hard cap on concurrently-running VCS subprocesses. Read endpoints are
    // gateway-fronted but otherwise unauthenticated, so this bounds the blast
    // radius of a request flood (no unbounded process fan-out).
    ops_semaphore: Arc<Semaphore>,
    // Hard cap on concurrent Postgres connections. The service connects per
    // request; this stops an endpoint flood from exhausting RDS connections
    // (a cluster-shared resource).
    db_semaphore: Arc<Semaphore>,
}

#[derive(Clone)]
struct Config {
    database_url: Option<String>,
    server_auth_secret: Option<String>,
    allow_unauthenticated: bool,
    storage_root: String,
    redis_prefix: String,
    mirror_timeout: Duration,
    read_timeout: Duration,
    refs_cache_ttl: u64,
    max_output_bytes: usize,
    max_body_bytes: usize,
    max_concurrent_ops: usize,
    max_db_connections: usize,
    max_repo_bytes: u64,
    allow_file_urls: bool,
    block_private_remotes: bool,
}

#[derive(Clone, Serialize)]
struct VcsAvailability {
    kind: &'static str,
    label: &'static str,
    binary: &'static str,
    available: bool,
    version: Option<String>,
}

#[derive(Debug)]
enum ServiceError {
    BadRequest(String),
    Unauthorized,
    NotFound(String),
    Conflict(String),
    Unavailable(String),
    Internal(String),
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ServiceError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            ServiceError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
            ServiceError::NotFound(message) => (StatusCode::NOT_FOUND, message),
            ServiceError::Conflict(message) => (StatusCode::CONFLICT, message),
            ServiceError::Unavailable(message) => (StatusCode::SERVICE_UNAVAILABLE, message),
            ServiceError::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, Json(json!({ "ok": false, "error": message }))).into_response()
    }
}

// ---------------------------------------------------------------------------
// Env + config
// ---------------------------------------------------------------------------

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| match env::var(key) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    })
}

fn env_bool(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

fn config_from_env() -> Config {
    Config {
        database_url: first_env(&[
            "DD_GIT_RDS_DATABASE_URL",
            "RDS_DATABASE_URL",
            "AGENT_TASKS_RDS_DATABASE_URL",
            "DATABASE_URL",
        ]),
        server_auth_secret: first_env(&["GIT_RS_SERVER_AUTH_SECRET", "SERVER_AUTH_SECRET"]),
        allow_unauthenticated: env_bool("GIT_RS_ALLOW_UNAUTHENTICATED", false),
        storage_root: first_env(&["GIT_RS_STORAGE_ROOT"])
            .unwrap_or_else(|| DEFAULT_STORAGE_ROOT.to_string()),
        redis_prefix: first_env(&["GIT_RS_REDIS_PREFIX"]).unwrap_or_else(|| "dd:git".to_string()),
        mirror_timeout: Duration::from_secs(env_u64(
            "GIT_RS_MIRROR_TIMEOUT_SECONDS",
            DEFAULT_MIRROR_TIMEOUT_SECONDS,
        )),
        read_timeout: Duration::from_secs(env_u64(
            "GIT_RS_READ_TIMEOUT_SECONDS",
            DEFAULT_READ_TIMEOUT_SECONDS,
        )),
        refs_cache_ttl: env_u64("GIT_RS_REFS_CACHE_TTL_SECONDS", DEFAULT_REFS_CACHE_TTL_SECONDS),
        max_output_bytes: env_u64(
            "GIT_RS_MAX_OUTPUT_BYTES",
            vcs::DEFAULT_MAX_OUTPUT_BYTES as u64,
        ) as usize,
        max_body_bytes: env_u64("GIT_RS_MAX_BODY_BYTES", DEFAULT_MAX_BODY_BYTES as u64) as usize,
        max_concurrent_ops: (env_u64(
            "GIT_RS_MAX_CONCURRENT_OPS",
            DEFAULT_MAX_CONCURRENT_OPS as u64,
        ) as usize)
            .max(1),
        max_db_connections: (env_u64(
            "GIT_RS_MAX_DB_CONNECTIONS",
            DEFAULT_MAX_DB_CONNECTIONS as u64,
        ) as usize)
            .max(1),
        max_repo_bytes: env_u64("GIT_RS_MAX_REPO_BYTES", DEFAULT_MAX_REPO_BYTES),
        // Local-file URLs (`file://`) and remotes that resolve to private
        // networks are off by default: both are SSRF / file-disclosure vectors.
        allow_file_urls: env_bool("GIT_RS_ALLOW_FILE_URLS", false),
        block_private_remotes: env_bool("GIT_RS_BLOCK_PRIVATE_REMOTES", false),
    }
}

async fn state_from_config(config: Config) -> AppState {
    let redis = first_env(&["GIT_RS_REDIS_URL", "REDIS_URL"])
        .or_else(|| Some(DEFAULT_REDIS_URL.to_string()))
        .and_then(|url| redis::Client::open(url).ok());

    let nats = match first_env(&["GIT_RS_NATS_URL", "NATS_URL"]) {
        Some(url) => match async_nats::connect(&url).await {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::warn!(%url, %error, "nats connect failed");
                None
            }
        },
        None => None,
    };

    let vcs_available = probe_vcs_kinds(Duration::from_secs(DEFAULT_PROBE_TIMEOUT_SECONDS)).await;
    let ops_semaphore = Arc::new(Semaphore::new(config.max_concurrent_ops));
    let db_semaphore = Arc::new(Semaphore::new(config.max_db_connections));

    AppState {
        config: Arc::new(config),
        redis,
        redis_connection: Arc::new(Mutex::new(None)),
        nats,
        vcs_available: Arc::new(vcs_available),
        ops_semaphore,
        db_semaphore,
    }
}

async fn probe_vcs_kinds(timeout: Duration) -> BTreeMap<&'static str, VcsAvailability> {
    let mut map = BTreeMap::new();
    for kind in VcsKind::ALL {
        let args = vcs::probe_args(kind);
        let result = vcs::run(
            kind.binary(),
            &args,
            None,
            &BTreeMap::new(),
            timeout,
            64 * 1024,
        )
        .await;
        let (available, version) = match result {
            Ok(output) if output.success => {
                let line = output
                    .stdout
                    .lines()
                    .next()
                    .or_else(|| output.stderr.lines().next())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                (true, if line.is_empty() { None } else { Some(line) })
            }
            _ => (false, None),
        };
        map.insert(
            kind.as_str(),
            VcsAvailability {
                kind: kind.as_str(),
                label: kind.label(),
                binary: kind.binary(),
                available,
                version,
            },
        );
    }
    map
}

impl AppState {
    async fn redis_connection(&self) -> Result<redis::aio::MultiplexedConnection, ServiceError> {
        let Some(client) = &self.redis else {
            return Err(ServiceError::Unavailable("redis is not configured".to_string()));
        };
        let mut guard = self.redis_connection.lock().await;
        if let Some(connection) = guard.as_ref() {
            return Ok(connection.clone());
        }
        let connection = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "redis connection failed");
                ServiceError::Unavailable("redis connection failed".to_string())
            })?;
        *guard = Some(connection.clone());
        Ok(connection)
    }

    fn vcs_binary_available(&self, kind: VcsKind) -> bool {
        self.vcs_available
            .get(kind.as_str())
            .map(|entry| entry.available)
            .unwrap_or(false)
    }
}

fn record_request(method: &str, path: &str, status: StatusCode) {
    HTTP_REQUESTS
        .with_label_values(&[method, path, status.as_str()])
        .inc();
}

/// A live Postgres client paired with the semaphore permit that bounds total
/// concurrent connections. Derefs to the client, so call sites and helpers that
/// expect `&tokio_postgres::Client` keep working via deref coercion. The permit
/// (and thus the slot) is released when this guard drops.
struct PooledClient {
    client: tokio_postgres::Client,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl std::ops::Deref for PooledClient {
    type Target = tokio_postgres::Client;
    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

async fn connect_postgres(state: &AppState) -> Result<PooledClient, ServiceError> {
    let database_url = state
        .config
        .database_url
        .as_deref()
        .ok_or_else(|| ServiceError::Unavailable("postgres is not configured".to_string()))?;
    // Reserve a connection slot before dialing, shedding with 503 when the cap
    // is reached so a flood can't exhaust RDS connections.
    let permit = Arc::clone(&state.db_semaphore)
        .try_acquire_owned()
        .map_err(|_| {
            ServiceError::Unavailable("server is busy: too many database connections".to_string())
        })?;
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
    let (client, connection) = tokio_postgres::connect(database_url, tls)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "postgres connect failed");
            ServiceError::Unavailable("postgres connect failed".to_string())
        })?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::warn!(%error, "postgres connection closed");
        }
    });
    Ok(PooledClient {
        client,
        _permit: permit,
    })
}

fn db_error(error: tokio_postgres::Error) -> ServiceError {
    tracing::warn!(%error, "postgres query failed");
    ServiceError::Internal("postgres query failed".to_string())
}

fn require_server_auth(headers: &HeaderMap, config: &Config) -> Result<(), ServiceError> {
    if config.allow_unauthenticated {
        return Ok(());
    }
    let expected = config
        .server_auth_secret
        .as_deref()
        .ok_or(ServiceError::Unauthorized)?;
    let presented = headers
        .get("X-Server-Auth")
        .or_else(|| headers.get("Auth"))
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if constant_time_eq(presented, expected) {
        Ok(())
    } else {
        Err(ServiceError::Unauthorized)
    }
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// True when the caller presents a valid server-auth header (or auth is off).
fn is_authenticated(headers: &HeaderMap, config: &Config) -> bool {
    require_server_auth(headers, config).is_ok()
}

/// Read-side authorization (defense in depth on top of the gateway): `public`
/// repositories are readable by anyone; `private`/`internal` ones require the
/// server-auth header.
fn authorize_read(
    headers: &HeaderMap,
    config: &Config,
    repo: &RepoRow,
) -> Result<(), ServiceError> {
    if repo.visibility == "public" {
        Ok(())
    } else {
        require_server_auth(headers, config)
    }
}

/// Reserve one of the bounded VCS-subprocess slots, or shed load with 503. The
/// permit is released when the returned guard drops.
fn acquire_op_permit(state: &AppState) -> Result<tokio::sync::OwnedSemaphorePermit, ServiceError> {
    Arc::clone(&state.ops_semaphore)
        .try_acquire_owned()
        .map_err(|_| {
            ServiceError::Unavailable(
                "server is busy: too many concurrent VCS operations".to_string(),
            )
        })
}

// ---------------------------------------------------------------------------
// Validators
// ---------------------------------------------------------------------------

fn validate_slug(slug: &str) -> Result<(), ServiceError> {
    let bytes = slug.as_bytes();
    if bytes.is_empty() || bytes.len() > 120 {
        return Err(ServiceError::BadRequest(
            "slug must be 1-120 chars".to_string(),
        ));
    }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(ServiceError::BadRequest(
            "slug must start with a lowercase letter or digit".to_string(),
        ));
    }
    for &c in bytes {
        let ok = c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-');
        if !ok {
            return Err(ServiceError::BadRequest(
                "slug allows [a-z0-9._-] only".to_string(),
            ));
        }
    }
    Ok(())
}

/// Validate a ref/revision. Rejects option-injection (leading '-'), whitespace,
/// and control characters; permits the punctuation real refs/revisions use.
fn validate_revision(rev: &str) -> Result<(), ServiceError> {
    if rev.is_empty() || rev.len() > 120 {
        return Err(ServiceError::BadRequest(
            "revision must be 1-120 chars".to_string(),
        ));
    }
    if rev.starts_with('-') {
        return Err(ServiceError::BadRequest(
            "revision must not start with '-'".to_string(),
        ));
    }
    for c in rev.chars() {
        let ok = c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '@' | ':' | '+' | '-');
        if !ok {
            return Err(ServiceError::BadRequest(
                "revision contains unsupported characters".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_remote_url(kind: VcsKind, url: &str, config: &Config) -> Result<(), ServiceError> {
    if url.is_empty() || url.len() > 2048 {
        return Err(ServiceError::BadRequest(
            "remoteUrl must be 1-2048 chars".to_string(),
        ));
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ServiceError::BadRequest(
            "remoteUrl must not contain whitespace or control characters".to_string(),
        ));
    }
    let allowed = kind.allowed_url_prefixes();
    let scheme_ok = allowed.iter().any(|prefix| url.starts_with(prefix))
        || (config.allow_file_urls && url.starts_with("file://"));
    if !scheme_ok {
        return Err(ServiceError::BadRequest(format!(
            "remoteUrl for {} must start with one of: {}{}",
            kind.as_str(),
            allowed.join(", "),
            if config.allow_file_urls { ", file://" } else { "" }
        )));
    }
    // SSRF guard: reject remotes that target the loopback/link-local ranges or
    // the cloud metadata endpoint, and (optionally) any private network.
    if let Some(host) = extract_host(url) {
        if host_is_blocked(&host, config.block_private_remotes) {
            return Err(ServiceError::BadRequest(
                "remoteUrl host is not permitted (loopback, link-local, metadata, or blocked private network)".to_string(),
            ));
        }
    }
    Ok(())
}

/// Best-effort host extraction covering `scheme://[user@]host[:port]/...` and
/// scp-style `user@host:path`. Returns a lowercased host with any IPv6 brackets
/// stripped.
fn extract_host(url: &str) -> Option<String> {
    let authority = if let Some(rest) = url.split("://").nth(1) {
        // scheme://[userinfo@]host[:port]/...
        rest.split(['/', '?', '#']).next().unwrap_or("")
    } else if let Some((before_path, _)) = url.split_once(':') {
        // scp-style git@host:path — authority is everything before the first ':'
        before_path
    } else {
        return None;
    };
    let after_userinfo = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    let host = if let Some(stripped) = after_userinfo.strip_prefix('[') {
        // [ipv6]:port
        stripped.split(']').next().unwrap_or("")
    } else {
        after_userinfo.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

fn host_is_blocked(host: &str, block_private: bool) -> bool {
    if matches!(host, "localhost" | "ip6-localhost" | "ip6-loopback") {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return ip_is_blocked(&ip, block_private);
    }
    // Not a standard dotted/colon IP literal. Reject alternate numeric encodings
    // (decimal, hex, octal) that libcurl/git still resolve to an address but that
    // sail past a textual-IP blocklist — e.g. 2130706433 or 0x7f000001 == 127.0.0.1.
    looks_like_numeric_host(host)
}

fn ip_is_blocked(ip: &std::net::IpAddr, block_private: bool) -> bool {
    // Always block loopback, link-local (incl. the 169.254.169.254 metadata
    // endpoint), and unspecified addresses.
    if ip.is_loopback() || ip.is_unspecified() {
        return true;
    }
    match ip {
        std::net::IpAddr::V4(v4) => {
            if v4.is_link_local() {
                return true;
            }
            if block_private && v4.is_private() {
                return true;
            }
        }
        std::net::IpAddr::V6(v6) => {
            // fe80::/10 link-local.
            if (v6.segments()[0] & 0xffc0) == 0xfe80 {
                return true;
            }
            // fc00::/7 unique-local treated as private.
            if block_private && (v6.segments()[0] & 0xfe00) == 0xfc00 {
                return true;
            }
        }
    }
    false
}

/// True when a host string is an alternate numeric IP encoding rather than a DNS
/// name: a hex form (`0x..` in any label) or an all-numeric form (a bare integer
/// like `2130706433`, or dotted forms like `127.1` / `010.0.0.1`). Real domain
/// names always carry at least one non-numeric label.
fn looks_like_numeric_host(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    if host.is_empty() {
        return false;
    }
    let mut all_numeric = true;
    for label in host.split('.') {
        if label.is_empty() {
            return false;
        }
        if label.to_ascii_lowercase().starts_with("0x") {
            return true;
        }
        if !label.bytes().all(|b| b.is_ascii_digit()) {
            all_numeric = false;
        }
    }
    all_numeric
}

/// Strip `user:pass@` userinfo from a URL so credentials never land in logs,
/// audit rows, or client responses.
fn redact_url(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        if let Some((authority, tail)) = rest.split_once('/') {
            if let Some((_, host)) = authority.rsplit_once('@') {
                return format!("{scheme}://{host}/{tail}");
            }
        } else if let Some((_, host)) = rest.rsplit_once('@') {
            return format!("{scheme}://{host}");
        }
    }
    url.to_string()
}

/// Scrub a free-text message (typically VCS stderr) before it reaches a client
/// or an audit row: drop the storage root and any embedded URL credentials.
fn sanitize_message(message: &str, storage_root: &str) -> String {
    let stripped = if storage_root.is_empty() {
        message.to_string()
    } else {
        message.replace(storage_root, "<storage>")
    };
    // Redact `scheme://user:pass@host` credentials token by token so an already
    // redacted token is never re-scanned.
    let cleaned: Vec<String> = stripped
        .split_whitespace()
        .map(|token| {
            if token.contains("://") && token.contains('@') {
                redact_url(token)
            } else {
                token.to_string()
            }
        })
        .collect();
    cleaned.join(" ").chars().take(2000).collect()
}

fn sanitize_display_name(name: &str) -> Result<String, ServiceError> {
    let cleaned: String = name.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() || cleaned.len() > 200 {
        return Err(ServiceError::BadRequest(
            "displayName must be 1-200 chars".to_string(),
        ));
    }
    Ok(cleaned)
}

fn validate_branch(branch: &str) -> Result<(), ServiceError> {
    if branch.is_empty() || branch.len() > 160 {
        return Err(ServiceError::BadRequest(
            "defaultBranch must be 1-160 chars".to_string(),
        ));
    }
    for c in branch.chars() {
        let ok = c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-');
        if !ok {
            return Err(ServiceError::BadRequest(
                "defaultBranch allows [A-Za-z0-9._/-] only".to_string(),
            ));
        }
    }
    Ok(())
}

/// Server-owned on-disk mirror path. Derived only from validated slug + kind, so
/// it can never escape the storage root.
fn repo_storage_path(config: &Config, kind: VcsKind, slug: &str) -> String {
    let base = format!("{}/{}/{}", config.storage_root.trim_end_matches('/'), kind.as_str(), slug);
    if kind.mirror_is_file() {
        format!("{base}.fossil")
    } else {
        base
    }
}

// ---------------------------------------------------------------------------
// Repository row
// ---------------------------------------------------------------------------

// `mirror_path` is intentionally write-only (set on sync for operator DB
// inspection) and never selected back into responses — it is an absolute
// server-internal path.
const REPO_COLUMNS: &str = "id, slug, display_name, vcs_kind, remote_url, default_branch, \
    mirror_status, visibility, last_synced_at, last_error, size_bytes, ref_count, \
    meta_data, is_soft_deleted, created_at, updated_at";

struct RepoRow {
    id: Uuid,
    slug: String,
    display_name: String,
    vcs_kind: String,
    remote_url: String,
    default_branch: String,
    mirror_status: String,
    visibility: String,
    last_synced_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    size_bytes: i64,
    ref_count: i32,
    meta_data: Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl RepoRow {
    fn from_row(row: &tokio_postgres::Row) -> RepoRow {
        RepoRow {
            id: row.get("id"),
            slug: row.get("slug"),
            display_name: row.get("display_name"),
            vcs_kind: row.get("vcs_kind"),
            remote_url: row.get("remote_url"),
            default_branch: row.get("default_branch"),
            mirror_status: row.get("mirror_status"),
            visibility: row.get("visibility"),
            last_synced_at: row.get("last_synced_at"),
            last_error: row.get("last_error"),
            size_bytes: row.get("size_bytes"),
            ref_count: row.get("ref_count"),
            meta_data: row.get("meta_data"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        }
    }

    fn kind(&self) -> Option<VcsKind> {
        VcsKind::parse(&self.vcs_kind)
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "slug": self.slug,
            "displayName": self.display_name,
            "vcsKind": self.vcs_kind,
            "remoteUrl": redact_url(&self.remote_url),
            "defaultBranch": self.default_branch,
            "mirrorStatus": self.mirror_status,
            "visibility": self.visibility,
            "lastSyncedAt": self.last_synced_at.map(|t| t.to_rfc3339()),
            "lastError": self.last_error,
            "sizeBytes": self.size_bytes,
            "refCount": self.ref_count,
            "metaData": self.meta_data,
            "createdAt": self.created_at.to_rfc3339(),
            "updatedAt": self.updated_at.to_rfc3339(),
        })
    }
}

async fn fetch_repo(
    client: &tokio_postgres::Client,
    id: Uuid,
) -> Result<Option<RepoRow>, ServiceError> {
    let sql = format!(
        "select {REPO_COLUMNS} from {} where id = $1 and is_soft_deleted = false",
        pg_contract::VCS_REPOSITORIES_TABLE
    );
    let row = client.query_opt(&sql, &[&id]).await.map_err(db_error)?;
    Ok(row.map(|row| RepoRow::from_row(&row)))
}

// ---------------------------------------------------------------------------
// Operation auditing
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn record_operation(
    state: &AppState,
    repository_id: Option<Uuid>,
    kind: VcsKind,
    op_type: &str,
    status: &str,
    params: Value,
    result_summary: Value,
    error: Option<&str>,
    duration_ms: Option<i32>,
    requested_by: Option<&str>,
) {
    VCS_OPERATIONS
        .with_label_values(&[kind.as_str(), op_type, status])
        .inc();
    let client = match connect_postgres(state).await {
        Ok(client) => client,
        Err(_) => return,
    };
    let sql = format!(
        "insert into {} (repository_id, vcs_kind, op_type, status, params, result_summary, error, duration_ms, requested_by) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        pg_contract::VCS_OPERATIONS_TABLE
    );
    if let Err(error) = client
        .execute(
            &sql,
            &[
                &repository_id,
                &kind.as_str(),
                &op_type,
                &status,
                &params,
                &result_summary,
                &error,
                &duration_ms,
                &requested_by,
            ],
        )
        .await
    {
        tracing::warn!(%error, "failed to record vcs operation");
    }
}

async fn publish_critical_event(state: &AppState, payload: Value) {
    let Some(nats) = &state.nats else { return };
    let subject = first_env(&["NATS_CRITICAL_EVENT_SUBJECT"])
        .unwrap_or_else(|| "dd.remote.events.critical".to_string());
    if let Ok(bytes) = serde_json::to_vec(&payload) {
        let _ = nats.publish(subject, bytes.into()).await;
    }
}

// ---------------------------------------------------------------------------
// HTTP handlers — service surface
// ---------------------------------------------------------------------------

async fn home() -> Html<&'static str> {
    record_request("GET", "/", StatusCode::OK);
    Html(HOME_HTML)
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    record_request("GET", "/healthz", StatusCode::OK);
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "postgresConfigured": state.config.database_url.is_some(),
        "redisConfigured": state.redis.is_some(),
        "storageRoot": state.config.storage_root,
    }))
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let storage_writable = storage_writable(&state.config.storage_root).await;
    let internal_auth_ready =
        state.config.allow_unauthenticated || state.config.server_auth_secret.is_some();
    let git_available = state.vcs_binary_available(VcsKind::Git);
    let ready = state.config.database_url.is_some()
        && storage_writable
        && git_available
        && internal_auth_ready;
    let status = StatusCode::OK;
    record_request("GET", "/readyz", status);
    (
        status,
        Json(json!({
            "ok": true,
            "ready": ready,
            "degraded": !ready,
            "postgresConfigured": state.config.database_url.is_some(),
            "storageWritable": storage_writable,
            "gitAvailable": git_available,
            "internalAuthReady": internal_auth_ready,
        })),
    )
}

async fn metrics() -> impl IntoResponse {
    record_request("GET", "/metrics", StatusCode::OK);
    UPTIME_SECONDS.set(STARTED_AT.elapsed().as_secs() as i64);
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut body = Vec::new();
    encoder
        .encode(&metric_families, &mut body)
        .expect("failed to encode prometheus metrics");
    (
        [(header::CONTENT_TYPE, encoder.format_type().to_string())],
        body,
    )
}

async fn api_docs_html() -> Html<&'static str> {
    record_request("GET", "/docs/api", StatusCode::OK);
    Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl IntoResponse {
    record_request("GET", "/api/docs.json", StatusCode::OK);
    (
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

async fn list_kinds(State(state): State<AppState>) -> impl IntoResponse {
    record_request("GET", "/api/v1/vcs/kinds", StatusCode::OK);
    let kinds: Vec<&VcsAvailability> = state.vcs_available.values().collect();
    Json(json!({ "ok": true, "kinds": kinds }))
}

// ---------------------------------------------------------------------------
// HTTP handlers — repository registry
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<i64>,
    kind: Option<String>,
}

async fn list_repos(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, ServiceError> {
    let limit = query.limit.unwrap_or(100).clamp(1, MAX_LIST_LIMIT);
    // Unauthenticated callers only see public repositories.
    let public_only = !is_authenticated(&headers, &state.config);
    let visibility_filter = if public_only {
        " and visibility = 'public'"
    } else {
        ""
    };
    let client = connect_postgres(&state).await?;
    let rows = if let Some(kind) = query.kind.as_deref() {
        let kind = VcsKind::parse(kind)
            .ok_or_else(|| ServiceError::BadRequest("unknown vcs kind".to_string()))?;
        let sql = format!(
            "select {REPO_COLUMNS} from {} where is_soft_deleted = false and vcs_kind = $1{} \
             order by updated_at desc limit $2",
            pg_contract::VCS_REPOSITORIES_TABLE,
            visibility_filter
        );
        client
            .query(&sql, &[&kind.as_str(), &limit])
            .await
            .map_err(db_error)?
    } else {
        let sql = format!(
            "select {REPO_COLUMNS} from {} where is_soft_deleted = false{} \
             order by updated_at desc limit $1",
            pg_contract::VCS_REPOSITORIES_TABLE,
            visibility_filter
        );
        client.query(&sql, &[&limit]).await.map_err(db_error)?
    };
    let repos: Vec<Value> = rows.iter().map(|row| RepoRow::from_row(row).to_json()).collect();
    record_request("GET", "/api/v1/repos", StatusCode::OK);
    Ok(Json(json!({ "ok": true, "count": repos.len(), "repos": repos })))
}

#[derive(Deserialize)]
struct CreateRepoRequest {
    slug: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "vcsKind")]
    vcs_kind: String,
    #[serde(rename = "remoteUrl")]
    remote_url: String,
    #[serde(rename = "defaultBranch")]
    default_branch: Option<String>,
    visibility: Option<String>,
}

async fn create_repo(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateRepoRequest>,
) -> Result<(StatusCode, Json<Value>), ServiceError> {
    require_server_auth(&headers, &state.config)?;

    let kind = VcsKind::parse(&body.vcs_kind)
        .ok_or_else(|| ServiceError::BadRequest("unknown vcsKind".to_string()))?;
    validate_slug(&body.slug)?;
    validate_remote_url(kind, &body.remote_url, &state.config)?;
    let display_name = match &body.display_name {
        Some(name) => sanitize_display_name(name)?,
        None => body.slug.clone(),
    };
    let default_branch = match &body.default_branch {
        Some(branch) => {
            validate_branch(branch)?;
            branch.clone()
        }
        None => default_branch_for(kind).to_string(),
    };
    let visibility = match body.visibility.as_deref() {
        None | Some("private") => "private",
        Some("internal") => "internal",
        Some("public") => "public",
        Some(_) => {
            return Err(ServiceError::BadRequest(
                "visibility must be private, internal, or public".to_string(),
            ))
        }
    };

    let client = connect_postgres(&state).await?;
    let sql = format!(
        "insert into {} (slug, display_name, vcs_kind, remote_url, default_branch, visibility) \
         values ($1, $2, $3, $4, $5, $6) returning {REPO_COLUMNS}",
        pg_contract::VCS_REPOSITORIES_TABLE
    );
    let row = client
        .query_one(
            &sql,
            &[
                &body.slug,
                &display_name,
                &kind.as_str(),
                &body.remote_url,
                &default_branch,
                &visibility,
            ],
        )
        .await
        .map_err(|error| {
            if let Some(db) = error.as_db_error() {
                if db.code() == &tokio_postgres::error::SqlState::UNIQUE_VIOLATION {
                    return ServiceError::Conflict(format!("slug '{}' already exists", body.slug));
                }
            }
            db_error(error)
        })?;
    record_request("POST", "/api/v1/repos", StatusCode::CREATED);
    Ok((StatusCode::CREATED, Json(json!({ "ok": true, "repo": RepoRow::from_row(&row).to_json() }))))
}

fn default_branch_for(kind: VcsKind) -> &'static str {
    match kind {
        VcsKind::Git => "main",
        VcsKind::Hg => "default",
        VcsKind::Svn => "trunk",
        VcsKind::Fossil => "trunk",
    }
}

async fn get_repo(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ServiceError> {
    let id = parse_uuid(&id)?;
    let client = connect_postgres(&state).await?;
    let repo = fetch_repo(&client, id)
        .await?
        .ok_or_else(|| ServiceError::NotFound("repository not found".to_string()))?;
    authorize_read(&headers, &state.config, &repo)?;
    record_request("GET", "/api/v1/repos/:id", StatusCode::OK);
    Ok(Json(json!({ "ok": true, "repo": repo.to_json() })))
}

async fn delete_repo(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ServiceError> {
    require_server_auth(&headers, &state.config)?;
    let id = parse_uuid(&id)?;
    let client = connect_postgres(&state).await?;
    let sql = format!(
        "update {} set is_soft_deleted = true, mirror_status = 'disabled', updated_at = now() \
         where id = $1 and is_soft_deleted = false",
        pg_contract::VCS_REPOSITORIES_TABLE
    );
    let affected = client.execute(&sql, &[&id]).await.map_err(db_error)?;
    if affected == 0 {
        return Err(ServiceError::NotFound("repository not found".to_string()));
    }
    record_request("DELETE", "/api/v1/repos/:id", StatusCode::OK);
    Ok(Json(json!({ "ok": true, "deleted": id.to_string() })))
}

// ---------------------------------------------------------------------------
// HTTP handlers — VCS operations
// ---------------------------------------------------------------------------

async fn sync_repo(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ServiceError> {
    require_server_auth(&headers, &state.config)?;
    let id = parse_uuid(&id)?;
    let requested_by = header_value(&headers, "X-Requested-By");

    let client = connect_postgres(&state).await?;
    let repo = fetch_repo(&client, id)
        .await?
        .ok_or_else(|| ServiceError::NotFound("repository not found".to_string()))?;
    let kind = repo
        .kind()
        .ok_or_else(|| ServiceError::Internal("repository has unknown vcs kind".to_string()))?;
    if !state.vcs_binary_available(kind) {
        return Err(ServiceError::Unavailable(format!(
            "{} binary is not installed",
            kind.binary()
        )));
    }

    let dest = repo_storage_path(&state.config, kind, &repo.slug);
    let lock_key = format!("{}:lock:mirror:{}", state.config.redis_prefix, id);
    let lock_token = random_token();
    if !acquire_lock(&state, &lock_key, &lock_token).await? {
        return Err(ServiceError::Conflict(
            "a sync is already in progress for this repository".to_string(),
        ));
    }

    let outcome = run_sync(&state, &client, &repo, kind, &dest).await;
    release_lock(&state, &lock_key, &lock_token).await;

    let (op_type, result) = outcome?;
    record_request("POST", "/api/v1/repos/:id/sync", StatusCode::OK);
    let _ = requested_by; // recorded inside run_sync's operation row
    Ok(Json(json!({
        "ok": true,
        "operation": op_type,
        "repo": result,
    })))
}

/// Perform the mirror-or-fetch, refresh refs, and persist status. Returns the
/// op type that ran ("mirror" or "fetch") plus the updated repo JSON.
async fn run_sync(
    state: &AppState,
    client: &tokio_postgres::Client,
    repo: &RepoRow,
    kind: VcsKind,
    dest: &str,
) -> Result<(&'static str, Value), ServiceError> {
    let already_mirrored = tokio::fs::metadata(dest).await.is_ok();
    let op_type = if already_mirrored { "fetch" } else { "mirror" };

    // Ensure parent directory exists for fresh mirrors.
    if let Some(parent) = std::path::Path::new(dest).parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            ServiceError::Internal(format!("failed to prepare storage dir: {error}"))
        })?;
    }

    set_mirror_status(client, repo.id, "mirroring", None).await?;

    let args = if already_mirrored {
        vcs::fetch_args(kind, dest)
    } else {
        vcs::mirror_args(kind, &repo.remote_url, dest)
    };
    // Audit params carry no credentials or raw server paths.
    let safe_params = json!({
        "dest": sanitize_message(dest, &state.config.storage_root),
        "remoteUrl": redact_url(&repo.remote_url),
    });
    let started = Instant::now();
    let result = {
        let _permit = acquire_op_permit(state)?;
        vcs::run(
            kind.binary(),
            &args,
            None,
            &BTreeMap::new(),
            state.config.mirror_timeout,
            state.config.max_output_bytes,
        )
        .await
        .and_then(vcs::require_success)
    };
    let duration_ms = started.elapsed().as_millis() as i32;

    match result {
        Ok(_) => {
            let size_bytes = directory_size(dest).await;
            // Disk guard: a mirror that overflows the per-repo budget is removed
            // and marked failed so one repo can't fill the node's storage volume.
            if state.config.max_repo_bytes > 0 && size_bytes as u64 > state.config.max_repo_bytes {
                remove_mirror(dest).await;
                let message = format!(
                    "mirror exceeds size limit ({} bytes > {} bytes); removed",
                    size_bytes, state.config.max_repo_bytes
                );
                set_mirror_status(client, repo.id, "error", Some(&message)).await?;
                record_operation(
                    state,
                    Some(repo.id),
                    kind,
                    op_type,
                    "error",
                    safe_params,
                    json!({ "sizeBytes": size_bytes }),
                    Some(&message),
                    Some(duration_ms),
                    None,
                )
                .await;
                return Err(ServiceError::BadRequest(message));
            }
            let ref_count = refresh_refs(state, client, repo, kind, dest).await.unwrap_or(0);
            update_after_sync(client, repo.id, dest, size_bytes, ref_count).await?;
            record_operation(
                state,
                Some(repo.id),
                kind,
                op_type,
                "success",
                safe_params,
                json!({ "sizeBytes": size_bytes, "refCount": ref_count }),
                None,
                Some(duration_ms),
                None,
            )
            .await;
            let updated = fetch_repo(client, repo.id)
                .await?
                .map(|r| r.to_json())
                .unwrap_or(Value::Null);
            Ok((op_type, updated))
        }
        Err(error) => {
            // Scrub credentials and the storage root out of the VCS error text
            // before it reaches the DB, NATS, or the HTTP client.
            let message = sanitize_message(&error.to_string(), &state.config.storage_root);
            set_mirror_status(client, repo.id, "error", Some(&message)).await?;
            record_operation(
                state,
                Some(repo.id),
                kind,
                op_type,
                "error",
                safe_params,
                json!({}),
                Some(&message),
                Some(duration_ms),
                None,
            )
            .await;
            publish_critical_event(
                state,
                json!({
                    "service": SERVICE_NAME,
                    "event": "vcs_sync_failed",
                    "repositoryId": repo.id.to_string(),
                    "vcsKind": kind.as_str(),
                    "op": op_type,
                    "error": message,
                }),
            )
            .await;
            Err(ServiceError::Internal(format!("{op_type} failed: {message}")))
        }
    }
}

async fn set_mirror_status(
    client: &tokio_postgres::Client,
    id: Uuid,
    status: &str,
    error: Option<&str>,
) -> Result<(), ServiceError> {
    let sql = format!(
        "update {} set mirror_status = $2, last_error = $3, updated_at = now() where id = $1",
        pg_contract::VCS_REPOSITORIES_TABLE
    );
    client.execute(&sql, &[&id, &status, &error]).await.map_err(db_error)?;
    Ok(())
}

async fn update_after_sync(
    client: &tokio_postgres::Client,
    id: Uuid,
    mirror_path: &str,
    size_bytes: i64,
    ref_count: i32,
) -> Result<(), ServiceError> {
    let sql = format!(
        "update {} set mirror_status = 'ready', last_error = null, last_synced_at = now(), \
         mirror_path = $2, size_bytes = $3, ref_count = $4, updated_at = now() where id = $1",
        pg_contract::VCS_REPOSITORIES_TABLE
    );
    client
        .execute(&sql, &[&id, &mirror_path, &size_bytes, &ref_count])
        .await
        .map_err(db_error)?;
    Ok(())
}

/// Run the VCS refs command, persist a fresh `vcs_refs` snapshot, cache it in
/// Redis, and return the ref count.
async fn refresh_refs(
    state: &AppState,
    client: &tokio_postgres::Client,
    repo: &RepoRow,
    kind: VcsKind,
    dest: &str,
) -> Result<i32, ServiceError> {
    let args = vcs::refs_args(kind, dest);
    let output = {
        let _permit = acquire_op_permit(state)?;
        vcs::run(
            kind.binary(),
            &args,
            None,
            &BTreeMap::new(),
            state.config.read_timeout,
            state.config.max_output_bytes,
        )
        .await
        .and_then(vcs::require_success)
    }
    .map_err(|error| {
        ServiceError::Internal(format!(
            "refs failed: {}",
            sanitize_message(&error.to_string(), &state.config.storage_root)
        ))
    })?;

    let (refs, payload) = vcs::refs_payload(kind, &output.stdout, &repo.default_branch);

    // Replace the persisted ref snapshot atomically.
    let delete_sql = format!(
        "delete from {} where repository_id = $1",
        pg_contract::VCS_REFS_TABLE
    );
    client.execute(&delete_sql, &[&repo.id]).await.map_err(db_error)?;
    let insert_sql = format!(
        "insert into {} (repository_id, ref_name, ref_type, target_revision, is_default) \
         values ($1, $2, $3, $4, $5) on conflict (repository_id, ref_name) do update set \
         ref_type = excluded.ref_type, target_revision = excluded.target_revision, \
         is_default = excluded.is_default, updated_at = now()",
        pg_contract::VCS_REFS_TABLE
    );
    for entry in &refs {
        // Skip refs that can't satisfy the stored-revision NOT-NULL/size contract.
        let target = if entry.target.is_empty() { "0" } else { entry.target.as_str() };
        let target = target.chars().take(120).collect::<String>();
        let name = entry.name.chars().take(255).collect::<String>();
        client
            .execute(
                &insert_sql,
                &[&repo.id, &name, &entry.ref_type, &target, &entry.is_default],
            )
            .await
            .map_err(db_error)?;
    }

    cache_refs(state, repo.id, &payload).await;
    Ok(refs.len() as i32)
}

async fn list_refs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ServiceError> {
    let id = parse_uuid(&id)?;

    // Authorize against the repo's visibility first, so the Redis fast path can
    // never serve a private repo's refs to an unauthenticated caller.
    let client = connect_postgres(&state).await?;
    let repo = fetch_repo(&client, id)
        .await?
        .ok_or_else(|| ServiceError::NotFound("repository not found".to_string()))?;
    authorize_read(&headers, &state.config, &repo)?;

    // Fast path: serve a fresh cached snapshot without touching the mirror.
    if let Some(cached) = read_cached_refs(&state, id).await {
        record_request("GET", "/api/v1/repos/:id/refs", StatusCode::OK);
        return Ok(Json(json!({ "ok": true, "cached": true, "refs": cached.get("refs") })));
    }

    let kind = require_ready_mirror(&state, &repo)?;
    let dest = repo_storage_path(&state.config, kind, &repo.slug);
    let ref_count = refresh_refs(&state, &client, &repo, kind, &dest).await?;
    let payload = read_cached_refs(&state, id).await.unwrap_or_else(|| json!({ "refs": [] }));
    record_operation(
        &state,
        Some(id),
        kind,
        "refs",
        "success",
        json!({}),
        json!({ "refCount": ref_count }),
        None,
        None,
        None,
    )
    .await;
    record_request("GET", "/api/v1/repos/:id/refs", StatusCode::OK);
    Ok(Json(json!({ "ok": true, "cached": false, "refs": payload.get("refs") })))
}

#[derive(Deserialize)]
struct LogQuery {
    limit: Option<i64>,
    rev: Option<String>,
}

async fn get_log(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<LogQuery>,
) -> Result<Json<Value>, ServiceError> {
    let id = parse_uuid(&id)?;
    let limit = query.limit.unwrap_or(DEFAULT_LOG_LIMIT).clamp(1, MAX_LOG_LIMIT);
    if let Some(rev) = &query.rev {
        validate_revision(rev)?;
    }

    let client = connect_postgres(&state).await?;
    let repo = fetch_repo(&client, id)
        .await?
        .ok_or_else(|| ServiceError::NotFound("repository not found".to_string()))?;
    authorize_read(&headers, &state.config, &repo)?;
    let kind = require_ready_mirror(&state, &repo)?;
    let dest = repo_storage_path(&state.config, kind, &repo.slug);

    let args = vcs::log_args(kind, &dest, query.rev.as_deref(), limit);
    let _permit = acquire_op_permit(&state)?;
    let output = run_read(&state, kind, &args).await.map_err(|error| {
        ServiceError::Internal(format!(
            "log failed: {}",
            sanitize_message(&error.to_string(), &state.config.storage_root)
        ))
    })?;
    let payload = vcs::log_payload(kind, &output.stdout);
    record_operation(
        &state,
        Some(id),
        kind,
        "log",
        "success",
        json!({ "limit": limit, "rev": query.rev }),
        json!({ "truncated": output.truncated }),
        None,
        None,
        None,
    )
    .await;
    record_request("GET", "/api/v1/repos/:id/log", StatusCode::OK);
    Ok(Json(json!({
        "ok": true,
        "vcsKind": kind.as_str(),
        "truncated": output.truncated,
        "log": payload,
    })))
}

async fn get_show(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, rev)): Path<(String, String)>,
) -> Result<Json<Value>, ServiceError> {
    let id = parse_uuid(&id)?;
    // The wildcard capture may include a leading slash; trim before validating.
    let rev = rev.trim_start_matches('/').to_string();
    validate_revision(&rev)?;

    let client = connect_postgres(&state).await?;
    let repo = fetch_repo(&client, id)
        .await?
        .ok_or_else(|| ServiceError::NotFound("repository not found".to_string()))?;
    authorize_read(&headers, &state.config, &repo)?;
    let kind = require_ready_mirror(&state, &repo)?;
    let dest = repo_storage_path(&state.config, kind, &repo.slug);

    let args = vcs::show_args(kind, &dest, &rev);
    let _permit = acquire_op_permit(&state)?;
    let output = run_read(&state, kind, &args).await.map_err(|error| {
        ServiceError::Internal(format!(
            "show failed: {}",
            sanitize_message(&error.to_string(), &state.config.storage_root)
        ))
    })?;
    record_operation(
        &state,
        Some(id),
        kind,
        "show",
        "success",
        json!({ "rev": rev }),
        json!({ "truncated": output.truncated }),
        None,
        None,
        None,
    )
    .await;
    record_request("GET", "/api/v1/repos/:id/show/:rev", StatusCode::OK);
    Ok(Json(json!({
        "ok": true,
        "vcsKind": kind.as_str(),
        "revision": rev,
        "truncated": output.truncated,
        "content": output.stdout,
    })))
}

async fn list_operations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, ServiceError> {
    let id = parse_uuid(&id)?;
    let limit = query.limit.unwrap_or(50).clamp(1, MAX_LIST_LIMIT);
    let client = connect_postgres(&state).await?;
    let repo = fetch_repo(&client, id)
        .await?
        .ok_or_else(|| ServiceError::NotFound("repository not found".to_string()))?;
    authorize_read(&headers, &state.config, &repo)?;
    let sql = format!(
        "select id, vcs_kind, op_type, status, params, result_summary, error, duration_ms, \
         requested_by, created_at from {} where repository_id = $1 order by created_at desc limit $2",
        pg_contract::VCS_OPERATIONS_TABLE
    );
    let rows = client.query(&sql, &[&id, &limit]).await.map_err(db_error)?;
    let operations: Vec<Value> = rows
        .iter()
        .map(|row| {
            let oid: Uuid = row.get("id");
            let created_at: DateTime<Utc> = row.get("created_at");
            let duration_ms: Option<i32> = row.get("duration_ms");
            json!({
                "id": oid.to_string(),
                "vcsKind": row.get::<_, String>("vcs_kind"),
                "opType": row.get::<_, String>("op_type"),
                "status": row.get::<_, String>("status"),
                "params": row.get::<_, Value>("params"),
                "resultSummary": row.get::<_, Value>("result_summary"),
                "error": row.get::<_, Option<String>>("error"),
                "durationMs": duration_ms,
                "requestedBy": row.get::<_, Option<String>>("requested_by"),
                "createdAt": created_at.to_rfc3339(),
            })
        })
        .collect();
    record_request("GET", "/api/v1/repos/:id/operations", StatusCode::OK);
    Ok(Json(json!({ "ok": true, "count": operations.len(), "operations": operations })))
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn require_ready_mirror(state: &AppState, repo: &RepoRow) -> Result<VcsKind, ServiceError> {
    let kind = repo
        .kind()
        .ok_or_else(|| ServiceError::Internal("repository has unknown vcs kind".to_string()))?;
    if !state.vcs_binary_available(kind) {
        return Err(ServiceError::Unavailable(format!(
            "{} binary is not installed",
            kind.binary()
        )));
    }
    if repo.mirror_status != "ready" {
        return Err(ServiceError::Conflict(format!(
            "repository is not mirrored yet (status: {})",
            repo.mirror_status
        )));
    }
    Ok(kind)
}

async fn run_read(
    state: &AppState,
    kind: VcsKind,
    args: &[String],
) -> Result<vcs::CmdOutput, VcsError> {
    vcs::run(
        kind.binary(),
        args,
        None,
        &BTreeMap::new(),
        state.config.read_timeout,
        state.config.max_output_bytes,
    )
    .await
    .and_then(vcs::require_success)
}

fn parse_uuid(value: &str) -> Result<Uuid, ServiceError> {
    Uuid::parse_str(value).map_err(|_| ServiceError::BadRequest("invalid id".to_string()))
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(200).collect())
}

fn random_token() -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = Sha256::new();
    hasher.update(nonce.to_le_bytes());
    hasher.update(Uuid::new_v4().as_bytes());
    format!("{:x}", hasher.finalize())
}

async fn acquire_lock(
    state: &AppState,
    key: &str,
    token: &str,
) -> Result<bool, ServiceError> {
    let mut connection = state.redis_connection().await?;
    let acquired: bool = redis::cmd("SET")
        .arg(key)
        .arg(token)
        .arg("NX")
        .arg("EX")
        .arg(MIRROR_LOCK_TTL_SECONDS)
        .query_async(&mut connection)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "redis lock acquire failed");
            ServiceError::Unavailable("redis lock failed".to_string())
        })?;
    Ok(acquired)
}

async fn release_lock(state: &AppState, key: &str, token: &str) {
    let Ok(mut connection) = state.redis_connection().await else {
        return;
    };
    // Only release the lock if we still own it (compare-and-delete).
    let _: Result<i64, _> = redis::cmd("EVAL")
        .arg("if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('del', KEYS[1]) else return 0 end")
        .arg(1)
        .arg(key)
        .arg(token)
        .query_async(&mut connection)
        .await;
}

async fn cache_refs(state: &AppState, id: Uuid, payload: &Value) {
    let Ok(mut connection) = state.redis_connection().await else {
        return;
    };
    let key = format!("{}:refs:{}", state.config.redis_prefix, id);
    if let Ok(serialized) = serde_json::to_string(payload) {
        let _: Result<(), _> = connection
            .set_ex(key, serialized, state.config.refs_cache_ttl)
            .await;
    }
}

async fn read_cached_refs(state: &AppState, id: Uuid) -> Option<Value> {
    let mut connection = state.redis_connection().await.ok()?;
    let key = format!("{}:refs:{}", state.config.redis_prefix, id);
    let serialized: Option<String> = connection.get(key).await.ok()?;
    serialized.and_then(|value| serde_json::from_str(&value).ok())
}

async fn storage_writable(root: &str) -> bool {
    if tokio::fs::create_dir_all(root).await.is_err() {
        return false;
    }
    let probe = format!("{}/.dd-git-rs-write-probe", root.trim_end_matches('/'));
    if tokio::fs::write(&probe, b"ok").await.is_err() {
        return false;
    }
    let _ = tokio::fs::remove_file(&probe).await;
    true
}

/// Remove a mirror from disk, whether it is a directory (git/hg/svn) or a single
/// file (fossil). Best-effort; failures are logged, not fatal.
async fn remove_mirror(path: &str) {
    match tokio::fs::metadata(path).await {
        Ok(meta) if meta.is_dir() => {
            if let Err(error) = tokio::fs::remove_dir_all(path).await {
                tracing::warn!(%error, path, "failed to remove mirror directory");
            }
        }
        Ok(_) => {
            if let Err(error) = tokio::fs::remove_file(path).await {
                tracing::warn!(%error, path, "failed to remove mirror file");
            }
        }
        Err(_) => {}
    }
}

/// Recursively sum file sizes under `path` in a blocking task. Best-effort:
/// errors yield 0 rather than failing the sync.
async fn directory_size(path: &str) -> i64 {
    let path = path.to_string();
    tokio::task::spawn_blocking(move || {
        // `entry.file_type()` does not follow symlinks, so a symlinked directory
        // is counted as a file and never recursed into; a depth cap bounds the
        // walk regardless.
        fn walk(dir: &std::path::Path, depth: u32) -> u64 {
            if depth == 0 {
                return 0;
            }
            let mut total = 0u64;
            let Ok(entries) = std::fs::read_dir(dir) else {
                return 0;
            };
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else { continue };
                if file_type.is_dir() {
                    total = total.saturating_add(walk(&entry.path(), depth - 1));
                } else if let Ok(meta) = entry.metadata() {
                    total = total.saturating_add(meta.len());
                }
            }
            total
        }
        let meta = std::fs::symlink_metadata(&path);
        match meta {
            Ok(meta) if meta.is_file() => meta.len(),
            Ok(meta) if meta.is_dir() => walk(std::path::Path::new(&path), 64),
            _ => 0,
        }
    })
    .await
    .unwrap_or(0)
    .min(i64::MAX as u64) as i64
}

// ---------------------------------------------------------------------------
// Router + main
// ---------------------------------------------------------------------------

fn app(state: AppState) -> Router {
    let body_limit = state.config.max_body_bytes;
    Router::new()
        .route("/", get(home))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/api/v1/vcs/kinds", get(list_kinds))
        .route("/api/v1/repos", get(list_repos).post(create_repo))
        .route("/api/v1/repos/:id", get(get_repo).delete(delete_repo))
        .route("/api/v1/repos/:id/sync", post(sync_repo))
        .route("/api/v1/repos/:id/refs", get(list_refs))
        .route("/api/v1/repos/:id/log", get(get_log))
        .route("/api/v1/repos/:id/show/*rev", get(get_show))
        .route("/api/v1/repos/:id/operations", get(list_operations))
        .layer(DefaultBodyLimit::max(body_limit))
        .with_state(state)
}

#[tokio::main]
async fn main() {
    init_tracing();
    rustls::crypto::ring::default_provider().install_default().ok();

    let config = config_from_env();
    let host = first_env(&["GIT_RS_HOST", "HOST"]).unwrap_or_else(|| "0.0.0.0".to_string());
    let port = env_u64("GIT_RS_PORT", env_u64("PORT", DEFAULT_PORT)) as u16;

    let storage_root = config.storage_root.clone();
    if let Err(error) = tokio::fs::create_dir_all(&storage_root).await {
        tracing::warn!(%error, root = %storage_root, "could not pre-create storage root");
    }

    let state = state_from_config(config).await;
    for entry in state.vcs_available.values() {
        tracing::info!(
            kind = entry.kind,
            available = entry.available,
            version = entry.version.as_deref().unwrap_or("n/a"),
            "vcs backend"
        );
    }

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("HOST/PORT must form a socket address");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind dd-git-rs");
    tracing::info!(%addr, "dd-git-rs listening");
    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("axum server crashed");
}

fn init_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter = EnvFilter::try_from_env("RUST_LOG")
        .unwrap_or_else(|_| EnvFilter::new("dd_git_rs=info,tower_http=info"));
    let json = first_env(&["GIT_RS_LOG_FORMAT"]).map(|v| v == "json").unwrap_or(false);
    let registry = tracing_subscriber::registry().with(filter);
    if json {
        registry.with(fmt::layer().json()).init();
    } else {
        registry.with(fmt::layer()).init();
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_host_handles_transports() {
        assert_eq!(extract_host("https://u:p@github.com/x/y.git").as_deref(), Some("github.com"));
        assert_eq!(extract_host("https://169.254.169.254/latest").as_deref(), Some("169.254.169.254"));
        assert_eq!(extract_host("git@github.com:org/repo.git").as_deref(), Some("github.com"));
        assert_eq!(extract_host("ssh://git@[::1]:22/repo").as_deref(), Some("::1"));
        assert_eq!(extract_host("svn://10.0.0.5:3690/trunk").as_deref(), Some("10.0.0.5"));
    }

    #[test]
    fn ssrf_guard_blocks_dangerous_hosts() {
        // Always blocked regardless of the private-network flag.
        for host in ["localhost", "127.0.0.1", "169.254.169.254", "0.0.0.0", "::1"] {
            assert!(host_is_blocked(host, false), "{host} should be blocked");
        }
        // Private ranges only when the flag is on.
        assert!(!host_is_blocked("10.0.0.5", false));
        assert!(host_is_blocked("10.0.0.5", true));
        assert!(host_is_blocked("192.168.1.10", true));
        // Public hosts are fine.
        assert!(!host_is_blocked("github.com", true));
        assert!(!host_is_blocked("140.82.112.3", true));
    }

    #[test]
    fn ssrf_guard_blocks_alternate_ip_encodings() {
        // Decimal, hex, and octal encodings of 127.0.0.1 and friends.
        assert!(host_is_blocked("2130706433", false)); // 127.0.0.1
        assert!(host_is_blocked("0x7f000001", false));
        assert!(host_is_blocked("0177.0.0.1", false));
        assert!(host_is_blocked("127.1", false));
        assert!(host_is_blocked("127.0x0.0.1", false));
        // Real domains with numeric labels are still allowed.
        assert!(!host_is_blocked("api.v2.github.com", false));
        assert!(!host_is_blocked("host123.example.com", false));
    }

    #[test]
    fn redact_url_strips_credentials() {
        assert_eq!(redact_url("https://user:pass@host.com/a/b.git"), "https://host.com/a/b.git");
        assert_eq!(redact_url("https://token@host.com"), "https://host.com");
        assert_eq!(redact_url("https://host.com/no/creds"), "https://host.com/no/creds");
        assert_eq!(redact_url("git@github.com:org/repo.git"), "git@github.com:org/repo.git");
    }

    #[test]
    fn sanitize_message_scrubs_paths_and_creds() {
        let msg = "fatal: /var/lib/dd-git-rs/repos/git/x failed for https://u:p@h/x.git";
        let cleaned = sanitize_message(msg, "/var/lib/dd-git-rs/repos");
        assert!(cleaned.contains("<storage>/git/x"));
        assert!(!cleaned.contains("u:p@"));
        assert!(!cleaned.contains("p@h"));
    }

    #[test]
    fn revision_validation_rejects_option_injection() {
        assert!(validate_revision("HEAD").is_ok());
        assert!(validate_revision("feature/x").is_ok());
        assert!(validate_revision("a1b2c3d").is_ok());
        assert!(validate_revision("--upload-pack=evil").is_err());
        assert!(validate_revision("-x").is_err());
        assert!(validate_revision("a b").is_err());
        assert!(validate_revision("rev;rm -rf").is_err());
    }

    #[test]
    fn slug_validation_blocks_traversal() {
        assert!(validate_slug("k8s-cluster").is_ok());
        assert!(validate_slug("../etc").is_err());
        assert!(validate_slug("a/b").is_err());
        assert!(validate_slug("UPPER").is_err());
        assert!(validate_slug("").is_err());
    }
}

const HOME_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>dd-git-rs</title>
  <style>
    body { margin:0; background:#0f1419; color:#e6edf3; font:14px/1.6 ui-monospace, SFMono-Regular, Menlo, monospace; }
    .wrap { max-width:760px; margin:0 auto; padding:48px 24px; }
    h1 { font-size:20px; margin:0 0 4px; }
    .muted { color:#8b949e; }
    code { background:#161b22; border:1px solid #21262d; border-radius:4px; padding:1px 5px; }
    table { border-collapse:collapse; width:100%; margin-top:16px; }
    td, th { text-align:left; padding:6px 10px; border-bottom:1px solid #21262d; vertical-align:top; }
    .pill { display:inline-block; background:#161b22; border:1px solid #21262d; border-radius:999px; padding:1px 10px; margin-right:6px; }
  </style>
</head>
<body>
  <div class="wrap">
    <h1>dd-git-rs</h1>
    <p class="muted">Multi-VCS operations server.
      <span class="pill">git</span><span class="pill">mercurial</span><span class="pill">subversion</span><span class="pill">fossil</span>
    </p>
    <table>
      <tr><th>Route</th><th>Purpose</th></tr>
      <tr><td><code>GET /api/v1/vcs/kinds</code></td><td>Supported VCS kinds and binary availability.</td></tr>
      <tr><td><code>GET /api/v1/repos</code></td><td>List registered repositories.</td></tr>
      <tr><td><code>POST /api/v1/repos</code></td><td>Register a repository <span class="muted">(auth)</span>.</td></tr>
      <tr><td><code>GET /api/v1/repos/:id</code></td><td>Repository detail.</td></tr>
      <tr><td><code>POST /api/v1/repos/:id/sync</code></td><td>Mirror or re-fetch from origin <span class="muted">(auth)</span>.</td></tr>
      <tr><td><code>GET /api/v1/repos/:id/refs</code></td><td>Branches / tags / bookmarks (Redis-cached).</td></tr>
      <tr><td><code>GET /api/v1/repos/:id/log</code></td><td>Commit log (<code>?rev=&amp;limit=</code>).</td></tr>
      <tr><td><code>GET /api/v1/repos/:id/show/:rev</code></td><td>Show a commit/revision with diff.</td></tr>
      <tr><td><code>GET /api/v1/repos/:id/operations</code></td><td>Operation audit trail.</td></tr>
    </table>
    <p class="muted" style="margin-top:24px">Docs: <code>/docs/api</code> · <code>/api/docs.json</code> · Health: <code>/healthz</code> · <code>/readyz</code> · <code>/metrics</code></p>
  </div>
</body>
</html>"##;
