//! `dd-sound-recorder-rs` — the Sonus Auris "sound-recorder" backend.
//!
//! A single-binary Axum/Tokio HTTP service. Mobile clients record short audio
//! segments, and this service issues presigned S3 upload URLs, tracks segment
//! metadata in Postgres, brokers cloud-copy (mirror) jobs to user-owned Google
//! Drive / OneDrive / iCloud destinations, and emails short-lived alert
//! "listen" links. Normal mobile transfer uses presigned URLs; bytes flow
//! through this process only when the authenticated cloud-copy worker mirrors a
//! segment to Google Drive or OneDrive, or when the storage-mirror worker
//! copies a segment into the backup object store. See `readme.md` for routes,
//! environment variables, and the wider product/deployment context.
//!
//! The process entry point and telemetry setup live in focused sibling
//! modules. This module contains the HTTP application and its domain behavior,
//! while the security-sensitive Supabase JWT verifier lives in
//! `src/supabase_auth.rs`. Its major sections, roughly in order, are:
//!
//! - **Metrics** — Prometheus `Lazy` collectors (`HTTP_REQUESTS`,
//!   `SEGMENT_PRESIGNS`, `RATE_LIMITED`, uptime) exposed at `GET /metrics`.
//! - **Constants** — service defaults and clamped limits (retention hours,
//!   segment sizes, URL TTLs, OAuth scopes, rate-limit window).
//! - **Config** — `Config`/`SupabaseConfig`/`S3StorageConfig` structs, the
//!   `env_*` parsing helpers, `config_from_env`, and `state_from_config` which
//!   builds the shared `AppState` (Postgres pool, S3 client, token sealer).
//! - **Types** — `ServiceError` (→ HTTP responses) plus the request/response
//!   DTO structs for every route.
//! - **Auth / JWT** — `authenticate_supabase_account`, opaque device bearer
//!   tokens (`authenticate_device`, SHA-256 + pepper), the registration bearer,
//!   and internal server-auth secret for `/internal/*` routes. The Supabase JWT
//!   verifier with cached JWKS is isolated in `supabase_auth`.
//! - **Presign** — upload-session lifecycle, `presign_segment`, and the
//!   `presign_put` / `presign_get` S3 URL builders (short-lived PUT/GET).
//! - **Cloud connections** — OAuth link start/complete, AES-256-GCM
//!   `CloudTokenSealer`, and the `/internal/cloud-copy/drain` worker that
//!   uploads segments to Google Drive / OneDrive.
//! - **Alerts** — `create_alert`, the `/listen/:alert_id` page, and
//!   `send_alert_email` webhook payload.
//! - **Account deletion** — `delete_account` and `delete_supabase_auth_user`
//!   (Supabase service-role key), which purge backend metadata and revoke tokens.
//! - **Retention** — `retention_sweep` marks expired (non-pinned) segment rows
//!   and physically deletes both the primary object and any mirror copy.
//! - **Storage mirror** — `mirror_drain` (`/internal/storage-mirror/drain`)
//!   asynchronously copies uploaded segments from the primary object store
//!   into the `SOUND_RECORDER_MIRROR_*` backup store (e.g. Cloudflare R2 next
//!   to AWS S3), recording per-segment mirror state in `meta_data`.
//! - **Rate limiting & security** — the `rate_limit` and `add_security_headers`
//!   middleware layers.
//! - **Runtime / router** — `Router::new()` wiring, SeaORM setup, and
//!   graceful shutdown. Unit tests live in the trailing `#[cfg(test)]` module.

use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use aws_config::retry::RetryConfig;
use aws_sdk_s3::{
    config::Region,
    presigning::PresigningConfig,
    primitives::ByteStream,
    types::{Delete, ObjectIdentifier, ServerSideEncryption},
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, DefaultBodyLimit, MatchedPath, Path, Query, Request, State,
    },
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{delete, get, post},
    Json, Router,
};
use base64::{
    engine::general_purpose::{STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures_util::{SinkExt, StreamExt};
#[cfg(test)]
use jsonwebtoken::Algorithm;
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounter, IntCounterVec, IntGauge, Opts, TextEncoder};
use rand::{rngs::OsRng, RngCore};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sonus_auris_interfaces::{
    AcousticEvent, DeviceRecord, UserConsent, UserSettings, ACOUSTIC_EVENTS_COLUMNS,
    ACOUSTIC_EVENTS_TABLE, CLOUD_CONNECTIONS_TABLE, DEVICES_COLUMNS, DEVICES_TABLE,
    USER_CONSENTS_COLUMNS, USER_CONSENTS_TABLE, USER_SETTINGS_CLOUD_PROVIDER_VALUES,
    USER_SETTINGS_COLUMNS, USER_SETTINGS_PREFERRED_USE_CASE_VALUES, USER_SETTINGS_TABLE,
};
use tokio::sync::{broadcast, Mutex as AsyncMutex, RwLock};
use tracing::{error, field, info, warn, Instrument};
use uuid::Uuid;

use crate::database::{DbClient, Row};
#[path = "supabase_auth.rs"]
mod supabase_auth;

#[cfg(test)]
use supabase_auth::is_supported_supabase_algorithm;
use supabase_auth::{SupabaseIdentity, SupabaseVerifier};

static STARTED_AT: Lazy<Instant> = Lazy::new(Instant::now);
static HTTP_REQUESTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "dd_sound_recorder_rs_http_requests_total",
            "HTTP requests observed by dd-sound-recorder-rs.",
        ),
        &["method", "path", "status"],
    )
    .expect("failed to create dd_sound_recorder_rs_http_requests_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("failed to register dd_sound_recorder_rs_http_requests_total");
    counter
});
static UPTIME_SECONDS: Lazy<IntGauge> = Lazy::new(|| {
    let gauge = IntGauge::new(
        "dd_sound_recorder_rs_uptime_seconds",
        "dd-sound-recorder-rs process uptime in seconds.",
    )
    .expect("failed to create dd_sound_recorder_rs_uptime_seconds");
    prometheus::default_registry()
        .register(Box::new(gauge.clone()))
        .expect("failed to register dd_sound_recorder_rs_uptime_seconds");
    gauge
});
static RATE_LIMITED: Lazy<IntCounter> = Lazy::new(|| {
    let counter = IntCounter::new(
        "dd_sound_recorder_rs_rate_limited_total",
        "Requests rejected by the per-client rate limiter.",
    )
    .expect("failed to create dd_sound_recorder_rs_rate_limited_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("failed to register dd_sound_recorder_rs_rate_limited_total");
    counter
});
static SEGMENT_PRESIGNS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "dd_sound_recorder_rs_segment_presigns_total",
            "S3 upload/download presigns minted by dd-sound-recorder-rs.",
        ),
        &["direction", "result"],
    )
    .expect("failed to create dd_sound_recorder_rs_segment_presigns_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("failed to register dd_sound_recorder_rs_segment_presigns_total");
    counter
});

static MIRROR_COPIES: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "dd_sound_recorder_rs_mirror_copies_total",
            "Segment mirror copy attempts by dd-sound-recorder-rs.",
        ),
        &["result"],
    )
    .expect("failed to create dd_sound_recorder_rs_mirror_copies_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("failed to register dd_sound_recorder_rs_mirror_copies_total");
    counter
});

const SERVICE_NAME: &str = "dd-sound-recorder-rs";
const DEFAULT_PORT: u16 = 8126;
const DEFAULT_RETENTION_HOURS: i32 = 500;
const MAX_RETENTION_HOURS: i32 = 500;
const DEFAULT_SEGMENT_SECONDS: i32 = 60;
const DEFAULT_MAX_SEGMENT_SECONDS: i32 = 120;
const DEFAULT_MAX_SEGMENT_BYTES: i32 = 10 * 1024 * 1024;
const MAX_SEGMENT_BYTES: i32 = 200 * 1024 * 1024;
const DEFAULT_UPLOAD_URL_TTL_SECONDS: u64 = 300;
const DEFAULT_DOWNLOAD_URL_TTL_SECONDS: u64 = 900;
// Do not mint a PUT URL right at the retention boundary. An upload that starts
// before its signature expires may still be in flight when the row expires;
// this settle window keeps normal retention deletion well after that request.
const PRESIGNED_UPLOAD_SETTLE_GRACE_SECONDS: u64 = 600;
const DEFAULT_S3_MAX_ATTEMPTS: u32 = 3;
const STORAGE_PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const STORAGE_HISTORY_CACHE_TTL: Duration = Duration::from_secs(60);
const SUPABASE_PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const STORAGE_OBJECT_TIMEOUT: Duration = Duration::from_secs(30);
const POSTGRES_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_SESSION_TTL_HOURS: i64 = 24;
const MAX_TIMELINE_LIMIT: i64 = 500;
const DEFAULT_USER_DATA_LIMIT: usize = 50;
const MAX_USER_DATA_LIMIT: usize = 200;
const MAX_EXPORT_SEGMENTS: i64 = 240;
const MAX_META_BYTES: usize = 4096;
const STORAGE_FINGERPRINT_META_KEY: &str = "sonusAurisStorageFingerprint";
const RETENTION_DELETE_PENDING_META_KEY: &str = "retentionDeletePending";
const RETENTION_DELETE_CLAIM_ID_META_KEY: &str = "retentionDeleteClaimId";
const RETENTION_DELETE_CLAIMED_AT_META_KEY: &str = "retentionDeleteClaimedAt";
const RETENTION_PREVIOUS_STATUS_META_KEY: &str = "retentionPreviousStatus";
// Server-owned mirror bookkeeping on `sound_recorder_segments.meta_data`. The
// mirror is a real backup only when `mirrorState = mirrored` and the recorded
// bucket/fingerprint match a deletable mirror target; retention and account
// erasure refuse to finalize while a mirror copy may still exist.
const MIRROR_STATE_META_KEY: &str = "mirrorState";
const MIRROR_CLAIM_ID_META_KEY: &str = "mirrorClaimId";
const MIRROR_CLAIMED_AT_META_KEY: &str = "mirrorClaimedAt";
const MIRROR_BUCKET_META_KEY: &str = "mirrorBucket";
const MIRROR_FINGERPRINT_META_KEY: &str = "mirrorFingerprint";
const MIRROR_MIRRORED_AT_META_KEY: &str = "mirrorMirroredAt";
const MIRROR_ATTEMPTS_META_KEY: &str = "mirrorAttempts";
const MIRROR_LAST_ERROR_META_KEY: &str = "mirrorLastError";
const MIRROR_NEXT_ATTEMPT_AT_META_KEY: &str = "mirrorNextAttemptAt";
const DEFAULT_MIRROR_BATCH_SIZE: i64 = 50;
const MAX_MIRROR_BATCH_SIZE: i64 = 500;
const DEFAULT_MIRROR_COPY_MAX_ATTEMPTS: i32 = 5;
/// A stale `copying` claim is reclaimable after this lease, far longer than the
/// bounded object download + upload (2 × [`STORAGE_OBJECT_TIMEOUT`]).
const MIRROR_CLAIM_LEASE: &str = "10 minutes";
const MAX_CAPTURE_CLOCK_SKEW_SECONDS: i64 = 300;
const DEFAULT_OAUTH_STATE_TTL_SECONDS: u64 = 600;
const DEFAULT_CLOUD_COPY_BATCH_SIZE: i64 = 25;
const MAX_CLOUD_COPY_BATCH_SIZE: i64 = 100;
const DEFAULT_CLOUD_PROJECTION_BATCH_SIZE: i64 = 25;
const MAX_CLOUD_PROJECTION_BATCH_SIZE: i64 = 100;
const CLOUD_PROJECTION_CLAIM_LEASE: &str = "2 minutes";
/// How long a device's reported transfer pause is honored before the drain
/// resumes its server-managed copies. A live paused client re-affirms well
/// within this window; a vanished one stops blocking after it (server copies
/// don't use the phone battery, so resuming once the client is gone is safe).
const TRANSFER_PAUSE_LEASE: &str = "6 hours";
const DEFAULT_CLOUD_COPY_MAX_ATTEMPTS: i32 = 3;
const DEFAULT_CLOUD_COPY_MAX_BYTES: i64 = 25 * 1024 * 1024;
const MAX_CLOUD_COPY_MAX_BYTES: i64 = 200 * 1024 * 1024;
// Google requires resumable-upload chunks to be multiples of 256 KiB. Eight
// MiB keeps each provider request bounded while remaining efficient for the
// normal 1-minute recording segments.
const GOOGLE_RESUMABLE_CHUNK_BYTES: usize = 8 * 1024 * 1024;
const DROPBOX_SINGLE_UPLOAD_MAX_BYTES: usize = 150 * 1024 * 1024;
const DROPBOX_SESSION_CHUNK_BYTES: usize = 8 * 1024 * 1024;
const CLOUD_PROVIDER_UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_CLOUD_BACKFILL_SEGMENTS: i64 = 240;
const MAX_CLOUD_BACKFILL_SEGMENTS: i64 = 1000;
const GOOGLE_DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const MICROSOFT_ONEDRIVE_SCOPE: &str = "offline_access Files.ReadWrite.AppFolder";
const DROPBOX_SCOPE: &str = "files.content.write";
const GOOGLE_TOKEN_REVOCATION_URL: &str = "https://oauth2.googleapis.com/revoke";
const DROPBOX_TOKEN_REVOCATION_URL: &str = "https://api.dropboxapi.com/2/auth/token/revoke";
const PROVIDER_REVOCATION_TIMEOUT: Duration = Duration::from_secs(5);
const SUPABASE_DEFAULT_AUDIENCE: &str = "authenticated";
const SHARED_AUTH_INTROSPECTION_TIMEOUT: Duration = Duration::from_secs(5);
const SHARED_AUTH_INTROSPECTION_MAX_BYTES: usize = 64 * 1024;
const SHARED_AUTH_REQUIRED_ACR: &str = "urn:oresoftware:loa:2";
const SHARED_AUTH_MAX_AUTH_AGE_SECONDS: u64 = 15 * 60;
const SHARED_AUTH_CLOCK_SKEW_SECONDS: u64 = 60;
const SHARED_AUTH_MAX_AMR_METHODS: usize = 16;
const SHARED_AUTH_MAX_AMR_METHOD_BYTES: usize = 64;
const DEFAULT_USE_CASE: &str = "security";
const SUPPORTED_USE_CASES: &[&str] = &["security", "music", "meeting", "voice_note", "ambient"];
const MAX_PERMANENT_SAVE_SEGMENTS: usize = 1000;
/// Default per-client request budget per 60s window. Generous enough that a
/// healthy device (segment presigns, timeline polls) never trips it, but it
/// caps abusive bursts against the anonymous register/auth surface. `0` (via
/// `SOUND_RECORDER_RATE_LIMIT_PER_MINUTE=0`) disables the limiter.
const DEFAULT_RATE_LIMIT_PER_MINUTE: u32 = 240;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

/// Ceiling on how long a client-facing request may occupy a task. Individual DB
/// and S3 calls carry their own timeouts, but nothing bounded a handler as a
/// whole, so a stalled dependency or a slow-reading client could pin a task
/// indefinitely. Comfortably above the slowest single dependency call
/// ([`STORAGE_OBJECT_TIMEOUT`], 30s) so a legitimately slow request still
/// returns its own error rather than a timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

/// The `/internal/*` drain endpoints walk a bounded batch of jobs, each with its
/// own object-store timeout, so their worst case is legitimately a multiple of
/// [`STORAGE_OBJECT_TIMEOUT`]. They are authenticated
/// ([`require_internal_auth`]) and driven by a trusted scheduler rather than by
/// clients, so they get a much longer ceiling instead of the client one.
const INTERNAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// SeaORM owns the bounded Postgres pool, rustls transport, parameter binding,
/// transactions, and row decoding. Schema DDL remains cluster-managed.
type PgPool = DbClient;
type PgConn = DbClient;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    s3: Option<aws_sdk_s3::Client>,
    /// Client for the backup/mirror object store (R2 alongside a primary S3, or
    /// vice versa). Never used to serve reads; only the mirror drain, retention
    /// sweep, and account erasure touch it.
    mirror: Option<aws_sdk_s3::Client>,
    http: reqwest::Client,
    cloud_sealer: Option<CloudTokenSealer>,
    supabase: Option<Arc<SupabaseVerifier>>,
    pg_pool: Option<PgPool>,
    storage_history_cache: Arc<RwLock<Option<(Instant, bool)>>>,
    storage_history_refresh_lock: Arc<AsyncMutex<()>>,
    device_presence: Arc<DevicePresenceHub>,
}

#[derive(Default)]
struct DevicePresenceHub {
    accounts: RwLock<HashMap<String, HashMap<String, usize>>>,
    senders: RwLock<HashMap<String, broadcast::Sender<Vec<String>>>>,
}

impl DevicePresenceHub {
    async fn join(&self, account_id: &str, device_id: &str) -> broadcast::Receiver<Vec<String>> {
        {
            let mut accounts = self.accounts.write().await;
            let devices = accounts.entry(account_id.to_string()).or_default();
            *devices.entry(device_id.to_string()).or_default() += 1;
        }
        let sender = self.sender(account_id).await;
        let receiver = sender.subscribe();
        self.publish(account_id, &sender).await;
        receiver
    }

    async fn leave(&self, account_id: &str, device_id: &str) {
        {
            let mut accounts = self.accounts.write().await;
            if let Some(devices) = accounts.get_mut(account_id) {
                if let Some(count) = devices.get_mut(device_id) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        devices.remove(device_id);
                    }
                }
                if devices.is_empty() {
                    accounts.remove(account_id);
                }
            }
        }
        let sender = self.sender(account_id).await;
        self.publish(account_id, &sender).await;
    }

    async fn sender(&self, account_id: &str) -> broadcast::Sender<Vec<String>> {
        if let Some(sender) = self.senders.read().await.get(account_id).cloned() {
            return sender;
        }
        let mut senders = self.senders.write().await;
        senders
            .entry(account_id.to_string())
            .or_insert_with(|| broadcast::channel(32).0)
            .clone()
    }

    async fn publish(&self, account_id: &str, sender: &broadcast::Sender<Vec<String>>) {
        let accounts = self.accounts.read().await;
        let mut online = accounts
            .get(account_id)
            .map(|devices| devices.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        online.sort();
        let _ = sender.send(online);
    }
}

#[derive(Clone)]
struct Config {
    /// Safe configuration diagnostics. Invalid booleans and other global
    /// settings fail readiness instead of silently changing security posture.
    validation_errors: Vec<String>,
    database_url: Option<String>,
    server_auth_secret: Option<String>,
    token_pepper: String,
    token_pepper_configured: bool,
    registration_bearer: Option<String>,
    allow_public_device_registration: bool,
    s3: S3StorageConfig,
    /// Backup/mirror object store. Independent of the primary: its own bucket,
    /// endpoint, and explicit credentials (no ambient AWS/R2 fallback, so a
    /// misconfigured mirror can never silently sign with primary credentials).
    mirror: S3StorageConfig,
    /// When true, a configured mirror must pass its readiness probe for
    /// `/readyz` to return 200. Default false: the mirror is a backup, not a
    /// serving dependency, so a mirror outage alone should not pull the
    /// service out of rotation. Misconfiguration (validation errors) always
    /// fails readiness regardless of this flag.
    mirror_readiness_required: bool,
    mirror_batch_size: i64,
    mirror_copy_max_attempts: i32,
    ios_app_store_url: Option<String>,
    android_play_store_url: Option<String>,
    default_retention_hours: i32,
    upload_url_ttl: Duration,
    download_url_ttl: Duration,
    session_ttl_hours: i64,
    default_segment_seconds: i32,
    max_segment_seconds: i32,
    max_segment_bytes: i32,
    oauth_state_ttl: Duration,
    cloud_copy_batch_size: i64,
    cloud_copy_max_attempts: i32,
    cloud_copy_max_bytes: i64,
    cloud_backfill_segments: i64,
    google_oauth: OAuthProviderConfig,
    microsoft_oauth: OAuthProviderConfig,
    dropbox_oauth: OAuthProviderConfig,
    /// Exact OAuth `redirectUri` values the backend will initiate a cloud-link
    /// flow with. Empty = accept any https / loopback-http URI (the OAuth
    /// provider still enforces its own registered-redirect allow-list). When
    /// set, the backend additionally pins redirects to these known app callbacks
    /// — defense in depth against an attacker-chosen redirect target.
    oauth_redirect_allowlist: Vec<String>,
    google_drive_upload_url: String,
    microsoft_graph_base_url: String,
    dropbox_upload_url: String,
    public_base_url: Option<String>,
    alert_email_to: String,
    alert_email_webhook_url: Option<String>,
    /// Per-client requests allowed per [`RATE_LIMIT_WINDOW`]. `0` disables it.
    rate_limit_per_minute: u32,
    /// When `true`, the leftmost `X-Forwarded-For` hop is the rate-limit key
    /// (correct behind a trusted reverse proxy / k8s ingress). When `false`,
    /// the limiter keys on the TCP peer address only (do this if the service is
    /// directly internet-exposed, so clients can't spoof their key via XFF).
    rate_limit_trust_forwarded_for: bool,
    /// Treat Supabase-backed sign-in, typed data reads, and account deletion as
    /// required service capabilities. Production defaults to strict; local
    /// anonymous-only development can explicitly opt out.
    require_supabase: bool,
    supabase: SupabaseConfig,
    shared_auth: SharedAuthConfig,
}

#[derive(Clone, Default)]
struct SupabaseConfig {
    /// Project base URL, e.g. https://<ref>.supabase.co. Retained for
    /// diagnostics; the issuer and JWKS URL are derived from it at startup.
    #[allow(dead_code)]
    url: Option<String>,
    /// Legacy HS256 JWT secret (Supabase "JWT Secret" under Settings -> API).
    jwt_secret: Option<String>,
    /// JWKS endpoint for asymmetric (RS256/ES256) signing keys.
    jwks_url: Option<String>,
    /// Expected `iss` claim; defaults to `<url>/auth/v1`.
    issuer: Option<String>,
    /// Expected `aud` claim; defaults to `authenticated`.
    audience: String,
    /// Publishable (or legacy anon) key used with the caller's JWT when the
    /// backend reads owner-scoped rows through the Supabase Data API.
    publishable_key: Option<String>,
    /// Server-only service-role key used for privileged Auth Admin operations
    /// such as deleting the signed-in user's Supabase Auth identity.
    service_role_key: Option<String>,
    /// Safe, secret-free configuration diagnostics. A malformed URL disables
    /// the verifier instead of being accepted as an outbound request target.
    validation_errors: Vec<String>,
}

impl SupabaseConfig {
    /// Auth is live only when a verification key AND a pinned issuer are both
    /// configured; anything less fails closed (the verifier is not built and the
    /// Supabase-backed surface reports unavailable rather than serving).
    ///
    /// The issuer is required rather than optional because `aud` is the literal
    /// string `"authenticated"` on *every* Supabase project. Without `iss`
    /// pinning, a token minted by an unrelated project — whose JWKS an attacker
    /// controls, or whose HS256 secret they know — satisfies every other check.
    /// `iss` is the only claim that binds a token to *this* project.
    ///
    /// Escape hatch for local/dev: the issuer is normally derived from
    /// `SOUND_RECORDER_SUPABASE_URL` (`<url>/auth/v1`), so any setup that sets
    /// the project URL already has one. A setup that configures only a raw
    /// `SOUND_RECORDER_SUPABASE_JWT_SECRET` (no project URL) must now also set
    /// `SOUND_RECORDER_SUPABASE_ISSUER` explicitly — see README "Supabase auth".
    fn is_enabled(&self) -> bool {
        self.validation_errors.is_empty()
            && (self.jwt_secret.is_some() || self.jwks_url.is_some())
            && self.issuer.is_some()
    }

    fn is_data_api_enabled(&self) -> bool {
        self.url.is_some() && self.publishable_key.is_some() && self.is_enabled()
    }

    fn account_features_configured(&self) -> bool {
        self.url.is_some()
            && self.issuer.is_some()
            && self.publishable_key.is_some()
            && self.service_role_key.is_some()
            && self.is_enabled()
    }

    fn auth_health_url(&self) -> Option<String> {
        self.url.as_ref().map(|url| format!("{url}/auth/v1/health"))
    }
}

#[derive(Clone)]
struct SharedAuthConfig {
    /// Root URL of shared-auth-server. The request path is always appended by
    /// this backend so configuration cannot redirect introspection elsewhere.
    base_url: Option<String>,
    /// Service credential accepted by shared-auth's `/auth/introspect`.
    introspect_secret: Option<String>,
    /// Minimum authentication assurance accepted for device enrollment.
    required_aal: u8,
    validation_errors: Vec<String>,
}

impl Default for SharedAuthConfig {
    fn default() -> Self {
        Self {
            base_url: None,
            introspect_secret: None,
            required_aal: 2,
            validation_errors: Vec::new(),
        }
    }
}

impl SharedAuthConfig {
    fn is_enabled(&self) -> bool {
        self.validation_errors.is_empty()
            && self.base_url.is_some()
            && self.introspect_secret.is_some()
    }

    fn introspect_url(&self) -> Option<String> {
        self.base_url
            .as_ref()
            .map(|url| format!("{url}/auth/introspect"))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ObjectStorageBackend {
    AmazonS3,
    CloudflareR2,
    S3Compatible,
}

impl ObjectStorageBackend {
    fn as_str(self) -> &'static str {
        match self {
            Self::AmazonS3 => "amazon_s3",
            Self::CloudflareR2 => "cloudflare_r2",
            Self::S3Compatible => "s3_compatible",
        }
    }
}

#[derive(Clone)]
struct S3StorageConfig {
    bucket: String,
    key_prefix: String,
    cdn_base_url: Option<String>,
    region: String,
    endpoint: Option<String>,
    force_path_style: bool,
    send_sse_aes256: bool,
    max_attempts: u32,
    readiness_object_key: Option<String>,
    /// Development-only escape hatch. Production readiness requires a remote
    /// object-level probe rather than merely proving that a request can sign.
    allow_signing_only_readiness: bool,
    /// Allows pre-fingerprint rows only after an operator has confirmed that
    /// they belong to the currently configured backend. Mismatched marked rows
    /// are never accepted.
    allow_unmarked_storage_history: bool,
    /// Non-secret hash of backend kind, endpoint, region, and bucket. New rows
    /// carry this value so a global-client cutover cannot misroute old objects.
    backend_fingerprint: String,
    /// R2 has no object versioning. Native/custom S3 must be explicitly
    /// declared unversioned because key-only deletes do not purge old versions.
    versioning_mode: &'static str,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    session_token: Option<String>,
    backend: ObjectStorageBackend,
    validation_errors: Vec<String>,
}

impl S3StorageConfig {
    fn is_configured(&self) -> bool {
        !self.bucket.is_empty() && self.validation_errors.is_empty()
    }

    fn readiness_probe_mode(&self) -> &'static str {
        if self.readiness_object_key.is_some() {
            "head_object"
        } else if self.allow_signing_only_readiness {
            "signing_dev_only"
        } else {
            "remote_probe_not_configured"
        }
    }
}

#[derive(Clone)]
struct OAuthProviderConfig {
    client_id: Option<String>,
    client_secret: Option<String>,
    authorization_url: Option<String>,
    token_url: Option<String>,
}

#[derive(Clone)]
struct CloudTokenSealer {
    cipher: Arc<Aes256Gcm>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SealedTokenEnvelope {
    ciphertext_b64: String,
    nonce_b64: String,
    aad_tag: String,
    version: i32,
}

#[derive(Debug)]
enum ServiceError {
    BadRequest(String),
    Unauthorized,
    MfaRequired,
    NotFound(String),
    Conflict(String),
    Unavailable(String),
    Internal(String),
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        let (status, error, message) = match self {
            ServiceError::BadRequest(message) => (StatusCode::BAD_REQUEST, "bad_request", message),
            ServiceError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "authentication required".to_string(),
            ),
            ServiceError::MfaRequired => (
                StatusCode::FORBIDDEN,
                "mfa_required",
                "a verified second factor is required".to_string(),
            ),
            ServiceError::NotFound(message) => (StatusCode::NOT_FOUND, "not_found", message),
            ServiceError::Conflict(message) => (StatusCode::CONFLICT, "conflict", message),
            ServiceError::Unavailable(message) => {
                (StatusCode::SERVICE_UNAVAILABLE, "unavailable", message)
            }
            ServiceError::Internal(message) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
            }
        };
        let mut response = (
            status,
            Json(json!({ "ok": false, "error": error, "message": message })),
        )
            .into_response();
        if status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                "Bearer realm=\"sound-recorder\""
                    .parse()
                    .expect("static header is valid"),
            );
        }
        response
    }
}

#[derive(Clone)]
struct DeviceAuth {
    account_id: String,
    device_id: String,
    install_id: String,
    retention_hours: i32,
}

#[derive(Clone, Debug)]
struct SharedAuthIdentity {
    subject: String,
    email: Option<String>,
}

impl SharedAuthIdentity {
    fn external_subject(&self) -> String {
        format!("shared-auth:{}", self.subject)
    }
}

#[derive(Debug, Deserialize)]
struct SharedAuthIntrospection {
    active: bool,
    #[serde(default)]
    sub: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
    #[serde(default = "default_auth_assurance_level")]
    aal: u8,
    #[serde(default)]
    amr: Vec<String>,
    #[serde(default)]
    acr: Option<String>,
    #[serde(default)]
    iat: Option<u64>,
}

fn default_auth_assurance_level() -> u8 {
    1
}

fn validate_shared_auth_assurance(
    introspection: &SharedAuthIntrospection,
    required_aal: u8,
    now: u64,
) -> Result<(), ServiceError> {
    if introspection.aal < required_aal {
        return Err(ServiceError::MfaRequired);
    }
    if required_aal < 2 {
        return Ok(());
    }
    if introspection.acr.as_deref() != Some(SHARED_AUTH_REQUIRED_ACR)
        || introspection.amr.is_empty()
        || introspection.amr.len() > SHARED_AUTH_MAX_AMR_METHODS
    {
        return Err(ServiceError::MfaRequired);
    }

    let mut methods = Vec::with_capacity(introspection.amr.len());
    for raw in &introspection.amr {
        let method = raw.trim().to_ascii_lowercase();
        if method.is_empty()
            || method.len() > SHARED_AUTH_MAX_AMR_METHOD_BYTES
            || !method.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_:-./".contains(&byte)
            })
            || methods.contains(&method)
        {
            return Err(ServiceError::MfaRequired);
        }
        methods.push(method);
    }

    if methods
        .iter()
        .any(|method| matches!(method.as_str(), "pwd" | "password"))
    {
        return Err(ServiceError::MfaRequired);
    }
    let passwordless_primary = methods.iter().any(|method| {
        matches!(
            method.as_str(),
            "federated" | "email_otp" | "otp" | "magiclink" | "magic_link" | "email/signup"
        )
    });
    let strong_second = methods
        .iter()
        .any(|method| matches!(method.as_str(), "totp" | "sms_otp" | "passkey" | "webauthn"));
    if !passwordless_primary || !strong_second {
        return Err(ServiceError::MfaRequired);
    }

    let issued_at = introspection.iat.ok_or(ServiceError::MfaRequired)?;
    if issued_at > now.saturating_add(SHARED_AUTH_CLOCK_SKEW_SECONDS)
        || now.saturating_sub(issued_at) > SHARED_AUTH_MAX_AUTH_AGE_SECONDS
    {
        return Err(ServiceError::MfaRequired);
    }
    Ok(())
}

#[cfg(test)]
mod shared_auth_assurance_tests {
    use super::*;

    const NOW: u64 = 2_000_000_000;

    fn claims(
        amr: &[&str],
        aal: u8,
        acr: Option<&str>,
        iat: Option<u64>,
    ) -> SharedAuthIntrospection {
        SharedAuthIntrospection {
            active: true,
            sub: Some(Uuid::nil().to_string()),
            email: None,
            email_verified: false,
            aal,
            amr: amr.iter().map(|method| (*method).to_string()).collect(),
            acr: acr.map(str::to_string),
            iat,
        }
    }

    #[test]
    fn accepts_passwordless_primary_with_approved_independent_second_factor() {
        for methods in [
            vec!["federated", "totp"],
            vec!["email_otp", "sms_otp"],
            vec!["federated", "passkey"],
        ] {
            let value = claims(&methods, 2, Some(SHARED_AUTH_REQUIRED_ACR), Some(NOW - 30));
            assert!(validate_shared_auth_assurance(&value, 2, NOW).is_ok());
        }
    }

    #[test]
    fn rejects_numeric_aal2_without_the_canonical_method_chain() {
        for methods in [
            vec![],
            vec!["federated"],
            vec!["email_otp"],
            vec!["pwd", "totp"],
            vec!["federated", "email_otp"],
            vec!["federated", "totp", "totp"],
        ] {
            let value = claims(&methods, 2, Some(SHARED_AUTH_REQUIRED_ACR), Some(NOW - 30));
            assert!(matches!(
                validate_shared_auth_assurance(&value, 2, NOW),
                Err(ServiceError::MfaRequired)
            ));
        }
    }

    #[test]
    fn rejects_missing_wrong_stale_or_future_assurance_context() {
        let missing_acr = claims(&["federated", "totp"], 2, None, Some(NOW));
        let stale = claims(
            &["federated", "totp"],
            2,
            Some(SHARED_AUTH_REQUIRED_ACR),
            Some(NOW - SHARED_AUTH_MAX_AUTH_AGE_SECONDS - 1),
        );
        let future = claims(
            &["federated", "totp"],
            2,
            Some(SHARED_AUTH_REQUIRED_ACR),
            Some(NOW + SHARED_AUTH_CLOCK_SKEW_SECONDS + 1),
        );
        let missing_iat = claims(
            &["federated", "totp"],
            2,
            Some(SHARED_AUTH_REQUIRED_ACR),
            None,
        );
        for value in [missing_acr, stale, future, missing_iat] {
            assert!(matches!(
                validate_shared_auth_assurance(&value, 2, NOW),
                Err(ServiceError::MfaRequired)
            ));
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    ok: bool,
    service: &'static str,
    mode: &'static str,
    postgres_configured: bool,
    s3_configured: bool,
    storage_ready: Option<bool>,
    storage_history_compatible: Option<bool>,
    storage_probe_mode: &'static str,
    storage_backend: &'static str,
    storage_backend_fingerprint: String,
    storage_versioning_mode: &'static str,
    configuration_valid: bool,
    token_pepper_configured: bool,
    registration_configured: bool,
    server_auth_configured: bool,
    cloud_token_sealer_configured: bool,
    google_drive_configured: bool,
    microsoft_onedrive_configured: bool,
    dropbox_configured: bool,
    supabase_configured: bool,
    supabase_data_api_configured: bool,
    supabase_accounts_configured: bool,
    supabase_ready: Option<bool>,
    supabase_required: bool,
    shared_auth_configured: bool,
    shared_auth_required_aal: u8,
    retention_hours: i32,
    mirror_configured: bool,
    mirror_ready: Option<bool>,
    mirror_probe_mode: &'static str,
    mirror_backend: Option<&'static str>,
    mirror_backend_fingerprint: Option<String>,
    mirror_readiness_required: bool,
}

#[derive(Deserialize)]
struct UserDataQuery {
    limit: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UserDataList<T> {
    count: usize,
    data: Vec<T>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct UserSettingsInput {
    preferred_use_case: String,
    device_retention_hours: i64,
    cloud_retention_hours: i64,
    segment_minutes: i64,
    overlap_seconds: i64,
    bit_rate: i64,
    sample_rate: i64,
    channels: i64,
    upload_enabled: bool,
    cloud_provider: String,
    mic_sensitivity: f64,
    noise_trigger_sensitivity: f64,
    bass_gain_db: f64,
    mid_gain_db: f64,
    treble_gain_db: f64,
    auto_gain: bool,
    noise_suppress: bool,
    acoustic_analysis_enabled: bool,
    analysis_activation_db: f64,
    analysis_sustain_seconds: f64,
    analysis_hold_seconds: f64,
    snore_detection_enabled: bool,
    sleep_analysis_enabled: bool,
    music_detection_enabled: bool,
    speech_detection_enabled: bool,
    adaptive_quality_enabled: bool,
    capture_sample_rate: i64,
    quiet_sample_rate: i64,
    adaptive_loudness_db: f64,
}

impl Default for UserSettingsInput {
    fn default() -> Self {
        Self {
            preferred_use_case: "security".to_string(),
            device_retention_hours: 100,
            cloud_retention_hours: 500,
            segment_minutes: 1,
            overlap_seconds: 2,
            bit_rate: 64_000,
            sample_rate: 16_000,
            channels: 1,
            upload_enabled: false,
            cloud_provider: "s3".to_string(),
            mic_sensitivity: 1.0,
            noise_trigger_sensitivity: 0.5,
            bass_gain_db: 0.0,
            mid_gain_db: 0.0,
            treble_gain_db: 0.0,
            auto_gain: true,
            noise_suppress: true,
            acoustic_analysis_enabled: false,
            analysis_activation_db: -40.0,
            analysis_sustain_seconds: 2.0,
            analysis_hold_seconds: 45.0,
            snore_detection_enabled: true,
            sleep_analysis_enabled: false,
            music_detection_enabled: true,
            speech_detection_enabled: true,
            adaptive_quality_enabled: false,
            capture_sample_rate: 48_000,
            quiet_sample_rate: 16_000,
            adaptive_loudness_db: -40.0,
        }
    }
}

impl UserSettingsInput {
    fn validate(&self) -> Result<(), ServiceError> {
        validate_choice(
            "preferred_use_case",
            &self.preferred_use_case,
            USER_SETTINGS_PREFERRED_USE_CASE_VALUES,
        )?;
        validate_integer_range(
            "device_retention_hours",
            self.device_retention_hours,
            1,
            500,
        )?;
        validate_integer_range(
            "cloud_retention_hours",
            self.cloud_retention_hours,
            1,
            2_000,
        )?;
        if self.cloud_retention_hours < self.device_retention_hours {
            return Err(ServiceError::BadRequest(
                "cloud_retention_hours must not be shorter than device_retention_hours".to_string(),
            ));
        }
        validate_integer_range("segment_minutes", self.segment_minutes, 1, 60)?;
        validate_integer_range("overlap_seconds", self.overlap_seconds, 0, 30)?;
        if self.overlap_seconds >= self.segment_minutes * 60 {
            return Err(ServiceError::BadRequest(
                "overlap_seconds must be shorter than the segment".to_string(),
            ));
        }
        validate_integer_range("bit_rate", self.bit_rate, 16_000, 320_000)?;
        validate_sample_rate("sample_rate", self.sample_rate)?;
        validate_integer_range("channels", self.channels, 1, 2)?;
        validate_choice(
            "cloud_provider",
            &self.cloud_provider,
            USER_SETTINGS_CLOUD_PROVIDER_VALUES,
        )?;
        validate_float_range("mic_sensitivity", self.mic_sensitivity, 0.25, 4.0)?;
        validate_float_range(
            "noise_trigger_sensitivity",
            self.noise_trigger_sensitivity,
            0.0,
            1.0,
        )?;
        for (name, value) in [
            ("bass_gain_db", self.bass_gain_db),
            ("mid_gain_db", self.mid_gain_db),
            ("treble_gain_db", self.treble_gain_db),
        ] {
            validate_float_range(name, value, -12.0, 12.0)?;
        }
        validate_float_range(
            "analysis_activation_db",
            self.analysis_activation_db,
            -90.0,
            0.0,
        )?;
        validate_float_range(
            "analysis_sustain_seconds",
            self.analysis_sustain_seconds,
            0.5,
            30.0,
        )?;
        validate_float_range(
            "analysis_hold_seconds",
            self.analysis_hold_seconds,
            0.0,
            600.0,
        )?;
        validate_sample_rate("capture_sample_rate", self.capture_sample_rate)?;
        validate_sample_rate("quiet_sample_rate", self.quiet_sample_rate)?;
        if self.quiet_sample_rate > self.capture_sample_rate {
            return Err(ServiceError::BadRequest(
                "quiet_sample_rate must not exceed capture_sample_rate".to_string(),
            ));
        }
        validate_float_range(
            "adaptive_loudness_db",
            self.adaptive_loudness_db,
            -90.0,
            0.0,
        )?;
        Ok(())
    }

    fn into_interface(self, user_id: String, updated_at: String) -> UserSettings {
        UserSettings {
            user_id,
            preferred_use_case: self.preferred_use_case,
            device_retention_hours: self.device_retention_hours,
            cloud_retention_hours: self.cloud_retention_hours,
            segment_minutes: self.segment_minutes,
            overlap_seconds: self.overlap_seconds,
            bit_rate: self.bit_rate,
            sample_rate: self.sample_rate,
            channels: self.channels,
            upload_enabled: self.upload_enabled,
            cloud_provider: self.cloud_provider,
            mic_sensitivity: self.mic_sensitivity,
            noise_trigger_sensitivity: self.noise_trigger_sensitivity,
            bass_gain_db: self.bass_gain_db,
            mid_gain_db: self.mid_gain_db,
            treble_gain_db: self.treble_gain_db,
            auto_gain: self.auto_gain,
            noise_suppress: self.noise_suppress,
            acoustic_analysis_enabled: self.acoustic_analysis_enabled,
            analysis_activation_db: self.analysis_activation_db,
            analysis_sustain_seconds: self.analysis_sustain_seconds,
            analysis_hold_seconds: self.analysis_hold_seconds,
            snore_detection_enabled: self.snore_detection_enabled,
            sleep_analysis_enabled: self.sleep_analysis_enabled,
            music_detection_enabled: self.music_detection_enabled,
            speech_detection_enabled: self.speech_detection_enabled,
            adaptive_quality_enabled: self.adaptive_quality_enabled,
            capture_sample_rate: self.capture_sample_rate,
            quiet_sample_rate: self.quiet_sample_rate,
            adaptive_loudness_db: self.adaptive_loudness_db,
            updated_at,
        }
    }
}

#[derive(Serialize)]
struct UserSettingsResponse {
    data: UserSettings,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterDeviceRequest {
    platform: String,
    install_id: String,
    device_label: Option<String>,
    app_version: Option<String>,
    os_version: Option<String>,
    external_subject: Option<String>,
    display_name: Option<String>,
    legal_region: Option<String>,
    consent_version: String,
    consent_accepted_at: Option<DateTime<Utc>>,
    recording_indicator_acknowledged: bool,
    attestation: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisterDeviceResponse {
    ok: bool,
    account_id: String,
    device_id: String,
    device_token: String,
    policy: MobilePolicy,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceHeartbeatResponse {
    ok: bool,
    device_id: String,
    server_time: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RevokeDeviceResponse {
    ok: bool,
    install_id: String,
    backend_tokens_revoked: usize,
}

#[derive(Deserialize)]
struct SupabaseVisibleDevice {
    user_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteAccountResponse {
    ok: bool,
    account_id: String,
    deleted_segments: u64,
    deleted_objects: u64,
    revoked_devices: u64,
    revoked_cloud_connections: u64,
    supabase_auth_deleted: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateTransferStateRequest {
    paused: bool,
    reason: Option<String>,
    network_policy: Option<String>,
    battery_level: Option<i32>,
    charging: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateTransferStateResponse {
    ok: bool,
    transfer_paused: bool,
    network_policy: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MobilePolicy {
    retention_hours: i32,
    default_segment_seconds: i32,
    max_segment_seconds: i32,
    max_segment_bytes: i32,
    upload_url_ttl_seconds: u64,
    download_url_ttl_seconds: u64,
    cloud_copy_supported_providers: Vec<&'static str>,
    supported_use_cases: Vec<&'static str>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateUploadSessionRequest {
    content_type: Option<String>,
    codec: Option<String>,
    sample_rate: Option<i32>,
    channel_count: Option<i32>,
    segment_duration_seconds: Option<i32>,
    max_segment_bytes: Option<i32>,
    client_timezone: Option<String>,
    legal_region: Option<String>,
    /// Capture intent: `security` (default), `music`, `meeting`, `voice_note`,
    /// or `ambient`. Drives client-side defaults (e.g. stereo high-fidelity for
    /// music) and is recorded for playback/audit.
    use_case: Option<String>,
    /// Optional client audio-tuning snapshot (sensitivity, treble/mid/bass gain,
    /// channel layout). Stored verbatim alongside session metadata.
    audio_profile: Option<Value>,
    meta_data: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateUploadSessionResponse {
    ok: bool,
    session: UploadSessionResponse,
    policy: MobilePolicy,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadSessionResponse {
    id: String,
    account_id: String,
    device_id: String,
    status: String,
    storage_prefix: String,
    content_type: String,
    codec: Option<String>,
    segment_duration_seconds: i32,
    max_segment_bytes: i32,
    started_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresignSegmentRequest {
    sequence_number: i32,
    captured_started_at: DateTime<Utc>,
    duration_millis: i32,
    content_type: Option<String>,
    codec: Option<String>,
    byte_count: Option<i32>,
    sha256_hex: Option<String>,
    meta_data: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PresignSegmentResponse {
    ok: bool,
    segment: SegmentResponse,
    upload: PresignedTransfer,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompleteSegmentRequest {
    etag: Option<String>,
    byte_count: Option<i32>,
    sha256_hex: Option<String>,
    captured_ended_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CompleteSegmentResponse {
    ok: bool,
    segment: SegmentResponse,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HeartbeatResponse {
    ok: bool,
    session_id: String,
    next_sequence_number: i32,
    retention_cutoff: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CloseSessionResponse {
    ok: bool,
    session_id: String,
    status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimelineQuery {
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    limit: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TimelineResponse {
    ok: bool,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    segments: Vec<SegmentResponse>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceExportRequest {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    device_id: Option<String>,
    max_segments: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceExportResponse {
    ok: bool,
    export_id: String,
    expires_at: DateTime<Utc>,
    segment_count: usize,
    segments: Vec<EvidenceSegmentLink>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermanentSaveRequest {
    provider: Option<String>,
    range_started_at: Option<DateTime<Utc>>,
    range_ended_at: Option<DateTime<Utc>>,
    #[serde(default)]
    segments: Vec<PermanentSaveSegmentRef>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermanentSaveSegmentRef {
    id: Option<String>,
    storage_key: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PermanentSaveResponse {
    ok: bool,
    saved_count: usize,
    segments: Vec<PermanentSaveSegmentResult>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PermanentSaveSegmentResult {
    id: Option<String>,
    storage_key: String,
    permanent_storage_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AlertRequest {
    trigger: String,
    occurred_at: DateTime<Utc>,
    listen_offset_seconds: Option<i64>,
    email_to: Option<String>,
    segment_id: Option<String>,
    sequence_number: Option<i32>,
    meta_data: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AlertResponse {
    ok: bool,
    alert_id: String,
    emailed: bool,
    email_to: String,
    listen_url: Option<String>,
    listen_from: DateTime<Utc>,
    listen_to: DateTime<Utc>,
    segment_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceSegmentLink {
    segment: SegmentResponse,
    download: PresignedTransfer,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SegmentResponse {
    id: String,
    account_id: String,
    device_id: String,
    session_id: String,
    sequence_number: i32,
    status: String,
    storage_provider: String,
    storage_bucket: String,
    storage_key: String,
    cdn_url: Option<String>,
    content_type: String,
    codec: Option<String>,
    captured_started_at: DateTime<Utc>,
    captured_ended_at: Option<DateTime<Utc>>,
    duration_millis: i32,
    byte_count: Option<i32>,
    sha256_hex: Option<String>,
    upload_url_expires_at: Option<DateTime<Utc>>,
    uploaded_at: Option<DateTime<Utc>>,
    expires_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PresignedTransfer {
    method: String,
    url: String,
    headers: Vec<SignedHeader>,
    expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct VerifiedStoredObject {
    byte_count: i32,
    etag: String,
}

#[derive(Clone, Copy, Debug)]
struct StoredObjectMetadata<'a> {
    content_length: Option<i64>,
    content_type: Option<&'a str>,
    etag: Option<&'a str>,
}

#[derive(Clone, Copy, Debug)]
struct StoredObjectExpectation<'a> {
    content_type: &'a str,
    presigned_byte_count: Option<i32>,
    reported_byte_count: Option<i32>,
    reported_etag: Option<&'a str>,
    max_segment_bytes: i32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignedHeader {
    name: String,
    value: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RetentionSweepResponse {
    ok: bool,
    expired_segments: u64,
    deleted_objects: u64,
    delete_failures: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartCloudLinkRequest {
    provider: String,
    redirect_uri: Option<String>,
    folder_path: Option<String>,
    root_folder_id: Option<String>,
    display_name: Option<String>,
    meta_data: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartCloudLinkResponse {
    ok: bool,
    provider: String,
    link_mode: String,
    state: String,
    authorization_url: Option<String>,
    expires_at: DateTime<Utc>,
    required_scope: Option<&'static str>,
    client_managed: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompleteCloudLinkRequest {
    provider: String,
    state: String,
    authorization_code: Option<String>,
    redirect_uri: Option<String>,
    display_name: Option<String>,
    provider_account_id: Option<String>,
    root_folder_id: Option<String>,
    folder_path: Option<String>,
    client_managed_acknowledged: Option<bool>,
    // Supabase-brokered OAuth tokens. When the client signs the user into the
    // cloud provider through Supabase, it forwards the provider token here and
    // the server seals it directly instead of exchanging an authorization code.
    provider_access_token: Option<String>,
    provider_refresh_token: Option<String>,
    provider_token_expires_in: Option<i64>,
    provider_token_type: Option<String>,
    provider_token_scope: Option<String>,
    meta_data: Option<Value>,
}

/// OAuth query returned by Google, Microsoft, or Dropbox to the hosted
/// callback registered for the Sonus Auris provider clients.
#[derive(Debug, Default, Deserialize)]
struct CloudOAuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CompleteCloudLinkResponse {
    ok: bool,
    connection: CloudConnectionResponse,
    backfilled_jobs: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListCloudConnectionsResponse {
    ok: bool,
    connections: Vec<CloudConnectionResponse>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudConnectionResponse {
    id: String,
    provider: String,
    link_mode: String,
    status: String,
    display_name: Option<String>,
    provider_account_id: Option<String>,
    root_folder_id: Option<String>,
    folder_path: String,
    token_expires_at: Option<DateTime<Utc>>,
    last_sync_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RevokeCloudConnectionResponse {
    ok: bool,
    connection_id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_authorization_revoked: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListCloudCopyJobsQuery {
    provider: Option<String>,
    limit: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListCloudCopyJobsResponse {
    ok: bool,
    jobs: Vec<CloudCopyJobWithDownload>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudCopyJobWithDownload {
    job: CloudCopyJobResponse,
    segment: SegmentResponse,
    download: PresignedTransfer,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudCopyJobResponse {
    id: String,
    connection_id: String,
    segment_id: String,
    provider: String,
    status: String,
    destination_key: String,
    provider_file_id: Option<String>,
    attempts: i32,
    completed_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompleteCloudCopyJobRequest {
    provider_file_id: Option<String>,
    destination_key: Option<String>,
    meta_data: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CompleteCloudCopyJobResponse {
    ok: bool,
    job: CloudCopyJobResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DrainCloudCopyRequest {
    max_jobs: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DrainCloudCopyResponse {
    ok: bool,
    attempted: usize,
    completed: usize,
    failed: usize,
    skipped: usize,
    results: Vec<CloudCopyDrainResult>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DrainCloudConnectionProjectionsRequest {
    max_items: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DrainCloudConnectionProjectionsResponse {
    ok: bool,
    attempted: usize,
    completed: usize,
    failed: usize,
    skipped: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudCopyDrainResult {
    job_id: String,
    provider: String,
    status: String,
    message: Option<String>,
}

struct CloudConnectionProjectionClaim {
    seq: i64,
    attempts: i32,
    external_subject: String,
    connection: CloudConnectionRecord,
}

struct SessionPolicy {
    status: String,
    storage_bucket: String,
    storage_prefix: String,
    storage_fingerprint: Option<String>,
    content_type: String,
    codec: Option<String>,
    segment_duration_seconds: i32,
    max_segment_bytes: i32,
}

#[derive(Clone)]
struct CloudConnectionRecord {
    id: String,
    account_id: String,
    provider: String,
    link_mode: String,
    status: String,
    display_name: Option<String>,
    provider_account_id: Option<String>,
    root_folder_id: Option<String>,
    folder_path: String,
    token_ciphertext: Option<String>,
    token_nonce: Option<String>,
    token_aad: Option<String>,
    token_version: Option<i32>,
    token_expires_at: Option<DateTime<Utc>>,
    last_sync_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Clone)]
struct CloudCopyJobRecord {
    id: String,
    provider: String,
    destination_key: String,
}

struct CloudCopyWorkItem {
    job: CloudCopyJobRecord,
    connection: CloudConnectionRecord,
    segment: SegmentResponse,
}

/// The monotonically increasing attempt number returned while claiming a job.
/// It doubles as a fencing token: a worker may only finalize the exact attempt
/// it claimed, so a worker that outlives its lease cannot overwrite a later
/// worker's result.
struct CloudCopyClaim {
    attempts: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudTokenSet {
    access_token: String,
    refresh_token: Option<String>,
    token_type: Option<String>,
    scope: Option<String>,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    token_type: Option<String>,
    scope: Option<String>,
    expires_in: Option<i64>,
    error: Option<String>,
    error_description: Option<String>,
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_bool_value(name: &str, value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(format!(
            "{name} must be one of true/false, 1/0, yes/no, or on/off"
        )),
    }
}

fn env_bool(name: &str, default: bool, validation_errors: &mut Vec<String>) -> bool {
    match env::var(name) {
        Ok(value) => match parse_bool_value(name, &value) {
            Ok(value) => value,
            Err(error) => {
                validation_errors.push(error);
                default
            }
        },
        Err(env::VarError::NotPresent) => default,
        Err(env::VarError::NotUnicode(_)) => {
            validation_errors.push(format!("{name} must contain valid UTF-8"));
            default
        }
    }
}

fn env_i32(name: &str, default: i32) -> i32 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_i64(name: &str, default: i64) -> i64 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

/// Like [`env_u64`] but `0` is a meaningful value (it disables the rate
/// limiter), so — unlike the other readers — we do not discard zero.
fn env_u32_allow_zero(name: &str, default: u32) -> u32 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(default)
}

fn env_duration_clamped(name: &str, default: u64, min: u64, max: u64) -> Duration {
    Duration::from_secs(env_u64(name, default).clamp(min, max))
}

fn env_i64_clamped(name: &str, default: i64, min: i64, max: i64) -> i64 {
    env_i64(name, default).clamp(min, max)
}

fn is_valid_r2_account_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_service_url(value: &str, root_path_only: bool) -> bool {
    let Ok(url) = reqwest::Url::parse(value.trim()) else {
        return false;
    };
    let safe_scheme = match url.scheme() {
        "https" => url.host_str().is_some(),
        "http" => matches!(
            url.host_str(),
            Some("localhost") | Some("127.0.0.1") | Some("::1")
        ),
        _ => false,
    };
    safe_scheme
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && (!root_path_only || matches!(url.path(), "" | "/"))
}

fn validate_shared_auth_url(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value.trim()) else {
        return false;
    };
    let host = url.host_str().unwrap_or_default();
    let internal_http_host = matches!(host, "localhost" | "127.0.0.1" | "::1")
        || host.ends_with(".svc")
        || host.ends_with(".svc.cluster.local")
        || (!host.is_empty()
            && !host.contains('.')
            && host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'));
    (matches!(url.scheme(), "https") || (url.scheme() == "http" && internal_http_host))
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && matches!(url.path(), "" | "/")
}

fn optional_service_url_from_env(
    names: &[&str],
    validation_errors: &mut Vec<String>,
) -> Option<String> {
    let value = first_env(names)?;
    if validate_service_url(&value, false) {
        Some(value)
    } else {
        validation_errors.push(format!(
            "{} must be HTTPS (or loopback HTTP) without credentials, query, or fragment",
            names[0]
        ));
        None
    }
}

fn shared_auth_config_from_env() -> SharedAuthConfig {
    let mut validation_errors = Vec::new();
    let mut base_url = first_env(&["SOUND_RECORDER_SHARED_AUTH_BASE_URL"])
        .map(|url| url.trim_end_matches('/').to_string());
    if base_url
        .as_deref()
        .is_some_and(|url| !validate_shared_auth_url(url))
    {
        validation_errors.push(
            "SOUND_RECORDER_SHARED_AUTH_BASE_URL must be HTTPS, loopback HTTP, or an in-cluster HTTP service root without credentials, query, or fragment"
                .to_string(),
        );
        base_url = None;
    }
    let introspect_secret = first_env(&["SOUND_RECORDER_SHARED_AUTH_INTROSPECT_SECRET"]);
    if introspect_secret
        .as_ref()
        .is_some_and(|secret| secret.len() < 32)
    {
        validation_errors.push(
            "SOUND_RECORDER_SHARED_AUTH_INTROSPECT_SECRET must contain at least 32 bytes"
                .to_string(),
        );
    }
    if base_url.is_some() != introspect_secret.is_some() {
        validation_errors.push(
            "SOUND_RECORDER_SHARED_AUTH_BASE_URL and SOUND_RECORDER_SHARED_AUTH_INTROSPECT_SECRET must be configured together"
                .to_string(),
        );
    }
    let required_aal = match first_env(&["SOUND_RECORDER_SHARED_AUTH_REQUIRED_AAL"]) {
        Some(value) => match value.parse::<u8>() {
            Ok(value @ 1..=2) => value,
            _ => {
                validation_errors
                    .push("SOUND_RECORDER_SHARED_AUTH_REQUIRED_AAL must be 1 or 2".to_string());
                2
            }
        },
        None => 2,
    };
    SharedAuthConfig {
        base_url,
        introspect_secret,
        required_aal,
        validation_errors,
    }
}

fn service_url_from_env(
    names: &[&str],
    default: &str,
    validation_errors: &mut Vec<String>,
) -> String {
    optional_service_url_from_env(names, validation_errors).unwrap_or_else(|| default.to_string())
}

fn urls_have_same_origin(left: &str, right: &str) -> bool {
    let (Ok(left), Ok(right)) = (reqwest::Url::parse(left), reqwest::Url::parse(right)) else {
        return false;
    };
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn storage_backend_for_endpoint(endpoint: Option<&str>) -> ObjectStorageBackend {
    let Some(endpoint) = endpoint else {
        return ObjectStorageBackend::AmazonS3;
    };
    let host = reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase));
    if host
        .as_deref()
        .is_some_and(|host| host.ends_with(".r2.cloudflarestorage.com"))
    {
        ObjectStorageBackend::CloudflareR2
    } else {
        ObjectStorageBackend::S3Compatible
    }
}

fn storage_backend_fingerprint(
    backend: ObjectStorageBackend,
    endpoint: Option<&str>,
    region: &str,
    bucket: &str,
) -> String {
    let endpoint = endpoint.unwrap_or("aws-default-endpoint");
    let identity = format!("{}|{endpoint}|{region}|{bucket}", backend.as_str());
    Sha256::digest(identity.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn unmarked_history_acknowledgment(
    requested: bool,
    acknowledged_fingerprint: Option<&str>,
    current_fingerprint: &str,
) -> Result<bool, String> {
    if !requested {
        return Ok(false);
    }
    if acknowledged_fingerprint == Some(current_fingerprint) {
        Ok(true)
    } else {
        Err(
            "SOUND_RECORDER_ALLOW_UNMARKED_STORAGE_HISTORY=true requires SOUND_RECORDER_UNMARKED_STORAGE_HISTORY_FINGERPRINT to exactly match the current /healthz fingerprint"
                .to_string(),
        )
    }
}

fn s3_storage_config_from_env() -> S3StorageConfig {
    let mut validation_errors = Vec::new();
    let r2_account_id = first_env(&[
        "SOUND_RECORDER_R2_ACCOUNT_ID",
        "CLOUDFLARE_R2_ACCOUNT_ID",
        "R2_ACCOUNT_ID",
    ]);
    if r2_account_id
        .as_deref()
        .is_some_and(|account_id| !is_valid_r2_account_id(account_id))
    {
        validation_errors.push(
            "SOUND_RECORDER_R2_ACCOUNT_ID must be a 32-character hexadecimal account id"
                .to_string(),
        );
    }
    let derived_r2_endpoint = r2_account_id
        .as_deref()
        .filter(|account_id| is_valid_r2_account_id(account_id))
        .map(|account_id| format!("https://{account_id}.r2.cloudflarestorage.com"));
    let endpoint = first_env(&[
        "SOUND_RECORDER_S3_ENDPOINT",
        "SOUND_RECORDER_R2_ENDPOINT",
        "CLOUDFLARE_R2_ENDPOINT",
        "R2_ENDPOINT",
        "S3_ENDPOINT",
        "AWS_ENDPOINT_URL_S3",
        "AWS_ENDPOINT_URL",
    ])
    .or(derived_r2_endpoint)
    .map(|endpoint| endpoint.trim_end_matches('/').to_string());
    if endpoint
        .as_deref()
        .is_some_and(|endpoint| !validate_service_url(endpoint, true))
    {
        validation_errors.push(
            "SOUND_RECORDER_S3_ENDPOINT must be HTTPS (or loopback HTTP) with no path, credentials, query, or fragment"
                .to_string(),
        );
    }
    let backend = storage_backend_for_endpoint(endpoint.as_deref());
    let region = if backend == ObjectStorageBackend::CloudflareR2 {
        // R2 requires `auto`; us-east-1 is accepted as an alias, but signing
        // explicitly with `auto` matches Cloudflare's SDK guidance.
        "auto".to_string()
    } else {
        first_env(&[
            "SOUND_RECORDER_S3_REGION",
            "R2_REGION",
            "S3_REGION",
            "AWS_REGION",
            "AWS_DEFAULT_REGION",
        ])
        .unwrap_or_else(|| "us-east-1".to_string())
    };
    if region.len() > 80
        || !region
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        validation_errors.push("SOUND_RECORDER_S3_REGION is invalid".to_string());
    }

    let bucket = first_env(&[
        "SOUND_RECORDER_S3_BUCKET",
        "SOUND_RECORDER_R2_BUCKET",
        "S3_BUCKET",
        "R2_BUCKET",
    ])
    .unwrap_or_default();
    if !bucket.is_empty()
        && (bucket.len() > 200
            || bucket
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace() || matches!(ch, '/' | '\\')))
    {
        validation_errors.push(
            "SOUND_RECORDER_S3_BUCKET must be 1-200 characters without whitespace or slashes"
                .to_string(),
        );
    }

    let access_key_id = first_env(&[
        "SOUND_RECORDER_S3_ACCESS_KEY_ID",
        "SOUND_RECORDER_R2_ACCESS_KEY_ID",
        "CLOUDFLARE_R2_ACCESS_KEY_ID",
        "R2_ACCESS_KEY_ID",
        "AWS_ACCESS_KEY_ID",
    ]);
    let secret_access_key = first_env(&[
        "SOUND_RECORDER_S3_SECRET_ACCESS_KEY",
        "SOUND_RECORDER_R2_SECRET_ACCESS_KEY",
        "CLOUDFLARE_R2_SECRET_ACCESS_KEY",
        "R2_SECRET_ACCESS_KEY",
        "AWS_SECRET_ACCESS_KEY",
    ]);
    if access_key_id.is_some() != secret_access_key.is_some() {
        validation_errors.push(
            "object-storage access key id and secret access key must be configured together"
                .to_string(),
        );
    }

    let key_prefix = first_env(&["SOUND_RECORDER_S3_KEY_PREFIX", "S3_KEY_PREFIX"])
        .unwrap_or_else(|| "sound-recorder/segments".to_string())
        .trim_matches('/')
        .to_string();
    if key_prefix.is_empty() || key_prefix.len() > 1024 || key_prefix.chars().any(char::is_control)
    {
        validation_errors
            .push("SOUND_RECORDER_S3_KEY_PREFIX must be 1-1024 non-control characters".to_string());
    }
    let readiness_object_key = first_env(&["SOUND_RECORDER_S3_READINESS_OBJECT_KEY"]);
    if readiness_object_key.as_deref().is_some_and(|key| {
        key.len() > 2048
            || key.chars().any(char::is_control)
            || !(key == key_prefix || key.starts_with(&format!("{key_prefix}/")))
    }) {
        validation_errors.push(
            "SOUND_RECORDER_S3_READINESS_OBJECT_KEY must be inside SOUND_RECORDER_S3_KEY_PREFIX"
                .to_string(),
        );
    }
    let cdn_base_url = first_env(&[
        "SOUND_RECORDER_CDN_BASE_URL",
        "SOUND_RECORDER_S3_PUBLIC_BASE_URL",
        "S3_PUBLIC_BASE_URL",
    ]);
    if cdn_base_url
        .as_deref()
        .is_some_and(|url| !validate_service_url(url, false))
    {
        validation_errors.push(
            "SOUND_RECORDER_CDN_BASE_URL must be HTTPS (or loopback HTTP) without credentials, query, or fragment"
                .to_string(),
        );
    }

    let requested_sse = first_env(&["SOUND_RECORDER_S3_SERVER_SIDE_ENCRYPTION"])
        .unwrap_or_else(|| "auto".to_string())
        .to_ascii_lowercase();
    let send_sse_aes256 = match requested_sse.as_str() {
        "auto" => backend == ObjectStorageBackend::AmazonS3,
        "aes256" | "aes-256" => {
            if backend == ObjectStorageBackend::CloudflareR2 {
                validation_errors.push(
                    "SOUND_RECORDER_S3_SERVER_SIDE_ENCRYPTION=aes256 is incompatible with Cloudflare R2; use auto or none"
                        .to_string(),
                );
                false
            } else {
                true
            }
        }
        "none" | "off" | "disabled" => false,
        _ => {
            validation_errors.push(
                "SOUND_RECORDER_S3_SERVER_SIDE_ENCRYPTION must be auto, aes256, or none"
                    .to_string(),
            );
            false
        }
    };

    // Cloudflare documents that R2 does not implement bucket versioning. For
    // AWS/custom S3, require an explicit unversioned declaration: DeleteObject
    // and DeleteObjects only create delete markers in versioned buckets and do
    // not satisfy physical retention/account erasure.
    let requested_versioning = first_env(&["SOUND_RECORDER_S3_VERSIONING_MODE"]);
    let versioning_mode = if backend == ObjectStorageBackend::CloudflareR2 {
        if requested_versioning.as_deref().is_some_and(|value| {
            !matches!(
                value.to_ascii_lowercase().as_str(),
                "unversioned" | "disabled"
            )
        }) {
            validation_errors.push(
                "Cloudflare R2 does not support versioning; SOUND_RECORDER_S3_VERSIONING_MODE must be unversioned"
                    .to_string(),
            );
        }
        "unversioned"
    } else {
        match requested_versioning
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("unversioned" | "disabled") => "unversioned",
            Some("versioned" | "enabled" | "suspended") => {
                validation_errors.push(
                    "versioned or versioning-suspended S3 buckets are unsupported: key-only deletion does not physically erase object versions"
                        .to_string(),
                );
                "unsupported"
            }
            Some(_) => {
                validation_errors.push(
                    "SOUND_RECORDER_S3_VERSIONING_MODE must be unversioned; versioned buckets are unsupported"
                        .to_string(),
                );
                "invalid"
            }
            None => {
                validation_errors.push(
                    "SOUND_RECORDER_S3_VERSIONING_MODE=unversioned must be explicitly set for AWS/custom S3"
                        .to_string(),
                );
                "unknown"
            }
        }
    };
    let force_path_style = env_bool(
        "SOUND_RECORDER_S3_FORCE_PATH_STYLE",
        backend == ObjectStorageBackend::S3Compatible,
        &mut validation_errors,
    );
    let allow_signing_only_readiness = env_bool(
        "SOUND_RECORDER_ALLOW_SIGNING_ONLY_STORAGE_READINESS",
        false,
        &mut validation_errors,
    );
    let requested_allow_unmarked_storage_history = env_bool(
        "SOUND_RECORDER_ALLOW_UNMARKED_STORAGE_HISTORY",
        false,
        &mut validation_errors,
    );
    let backend_fingerprint =
        storage_backend_fingerprint(backend, endpoint.as_deref(), &region, &bucket);
    let history_fingerprint_ack =
        first_env(&["SOUND_RECORDER_UNMARKED_STORAGE_HISTORY_FINGERPRINT"]);
    let allow_unmarked_storage_history = match unmarked_history_acknowledgment(
        requested_allow_unmarked_storage_history,
        history_fingerprint_ack.as_deref(),
        &backend_fingerprint,
    ) {
        Ok(allowed) => allowed,
        Err(error) => {
            validation_errors.push(error);
            false
        }
    };

    S3StorageConfig {
        bucket,
        key_prefix,
        cdn_base_url,
        region,
        endpoint,
        force_path_style,
        send_sse_aes256,
        max_attempts: env_u64(
            "SOUND_RECORDER_S3_MAX_ATTEMPTS",
            DEFAULT_S3_MAX_ATTEMPTS as u64,
        )
        .clamp(1, 10) as u32,
        readiness_object_key,
        allow_signing_only_readiness,
        allow_unmarked_storage_history,
        backend_fingerprint,
        versioning_mode,
        access_key_id,
        secret_access_key,
        session_token: first_env(&[
            "SOUND_RECORDER_S3_SESSION_TOKEN",
            "SOUND_RECORDER_R2_SESSION_TOKEN",
            "R2_SESSION_TOKEN",
            "AWS_SESSION_TOKEN",
        ]),
        backend,
        validation_errors,
    }
}

/// Two storage targets conflict when they resolve to the same backend, endpoint,
/// region, and bucket — a "mirror" that writes back into the primary bucket is
/// not a backup and must be rejected at configuration time.
fn mirror_targets_conflict(primary: &S3StorageConfig, mirror: &S3StorageConfig) -> bool {
    !mirror.bucket.is_empty() && mirror.backend_fingerprint == primary.backend_fingerprint
}

/// Backup/mirror target configuration. Unlike the primary reader, this reads
/// only `SOUND_RECORDER_MIRROR_*` names: the primary's generic `R2_*` / `AWS_*`
/// alias chains are deliberately not consulted, so the mirror can never
/// accidentally inherit the primary's credentials or endpoint. An empty
/// `SOUND_RECORDER_MIRROR_S3_BUCKET` (and no mirror account id) disables the
/// mirror entirely and produces no validation errors.
fn mirror_storage_config_from_env(primary: &S3StorageConfig) -> S3StorageConfig {
    let mut validation_errors = Vec::new();
    let r2_account_id = first_env(&["SOUND_RECORDER_MIRROR_R2_ACCOUNT_ID"]);
    if r2_account_id
        .as_deref()
        .is_some_and(|account_id| !is_valid_r2_account_id(account_id))
    {
        validation_errors.push(
            "SOUND_RECORDER_MIRROR_R2_ACCOUNT_ID must be a 32-character hexadecimal account id"
                .to_string(),
        );
    }
    let derived_r2_endpoint = r2_account_id
        .as_deref()
        .filter(|account_id| is_valid_r2_account_id(account_id))
        .map(|account_id| format!("https://{account_id}.r2.cloudflarestorage.com"));
    let endpoint = first_env(&["SOUND_RECORDER_MIRROR_S3_ENDPOINT"])
        .or(derived_r2_endpoint)
        .map(|endpoint| endpoint.trim_end_matches('/').to_string());
    let bucket = first_env(&["SOUND_RECORDER_MIRROR_S3_BUCKET"]).unwrap_or_default();
    if bucket.is_empty() {
        // Mirror disabled. Ignore every other mirror variable rather than
        // validating a half-configured target into a readiness failure, but
        // do flag a likely operator mistake: credentials without a bucket.
        if endpoint.is_some() || first_env(&["SOUND_RECORDER_MIRROR_S3_ACCESS_KEY_ID"]).is_some() {
            validation_errors.push(
                "mirror storage is partially configured; set SOUND_RECORDER_MIRROR_S3_BUCKET to enable it or unset the other SOUND_RECORDER_MIRROR_* variables"
                    .to_string(),
            );
        }
        return S3StorageConfig {
            bucket: String::new(),
            key_prefix: primary.key_prefix.clone(),
            cdn_base_url: None,
            region: "auto".to_string(),
            endpoint: None,
            force_path_style: false,
            send_sse_aes256: false,
            max_attempts: DEFAULT_S3_MAX_ATTEMPTS,
            readiness_object_key: None,
            allow_signing_only_readiness: false,
            allow_unmarked_storage_history: false,
            backend_fingerprint: String::new(),
            versioning_mode: "unversioned",
            access_key_id: None,
            secret_access_key: None,
            session_token: None,
            backend: ObjectStorageBackend::S3Compatible,
            validation_errors,
        };
    }
    if bucket.len() > 200
        || bucket
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace() || matches!(ch, '/' | '\\'))
    {
        validation_errors.push(
            "SOUND_RECORDER_MIRROR_S3_BUCKET must be 1-200 characters without whitespace or slashes"
                .to_string(),
        );
    }
    if endpoint
        .as_deref()
        .is_some_and(|endpoint| !validate_service_url(endpoint, true))
    {
        validation_errors.push(
            "SOUND_RECORDER_MIRROR_S3_ENDPOINT must be HTTPS (or loopback HTTP) with no path, credentials, query, or fragment"
                .to_string(),
        );
    }
    let backend = storage_backend_for_endpoint(endpoint.as_deref());
    let region = if backend == ObjectStorageBackend::CloudflareR2 {
        "auto".to_string()
    } else {
        first_env(&["SOUND_RECORDER_MIRROR_S3_REGION"]).unwrap_or_else(|| "us-east-1".to_string())
    };
    if region.len() > 80
        || !region
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        validation_errors.push("SOUND_RECORDER_MIRROR_S3_REGION is invalid".to_string());
    }
    let access_key_id = first_env(&["SOUND_RECORDER_MIRROR_S3_ACCESS_KEY_ID"]);
    let secret_access_key = first_env(&["SOUND_RECORDER_MIRROR_S3_SECRET_ACCESS_KEY"]);
    // The primary may fall back to the ambient AWS credential chain; the mirror
    // must not, because in a mixed S3+R2 deployment the ambient chain belongs
    // to the primary. Explicit credentials are therefore required.
    if access_key_id.is_none() || secret_access_key.is_none() {
        validation_errors.push(
            "mirror storage requires explicit SOUND_RECORDER_MIRROR_S3_ACCESS_KEY_ID and SOUND_RECORDER_MIRROR_S3_SECRET_ACCESS_KEY"
                .to_string(),
        );
    }
    let readiness_object_key = first_env(&["SOUND_RECORDER_MIRROR_S3_READINESS_OBJECT_KEY"]);
    if readiness_object_key
        .as_deref()
        .is_some_and(|key| key.len() > 2048 || key.chars().any(char::is_control))
    {
        validation_errors.push(
            "SOUND_RECORDER_MIRROR_S3_READINESS_OBJECT_KEY must be at most 2048 non-control characters"
                .to_string(),
        );
    }
    let requested_sse = first_env(&["SOUND_RECORDER_MIRROR_S3_SERVER_SIDE_ENCRYPTION"])
        .unwrap_or_else(|| "auto".to_string())
        .to_ascii_lowercase();
    let send_sse_aes256 = match requested_sse.as_str() {
        "auto" => backend == ObjectStorageBackend::AmazonS3,
        "aes256" | "aes-256" => {
            if backend == ObjectStorageBackend::CloudflareR2 {
                validation_errors.push(
                    "SOUND_RECORDER_MIRROR_S3_SERVER_SIDE_ENCRYPTION=aes256 is incompatible with Cloudflare R2; use auto or none"
                        .to_string(),
                );
                false
            } else {
                true
            }
        }
        "none" | "off" | "disabled" => false,
        _ => {
            validation_errors.push(
                "SOUND_RECORDER_MIRROR_S3_SERVER_SIDE_ENCRYPTION must be auto, aes256, or none"
                    .to_string(),
            );
            false
        }
    };
    // The mirror carries the same physical-erasure obligation as the primary:
    // retention and account deletion must actually destroy the backup copy, so
    // versioned mirror buckets (delete markers only) are equally unsupported.
    let requested_versioning = first_env(&["SOUND_RECORDER_MIRROR_S3_VERSIONING_MODE"]);
    let versioning_mode = if backend == ObjectStorageBackend::CloudflareR2 {
        if requested_versioning.as_deref().is_some_and(|value| {
            !matches!(
                value.to_ascii_lowercase().as_str(),
                "unversioned" | "disabled"
            )
        }) {
            validation_errors.push(
                "Cloudflare R2 does not support versioning; SOUND_RECORDER_MIRROR_S3_VERSIONING_MODE must be unversioned"
                    .to_string(),
            );
        }
        "unversioned"
    } else {
        match requested_versioning
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("unversioned" | "disabled") => "unversioned",
            Some(_) => {
                validation_errors.push(
                    "SOUND_RECORDER_MIRROR_S3_VERSIONING_MODE must be unversioned; versioned mirror buckets are unsupported"
                        .to_string(),
                );
                "invalid"
            }
            None => {
                validation_errors.push(
                    "SOUND_RECORDER_MIRROR_S3_VERSIONING_MODE=unversioned must be explicitly set for a non-R2 mirror"
                        .to_string(),
                );
                "unknown"
            }
        }
    };
    let force_path_style = env_bool(
        "SOUND_RECORDER_MIRROR_S3_FORCE_PATH_STYLE",
        backend == ObjectStorageBackend::S3Compatible,
        &mut validation_errors,
    );
    let backend_fingerprint =
        storage_backend_fingerprint(backend, endpoint.as_deref(), &region, &bucket);
    let mirror = S3StorageConfig {
        bucket,
        key_prefix: primary.key_prefix.clone(),
        cdn_base_url: None,
        region,
        endpoint,
        force_path_style,
        send_sse_aes256,
        max_attempts: env_u64(
            "SOUND_RECORDER_MIRROR_S3_MAX_ATTEMPTS",
            DEFAULT_S3_MAX_ATTEMPTS as u64,
        )
        .clamp(1, 10) as u32,
        readiness_object_key,
        allow_signing_only_readiness: false,
        allow_unmarked_storage_history: false,
        backend_fingerprint,
        versioning_mode,
        access_key_id,
        secret_access_key,
        session_token: first_env(&["SOUND_RECORDER_MIRROR_S3_SESSION_TOKEN"]),
        backend,
        validation_errors,
    };
    let mut mirror = mirror;
    if mirror_targets_conflict(primary, &mirror) {
        mirror.validation_errors.push(
            "mirror storage must target a different bucket/endpoint than the primary; a mirror into the same bucket is not a backup"
                .to_string(),
        );
    }
    mirror
}

fn config_from_env() -> Config {
    let mut validation_errors = Vec::new();
    let token_pepper = first_env(&["SOUND_RECORDER_DEVICE_TOKEN_PEPPER"]);
    let token_pepper_configured = token_pepper.is_some();
    let token_pepper =
        token_pepper.unwrap_or_else(|| format!("dd-sound-recorder-local-{}", Uuid::new_v4()));

    let mut default_retention_hours = env_i32(
        "SOUND_RECORDER_DEFAULT_RETENTION_HOURS",
        DEFAULT_RETENTION_HOURS,
    );
    default_retention_hours = default_retention_hours.clamp(1, MAX_RETENTION_HOURS);

    let mut max_segment_seconds = env_i32(
        "SOUND_RECORDER_MAX_SEGMENT_SECONDS",
        DEFAULT_MAX_SEGMENT_SECONDS,
    );
    max_segment_seconds = max_segment_seconds.clamp(1, 600);

    let mut default_segment_seconds = env_i32(
        "SOUND_RECORDER_DEFAULT_SEGMENT_SECONDS",
        DEFAULT_SEGMENT_SECONDS,
    );
    default_segment_seconds = default_segment_seconds.clamp(1, max_segment_seconds);

    let mut max_segment_bytes = env_i32(
        "SOUND_RECORDER_MAX_SEGMENT_BYTES",
        DEFAULT_MAX_SEGMENT_BYTES,
    );
    max_segment_bytes = max_segment_bytes.clamp(1, MAX_SEGMENT_BYTES);

    let allow_public_device_registration = env_bool(
        "SOUND_RECORDER_ALLOW_PUBLIC_DEVICE_REGISTRATION",
        false,
        &mut validation_errors,
    );
    let rate_limit_trust_forwarded_for = env_bool(
        "SOUND_RECORDER_RATE_LIMIT_TRUST_FORWARDED_FOR",
        false,
        &mut validation_errors,
    );
    let require_supabase = env_bool(
        "SOUND_RECORDER_REQUIRE_SUPABASE",
        false,
        &mut validation_errors,
    );
    let mirror_readiness_required = env_bool(
        "SOUND_RECORDER_MIRROR_READINESS_REQUIRED",
        false,
        &mut validation_errors,
    );
    let s3 = s3_storage_config_from_env();
    let mirror = mirror_storage_config_from_env(&s3);
    let google_oauth = OAuthProviderConfig {
        client_id: first_env(&["SOUND_RECORDER_GOOGLE_CLIENT_ID"]),
        client_secret: first_env(&["SOUND_RECORDER_GOOGLE_CLIENT_SECRET"]),
        authorization_url: optional_service_url_from_env(
            &["SOUND_RECORDER_GOOGLE_AUTHORIZATION_URL"],
            &mut validation_errors,
        ),
        token_url: optional_service_url_from_env(
            &["SOUND_RECORDER_GOOGLE_TOKEN_URL"],
            &mut validation_errors,
        ),
    };
    let microsoft_oauth = OAuthProviderConfig {
        client_id: first_env(&["SOUND_RECORDER_MICROSOFT_CLIENT_ID"]),
        client_secret: first_env(&["SOUND_RECORDER_MICROSOFT_CLIENT_SECRET"]),
        authorization_url: optional_service_url_from_env(
            &["SOUND_RECORDER_MICROSOFT_AUTHORIZATION_URL"],
            &mut validation_errors,
        ),
        token_url: optional_service_url_from_env(
            &["SOUND_RECORDER_MICROSOFT_TOKEN_URL"],
            &mut validation_errors,
        ),
    };
    let dropbox_oauth = OAuthProviderConfig {
        client_id: first_env(&["SOUND_RECORDER_DROPBOX_CLIENT_ID"]),
        client_secret: first_env(&["SOUND_RECORDER_DROPBOX_CLIENT_SECRET"]),
        authorization_url: optional_service_url_from_env(
            &["SOUND_RECORDER_DROPBOX_AUTHORIZATION_URL"],
            &mut validation_errors,
        ),
        token_url: optional_service_url_from_env(
            &["SOUND_RECORDER_DROPBOX_TOKEN_URL"],
            &mut validation_errors,
        ),
    };
    let google_drive_upload_url = service_url_from_env(
        &["SOUND_RECORDER_GOOGLE_DRIVE_UPLOAD_URL"],
        "https://www.googleapis.com/upload/drive/v3/files",
        &mut validation_errors,
    );
    let microsoft_graph_base_url = service_url_from_env(
        &["SOUND_RECORDER_MICROSOFT_GRAPH_BASE_URL"],
        "https://graph.microsoft.com/v1.0",
        &mut validation_errors,
    );
    let dropbox_upload_url = service_url_from_env(
        &["SOUND_RECORDER_DROPBOX_UPLOAD_URL"],
        "https://content.dropboxapi.com/2/files/upload",
        &mut validation_errors,
    );

    Config {
        validation_errors,
        database_url: first_env(&[
            "SOUND_RECORDER_RDS_DATABASE_URL",
            "AGENT_TASKS_RDS_DATABASE_URL",
            "RDS_DATABASE_URL",
            "DATABASE_URL",
            "PG_DATABASE_URL",
        ]),
        server_auth_secret: first_env(&["SOUND_RECORDER_SERVER_AUTH_SECRET", "SERVER_AUTH_SECRET"]),
        token_pepper,
        token_pepper_configured,
        registration_bearer: first_env(&["SOUND_RECORDER_REGISTRATION_BEARER"]),
        allow_public_device_registration,
        s3,
        mirror,
        mirror_readiness_required,
        mirror_batch_size: env_i64_clamped(
            "SOUND_RECORDER_MIRROR_BATCH_SIZE",
            DEFAULT_MIRROR_BATCH_SIZE,
            1,
            MAX_MIRROR_BATCH_SIZE,
        ),
        mirror_copy_max_attempts: env_i32(
            "SOUND_RECORDER_MIRROR_COPY_MAX_ATTEMPTS",
            DEFAULT_MIRROR_COPY_MAX_ATTEMPTS,
        )
        .clamp(1, 20),
        ios_app_store_url: first_env(&["SOUND_RECORDER_IOS_APP_STORE_URL"]),
        android_play_store_url: first_env(&["SOUND_RECORDER_ANDROID_PLAY_STORE_URL"]),
        default_retention_hours,
        upload_url_ttl: env_duration_clamped(
            "SOUND_RECORDER_UPLOAD_URL_TTL_SECONDS",
            DEFAULT_UPLOAD_URL_TTL_SECONDS,
            30,
            900,
        ),
        download_url_ttl: env_duration_clamped(
            "SOUND_RECORDER_DOWNLOAD_URL_TTL_SECONDS",
            DEFAULT_DOWNLOAD_URL_TTL_SECONDS,
            60,
            3600,
        ),
        session_ttl_hours: env_i64(
            "SOUND_RECORDER_SESSION_TTL_HOURS",
            DEFAULT_SESSION_TTL_HOURS,
        ),
        default_segment_seconds,
        max_segment_seconds,
        max_segment_bytes,
        oauth_state_ttl: env_duration_clamped(
            "SOUND_RECORDER_OAUTH_STATE_TTL_SECONDS",
            DEFAULT_OAUTH_STATE_TTL_SECONDS,
            60,
            3600,
        ),
        cloud_copy_batch_size: env_i64_clamped(
            "SOUND_RECORDER_CLOUD_COPY_BATCH_SIZE",
            DEFAULT_CLOUD_COPY_BATCH_SIZE,
            1,
            MAX_CLOUD_COPY_BATCH_SIZE,
        ),
        cloud_copy_max_attempts: env_i32(
            "SOUND_RECORDER_CLOUD_COPY_MAX_ATTEMPTS",
            DEFAULT_CLOUD_COPY_MAX_ATTEMPTS,
        )
        .clamp(1, 10),
        cloud_copy_max_bytes: env_i64_clamped(
            "SOUND_RECORDER_CLOUD_COPY_MAX_BYTES",
            DEFAULT_CLOUD_COPY_MAX_BYTES,
            1,
            MAX_CLOUD_COPY_MAX_BYTES,
        ),
        cloud_backfill_segments: env_i64_clamped(
            "SOUND_RECORDER_CLOUD_BACKFILL_SEGMENTS",
            DEFAULT_CLOUD_BACKFILL_SEGMENTS,
            0,
            MAX_CLOUD_BACKFILL_SEGMENTS,
        ),
        google_oauth,
        microsoft_oauth,
        dropbox_oauth,
        oauth_redirect_allowlist: first_env(&["SOUND_RECORDER_OAUTH_REDIRECT_ALLOWLIST"])
            .map(|raw| {
                raw.split(',')
                    .map(|item| item.trim().to_string())
                    .filter(|item| !item.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        google_drive_upload_url,
        microsoft_graph_base_url,
        dropbox_upload_url,
        public_base_url: first_env(&["SOUND_RECORDER_PUBLIC_BASE_URL"]),
        // No hardcoded default recipient: a baked-in personal address would receive
        // every deployment's alert audio links if the operator forgot to set this.
        // Unset => alerts are disabled (fail closed), exactly like the webhook below.
        alert_email_to: first_env(&["SOUND_RECORDER_ALERT_EMAIL_TO"]).unwrap_or_default(),
        alert_email_webhook_url: first_env(&["SOUND_RECORDER_ALERT_EMAIL_WEBHOOK_URL"]),
        rate_limit_per_minute: env_u32_allow_zero(
            "SOUND_RECORDER_RATE_LIMIT_PER_MINUTE",
            DEFAULT_RATE_LIMIT_PER_MINUTE,
        ),
        // Secure by default: key the rate limiter on the real peer IP, not a
        // client-spoofable X-Forwarded-For header. Operators behind a trusted
        // proxy that sets XFF must opt in with
        // SOUND_RECORDER_RATE_LIMIT_TRUST_FORWARDED_FOR=1; otherwise an attacker
        // could rotate XFF per request to get an unbounded per-key budget.
        rate_limit_trust_forwarded_for,
        require_supabase,
        supabase: supabase_config_from_env(),
        shared_auth: shared_auth_config_from_env(),
    }
}

fn supabase_config_from_env() -> SupabaseConfig {
    let mut validation_errors = Vec::new();
    let mut url = first_env(&["SOUND_RECORDER_SUPABASE_URL", "SUPABASE_URL"])
        .map(|url| url.trim_end_matches('/').to_string());
    if url
        .as_deref()
        .is_some_and(|url| !validate_service_url(url, true))
    {
        validation_errors.push(
            "SOUND_RECORDER_SUPABASE_URL must be HTTPS (or loopback HTTP) with no path, credentials, query, or fragment"
                .to_string(),
        );
        url = None;
    }
    let mut jwks_url = first_env(&["SOUND_RECORDER_SUPABASE_JWKS_URL", "SUPABASE_JWKS_URL"])
        .or_else(|| {
            url.as_ref()
                .map(|url| format!("{url}/auth/v1/.well-known/jwks.json"))
        });
    if jwks_url
        .as_deref()
        .is_some_and(|url| !validate_service_url(url, false))
    {
        validation_errors.push(
            "SOUND_RECORDER_SUPABASE_JWKS_URL must be HTTPS (or loopback HTTP) without credentials, query, or fragment"
                .to_string(),
        );
        jwks_url = None;
    }
    if let (Some(project_url), Some(candidate)) = (url.as_deref(), jwks_url.as_deref()) {
        if !urls_have_same_origin(project_url, candidate) {
            validation_errors.push(
                "SOUND_RECORDER_SUPABASE_JWKS_URL must use the same origin as SOUND_RECORDER_SUPABASE_URL"
                    .to_string(),
            );
            jwks_url = None;
        }
    }
    let mut issuer = first_env(&["SOUND_RECORDER_SUPABASE_ISSUER", "SUPABASE_ISSUER"])
        .or_else(|| url.as_ref().map(|url| format!("{url}/auth/v1")));
    if issuer
        .as_deref()
        .is_some_and(|url| !validate_service_url(url, false))
    {
        validation_errors.push(
            "SOUND_RECORDER_SUPABASE_ISSUER must be an HTTPS (or loopback HTTP) URL".to_string(),
        );
        issuer = None;
    }
    if let (Some(project_url), Some(candidate)) = (url.as_deref(), issuer.as_deref()) {
        if !urls_have_same_origin(project_url, candidate) {
            validation_errors.push(
                "SOUND_RECORDER_SUPABASE_ISSUER must use the same origin as SOUND_RECORDER_SUPABASE_URL"
                    .to_string(),
            );
            issuer = None;
        }
    }
    SupabaseConfig {
        url,
        jwt_secret: first_env(&["SOUND_RECORDER_SUPABASE_JWT_SECRET", "SUPABASE_JWT_SECRET"]),
        jwks_url,
        issuer,
        audience: first_env(&["SOUND_RECORDER_SUPABASE_AUDIENCE", "SUPABASE_AUDIENCE"])
            .unwrap_or_else(|| SUPABASE_DEFAULT_AUDIENCE.to_string()),
        publishable_key: first_env(&[
            "SOUND_RECORDER_SUPABASE_PUBLISHABLE_KEY",
            "SUPABASE_PUBLISHABLE_KEY",
            "SOUND_RECORDER_SUPABASE_ANON_KEY",
            "SUPABASE_ANON_KEY",
        ]),
        service_role_key: first_env(&[
            "SOUND_RECORDER_SUPABASE_SERVICE_ROLE_KEY",
            "SUPABASE_SERVICE_ROLE_KEY",
        ]),
        validation_errors,
    }
}

/// Builds one S3-compatible client from a validated storage config, or `None`
/// when that target is unconfigured/invalid. `role` only labels the log line.
async fn build_object_storage_client(
    config: &S3StorageConfig,
    role: &'static str,
) -> Option<aws_sdk_s3::Client> {
    if !config.is_configured() {
        return None;
    }
    let retry_config = RetryConfig::standard().with_max_attempts(config.max_attempts);
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(Region::new(config.region.clone()))
        .retry_config(retry_config);
    if let (Some(access_key_id), Some(secret_access_key)) = (
        config.access_key_id.as_deref(),
        config.secret_access_key.as_deref(),
    ) {
        loader = loader.credentials_provider(aws_sdk_s3::config::Credentials::new(
            access_key_id,
            secret_access_key,
            config.session_token.clone(),
            None,
            "sonus-auris-object-storage",
        ));
    }
    let shared_config = loader.load().await;
    let mut builder = aws_sdk_s3::config::Builder::from(&shared_config);
    if let Some(endpoint) = &config.endpoint {
        builder = builder.endpoint_url(endpoint);
    }
    builder = builder.force_path_style(config.force_path_style);
    info!(
        role,
        backend = config.backend.as_str(),
        region = %config.region,
        custom_endpoint = config.endpoint.is_some(),
        force_path_style = config.force_path_style,
        max_attempts = config.max_attempts,
        "object storage client configured"
    );
    Some(aws_sdk_s3::Client::from_conf(builder.build()))
}

async fn state_from_config(config: Config) -> AppState {
    for error in &config.validation_errors {
        warn!(error, "service configuration is invalid");
    }
    for error in &config.s3.validation_errors {
        warn!(error, "object storage configuration is invalid");
    }
    for error in &config.supabase.validation_errors {
        warn!(error, "Supabase configuration is invalid");
    }
    for error in &config.shared_auth.validation_errors {
        warn!(error, "shared-auth configuration is invalid");
    }
    // Fail-closed diagnostic: a key without a pinned issuer would accept tokens
    // from *any* Supabase project (`aud` is "authenticated" everywhere), so
    // is_enabled() refuses to build a verifier. Say so loudly, because the
    // symptom is otherwise a silent "auth is off".
    if config.supabase.validation_errors.is_empty()
        && (config.supabase.jwt_secret.is_some() || config.supabase.jwks_url.is_some())
        && config.supabase.issuer.is_none()
    {
        warn!(
            "Supabase auth is disabled: a signing key is configured but no issuer is pinned. \
             Set SOUND_RECORDER_SUPABASE_URL (the issuer is derived as <url>/auth/v1) or set \
             SOUND_RECORDER_SUPABASE_ISSUER explicitly."
        );
    }
    let s3 = build_object_storage_client(&config.s3, "primary").await;
    let mirror = build_object_storage_client(&config.mirror, "mirror").await;

    let cloud_sealer = match first_env(&["SOUND_RECORDER_CLOUD_TOKEN_ENCRYPTION_KEY"]) {
        Some(key) => match CloudTokenSealer::from_base64_key(&key) {
            Ok(sealer) => Some(sealer),
            Err(err) => {
                warn!(error = ?err, "SOUND_RECORDER_CLOUD_TOKEN_ENCRYPTION_KEY is invalid; cloud OAuth linking is disabled");
                None
            }
        },
        None => None,
    };

    let http = reqwest::Client::builder()
        .user_agent("dd-sound-recorder-rs/0.1")
        .timeout(Duration::from_secs(30))
        .build()
        .expect("reqwest client can be built");

    let supabase = SupabaseVerifier::from_config(&config.supabase).map(Arc::new);
    if supabase.is_some() {
        info!("Supabase token verification is enabled");
    }

    let pg_pool = match config.database_url.as_deref() {
        Some(database_url) => build_pg_pool(database_url).await,
        None => None,
    };

    AppState {
        config: Arc::new(config),
        s3,
        mirror,
        http,
        cloud_sealer,
        supabase,
        pg_pool,
        storage_history_cache: Arc::new(RwLock::new(None)),
        storage_history_refresh_lock: Arc::new(AsyncMutex::new(())),
        device_presence: Arc::new(DevicePresenceHub::default()),
    }
}

/// Builds the shared SeaORM Postgres pool. Readiness remains responsible for
/// proving downstream reachability; invalid or unavailable database setup
/// disables persistence without crashing the process.
async fn build_pg_pool(database_url: &str) -> Option<PgPool> {
    let max_size = env_u64("SOUND_RECORDER_PG_POOL_MAX_SIZE", 16).clamp(1, 100) as u32;
    match DbClient::connect(database_url, max_size).await {
        Ok(pool) => Some(pool),
        Err(_) => {
            error!("SeaORM Postgres pool initialization failed; database is disabled");
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CloudProvider {
    GoogleDrive,
    MicrosoftOneDrive,
    AppleICloud,
    Dropbox,
    AmazonS3,
    CloudflareR2,
}

impl CloudProvider {
    fn parse(value: &str) -> Result<Self, ServiceError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "google_drive" | "googledrive" | "google" => Ok(Self::GoogleDrive),
            "microsoft_onedrive" | "onedrive" | "microsoft" => Ok(Self::MicrosoftOneDrive),
            "apple_icloud" | "icloud" | "apple" => Ok(Self::AppleICloud),
            "dropbox" | "drop_box" => Ok(Self::Dropbox),
            "amazon_s3" | "aws_s3" | "s3" => Ok(Self::AmazonS3),
            "cloudflare_r2" | "r2" => Ok(Self::CloudflareR2),
            _ => Err(ServiceError::BadRequest(
                "provider must be google_drive, microsoft_onedrive, apple_icloud, dropbox, amazon_s3, or cloudflare_r2".to_string(),
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::GoogleDrive => "google_drive",
            Self::MicrosoftOneDrive => "microsoft_onedrive",
            Self::AppleICloud => "apple_icloud",
            Self::Dropbox => "dropbox",
            Self::AmazonS3 => "amazon_s3",
            Self::CloudflareR2 => "cloudflare_r2",
        }
    }

    fn link_mode(self) -> &'static str {
        match self {
            Self::AppleICloud | Self::AmazonS3 | Self::CloudflareR2 => "client_managed",
            Self::GoogleDrive | Self::MicrosoftOneDrive | Self::Dropbox => "server_oauth",
        }
    }

    fn required_scope(self) -> Option<&'static str> {
        match self {
            Self::GoogleDrive => Some(GOOGLE_DRIVE_SCOPE),
            Self::MicrosoftOneDrive => Some(MICROSOFT_ONEDRIVE_SCOPE),
            Self::AppleICloud | Self::AmazonS3 | Self::CloudflareR2 => None,
            Self::Dropbox => Some(DROPBOX_SCOPE),
        }
    }

    fn oauth_config(self, config: &Config) -> Option<&OAuthProviderConfig> {
        match self {
            Self::GoogleDrive => Some(&config.google_oauth),
            Self::MicrosoftOneDrive => Some(&config.microsoft_oauth),
            Self::AppleICloud | Self::AmazonS3 | Self::CloudflareR2 => None,
            Self::Dropbox => Some(&config.dropbox_oauth),
        }
    }

    fn authorization_endpoint(self) -> Option<&'static str> {
        match self {
            Self::GoogleDrive => Some("https://accounts.google.com/o/oauth2/v2/auth"),
            Self::MicrosoftOneDrive => {
                Some("https://login.microsoftonline.com/common/oauth2/v2.0/authorize")
            }
            Self::AppleICloud | Self::AmazonS3 | Self::CloudflareR2 => None,
            Self::Dropbox => Some("https://www.dropbox.com/oauth2/authorize"),
        }
    }

    fn token_endpoint(self) -> Option<&'static str> {
        match self {
            Self::GoogleDrive => Some("https://oauth2.googleapis.com/token"),
            Self::MicrosoftOneDrive => {
                Some("https://login.microsoftonline.com/common/oauth2/v2.0/token")
            }
            Self::AppleICloud | Self::AmazonS3 | Self::CloudflareR2 => None,
            Self::Dropbox => Some("https://api.dropboxapi.com/oauth2/token"),
        }
    }

    fn is_server_managed(self) -> bool {
        matches!(
            self,
            Self::GoogleDrive | Self::MicrosoftOneDrive | Self::Dropbox
        )
    }

    fn is_client_managed(self) -> bool {
        !self.is_server_managed()
    }

    fn supports_copy_jobs(self) -> bool {
        !matches!(self, Self::AmazonS3 | Self::CloudflareR2)
    }
}

impl CloudTokenSealer {
    fn from_base64_key(key: &str) -> Result<Self, ServiceError> {
        let raw = BASE64_STANDARD.decode(key.trim()).map_err(|_| {
            ServiceError::Unavailable(
                "SOUND_RECORDER_CLOUD_TOKEN_ENCRYPTION_KEY must be base64".to_string(),
            )
        })?;
        if raw.len() != 32 {
            return Err(ServiceError::Unavailable(
                "SOUND_RECORDER_CLOUD_TOKEN_ENCRYPTION_KEY must decode to 32 bytes".to_string(),
            ));
        }
        let cipher = Aes256Gcm::new_from_slice(&raw).map_err(|_| {
            ServiceError::Unavailable(
                "SOUND_RECORDER_CLOUD_TOKEN_ENCRYPTION_KEY is invalid".to_string(),
            )
        })?;
        Ok(Self {
            cipher: Arc::new(cipher),
        })
    }

    fn seal(
        &self,
        account_id: &str,
        provider: CloudProvider,
        plaintext: &[u8],
    ) -> Result<SealedTokenEnvelope, ServiceError> {
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let aad = format!(
            "dd-sound-recorder-rs/v1|account={account_id}|provider={}",
            provider.as_str()
        );
        let ciphertext = self
            .cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| ServiceError::Internal("cloud credential seal failed".to_string()))?;
        Ok(SealedTokenEnvelope {
            ciphertext_b64: BASE64_STANDARD.encode(ciphertext),
            nonce_b64: BASE64_STANDARD.encode(nonce_bytes),
            aad_tag: aad,
            version: 1,
        })
    }

    fn unseal(
        &self,
        account_id: &str,
        provider: CloudProvider,
        envelope: &SealedTokenEnvelope,
    ) -> Result<Vec<u8>, ServiceError> {
        let expected_aad = format!(
            "dd-sound-recorder-rs/v1|account={account_id}|provider={}",
            provider.as_str()
        );
        if envelope.aad_tag != expected_aad {
            return Err(ServiceError::Internal(
                "cloud credential envelope is scoped to another account/provider".to_string(),
            ));
        }
        let nonce_bytes = BASE64_STANDARD
            .decode(&envelope.nonce_b64)
            .map_err(|_| ServiceError::Internal("cloud credential nonce is invalid".to_string()))?;
        if nonce_bytes.len() != 12 {
            return Err(ServiceError::Internal(
                "cloud credential nonce has invalid length".to_string(),
            ));
        }
        let ciphertext = BASE64_STANDARD
            .decode(&envelope.ciphertext_b64)
            .map_err(|_| {
                ServiceError::Internal("cloud credential ciphertext is invalid".to_string())
            })?;
        self.cipher
            .decrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: &ciphertext,
                    aad: envelope.aad_tag.as_bytes(),
                },
            )
            .map_err(|_| ServiceError::Internal("cloud credential unseal failed".to_string()))
    }
}

/// Rare-case, opt-in segment decryption.
///
/// Audio is sealed on the device (see the Flutter `SegmentCipher`): the cloud
/// and this backend normally only ever see the `SAC1` ciphertext container and
/// cannot read it — that is the zero-knowledge default. For a *user-initiated*
/// server-side job (today: mirroring a saved clip into a server-managed Google
/// Drive / OneDrive so it lands as a playable file) the client may opt in and
/// release the single per-segment data key (DEK). We then decrypt exactly that
/// one segment in memory with the supplied DEK. The device master key is never
/// involved here and no key material is ever persisted server-side.
mod segment_job_cipher {
    use super::ServiceError;
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};

    const MAGIC: &[u8; 4] = b"SAC1";
    const VERSION: u8 = 1;
    const NONCE_LEN: usize = 12;
    const TAG_LEN: usize = 16;
    const HEADER_FIXED_LEN: usize = 8;
    const DEK_LEN: usize = 32;

    /// Decrypts a `SAC1` container's audio payload using a client-released DEK.
    /// The wrapped-DEK bytes in the header are ignored: unwrapping needs the
    /// device master key, which never reaches the server — the client hands us
    /// the already-unwrapped DEK for this one clip.
    pub(crate) fn decrypt_segment(dek: &[u8], container: &[u8]) -> Result<Vec<u8>, ServiceError> {
        if dek.len() != DEK_LEN {
            return Err(ServiceError::BadRequest(
                "released segment key must be 32 bytes".to_string(),
            ));
        }
        if container.len() < HEADER_FIXED_LEN || &container[0..4] != MAGIC {
            return Err(ServiceError::BadRequest(
                "not a Sonus Auris encrypted segment".to_string(),
            ));
        }
        if container[4] != VERSION {
            return Err(ServiceError::BadRequest(
                "unsupported segment cipher version".to_string(),
            ));
        }
        let wrapped_len = ((container[6] as usize) << 8) | container[7] as usize;
        let content_offset = HEADER_FIXED_LEN + wrapped_len;
        if container.len() < content_offset + NONCE_LEN + TAG_LEN {
            return Err(ServiceError::BadRequest(
                "encrypted segment is truncated".to_string(),
            ));
        }
        let (nonce_bytes, ciphertext_and_tag) = container[content_offset..].split_at(NONCE_LEN);
        let cipher = Aes256Gcm::new_from_slice(dek)
            .map_err(|_| ServiceError::Internal("invalid segment key".to_string()))?;
        cipher
            .decrypt(Nonce::from_slice(nonce_bytes), ciphertext_and_tag)
            .map_err(|_| ServiceError::BadRequest("segment decryption failed".to_string()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use aes_gcm::aead::OsRng;
        use aes_gcm::AeadCore;

        /// Build a `SAC1` container exactly the way the Flutter `SegmentCipher`
        /// does, so the parser + AEAD path is exercised end to end.
        fn seal(dek: &[u8], plaintext: &[u8]) -> Vec<u8> {
            let cipher = Aes256Gcm::new_from_slice(dek).unwrap();
            let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
            let ct = cipher.encrypt(&nonce, plaintext).unwrap();
            let wrapped_dek = [0u8; 60]; // nonce(12)+dek(32)+tag(16), opaque here
            let mut out = Vec::new();
            out.extend_from_slice(MAGIC);
            out.push(VERSION);
            out.push(0x01);
            out.push((wrapped_dek.len() >> 8) as u8);
            out.push((wrapped_dek.len() & 0xFF) as u8);
            out.extend_from_slice(&wrapped_dek);
            out.extend_from_slice(nonce.as_slice());
            out.extend_from_slice(&ct);
            out
        }

        #[test]
        fn round_trips_with_the_released_dek() {
            let dek = [7u8; 32];
            let plaintext = b"a short captured riff".to_vec();
            let container = seal(&dek, &plaintext);
            let recovered = decrypt_segment(&dek, &container).unwrap();
            assert_eq!(recovered, plaintext);
        }

        #[test]
        fn wrong_dek_is_rejected() {
            let container = seal(&[7u8; 32], b"secret");
            assert!(decrypt_segment(&[9u8; 32], &container).is_err());
        }

        #[test]
        fn non_container_bytes_are_rejected() {
            assert!(decrypt_segment(&[7u8; 32], b"not-a-container").is_err());
        }

        #[test]
        fn short_key_is_rejected() {
            let container = seal(&[7u8; 32], b"secret");
            assert!(decrypt_segment(&[7u8; 16], &container).is_err());
        }
    }
}

/// Applies opt-in, client-released segment decryption before a server-managed
/// cloud copy. When `released_dek` is `None` (the default), the ciphertext is
/// mirrored as-is and the copy stays zero-knowledge; when the user has opted a
/// clip in, the per-segment DEK decrypts it in memory so it lands playable.
fn apply_opt_in_segment_decryption(
    released_dek: Option<&[u8]>,
    bytes: Vec<u8>,
) -> Result<Vec<u8>, ServiceError> {
    match released_dek {
        Some(dek) => segment_job_cipher::decrypt_segment(dek, &bytes),
        None => Ok(bytes),
    }
}

fn record_request(method: &str, path: &str, status: StatusCode) {
    HTTP_REQUESTS
        .with_label_values(&[method, path, status.as_str()])
        .inc();
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn const_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right.iter())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

fn strip_bearer_scheme(value: &str) -> Option<&str> {
    let value = value.trim();
    let (scheme, token) = value.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        let token = token.trim();
        (!token.is_empty()).then_some(token)
    } else {
        None
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(strip_bearer_scheme)
}

/// Reads a Supabase access token from `x-supabase-auth` (with or without a
/// `Bearer ` prefix). Kept distinct from `Authorization` so the device bearer
/// token and the identity token never have to share one header.
fn supabase_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-supabase-auth")
        .and_then(|value| value.to_str().ok())
        .map(|value| strip_bearer_scheme(value).unwrap_or(value))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Shared-auth is kept separate from the device bearer token and the optional
/// Supabase token. The header accepts either a raw JWT or `Bearer <JWT>`.
fn shared_auth_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-shared-auth")
        .and_then(|value| value.to_str().ok())
        .map(|value| strip_bearer_scheme(value).unwrap_or(value))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

async fn introspect_shared_auth(
    state: &AppState,
    token: &str,
) -> Result<SharedAuthIdentity, ServiceError> {
    if token.len() > 16 * 1024 {
        return Err(ServiceError::Unauthorized);
    }
    let config = &state.config.shared_auth;
    let url = config.introspect_url().ok_or_else(|| {
        ServiceError::Unavailable("shared-auth introspection is not configured".to_string())
    })?;
    let secret = config.introspect_secret.as_deref().ok_or_else(|| {
        ServiceError::Unavailable("shared-auth introspection is not configured".to_string())
    })?;
    let request = state
        .http
        .post(url)
        .bearer_auth(secret)
        .json(&json!({ "token": token }))
        .send();
    let response = tokio::time::timeout(SHARED_AUTH_INTROSPECTION_TIMEOUT, request)
        .await
        .map_err(|_| ServiceError::Unavailable("shared-auth introspection timed out".to_string()))?
        .map_err(|_| {
            ServiceError::Unavailable("shared-auth introspection is unavailable".to_string())
        })?;
    if !response.status().is_success() {
        warn!(
            status = response.status().as_u16(),
            "shared-auth introspection rejected the service request"
        );
        return Err(ServiceError::Unavailable(
            "shared-auth introspection is unavailable".to_string(),
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > SHARED_AUTH_INTROSPECTION_MAX_BYTES as u64)
    {
        return Err(ServiceError::Unavailable(
            "shared-auth introspection response is invalid".to_string(),
        ));
    }
    let body = response.bytes().await.map_err(|_| {
        ServiceError::Unavailable("shared-auth introspection response is invalid".to_string())
    })?;
    if body.len() > SHARED_AUTH_INTROSPECTION_MAX_BYTES {
        return Err(ServiceError::Unavailable(
            "shared-auth introspection response is invalid".to_string(),
        ));
    }
    let introspection: SharedAuthIntrospection = serde_json::from_slice(&body).map_err(|_| {
        ServiceError::Unavailable("shared-auth introspection response is invalid".to_string())
    })?;
    if !introspection.active {
        return Err(ServiceError::Unauthorized);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    validate_shared_auth_assurance(&introspection, config.required_aal, now)?;
    let subject = introspection
        .sub
        .as_deref()
        .and_then(|subject| Uuid::parse_str(subject).ok())
        .ok_or_else(|| {
            ServiceError::Unavailable("shared-auth introspection response is invalid".to_string())
        })?
        .to_string();
    let email = introspection
        .email_verified
        .then_some(introspection.email)
        .flatten()
        .map(|email| email.trim().to_string())
        .filter(|email| !email.is_empty() && email.len() <= 320);
    Ok(SharedAuthIdentity { subject, email })
}

fn internal_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-server-auth")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| bearer_token(headers))
}

fn hash_secret(secret: &str, pepper: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(pepper.as_bytes());
    hasher.update(b":");
    hasher.update(secret.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn new_device_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("sr_live_{}", URL_SAFE_NO_PAD.encode(bytes))
}

fn last4(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    chars
        .iter()
        .skip(chars.len().saturating_sub(4))
        .collect::<String>()
}

fn clean_string(value: Option<String>, max_len: usize) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.len() > max_len {
                value.chars().take(max_len).collect()
            } else {
                value
            }
        })
}

fn clean_optional_nonempty(
    value: Option<String>,
    max_len: usize,
) -> Result<Option<String>, ServiceError> {
    let Some(value) = value else {
        return Ok(None);
    };
    Ok(Some(validate_nonempty(&value, "value", max_len)?))
}

fn validate_nonempty(value: &str, field: &str, max_len: usize) -> Result<String, ServiceError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ServiceError::BadRequest(format!("{field} is required")));
    }
    if trimmed.len() > max_len {
        return Err(ServiceError::BadRequest(format!(
            "{field} must be at most {max_len} characters"
        )));
    }
    Ok(trimmed.to_string())
}

fn validate_uuid(value: &str, field: &str) -> Result<String, ServiceError> {
    Uuid::parse_str(value)
        .map(|uuid| uuid.to_string())
        .map_err(|_| ServiceError::BadRequest(format!("{field} must be a UUID")))
}

fn normalize_platform(value: &str) -> Result<String, ServiceError> {
    let platform = value.trim().to_ascii_lowercase();
    if matches!(
        platform.as_str(),
        "ios" | "android" | "macos" | "windows" | "linux"
    ) {
        Ok(platform)
    } else {
        Err(ServiceError::BadRequest(
            "platform must be ios, android, macos, windows, or linux".to_string(),
        ))
    }
}

fn validate_network_policy(value: Option<String>) -> Result<String, ServiceError> {
    let policy = clean_string(value, 20)
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_else(|| "any".to_string());
    if matches!(policy.as_str(), "any" | "wifi_only" | "cellular_only") {
        Ok(policy)
    } else {
        Err(ServiceError::BadRequest(
            "networkPolicy must be any, wifi_only, or cellular_only".to_string(),
        ))
    }
}

fn validate_pause_reason(value: Option<String>) -> Result<Option<String>, ServiceError> {
    let Some(reason) = clean_string(value, 40).map(|value| value.to_ascii_lowercase()) else {
        return Ok(None);
    };
    if matches!(
        reason.as_str(),
        "low_battery" | "network_constraint" | "offline" | "manual"
    ) {
        Ok(Some(reason))
    } else {
        Err(ServiceError::BadRequest(
            "reason must be low_battery, network_constraint, offline, or manual".to_string(),
        ))
    }
}

fn validate_legal_region(value: Option<String>) -> Result<Option<String>, ServiceError> {
    let Some(value) = clean_string(value, 64) else {
        return Ok(None);
    };
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | ':' | '/' | '-'))
    {
        Ok(Some(value))
    } else {
        Err(ServiceError::BadRequest(
            "legalRegion contains unsupported characters".to_string(),
        ))
    }
}

fn validate_sha256(value: Option<String>) -> Result<Option<String>, ServiceError> {
    let Some(value) = clean_string(value, 64) else {
        return Ok(None);
    };
    if value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Ok(Some(value.to_ascii_lowercase()))
    } else {
        Err(ServiceError::BadRequest(
            "sha256Hex must be a 64-character hex digest".to_string(),
        ))
    }
}

fn validate_content_type(value: Option<String>, default: &str) -> Result<String, ServiceError> {
    let content_type = clean_string(value, 120).unwrap_or_else(|| default.to_string());
    let normalized = content_type.to_ascii_lowercase();
    let allowed = normalized.starts_with("audio/")
        || normalized == "application/octet-stream"
        || normalized == "binary/octet-stream";
    if allowed {
        Ok(content_type)
    } else {
        Err(ServiceError::BadRequest(
            "contentType must be an audio media type".to_string(),
        ))
    }
}

fn validate_meta(value: Option<Value>) -> Result<Value, ServiceError> {
    match value {
        None => Ok(json!({})),
        Some(value) if value.is_object() => {
            let size = serde_json::to_vec(&value)
                .map(|bytes| bytes.len())
                .unwrap_or(MAX_META_BYTES + 1);
            if size > MAX_META_BYTES {
                return Err(ServiceError::BadRequest(format!(
                    "metaData must be at most {MAX_META_BYTES} bytes"
                )));
            }
            Ok(value)
        }
        Some(_) => Err(ServiceError::BadRequest(
            "metaData must be a JSON object".to_string(),
        )),
    }
}

fn attach_storage_metadata(
    mut meta_data: Value,
    backend_fingerprint: &str,
) -> Result<Value, ServiceError> {
    let object = meta_data
        .as_object_mut()
        .ok_or_else(|| ServiceError::BadRequest("metaData must be a JSON object".to_string()))?;
    // These fields are owned by the service. Removing client-supplied values is
    // necessary both for cutover enforcement and for safe retention retries.
    for key in [
        STORAGE_FINGERPRINT_META_KEY,
        RETENTION_DELETE_PENDING_META_KEY,
        RETENTION_DELETE_CLAIM_ID_META_KEY,
        RETENTION_DELETE_CLAIMED_AT_META_KEY,
        RETENTION_PREVIOUS_STATUS_META_KEY,
        MIRROR_STATE_META_KEY,
        MIRROR_CLAIM_ID_META_KEY,
        MIRROR_CLAIMED_AT_META_KEY,
        MIRROR_BUCKET_META_KEY,
        MIRROR_FINGERPRINT_META_KEY,
        MIRROR_MIRRORED_AT_META_KEY,
        MIRROR_ATTEMPTS_META_KEY,
        MIRROR_LAST_ERROR_META_KEY,
        MIRROR_NEXT_ATTEMPT_AT_META_KEY,
    ] {
        object.remove(key);
    }
    object.insert(
        STORAGE_FINGERPRINT_META_KEY.to_string(),
        Value::String(backend_fingerprint.to_string()),
    );
    let size = serde_json::to_vec(&meta_data)
        .map(|bytes| bytes.len())
        .unwrap_or(MAX_META_BYTES + 1);
    if size > MAX_META_BYTES {
        return Err(ServiceError::BadRequest(format!(
            "metaData including server storage metadata must be at most {MAX_META_BYTES} bytes"
        )));
    }
    Ok(meta_data)
}

fn storage_record_is_compatible(config: &S3StorageConfig, fingerprint: Option<&str>) -> bool {
    match fingerprint.filter(|value| !value.is_empty()) {
        Some(fingerprint) => fingerprint == config.backend_fingerprint,
        None => config.allow_unmarked_storage_history,
    }
}

fn validate_use_case(value: Option<String>) -> Result<String, ServiceError> {
    let Some(value) = clean_string(value, 32) else {
        return Ok(DEFAULT_USE_CASE.to_string());
    };
    let normalized = value.to_ascii_lowercase();
    if SUPPORTED_USE_CASES.contains(&normalized.as_str()) {
        Ok(normalized)
    } else {
        Err(ServiceError::BadRequest(format!(
            "useCase must be one of: {}",
            SUPPORTED_USE_CASES.join(", ")
        )))
    }
}

fn validate_alert_trigger(value: &str) -> Result<String, ServiceError> {
    let trigger = value.trim().to_ascii_lowercase();
    if matches!(trigger.as_str(), "manual" | "commotion" | "magic_phrase") {
        Ok(trigger)
    } else {
        Err(ServiceError::BadRequest(
            "trigger must be manual, commotion, or magic_phrase".to_string(),
        ))
    }
}

fn validate_redirect_uri(
    provider: CloudProvider,
    value: Option<String>,
    allowlist: &[String],
) -> Result<String, ServiceError> {
    if provider == CloudProvider::AppleICloud {
        return Ok("client-managed://apple-icloud".to_string());
    }
    let value = value.ok_or_else(|| {
        ServiceError::BadRequest("redirectUri is required for OAuth cloud links".to_string())
    })?;
    let uri = validate_nonempty(&value, "redirectUri", 512)?;
    // Parse the host rather than prefix-match: `starts_with("http://localhost")`
    // also accepted `http://localhost.evil.example`. is_safe_public_url enforces
    // scheme https (any host) or http only for a real loopback host.
    if !is_safe_public_url(&uri) {
        return Err(ServiceError::BadRequest(
            "redirectUri must be https or local loopback http".to_string(),
        ));
    }
    // Defense in depth: when an allow-list is configured, the redirect must be a
    // known app callback. (The OAuth provider also enforces its own registered
    // redirects; this pins the target before we ever initiate the flow.)
    if !allowlist.is_empty() && !allowlist.iter().any(|allowed| allowed == &uri) {
        return Err(ServiceError::BadRequest(
            "redirectUri is not in the allowed redirect list".to_string(),
        ));
    }
    Ok(uri)
}

fn validate_folder_path(value: Option<String>) -> Result<String, ServiceError> {
    let path = clean_string(value, 512).unwrap_or_else(|| "sound-recorder".to_string());
    if path.contains("..")
        || path.starts_with('/')
        || path
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '\\' | '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Err(ServiceError::BadRequest(
            "folderPath must be a relative cloud folder path".to_string(),
        ));
    }
    let path = path.trim_matches('/').to_string();
    if path.is_empty() {
        Ok("sound-recorder".to_string())
    } else {
        Ok(path)
    }
}

/// Builds the custom-scheme redirect consumed by `flutter_web_auth_2`. Query
/// pairs are encoded by `Url`, so provider-controlled errors and codes can
/// never inject response headers or alter the callback target.
fn cloud_oauth_app_callback(query: CloudOAuthCallbackQuery) -> Result<String, ServiceError> {
    let state = validate_nonempty(query.state.as_deref().unwrap_or(""), "state", 160)?;
    let mut target = reqwest::Url::parse("sonusauris://oauth/callback")
        .map_err(|_| ServiceError::Internal("OAuth app callback is invalid".to_string()))?;
    {
        let mut params = target.query_pairs_mut();
        params.append_pair("state", &state);
        if let Some(error) = clean_string(query.error, 160) {
            params.append_pair("error", &error);
            if let Some(description) = clean_string(query.error_description, 500) {
                params.append_pair("error_description", &description);
            }
        } else {
            let code = validate_nonempty(query.code.as_deref().unwrap_or(""), "code", 4096)?;
            params.append_pair("code", &code);
        }
    }
    Ok(target.to_string())
}

async fn cloud_oauth_callback(
    Query(query): Query<CloudOAuthCallbackQuery>,
) -> Result<Redirect, ServiceError> {
    let target = cloud_oauth_app_callback(query)?;
    record_request("GET", "/oauth/callback", StatusCode::TEMPORARY_REDIRECT);
    Ok(Redirect::temporary(&target))
}

/// Browser-only OAuth completion for Windows/Linux desktop builds. Those
/// platforms do not have flutter_web_auth_2's native callback session, so the
/// page displays the short-lived provider code for an explicit paste back into
/// Sonus Auris. The state remains held by the app and is verified again when
/// `/oauth/complete` consumes the pending database row.
fn cloud_oauth_manual_page(query: CloudOAuthCallbackQuery) -> Result<String, ServiceError> {
    validate_nonempty(query.state.as_deref().unwrap_or(""), "state", 160)?;
    let body = if let Some(error) = clean_string(query.error, 160) {
        let description = clean_string(query.error_description, 500)
            .unwrap_or_else(|| "The provider did not authorize this connection.".to_string());
        format!(
            "<h1>Connection not authorized</h1><p>{}</p><p>Error: <code>{}</code></p>",
            html_escape(&description),
            html_escape(&error)
        )
    } else {
        let code = validate_nonempty(query.code.as_deref().unwrap_or(""), "code", 4096)?;
        format!(
            "<h1>Return to Sonus Auris</h1>\
             <p>Copy this one-time authorization code and paste it into the Connections window.</p>\
             <textarea readonly rows=\"6\" cols=\"72\" aria-label=\"One-time authorization code\">{}</textarea>\
             <p>Close this browser tab after Sonus Auris confirms the connection. Never send this code to another person.</p>",
            html_escape(&code)
        )
    };
    Ok(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>Sonus Auris cloud connection</title></head><body><main>{body}</main></body></html>"
    ))
}

async fn cloud_oauth_manual_callback(
    Query(query): Query<CloudOAuthCallbackQuery>,
) -> Result<Html<String>, ServiceError> {
    let page = cloud_oauth_manual_page(query)?;
    record_request("GET", "/oauth/manual-callback", StatusCode::OK);
    Ok(Html(page))
}

fn validate_provider_account_id(value: Option<String>) -> Result<Option<String>, ServiceError> {
    clean_optional_nonempty(value, 240).map(|value| {
        value.map(|provider_account_id| {
            provider_account_id
                .chars()
                .filter(|ch| !ch.is_control())
                .collect::<String>()
        })
    })
}

fn extension_for_content_type(content_type: &str) -> &'static str {
    let normalized = content_type.to_ascii_lowercase();
    if normalized.contains("webm") {
        "webm"
    } else if normalized.contains("ogg") || normalized.contains("opus") {
        "opus"
    } else if normalized.contains("wav") {
        "wav"
    } else if normalized.contains("mpeg") || normalized.contains("mp3") {
        "mp3"
    } else if normalized.contains("3gpp") {
        "3gp"
    } else {
        "m4a"
    }
}

fn storage_key(prefix: &str, sequence_number: i32, content_type: &str) -> String {
    format!(
        "{prefix}/segment-{sequence_number:010}.{}",
        extension_for_content_type(content_type)
    )
}

fn cdn_url(config: &Config, key: &str) -> Option<String> {
    config.s3.cdn_base_url.as_ref().map(|base| {
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            key.trim_start_matches('/')
        )
    })
}

fn query_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~') {
            escaped.push(*byte as char);
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    escaped
}

fn graph_path_escape(path: &str) -> String {
    path.split('/')
        .filter(|part| !part.is_empty())
        .map(query_escape)
        .collect::<Vec<_>>()
        .join("/")
}

fn append_query(base_url: &str, query: &str) -> String {
    let separator = if base_url.contains('?') { '&' } else { '?' };
    format!("{base_url}{separator}{query}")
}

fn listen_url_for_alert(config: &Config, alert_id: &str) -> Option<String> {
    config.public_base_url.as_ref().and_then(|base| {
        let base = base.trim().trim_end_matches('/');
        if !is_safe_public_url(base) {
            return None;
        }
        Some(format!("{}/listen/{}", base, query_escape(alert_id)))
    })
}

fn render_listen_alert(alert_id: &str, manifest: &Value) -> String {
    let trigger = manifest
        .get("trigger")
        .and_then(Value::as_str)
        .unwrap_or("alert");
    let download_urls = listen_download_urls(manifest);
    let start_offset = manifest
        .get("startOffsetSeconds")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0);
    let occurred_at = manifest
        .get("occurredAt")
        .and_then(Value::as_str)
        .unwrap_or("");
    if download_urls.is_empty() {
        return format!(
            r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Audio alert</title></head>
<body style="font:16px system-ui,sans-serif;margin:32px;max-width:720px">
<h1>Audio alert</h1>
<p>No uploaded audio segment is available yet for alert <code>{}</code>.</p>
<p>Trigger: <strong>{}</strong><br>Occurred: <strong>{}</strong></p>
</body></html>"#,
            html_escape(alert_id),
            html_escape(trigger),
            html_escape(occurred_at)
        );
    }
    // Embed the URL list as a JS literal inside <script>. JSON is a near-subset
    // of JS, but `<` (so `</script>` can't break out of the element) and the
    // U+2028/U+2029 line separators (legal in JSON strings, illegal in JS string
    // literals) must be escaped. Server-minted S3 URLs never contain these, but
    // escaping keeps the page robust if the manifest source ever changes.
    let download_urls_json = serde_json::to_string(&download_urls)
        .unwrap_or_else(|_| "[]".to_string())
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    format!(
        r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Audio alert</title></head>
<body style="font:16px system-ui,sans-serif;margin:32px;max-width:720px">
<h1>Audio alert</h1>
<p>Trigger: <strong>{}</strong><br>Occurred: <strong>{}</strong></p>
<p>Segment <span id="segment-index">1</span> of {}</p>
<audio id="audio" controls preload="metadata" style="width:100%"></audio>
<script>
const urls = {};
const startOffset = {};
const audio = document.getElementById('audio');
const segmentIndex = document.getElementById('segment-index');
let currentIndex = 0;
function loadSegment(offsetSeconds) {{
  if (!urls[currentIndex]) return;
  segmentIndex.textContent = String(currentIndex + 1);
  audio.src = urls[currentIndex];
  audio.addEventListener('loadedmetadata', () => {{
    audio.currentTime = Math.max(0, offsetSeconds);
    audio.play().catch(() => {{}});
  }}, {{ once: true }});
}}
audio.addEventListener('ended', () => {{
  if (currentIndex + 1 < urls.length) {{
    currentIndex += 1;
    loadSegment(0);
  }}
}});
loadSegment(startOffset);
</script>
</body></html>"#,
        html_escape(trigger),
        html_escape(occurred_at),
        download_urls.len(),
        download_urls_json,
        start_offset
    )
}

fn listen_download_urls(manifest: &Value) -> Vec<String> {
    let mut urls = manifest
        .get("downloadUrls")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|url| is_safe_public_url(url))
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if urls.is_empty() {
        if let Some(url) = manifest
            .get("downloadUrl")
            .and_then(Value::as_str)
            .filter(|url| is_safe_public_url(url))
        {
            urls.push(url.to_string());
        }
    }
    urls
}

fn is_safe_public_url(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value.trim()) else {
        return false;
    };
    match url.scheme() {
        "https" => url.host_str().is_some(),
        "http" => matches!(
            url.host_str(),
            Some("localhost") | Some("127.0.0.1") | Some("::1")
        ),
        _ => false,
    }
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn google_drive_file_name(destination_key: &str) -> String {
    let mut name = String::with_capacity(destination_key.len().min(512));
    for part in destination_key.split('/').filter(|part| !part.is_empty()) {
        if !name.is_empty() {
            name.push_str("__");
        }
        for ch in part.chars() {
            if ch.is_control() || matches!(ch, '/' | '\\') {
                name.push('_');
            } else {
                name.push(ch);
            }
        }
    }
    if name.is_empty() {
        "segment.m4a".to_string()
    } else {
        name
    }
}

fn authorization_url(
    provider: CloudProvider,
    oauth: &OAuthProviderConfig,
    redirect_uri: &str,
    state: &str,
    code_challenge: Option<&str>,
) -> Result<String, ServiceError> {
    let client_id = oauth.client_id.as_deref().ok_or_else(|| {
        ServiceError::Unavailable(format!(
            "{} OAuth client id is not configured",
            provider.as_str()
        ))
    })?;
    let endpoint = oauth
        .authorization_url
        .as_deref()
        .or_else(|| provider.authorization_endpoint())
        .ok_or_else(|| {
            ServiceError::BadRequest("provider does not use server OAuth".to_string())
        })?;
    let scope = provider.required_scope().unwrap_or_default();
    let mut params = vec![
        ("client_id", client_id.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("response_type", "code".to_string()),
        ("scope", scope.to_string()),
        ("state", state.to_string()),
    ];
    if let Some(challenge) = code_challenge {
        params.push(("code_challenge", challenge.to_string()));
        params.push(("code_challenge_method", "S256".to_string()));
    }
    match provider {
        CloudProvider::GoogleDrive => {
            params.push(("access_type", "offline".to_string()));
            params.push(("prompt", "consent".to_string()));
        }
        CloudProvider::MicrosoftOneDrive => {
            params.push(("response_mode", "query".to_string()));
        }
        CloudProvider::AppleICloud | CloudProvider::AmazonS3 | CloudProvider::CloudflareR2 => {}
        CloudProvider::Dropbox => {
            params.push(("token_access_type", "offline".to_string()));
        }
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{}={}", query_escape(key), query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    Ok(format!("{endpoint}?{query}"))
}

fn policy(config: &Config, retention_hours: i32) -> MobilePolicy {
    MobilePolicy {
        retention_hours,
        default_segment_seconds: config.default_segment_seconds,
        max_segment_seconds: config.max_segment_seconds,
        max_segment_bytes: config.max_segment_bytes,
        upload_url_ttl_seconds: config.upload_url_ttl.as_secs(),
        download_url_ttl_seconds: config.download_url_ttl.as_secs(),
        cloud_copy_supported_providers: vec![
            CloudProvider::GoogleDrive.as_str(),
            CloudProvider::MicrosoftOneDrive.as_str(),
            CloudProvider::AppleICloud.as_str(),
            CloudProvider::Dropbox.as_str(),
        ],
        supported_use_cases: SUPPORTED_USE_CASES.to_vec(),
    }
}

/// Clones the SeaORM pool handle for one request. Acquiring an individual
/// connection remains SeaORM's responsibility when a query is executed.
async fn db_conn(state: &AppState) -> Result<PgConn, ServiceError> {
    let pool = state.pg_pool.as_ref().ok_or_else(|| {
        ServiceError::Unavailable("sound recorder database is not configured".to_string())
    })?;
    Ok(pool.clone())
}

fn db_error(error: sea_orm::DbErr) -> ServiceError {
    error!(error = %error, "postgres query failed");
    ServiceError::Internal("postgres query failed".to_string())
}

fn require_internal_auth(config: &Config, headers: &HeaderMap) -> Result<(), ServiceError> {
    let Some(expected) = config.server_auth_secret.as_deref() else {
        return Err(ServiceError::Unavailable(
            "internal auth secret is not configured".to_string(),
        ));
    };
    let provided = internal_token(headers).unwrap_or("");
    if !provided.is_empty() && const_time_eq(provided.as_bytes(), expected.as_bytes()) {
        Ok(())
    } else {
        Err(ServiceError::Unauthorized)
    }
}

/// How much we trust the caller of device registration, which decides whose
/// account a device may attach to.
enum RegistrationTrust {
    /// A shared-auth access token introspected against the RDS-backed authority.
    /// The account key uses shared-auth's stable UUID and never a client claim.
    SharedAuth(SharedAuthIdentity),
    /// A Supabase access token verified server-side. The account is keyed to the
    /// verified `sub`, so it cannot be spoofed by the client.
    Supabase(SupabaseIdentity),
    /// The shared registration bearer matched. A trusted server-to-server caller
    /// may assert an arbitrary `externalSubject`.
    TrustedServer,
    /// Open registration with no verified identity. The account is keyed to the
    /// install id and any client-supplied `externalSubject` is ignored, so an
    /// anonymous caller can never claim another user's account.
    Public,
}

async fn authorize_registration(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<RegistrationTrust, ServiceError> {
    if let Some(token) = shared_auth_token(headers) {
        if !state.config.shared_auth.is_enabled() {
            return Err(ServiceError::Unavailable(
                "shared-auth introspection is not configured".to_string(),
            ));
        }
        return Ok(RegistrationTrust::SharedAuth(
            introspect_shared_auth(state, token).await?,
        ));
    }
    if let Some(verifier) = &state.supabase {
        if let Some(token) = supabase_token(headers) {
            let identity = verifier.verify(&state.http, token).await?;
            return Ok(RegistrationTrust::Supabase(identity));
        }
    }
    if let Some(expected) = state.config.registration_bearer.as_deref() {
        let provided = bearer_token(headers).unwrap_or("");
        if !provided.is_empty() && const_time_eq(provided.as_bytes(), expected.as_bytes()) {
            return Ok(RegistrationTrust::TrustedServer);
        }
        return Err(ServiceError::Unauthorized);
    }
    if state.config.allow_public_device_registration {
        Ok(RegistrationTrust::Public)
    } else {
        Err(ServiceError::Unavailable(
            "device registration is disabled until a shared-auth or Supabase token, SOUND_RECORDER_REGISTRATION_BEARER, or SOUND_RECORDER_ALLOW_PUBLIC_DEVICE_REGISTRATION is configured".to_string(),
        ))
    }
}

/// Authenticates the Supabase owner for the deletion workflow. A previously
/// marked-deleted row remains eligible only while its external subject is still
/// present, allowing an idempotent retry if Supabase Auth was temporarily down.
/// Do not use this helper for ordinary account reads.
async fn authenticate_supabase_account_for_deletion(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(String, SupabaseIdentity, PgConn), ServiceError> {
    let verifier = state
        .supabase
        .as_ref()
        .ok_or_else(|| ServiceError::Unavailable("Supabase auth is not configured".to_string()))?;
    let token = supabase_token(headers).ok_or(ServiceError::Unauthorized)?;
    let identity = verifier.verify(&state.http, token).await?;
    let external_subject = identity.external_subject();
    let client = db_conn(state).await?;
    let row = client
        .query_opt(
            "select id::text
             from sound_recorder_accounts
             where external_subject = $1 and status in ('active', 'deleted')",
            &[&external_subject],
        )
        .await
        .map_err(db_error)?;
    let Some(row) = row else {
        return Err(ServiceError::NotFound("account not found".to_string()));
    };
    Ok((row.get("id"), identity, client))
}

async fn authenticate_device(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(DeviceAuth, PgConn), ServiceError> {
    if !state.config.token_pepper_configured {
        return Err(ServiceError::Unavailable(
            "device token pepper is not configured".to_string(),
        ));
    }
    let token = bearer_token(headers).ok_or(ServiceError::Unauthorized)?;
    let token_hash = hash_secret(token, &state.config.token_pepper);
    let client = db_conn(state).await?;
    let row = client
        .query_opt(
            "select d.id::text as device_id, d.install_id, d.account_id::text as account_id,
                    a.retention_hours
             from sound_recorder_devices d
             join sound_recorder_accounts a on a.id = d.account_id
             where d.token_hash = $1 and d.status = 'active' and a.status = 'active'",
            &[&token_hash],
        )
        .await
        .map_err(db_error)?;
    let Some(row) = row else {
        return Err(ServiceError::Unauthorized);
    };
    let auth = DeviceAuth {
        device_id: row.get("device_id"),
        install_id: row.get("install_id"),
        account_id: row.get("account_id"),
        retention_hours: row.get("retention_hours"),
    };
    let _ = client
        .execute(
            "update sound_recorder_devices
             set last_seen_at = now(), updated_at = now()
             where id = $1::uuid",
            &[&auth.device_id],
        )
        .await;
    Ok((auth, client))
}

async fn audit_event(
    client: &DbClient,
    account_id: Option<&str>,
    device_id: Option<&str>,
    event_type: &str,
    payload: Value,
) {
    let event_hash = hash_secret(
        &format!("{event_type}:{}:{}", now_ms(), Uuid::new_v4()),
        "sound-recorder-audit",
    );
    let result = client
        .execute(
            "insert into sound_recorder_audit_events
              (account_id, device_id, event_type, event_hash, payload)
             values ($1::uuid, $2::uuid, $3, $4, $5)
             on conflict (event_hash) do nothing",
            &[&account_id, &device_id, &event_type, &event_hash, &payload],
        )
        .await;
    if let Err(err) = result {
        warn!(error = %err, event_type, "failed to insert sound recorder audit event");
    }
}

async fn find_or_create_account(
    client: &DbClient,
    config: &Config,
    external_subject: Option<&str>,
    display_name: Option<String>,
    legal_region: Option<&str>,
) -> Result<(String, i32), ServiceError> {
    let display_name = clean_string(display_name, 160);
    if let Some(external_subject) = external_subject {
        if external_subject.len() > 240 || external_subject.chars().any(char::is_control) {
            return Err(ServiceError::BadRequest(
                "externalSubject is invalid".to_string(),
            ));
        }
        let account_id = Uuid::new_v4().to_string();
        let row = client
            .query_opt(
                "insert into sound_recorder_accounts
                  (id, external_subject, display_name, legal_region, retention_hours)
                 values ($1::uuid, $2, $3, $4, $5)
                 on conflict (external_subject) where external_subject is not null
                 do update set
                   display_name = coalesce(excluded.display_name, sound_recorder_accounts.display_name),
                   legal_region = coalesce(excluded.legal_region, sound_recorder_accounts.legal_region),
                   updated_at = now()
                 where sound_recorder_accounts.status = 'active'
                 returning id::text, retention_hours",
                &[
                    &account_id,
                    &external_subject,
                    &display_name,
                    &legal_region,
                    &config.default_retention_hours,
                ],
            )
            .await
            .map_err(db_error)?;
        let Some(row) = row else {
            return Err(ServiceError::Conflict(
                "account is paused, locked, or deleted".to_string(),
            ));
        };
        return Ok((row.get("id"), row.get("retention_hours")));
    }

    let account_id = Uuid::new_v4().to_string();
    let row = client
        .query_one(
            "insert into sound_recorder_accounts
              (id, display_name, legal_region, retention_hours)
             values ($1::uuid, $2, $3, $4)
             returning id::text, retention_hours",
            &[
                &account_id,
                &display_name,
                &legal_region,
                &config.default_retention_hours,
            ],
        )
        .await
        .map_err(db_error)?;
    Ok((row.get("id"), row.get("retention_hours")))
}

/// Resolves the account subject + default display name from the registration
/// trust level. This is the single place that decides whose account a device
/// attaches to, so a client can never assert another identity's subject.
fn resolve_registration_subject(
    trust: &RegistrationTrust,
    req: &RegisterDeviceRequest,
    install_id: &str,
) -> Result<(Option<String>, Option<String>), ServiceError> {
    match trust {
        RegistrationTrust::SharedAuth(identity) => Ok((
            Some(identity.external_subject()),
            identity.email.clone().or_else(|| req.display_name.clone()),
        )),
        RegistrationTrust::Supabase(identity) => Ok((
            Some(identity.external_subject()),
            identity.email.clone().or_else(|| req.display_name.clone()),
        )),
        RegistrationTrust::TrustedServer => {
            let subject = req
                .external_subject
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| validate_nonempty(value, "externalSubject", 240))
                .transpose()?;
            Ok((subject, req.display_name.clone()))
        }
        RegistrationTrust::Public => {
            // No verified identity: key the account to the install id and ignore
            // any client-supplied externalSubject so accounts can't be claimed.
            Ok((
                Some(format!("install:{install_id}")),
                req.display_name.clone(),
            ))
        }
    }
}

async fn home(State(state): State<AppState>) -> Html<String> {
    record_request("GET", "/", StatusCode::OK);
    Html(render_home(&state.config))
}

async fn privacy() -> Html<&'static str> {
    record_request("GET", "/privacy", StatusCode::OK);
    Html(PRIVACY_HTML)
}

async fn download_ios(State(state): State<AppState>) -> Result<Redirect, ServiceError> {
    if let Some(url) = &state.config.ios_app_store_url {
        record_request("GET", "/download/ios", StatusCode::FOUND);
        Ok(Redirect::temporary(url))
    } else {
        Err(ServiceError::NotFound(
            "iOS App Store URL is not configured yet".to_string(),
        ))
    }
}

async fn download_android(State(state): State<AppState>) -> Result<Redirect, ServiceError> {
    if let Some(url) = &state.config.android_play_store_url {
        record_request("GET", "/download/android", StatusCode::FOUND);
        Ok(Redirect::temporary(url))
    } else {
        Err(ServiceError::NotFound(
            "Android Play Store URL is not configured yet".to_string(),
        ))
    }
}

async fn healthz(State(state): State<AppState>) -> Json<HealthResponse> {
    record_request("GET", "/healthz", StatusCode::OK);
    Json(HealthResponse {
        ok: true,
        service: SERVICE_NAME,
        mode: "http",
        postgres_configured: state.config.database_url.is_some(),
        s3_configured: state.s3.is_some() && state.config.s3.is_configured(),
        storage_ready: None,
        storage_history_compatible: None,
        storage_probe_mode: state.config.s3.readiness_probe_mode(),
        storage_backend: state.config.s3.backend.as_str(),
        storage_backend_fingerprint: state.config.s3.backend_fingerprint.clone(),
        storage_versioning_mode: state.config.s3.versioning_mode,
        configuration_valid: state.config.validation_errors.is_empty()
            && state.config.s3.validation_errors.is_empty()
            && state.config.mirror.validation_errors.is_empty()
            && state.config.supabase.validation_errors.is_empty()
            && state.config.shared_auth.validation_errors.is_empty(),
        token_pepper_configured: state.config.token_pepper_configured,
        registration_configured: state.config.shared_auth.is_enabled()
            || state.supabase.is_some()
            || state.config.registration_bearer.is_some()
            || state.config.allow_public_device_registration,
        server_auth_configured: state.config.server_auth_secret.is_some(),
        cloud_token_sealer_configured: state.cloud_sealer.is_some(),
        google_drive_configured: state.config.google_oauth.client_id.is_some()
            && state.config.google_oauth.client_secret.is_some(),
        microsoft_onedrive_configured: state.config.microsoft_oauth.client_id.is_some()
            && state.config.microsoft_oauth.client_secret.is_some(),
        dropbox_configured: state.config.dropbox_oauth.client_id.is_some()
            && state.config.dropbox_oauth.client_secret.is_some(),
        supabase_configured: state.supabase.is_some(),
        supabase_data_api_configured: state.config.supabase.is_data_api_enabled(),
        supabase_accounts_configured: state.config.supabase.account_features_configured(),
        supabase_ready: None,
        supabase_required: state.config.require_supabase,
        shared_auth_configured: state.config.shared_auth.is_enabled(),
        shared_auth_required_aal: state.config.shared_auth.required_aal,
        retention_hours: state.config.default_retention_hours,
        mirror_configured: state.mirror.is_some() && state.config.mirror.is_configured(),
        mirror_ready: None,
        mirror_probe_mode: mirror_probe_mode(&state.config.mirror),
        mirror_backend: (!state.config.mirror.bucket.is_empty())
            .then(|| state.config.mirror.backend.as_str()),
        mirror_backend_fingerprint: (!state.config.mirror.bucket.is_empty())
            .then(|| state.config.mirror.backend_fingerprint.clone()),
        mirror_readiness_required: state.config.mirror_readiness_required,
    })
}

async fn postgres_is_reachable(state: &AppState) -> bool {
    let client = match tokio::time::timeout(POSTGRES_PROBE_TIMEOUT, db_conn(state)).await {
        Ok(Ok(client)) => client,
        _ => return false,
    };
    matches!(
        tokio::time::timeout(POSTGRES_PROBE_TIMEOUT, client.simple_query("select 1")).await,
        Ok(Ok(_))
    )
}

fn mirror_probe_mode(config: &S3StorageConfig) -> &'static str {
    if config.bucket.is_empty() {
        "unconfigured"
    } else if config.readiness_object_key.is_some() {
        "head_object"
    } else {
        "head_probe_not_found_ok"
    }
}

/// `None` when no mirror is intended; otherwise whether the mirror target is
/// reachable with the configured credentials.
async fn mirror_is_ready(state: &AppState) -> Option<bool> {
    if state.config.mirror.bucket.is_empty() && state.config.mirror.validation_errors.is_empty() {
        return None;
    }
    let Some(mirror) = state.mirror.as_ref() else {
        return Some(false);
    };
    if let Some(key) = state.config.mirror.readiness_object_key.as_deref() {
        return Some(matches!(
            tokio::time::timeout(
                STORAGE_PROBE_TIMEOUT,
                mirror
                    .head_object()
                    .bucket(&state.config.mirror.bucket)
                    .key(key)
                    .send()
            )
            .await,
            Ok(Ok(_))
        ));
    }
    // Without a sentinel object, HeadObject on a never-written probe key still
    // proves endpoint, credentials, and bucket-level object access: an
    // authorized miss is a clean 404, while bad credentials, a bad endpoint,
    // or a missing bucket surface as other errors.
    let key = format!("{}/.mirror-readiness-probe", state.config.mirror.key_prefix);
    let result = tokio::time::timeout(
        STORAGE_PROBE_TIMEOUT,
        mirror
            .head_object()
            .bucket(&state.config.mirror.bucket)
            .key(key)
            .send(),
    )
    .await;
    Some(match result {
        Ok(Ok(_)) => true,
        Ok(Err(err)) => err
            .as_service_error()
            .map(|service_error| service_error.is_not_found())
            .unwrap_or(false),
        Err(_) => false,
    })
}

async fn storage_is_ready(state: &AppState) -> bool {
    let Some(s3) = state.s3.as_ref() else {
        return false;
    };
    if !state.config.s3.is_configured() {
        return false;
    }
    if let Some(key) = state.config.s3.readiness_object_key.as_deref() {
        return matches!(
            tokio::time::timeout(
                STORAGE_PROBE_TIMEOUT,
                s3.head_object()
                    .bucket(&state.config.s3.bucket)
                    .key(key)
                    .send()
            )
            .await,
            Ok(Ok(_))
        );
    }
    if !state.config.s3.allow_signing_only_readiness {
        return false;
    }
    // Explicit local-development escape hatch. This is intentionally not the
    // production default because signing proves neither remote reachability nor
    // that the configured principal can access the sentinel/prefix.
    let key = format!("{}/.readiness-signing-probe", state.config.s3.key_prefix);
    let Ok(presigning_config) = PresigningConfig::builder()
        .expires_in(Duration::from_secs(30))
        .build()
    else {
        return false;
    };
    matches!(
        tokio::time::timeout(
            STORAGE_PROBE_TIMEOUT,
            s3.head_object()
                .bucket(&state.config.s3.bucket)
                .key(key)
                .presigned(presigning_config)
        )
        .await,
        Ok(Ok(_))
    )
}

fn storage_history_compatible(
    has_mismatch: bool,
    has_unmarked: bool,
    allow_unmarked: bool,
) -> bool {
    !has_mismatch && (!has_unmarked || allow_unmarked)
}

async fn storage_history_is_compatible(state: &AppState) -> bool {
    if let Some((checked_at, compatible)) = *state.storage_history_cache.read().await {
        if checked_at.elapsed() < STORAGE_HISTORY_CACHE_TTL {
            return compatible;
        }
    }
    let _refresh_guard = state.storage_history_refresh_lock.lock().await;
    if let Some((checked_at, compatible)) = *state.storage_history_cache.read().await {
        if checked_at.elapsed() < STORAGE_HISTORY_CACHE_TTL {
            return compatible;
        }
    }
    let client = match tokio::time::timeout(POSTGRES_PROBE_TIMEOUT, db_conn(state)).await {
        Ok(Ok(client)) => client,
        _ => return false,
    };
    let row = match tokio::time::timeout(
        POSTGRES_PROBE_TIMEOUT,
        client.query_one(
            "select
               (exists (
                 select 1 from sound_recorder_segments
                 where storage_bucket <> '' and storage_key <> ''
                   and (status <> 'expired' or meta_data->>($1::text) = 'true')
                   and nullif(meta_data->>($2::text), '') is not null
                   and meta_data->>($2::text) <> $3
               ) or exists (
                 select 1 from sound_recorder_upload_sessions
                 where status = 'active'
                   and nullif(meta_data->>($2::text), '') is not null
                   and meta_data->>($2::text) <> $3
               )) as has_mismatch,
               (exists (
                 select 1 from sound_recorder_segments
                 where storage_bucket <> '' and storage_key <> ''
                   and (status <> 'expired' or meta_data->>($1::text) = 'true')
                   and nullif(meta_data->>($2::text), '') is null
               ) or exists (
                 select 1 from sound_recorder_upload_sessions
                 where status = 'active'
                   and nullif(meta_data->>($2::text), '') is null
               )) as has_unmarked",
            &[
                &RETENTION_DELETE_PENDING_META_KEY,
                &STORAGE_FINGERPRINT_META_KEY,
                &state.config.s3.backend_fingerprint,
            ],
        ),
    )
    .await
    {
        Ok(Ok(row)) => row,
        _ => return false,
    };
    let compatible = storage_history_compatible(
        row.get("has_mismatch"),
        row.get("has_unmarked"),
        state.config.s3.allow_unmarked_storage_history,
    );
    *state.storage_history_cache.write().await = Some((Instant::now(), compatible));
    compatible
}

async fn require_storage_history_compatible(state: &AppState) -> Result<(), ServiceError> {
    // Unit/local rendering paths can operate without a database. Every real
    // storage-backed deployment already requires Postgres, and must not serve
    // object operations merely because an orchestrator ignored readiness.
    if state.config.database_url.is_some() && !storage_history_is_compatible(state).await {
        return Err(ServiceError::Unavailable(
            "object-storage history is incompatible with the configured backend".to_string(),
        ));
    }
    Ok(())
}

async fn supabase_is_ready(state: &AppState) -> bool {
    let Some(verifier) = state.supabase.as_deref() else {
        return false;
    };
    if !state.config.supabase.account_features_configured() {
        return false;
    }
    let probe = async {
        if verifier.jwt_secret.is_none() {
            // The JWKS endpoint is a real, object-level Auth dependency probe
            // and must contain at least one usable signing key.
            return verifier.refresh_jwks(&state.http).await;
        }
        // Legacy HS256 projects can legitimately publish an empty asymmetric
        // JWKS. Probe the documented GoTrue health route instead.
        let health_url =
            state.config.supabase.auth_health_url().ok_or_else(|| {
                ServiceError::Unavailable("Supabase URL is not configured".into())
            })?;
        let api_key = state
            .config
            .supabase
            .publishable_key
            .as_deref()
            .ok_or_else(|| {
                ServiceError::Unavailable("Supabase publishable key is not configured".into())
            })?;
        let response = state
            .http
            .get(health_url)
            .header("apikey", api_key)
            .send()
            .await
            .map_err(|_| ServiceError::Unavailable("Supabase Auth probe failed".into()))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(ServiceError::Unavailable(format!(
                "Supabase Auth probe returned status {}",
                response.status().as_u16()
            )))
        }
    };
    matches!(
        tokio::time::timeout(SUPABASE_PROBE_TIMEOUT, probe).await,
        Ok(Ok(()))
    )
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let supabase_probe = async {
        if state.config.require_supabase {
            Some(supabase_is_ready(&state).await)
        } else {
            None
        }
    };
    let (
        postgres_reachable,
        storage_ready,
        storage_history_compatible,
        supabase_ready,
        mirror_ready,
    ) = tokio::join!(
        postgres_is_reachable(&state),
        storage_is_ready(&state),
        storage_history_is_compatible(&state),
        supabase_probe,
        mirror_is_ready(&state)
    );
    let registration_configured = state.config.shared_auth.is_enabled()
        || state.supabase.is_some()
        || state.config.registration_bearer.is_some()
        || state.config.allow_public_device_registration;
    let supabase_accounts_configured = state.config.supabase.account_features_configured();
    // A configured-but-invalid mirror always fails readiness (misconfiguration
    // must be caught at rollout); a probe failure of a valid mirror only fails
    // readiness when the operator opted into gating on the backup target.
    let mirror_intended =
        !state.config.mirror.bucket.is_empty() || !state.config.mirror.validation_errors.is_empty();
    let mirror_ok = state.config.mirror.validation_errors.is_empty()
        && (!mirror_intended || state.mirror.is_some())
        && (!state.config.mirror_readiness_required
            || !mirror_intended
            || mirror_ready == Some(true));
    let ready = state.config.database_url.is_some()
        && postgres_reachable
        && state.config.validation_errors.is_empty()
        && state.s3.is_some()
        && state.config.s3.is_configured()
        && storage_ready
        && storage_history_compatible
        && mirror_ok
        && state.config.token_pepper_configured
        && registration_configured
        && state.config.server_auth_secret.is_some()
        && (!state.config.require_supabase
            || (supabase_accounts_configured && supabase_ready == Some(true)));
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    record_request("GET", "/readyz", status);
    (
        status,
        Json(HealthResponse {
            ok: ready,
            service: SERVICE_NAME,
            mode: "http",
            postgres_configured: state.config.database_url.is_some(),
            s3_configured: state.s3.is_some() && state.config.s3.is_configured(),
            storage_ready: Some(storage_ready),
            storage_history_compatible: Some(storage_history_compatible),
            storage_probe_mode: state.config.s3.readiness_probe_mode(),
            storage_backend: state.config.s3.backend.as_str(),
            storage_backend_fingerprint: state.config.s3.backend_fingerprint.clone(),
            storage_versioning_mode: state.config.s3.versioning_mode,
            configuration_valid: state.config.validation_errors.is_empty()
                && state.config.s3.validation_errors.is_empty()
                && state.config.mirror.validation_errors.is_empty()
                && state.config.supabase.validation_errors.is_empty()
                && state.config.shared_auth.validation_errors.is_empty(),
            token_pepper_configured: state.config.token_pepper_configured,
            registration_configured,
            server_auth_configured: state.config.server_auth_secret.is_some(),
            cloud_token_sealer_configured: state.cloud_sealer.is_some(),
            google_drive_configured: state.config.google_oauth.client_id.is_some()
                && state.config.google_oauth.client_secret.is_some(),
            microsoft_onedrive_configured: state.config.microsoft_oauth.client_id.is_some()
                && state.config.microsoft_oauth.client_secret.is_some(),
            dropbox_configured: state.config.dropbox_oauth.client_id.is_some()
                && state.config.dropbox_oauth.client_secret.is_some(),
            supabase_configured: state.supabase.is_some(),
            supabase_data_api_configured: state.config.supabase.is_data_api_enabled(),
            supabase_accounts_configured,
            supabase_ready,
            supabase_required: state.config.require_supabase,
            shared_auth_configured: state.config.shared_auth.is_enabled(),
            shared_auth_required_aal: state.config.shared_auth.required_aal,
            retention_hours: state.config.default_retention_hours,
            mirror_configured: state.mirror.is_some() && state.config.mirror.is_configured(),
            mirror_ready,
            mirror_probe_mode: mirror_probe_mode(&state.config.mirror),
            mirror_backend: (!state.config.mirror.bucket.is_empty())
                .then(|| state.config.mirror.backend.as_str()),
            mirror_backend_fingerprint: (!state.config.mirror.bucket.is_empty())
                .then(|| state.config.mirror.backend_fingerprint.clone()),
            mirror_readiness_required: state.config.mirror_readiness_required,
        }),
    )
}

async fn metrics() -> impl IntoResponse {
    UPTIME_SECONDS.set(STARTED_AT.elapsed().as_secs() as i64);
    let encoder = TextEncoder::new();
    let families = prometheus::gather();
    let mut buffer = Vec::new();
    let status = match encoder.encode(&families, &mut buffer) {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    record_request("GET", "/metrics", status);
    (
        status,
        [(header::CONTENT_TYPE, encoder.format_type().to_string())],
        buffer,
    )
}

async fn api_docs_html() -> Html<&'static str> {
    record_request("GET", "/docs/api", StatusCode::OK);
    Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl IntoResponse {
    record_request("GET", "/api/docs.json", StatusCode::OK);
    (
        [(header::CONTENT_TYPE, "application/json")],
        include_str!("../generated/api-docs.json"),
    )
}

fn user_data_limit(requested: Option<usize>) -> usize {
    requested
        .unwrap_or(DEFAULT_USER_DATA_LIMIT)
        .clamp(1, MAX_USER_DATA_LIMIT)
}

fn validate_choice(field: &str, value: &str, choices: &[&str]) -> Result<(), ServiceError> {
    if choices.contains(&value) {
        Ok(())
    } else {
        Err(ServiceError::BadRequest(format!(
            "{field} must be one of {}",
            choices.join(", ")
        )))
    }
}

fn validate_integer_range(
    field: &str,
    value: i64,
    minimum: i64,
    maximum: i64,
) -> Result<(), ServiceError> {
    if (minimum..=maximum).contains(&value) {
        Ok(())
    } else {
        Err(ServiceError::BadRequest(format!(
            "{field} must be between {minimum} and {maximum}"
        )))
    }
}

fn validate_float_range(
    field: &str,
    value: f64,
    minimum: f64,
    maximum: f64,
) -> Result<(), ServiceError> {
    if value.is_finite() && (minimum..=maximum).contains(&value) {
        Ok(())
    } else {
        Err(ServiceError::BadRequest(format!(
            "{field} must be between {minimum} and {maximum}"
        )))
    }
}

fn validate_sample_rate(field: &str, value: i64) -> Result<(), ServiceError> {
    const SAMPLE_RATES: &[i64] = &[8_000, 16_000, 22_050, 24_000, 44_100, 48_000];
    if SAMPLE_RATES.contains(&value) {
        Ok(())
    } else {
        Err(ServiceError::BadRequest(format!(
            "{field} must be a supported sample rate"
        )))
    }
}

struct SupabaseDataContext {
    token: String,
    base_url: String,
    publishable_key: String,
    identity: SupabaseIdentity,
}

async fn supabase_data_context(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<SupabaseDataContext, ServiceError> {
    let verifier = state
        .supabase
        .as_deref()
        .ok_or_else(|| ServiceError::Unavailable("Supabase auth is not configured".to_string()))?;
    let token = supabase_token(headers)
        .ok_or(ServiceError::Unauthorized)?
        .to_string();
    let identity = verifier.verify(&state.http, &token).await?;
    let base_url =
        state.config.supabase.url.clone().ok_or_else(|| {
            ServiceError::Unavailable("Supabase URL is not configured".to_string())
        })?;
    let publishable_key = state
        .config
        .supabase
        .publishable_key
        .clone()
        .ok_or_else(|| {
            ServiceError::Unavailable("Supabase publishable key is not configured".to_string())
        })?;
    Ok(SupabaseDataContext {
        token,
        base_url,
        publishable_key,
        identity,
    })
}

async fn fetch_supabase_rows<T: DeserializeOwned>(
    state: &AppState,
    headers: &HeaderMap,
    table: &str,
    columns: &[&str],
    order: &str,
    limit: usize,
) -> Result<Vec<T>, ServiceError> {
    let context = supabase_data_context(state, headers).await?;
    fetch_supabase_rows_with_context(state, &context, table, columns, order, limit).await
}

async fn fetch_supabase_rows_with_context<T: DeserializeOwned>(
    state: &AppState,
    context: &SupabaseDataContext,
    table: &str,
    columns: &[&str],
    order: &str,
    limit: usize,
) -> Result<Vec<T>, ServiceError> {
    let response = state
        .http
        .get(format!("{}/rest/v1/{table}", context.base_url))
        .header("apikey", &context.publishable_key)
        .bearer_auth(&context.token)
        .query(&[
            ("select", columns.join(",")),
            ("order", order.to_string()),
            ("limit", limit.to_string()),
        ])
        .send()
        .await
        .map_err(|_| {
            ServiceError::Unavailable(format!(
                "Supabase Data API request for {table} could not be completed"
            ))
        })?;
    let status = response.status();
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(ServiceError::Unauthorized);
    }
    if !status.is_success() {
        return Err(ServiceError::Unavailable(format!(
            "Supabase Data API request for {table} failed with {status}"
        )));
    }
    response.json::<Vec<T>>().await.map_err(|_| {
        ServiceError::Internal(format!(
            "Supabase Data API returned an invalid {table} payload"
        ))
    })
}

async fn list_acoustic_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UserDataQuery>,
) -> Result<Json<UserDataList<AcousticEvent>>, ServiceError> {
    let data = fetch_supabase_rows(
        &state,
        &headers,
        ACOUSTIC_EVENTS_TABLE,
        ACOUSTIC_EVENTS_COLUMNS,
        "started_at.desc",
        user_data_limit(query.limit),
    )
    .await?;
    let count = data.len();
    record_request("GET", "/api/v1/data/acoustic-events", StatusCode::OK);
    Ok(Json(UserDataList { count, data }))
}

async fn list_user_consents(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UserDataQuery>,
) -> Result<Json<UserDataList<UserConsent>>, ServiceError> {
    let data = fetch_supabase_rows(
        &state,
        &headers,
        USER_CONSENTS_TABLE,
        USER_CONSENTS_COLUMNS,
        "accepted_at.desc",
        user_data_limit(query.limit),
    )
    .await?;
    let count = data.len();
    record_request("GET", "/api/v1/data/user-consents", StatusCode::OK);
    Ok(Json(UserDataList { count, data }))
}

async fn list_devices(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UserDataQuery>,
) -> Result<Json<UserDataList<DeviceRecord>>, ServiceError> {
    let data = fetch_supabase_rows(
        &state,
        &headers,
        DEVICES_TABLE,
        DEVICES_COLUMNS,
        "last_seen_at.desc",
        user_data_limit(query.limit),
    )
    .await?;
    let count = data.len();
    record_request("GET", "/api/v1/data/devices", StatusCode::OK);
    Ok(Json(UserDataList { count, data }))
}

async fn get_user_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<UserSettingsResponse>, ServiceError> {
    let context = supabase_data_context(&state, &headers).await?;
    let mut rows = fetch_supabase_rows_with_context(
        &state,
        &context,
        USER_SETTINGS_TABLE,
        USER_SETTINGS_COLUMNS,
        "updated_at.desc",
        1,
    )
    .await?;
    let data = rows.pop().unwrap_or_else(|| {
        UserSettingsInput::default()
            .into_interface(context.identity.subject.clone(), Utc::now().to_rfc3339())
    });
    record_request("GET", "/api/v1/data/user-settings", StatusCode::OK);
    Ok(Json(UserSettingsResponse { data }))
}

async fn update_user_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<UserSettingsInput>,
) -> Result<Json<UserSettingsResponse>, ServiceError> {
    input.validate()?;
    let context = supabase_data_context(&state, &headers).await?;
    let mut payload = serde_json::to_value(&input)
        .map_err(|_| ServiceError::BadRequest("settings payload is invalid".to_string()))?;
    let Value::Object(ref mut object) = payload else {
        return Err(ServiceError::Internal(
            "settings payload could not be serialized".to_string(),
        ));
    };
    object.insert(
        "updated_at".to_string(),
        Value::String(Utc::now().to_rfc3339()),
    );

    let response = state
        .http
        .post(format!(
            "{}/rest/v1/{USER_SETTINGS_TABLE}",
            context.base_url
        ))
        .header("apikey", &context.publishable_key)
        .bearer_auth(&context.token)
        .header(
            "Prefer",
            "resolution=merge-duplicates,missing=default,return=representation",
        )
        .query(&[("on_conflict", "user_id")])
        .json(&payload)
        .send()
        .await
        .map_err(|_| {
            ServiceError::Unavailable(
                "Supabase Data API settings update could not be completed".to_string(),
            )
        })?;
    let status = response.status();
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(ServiceError::Unauthorized);
    }
    if !status.is_success() {
        return Err(ServiceError::Unavailable(format!(
            "Supabase Data API settings update failed with {status}"
        )));
    }
    let mut rows = response.json::<Vec<UserSettings>>().await.map_err(|_| {
        ServiceError::Internal(
            "Supabase Data API returned an invalid user_settings payload".to_string(),
        )
    })?;
    let data = rows.pop().ok_or_else(|| {
        ServiceError::Internal("Supabase Data API returned no updated settings".to_string())
    })?;
    if data.user_id != context.identity.subject {
        return Err(ServiceError::Internal(
            "Supabase Data API returned settings for another user".to_string(),
        ));
    }
    record_request("PUT", "/api/v1/data/user-settings", StatusCode::OK);
    Ok(Json(UserSettingsResponse { data }))
}

async fn register_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegisterDeviceRequest>,
) -> Result<Json<RegisterDeviceResponse>, ServiceError> {
    let trust = authorize_registration(&state, &headers).await?;
    if !state.config.token_pepper_configured {
        return Err(ServiceError::Unavailable(
            "device token pepper is not configured".to_string(),
        ));
    }
    if !req.recording_indicator_acknowledged {
        return Err(ServiceError::BadRequest(
            "recordingIndicatorAcknowledged must be true".to_string(),
        ));
    }
    let platform = normalize_platform(&req.platform)?;
    let install_id = validate_nonempty(&req.install_id, "installId", 160)?;
    let consent_version = validate_nonempty(&req.consent_version, "consentVersion", 80)?;
    let legal_region = validate_legal_region(req.legal_region.clone())?;
    let (external_subject, display_name_default) =
        resolve_registration_subject(&trust, &req, &install_id)?;
    let device_label = clean_string(req.device_label.clone(), 160);
    let app_version = clean_string(req.app_version.clone(), 80);
    let os_version = clean_string(req.os_version.clone(), 80);
    let consent_accepted_at = req.consent_accepted_at.unwrap_or_else(Utc::now);
    let attestation = validate_meta(req.attestation.clone())?;
    let token = new_device_token();
    let token_hash = hash_secret(&token, &state.config.token_pepper);
    let token_last4 = last4(&token);

    let client = db_conn(&state).await?;
    let (account_id, retention_hours) = find_or_create_account(
        &client,
        &state.config,
        external_subject.as_deref(),
        display_name_default,
        legal_region.as_deref(),
    )
    .await?;
    let device_id = Uuid::new_v4().to_string();
    let row = client
        .query_one(
            "insert into sound_recorder_devices
              (id, account_id, platform, install_id, device_label, app_version, os_version,
               token_hash, token_last4, consent_version, consent_accepted_at,
               recording_indicator_acknowledged, last_seen_at)
             values
              ($1::uuid, $2::uuid, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now())
             on conflict (account_id, install_id) do update set
               platform = excluded.platform,
               status = 'active',
               device_label = excluded.device_label,
               app_version = excluded.app_version,
               os_version = excluded.os_version,
               token_hash = excluded.token_hash,
               token_last4 = excluded.token_last4,
               consent_version = excluded.consent_version,
               consent_accepted_at = excluded.consent_accepted_at,
               recording_indicator_acknowledged = excluded.recording_indicator_acknowledged,
               last_seen_at = now(),
               updated_at = now()
             returning id::text",
            &[
                &device_id,
                &account_id,
                &platform,
                &install_id,
                &device_label,
                &app_version,
                &os_version,
                &token_hash,
                &token_last4,
                &consent_version,
                &consent_accepted_at,
                &req.recording_indicator_acknowledged,
            ],
        )
        .await
        .map_err(db_error)?;
    let device_id: String = row.get("id");
    audit_event(
        &client,
        Some(&account_id),
        Some(&device_id),
        "sound_recorder.device.registered",
        json!({
            "platform": platform,
            "installId": install_id,
            "consentVersion": consent_version,
            "legalRegion": legal_region,
            "attestationKeys": attestation.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()
        }),
    )
    .await;
    record_request("POST", "/api/mobile/v1/devices/register", StatusCode::OK);
    Ok(Json(RegisterDeviceResponse {
        ok: true,
        account_id,
        device_id,
        device_token: token,
        policy: policy(&state.config, retention_hours),
    }))
}

async fn heartbeat_device(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<DeviceHeartbeatResponse>, ServiceError> {
    // authenticate_device refreshes last_seen_at in the same database as token
    // revocation, so this is the durable fallback when Supabase Presence is
    // disconnected or the app is background-throttled.
    let (auth, _client) = authenticate_device(&state, &headers).await?;
    record_request("POST", "/api/mobile/v1/devices/heartbeat", StatusCode::OK);
    Ok(Json(DeviceHeartbeatResponse {
        ok: true,
        device_id: auth.device_id,
        server_time: Utc::now(),
    }))
}

/// Revokes every Rust API token issued for a Supabase-visible install.
///
/// Authorization is deliberately delegated to the caller's Supabase RLS view:
/// the device owner can see their own row, and an account-group owner can see
/// member rows after the group migration. The returned row's `user_id` is then
/// bound to the namespaced backend account before any token is touched, so an
/// arbitrary install ID can never revoke a device outside that visible group.
async fn revoke_device(
    State(state): State<AppState>,
    Path(raw_install_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<RevokeDeviceResponse>, ServiceError> {
    let install_id = validate_nonempty(&raw_install_id, "installId", 160)?;
    if install_id.chars().any(char::is_control) {
        return Err(ServiceError::BadRequest(
            "installId contains invalid characters".to_string(),
        ));
    }

    let context = supabase_data_context(&state, &headers).await?;
    let response = state
        .http
        .get(format!("{}/rest/v1/devices", context.base_url))
        .header("apikey", &context.publishable_key)
        .bearer_auth(&context.token)
        .query(&[
            ("select", "user_id"),
            ("device_id", &format!("eq.{install_id}")),
            ("limit", "1"),
        ])
        .send()
        .await
        .map_err(|_| {
            ServiceError::Unavailable(
                "Supabase device authorization could not be completed".to_string(),
            )
        })?;
    let status = response.status();
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(ServiceError::Unauthorized);
    }
    if !status.is_success() {
        return Err(ServiceError::Unavailable(format!(
            "Supabase device authorization failed with {status}"
        )));
    }
    let visible = response
        .json::<Vec<SupabaseVisibleDevice>>()
        .await
        .map_err(|_| {
            ServiceError::Internal(
                "Supabase returned an invalid device authorization payload".to_string(),
            )
        })?
        .into_iter()
        .next()
        .ok_or_else(|| ServiceError::NotFound("device not found".to_string()))?;

    let external_subject = format!("supabase:{}", visible.user_id);
    let invalid_token_hash = hash_secret(&new_device_token(), &state.config.token_pepper);
    let client = db_conn(&state).await?;
    let rows = client
        .query(
            "update sound_recorder_devices device
             set status = 'revoked',
                 token_hash = $3,
                 token_last4 = 'none',
                 updated_at = now()
             from sound_recorder_accounts account
             where device.account_id = account.id
               and account.external_subject = $1
               and device.install_id = $2
               and device.status = 'active'
             returning device.id::text as device_id,
                       device.account_id::text as account_id",
            &[&external_subject, &install_id, &invalid_token_hash],
        )
        .await
        .map_err(db_error)?;
    for row in &rows {
        let device_id: String = row.get("device_id");
        let account_id: String = row.get("account_id");
        audit_event(
            &client,
            Some(&account_id),
            Some(&device_id),
            "sound_recorder.device.revoked",
            json!({"installId": install_id, "source": "supabase_device_registry"}),
        )
        .await;
    }

    record_request(
        "POST",
        "/api/mobile/v1/devices/:install_id/revoke",
        StatusCode::OK,
    );
    Ok(Json(RevokeDeviceResponse {
        ok: true,
        install_id,
        backend_tokens_revoked: rows.len(),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevicePresenceAuthenticate {
    #[serde(rename = "type")]
    message_type: String,
    device_token: String,
}

async fn device_presence_upgrade(
    State(state): State<AppState>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| device_presence_socket(state, socket))
}

async fn device_presence_socket(state: AppState, mut socket: WebSocket) {
    let auth_frame = match tokio::time::timeout(Duration::from_secs(10), socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) if text.len() <= 2048 => text,
        _ => return,
    };
    let request = match serde_json::from_str::<DevicePresenceAuthenticate>(&auth_frame) {
        Ok(request)
            if request.message_type == "authenticate"
                && request.device_token.len() >= 20
                && request.device_token.len() <= 1024 =>
        {
            request
        }
        _ => return,
    };
    let mut headers = HeaderMap::new();
    let authorization = match HeaderValue::from_str(&format!("Bearer {}", request.device_token)) {
        Ok(value) => value,
        Err(_) => return,
    };
    headers.insert(header::AUTHORIZATION, authorization);
    let (auth, client) = match authenticate_device(&state, &headers).await {
        Ok(result) => result,
        Err(_) => return,
    };

    let mut presence_updates = state
        .device_presence
        .join(&auth.account_id, &auth.install_id)
        .await;
    let (mut sender, mut receiver) = socket.split();
    if sender
        .send(Message::Text(
            json!({"type": "ready", "deviceId": auth.install_id}).to_string(),
        ))
        .await
        .is_err()
    {
        state
            .device_presence
            .leave(&auth.account_id, &auth.install_id)
            .await;
        return;
    }

    // Re-check the durable row frequently enough that an owner revocation also
    // disconnects an already-open fallback socket promptly. The client sends a
    // 25-second heartbeat too, so normal traffic usually notices first.
    let mut durable_heartbeat = tokio::time::interval(Duration::from_secs(30));
    durable_heartbeat.tick().await;
    loop {
        tokio::select! {
            message = receiver.next() => {
                match message {
                    Some(Ok(Message::Text(text))) if text.len() <= 1024 => {
                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                            if value.get("type").and_then(Value::as_str) == Some("heartbeat") {
                                let result = client.execute(
                                    "update sound_recorder_devices
                                     set last_seen_at = now(), updated_at = now()
                                     where id = $1::uuid and status = 'active'",
                                    &[&auth.device_id],
                                ).await;
                                if !matches!(result, Ok(1)) {
                                    break;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if sender.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            update = presence_updates.recv() => {
                match update {
                    Ok(online_device_ids) => {
                        let frame = json!({
                            "type": "presence",
                            "onlineDeviceIds": online_device_ids,
                            "serverTime": Utc::now(),
                        }).to_string();
                        if sender.send(Message::Text(frame)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = durable_heartbeat.tick() => {
                let result = client.execute(
                    "update sound_recorder_devices
                     set last_seen_at = now(), updated_at = now()
                     where id = $1::uuid and status = 'active'",
                    &[&auth.device_id],
                ).await;
                if !matches!(result, Ok(1)) {
                    break;
                }
            }
        }
    }
    state
        .device_presence
        .leave(&auth.account_id, &auth.install_id)
        .await;
}

async fn delete_account_storage_objects(
    state: &AppState,
    client: &DbClient,
    account_id: &str,
) -> Result<u64, ServiceError> {
    require_storage_history_compatible(state).await?;
    let rows = client
        .query(
            "select storage_bucket, storage_key, meta_data->>($2::text) as mirror_bucket
             from sound_recorder_segments
             where account_id = $1::uuid
               and storage_bucket <> ''
               and storage_key <> ''
             order by storage_bucket, storage_key",
            &[&account_id, &MIRROR_BUCKET_META_KEY],
        )
        .await
        .map_err(db_error)?;
    if rows.is_empty() {
        return Ok(0);
    }
    let s3 = state
        .s3
        .as_ref()
        .ok_or_else(|| ServiceError::Unavailable("S3 client is not configured".to_string()))?;
    let mut by_bucket: HashMap<String, Vec<String>> = HashMap::new();
    let mut mirror_by_bucket: HashMap<String, Vec<String>> = HashMap::new();
    let mut has_mirror_copies = false;
    for row in rows {
        let key: String = row.get("storage_key");
        let recorded_mirror: Option<String> = row.get("mirror_bucket");
        if let Some(mirror_bucket) = recorded_mirror.filter(|value| !value.is_empty()) {
            has_mirror_copies = true;
            mirror_by_bucket
                .entry(mirror_bucket)
                .or_default()
                .push(key.clone());
        } else if !state.config.mirror.bucket.is_empty() {
            // No bookkeeping, but a mirror is configured: erase the same key
            // defensively (idempotent) in case a copy's meta_data update was
            // lost between the mirror PUT and the bookkeeping write.
            mirror_by_bucket
                .entry(state.config.mirror.bucket.clone())
                .or_default()
                .push(key.clone());
        }
        by_bucket
            .entry(row.get("storage_bucket"))
            .or_default()
            .push(key);
    }
    // Account deletion is an erasure guarantee: refuse to report success while
    // a recorded backup copy exists that we have no client to delete with.
    if has_mirror_copies && state.mirror.is_none() {
        warn!(
            account_id,
            "account has mirrored segments but no mirror storage client is configured"
        );
        return Err(ServiceError::Unavailable(
            "mirrored account objects cannot be deleted; mirror storage is not configured"
                .to_string(),
        ));
    }
    let mut deleted = 0u64;
    for (bucket, keys) in by_bucket {
        deleted =
            deleted.saturating_add(delete_objects_in_bucket(s3, &bucket, &keys, account_id).await?);
    }
    if let Some(mirror) = state.mirror.as_ref() {
        for (bucket, keys) in mirror_by_bucket {
            delete_objects_in_bucket(mirror, &bucket, &keys, account_id).await?;
        }
    }
    Ok(deleted)
}

/// Batch-deletes `keys` from `bucket`, failing if any per-object delete fails.
/// DeleteObjects on missing keys succeeds, so callers may pass defensive
/// candidates that were never actually written.
async fn delete_objects_in_bucket(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    keys: &[String],
    account_id: &str,
) -> Result<u64, ServiceError> {
    let mut deleted = 0u64;
    for chunk in keys.chunks(1000) {
        let objects = chunk
            .iter()
            .map(|key| {
                ObjectIdentifier::builder().key(key).build().map_err(|_| {
                    ServiceError::Internal("failed to build object deletion request".to_string())
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let delete = Delete::builder()
            .set_objects(Some(objects))
            .quiet(true)
            .build()
            .map_err(|_| {
                ServiceError::Internal("failed to build object deletion request".to_string())
            })?;
        let output = tokio::time::timeout(
            STORAGE_OBJECT_TIMEOUT,
            client.delete_objects().bucket(bucket).delete(delete).send(),
        )
        .await
        .map_err(|_| {
            warn!(account_id, "account object deletion timed out");
            ServiceError::Unavailable("account object deletion timed out".to_string())
        })?
        .map_err(|err| {
            warn!(error = %err, account_id, "account object deletion failed");
            ServiceError::Unavailable("account object deletion failed".to_string())
        })?;
        if !output.errors().is_empty() {
            warn!(
                account_id,
                failures = output.errors().len(),
                "account object deletion returned per-object failures"
            );
            return Err(ServiceError::Unavailable(
                "one or more account objects could not be deleted".to_string(),
            ));
        }
        deleted = deleted.saturating_add(chunk.len() as u64);
    }
    Ok(deleted)
}

async fn delete_supabase_auth_user(state: &AppState, user_id: &str) -> Result<(), ServiceError> {
    let user_id = validate_nonempty(user_id, "supabaseUserId", 160)?;
    if user_id.contains('/') {
        return Err(ServiceError::BadRequest(
            "supabaseUserId must not contain path separators".to_string(),
        ));
    }
    let url = state.config.supabase.url.as_deref().ok_or_else(|| {
        ServiceError::Unavailable("Supabase URL is required for account deletion".to_string())
    })?;
    let service_role_key = state
        .config
        .supabase
        .service_role_key
        .as_deref()
        .ok_or_else(|| {
            ServiceError::Unavailable(
                "SOUND_RECORDER_SUPABASE_SERVICE_ROLE_KEY is required for account deletion"
                    .to_string(),
            )
        })?;
    let uri = format!(
        "{}/auth/v1/admin/users/{user_id}",
        url.trim_end_matches('/')
    );
    let response = state
        .http
        .delete(uri)
        .header("apikey", service_role_key)
        .bearer_auth(service_role_key)
        .send()
        .await
        .map_err(|err| {
            warn!(error = %err, "Supabase Auth delete request failed");
            ServiceError::Unavailable("Supabase Auth deletion failed".to_string())
        })?;
    let status = response.status();
    if status.is_success() || status == StatusCode::NOT_FOUND {
        return Ok(());
    }
    warn!(
        status = status.as_u16(),
        "Supabase Auth deletion returned non-success status"
    );
    Err(ServiceError::Unavailable(format!(
        "Supabase Auth deletion failed with status {}",
        status.as_u16()
    )))
}

async fn delete_account(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<DeleteAccountResponse>, ServiceError> {
    let (account_id, identity, client) =
        authenticate_supabase_account_for_deletion(&state, &headers).await?;
    if !state.config.supabase.account_features_configured() {
        return Err(ServiceError::Unavailable(
            "Supabase account administration is not fully configured".to_string(),
        ));
    }
    // Delete private audio first and fail before mutating account state if the
    // storage provider cannot confirm the purge. DeleteObjects is idempotent,
    // so a retry safely covers a partially completed multi-bucket purge.
    let deleted_objects = delete_account_storage_objects(&state, &client, &account_id).await?;
    let tx = client.transaction().await.map_err(db_error)?;
    let deletion_manifest = json!({ "deletedAt": Utc::now() });
    let deleted_segments = tx
        .execute(
            "update sound_recorder_segments
             set status = 'deleted',
                 expires_at = now(),
                 updated_at = now()
             where account_id = $1::uuid and status <> 'deleted'",
            &[&account_id],
        )
        .await
        .map_err(db_error)?;
    tx.execute(
        "update sound_recorder_upload_sessions
         set status = 'closed',
             closed_at = coalesce(closed_at, now()),
             updated_at = now()
         where account_id = $1::uuid and status <> 'closed'",
        &[&account_id],
    )
    .await
    .map_err(db_error)?;
    let revoked_devices = tx
        .execute(
            "update sound_recorder_devices
             set status = 'deleted',
                 transfer_paused = true,
                 transfer_pause_reason = 'manual',
                 updated_at = now()
             where account_id = $1::uuid and status <> 'deleted'",
            &[&account_id],
        )
        .await
        .map_err(db_error)?;
    let revoked_cloud_connections = tx
        .execute(
            "update sound_recorder_cloud_connections
             set status = 'revoked',
                 token_ciphertext = null,
                 token_nonce = null,
                 token_aad = null,
                 token_version = null,
                 token_expires_at = null,
                 updated_at = now()
             where account_id = $1::uuid and status <> 'revoked'",
            &[&account_id],
        )
        .await
        .map_err(db_error)?;
    tx.execute(
        "update sound_recorder_cloud_copy_jobs
         set status = 'skipped', updated_at = now()
         where account_id = $1::uuid
           and status in ('pending', 'waiting_client', 'running')",
        &[&account_id],
    )
    .await
    .map_err(db_error)?;
    tx.execute(
        "update sound_recorder_oauth_states
         set status = 'consumed', consumed_at = coalesce(consumed_at, now()), updated_at = now()
         where account_id = $1::uuid and status = 'pending'",
        &[&account_id],
    )
    .await
    .map_err(db_error)?;
    tx.execute(
        "update sound_recorder_evidence_exports
         set manifest = $2,
             download_url_expires_at = now(),
             expires_at = now()
         where account_id = $1::uuid",
        &[&account_id, &deletion_manifest],
    )
    .await
    .map_err(db_error)?;
    let deleted_account = tx
        .execute(
            "update sound_recorder_accounts
             set status = 'deleted',
                 display_name = null,
                 legal_region = null,
                 updated_at = now()
             where id = $1::uuid",
            &[&account_id],
        )
        .await
        .map_err(db_error)?;
    if deleted_account == 0 {
        return Err(ServiceError::NotFound("account not found".to_string()));
    }
    tx.commit().await.map_err(db_error)?;
    // Keep the verified external subject only until the upstream delete
    // succeeds. If Supabase is temporarily unavailable, the still-valid JWT can
    // retry this one deletion endpoint even though all device access is revoked.
    delete_supabase_auth_user(&state, &identity.subject).await?;
    client
        .execute(
            "update sound_recorder_accounts
             set external_subject = concat('deleted:', id::text), updated_at = now()
             where id = $1::uuid",
            &[&account_id],
        )
        .await
        .map_err(db_error)?;
    audit_event(
        &client,
        Some(&account_id),
        None,
        "sound_recorder.account.deleted",
        json!({
            "deletedSegments": deleted_segments,
            "deletedObjects": deleted_objects,
            "revokedDevices": revoked_devices,
            "revokedCloudConnections": revoked_cloud_connections,
            "supabaseAuthDeleted": true,
        }),
    )
    .await;
    record_request("DELETE", "/api/mobile/v1/account", StatusCode::OK);
    Ok(Json(DeleteAccountResponse {
        ok: true,
        account_id,
        deleted_segments,
        deleted_objects,
        revoked_devices,
        revoked_cloud_connections,
        supabase_auth_deleted: true,
    }))
}

/// Records the device's current transfer gate: whether it is pausing cloud
/// streaming (low battery / network policy) and its network preference. Paused
/// devices have their server-managed (Google Drive / OneDrive) copies held by
/// [drain_cloud_copy_jobs] until they report unpaused, keeping server delivery
/// consistent with the device's intent. Local capture is never affected.
async fn update_transfer_state(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<UpdateTransferStateRequest>,
) -> Result<Json<UpdateTransferStateResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let network_policy = validate_network_policy(req.network_policy)?;
    // A paused device must give a reason so the drain and audit trail are
    // meaningful; default an unlabeled pause to an explicit manual one. An
    // unpaused device clears any prior reason.
    let reason = if req.paused {
        Some(validate_pause_reason(req.reason)?.unwrap_or_else(|| "manual".to_string()))
    } else {
        None
    };
    let battery_level: Option<i16> = req
        .battery_level
        .filter(|value| (0..=100).contains(value))
        .map(|value| value as i16);
    let charging = req.charging;
    let row = client
        .query_one(
            "update sound_recorder_devices
             set transfer_paused = $2,
                 transfer_pause_reason = $3,
                 network_policy = $4,
                 battery_level = $5,
                 charging = $6,
                 transfer_state_updated_at = now(),
                 updated_at = now()
             where id = $1::uuid
             returning transfer_paused, network_policy",
            &[
                &auth.device_id,
                &req.paused,
                &reason,
                &network_policy,
                &battery_level,
                &charging,
            ],
        )
        .await
        .map_err(db_error)?;
    audit_event(
        &client,
        Some(&auth.account_id),
        Some(&auth.device_id),
        "sound_recorder.device.transfer_state",
        json!({
            "paused": req.paused,
            "reason": reason,
            "networkPolicy": network_policy,
            "batteryLevel": battery_level,
            "charging": charging
        }),
    )
    .await;
    record_request(
        "POST",
        "/api/mobile/v1/devices/transfer-state",
        StatusCode::OK,
    );
    Ok(Json(UpdateTransferStateResponse {
        ok: true,
        transfer_paused: row.get("transfer_paused"),
        network_policy: row.get("network_policy"),
    }))
}

async fn create_upload_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateUploadSessionRequest>,
) -> Result<Json<CreateUploadSessionResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    require_storage_history_compatible(&state).await?;
    let bucket = state.config.s3.bucket.trim().to_string();
    if bucket.is_empty() || state.s3.is_none() {
        return Err(ServiceError::Unavailable(
            "S3 storage is not configured".to_string(),
        ));
    }
    let content_type = validate_content_type(req.content_type, "audio/mp4")?;
    let codec = clean_string(req.codec, 80);
    let sample_rate = req
        .sample_rate
        .filter(|value| (8000..=192000).contains(value));
    let channel_count = req.channel_count.unwrap_or(1).clamp(1, 8);
    let segment_duration_seconds = req
        .segment_duration_seconds
        .unwrap_or(state.config.default_segment_seconds)
        .clamp(1, state.config.max_segment_seconds);
    let max_segment_bytes = req
        .max_segment_bytes
        .unwrap_or(state.config.max_segment_bytes)
        .clamp(1, state.config.max_segment_bytes);
    let client_timezone = clean_string(req.client_timezone, 80);
    let legal_region = validate_legal_region(req.legal_region)?;
    let use_case = validate_use_case(req.use_case)?;
    let mut meta_data = validate_meta(req.meta_data)?;
    if let Some(audio_profile) = req.audio_profile {
        let audio_profile = validate_meta(Some(audio_profile))?;
        if let Some(object) = meta_data.as_object_mut() {
            object.insert("audioProfile".to_string(), audio_profile);
            object.insert("useCase".to_string(), Value::String(use_case.clone()));
        }
    } else if let Some(object) = meta_data.as_object_mut() {
        object.insert("useCase".to_string(), Value::String(use_case.clone()));
    }
    let meta_data = attach_storage_metadata(meta_data, &state.config.s3.backend_fingerprint)?;
    let session_id = Uuid::new_v4().to_string();
    let storage_prefix = format!(
        "{}/account={}/device={}/session={}",
        state.config.s3.key_prefix.trim_matches('/'),
        auth.account_id,
        auth.device_id,
        session_id
    );
    let started_at = Utc::now();
    let expires_at = started_at
        .checked_add_signed(ChronoDuration::hours(state.config.session_ttl_hours))
        .unwrap_or(started_at);
    let row = client
        .query_one(
            "insert into sound_recorder_upload_sessions
              (id, account_id, device_id, storage_bucket, storage_prefix, content_type, codec,
               sample_rate, channel_count, segment_duration_seconds, max_segment_bytes,
               started_at, last_heartbeat_at, expires_at, client_timezone, legal_region,
               use_case, meta_data)
             values
              ($1::uuid, $2::uuid, $3::uuid, $4, $5, $6, $7,
               $8, $9, $10, $11, $12, $12, $13, $14, $15, $16, $17)
             returning id::text, account_id::text, device_id::text, status, storage_prefix,
                       content_type, codec, segment_duration_seconds, max_segment_bytes,
                       started_at, expires_at",
            &[
                &session_id,
                &auth.account_id,
                &auth.device_id,
                &bucket,
                &storage_prefix,
                &content_type,
                &codec,
                &sample_rate,
                &channel_count,
                &segment_duration_seconds,
                &max_segment_bytes,
                &started_at,
                &expires_at,
                &client_timezone,
                &legal_region,
                &use_case,
                &meta_data,
            ],
        )
        .await
        .map_err(db_error)?;
    audit_event(
        &client,
        Some(&auth.account_id),
        Some(&auth.device_id),
        "sound_recorder.upload_session.created",
        json!({
            "sessionId": session_id,
            "segmentDurationSeconds": segment_duration_seconds,
            "contentType": content_type,
            "useCase": use_case
        }),
    )
    .await;
    record_request("POST", "/api/mobile/v1/upload-sessions", StatusCode::OK);
    Ok(Json(CreateUploadSessionResponse {
        ok: true,
        session: upload_session_from_row(&row),
        policy: policy(&state.config, auth.retention_hours),
    }))
}

async fn presign_segment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<PresignSegmentRequest>,
) -> Result<Json<PresignSegmentResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let session_id = validate_uuid(&session_id, "sessionId")?;
    let session = load_session_policy(&client, &auth, &session_id).await?;
    if !storage_record_is_compatible(&state.config.s3, session.storage_fingerprint.as_deref()) {
        return Err(ServiceError::Unavailable(
            "upload session belongs to a different or unacknowledged object-storage backend"
                .to_string(),
        ));
    }
    if session.status != "active" {
        return Err(ServiceError::Conflict(
            "upload session is not active".to_string(),
        ));
    }
    if req.sequence_number < 0 {
        return Err(ServiceError::BadRequest(
            "sequenceNumber must be non-negative".to_string(),
        ));
    }
    if req.duration_millis <= 0
        || req.duration_millis > session.segment_duration_seconds.saturating_mul(1000)
    {
        return Err(ServiceError::BadRequest(format!(
            "durationMillis must be between 1 and {}",
            session.segment_duration_seconds.saturating_mul(1000)
        )));
    }
    let content_type = validate_content_type(req.content_type, &session.content_type)?;
    let codec = clean_string(req.codec, 80).or(session.codec.clone());
    let byte_count = req.byte_count;
    if let Some(byte_count) = byte_count {
        if byte_count <= 0 || byte_count > session.max_segment_bytes {
            return Err(ServiceError::BadRequest(format!(
                "byteCount must be between 1 and {}",
                session.max_segment_bytes
            )));
        }
    }
    let sha256_hex = validate_sha256(req.sha256_hex)?;
    let meta_data = attach_storage_metadata(
        validate_meta(req.meta_data)?,
        &state.config.s3.backend_fingerprint,
    )?;
    let now = Utc::now();
    let max_future_capture = now
        .checked_add_signed(ChronoDuration::seconds(MAX_CAPTURE_CLOCK_SKEW_SECONDS))
        .unwrap_or(now);
    if req.captured_started_at > max_future_capture {
        return Err(ServiceError::BadRequest(
            "capturedStartedAt is too far in the future".to_string(),
        ));
    }
    let retention_cutoff = now
        .checked_sub_signed(ChronoDuration::hours(auth.retention_hours as i64))
        .unwrap_or(now);
    if req.captured_started_at < retention_cutoff {
        return Err(ServiceError::BadRequest(
            "capturedStartedAt is outside the rolling retention window".to_string(),
        ));
    }
    let captured_ended_at = req
        .captured_started_at
        .checked_add_signed(ChronoDuration::milliseconds(req.duration_millis as i64))
        .unwrap_or(req.captured_started_at);
    let expires_at = req
        .captured_started_at
        .checked_add_signed(ChronoDuration::hours(auth.retention_hours as i64))
        .unwrap_or_else(Utc::now);
    let required_upload_window = state
        .config
        .upload_url_ttl
        .checked_add(Duration::from_secs(PRESIGNED_UPLOAD_SETTLE_GRACE_SECONDS))
        .ok_or_else(|| ServiceError::Internal("upload window overflow".to_string()))?;
    let minimum_retention_expiry = now
        .checked_add_signed(chrono_duration_from_std(required_upload_window)?)
        .unwrap_or(now);
    if expires_at <= minimum_retention_expiry {
        return Err(ServiceError::BadRequest(
            "capturedStartedAt is too close to the retention cutoff for a safe upload".to_string(),
        ));
    }
    let upload_expires_at = now
        .checked_add_signed(chrono_duration_from_std(state.config.upload_url_ttl)?)
        .unwrap_or(now)
        .min(expires_at);
    let key = storage_key(&session.storage_prefix, req.sequence_number, &content_type);

    let upload = presign_put(
        &state,
        &session.storage_bucket,
        &key,
        &content_type,
        byte_count,
        upload_expires_at,
    )
    .await?;

    let segment_id = Uuid::new_v4().to_string();
    let row = client
        .query_opt(
            "insert into sound_recorder_segments
              (id, account_id, device_id, session_id, sequence_number, storage_bucket,
               storage_key, content_type, codec, captured_started_at, captured_ended_at,
               duration_millis, byte_count, sha256_hex, upload_url_expires_at, expires_at,
               meta_data)
             values
              ($1::uuid, $2::uuid, $3::uuid, $4::uuid, $5, $6,
               $7, $8, $9, $10, $11,
               $12, $13, $14, $15, $16, $17)
             on conflict (session_id, sequence_number) do update set
               storage_key = excluded.storage_key,
               content_type = excluded.content_type,
               codec = excluded.codec,
               captured_started_at = excluded.captured_started_at,
               captured_ended_at = excluded.captured_ended_at,
               duration_millis = excluded.duration_millis,
               byte_count = excluded.byte_count,
               sha256_hex = excluded.sha256_hex,
               upload_url_expires_at = excluded.upload_url_expires_at,
               expires_at = excluded.expires_at,
               meta_data = excluded.meta_data,
               updated_at = now()
             where sound_recorder_segments.status in ('pending', 'failed')
             returning id::text, account_id::text, device_id::text, session_id::text,
                       sequence_number, status, storage_provider, storage_bucket, storage_key,
                       content_type, codec, captured_started_at, captured_ended_at,
                       duration_millis, byte_count, sha256_hex, upload_url_expires_at,
                       uploaded_at, expires_at",
            &[
                &segment_id,
                &auth.account_id,
                &auth.device_id,
                &session_id,
                &req.sequence_number,
                &session.storage_bucket,
                &key,
                &content_type,
                &codec,
                &req.captured_started_at,
                &captured_ended_at,
                &req.duration_millis,
                &byte_count,
                &sha256_hex,
                &upload_expires_at,
                &expires_at,
                &meta_data,
            ],
        )
        .await
        .map_err(db_error)?;
    let Some(row) = row else {
        return Err(ServiceError::Conflict(
            "segment is already uploaded".to_string(),
        ));
    };
    audit_event(
        &client,
        Some(&auth.account_id),
        Some(&auth.device_id),
        "sound_recorder.segment.presigned",
        json!({
            "sessionId": session_id,
            "sequenceNumber": req.sequence_number,
            "storageKey": key,
            "uploadUrlExpiresAt": upload_expires_at
        }),
    )
    .await;
    SEGMENT_PRESIGNS.with_label_values(&["upload", "ok"]).inc();
    record_request(
        "POST",
        "/api/mobile/v1/upload-sessions/:session_id/segments/presign",
        StatusCode::OK,
    );
    Ok(Json(PresignSegmentResponse {
        ok: true,
        segment: segment_from_row(&state.config, &row),
        upload,
    }))
}

fn normalize_etag(value: &str) -> String {
    let value = value.trim();
    let value = value
        .strip_prefix("W/")
        .or_else(|| value.strip_prefix("w/"))
        .unwrap_or(value)
        .trim();
    value.trim_matches('"').to_string()
}

fn validate_stored_object_metadata(
    metadata: StoredObjectMetadata<'_>,
    expected: StoredObjectExpectation<'_>,
) -> Result<VerifiedStoredObject, ServiceError> {
    let byte_count = metadata.content_length.ok_or_else(|| {
        ServiceError::Conflict("uploaded object did not report a content length".to_string())
    })?;
    if byte_count <= 0
        || byte_count > expected.max_segment_bytes as i64
        || byte_count > i32::MAX as i64
    {
        return Err(ServiceError::Conflict(format!(
            "uploaded object size must be between 1 and {} bytes",
            expected.max_segment_bytes
        )));
    }
    let byte_count = byte_count as i32;
    if expected
        .presigned_byte_count
        .is_some_and(|expected| expected != byte_count)
    {
        return Err(ServiceError::Conflict(
            "uploaded object size does not match the presigned byteCount".to_string(),
        ));
    }
    if expected
        .reported_byte_count
        .is_some_and(|reported| reported != byte_count)
    {
        return Err(ServiceError::Conflict(
            "uploaded object size does not match the completed byteCount".to_string(),
        ));
    }
    let content_type = metadata.content_type.ok_or_else(|| {
        ServiceError::Conflict("uploaded object did not report a content type".to_string())
    })?;
    if !content_type
        .trim()
        .eq_ignore_ascii_case(expected.content_type.trim())
    {
        return Err(ServiceError::Conflict(
            "uploaded object content type does not match the presigned contentType".to_string(),
        ));
    }
    let etag = metadata
        .etag
        .map(normalize_etag)
        .filter(|etag| !etag.is_empty())
        .ok_or_else(|| {
            ServiceError::Conflict("uploaded object did not report an ETag".to_string())
        })?;
    if etag.len() > 160 {
        return Err(ServiceError::Conflict(
            "uploaded object ETag is too long".to_string(),
        ));
    }
    if expected
        .reported_etag
        .map(normalize_etag)
        .is_some_and(|reported| reported != etag)
    {
        return Err(ServiceError::Conflict(
            "uploaded object ETag does not match the completed ETag".to_string(),
        ));
    }
    Ok(VerifiedStoredObject { byte_count, etag })
}

async fn verify_uploaded_object(
    state: &AppState,
    segment_id: &str,
    bucket: &str,
    key: &str,
    expected: StoredObjectExpectation<'_>,
) -> Result<VerifiedStoredObject, ServiceError> {
    require_storage_history_compatible(state).await?;
    let s3 = state
        .s3
        .as_ref()
        .ok_or_else(|| ServiceError::Unavailable("S3 client is not configured".to_string()))?;
    let head = tokio::time::timeout(
        STORAGE_OBJECT_TIMEOUT,
        s3.head_object().bucket(bucket).key(key).send(),
    )
    .await
    .map_err(|_| {
        warn!(segment_id, "uploaded object verification timed out");
        ServiceError::Unavailable("uploaded object verification timed out".to_string())
    })?
    .map_err(|err| {
        warn!(error = %err, segment_id, "uploaded object verification failed");
        ServiceError::Unavailable(
            "uploaded object could not be verified in object storage".to_string(),
        )
    })?;
    validate_stored_object_metadata(
        StoredObjectMetadata {
            content_length: head.content_length(),
            content_type: head.content_type(),
            etag: head.e_tag(),
        },
        expected,
    )
}

async fn complete_segment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, segment_id)): Path<(String, String)>,
    Json(req): Json<CompleteSegmentRequest>,
) -> Result<Json<CompleteSegmentResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let session_id = validate_uuid(&session_id, "sessionId")?;
    let segment_id = validate_uuid(&segment_id, "segmentId")?;
    let sha256_hex = validate_sha256(req.sha256_hex)?;
    let etag = clean_string(req.etag, 160);
    let policy_row = client
        .query_opt(
            "select s.captured_started_at, s.duration_millis, s.storage_bucket,
                    s.storage_key, s.content_type, s.byte_count, s.meta_data,
                    us.max_segment_bytes
             from sound_recorder_segments s
             join sound_recorder_upload_sessions us on us.id = s.session_id
             where s.id = $1::uuid
               and s.session_id = $2::uuid
               and s.account_id = $3::uuid
               and s.device_id = $4::uuid
               and s.status in ('pending', 'uploaded')
               and s.expires_at > now()",
            &[&segment_id, &session_id, &auth.account_id, &auth.device_id],
        )
        .await
        .map_err(db_error)?;
    let Some(policy_row) = policy_row else {
        return Err(ServiceError::NotFound("segment not found".to_string()));
    };
    let segment_meta: Value = policy_row.get("meta_data");
    let storage_fingerprint = segment_meta
        .get(STORAGE_FINGERPRINT_META_KEY)
        .and_then(Value::as_str);
    if !storage_record_is_compatible(&state.config.s3, storage_fingerprint) {
        return Err(ServiceError::Unavailable(
            "segment belongs to a different or unacknowledged object-storage backend".to_string(),
        ));
    }
    let max_segment_bytes: i32 = policy_row.get("max_segment_bytes");
    if let Some(byte_count) = req.byte_count {
        if byte_count <= 0 || byte_count > max_segment_bytes {
            return Err(ServiceError::BadRequest(format!(
                "byteCount must be between 1 and {max_segment_bytes}"
            )));
        }
    }
    if let Some(captured_ended_at) = req.captured_ended_at {
        let captured_started_at: DateTime<Utc> = policy_row.get("captured_started_at");
        let duration_millis: i32 = policy_row.get("duration_millis");
        let max_end = captured_started_at
            .checked_add_signed(ChronoDuration::milliseconds(
                duration_millis as i64 + (MAX_CAPTURE_CLOCK_SKEW_SECONDS * 1000),
            ))
            .unwrap_or(captured_started_at);
        if captured_ended_at < captured_started_at || captured_ended_at > max_end {
            return Err(ServiceError::BadRequest(
                "capturedEndedAt is outside the segment capture window".to_string(),
            ));
        }
    }
    let storage_bucket: String = policy_row.get("storage_bucket");
    let storage_key: String = policy_row.get("storage_key");
    let content_type: String = policy_row.get("content_type");
    let presigned_byte_count: Option<i32> = policy_row.get("byte_count");
    let verified_object = verify_uploaded_object(
        &state,
        &segment_id,
        &storage_bucket,
        &storage_key,
        StoredObjectExpectation {
            content_type: &content_type,
            presigned_byte_count,
            reported_byte_count: req.byte_count,
            reported_etag: etag.as_deref(),
            max_segment_bytes,
        },
    )
    .await?;
    let row = client
        .query_opt(
            "update sound_recorder_segments
             set status = 'uploaded',
                 etag = $5,
                 byte_count = $6,
                 sha256_hex = coalesce($7, sha256_hex),
                 captured_ended_at = coalesce($8, captured_ended_at),
                 uploaded_at = now(),
                 updated_at = now()
             where id = $1::uuid
               and session_id = $2::uuid
               and account_id = $3::uuid
               and device_id = $4::uuid
               and status in ('pending', 'uploaded')
               and (pinned_at is not null or expires_at > now())
             returning id::text, account_id::text, device_id::text, session_id::text,
                       sequence_number, status, storage_provider, storage_bucket, storage_key,
                       content_type, codec, captured_started_at, captured_ended_at,
                       duration_millis, byte_count, sha256_hex, upload_url_expires_at,
                       uploaded_at, expires_at",
            &[
                &segment_id,
                &session_id,
                &auth.account_id,
                &auth.device_id,
                &verified_object.etag,
                &verified_object.byte_count,
                &sha256_hex,
                &req.captured_ended_at,
            ],
        )
        .await
        .map_err(db_error)?;
    let Some(row) = row else {
        return Err(ServiceError::NotFound("segment not found".to_string()));
    };
    let _ = client
        .execute(
            "update sound_recorder_upload_sessions
             set last_heartbeat_at = now(), updated_at = now()
             where id = $1::uuid",
            &[&session_id],
        )
        .await;
    let cloud_jobs_queued =
        enqueue_cloud_copy_jobs_for_segment(&client, &state.config, &auth.account_id, &row).await?;
    audit_event(
        &client,
        Some(&auth.account_id),
        Some(&auth.device_id),
        "sound_recorder.segment.completed",
        json!({
            "sessionId": session_id,
            "segmentId": segment_id,
            "byteCount": req.byte_count,
            "cloudCopyJobsQueued": cloud_jobs_queued
        }),
    )
    .await;
    record_request(
        "POST",
        "/api/mobile/v1/upload-sessions/:session_id/segments/:segment_id/complete",
        StatusCode::OK,
    );
    Ok(Json(CompleteSegmentResponse {
        ok: true,
        segment: segment_from_row(&state.config, &row),
    }))
}

async fn heartbeat_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<HeartbeatResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let session_id = validate_uuid(&session_id, "sessionId")?;
    let updated = client
        .execute(
            "update sound_recorder_upload_sessions
             set last_heartbeat_at = now(), updated_at = now()
             where id = $1::uuid and account_id = $2::uuid and device_id = $3::uuid and status = 'active'",
            &[&session_id, &auth.account_id, &auth.device_id],
        )
        .await
        .map_err(db_error)?;
    if updated == 0 {
        return Err(ServiceError::NotFound(
            "active upload session not found".to_string(),
        ));
    }
    let row = client
        .query_one(
            "select coalesce(max(sequence_number) + 1, 0)::integer as next_sequence_number
             from sound_recorder_segments
             where session_id = $1::uuid",
            &[&session_id],
        )
        .await
        .map_err(db_error)?;
    let retention_cutoff = Utc::now()
        .checked_sub_signed(ChronoDuration::hours(auth.retention_hours as i64))
        .unwrap_or_else(Utc::now);
    record_request(
        "POST",
        "/api/mobile/v1/upload-sessions/:session_id/heartbeat",
        StatusCode::OK,
    );
    Ok(Json(HeartbeatResponse {
        ok: true,
        session_id,
        next_sequence_number: row.get("next_sequence_number"),
        retention_cutoff,
    }))
}

async fn close_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<CloseSessionResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let session_id = validate_uuid(&session_id, "sessionId")?;
    let updated = client
        .execute(
            "update sound_recorder_upload_sessions
             set status = 'closed', closed_at = now(), updated_at = now()
             where id = $1::uuid and account_id = $2::uuid and device_id = $3::uuid and status = 'active'",
            &[&session_id, &auth.account_id, &auth.device_id],
        )
        .await
        .map_err(db_error)?;
    if updated == 0 {
        return Err(ServiceError::NotFound(
            "active upload session not found".to_string(),
        ));
    }
    audit_event(
        &client,
        Some(&auth.account_id),
        Some(&auth.device_id),
        "sound_recorder.upload_session.closed",
        json!({ "sessionId": session_id }),
    )
    .await;
    record_request(
        "POST",
        "/api/mobile/v1/upload-sessions/:session_id/close",
        StatusCode::OK,
    );
    Ok(Json(CloseSessionResponse {
        ok: true,
        session_id,
        status: "closed".to_string(),
    }))
}

async fn timeline(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TimelineQuery>,
) -> Result<Json<TimelineResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let now = Utc::now();
    let retention_cutoff = now
        .checked_sub_signed(ChronoDuration::hours(auth.retention_hours as i64))
        .unwrap_or(now);
    let from = query.from.unwrap_or(retention_cutoff).max(retention_cutoff);
    let to = query.to.unwrap_or(now);
    if to <= from {
        return Err(ServiceError::BadRequest(
            "to must be later than from".to_string(),
        ));
    }
    let limit = query.limit.unwrap_or(100).clamp(1, MAX_TIMELINE_LIMIT);
    let rows = client
        .query(
            "select id::text, account_id::text, device_id::text, session_id::text,
                    sequence_number, status, storage_provider, storage_bucket, storage_key,
                    content_type, codec, captured_started_at, captured_ended_at,
                    duration_millis, byte_count, sha256_hex, upload_url_expires_at,
                    uploaded_at, expires_at
             from sound_recorder_segments
             where account_id = $1::uuid
               and status = 'uploaded'
               and (pinned_at is not null or expires_at > now())
               and captured_started_at >= $2
               and captured_started_at <= $3
             order by captured_started_at asc
             limit $4",
            &[&auth.account_id, &from, &to, &limit],
        )
        .await
        .map_err(db_error)?;
    record_request("GET", "/api/mobile/v1/timeline", StatusCode::OK);
    Ok(Json(TimelineResponse {
        ok: true,
        from,
        to,
        segments: rows
            .iter()
            .map(|row| segment_from_row(&state.config, row))
            .collect(),
    }))
}

async fn create_evidence_export(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<EvidenceExportRequest>,
) -> Result<Json<EvidenceExportResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let now = Utc::now();
    let retention_cutoff = now
        .checked_sub_signed(ChronoDuration::hours(auth.retention_hours as i64))
        .unwrap_or(now);
    if req.from < retention_cutoff {
        return Err(ServiceError::BadRequest(
            "export range starts outside the rolling retention window".to_string(),
        ));
    }
    if req.to <= req.from {
        return Err(ServiceError::BadRequest(
            "to must be later than from".to_string(),
        ));
    }
    let device_id = req
        .device_id
        .as_deref()
        .map(|device_id| validate_uuid(device_id, "deviceId"))
        .transpose()?;
    if let Some(device_id) = &device_id {
        let owns_device = client
            .query_opt(
                "select 1
                 from sound_recorder_devices
                 where id = $1::uuid and account_id = $2::uuid and status <> 'deleted'",
                &[device_id, &auth.account_id],
            )
            .await
            .map_err(db_error)?
            .is_some();
        if !owns_device {
            return Err(ServiceError::NotFound("device not found".to_string()));
        }
    }
    let limit = req
        .max_segments
        .unwrap_or(120)
        .clamp(1, MAX_EXPORT_SEGMENTS);
    let rows = if let Some(device_id) = &device_id {
        client
            .query(
                "select id::text, account_id::text, device_id::text, session_id::text,
                        sequence_number, status, storage_provider, storage_bucket, storage_key,
                        content_type, codec, captured_started_at, captured_ended_at,
                        duration_millis, byte_count, sha256_hex, upload_url_expires_at,
                        uploaded_at, expires_at
                 from sound_recorder_segments
                 where account_id = $1::uuid
                   and device_id = $2::uuid
                   and status = 'uploaded'
                   and (pinned_at is not null or expires_at > now())
                   and captured_started_at >= $3
                   and captured_started_at <= $4
                 order by captured_started_at asc
                 limit $5",
                &[&auth.account_id, device_id, &req.from, &req.to, &limit],
            )
            .await
            .map_err(db_error)?
    } else {
        client
            .query(
                "select id::text, account_id::text, device_id::text, session_id::text,
                        sequence_number, status, storage_provider, storage_bucket, storage_key,
                        content_type, codec, captured_started_at, captured_ended_at,
                        duration_millis, byte_count, sha256_hex, upload_url_expires_at,
                        uploaded_at, expires_at
                 from sound_recorder_segments
                 where account_id = $1::uuid
                   and status = 'uploaded'
                   and (pinned_at is not null or expires_at > now())
                   and captured_started_at >= $2
                   and captured_started_at <= $3
                 order by captured_started_at asc
                 limit $4",
                &[&auth.account_id, &req.from, &req.to, &limit],
            )
            .await
            .map_err(db_error)?
    };
    let download_expires_at = now
        .checked_add_signed(chrono_duration_from_std(state.config.download_url_ttl)?)
        .unwrap_or(now);
    let mut links = Vec::with_capacity(rows.len());
    for row in &rows {
        let segment = segment_from_row(&state.config, row);
        let download = presign_get(
            &state,
            &segment.storage_bucket,
            &segment.storage_key,
            download_expires_at,
        )
        .await?;
        SEGMENT_PRESIGNS
            .with_label_values(&["download", "ok"])
            .inc();
        links.push(EvidenceSegmentLink { segment, download });
    }
    let export_id = Uuid::new_v4().to_string();
    let manifest = json!({
        "from": req.from,
        "to": req.to,
        "segmentIds": links.iter().map(|link| link.segment.id.clone()).collect::<Vec<_>>()
    });
    client
        .execute(
            "insert into sound_recorder_evidence_exports
              (id, account_id, device_id, created_by_device_id, status, requested_from,
               requested_to, segment_count, manifest, download_url_expires_at, ready_at, expires_at)
             values
              ($1::uuid, $2::uuid, $3::uuid, $4::uuid, 'ready', $5,
               $6, $7, $8, $9, now(), $9)",
            &[
                &export_id,
                &auth.account_id,
                &device_id,
                &auth.device_id,
                &req.from,
                &req.to,
                &(links.len() as i32),
                &manifest,
                &download_expires_at,
            ],
        )
        .await
        .map_err(db_error)?;
    audit_event(
        &client,
        Some(&auth.account_id),
        Some(&auth.device_id),
        "sound_recorder.evidence_export.created",
        json!({
            "exportId": export_id,
            "from": req.from,
            "to": req.to,
            "segmentCount": links.len()
        }),
    )
    .await;
    record_request("POST", "/api/mobile/v1/evidence-exports", StatusCode::OK);
    Ok(Json(EvidenceExportResponse {
        ok: true,
        export_id,
        expires_at: download_expires_at,
        segment_count: links.len(),
        segments: links,
    }))
}

async fn create_permanent_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PermanentSaveRequest>,
) -> Result<Json<PermanentSaveResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    // The cloud provider hint (if any) is validated but not required: pinning is
    // a server-side retention exemption independent of the mirror destination.
    if let Some(provider) = req.provider.as_deref() {
        CloudProvider::parse(provider)?;
    }

    // Map each requested storage key back to the caller's segment id so the
    // response can echo client ids for the keys that were actually pinned.
    let mut requested_ids_by_key: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    for segment in &req.segments {
        if let Some(storage_key) = segment
            .storage_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if storage_key.len() > 2048 {
                return Err(ServiceError::BadRequest(
                    "storageKey is too long".to_string(),
                ));
            }
            requested_ids_by_key
                .entry(storage_key.to_string())
                .or_insert_with(|| clean_string(segment.id.clone(), 240));
        }
    }
    if requested_ids_by_key.len() > MAX_PERMANENT_SAVE_SEGMENTS {
        return Err(ServiceError::BadRequest(format!(
            "at most {MAX_PERMANENT_SAVE_SEGMENTS} segments can be saved per request"
        )));
    }

    let rows = if !requested_ids_by_key.is_empty() {
        let keys: Vec<String> = requested_ids_by_key.keys().cloned().collect();
        client
            .query(
                "update sound_recorder_segments
                 set pinned_at = coalesce(pinned_at, now()), updated_at = now()
                 where account_id = $1::uuid
                   and status = 'uploaded'
                   and expires_at > now()
                   and storage_key = any($2::text[])
                 returning storage_key",
                &[&auth.account_id, &keys],
            )
            .await
            .map_err(db_error)?
    } else {
        // No explicit segments: pin everything uploaded in the requested range.
        let (Some(from), Some(to)) = (req.range_started_at, req.range_ended_at) else {
            return Err(ServiceError::BadRequest(
                "provide segments or rangeStartedAt and rangeEndedAt".to_string(),
            ));
        };
        if to <= from {
            return Err(ServiceError::BadRequest(
                "rangeEndedAt must be later than rangeStartedAt".to_string(),
            ));
        }
        client
            .query(
                "update sound_recorder_segments
                 set pinned_at = coalesce(pinned_at, now()), updated_at = now()
                 where id in (
                   select id from sound_recorder_segments
                   where account_id = $1::uuid
                     and status = 'uploaded'
                     and expires_at > now()
                     and captured_started_at >= $2
                     and captured_started_at <= $3
                   order by captured_started_at asc
                   limit $4
                 )
                 returning storage_key",
                &[
                    &auth.account_id,
                    &from,
                    &to,
                    &(MAX_PERMANENT_SAVE_SEGMENTS as i64),
                ],
            )
            .await
            .map_err(db_error)?
    };

    let segments: Vec<PermanentSaveSegmentResult> = rows
        .iter()
        .map(|row| {
            let storage_key: String = row.get("storage_key");
            let id = requested_ids_by_key.get(&storage_key).cloned().flatten();
            PermanentSaveSegmentResult {
                id,
                permanent_storage_key: storage_key.clone(),
                storage_key,
            }
        })
        .collect();
    audit_event(
        &client,
        Some(&auth.account_id),
        Some(&auth.device_id),
        "sound_recorder.segments.pinned",
        json!({
            "savedCount": segments.len(),
            "provider": req.provider,
        }),
    )
    .await;
    record_request("POST", "/api/mobile/v1/permanent-saves", StatusCode::OK);
    Ok(Json(PermanentSaveResponse {
        ok: true,
        saved_count: segments.len(),
        segments,
    }))
}

async fn create_alert(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AlertRequest>,
) -> Result<Json<AlertResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let trigger = validate_alert_trigger(&req.trigger)?;
    let now = Utc::now();
    let max_future_capture = now
        .checked_add_signed(ChronoDuration::seconds(MAX_CAPTURE_CLOCK_SKEW_SECONDS))
        .unwrap_or(now);
    if req.occurred_at > max_future_capture {
        return Err(ServiceError::BadRequest(
            "occurredAt is too far in the future".to_string(),
        ));
    }
    let retention_cutoff = now
        .checked_sub_signed(ChronoDuration::hours(auth.retention_hours as i64))
        .unwrap_or(now);
    if req.occurred_at < retention_cutoff {
        return Err(ServiceError::BadRequest(
            "occurredAt is outside the rolling retention window".to_string(),
        ));
    }
    let offset_seconds = req.listen_offset_seconds.unwrap_or(20).clamp(0, 300);
    let listen_from = req
        .occurred_at
        .checked_sub_signed(ChronoDuration::seconds(offset_seconds))
        .unwrap_or(req.occurred_at);
    let listen_to = req
        .occurred_at
        .checked_add_signed(ChronoDuration::seconds(90))
        .unwrap_or(req.occurred_at);
    let meta_data = validate_meta(req.meta_data)?;
    let requested_email_to = clean_string(req.email_to, 320);
    let client_segment_id = clean_string(req.segment_id, 160);
    let email_to = state.config.alert_email_to.clone();
    let rows = client
        .query(
            "select id::text, account_id::text, device_id::text, session_id::text,
                    sequence_number, status, storage_provider, storage_bucket, storage_key,
                    content_type, codec, captured_started_at, captured_ended_at,
                    duration_millis, byte_count, sha256_hex, upload_url_expires_at,
                    uploaded_at, expires_at
             from sound_recorder_segments
             where account_id = $1::uuid
               and device_id = $2::uuid
               and status = 'uploaded'
               and captured_started_at <= $4
               and coalesce(captured_ended_at, captured_started_at + (duration_millis * interval '1 millisecond')) >= $3
               and (pinned_at is not null or expires_at > now())
             order by captured_started_at asc
             limit 8",
            &[&auth.account_id, &auth.device_id, &listen_from, &listen_to],
        )
        .await
        .map_err(db_error)?;
    if rows.is_empty() {
        return Err(ServiceError::Conflict(
            "no uploaded audio segment is available for that alert window".to_string(),
        ));
    }
    let download_expires_at = Utc::now()
        .checked_add_signed(chrono_duration_from_std(state.config.download_url_ttl)?)
        .unwrap_or_else(Utc::now);
    let mut links = Vec::with_capacity(rows.len());
    for row in &rows {
        let segment = segment_from_row(&state.config, row);
        let download = presign_get(
            &state,
            &segment.storage_bucket,
            &segment.storage_key,
            download_expires_at,
        )
        .await?;
        SEGMENT_PRESIGNS
            .with_label_values(&["download", "ok"])
            .inc();
        links.push(EvidenceSegmentLink { segment, download });
    }
    let alert_id = Uuid::new_v4().to_string();
    let listen_url = listen_url_for_alert(&state.config, &alert_id);
    let start_offset_seconds = links
        .first()
        .map(|link| {
            listen_from
                .signed_duration_since(link.segment.captured_started_at)
                .num_seconds()
                .max(0)
        })
        .unwrap_or(0);
    let download_urls = links
        .iter()
        .map(|link| link.download.url.clone())
        .collect::<Vec<_>>();
    let first_download_url = download_urls.first().cloned();
    let manifest = json!({
        "kind": "alert",
        "trigger": trigger,
        "occurredAt": req.occurred_at,
        "listenFrom": listen_from,
        "listenTo": listen_to,
        "listenUrl": listen_url.clone(),
        "downloadUrl": first_download_url.clone(),
        "downloadUrls": download_urls,
        "startOffsetSeconds": start_offset_seconds,
        "segmentIds": links.iter().map(|link| link.segment.id.clone()).collect::<Vec<_>>(),
        "clientSegmentId": client_segment_id,
        "sequenceNumber": req.sequence_number,
        "requestedEmailTo": requested_email_to,
        "emailTo": email_to,
        "metaData": meta_data,
    });
    client
        .execute(
            "insert into sound_recorder_evidence_exports
              (id, account_id, device_id, created_by_device_id, status, requested_from,
               requested_to, segment_count, manifest, download_url_expires_at, ready_at, expires_at)
             values
              ($1::uuid, $2::uuid, $3::uuid, $4::uuid, 'ready', $5,
               $6, $7, $8, $9, now(), $9)",
            &[
                &alert_id,
                &auth.account_id,
                &auth.device_id,
                &auth.device_id,
                &listen_from,
                &listen_to,
                &(links.len() as i32),
                &manifest,
                &download_expires_at,
            ],
        )
        .await
        .map_err(db_error)?;
    let emailed = send_alert_email(
        &state,
        &email_to,
        &trigger,
        req.occurred_at,
        listen_url.as_deref().or(first_download_url.as_deref()),
        links.len(),
    )
    .await;
    audit_event(
        &client,
        Some(&auth.account_id),
        Some(&auth.device_id),
        "sound_recorder.alert.created",
        json!({
            "alertId": alert_id,
            "trigger": trigger,
            "occurredAt": req.occurred_at,
            "listenFrom": listen_from,
            "segmentCount": links.len(),
            "emailed": emailed,
        }),
    )
    .await;
    record_request("POST", "/api/mobile/v1/alerts", StatusCode::OK);
    Ok(Json(AlertResponse {
        ok: true,
        alert_id,
        emailed,
        email_to,
        listen_url: listen_url.or(first_download_url),
        listen_from,
        listen_to,
        segment_count: links.len(),
    }))
}

async fn listen_alert(
    State(state): State<AppState>,
    Path(alert_id): Path<String>,
) -> Result<(HeaderMap, Html<String>), ServiceError> {
    let alert_id = validate_uuid(&alert_id, "alertId")?;
    // Reuse the rustls-backed connector so this public route uses the same TLS
    // posture as every other database path (RDS rejects plaintext connections).
    let client = db_conn(&state).await?;
    let row = client
        .query_opt(
            "select manifest, requested_from, requested_to, expires_at
             from sound_recorder_evidence_exports
             where id = $1::uuid and status = 'ready' and expires_at > now()",
            &[&alert_id],
        )
        .await
        .map_err(db_error)?;
    let Some(row) = row else {
        return Err(ServiceError::NotFound(
            "listen link expired or missing".to_string(),
        ));
    };
    let manifest: Value = row.get("manifest");
    record_request("GET", "/listen/:alert_id", StatusCode::OK);
    // This page needs its inline audio-player script and must load segment audio
    // from presigned HTTPS URLs, so it carries its own (looser) CSP. The global
    // middleware leaves an already-present CSP untouched.
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'unsafe-inline'; media-src https:; \
             connect-src https:; img-src 'self' data:; style-src 'unsafe-inline'; \
             base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    Ok((headers, Html(render_listen_alert(&alert_id, &manifest))))
}

async fn send_alert_email(
    state: &AppState,
    to: &str,
    trigger: &str,
    occurred_at: DateTime<Utc>,
    listen_url: Option<&str>,
    segment_count: usize,
) -> bool {
    if to.trim().is_empty() {
        info!(
            trigger,
            %occurred_at,
            segment_count,
            "alert email recipient (SOUND_RECORDER_ALERT_EMAIL_TO) is not configured; skipping send"
        );
        return false;
    }
    let Some(webhook_url) = state.config.alert_email_webhook_url.as_deref() else {
        info!(
            to,
            trigger,
            %occurred_at,
            segment_count,
            "alert email webhook is not configured"
        );
        return false;
    };
    let subject = format!("Audio dashcam alert: {trigger}");
    let link_text = listen_url.unwrap_or("No uploaded audio segment is available yet.");
    let body = format!(
        "Audio dashcam alert\n\nTrigger: {trigger}\nOccurred: {occurred_at}\nSegments: {segment_count}\nListen: {link_text}\n"
    );
    let payload = json!({
        "to": to,
        "subject": subject,
        "text": body,
        "html": format!(
            "<p><strong>Trigger:</strong> {}</p><p><strong>Occurred:</strong> {}</p><p><a href=\"{}\">Listen from 20 seconds before</a></p>",
            html_escape(trigger),
            occurred_at,
            html_escape(link_text)
        ),
    });
    match state.http.post(webhook_url).json(&payload).send().await {
        Ok(response) if response.status().is_success() => true,
        Ok(response) => {
            warn!(
                status = %response.status(),
                "alert email webhook returned non-success status"
            );
            false
        }
        Err(err) => {
            warn!(error = %err, "alert email webhook request failed");
            false
        }
    }
}

async fn list_cloud_connections(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ListCloudConnectionsResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let rows = client
        .query(
            "select id::text, provider, link_mode, status, display_name, provider_account_id,
                    root_folder_id, folder_path, token_expires_at, last_sync_at,
                    created_at, updated_at
             from sound_recorder_cloud_connections
             where account_id = $1::uuid and status <> 'revoked'
             order by provider asc, updated_at desc",
            &[&auth.account_id],
        )
        .await
        .map_err(db_error)?;
    record_request("GET", "/api/mobile/v1/cloud-connections", StatusCode::OK);
    Ok(Json(ListCloudConnectionsResponse {
        ok: true,
        connections: rows.iter().map(cloud_connection_from_row).collect(),
    }))
}

async fn start_cloud_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<StartCloudLinkRequest>,
) -> Result<Json<StartCloudLinkResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let provider = CloudProvider::parse(&req.provider)?;
    let redirect_uri = validate_redirect_uri(
        provider,
        req.redirect_uri,
        &state.config.oauth_redirect_allowlist,
    )?;
    let folder_path = validate_folder_path(req.folder_path)?;
    let root_folder_id = clean_optional_nonempty(req.root_folder_id, 512)?;
    let display_name = clean_string(req.display_name, 160);
    let meta_data = validate_meta(req.meta_data)?;
    let state_token = new_oauth_state();
    let pkce = provider.is_server_managed().then(new_oauth_pkce);
    let state_hash = oauth_state_hash(&state.config, &state_token);
    let expires_at = Utc::now()
        .checked_add_signed(chrono_duration_from_std(state.config.oauth_state_ttl)?)
        .unwrap_or_else(Utc::now);
    client
        .execute(
            "insert into sound_recorder_oauth_states
              (id, account_id, device_id, provider, state_hash, redirect_uri,
               folder_path, expires_at, meta_data)
             values
              ($1::uuid, $2::uuid, $3::uuid, $4, $5, $6, $7, $8, $9)",
            &[
                &Uuid::new_v4().to_string(),
                &auth.account_id,
                &auth.device_id,
                &provider.as_str(),
                &state_hash,
                &redirect_uri,
                &folder_path,
                &expires_at,
                &json!({
                    "rootFolderId": root_folder_id,
                    "displayName": display_name,
                    "codeVerifier": pkce.as_ref().map(|value| value.verifier.as_str()),
                    "clientMeta": meta_data
                }),
            ],
        )
        .await
        .map_err(db_error)?;
    let authorization_url = if let Some(oauth) = provider.oauth_config(&state.config) {
        Some(authorization_url(
            provider,
            oauth,
            &redirect_uri,
            &state_token,
            pkce.as_ref().map(|value| value.challenge.as_str()),
        )?)
    } else {
        None
    };
    audit_event(
        &client,
        Some(&auth.account_id),
        Some(&auth.device_id),
        "sound_recorder.cloud_link.started",
        json!({
            "provider": provider.as_str(),
            "linkMode": provider.link_mode(),
            "folderPath": folder_path
        }),
    )
    .await;
    record_request(
        "POST",
        "/api/mobile/v1/cloud-connections/oauth/start",
        StatusCode::OK,
    );
    Ok(Json(StartCloudLinkResponse {
        ok: true,
        provider: provider.as_str().to_string(),
        link_mode: provider.link_mode().to_string(),
        state: state_token,
        authorization_url,
        expires_at,
        required_scope: provider.required_scope(),
        client_managed: provider.is_client_managed(),
    }))
}

async fn complete_cloud_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CompleteCloudLinkRequest>,
) -> Result<Json<CompleteCloudLinkResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let provider = CloudProvider::parse(&req.provider)?;
    let supabase_token_set = supabase_provider_token_set(&req);
    let state_token = validate_nonempty(&req.state, "state", 160)?;
    let state_hash = oauth_state_hash(&state.config, &state_token);
    // Atomically claim the state before exchanging the provider code. A plain
    // SELECT followed by a later UPDATE lets two concurrent completion
    // requests both observe `pending`; this transition guarantees exactly one
    // request can cross the trust boundary. Failed exchanges intentionally burn
    // the state, requiring a fresh authorization rather than making a captured
    // code/state pair replayable.
    let state_row = client
        .query_opt(
            "update sound_recorder_oauth_states
             set status = 'processing', updated_at = now()
             where account_id = $1::uuid
               and device_id = $2::uuid
               and provider = $3
               and state_hash = $4
               and status = 'pending'
               and expires_at > now()
             returning id::text, redirect_uri, folder_path, meta_data",
            &[
                &auth.account_id,
                &auth.device_id,
                &provider.as_str(),
                &state_hash,
            ],
        )
        .await
        .map_err(db_error)?;
    let Some(state_row) = state_row else {
        return Err(ServiceError::Unauthorized);
    };
    let oauth_state_id: String = state_row.get("id");
    let redirect_uri: String = state_row.get("redirect_uri");
    if let Some(req_redirect_uri) = req.redirect_uri.as_deref() {
        if req_redirect_uri.trim() != redirect_uri {
            return Err(ServiceError::BadRequest(
                "redirectUri does not match the started OAuth flow".to_string(),
            ));
        }
    }
    let state_meta: Value = state_row.get("meta_data");
    let code_verifier = state_meta
        .get("codeVerifier")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let folder_path = validate_folder_path(
        req.folder_path
            .or_else(|| state_row.get::<Option<String>>("folder_path")),
    )?;
    let root_folder_id = clean_optional_nonempty(req.root_folder_id, 512)?.or_else(|| {
        state_meta
            .get("rootFolderId")
            .and_then(Value::as_str)
            .map(ToString::to_string)
    });
    let display_name = clean_string(req.display_name, 160).or_else(|| {
        state_meta
            .get("displayName")
            .and_then(Value::as_str)
            .map(ToString::to_string)
    });
    let request_meta = validate_meta(req.meta_data)?;
    let provider_account_id = validate_provider_account_id(req.provider_account_id)?
        .unwrap_or_else(|| format!("{}-default", provider.as_str()));

    let (sealed, token_expires_at, oauth_scope) = if provider.is_server_managed() {
        let sealer = state.cloud_sealer.as_ref().ok_or_else(|| {
            ServiceError::Unavailable(
                "SOUND_RECORDER_CLOUD_TOKEN_ENCRYPTION_KEY is required for server-managed cloud links".to_string(),
            )
        })?;
        let token_set = if let Some(token_set) = supabase_token_set {
            // Hybrid path: Supabase already performed the user-facing OAuth and
            // handed us the provider access/refresh token to seal.
            token_set
        } else {
            let authorization_code = validate_nonempty(
                req.authorization_code.as_deref().unwrap_or(""),
                "authorizationCode",
                4096,
            )?;
            exchange_authorization_code(
                &state,
                provider,
                &authorization_code,
                &redirect_uri,
                code_verifier,
            )
            .await?
        };
        let plaintext = serde_json::to_vec(&token_set)
            .map_err(|_| ServiceError::Internal("cloud token encode failed".to_string()))?;
        let sealed = sealer.seal(&auth.account_id, provider, &plaintext)?;
        (Some(sealed), token_set.expires_at, token_set.scope.clone())
    } else {
        if !req.client_managed_acknowledged.unwrap_or(false) {
            return Err(ServiceError::BadRequest(
                "clientManagedAcknowledged must be true for client-managed links".to_string(),
            ));
        }
        (None, None, None)
    };

    let connection = upsert_cloud_connection(
        &client,
        &auth,
        provider,
        display_name,
        Some(provider_account_id),
        root_folder_id,
        folder_path,
        oauth_scope,
        sealed,
        token_expires_at,
        request_meta,
    )
    .await?;
    client
        .execute(
            "update sound_recorder_oauth_states
             set status = 'consumed', consumed_at = now(), updated_at = now()
             where id = $1::uuid and status = 'processing'",
            &[&oauth_state_id],
        )
        .await
        .map_err(db_error)?;
    let backfilled =
        enqueue_retained_cloud_copy_jobs(&client, &state.config, &auth.account_id, &connection)
            .await?;
    audit_event(
        &client,
        Some(&auth.account_id),
        Some(&auth.device_id),
        "sound_recorder.cloud_link.completed",
        json!({
            "provider": provider.as_str(),
            "connectionId": connection.id,
            "linkMode": provider.link_mode(),
            "backfilledJobs": backfilled
        }),
    )
    .await;
    record_request(
        "POST",
        "/api/mobile/v1/cloud-connections/oauth/complete",
        StatusCode::OK,
    );
    Ok(Json(CompleteCloudLinkResponse {
        ok: true,
        connection: CloudConnectionResponse {
            id: connection.id,
            provider: connection.provider,
            link_mode: connection.link_mode,
            status: connection.status,
            display_name: connection.display_name,
            provider_account_id: connection.provider_account_id,
            root_folder_id: connection.root_folder_id,
            folder_path: connection.folder_path,
            token_expires_at: connection.token_expires_at,
            last_sync_at: connection.last_sync_at,
            created_at: connection.created_at,
            updated_at: connection.updated_at,
        },
        backfilled_jobs: backfilled,
    }))
}

async fn revoke_cloud_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(connection_id): Path<String>,
) -> Result<Json<RevokeCloudConnectionResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let connection_id = validate_uuid(&connection_id, "connectionId")?;
    let connection_row = client
        .query_opt(
            "select id::text, account_id::text, provider, link_mode, status, display_name,
                    provider_account_id, root_folder_id, folder_path, token_ciphertext,
                    token_nonce, token_aad, token_version, token_expires_at, last_sync_at,
                    created_at, updated_at
             from sound_recorder_cloud_connections
             where id = $1::uuid and account_id = $2::uuid and status <> 'revoked'",
            &[&connection_id, &auth.account_id],
        )
        .await
        .map_err(db_error)?;
    let Some(connection_row) = connection_row else {
        return Err(ServiceError::NotFound(
            "cloud connection not found".to_string(),
        ));
    };
    let connection = cloud_connection_record_from_row(&connection_row);
    let updated = client
        .execute(
            "update sound_recorder_cloud_connections
             set status = 'revoked',
                 token_ciphertext = null,
                 token_nonce = null,
                 token_aad = null,
                 token_version = null,
                 token_expires_at = null,
                 updated_at = now()
             where id = $1::uuid and account_id = $2::uuid and status <> 'revoked'",
            &[&connection_id, &auth.account_id],
        )
        .await
        .map_err(db_error)?;
    if updated == 0 {
        return Err(ServiceError::NotFound(
            "cloud connection not found".to_string(),
        ));
    }
    // Local authority is removed first. The in-memory row still carries the
    // sealed token for a bounded upstream revocation attempt, but a slow or
    // unavailable provider can no longer race new copy jobs or keep the
    // Sonus-side connection active.
    let provider_authorization_revoked =
        revoke_provider_authorization(&state, &connection, None).await;
    client
        .execute(
            "update sound_recorder_cloud_copy_jobs
             set status = 'skipped', updated_at = now()
             where connection_id = $1::uuid and status in ('pending', 'waiting_client', 'running')",
            &[&connection_id],
        )
        .await
        .map_err(db_error)?;
    audit_event(
        &client,
        Some(&auth.account_id),
        Some(&auth.device_id),
        "sound_recorder.cloud_link.revoked",
        json!({
            "connectionId": connection_id,
            "provider": connection.provider,
            "providerAuthorizationRevoked": provider_authorization_revoked
        }),
    )
    .await;
    record_request(
        "POST",
        "/api/mobile/v1/cloud-connections/:connection_id/revoke",
        StatusCode::OK,
    );
    Ok(Json(RevokeCloudConnectionResponse {
        ok: true,
        connection_id,
        status: "revoked".to_string(),
        provider_authorization_revoked,
    }))
}

async fn list_client_cloud_copy_jobs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListCloudCopyJobsQuery>,
) -> Result<Json<ListCloudCopyJobsResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let provider = query
        .provider
        .as_deref()
        .map(CloudProvider::parse)
        .transpose()?
        .unwrap_or(CloudProvider::AppleICloud);
    if provider != CloudProvider::AppleICloud {
        return Err(ServiceError::BadRequest(
            "client-managed copy jobs are currently only available for apple_icloud".to_string(),
        ));
    }
    let limit = query.limit.unwrap_or(25).clamp(1, 100);
    let rows = client
        .query(
            "select j.id::text as job_id, j.connection_id::text, j.segment_id::text,
                    j.provider as job_provider, j.status as job_status, j.destination_key,
                    j.provider_file_id, j.attempts, j.completed_at, j.last_error,
                    s.id::text, s.account_id::text, s.device_id::text, s.session_id::text,
                    s.sequence_number, s.status, s.storage_provider, s.storage_bucket,
                    s.storage_key, s.content_type, s.codec, s.captured_started_at,
                    s.captured_ended_at, s.duration_millis, s.byte_count, s.sha256_hex,
                    s.upload_url_expires_at, s.uploaded_at, s.expires_at
             from sound_recorder_cloud_copy_jobs j
             join sound_recorder_segments s on s.id = j.segment_id
             join sound_recorder_cloud_connections c on c.id = j.connection_id
             where j.account_id = $1::uuid
               and j.provider = $2
               and j.status = 'waiting_client'
               and c.status = 'active'
               and s.status = 'uploaded'
               and (s.pinned_at is not null or s.expires_at > now())
             order by j.created_at asc
             limit $3",
            &[&auth.account_id, &provider.as_str(), &limit],
        )
        .await
        .map_err(db_error)?;
    let download_expires_at = Utc::now()
        .checked_add_signed(chrono_duration_from_std(state.config.download_url_ttl)?)
        .unwrap_or_else(Utc::now);
    let mut jobs = Vec::with_capacity(rows.len());
    for row in rows {
        let segment = segment_from_row(&state.config, &row);
        let download = presign_get(
            &state,
            &segment.storage_bucket,
            &segment.storage_key,
            download_expires_at,
        )
        .await?;
        let job = CloudCopyJobResponse {
            id: row.get("job_id"),
            connection_id: row.get("connection_id"),
            segment_id: row.get("segment_id"),
            provider: row.get("job_provider"),
            status: row.get("job_status"),
            destination_key: row.get("destination_key"),
            provider_file_id: row.get("provider_file_id"),
            attempts: row.get("attempts"),
            completed_at: row.get("completed_at"),
            last_error: row.get("last_error"),
        };
        jobs.push(CloudCopyJobWithDownload {
            job,
            segment,
            download,
        });
    }
    record_request("GET", "/api/mobile/v1/cloud-copy-jobs", StatusCode::OK);
    Ok(Json(ListCloudCopyJobsResponse { ok: true, jobs }))
}

async fn complete_client_cloud_copy_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
    Json(req): Json<CompleteCloudCopyJobRequest>,
) -> Result<Json<CompleteCloudCopyJobResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let job_id = validate_uuid(&job_id, "jobId")?;
    let provider_file_id = clean_optional_nonempty(req.provider_file_id, 512)?;
    let destination_key = clean_optional_nonempty(req.destination_key, 2048)?;
    let meta_data = validate_meta(req.meta_data)?;
    let row = client
        .query_opt(
            "update sound_recorder_cloud_copy_jobs
             set status = 'completed',
                 provider_file_id = coalesce($3, provider_file_id),
                 destination_key = coalesce($4, destination_key),
                 completed_at = now(),
                 meta_data = meta_data || $5::jsonb,
                 updated_at = now()
             where id = $1::uuid
               and account_id = $2::uuid
               and status = 'waiting_client'
             returning id::text, account_id::text, connection_id::text, segment_id::text,
                       provider, status, destination_key, provider_file_id, attempts,
                       completed_at, last_error",
            &[
                &job_id,
                &auth.account_id,
                &provider_file_id,
                &destination_key,
                &meta_data,
            ],
        )
        .await
        .map_err(db_error)?;
    let Some(row) = row else {
        return Err(ServiceError::NotFound(
            "client-managed cloud copy job not found".to_string(),
        ));
    };
    let connection_id: String = row.get("connection_id");
    client
        .execute(
            "update sound_recorder_cloud_connections
             set last_sync_at = now(), updated_at = now()
             where id = $1::uuid and account_id = $2::uuid and status = 'active'",
            &[&connection_id, &auth.account_id],
        )
        .await
        .map_err(db_error)?;
    audit_event(
        &client,
        Some(&auth.account_id),
        Some(&auth.device_id),
        "sound_recorder.cloud_copy.client_completed",
        json!({ "jobId": job_id }),
    )
    .await;
    record_request(
        "POST",
        "/api/mobile/v1/cloud-copy-jobs/:job_id/complete",
        StatusCode::OK,
    );
    Ok(Json(CompleteCloudCopyJobResponse {
        ok: true,
        job: cloud_copy_job_from_row(&row),
    }))
}

async fn drain_cloud_copy_jobs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<DrainCloudCopyRequest>,
) -> Result<Json<DrainCloudCopyResponse>, ServiceError> {
    require_internal_auth(&state.config, &headers)?;
    let client = db_conn(&state).await?;
    let limit = req
        .max_jobs
        .unwrap_or(state.config.cloud_copy_batch_size)
        .clamp(1, MAX_CLOUD_COPY_BATCH_SIZE);
    let rows = client
        .query(
            "select j.id::text as job_id, j.account_id::text as job_account_id,
                    j.connection_id::text, j.segment_id::text, j.provider as job_provider,
                    j.status as job_status, j.destination_key, j.provider_file_id,
                    j.attempts, j.completed_at, j.last_error,
                    c.id::text as connection_id, c.account_id::text as connection_account_id,
                    c.provider as connection_provider, c.link_mode, c.status as connection_status,
                    c.display_name, c.provider_account_id, c.root_folder_id, c.folder_path,
                    c.token_ciphertext, c.token_nonce, c.token_aad, c.token_version,
                    c.token_expires_at, c.last_sync_at, c.created_at as connection_created_at,
                    c.updated_at as connection_updated_at,
                    s.id::text, s.account_id::text, s.device_id::text, s.session_id::text,
                    s.sequence_number, s.status, s.storage_provider, s.storage_bucket,
                    s.storage_key, s.content_type, s.codec, s.captured_started_at,
                    s.captured_ended_at, s.duration_millis, s.byte_count, s.sha256_hex,
                    s.upload_url_expires_at, s.uploaded_at, s.expires_at
             from sound_recorder_cloud_copy_jobs j
             join sound_recorder_cloud_connections c on c.id = j.connection_id
             join sound_recorder_segments s on s.id = j.segment_id
             where (
                 (j.status = 'pending' and (j.locked_until is null or j.locked_until < now()))
                 or (j.status = 'running' and j.locked_until < now())
               )
               and c.status = 'active'
               and c.link_mode = 'server_oauth'
               and s.status = 'uploaded'
               and (s.pinned_at is not null or s.expires_at > now())
               -- Hold server-managed copies for any segment whose source device
               -- is currently pausing cloud streaming, so server delivery stays
               -- consistent with the device's battery/network intent. The pause
               -- is lease-based: only a *fresh* pause is honored, so a device
               -- that reported paused and then vanished (app killed/uninstalled)
               -- cannot strand its already-uploaded segments forever — server
               -- copies don't use the phone battery, so resuming once the client
               -- is gone is safe. A live paused client re-affirms within the lease.
               and not exists (
                 select 1 from sound_recorder_devices d
                 where d.id = s.device_id
                   and d.transfer_paused = true
                   and d.transfer_state_updated_at is not null
                   and d.transfer_state_updated_at > now() - $2::interval
               )
             order by j.updated_at asc
             limit $1",
            &[&limit, &TRANSFER_PAUSE_LEASE],
        )
        .await
        .map_err(db_error)?;
    let mut attempted = 0usize;
    let mut completed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut results = Vec::with_capacity(rows.len());
    for row in rows {
        let item = cloud_copy_work_item_from_row(&state.config, &row);
        let claim = claim_cloud_copy_job(&client, &item.job.id).await?;
        let Some(claim) = claim else {
            skipped += 1;
            results.push(CloudCopyDrainResult {
                job_id: item.job.id,
                provider: item.job.provider,
                status: "skipped".to_string(),
                message: Some("job was already claimed".to_string()),
            });
            continue;
        };
        attempted += 1;
        match process_cloud_copy_job(&state, &client, &item).await {
            Ok(provider_file_id) => {
                let finalized =
                    mark_cloud_copy_job_success(&client, &item, &provider_file_id, claim.attempts)
                        .await?;
                if finalized {
                    completed += 1;
                } else {
                    skipped += 1;
                }
                results.push(CloudCopyDrainResult {
                    job_id: item.job.id,
                    provider: item.job.provider,
                    status: if finalized { "completed" } else { "skipped" }.to_string(),
                    message: (!finalized).then(|| {
                        "job lease was superseded before its result could be recorded".to_string()
                    }),
                });
            }
            Err(err) => {
                let message = service_error_message(&err);
                let finalized = mark_cloud_copy_job_error(
                    &client,
                    &item.job.id,
                    claim.attempts,
                    &message,
                    &state.config,
                )
                .await?;
                if finalized {
                    failed += 1;
                } else {
                    skipped += 1;
                }
                results.push(CloudCopyDrainResult {
                    job_id: item.job.id,
                    provider: item.job.provider,
                    status: if finalized { "failed" } else { "skipped" }.to_string(),
                    message: Some(if finalized {
                        message
                    } else {
                        "job lease was superseded before its failure could be recorded".to_string()
                    }),
                });
            }
        }
    }
    record_request("POST", "/internal/cloud-copy/drain", StatusCode::OK);
    Ok(Json(DrainCloudCopyResponse {
        ok: true,
        attempted,
        completed,
        failed,
        skipped,
        results,
    }))
}

fn supabase_user_id_from_external_subject(external_subject: &str) -> Option<String> {
    external_subject
        .strip_prefix("supabase:")
        .and_then(|value| Uuid::parse_str(value).ok())
        .map(|value| value.to_string())
}

fn cloud_connection_projection_payload(user_id: &str, connection: &CloudConnectionRecord) -> Value {
    json!({
        "id": connection.id,
        "user_id": user_id,
        "provider": connection.provider,
        "link_mode": connection.link_mode,
        "status": connection.status,
        "display_name": connection.display_name,
        "folder_path": connection.folder_path,
        "token_expires_at": connection.token_expires_at,
        "last_sync_at": connection.last_sync_at,
        "created_at": connection.created_at,
        "updated_at": connection.updated_at,
    })
}

async fn claim_cloud_connection_projections(
    client: &DbClient,
    limit: i64,
) -> Result<Vec<CloudConnectionProjectionClaim>, ServiceError> {
    let rows = client
        .query(
            "with candidates as (
               select outbox.seq
               from sound_recorder_cloud_connection_projection_outbox outbox
               where outbox.processed_at is null
                 and outbox.available_at <= now()
                 and (outbox.locked_until is null or outbox.locked_until < now())
               order by outbox.available_at asc, outbox.seq asc
               for update skip locked
               limit $1
             ),
             claimed as (
               update sound_recorder_cloud_connection_projection_outbox outbox
               set attempts = least(outbox.attempts + 1, 50),
                   locked_until = now() + $2::interval,
                   updated_at = now()
               from candidates
               where outbox.seq = candidates.seq
               returning outbox.seq, outbox.connection_id, outbox.attempts
             )
             select claimed.seq, claimed.attempts, account.external_subject,
                    connection.id::text, connection.account_id::text,
                    connection.provider, connection.link_mode, connection.status,
                    connection.display_name, connection.provider_account_id,
                    connection.root_folder_id, connection.folder_path,
                    connection.token_ciphertext, connection.token_nonce,
                    connection.token_aad, connection.token_version,
                    connection.token_expires_at, connection.last_sync_at,
                    connection.created_at, connection.updated_at
             from claimed
             join sound_recorder_cloud_connections connection
               on connection.id = claimed.connection_id
             join sound_recorder_accounts account
               on account.id = connection.account_id
             order by claimed.seq asc",
            &[&limit, &CLOUD_PROJECTION_CLAIM_LEASE],
        )
        .await
        .map_err(db_error)?;
    Ok(rows
        .iter()
        .map(|row| CloudConnectionProjectionClaim {
            seq: row.get("seq"),
            attempts: row.get("attempts"),
            external_subject: row
                .get::<Option<String>>("external_subject")
                .unwrap_or_default(),
            connection: cloud_connection_record_from_row(row),
        })
        .collect())
}

async fn write_cloud_connection_projection(
    state: &AppState,
    claim: &CloudConnectionProjectionClaim,
) -> Result<bool, ServiceError> {
    let Some(user_id) = supabase_user_id_from_external_subject(&claim.external_subject) else {
        return Ok(false);
    };
    let base_url = state.config.supabase.url.as_deref().ok_or_else(|| {
        ServiceError::Unavailable(
            "SOUND_RECORDER_SUPABASE_URL is required for cloud connection projection".to_string(),
        )
    })?;
    let service_role_key = state
        .config
        .supabase
        .service_role_key
        .as_deref()
        .ok_or_else(|| {
            ServiceError::Unavailable(
                "SOUND_RECORDER_SUPABASE_SERVICE_ROLE_KEY is required for cloud connection projection"
                    .to_string(),
            )
        })?;
    let payload = cloud_connection_projection_payload(&user_id, &claim.connection);
    let request = state
        .http
        .post(format!(
            "{}/rest/v1/{CLOUD_CONNECTIONS_TABLE}?on_conflict=id",
            base_url.trim_end_matches('/')
        ))
        .header("apikey", service_role_key)
        .bearer_auth(service_role_key)
        .header("prefer", "resolution=merge-duplicates,return=minimal")
        .json(&payload)
        .send();
    let response = tokio::time::timeout(SUPABASE_PROBE_TIMEOUT, request)
        .await
        .map_err(|_| {
            ServiceError::Unavailable("Supabase cloud connection projection timed out".to_string())
        })?
        .map_err(|error| {
            warn!(
                error = %error,
                connection_id = claim.connection.id,
                "Supabase cloud connection projection request failed"
            );
            ServiceError::Unavailable("Supabase cloud connection projection failed".to_string())
        })?;
    if response.status().is_success() {
        Ok(true)
    } else {
        warn!(
            status = response.status().as_u16(),
            connection_id = claim.connection.id,
            "Supabase cloud connection projection returned non-success"
        );
        Err(ServiceError::Unavailable(format!(
            "Supabase cloud connection projection failed with status {}",
            response.status().as_u16()
        )))
    }
}

async fn mark_cloud_connection_projection_success(
    client: &DbClient,
    claim: &CloudConnectionProjectionClaim,
) -> Result<bool, ServiceError> {
    client
        .execute(
            "update sound_recorder_cloud_connection_projection_outbox
             set processed_at = now(),
                 locked_until = null,
                 last_error = null,
                 updated_at = now()
             where seq = $1 and attempts = $2 and processed_at is null",
            &[&claim.seq, &claim.attempts],
        )
        .await
        .map(|updated| updated > 0)
        .map_err(db_error)
}

fn cloud_projection_retry_at(attempts: i32) -> DateTime<Utc> {
    let exponent = (attempts.saturating_sub(1) as u32).min(6);
    let seconds = 60_i64.saturating_mul(1_i64 << exponent).min(3600);
    Utc::now() + ChronoDuration::seconds(seconds)
}

async fn mark_cloud_connection_projection_error(
    client: &DbClient,
    claim: &CloudConnectionProjectionClaim,
    message: &str,
) -> Result<bool, ServiceError> {
    let available_at = cloud_projection_retry_at(claim.attempts);
    let last_error = message.chars().take(500).collect::<String>();
    client
        .execute(
            "update sound_recorder_cloud_connection_projection_outbox
             set available_at = $3,
                 locked_until = null,
                 last_error = $4,
                 updated_at = now()
             where seq = $1 and attempts = $2 and processed_at is null",
            &[&claim.seq, &claim.attempts, &available_at, &last_error],
        )
        .await
        .map(|updated| updated > 0)
        .map_err(db_error)
}

async fn drain_cloud_connection_projections(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<DrainCloudConnectionProjectionsRequest>,
) -> Result<Json<DrainCloudConnectionProjectionsResponse>, ServiceError> {
    require_internal_auth(&state.config, &headers)?;
    let client = db_conn(&state).await?;
    let limit = req
        .max_items
        .unwrap_or(DEFAULT_CLOUD_PROJECTION_BATCH_SIZE)
        .clamp(1, MAX_CLOUD_PROJECTION_BATCH_SIZE);
    let claims = claim_cloud_connection_projections(&client, limit).await?;
    let mut completed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    for claim in &claims {
        match write_cloud_connection_projection(&state, claim).await {
            Ok(written) => {
                if mark_cloud_connection_projection_success(&client, claim).await? {
                    if written {
                        completed += 1;
                    } else {
                        skipped += 1;
                    }
                } else {
                    skipped += 1;
                }
            }
            Err(error) => {
                let message = service_error_message(&error);
                if mark_cloud_connection_projection_error(&client, claim, &message).await? {
                    failed += 1;
                } else {
                    skipped += 1;
                }
            }
        }
    }
    record_request(
        "POST",
        "/internal/cloud-connection-projections/drain",
        StatusCode::OK,
    );
    Ok(Json(DrainCloudConnectionProjectionsResponse {
        ok: true,
        attempted: claims.len(),
        completed,
        failed,
        skipped,
    }))
}

fn service_error_message(error: &ServiceError) -> String {
    let message = match error {
        ServiceError::BadRequest(message)
        | ServiceError::NotFound(message)
        | ServiceError::Conflict(message)
        | ServiceError::Unavailable(message)
        | ServiceError::Internal(message) => message.as_str(),
        ServiceError::Unauthorized => "unauthorized",
        ServiceError::MfaRequired => "mfa required",
    };
    message.chars().take(500).collect()
}

async fn claim_cloud_copy_job(
    client: &DbClient,
    job_id: &str,
) -> Result<Option<CloudCopyClaim>, ServiceError> {
    let locked_until = Utc::now()
        .checked_add_signed(ChronoDuration::minutes(5))
        .unwrap_or_else(Utc::now);
    let row = client
        .query_opt(
            "update sound_recorder_cloud_copy_jobs
             set status = 'running',
                 attempts = attempts + 1,
                 started_at = coalesce(started_at, now()),
                 locked_until = $2,
                 updated_at = now()
             where id = $1::uuid
               and (
                 (status = 'pending' and (locked_until is null or locked_until < now()))
                 or (status = 'running' and locked_until < now())
               )
             returning attempts",
            &[&job_id, &locked_until],
        )
        .await
        .map_err(db_error)?;
    Ok(row.map(|row| CloudCopyClaim {
        attempts: row.get("attempts"),
    }))
}

async fn process_cloud_copy_job(
    state: &AppState,
    client: &DbClient,
    item: &CloudCopyWorkItem,
) -> Result<String, ServiceError> {
    let provider = CloudProvider::parse(&item.job.provider)?;
    if !provider.is_server_managed() {
        return Err(ServiceError::BadRequest(
            "cloud provider is not server managed".to_string(),
        ));
    }
    if item
        .segment
        .byte_count
        .map(|bytes| bytes as i64 > state.config.cloud_copy_max_bytes)
        .unwrap_or(false)
    {
        return Err(ServiceError::BadRequest(
            "segment is larger than the cloud copy byte limit".to_string(),
        ));
    }
    let token_set = token_set_for_connection(state, client, &item.connection).await?;
    let bytes = download_segment_bytes(state, &item.segment).await?;
    // Zero-knowledge by default: we mirror the on-device ciphertext untouched.
    // The opt-in per-clip DEK plumbing (a follow-up: schema column + mobile
    // authorisation flow) passes a released key here so a clip can land playable.
    let bytes = apply_opt_in_segment_decryption(None, bytes)?;
    if bytes.len() as i64 > state.config.cloud_copy_max_bytes {
        return Err(ServiceError::BadRequest(
            "segment is larger than the cloud copy byte limit".to_string(),
        ));
    }
    match provider {
        CloudProvider::GoogleDrive => {
            upload_to_google_drive(
                state,
                &item.connection,
                &item.segment,
                &item.job,
                bytes,
                &token_set,
            )
            .await
        }
        CloudProvider::MicrosoftOneDrive => {
            upload_to_microsoft_onedrive(state, &item.segment, &item.job, bytes, &token_set).await
        }
        CloudProvider::Dropbox => {
            upload_to_dropbox(state, &item.segment, &item.job, bytes, &token_set).await
        }
        CloudProvider::AppleICloud | CloudProvider::AmazonS3 | CloudProvider::CloudflareR2 => Err(
            ServiceError::BadRequest(format!("{} is client managed", provider.as_str())),
        ),
    }
}

async fn download_segment_bytes(
    state: &AppState,
    segment: &SegmentResponse,
) -> Result<Vec<u8>, ServiceError> {
    require_storage_history_compatible(state).await?;
    let s3 = state
        .s3
        .as_ref()
        .ok_or_else(|| ServiceError::Unavailable("S3 client is not configured".to_string()))?;
    let object = tokio::time::timeout(
        STORAGE_OBJECT_TIMEOUT,
        s3.get_object()
            .bucket(&segment.storage_bucket)
            .key(&segment.storage_key)
            .send(),
    )
    .await
    .map_err(|_| {
        error!(segment_id = segment.id, "S3 segment download timed out");
        ServiceError::Unavailable("S3 segment download timed out".to_string())
    })?
    .map_err(|err| {
        error!(error = %err, segment_id = segment.id, "S3 segment download failed");
        ServiceError::Unavailable("S3 segment download failed".to_string())
    })?;
    let bytes = tokio::time::timeout(STORAGE_OBJECT_TIMEOUT, object.body.collect())
        .await
        .map_err(|_| {
            error!(segment_id = segment.id, "S3 segment body read timed out");
            ServiceError::Unavailable("S3 segment body read timed out".to_string())
        })?
        .map_err(|err| {
            error!(error = %err, segment_id = segment.id, "S3 segment body read failed");
            ServiceError::Unavailable("S3 segment body read failed".to_string())
        })?;
    Ok(bytes.into_bytes().to_vec())
}

async fn upload_to_google_drive(
    state: &AppState,
    connection: &CloudConnectionRecord,
    segment: &SegmentResponse,
    job: &CloudCopyJobRecord,
    bytes: Vec<u8>,
    token_set: &CloudTokenSet,
) -> Result<String, ServiceError> {
    upload_to_google_drive_in_chunks(
        state,
        connection,
        segment,
        job,
        bytes,
        token_set,
        GOOGLE_RESUMABLE_CHUNK_BYTES,
    )
    .await
}

async fn upload_to_google_drive_in_chunks(
    state: &AppState,
    connection: &CloudConnectionRecord,
    segment: &SegmentResponse,
    job: &CloudCopyJobRecord,
    bytes: Vec<u8>,
    token_set: &CloudTokenSet,
    chunk_bytes: usize,
) -> Result<String, ServiceError> {
    if bytes.is_empty() || chunk_bytes == 0 {
        return Err(ServiceError::BadRequest(
            "Google Drive upload requires non-empty content".to_string(),
        ));
    }
    let file_name = google_drive_file_name(&job.destination_key);
    let mut metadata = json!({
        "name": file_name,
        "description": format!("Sound recorder segment {}", segment.id)
    });
    if let Some(root_folder_id) = &connection.root_folder_id {
        metadata["parents"] = json!([root_folder_id]);
    }
    let url = append_query(
        &state.config.google_drive_upload_url,
        "uploadType=resumable&fields=id,name,webViewLink",
    );
    let start_response = state
        .http
        .post(url)
        .bearer_auth(&token_set.access_token)
        .header("content-type", "application/json; charset=UTF-8")
        .header("x-upload-content-type", segment.content_type.as_str())
        .header("x-upload-content-length", bytes.len())
        .body(metadata.to_string())
        .timeout(CLOUD_PROVIDER_UPLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|err| {
            error!(error = %err, segment_id = segment.id, "Google Drive resumable session request failed");
            ServiceError::Unavailable("Google Drive upload session failed".to_string())
        })?;
    let start_status = start_response.status();
    if !start_status.is_success() {
        let body = start_response.text().await.unwrap_or_default();
        error!(status = start_status.as_u16(), body = %body.chars().take(200).collect::<String>(), "Google Drive upload session failed");
        return Err(ServiceError::Unavailable(format!(
            "Google Drive upload session failed with status {}",
            start_status.as_u16()
        )));
    }
    let upload_url = start_response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            ServiceError::Unavailable(
                "Google Drive upload session returned no location".to_string(),
            )
        })?
        .to_string();
    if !is_safe_public_url(&upload_url) {
        return Err(ServiceError::Unavailable(
            "Google Drive upload session returned an unsafe location".to_string(),
        ));
    }

    let total = bytes.len();
    let mut offset = 0usize;
    let mut final_response = None;
    while offset < total {
        let end = offset.saturating_add(chunk_bytes).min(total);
        let response = state
            .http
            .put(&upload_url)
            .header("content-type", segment.content_type.as_str())
            .header("content-length", end - offset)
            .header(
                "content-range",
                format!("bytes {}-{}/{}", offset, end - 1, total),
            )
            .body(bytes[offset..end].to_vec())
            .timeout(CLOUD_PROVIDER_UPLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|err| {
                error!(error = %err, segment_id = segment.id, offset, "Google Drive resumable chunk failed");
                ServiceError::Unavailable("Google Drive upload failed".to_string())
            })?;
        if end < total {
            if response.status().as_u16() != 308 {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                error!(status = status.as_u16(), body = %body.chars().take(200).collect::<String>(), "Google Drive resumable chunk was not accepted");
                return Err(ServiceError::Unavailable(format!(
                    "Google Drive upload failed with status {}",
                    status.as_u16()
                )));
            }
        } else {
            final_response = Some(response);
        }
        offset = end;
    }

    let response = final_response.ok_or_else(|| {
        ServiceError::Internal("Google Drive upload produced no final response".to_string())
    })?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        error!(status = status.as_u16(), body = %body.chars().take(200).collect::<String>(), "Google Drive upload failed");
        return Err(ServiceError::Unavailable(format!(
            "Google Drive upload failed with status {}",
            status.as_u16()
        )));
    }
    let value = response.json::<Value>().await.map_err(|err| {
        error!(error = %err, "Google Drive upload response decode failed");
        ServiceError::Unavailable("Google Drive upload response was invalid".to_string())
    })?;
    value
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| {
            ServiceError::Unavailable("Google Drive upload did not return a file id".to_string())
        })
}

async fn upload_to_microsoft_onedrive(
    state: &AppState,
    segment: &SegmentResponse,
    job: &CloudCopyJobRecord,
    bytes: Vec<u8>,
    token_set: &CloudTokenSet,
) -> Result<String, ServiceError> {
    let path = graph_path_escape(&job.destination_key);
    let url = format!(
        "{}/me/drive/special/approot:/{path}:/content",
        state.config.microsoft_graph_base_url.trim_end_matches('/')
    );
    let response = state
        .http
        .put(url)
        .bearer_auth(&token_set.access_token)
        .header("content-type", segment.content_type.as_str())
        .body(bytes)
        .timeout(CLOUD_PROVIDER_UPLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|err| {
            error!(error = %err, segment_id = segment.id, "Microsoft OneDrive upload request failed");
            ServiceError::Unavailable("Microsoft OneDrive upload failed".to_string())
        })?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        error!(status = status.as_u16(), body = %body.chars().take(200).collect::<String>(), "Microsoft OneDrive upload failed");
        return Err(ServiceError::Unavailable(format!(
            "Microsoft OneDrive upload failed with status {}",
            status.as_u16()
        )));
    }
    let value = response.json::<Value>().await.map_err(|err| {
        error!(error = %err, "Microsoft OneDrive upload response decode failed");
        ServiceError::Unavailable("Microsoft OneDrive upload response was invalid".to_string())
    })?;
    value
        .get("id")
        .or_else(|| value.get("webUrl"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| {
            ServiceError::Unavailable(
                "Microsoft OneDrive upload did not return a file id".to_string(),
            )
        })
}

async fn upload_to_dropbox(
    state: &AppState,
    segment: &SegmentResponse,
    job: &CloudCopyJobRecord,
    bytes: Vec<u8>,
    token_set: &CloudTokenSet,
) -> Result<String, ServiceError> {
    if bytes.len() > DROPBOX_SINGLE_UPLOAD_MAX_BYTES {
        return upload_to_dropbox_session(
            state,
            segment,
            job,
            bytes,
            token_set,
            DROPBOX_SESSION_CHUNK_BYTES,
        )
        .await;
    }
    let path = format!(
        "/{}",
        job.destination_key
            .trim()
            .trim_start_matches('/')
            .replace('\\', "_")
    );
    let api_arg = json!({
        "path": path,
        "mode": "overwrite",
        "autorename": false,
        "mute": true,
        "strict_conflict": false
    });
    let response = state
        .http
        .post(&state.config.dropbox_upload_url)
        .bearer_auth(&token_set.access_token)
        .header("Dropbox-API-Arg", api_arg.to_string())
        .header("content-type", "application/octet-stream")
        .body(bytes)
        .timeout(CLOUD_PROVIDER_UPLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|err| {
            error!(error = %err, segment_id = segment.id, "Dropbox upload request failed");
            ServiceError::Unavailable("Dropbox upload failed".to_string())
        })?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        error!(status = status.as_u16(), body = %body.chars().take(200).collect::<String>(), "Dropbox upload failed");
        return Err(ServiceError::Unavailable(format!(
            "Dropbox upload failed with status {}",
            status.as_u16()
        )));
    }
    let value = response.json::<Value>().await.map_err(|err| {
        error!(error = %err, "Dropbox upload response decode failed");
        ServiceError::Unavailable("Dropbox upload response was invalid".to_string())
    })?;
    value
        .get("id")
        .or_else(|| value.get("path_display"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| {
            ServiceError::Unavailable("Dropbox upload did not return a file id".to_string())
        })
}

fn dropbox_session_endpoint(upload_url: &str, operation: &str) -> Result<String, ServiceError> {
    let suffix = "/files/upload";
    let base = upload_url.strip_suffix(suffix).ok_or_else(|| {
        ServiceError::Unavailable("Dropbox upload URL must end with /files/upload".to_string())
    })?;
    Ok(format!("{base}/files/upload_session/{operation}"))
}

async fn upload_to_dropbox_session(
    state: &AppState,
    segment: &SegmentResponse,
    job: &CloudCopyJobRecord,
    bytes: Vec<u8>,
    token_set: &CloudTokenSet,
    chunk_bytes: usize,
) -> Result<String, ServiceError> {
    if bytes.is_empty() || chunk_bytes == 0 {
        return Err(ServiceError::BadRequest(
            "Dropbox upload requires non-empty content".to_string(),
        ));
    }
    let start_url = dropbox_session_endpoint(&state.config.dropbox_upload_url, "start")?;
    let append_url = dropbox_session_endpoint(&state.config.dropbox_upload_url, "append_v2")?;
    let finish_url = dropbox_session_endpoint(&state.config.dropbox_upload_url, "finish")?;
    let start_response = state
        .http
        .post(start_url)
        .bearer_auth(&token_set.access_token)
        .header("Dropbox-API-Arg", json!({"close": false}).to_string())
        .header("content-type", "application/octet-stream")
        .body(Vec::<u8>::new())
        .timeout(CLOUD_PROVIDER_UPLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|err| {
            error!(error = %err, segment_id = segment.id, "Dropbox upload session request failed");
            ServiceError::Unavailable("Dropbox upload session failed".to_string())
        })?;
    let start_status = start_response.status();
    if !start_status.is_success() {
        return Err(ServiceError::Unavailable(format!(
            "Dropbox upload session failed with status {}",
            start_status.as_u16()
        )));
    }
    let start_body = start_response.json::<Value>().await.map_err(|_| {
        ServiceError::Unavailable("Dropbox upload session response was invalid".to_string())
    })?;
    let session_id = start_body
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ServiceError::Unavailable("Dropbox upload session returned no session id".to_string())
        })?;
    let path = format!(
        "/{}",
        job.destination_key
            .trim()
            .trim_start_matches('/')
            .replace('\\', "_")
    );
    let commit = json!({
        "path": path,
        "mode": "overwrite",
        "autorename": false,
        "mute": true,
        "strict_conflict": false
    });
    let total = bytes.len();
    let mut offset = 0usize;
    while offset < total {
        let end = offset.saturating_add(chunk_bytes).min(total);
        let final_chunk = end == total;
        let cursor = json!({"session_id": session_id, "offset": offset});
        let (url, api_arg) = if final_chunk {
            (
                finish_url.as_str(),
                json!({"cursor": cursor, "commit": commit.clone()}),
            )
        } else {
            (
                append_url.as_str(),
                json!({"cursor": cursor, "close": false}),
            )
        };
        let response = state
            .http
            .post(url)
            .bearer_auth(&token_set.access_token)
            .header("Dropbox-API-Arg", api_arg.to_string())
            .header("content-type", "application/octet-stream")
            .body(bytes[offset..end].to_vec())
            .timeout(CLOUD_PROVIDER_UPLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|err| {
                error!(error = %err, segment_id = segment.id, offset, "Dropbox upload session chunk failed");
                ServiceError::Unavailable("Dropbox upload failed".to_string())
            })?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = status.as_u16(), body = %body.chars().take(200).collect::<String>(), "Dropbox upload session chunk failed");
            return Err(ServiceError::Unavailable(format!(
                "Dropbox upload failed with status {}",
                status.as_u16()
            )));
        }
        if final_chunk {
            let value = response.json::<Value>().await.map_err(|_| {
                ServiceError::Unavailable("Dropbox upload response was invalid".to_string())
            })?;
            return value
                .get("id")
                .or_else(|| value.get("path_display"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .ok_or_else(|| {
                    ServiceError::Unavailable("Dropbox upload did not return a file id".to_string())
                });
        }
        offset = end;
    }
    Err(ServiceError::Internal(
        "Dropbox upload produced no final response".to_string(),
    ))
}

async fn mark_cloud_copy_job_success(
    client: &DbClient,
    item: &CloudCopyWorkItem,
    provider_file_id: &str,
    attempts: i32,
) -> Result<bool, ServiceError> {
    let updated = client
        .execute(
            "update sound_recorder_cloud_copy_jobs
             set status = 'completed',
                 provider_file_id = $2,
                 completed_at = now(),
                 locked_until = null,
                 last_error = null,
                 updated_at = now()
             where id = $1::uuid
               and status = 'running'
               and attempts = $3",
            &[&item.job.id, &provider_file_id, &attempts],
        )
        .await
        .map_err(db_error)?;
    if updated == 0 {
        warn!(
            job_id = item.job.id,
            attempts, "cloud-copy completion ignored because the lease was superseded"
        );
        return Ok(false);
    }
    client
        .execute(
            "update sound_recorder_cloud_connections
             set last_sync_at = now(), updated_at = now()
             where id = $1::uuid",
            &[&item.connection.id],
        )
        .await
        .map_err(db_error)?;
    Ok(true)
}

async fn mark_cloud_copy_job_error(
    client: &DbClient,
    job_id: &str,
    attempts: i32,
    message: &str,
    config: &Config,
) -> Result<bool, ServiceError> {
    let status = if attempts >= config.cloud_copy_max_attempts {
        "failed"
    } else {
        "pending"
    };
    let locked_until = if status == "pending" {
        Utc::now().checked_add_signed(ChronoDuration::seconds(
            60_i64.saturating_mul(attempts.max(1) as i64),
        ))
    } else {
        None
    };
    let last_error = message.chars().take(500).collect::<String>();
    let updated = client
        .execute(
            "update sound_recorder_cloud_copy_jobs
             set status = $2,
                 locked_until = $3,
                 last_error = $4,
                 updated_at = now()
             where id = $1::uuid
               and status = 'running'
               and attempts = $5",
            &[&job_id, &status, &locked_until, &last_error, &attempts],
        )
        .await
        .map_err(db_error)?;
    if updated == 0 {
        warn!(
            job_id,
            attempts, "cloud-copy failure ignored because the lease was superseded"
        );
        return Ok(false);
    }
    Ok(true)
}

async fn retention_sweep(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<RetentionSweepResponse>, ServiceError> {
    require_internal_auth(&state.config, &headers)?;
    let client = db_conn(&state).await?;
    require_storage_history_compatible(&state).await?;
    // Bound work per call so a large backlog is drained across cron runs instead
    // of one unbounded transaction. The cron re-invokes until nothing is left.
    const SWEEP_BATCH: i64 = 1000;
    let claim_id = Uuid::new_v4().to_string();
    let rows = client
        .query(
            "with candidates as (
               select id,
                      case
                        when status in ('pending', 'uploaded') then status
                        when meta_data->>($5::text) in ('pending', 'uploaded')
                          then meta_data->>($5::text)
                        else 'uploaded'
                      end as previous_status
               from sound_recorder_segments
               where pinned_at is null
                 and (
                   (status in ('pending', 'uploaded') and expires_at < now()
                     and (status <> 'pending' or upload_url_expires_at is null
                          or upload_url_expires_at < now()))
                   or (
                     status = 'expired'
                     and meta_data->>($2::text) = 'true'
                     and (
                       nullif(meta_data->>($4::text), '') is null
                       or (meta_data->>($4::text))::timestamptz
                            < now() - interval '10 minutes'
                     )
                   )
                 )
               order by expires_at asc
               limit $1
               for update skip locked
             )
             update sound_recorder_segments s
             set status = 'expired',
                 meta_data = (
                   coalesce(s.meta_data, '{}'::jsonb)
                   - $2::text - $3::text - $4::text - $5::text
                 ) || jsonb_build_object(
                   $2::text, true,
                   $3::text, $6::text,
                   $4::text, now(),
                   $5::text, candidates.previous_status
                 ),
                 updated_at = now()
             from candidates
             where s.id = candidates.id and s.pinned_at is null
             returning s.id::text, s.storage_bucket, s.storage_key,
                       s.meta_data->>($7::text) as mirror_bucket,
                       candidates.previous_status",
            &[
                &SWEEP_BATCH,
                &RETENTION_DELETE_PENDING_META_KEY,
                &RETENTION_DELETE_CLAIM_ID_META_KEY,
                &RETENTION_DELETE_CLAIMED_AT_META_KEY,
                &RETENTION_PREVIOUS_STATUS_META_KEY,
                &claim_id,
                &MIRROR_BUCKET_META_KEY,
            ],
        )
        .await
        .map_err(db_error)?;

    // The atomic claim above moves each candidate out of the readable/uploadable
    // states before remote I/O. That closes the pin/delete and completion/delete
    // races. A bounded lease lets a later sweep reclaim work after a process
    // crash; the S3 DeleteObject call is idempotent.
    let mut deleted_objects: u64 = 0;
    let mut delete_failures: u64 = 0;
    let mut expired: u64 = 0;
    for row in &rows {
        let id: String = row.get("id");
        let bucket: String = row.get("storage_bucket");
        let key: String = row.get("storage_key");
        let mirror_bucket: Option<String> = row.get("mirror_bucket");
        let previous_status: String = row.get("previous_status");
        let primary_deleted = if bucket.is_empty() || key.is_empty() {
            true
        } else if let Some(s3) = state.s3.as_ref() {
            match tokio::time::timeout(
                STORAGE_OBJECT_TIMEOUT,
                s3.delete_object().bucket(&bucket).key(&key).send(),
            )
            .await
            {
                Ok(Ok(_)) => {
                    deleted_objects += 1;
                    true
                }
                Ok(Err(err)) => {
                    delete_failures += 1;
                    warn!(error = %err, segment_id = id, "retention S3 object delete failed");
                    false
                }
                Err(_) => {
                    delete_failures += 1;
                    warn!(segment_id = id, "retention S3 object delete timed out");
                    false
                }
            }
        } else {
            delete_failures += 1;
            warn!(
                segment_id = id,
                "retention object delete skipped; S3 client unavailable"
            );
            false
        };
        // Retention is a physical-erasure guarantee, so a row only finalizes
        // once the backup copy is gone too. A recorded mirror copy with no
        // mirror client keeps the row claimed (and retried) instead of
        // silently leaving audio behind in the mirror bucket.
        let mirror_deleted = if !primary_deleted || key.is_empty() {
            primary_deleted
        } else {
            let recorded_bucket = mirror_bucket.as_deref().filter(|value| !value.is_empty());
            match (recorded_bucket, state.mirror.as_ref()) {
                (None, None) => true,
                // No bookkeeping but a mirror is configured: delete the same
                // key defensively. DeleteObject on a missing key succeeds, and
                // this covers a copy whose meta_data update was lost between
                // the mirror PUT and the bookkeeping write.
                (None, Some(mirror)) => {
                    delete_mirror_object(mirror, &state.config.mirror.bucket, &key, &id).await
                }
                (Some(recorded), Some(mirror)) => {
                    delete_mirror_object(mirror, recorded, &key, &id).await
                }
                (Some(recorded), None) => {
                    warn!(
                        segment_id = id,
                        mirror_bucket = recorded,
                        "retention mirror delete skipped; mirror client unavailable"
                    );
                    false
                }
            }
        };
        if primary_deleted && !mirror_deleted {
            delete_failures += 1;
        }
        let deleted = primary_deleted && mirror_deleted;
        if deleted {
            expired += client
                .execute(
                    "update sound_recorder_segments
                     set meta_data = coalesce(meta_data, '{}'::jsonb)
                       - $3::text - $4::text - $5::text - $6::text,
                         updated_at = now()
                     where id = $1::uuid
                       and status = 'expired'
                       and meta_data->>($3::text) = 'true'
                       and meta_data->>($4::text) = $2",
                    &[
                        &id,
                        &claim_id,
                        &RETENTION_DELETE_PENDING_META_KEY,
                        &RETENTION_DELETE_CLAIM_ID_META_KEY,
                        &RETENTION_DELETE_CLAIMED_AT_META_KEY,
                        &RETENTION_PREVIOUS_STATUS_META_KEY,
                    ],
                )
                .await
                .map_err(db_error)?;
        } else {
            // Restore the prior state only if this sweep still owns the claim.
            // Another process cannot steal a live claim before its ten-minute
            // lease, far longer than the bounded 30-second object operation.
            let previous_status = if previous_status == "pending" {
                "pending"
            } else {
                "uploaded"
            };
            client
                .execute(
                    "update sound_recorder_segments
                 set status = $3,
                     meta_data = coalesce(meta_data, '{}'::jsonb)
                       - $4::text - $5::text - $6::text - $7::text,
                     updated_at = now()
                 where id = $1::uuid
                   and status = 'expired'
                   and meta_data->>($4::text) = 'true'
                   and meta_data->>($5::text) = $2",
                    &[
                        &id,
                        &claim_id,
                        &previous_status,
                        &RETENTION_DELETE_PENDING_META_KEY,
                        &RETENTION_DELETE_CLAIM_ID_META_KEY,
                        &RETENTION_DELETE_CLAIMED_AT_META_KEY,
                        &RETENTION_PREVIOUS_STATUS_META_KEY,
                    ],
                )
                .await
                .map_err(db_error)?;
        }
    }
    audit_event(
        &client,
        None,
        None,
        "sound_recorder.retention.swept",
        json!({
            "expiredSegments": expired,
            "deletedObjects": deleted_objects,
            "deleteFailures": delete_failures,
        }),
    )
    .await;
    record_request("POST", "/internal/retention/sweep", StatusCode::OK);
    Ok(Json(RetentionSweepResponse {
        ok: true,
        expired_segments: expired,
        deleted_objects,
        delete_failures,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MirrorDrainResponse {
    ok: bool,
    attempted: u64,
    mirrored: u64,
    failed: u64,
    skipped: u64,
}

/// Exponential retry backoff for failed mirror copies, capped at one hour.
fn mirror_retry_backoff(attempts: i32) -> ChronoDuration {
    let exponent = attempts.clamp(0, 6) as u32;
    let seconds = 60_i64.saturating_mul(1_i64 << exponent).min(3600);
    ChronoDuration::seconds(seconds)
}

/// Idempotently deletes one mirror copy; DeleteObject on a missing key
/// succeeds, so `true` means "no mirror copy remains at this bucket/key".
async fn delete_mirror_object(
    mirror: &aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
    segment_id: &str,
) -> bool {
    match tokio::time::timeout(
        STORAGE_OBJECT_TIMEOUT,
        mirror.delete_object().bucket(bucket).key(key).send(),
    )
    .await
    {
        Ok(Ok(_)) => true,
        Ok(Err(err)) => {
            warn!(error = %err, segment_id, "mirror object delete failed");
            false
        }
        Err(_) => {
            warn!(segment_id, "mirror object delete timed out");
            false
        }
    }
}

async fn download_object_bytes(
    state: &AppState,
    segment_id: &str,
    bucket: &str,
    key: &str,
) -> Result<Vec<u8>, ServiceError> {
    let s3 = state
        .s3
        .as_ref()
        .ok_or_else(|| ServiceError::Unavailable("S3 client is not configured".to_string()))?;
    let object = tokio::time::timeout(
        STORAGE_OBJECT_TIMEOUT,
        s3.get_object().bucket(bucket).key(key).send(),
    )
    .await
    .map_err(|_| ServiceError::Unavailable("primary object download timed out".to_string()))?
    .map_err(|err| {
        warn!(error = %err, segment_id, "primary object download failed");
        ServiceError::Unavailable("primary object download failed".to_string())
    })?;
    let bytes = tokio::time::timeout(STORAGE_OBJECT_TIMEOUT, object.body.collect())
        .await
        .map_err(|_| ServiceError::Unavailable("primary object body read timed out".to_string()))?
        .map_err(|err| {
            warn!(error = %err, segment_id, "primary object body read failed");
            ServiceError::Unavailable("primary object body read failed".to_string())
        })?;
    Ok(bytes.into_bytes().to_vec())
}

/// The primary-store location and recorded integrity expectations of one
/// claimed segment awaiting a mirror copy.
struct MirrorCopySource<'a> {
    segment_id: &'a str,
    bucket: &'a str,
    key: &'a str,
    content_type: &'a str,
    byte_count: Option<i32>,
    sha256_hex: Option<&'a str>,
}

/// Copies one claimed segment from the primary store into the mirror bucket
/// under the same account-scoped key, verifying size and (when recorded)
/// SHA-256 before the copy so a corrupted or tampered primary object can never
/// silently poison the backup.
async fn mirror_copy_segment(
    state: &AppState,
    mirror: &aws_sdk_s3::Client,
    source: &MirrorCopySource<'_>,
) -> Result<(), ServiceError> {
    let MirrorCopySource {
        segment_id,
        bucket,
        key,
        content_type,
        byte_count,
        sha256_hex,
    } = *source;
    let bytes = download_object_bytes(state, segment_id, bucket, key).await?;
    if let Some(expected) = byte_count {
        if bytes.len() as i64 != expected as i64 {
            return Err(ServiceError::Internal(format!(
                "primary object size {} does not match recorded byteCount {expected}",
                bytes.len()
            )));
        }
    }
    if let Some(expected) = sha256_hex.filter(|value| !value.is_empty()) {
        let digest = Sha256::digest(&bytes);
        let actual = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(ServiceError::Internal(
                "primary object SHA-256 does not match the recorded segment hash".to_string(),
            ));
        }
    }
    let mut request = mirror
        .put_object()
        .bucket(&state.config.mirror.bucket)
        .key(key)
        .content_type(content_type)
        .content_length(bytes.len() as i64)
        .body(ByteStream::from(bytes));
    if state.config.mirror.send_sse_aes256 {
        request = request.server_side_encryption(ServerSideEncryption::Aes256);
    }
    tokio::time::timeout(STORAGE_OBJECT_TIMEOUT, request.send())
        .await
        .map_err(|_| ServiceError::Unavailable("mirror object upload timed out".to_string()))?
        .map_err(|err| {
            warn!(error = %err, segment_id, "mirror object upload failed");
            ServiceError::Unavailable("mirror object upload failed".to_string())
        })?;
    Ok(())
}

/// Server-side backup job: copies settled (`uploaded`) segments from the
/// primary object store into the configured mirror (e.g. Cloudflare R2 next to
/// AWS S3) and records per-segment mirror state in `meta_data`. Cron-driven
/// like the retention sweep — each call drains one bounded batch, claims rows
/// with a reclaimable lease, and the cron re-invokes until nothing is left.
async fn mirror_drain(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<MirrorDrainResponse>, ServiceError> {
    require_internal_auth(&state.config, &headers)?;
    if !state.config.mirror.is_configured() {
        return Err(ServiceError::Unavailable(
            "mirror storage is not configured".to_string(),
        ));
    }
    let Some(mirror) = state.mirror.clone() else {
        return Err(ServiceError::Unavailable(
            "mirror storage client is not configured".to_string(),
        ));
    };
    require_storage_history_compatible(&state).await?;
    let client = db_conn(&state).await?;
    let claim_id = Uuid::new_v4().to_string();
    // meta_data values on legacy rows may predate the server-owned strip list,
    // so every cast is guarded by a CASE + format check instead of trusting
    // client-influenced text to be a number or timestamp.
    let rows = client
        .query(
            "with candidates as (
               select id
               from sound_recorder_segments
               where status = 'uploaded'
                 and storage_bucket <> ''
                 and storage_key <> ''
                 and meta_data->>($2::text) is distinct from 'mirrored'
                 and case
                   when meta_data->>($2::text) is distinct from 'copying' then true
                   when coalesce(meta_data->>($4::text), '') !~ '^\\d{4}-\\d{2}-\\d{2}' then true
                   else (meta_data->>($4::text))::timestamptz < now() - ($9::text)::interval
                 end
                 and case
                   when coalesce(meta_data->>($5::text), '') ~ '^\\d{1,9}$'
                     then (meta_data->>($5::text))::int < $6
                   else true
                 end
                 and case
                   when coalesce(meta_data->>($7::text), '') !~ '^\\d{4}-\\d{2}-\\d{2}' then true
                   else (meta_data->>($7::text))::timestamptz <= now()
                 end
               order by uploaded_at asc nulls last
               limit $1
               for update skip locked
             )
             update sound_recorder_segments s
             set meta_data = coalesce(s.meta_data, '{}'::jsonb) || jsonb_build_object(
                   $2::text, 'copying',
                   $3::text, $8::text,
                   $4::text, now()
                 ),
                 updated_at = now()
             from candidates
             where s.id = candidates.id
             returning s.id::text, s.storage_bucket, s.storage_key, s.content_type,
                       s.byte_count, s.sha256_hex,
                       case
                         when coalesce(s.meta_data->>($5::text), '') ~ '^\\d{1,9}$'
                           then (s.meta_data->>($5::text))::int
                         else 0
                       end as attempts,
                       s.meta_data->>($10::text) as storage_fingerprint",
            &[
                &state.config.mirror_batch_size,
                &MIRROR_STATE_META_KEY,
                &MIRROR_CLAIM_ID_META_KEY,
                &MIRROR_CLAIMED_AT_META_KEY,
                &MIRROR_ATTEMPTS_META_KEY,
                &state.config.mirror_copy_max_attempts,
                &MIRROR_NEXT_ATTEMPT_AT_META_KEY,
                &claim_id,
                &MIRROR_CLAIM_LEASE,
                &STORAGE_FINGERPRINT_META_KEY,
            ],
        )
        .await
        .map_err(db_error)?;

    let mut mirrored: u64 = 0;
    let mut failed: u64 = 0;
    let mut skipped: u64 = 0;
    for row in &rows {
        let id: String = row.get("id");
        let bucket: String = row.get("storage_bucket");
        let key: String = row.get("storage_key");
        let content_type: String = row.get("content_type");
        let byte_count: Option<i32> = row.get("byte_count");
        let sha256_hex: Option<String> = row.get("sha256_hex");
        let attempts: i32 = row.get("attempts");
        let storage_fingerprint: Option<String> = row.get("storage_fingerprint");
        // A row from a different (unacknowledged) backend must not be copied:
        // the primary client would read the wrong store. Release the claim
        // without consuming an attempt.
        if !storage_record_is_compatible(&state.config.s3, storage_fingerprint.as_deref()) {
            skipped += 1;
            client
                .execute(
                    "update sound_recorder_segments
                     set meta_data = coalesce(meta_data, '{}'::jsonb)
                           - $3::text - $4::text - $5::text,
                         updated_at = now()
                     where id = $1::uuid and meta_data->>($4::text) = $2",
                    &[
                        &id,
                        &claim_id,
                        &MIRROR_STATE_META_KEY,
                        &MIRROR_CLAIM_ID_META_KEY,
                        &MIRROR_CLAIMED_AT_META_KEY,
                    ],
                )
                .await
                .map_err(db_error)?;
            continue;
        }
        let copy_result = mirror_copy_segment(
            &state,
            &mirror,
            &MirrorCopySource {
                segment_id: &id,
                bucket: &bucket,
                key: &key,
                content_type: &content_type,
                byte_count,
                sha256_hex: sha256_hex.as_deref(),
            },
        )
        .await;
        match copy_result {
            Ok(()) => {
                mirrored += 1;
                MIRROR_COPIES.with_label_values(&["mirrored"]).inc();
                client
                    .execute(
                        "update sound_recorder_segments
                         set meta_data = (coalesce(meta_data, '{}'::jsonb)
                               - $3::text - $4::text - $5::text - $6::text - $7::text)
                             || jsonb_build_object(
                               $8::text, 'mirrored',
                               $9::text, now(),
                               $10::text, $11::text,
                               $12::text, $13::text
                             ),
                             updated_at = now()
                         where id = $1::uuid and meta_data->>($4::text) = $2",
                        &[
                            &id,
                            &claim_id,
                            &MIRROR_CLAIMED_AT_META_KEY,
                            &MIRROR_CLAIM_ID_META_KEY,
                            &MIRROR_ATTEMPTS_META_KEY,
                            &MIRROR_LAST_ERROR_META_KEY,
                            &MIRROR_NEXT_ATTEMPT_AT_META_KEY,
                            &MIRROR_STATE_META_KEY,
                            &MIRROR_MIRRORED_AT_META_KEY,
                            &MIRROR_BUCKET_META_KEY,
                            &state.config.mirror.bucket,
                            &MIRROR_FINGERPRINT_META_KEY,
                            &state.config.mirror.backend_fingerprint,
                        ],
                    )
                    .await
                    .map_err(db_error)?;
            }
            Err(error) => {
                failed += 1;
                MIRROR_COPIES.with_label_values(&["failed"]).inc();
                let attempts_next = attempts.saturating_add(1);
                let next_attempt_at = Utc::now() + mirror_retry_backoff(attempts_next);
                let message = service_error_message(&error);
                warn!(segment_id = id, attempts = attempts_next, error = %message, "segment mirror copy failed");
                client
                    .execute(
                        "update sound_recorder_segments
                         set meta_data = (coalesce(meta_data, '{}'::jsonb)
                               - $3::text - $4::text)
                             || jsonb_build_object(
                               $5::text, 'failed',
                               $6::text, $7,
                               $8::text, $9::text,
                               $10::text, $11
                             ),
                             updated_at = now()
                         where id = $1::uuid and meta_data->>($4::text) = $2",
                        &[
                            &id,
                            &claim_id,
                            &MIRROR_CLAIMED_AT_META_KEY,
                            &MIRROR_CLAIM_ID_META_KEY,
                            &MIRROR_STATE_META_KEY,
                            &MIRROR_ATTEMPTS_META_KEY,
                            &attempts_next,
                            &MIRROR_LAST_ERROR_META_KEY,
                            &message,
                            &MIRROR_NEXT_ATTEMPT_AT_META_KEY,
                            &next_attempt_at,
                        ],
                    )
                    .await
                    .map_err(db_error)?;
            }
        }
    }
    audit_event(
        &client,
        None,
        None,
        "sound_recorder.mirror.drained",
        json!({
            "attempted": rows.len() as u64,
            "mirrored": mirrored,
            "failed": failed,
            "skipped": skipped,
        }),
    )
    .await;
    record_request("POST", "/internal/storage-mirror/drain", StatusCode::OK);
    Ok(Json(MirrorDrainResponse {
        ok: true,
        attempted: rows.len() as u64,
        mirrored,
        failed,
        skipped,
    }))
}

async fn load_session_policy(
    client: &DbClient,
    auth: &DeviceAuth,
    session_id: &str,
) -> Result<SessionPolicy, ServiceError> {
    let row = client
        .query_opt(
            "select account_id::text, device_id::text, status, storage_bucket, storage_prefix,
                    content_type, codec, segment_duration_seconds, max_segment_bytes, meta_data
             from sound_recorder_upload_sessions
             where id = $1::uuid and account_id = $2::uuid and device_id = $3::uuid",
            &[&session_id, &auth.account_id, &auth.device_id],
        )
        .await
        .map_err(db_error)?;
    let Some(row) = row else {
        return Err(ServiceError::NotFound(
            "upload session not found".to_string(),
        ));
    };
    let meta_data: Value = row.get("meta_data");
    Ok(SessionPolicy {
        status: row.get("status"),
        storage_bucket: row.get("storage_bucket"),
        storage_prefix: row.get("storage_prefix"),
        storage_fingerprint: meta_data
            .get(STORAGE_FINGERPRINT_META_KEY)
            .and_then(Value::as_str)
            .map(ToString::to_string),
        content_type: row.get("content_type"),
        codec: row.get("codec"),
        segment_duration_seconds: row.get("segment_duration_seconds"),
        max_segment_bytes: row.get("max_segment_bytes"),
    })
}

async fn presign_put(
    state: &AppState,
    bucket: &str,
    key: &str,
    content_type: &str,
    byte_count: Option<i32>,
    expires_at: DateTime<Utc>,
) -> Result<PresignedTransfer, ServiceError> {
    require_storage_history_compatible(state).await?;
    let Some(s3) = &state.s3 else {
        return Err(ServiceError::Unavailable(
            "S3 client is not configured".to_string(),
        ));
    };
    let ttl = signed_ttl(expires_at);
    let presigning_config = PresigningConfig::builder()
        .expires_in(ttl)
        .build()
        .map_err(|err| ServiceError::Internal(format!("invalid presign ttl: {err}")))?;
    let mut request = s3
        .put_object()
        .bucket(bucket)
        .key(key)
        .content_type(content_type);
    // Cloudflare R2 rejects x-amz-server-side-encryption even though it
    // encrypts every object at rest. AWS S3 accepts the explicit AES256 header,
    // so `auto` emits it only for native AWS and omits it for R2/custom stores.
    if state.config.s3.send_sse_aes256 {
        request = request.server_side_encryption(ServerSideEncryption::Aes256);
    }
    if let Some(byte_count) = byte_count {
        request = request.content_length(byte_count as i64);
    }
    let presigned =
        tokio::time::timeout(STORAGE_PROBE_TIMEOUT, request.presigned(presigning_config))
            .await
            .map_err(|_| {
                SEGMENT_PRESIGNS
                    .with_label_values(&["upload", "error"])
                    .inc();
                ServiceError::Unavailable("S3 upload presign timed out".to_string())
            })?
            .map_err(|err| {
                error!(error = %err, "S3 upload presign failed");
                SEGMENT_PRESIGNS
                    .with_label_values(&["upload", "error"])
                    .inc();
                ServiceError::Unavailable("S3 upload presign failed".to_string())
            })?;
    Ok(PresignedTransfer {
        method: presigned.method().to_string(),
        url: presigned.uri().to_string(),
        headers: signed_headers(presigned.headers()),
        expires_at,
    })
}

async fn presign_get(
    state: &AppState,
    bucket: &str,
    key: &str,
    expires_at: DateTime<Utc>,
) -> Result<PresignedTransfer, ServiceError> {
    require_storage_history_compatible(state).await?;
    let Some(s3) = &state.s3 else {
        return Err(ServiceError::Unavailable(
            "S3 client is not configured".to_string(),
        ));
    };
    let ttl = signed_ttl(expires_at);
    let presigning_config = PresigningConfig::builder()
        .expires_in(ttl)
        .build()
        .map_err(|err| ServiceError::Internal(format!("invalid presign ttl: {err}")))?;
    let request = s3.get_object().bucket(bucket).key(key);
    let presigned =
        tokio::time::timeout(STORAGE_PROBE_TIMEOUT, request.presigned(presigning_config))
            .await
            .map_err(|_| {
                SEGMENT_PRESIGNS
                    .with_label_values(&["download", "error"])
                    .inc();
                ServiceError::Unavailable("S3 download presign timed out".to_string())
            })?
            .map_err(|err| {
                error!(error = %err, "S3 download presign failed");
                SEGMENT_PRESIGNS
                    .with_label_values(&["download", "error"])
                    .inc();
                ServiceError::Unavailable("S3 download presign failed".to_string())
            })?;
    Ok(PresignedTransfer {
        method: presigned.method().to_string(),
        url: presigned.uri().to_string(),
        headers: signed_headers(presigned.headers()),
        expires_at,
    })
}

fn signed_ttl(expires_at: DateTime<Utc>) -> Duration {
    let now = Utc::now();
    if expires_at <= now {
        Duration::from_secs(1)
    } else {
        (expires_at - now)
            .to_std()
            .unwrap_or_else(|_| Duration::from_secs(1))
    }
}

fn signed_headers<'a, I>(headers: I) -> Vec<SignedHeader>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    headers
        .into_iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("host"))
        .map(|(name, value)| SignedHeader {
            name: name.to_string(),
            value: value.to_string(),
        })
        .collect()
}

fn chrono_duration_from_std(duration: Duration) -> Result<ChronoDuration, ServiceError> {
    ChronoDuration::from_std(duration)
        .map_err(|_| ServiceError::Internal("duration is too large".to_string()))
}

fn upload_session_from_row(row: &Row) -> UploadSessionResponse {
    UploadSessionResponse {
        id: row.get("id"),
        account_id: row.get("account_id"),
        device_id: row.get("device_id"),
        status: row.get("status"),
        storage_prefix: row.get("storage_prefix"),
        content_type: row.get("content_type"),
        codec: row.get("codec"),
        segment_duration_seconds: row.get("segment_duration_seconds"),
        max_segment_bytes: row.get("max_segment_bytes"),
        started_at: row.get("started_at"),
        expires_at: row.get("expires_at"),
    }
}

fn segment_from_row(config: &Config, row: &Row) -> SegmentResponse {
    let storage_key: String = row.get("storage_key");
    SegmentResponse {
        id: row.get("id"),
        account_id: row.get("account_id"),
        device_id: row.get("device_id"),
        session_id: row.get("session_id"),
        sequence_number: row.get("sequence_number"),
        status: row.get("status"),
        storage_provider: row.get("storage_provider"),
        storage_bucket: row.get("storage_bucket"),
        cdn_url: cdn_url(config, &storage_key),
        storage_key,
        content_type: row.get("content_type"),
        codec: row.get("codec"),
        captured_started_at: row.get("captured_started_at"),
        captured_ended_at: row.get("captured_ended_at"),
        duration_millis: row.get("duration_millis"),
        byte_count: row.get("byte_count"),
        sha256_hex: row.get("sha256_hex"),
        upload_url_expires_at: row.get("upload_url_expires_at"),
        uploaded_at: row.get("uploaded_at"),
        expires_at: row.get("expires_at"),
    }
}

fn cloud_connection_from_row(row: &Row) -> CloudConnectionResponse {
    CloudConnectionResponse {
        id: row.get("id"),
        provider: row.get("provider"),
        link_mode: row.get("link_mode"),
        status: row.get("status"),
        display_name: row.get("display_name"),
        provider_account_id: row.get("provider_account_id"),
        root_folder_id: row.get("root_folder_id"),
        folder_path: row.get("folder_path"),
        token_expires_at: row.get("token_expires_at"),
        last_sync_at: row.get("last_sync_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn cloud_connection_record_from_row(row: &Row) -> CloudConnectionRecord {
    CloudConnectionRecord {
        id: row.get("id"),
        account_id: row.get("account_id"),
        provider: row.get("provider"),
        link_mode: row.get("link_mode"),
        status: row.get("status"),
        display_name: row.get("display_name"),
        provider_account_id: row.get("provider_account_id"),
        root_folder_id: row.get("root_folder_id"),
        folder_path: row.get("folder_path"),
        token_ciphertext: row.get("token_ciphertext"),
        token_nonce: row.get("token_nonce"),
        token_aad: row.get("token_aad"),
        token_version: row.get("token_version"),
        token_expires_at: row.get("token_expires_at"),
        last_sync_at: row.get("last_sync_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn cloud_copy_job_from_row(row: &Row) -> CloudCopyJobResponse {
    CloudCopyJobResponse {
        id: row.get("id"),
        connection_id: row.get("connection_id"),
        segment_id: row.get("segment_id"),
        provider: row.get("provider"),
        status: row.get("status"),
        destination_key: row.get("destination_key"),
        provider_file_id: row.get("provider_file_id"),
        attempts: row.get("attempts"),
        completed_at: row.get("completed_at"),
        last_error: row.get("last_error"),
    }
}

fn cloud_copy_work_item_from_row(config: &Config, row: &Row) -> CloudCopyWorkItem {
    CloudCopyWorkItem {
        job: CloudCopyJobRecord {
            id: row.get("job_id"),
            provider: row.get("job_provider"),
            destination_key: row.get("destination_key"),
        },
        connection: CloudConnectionRecord {
            id: row.get("connection_id"),
            account_id: row.get("connection_account_id"),
            provider: row.get("connection_provider"),
            link_mode: row.get("link_mode"),
            status: row.get("connection_status"),
            display_name: row.get("display_name"),
            provider_account_id: row.get("provider_account_id"),
            root_folder_id: row.get("root_folder_id"),
            folder_path: row.get("folder_path"),
            token_ciphertext: row.get("token_ciphertext"),
            token_nonce: row.get("token_nonce"),
            token_aad: row.get("token_aad"),
            token_version: row.get("token_version"),
            token_expires_at: row.get("token_expires_at"),
            last_sync_at: row.get("last_sync_at"),
            created_at: row.get("connection_created_at"),
            updated_at: row.get("connection_updated_at"),
        },
        segment: segment_from_row(config, row),
    }
}

fn new_oauth_state() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("sr_oauth_{}", URL_SAFE_NO_PAD.encode(bytes))
}

#[derive(Debug)]
struct OAuthPkce {
    verifier: String,
    challenge: String,
}

/// Generates an RFC 7636 verifier/challenge pair. The verifier stays in the
/// short-lived, account/device-bound OAuth state row; only its S256 challenge
/// is sent through the browser. This prevents a stolen authorization code from
/// being redeemed without the server-held verifier.
fn new_oauth_pkce() -> OAuthPkce {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    OAuthPkce {
        verifier,
        challenge,
    }
}

fn oauth_state_hash(state: &Config, token: &str) -> String {
    hash_secret(token, &state.token_pepper)
}

fn token_set_from_response(response: OAuthTokenResponse) -> Result<CloudTokenSet, ServiceError> {
    if let Some(error) = response.error {
        return Err(ServiceError::Unavailable(format!(
            "cloud OAuth token exchange failed: {}",
            response
                .error_description
                .unwrap_or_else(|| error.chars().take(80).collect())
        )));
    }
    let expires_at = response
        .expires_in
        .filter(|seconds| *seconds > 0)
        .and_then(|seconds| Utc::now().checked_add_signed(ChronoDuration::seconds(seconds)));
    Ok(CloudTokenSet {
        access_token: response.access_token.ok_or_else(|| {
            ServiceError::Unavailable(
                "cloud OAuth token response did not include an access token".to_string(),
            )
        })?,
        refresh_token: response.refresh_token,
        token_type: response.token_type,
        scope: response.scope,
        expires_at,
    })
}

/// Builds a sealable token set from Supabase-brokered provider credentials on a
/// complete-cloud-link request. Returns `None` when no provider access token was
/// supplied so the caller falls back to the authorization-code exchange.
fn supabase_provider_token_set(req: &CompleteCloudLinkRequest) -> Option<CloudTokenSet> {
    let access_token = req
        .provider_access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let expires_at = req
        .provider_token_expires_in
        .filter(|seconds| *seconds > 0)
        .and_then(|seconds| Utc::now().checked_add_signed(ChronoDuration::seconds(seconds)));
    Some(CloudTokenSet {
        access_token,
        refresh_token: req
            .provider_refresh_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
        token_type: req
            .provider_token_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
        scope: req
            .provider_token_scope
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
        expires_at,
    })
}

fn sealed_envelope_from_connection(
    connection: &CloudConnectionRecord,
) -> Result<SealedTokenEnvelope, ServiceError> {
    Ok(SealedTokenEnvelope {
        ciphertext_b64: connection.token_ciphertext.clone().ok_or_else(|| {
            ServiceError::Unavailable("cloud connection is missing sealed credentials".to_string())
        })?,
        nonce_b64: connection.token_nonce.clone().ok_or_else(|| {
            ServiceError::Unavailable("cloud connection is missing credential nonce".to_string())
        })?,
        aad_tag: connection.token_aad.clone().ok_or_else(|| {
            ServiceError::Unavailable("cloud connection is missing credential aad".to_string())
        })?,
        version: connection.token_version.unwrap_or(1),
    })
}

/// Best-effort upstream grant revocation. `None` means the provider has no
/// app-scoped revocation endpoint (OneDrive) or is client-managed (iCloud).
/// `Some(false)` never blocks local disconnect: the sealed credentials are
/// still erased immediately after this bounded attempt.
async fn revoke_provider_authorization(
    state: &AppState,
    connection: &CloudConnectionRecord,
    endpoint_override: Option<&str>,
) -> Option<bool> {
    let provider = CloudProvider::parse(&connection.provider).ok()?;
    if !matches!(
        provider,
        CloudProvider::GoogleDrive | CloudProvider::Dropbox
    ) {
        return None;
    }
    let sealer = match state.cloud_sealer.as_ref() {
        Some(sealer) => sealer,
        None => return Some(false),
    };
    let envelope = match sealed_envelope_from_connection(connection) {
        Ok(envelope) => envelope,
        Err(_) => return Some(false),
    };
    let plaintext = match sealer.unseal(&connection.account_id, provider, &envelope) {
        Ok(plaintext) => plaintext,
        Err(_) => return Some(false),
    };
    let mut token_set = match serde_json::from_slice::<CloudTokenSet>(&plaintext) {
        Ok(token_set) => token_set,
        Err(_) => return Some(false),
    };

    if provider == CloudProvider::Dropbox
        && token_set
            .expires_at
            .is_some_and(|expiry| expiry <= Utc::now() + ChronoDuration::seconds(90))
    {
        if let Ok(refreshed) = refresh_access_token(state, provider, &token_set).await {
            token_set = refreshed;
        }
    }

    let endpoint = endpoint_override.unwrap_or(match provider {
        CloudProvider::GoogleDrive => GOOGLE_TOKEN_REVOCATION_URL,
        CloudProvider::Dropbox => DROPBOX_TOKEN_REVOCATION_URL,
        CloudProvider::MicrosoftOneDrive
        | CloudProvider::AppleICloud
        | CloudProvider::AmazonS3
        | CloudProvider::CloudflareR2 => return None,
    });
    let request = match provider {
        CloudProvider::GoogleDrive => {
            let token = token_set
                .refresh_token
                .as_deref()
                .unwrap_or(&token_set.access_token);
            state.http.post(endpoint).form(&[("token", token)])
        }
        CloudProvider::Dropbox => state
            .http
            .post(endpoint)
            .bearer_auth(&token_set.access_token),
        CloudProvider::MicrosoftOneDrive
        | CloudProvider::AppleICloud
        | CloudProvider::AmazonS3
        | CloudProvider::CloudflareR2 => return None,
    };
    match tokio::time::timeout(PROVIDER_REVOCATION_TIMEOUT, request.send()).await {
        Ok(Ok(response)) if response.status().is_success() => Some(true),
        Ok(Ok(response)) => {
            warn!(
                provider = provider.as_str(),
                status = response.status().as_u16(),
                "cloud provider authorization revocation returned non-success"
            );
            Some(false)
        }
        Ok(Err(error)) => {
            warn!(
                provider = provider.as_str(),
                error = %error,
                "cloud provider authorization revocation failed"
            );
            Some(false)
        }
        Err(_) => {
            warn!(
                provider = provider.as_str(),
                "cloud provider authorization revocation timed out"
            );
            Some(false)
        }
    }
}

fn destination_key(folder_path: &str, segment: &SegmentResponse) -> String {
    let file_name = segment
        .storage_key
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("segment.m4a");
    format!(
        "{}/device={}/session={}/{}",
        folder_path.trim_matches('/'),
        segment.device_id,
        segment.session_id,
        file_name
    )
}

fn initial_cloud_copy_status(provider: CloudProvider) -> &'static str {
    if provider.is_client_managed() {
        "waiting_client"
    } else {
        "pending"
    }
}

async fn exchange_authorization_code(
    state: &AppState,
    provider: CloudProvider,
    code: &str,
    redirect_uri: &str,
    code_verifier: Option<&str>,
) -> Result<CloudTokenSet, ServiceError> {
    let oauth = provider.oauth_config(&state.config).ok_or_else(|| {
        ServiceError::BadRequest("provider does not use server OAuth".to_string())
    })?;
    let client_id = oauth.client_id.as_deref().ok_or_else(|| {
        ServiceError::Unavailable(format!(
            "{} OAuth client id is not configured",
            provider.as_str()
        ))
    })?;
    let client_secret = oauth.client_secret.as_deref().ok_or_else(|| {
        ServiceError::Unavailable(format!(
            "{} OAuth client secret is not configured",
            provider.as_str()
        ))
    })?;
    let endpoint = oauth
        .token_url
        .as_deref()
        .or_else(|| provider.token_endpoint())
        .ok_or_else(|| {
            ServiceError::BadRequest("provider does not use server OAuth".to_string())
        })?;
    let mut params = vec![
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("grant_type", "authorization_code"),
    ];
    if let Some(verifier) = code_verifier {
        params.push(("code_verifier", verifier));
    }
    let response = state
        .http
        .post(endpoint)
        .form(&params)
        .send()
        .await
        .map_err(|err| {
            error!(error = %err, provider = provider.as_str(), "cloud OAuth token exchange request failed");
            ServiceError::Unavailable("cloud OAuth token exchange failed".to_string())
        })?;
    let status = response.status();
    let token_response = response.json::<OAuthTokenResponse>().await.map_err(|err| {
        error!(error = %err, provider = provider.as_str(), "cloud OAuth token response decode failed");
        ServiceError::Unavailable("cloud OAuth token response was invalid".to_string())
    })?;
    if !status.is_success() {
        return Err(ServiceError::Unavailable(format!(
            "cloud OAuth token exchange failed with status {}",
            status.as_u16()
        )));
    }
    token_set_from_response(token_response)
}

async fn refresh_access_token(
    state: &AppState,
    provider: CloudProvider,
    token_set: &CloudTokenSet,
) -> Result<CloudTokenSet, ServiceError> {
    let Some(refresh_token) = token_set.refresh_token.as_deref() else {
        return Ok(token_set.clone());
    };
    let Some(expires_at) = token_set.expires_at else {
        return Ok(token_set.clone());
    };
    let refresh_deadline = Utc::now()
        .checked_add_signed(ChronoDuration::seconds(90))
        .unwrap_or_else(Utc::now);
    if expires_at > refresh_deadline {
        return Ok(token_set.clone());
    }
    let oauth = provider.oauth_config(&state.config).ok_or_else(|| {
        ServiceError::BadRequest("provider does not use server OAuth".to_string())
    })?;
    let client_id = oauth.client_id.as_deref().ok_or_else(|| {
        ServiceError::Unavailable(format!(
            "{} OAuth client id is not configured",
            provider.as_str()
        ))
    })?;
    let client_secret = oauth.client_secret.as_deref().ok_or_else(|| {
        ServiceError::Unavailable(format!(
            "{} OAuth client secret is not configured",
            provider.as_str()
        ))
    })?;
    let endpoint = oauth
        .token_url
        .as_deref()
        .or_else(|| provider.token_endpoint())
        .ok_or_else(|| {
            ServiceError::BadRequest("provider does not use server OAuth".to_string())
        })?;
    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    let response = state
        .http
        .post(endpoint)
        .form(&params)
        .send()
        .await
        .map_err(|err| {
            error!(error = %err, provider = provider.as_str(), "cloud OAuth refresh request failed");
            ServiceError::Unavailable("cloud OAuth refresh failed".to_string())
        })?;
    let status = response.status();
    let token_response = response.json::<OAuthTokenResponse>().await.map_err(|err| {
        error!(error = %err, provider = provider.as_str(), "cloud OAuth refresh response decode failed");
        ServiceError::Unavailable("cloud OAuth refresh response was invalid".to_string())
    })?;
    if !status.is_success() {
        return Err(ServiceError::Unavailable(format!(
            "cloud OAuth refresh failed with status {}",
            status.as_u16()
        )));
    }
    let mut refreshed = token_set_from_response(token_response)?;
    if refreshed.refresh_token.is_none() {
        refreshed.refresh_token = token_set.refresh_token.clone();
    }
    Ok(refreshed)
}

async fn token_set_for_connection(
    state: &AppState,
    client: &DbClient,
    connection: &CloudConnectionRecord,
) -> Result<CloudTokenSet, ServiceError> {
    let provider = CloudProvider::parse(&connection.provider)?;
    let sealer = state.cloud_sealer.as_ref().ok_or_else(|| {
        ServiceError::Unavailable(
            "SOUND_RECORDER_CLOUD_TOKEN_ENCRYPTION_KEY is not configured".to_string(),
        )
    })?;
    let envelope = sealed_envelope_from_connection(connection)?;
    let plaintext = sealer.unseal(&connection.account_id, provider, &envelope)?;
    let token_set: CloudTokenSet = serde_json::from_slice(&plaintext)
        .map_err(|_| ServiceError::Internal("sealed cloud token payload is invalid".to_string()))?;
    let refreshed = refresh_access_token(state, provider, &token_set).await?;
    if refreshed.access_token != token_set.access_token
        || refreshed.expires_at != token_set.expires_at
        || refreshed.refresh_token != token_set.refresh_token
    {
        let sealed = sealer.seal(
            &connection.account_id,
            provider,
            &serde_json::to_vec(&refreshed)
                .map_err(|_| ServiceError::Internal("cloud token encode failed".to_string()))?,
        )?;
        client
            .execute(
                "update sound_recorder_cloud_connections
                 set token_ciphertext = $2,
                     token_nonce = $3,
                     token_aad = $4,
                     token_version = $5,
                     token_expires_at = $6,
                     updated_at = now()
                 where id = $1::uuid",
                &[
                    &connection.id,
                    &sealed.ciphertext_b64,
                    &sealed.nonce_b64,
                    &sealed.aad_tag,
                    &sealed.version,
                    &refreshed.expires_at,
                ],
            )
            .await
            .map_err(db_error)?;
    }
    Ok(refreshed)
}

#[allow(clippy::too_many_arguments)]
async fn upsert_cloud_connection(
    client: &DbClient,
    auth: &DeviceAuth,
    provider: CloudProvider,
    display_name: Option<String>,
    provider_account_id: Option<String>,
    root_folder_id: Option<String>,
    folder_path: String,
    oauth_scope: Option<String>,
    sealed: Option<SealedTokenEnvelope>,
    token_expires_at: Option<DateTime<Utc>>,
    meta_data: Value,
) -> Result<CloudConnectionRecord, ServiceError> {
    let provider_name = provider.as_str();
    let existing_id = if let Some(provider_account_id) = &provider_account_id {
        client
            .query_opt(
                "select id::text
                 from sound_recorder_cloud_connections
                 where account_id = $1::uuid
                   and provider = $2
                   and provider_account_id = $3
                   and status <> 'revoked'",
                &[&auth.account_id, &provider_name, provider_account_id],
            )
            .await
            .map_err(db_error)?
            .map(|row| row.get::<String>("id"))
    } else {
        None
    };
    let link_mode = provider.link_mode();
    let (token_ciphertext, token_nonce, token_aad, token_version) = sealed
        .map(|sealed| {
            (
                Some(sealed.ciphertext_b64),
                Some(sealed.nonce_b64),
                Some(sealed.aad_tag),
                Some(sealed.version),
            )
        })
        .unwrap_or((None, None, None, None));
    let row = if let Some(existing_id) = existing_id {
        client
            .query_one(
                "update sound_recorder_cloud_connections
                 set created_by_device_id = $2::uuid,
                     link_mode = $3,
                     status = 'active',
                     display_name = $4,
                     root_folder_id = $5,
                     folder_path = $6,
                     oauth_scope = $7,
                     token_ciphertext = $8,
                     token_nonce = $9,
                     token_aad = $10,
                     token_version = $11,
                     token_expires_at = $12,
                     meta_data = $13,
                     updated_at = now()
                 where id = $1::uuid
                 returning id::text, account_id::text, provider, link_mode, status, display_name,
                           provider_account_id, root_folder_id, folder_path, token_ciphertext,
                           token_nonce, token_aad, token_version, token_expires_at, last_sync_at,
                           created_at, updated_at",
                &[
                    &existing_id,
                    &auth.device_id,
                    &link_mode,
                    &display_name,
                    &root_folder_id,
                    &folder_path,
                    &oauth_scope,
                    &token_ciphertext,
                    &token_nonce,
                    &token_aad,
                    &token_version,
                    &token_expires_at,
                    &meta_data,
                ],
            )
            .await
            .map_err(db_error)?
    } else {
        let connection_id = Uuid::new_v4().to_string();
        let provider_subject_hash = provider_account_id
            .as_ref()
            .map(|value| hash_secret(value, "sound-recorder-cloud-subject"));
        client
            .query_one(
                "insert into sound_recorder_cloud_connections
                  (id, account_id, created_by_device_id, provider, link_mode, status,
                   display_name, provider_account_id, provider_subject_hash, root_folder_id,
                   folder_path, oauth_scope, token_ciphertext, token_nonce, token_aad,
                   token_version, token_expires_at, meta_data)
                 values
                  ($1::uuid, $2::uuid, $3::uuid, $4, $5, 'active',
                   $6, $7, $8, $9,
                   $10, $11, $12, $13, $14,
                   $15, $16, $17)
                 returning id::text, account_id::text, provider, link_mode, status, display_name,
                           provider_account_id, root_folder_id, folder_path, token_ciphertext,
                           token_nonce, token_aad, token_version, token_expires_at, last_sync_at,
                           created_at, updated_at",
                &[
                    &connection_id,
                    &auth.account_id,
                    &auth.device_id,
                    &provider_name,
                    &link_mode,
                    &display_name,
                    &provider_account_id,
                    &provider_subject_hash,
                    &root_folder_id,
                    &folder_path,
                    &oauth_scope,
                    &token_ciphertext,
                    &token_nonce,
                    &token_aad,
                    &token_version,
                    &token_expires_at,
                    &meta_data,
                ],
            )
            .await
            .map_err(db_error)?
    };
    Ok(cloud_connection_record_from_row(&row))
}

async fn enqueue_cloud_copy_job_for_segment(
    client: &DbClient,
    connection: &CloudConnectionRecord,
    segment: &SegmentResponse,
) -> Result<u64, ServiceError> {
    let provider = CloudProvider::parse(&connection.provider)?;
    if !provider.supports_copy_jobs() {
        return Ok(0);
    }
    let status = initial_cloud_copy_status(provider);
    let destination_key = destination_key(&connection.folder_path, segment);
    let job_id = Uuid::new_v4().to_string();
    client
        .execute(
            "insert into sound_recorder_cloud_copy_jobs
              (id, account_id, connection_id, segment_id, provider, status, destination_key)
             values
              ($1::uuid, $2::uuid, $3::uuid, $4::uuid, $5, $6, $7)
             on conflict (connection_id, segment_id) do nothing",
            &[
                &job_id,
                &segment.account_id,
                &connection.id,
                &segment.id,
                &connection.provider,
                &status,
                &destination_key,
            ],
        )
        .await
        .map_err(db_error)
}

async fn enqueue_cloud_copy_jobs_for_segment(
    client: &DbClient,
    config: &Config,
    account_id: &str,
    segment_row: &Row,
) -> Result<u64, ServiceError> {
    let segment = segment_from_row(config, segment_row);
    let rows = client
        .query(
            "select id::text, account_id::text, provider, link_mode, status, display_name,
                    provider_account_id, root_folder_id, folder_path, token_ciphertext,
                    token_nonce, token_aad, token_version, token_expires_at, last_sync_at,
                    created_at, updated_at
             from sound_recorder_cloud_connections
             where account_id = $1::uuid and status = 'active'",
            &[&account_id],
        )
        .await
        .map_err(db_error)?;
    let mut inserted = 0;
    for row in rows {
        let connection = cloud_connection_record_from_row(&row);
        inserted += enqueue_cloud_copy_job_for_segment(client, &connection, &segment).await?;
    }
    Ok(inserted)
}

async fn enqueue_retained_cloud_copy_jobs(
    client: &DbClient,
    config: &Config,
    account_id: &str,
    connection: &CloudConnectionRecord,
) -> Result<u64, ServiceError> {
    if config.cloud_backfill_segments <= 0 {
        return Ok(0);
    }
    let rows = client
        .query(
            "select id::text, account_id::text, device_id::text, session_id::text,
                    sequence_number, status, storage_provider, storage_bucket, storage_key,
                    content_type, codec, captured_started_at, captured_ended_at,
                    duration_millis, byte_count, sha256_hex, upload_url_expires_at,
                    uploaded_at, expires_at
             from sound_recorder_segments
             where account_id = $1::uuid
               and status = 'uploaded'
               and (pinned_at is not null or expires_at > now())
             order by captured_started_at desc
             limit $2",
            &[&account_id, &config.cloud_backfill_segments],
        )
        .await
        .map_err(db_error)?;
    let mut inserted = 0;
    for row in rows {
        let segment = segment_from_row(config, &row);
        inserted += enqueue_cloud_copy_job_for_segment(client, connection, &segment).await?;
    }
    Ok(inserted)
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(home))
        .route("/privacy", get(privacy))
        .route("/oauth/callback", get(cloud_oauth_callback))
        .route("/oauth/manual-callback", get(cloud_oauth_manual_callback))
        .route("/listen/:alert_id", get(listen_alert))
        .route("/download/ios", get(download_ios))
        .route("/download/android", get(download_android))
        .route("/api/v1/data/acoustic-events", get(list_acoustic_events))
        .route("/api/v1/data/user-consents", get(list_user_consents))
        .route("/api/v1/data/devices", get(list_devices))
        .route(
            "/api/v1/data/user-settings",
            get(get_user_settings).put(update_user_settings),
        )
        .route("/api/mobile/v1/account", delete(delete_account))
        .route("/api/mobile/v1/devices/register", post(register_device))
        .route("/api/mobile/v1/devices/heartbeat", post(heartbeat_device))
        .route(
            "/api/mobile/v1/devices/:install_id/revoke",
            post(revoke_device),
        )
        .route(
            "/api/mobile/v1/devices/presence",
            get(device_presence_upgrade),
        )
        .route(
            "/api/mobile/v1/devices/transfer-state",
            post(update_transfer_state),
        )
        .route(
            "/api/mobile/v1/upload-sessions",
            post(create_upload_session),
        )
        .route(
            "/api/mobile/v1/upload-sessions/:session_id/segments/presign",
            post(presign_segment),
        )
        .route(
            "/api/mobile/v1/upload-sessions/:session_id/segments/:segment_id/complete",
            post(complete_segment),
        )
        .route(
            "/api/mobile/v1/upload-sessions/:session_id/heartbeat",
            post(heartbeat_session),
        )
        .route(
            "/api/mobile/v1/upload-sessions/:session_id/close",
            post(close_session),
        )
        .route("/api/mobile/v1/timeline", get(timeline))
        .route(
            "/api/mobile/v1/evidence-exports",
            post(create_evidence_export),
        )
        .route(
            "/api/mobile/v1/permanent-saves",
            post(create_permanent_save),
        )
        .route("/api/mobile/v1/alerts", post(create_alert))
        .route(
            "/api/mobile/v1/cloud-connections",
            get(list_cloud_connections),
        )
        .route(
            "/api/mobile/v1/cloud-connections/oauth/start",
            post(start_cloud_link),
        )
        .route(
            "/api/mobile/v1/cloud-connections/oauth/complete",
            post(complete_cloud_link),
        )
        .route(
            "/api/mobile/v1/cloud-connections/:connection_id/revoke",
            post(revoke_cloud_connection),
        )
        .route(
            "/api/mobile/v1/cloud-copy-jobs",
            get(list_client_cloud_copy_jobs),
        )
        .route(
            "/api/mobile/v1/cloud-copy-jobs/:job_id/complete",
            post(complete_client_cloud_copy_job),
        )
        .route("/internal/retention/sweep", post(retention_sweep))
        .route("/internal/cloud-copy/drain", post(drain_cloud_copy_jobs))
        .route(
            "/internal/cloud-connection-projections/drain",
            post(drain_cloud_connection_projections),
        )
        .route("/internal/storage-mirror/drain", post(mirror_drain))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        // Explicit request-body ceiling for every route. All JSON payloads are
        // small; the largest legitimate one is a permanent-save batch (bounded
        // by MAX_PERMANENT_SAVE_SEGMENTS). Audio bytes never transit this process
        // — they go straight to S3 via presigned URLs — so 2 MiB is generous.
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(axum::middleware::from_fn(add_security_headers))
        // Inside the rate limiter, so a throttled request is refused without
        // starting a timer, and outside the handlers so the ceiling covers all
        // of them.
        .layer(axum::middleware::from_fn(request_timeout))
        // Outermost layer (added last → runs first): reject over-budget clients
        // before any handler, DB, or S3 work is done.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            rate_limit,
        ))
        .layer(axum::middleware::from_fn(observe_request))
        .with_state(state)
}

/// Creates one server span, one structured completion log, and OTLP metrics
/// for every request. Route templates are used instead of raw paths so ids do
/// not create unbounded labels in Prometheus.
async fn observe_request(req: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = req.method().to_string();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or("<unmatched>")
        .to_string();
    let span = tracing::info_span!(
        "http.server.request",
        http.request.method = %method,
        http.route = %route,
        http.response.status_code = field::Empty,
        otel.status_code = field::Empty,
    );
    let response = next.run(req).instrument(span.clone()).await;
    let status = response.status();
    let elapsed = started.elapsed();
    span.record("http.response.status_code", status.as_u16());
    span.record(
        "otel.status_code",
        if status.is_server_error() {
            "ERROR"
        } else {
            "OK"
        },
    );
    crate::telemetry::record_http_request(&method, &route, status.as_u16(), elapsed);
    info!(
        parent: &span,
        duration_ms = elapsed.as_secs_f64() * 1_000.0,
        "request completed"
    );
    response
}

/// Defense-in-depth response headers applied to every route. The API is JSON,
/// but a few routes serve HTML (home, privacy, API docs), so a strict CSP and
/// the usual hardening headers are worthwhile.
async fn add_security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    fn set(h: &mut HeaderMap, name: HeaderName, value: &'static str) {
        h.entry(name)
            .or_insert_with(|| HeaderValue::from_static(value));
    }
    set(h, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(h, header::X_FRAME_OPTIONS, "DENY");
    set(
        h,
        header::REFERRER_POLICY,
        "strict-origin-when-cross-origin",
    );
    set(
        h,
        header::STRICT_TRANSPORT_SECURITY,
        "max-age=63072000; includeSubDomains",
    );
    set(
        h,
        HeaderName::from_static("permissions-policy"),
        "geolocation=(), microphone=(), camera=()",
    );
    set(
        h,
        header::CONTENT_SECURITY_POLICY,
        "default-src 'none'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; \
         script-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
    );
    res
}

/// In-process fixed-window rate limiter, keyed per client. This is
/// defense-in-depth against burst abuse / cheap DoS on the auth-adjacent and
/// presign routes — it is intentionally simple (no external store) and so it is
/// per-replica: scale the configured budget down if you run many replicas, or
/// pair it with edge throttling at the ingress for a global cap.
struct RateLimiter {
    windows: Mutex<HashMap<String, RateWindow>>,
}

struct RateWindow {
    started: Instant,
    count: u32,
}

static RATE_LIMITER: Lazy<RateLimiter> = Lazy::new(RateLimiter::new);

impl RateLimiter {
    fn new() -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Records one request for `key`. Returns `Ok(())` when it is within budget,
    /// or `Err(retry_after_secs)` when the window's limit has been exceeded.
    fn check(&self, key: &str, limit: u32, window: Duration, now: Instant) -> Result<(), u64> {
        let mut guard = self.windows.lock().unwrap_or_else(|err| err.into_inner());
        // Opportunistically evict stale windows so a churn of distinct client
        // keys can't grow the map without bound.
        if guard.len() > 8192 {
            guard.retain(|_, w| now.duration_since(w.started) < window);
        }
        let entry = guard.entry(key.to_string()).or_insert(RateWindow {
            started: now,
            count: 0,
        });
        if now.duration_since(entry.started) >= window {
            entry.started = now;
            entry.count = 0;
        }
        entry.count = entry.count.saturating_add(1);
        if entry.count > limit {
            let elapsed = now.duration_since(entry.started);
            Err(window.saturating_sub(elapsed).as_secs().max(1))
        } else {
            Ok(())
        }
    }
}

/// Derives the rate-limit bucket key for a request: the first `X-Forwarded-For`
/// hop when we trust the proxy, otherwise the TCP peer address.
fn client_rate_key(req: &Request, trust_forwarded_for: bool) -> String {
    if trust_forwarded_for {
        if let Some(forwarded) = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
        {
            let first = forwarded.split(',').next().unwrap_or("").trim();
            if !first.is_empty() {
                return first.to_string();
            }
        }
    }
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// How long `path` may run, or `None` when it is exempt.
///
/// Health and metrics probes are exempt: they already answer from bounded probes
/// with their own timeouts, and a load balancer's probe must not be the thing
/// that reports a confusing 504.
fn request_budget(path: &str) -> Option<Duration> {
    match path {
        "/healthz" | "/readyz" | "/metrics" => None,
        path if path.starts_with("/internal/") => Some(INTERNAL_REQUEST_TIMEOUT),
        _ => Some(REQUEST_TIMEOUT),
    }
}

/// Bounds how long any one request may occupy a task.
///
/// Implemented with `tokio::time::timeout` rather than a tower layer to avoid
/// adding a dependency for it, and to match how the rest of this module bounds
/// slow work. Dropping the handler future cancels the work it was awaiting.
///
/// Health and metrics probes are exempt: they already answer from bounded probes
/// with their own timeouts, and a load balancer's probe must not be able to
/// produce a confusing 504.
async fn request_timeout(req: Request, next: Next) -> Response {
    let Some(budget) = request_budget(req.uri().path()) else {
        return next.run(req).await;
    };
    // Captured before the request is moved into the handler future.
    let method = req.method().clone();
    let route = req.uri().path().to_string();
    match tokio::time::timeout(budget, next.run(req)).await {
        Ok(response) => response,
        Err(_) => {
            tracing::error!(
                http.request.method = %method,
                http.route = %route,
                timeout_secs = budget.as_secs(),
                "request exceeded its time budget and was abandoned"
            );
            (
                StatusCode::GATEWAY_TIMEOUT,
                Json(json!({ "error": "request timed out" })),
            )
                .into_response()
        }
    }
}

/// Per-client rate-limit middleware. Health/metrics probes are exempt so a
/// busy load balancer can't be throttled, and the limiter is skipped entirely
/// when the configured budget is `0`.
async fn rate_limit(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let limit = state.config.rate_limit_per_minute;
    if limit == 0 || matches!(req.uri().path(), "/healthz" | "/readyz" | "/metrics") {
        return next.run(req).await;
    }
    let key = client_rate_key(&req, state.config.rate_limit_trust_forwarded_for);
    match RATE_LIMITER.check(&key, limit, RATE_LIMIT_WINDOW, Instant::now()) {
        Ok(()) => next.run(req).await,
        Err(retry_after) => {
            RATE_LIMITED.inc();
            let mut res = (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({ "error": "rate limit exceeded" })),
            )
                .into_response();
            res.headers_mut().insert(
                header::RETRY_AFTER,
                HeaderValue::from_str(&retry_after.to_string())
                    .unwrap_or_else(|_| HeaderValue::from_static("60")),
            );
            res
        }
    }
}

/// Parse this process's CLI flags into env-var overrides via flags2env
/// (`.cli-flags.toml`), applied before we read config so `--flag` and env both
/// work and flags win. Best-effort: if the native flags2env lib isn't installed
/// we log and fall back to plain env vars.
fn apply_cli_flags() {
    let f2e = match unsafe { flags2env::Flags2Env::load(None) } {
        Ok(f) => f,
        Err(cause) => {
            tracing::debug!(%cause, "flags2env native lib not loaded; using plain env");
            return;
        }
    };
    let argv: Vec<String> = std::env::args().collect();
    match f2e.parse(&argv, None) {
        Ok(overrides) => {
            for (key, value) in overrides {
                std::env::set_var(key, value);
            }
        }
        Err(cause) => warn!(%cause, "flags2env parse failed; using plain env"),
    }
}

pub async fn run() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    apply_cli_flags();
    let _telemetry_guard = crate::telemetry::init();

    let config = config_from_env();
    if !config.token_pepper_configured {
        warn!("SOUND_RECORDER_DEVICE_TOKEN_PEPPER is not configured; device tokens will not survive process restart");
    }
    if !config.shared_auth.is_enabled()
        && !config.supabase.is_enabled()
        && config.registration_bearer.is_none()
        && !config.allow_public_device_registration
    {
        warn!("device registration is disabled until registration auth is configured");
    }
    if config.require_supabase && !config.supabase.account_features_configured() {
        let mut missing = Vec::new();
        if config.supabase.url.is_none() {
            missing.push("SOUND_RECORDER_SUPABASE_URL");
        }
        if config.supabase.jwks_url.is_none() && config.supabase.jwt_secret.is_none() {
            missing.push("SOUND_RECORDER_SUPABASE_JWKS_URL");
        }
        if config.supabase.issuer.is_none() {
            missing.push("SOUND_RECORDER_SUPABASE_ISSUER");
        }
        if config.supabase.publishable_key.is_none() {
            missing.push("SOUND_RECORDER_SUPABASE_PUBLISHABLE_KEY");
        }
        if config.supabase.service_role_key.is_none() {
            missing.push("SOUND_RECORDER_SUPABASE_SERVICE_ROLE_KEY");
        }
        warn!(
            missing = ?missing,
            "strict Supabase readiness is enabled but account configuration is incomplete"
        );
    }
    let host = first_env(&["HOST"]).unwrap_or_else(|| "0.0.0.0".to_string());
    let port = env_u64("PORT", DEFAULT_PORT as u64).clamp(1, u16::MAX as u64) as u16;
    let state = state_from_config(config).await;

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("HOST/PORT must form a socket address");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind dd-sound-recorder-rs");
    info!("dd-sound-recorder-rs listening on http://{addr}");
    // `into_make_service_with_connect_info` surfaces the TCP peer address to the
    // rate-limit middleware (its fallback key when `X-Forwarded-For` is absent).
    axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("axum server crashed");
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    fn request_has_full_body(bytes: &[u8]) -> bool {
        let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            return false;
        };
        let header_text = String::from_utf8_lossy(&bytes[..header_end]);
        let content_length = header_text.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        });
        match content_length {
            Some(length) => bytes.len() >= header_end + 4 + length,
            None => true,
        }
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    request.extend_from_slice(&buf[..n]);
                    if request_has_full_body(&request) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        request
    }

    fn spawn_json_server(
        body: &'static str,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            tx.send(String::from_utf8_lossy(&request).to_string())
                .unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (format!("http://{addr}"), rx, handle)
    }

    fn spawn_google_resumable_server(
        body: &'static str,
        upload_chunks: usize,
    ) -> (String, mpsc::Receiver<Vec<String>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut requests = Vec::with_capacity(upload_chunks + 1);
            for index in 0..=upload_chunks {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                requests.push(String::from_utf8_lossy(&request).to_string());
                let response = if index == 0 {
                    format!(
                        "HTTP/1.1 200 OK\r\nlocation: http://{addr}/resumable/session-1\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    )
                } else if index < upload_chunks {
                    "HTTP/1.1 308 Resume Incomplete\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        .to_string()
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                };
                stream.write_all(response.as_bytes()).unwrap();
            }
            tx.send(requests).unwrap();
        });
        (format!("http://{addr}"), rx, handle)
    }

    fn spawn_dropbox_session_server(
        upload_chunks: usize,
    ) -> (String, mpsc::Receiver<Vec<String>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut requests = Vec::with_capacity(upload_chunks + 1);
            for index in 0..=upload_chunks {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                requests.push(String::from_utf8_lossy(&request).to_string());
                let body = if index == 0 {
                    r#"{"session_id":"dropbox-session-1"}"#
                } else if index < upload_chunks {
                    "{}"
                } else {
                    r#"{"id":"id:dropbox-session-file-1"}"#
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
            tx.send(requests).unwrap();
        });
        (format!("http://{addr}"), rx, handle)
    }

    fn test_config() -> Config {
        Config {
            validation_errors: Vec::new(),
            database_url: None,
            server_auth_secret: Some("test-server-secret".to_string()),
            token_pepper: "test-token-pepper".to_string(),
            token_pepper_configured: true,
            registration_bearer: None,
            allow_public_device_registration: true,
            s3: S3StorageConfig {
                bucket: "test-bucket".to_string(),
                key_prefix: "sound-recorder/segments".to_string(),
                cdn_base_url: None,
                region: "us-east-1".to_string(),
                endpoint: None,
                force_path_style: false,
                send_sse_aes256: true,
                max_attempts: DEFAULT_S3_MAX_ATTEMPTS,
                readiness_object_key: None,
                allow_signing_only_readiness: false,
                allow_unmarked_storage_history: false,
                backend_fingerprint: storage_backend_fingerprint(
                    ObjectStorageBackend::AmazonS3,
                    None,
                    "us-east-1",
                    "test-bucket",
                ),
                versioning_mode: "unversioned",
                access_key_id: None,
                secret_access_key: None,
                session_token: None,
                backend: ObjectStorageBackend::AmazonS3,
                validation_errors: Vec::new(),
            },
            mirror: S3StorageConfig {
                bucket: String::new(),
                key_prefix: "sound-recorder/segments".to_string(),
                cdn_base_url: None,
                region: "auto".to_string(),
                endpoint: None,
                force_path_style: false,
                send_sse_aes256: false,
                max_attempts: DEFAULT_S3_MAX_ATTEMPTS,
                readiness_object_key: None,
                allow_signing_only_readiness: false,
                allow_unmarked_storage_history: false,
                backend_fingerprint: String::new(),
                versioning_mode: "unversioned",
                access_key_id: None,
                secret_access_key: None,
                session_token: None,
                backend: ObjectStorageBackend::S3Compatible,
                validation_errors: Vec::new(),
            },
            mirror_readiness_required: false,
            mirror_batch_size: DEFAULT_MIRROR_BATCH_SIZE,
            mirror_copy_max_attempts: DEFAULT_MIRROR_COPY_MAX_ATTEMPTS,
            ios_app_store_url: None,
            android_play_store_url: None,
            default_retention_hours: DEFAULT_RETENTION_HOURS,
            upload_url_ttl: Duration::from_secs(DEFAULT_UPLOAD_URL_TTL_SECONDS),
            download_url_ttl: Duration::from_secs(DEFAULT_DOWNLOAD_URL_TTL_SECONDS),
            session_ttl_hours: DEFAULT_SESSION_TTL_HOURS,
            default_segment_seconds: DEFAULT_SEGMENT_SECONDS,
            max_segment_seconds: DEFAULT_MAX_SEGMENT_SECONDS,
            max_segment_bytes: DEFAULT_MAX_SEGMENT_BYTES,
            oauth_state_ttl: Duration::from_secs(DEFAULT_OAUTH_STATE_TTL_SECONDS),
            cloud_copy_batch_size: DEFAULT_CLOUD_COPY_BATCH_SIZE,
            cloud_copy_max_attempts: DEFAULT_CLOUD_COPY_MAX_ATTEMPTS,
            cloud_copy_max_bytes: DEFAULT_CLOUD_COPY_MAX_BYTES,
            cloud_backfill_segments: DEFAULT_CLOUD_BACKFILL_SEGMENTS,
            google_oauth: OAuthProviderConfig {
                client_id: Some("google-client".to_string()),
                client_secret: Some("google-secret".to_string()),
                authorization_url: None,
                token_url: None,
            },
            microsoft_oauth: OAuthProviderConfig {
                client_id: Some("microsoft-client".to_string()),
                client_secret: Some("microsoft-secret".to_string()),
                authorization_url: None,
                token_url: None,
            },
            dropbox_oauth: OAuthProviderConfig {
                client_id: Some("dropbox-client".to_string()),
                client_secret: Some("dropbox-secret".to_string()),
                authorization_url: None,
                token_url: None,
            },
            oauth_redirect_allowlist: Vec::new(),
            google_drive_upload_url: "https://www.googleapis.com/upload/drive/v3/files".to_string(),
            microsoft_graph_base_url: "https://graph.microsoft.com/v1.0".to_string(),
            dropbox_upload_url: "https://content.dropboxapi.com/2/files/upload".to_string(),
            public_base_url: Some("https://sound.example".to_string()),
            alert_email_to: "alerts@sound.example".to_string(),
            alert_email_webhook_url: None,
            rate_limit_per_minute: 0,
            rate_limit_trust_forwarded_for: true,
            require_supabase: false,
            supabase: SupabaseConfig::default(),
            shared_auth: SharedAuthConfig::default(),
        }
    }

    #[test]
    fn user_data_limits_are_bounded() {
        assert_eq!(user_data_limit(None), DEFAULT_USER_DATA_LIMIT);
        assert_eq!(user_data_limit(Some(0)), 1);
        assert_eq!(
            user_data_limit(Some(MAX_USER_DATA_LIMIT + 1)),
            MAX_USER_DATA_LIMIT
        );
    }

    #[test]
    fn probes_are_exempt_from_the_request_timeout() {
        // A probe must never be the thing that reports a 504 to a load balancer.
        for path in ["/healthz", "/readyz", "/metrics"] {
            assert_eq!(request_budget(path), None, "{path} should be exempt");
        }
    }

    #[test]
    fn client_routes_are_bounded_and_internal_drains_get_longer() {
        // Every client-facing route is bounded...
        assert_eq!(
            request_budget("/v1/segments/presign"),
            Some(REQUEST_TIMEOUT)
        );
        assert_eq!(request_budget("/"), Some(REQUEST_TIMEOUT));

        // ...while the drain endpoints, which walk a batch of objects each with
        // its own object-store timeout, would be cut short by that budget.
        assert_eq!(
            request_budget("/internal/cloud-copy/drain"),
            Some(INTERNAL_REQUEST_TIMEOUT)
        );
        assert!(INTERNAL_REQUEST_TIMEOUT > REQUEST_TIMEOUT);
        assert!(
            REQUEST_TIMEOUT > STORAGE_OBJECT_TIMEOUT,
            "a single object-store call must be able to finish inside the client budget"
        );
    }

    #[test]
    fn rate_limiter_allows_up_to_limit_then_rejects() {
        let limiter = RateLimiter::new();
        let now = Instant::now();
        let window = Duration::from_secs(60);
        // First three are within a budget of 3.
        assert!(limiter.check("1.2.3.4", 3, window, now).is_ok());
        assert!(limiter.check("1.2.3.4", 3, window, now).is_ok());
        assert!(limiter.check("1.2.3.4", 3, window, now).is_ok());
        // The fourth in the same window is rejected with a Retry-After.
        let retry = limiter.check("1.2.3.4", 3, window, now).unwrap_err();
        assert!((1..=60).contains(&retry));
        // A different client has its own independent budget.
        assert!(limiter.check("5.6.7.8", 3, window, now).is_ok());
    }

    #[test]
    fn rate_limiter_resets_after_window() {
        let limiter = RateLimiter::new();
        let now = Instant::now();
        let window = Duration::from_secs(60);
        assert!(limiter.check("9.9.9.9", 1, window, now).is_ok());
        assert!(limiter.check("9.9.9.9", 1, window, now).is_err());
        // Once the window has elapsed the counter resets.
        let later = now + Duration::from_secs(61);
        assert!(limiter.check("9.9.9.9", 1, window, later).is_ok());
    }

    fn test_state(config: Config) -> AppState {
        AppState {
            config: Arc::new(config),
            s3: None,
            mirror: None,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            cloud_sealer: None,
            supabase: None,
            pg_pool: None,
            storage_history_cache: Arc::new(RwLock::new(None)),
            storage_history_refresh_lock: Arc::new(AsyncMutex::new(())),
            device_presence: Arc::new(DevicePresenceHub::default()),
        }
    }

    fn test_segment() -> SegmentResponse {
        let now = Utc::now();
        SegmentResponse {
            id: Uuid::new_v4().to_string(),
            account_id: Uuid::new_v4().to_string(),
            device_id: Uuid::new_v4().to_string(),
            session_id: Uuid::new_v4().to_string(),
            sequence_number: 1,
            status: "uploaded".to_string(),
            storage_provider: "s3".to_string(),
            storage_bucket: "test-bucket".to_string(),
            storage_key: "sound-recorder/segments/device=dev/session=s/segment-0000000001.m4a"
                .to_string(),
            cdn_url: None,
            content_type: "audio/m4a".to_string(),
            codec: Some("aac".to_string()),
            captured_started_at: now,
            captured_ended_at: Some(now),
            duration_millis: 1000,
            byte_count: Some(4),
            sha256_hex: None,
            upload_url_expires_at: None,
            uploaded_at: Some(now),
            expires_at: now + ChronoDuration::hours(1),
        }
    }

    fn test_connection(provider: CloudProvider) -> CloudConnectionRecord {
        let now = Utc::now();
        CloudConnectionRecord {
            id: Uuid::new_v4().to_string(),
            account_id: Uuid::new_v4().to_string(),
            provider: provider.as_str().to_string(),
            link_mode: provider.link_mode().to_string(),
            status: "active".to_string(),
            display_name: Some("test.user.zdm@proton.me".to_string()),
            provider_account_id: Some("test.user.zdm@proton.me".to_string()),
            root_folder_id: None,
            folder_path: "sound-recorder".to_string(),
            token_ciphertext: None,
            token_nonce: None,
            token_aad: None,
            token_version: None,
            token_expires_at: None,
            last_sync_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn test_job(provider: CloudProvider, destination_key: &str) -> CloudCopyJobRecord {
        CloudCopyJobRecord {
            id: Uuid::new_v4().to_string(),
            provider: provider.as_str().to_string(),
            destination_key: destination_key.to_string(),
        }
    }

    fn test_token_set() -> CloudTokenSet {
        CloudTokenSet {
            access_token: "test-access-token".to_string(),
            refresh_token: Some("test-refresh-token".to_string()),
            token_type: Some("Bearer".to_string()),
            scope: None,
            expires_at: Some(Utc::now() + ChronoDuration::minutes(15)),
        }
    }

    #[test]
    fn provider_aliases_normalize() {
        assert_eq!(
            CloudProvider::parse("google").unwrap().as_str(),
            "google_drive"
        );
        assert_eq!(
            CloudProvider::parse("onedrive").unwrap().as_str(),
            "microsoft_onedrive"
        );
        assert_eq!(
            CloudProvider::parse("icloud").unwrap().link_mode(),
            "client_managed"
        );
        assert_eq!(
            CloudProvider::parse("drop_box").unwrap().as_str(),
            "dropbox"
        );
        assert_eq!(CloudProvider::parse("s3").unwrap().as_str(), "amazon_s3");
        assert_eq!(
            CloudProvider::parse("r2").unwrap().as_str(),
            "cloudflare_r2"
        );
        assert!(!CloudProvider::AmazonS3.supports_copy_jobs());
        assert!(!CloudProvider::CloudflareR2.supports_copy_jobs());
    }

    #[test]
    fn desktop_platforms_register_with_the_backend() {
        for platform in ["ios", "android", "macos", "windows", "linux"] {
            assert_eq!(normalize_platform(platform).unwrap(), platform);
        }
        assert!(normalize_platform("web").is_err());
    }

    #[test]
    fn cloud_projection_contains_status_but_never_credentials_or_provider_subjects() {
        let connection = test_connection(CloudProvider::GoogleDrive);
        let user_id = Uuid::new_v4().to_string();
        let payload = cloud_connection_projection_payload(&user_id, &connection);
        let object = payload.as_object().unwrap();

        assert_eq!(
            object.get("user_id").and_then(Value::as_str),
            Some(user_id.as_str())
        );
        assert_eq!(
            object.get("provider").and_then(Value::as_str),
            Some("google_drive")
        );
        for forbidden in [
            "provider_account_id",
            "root_folder_id",
            "oauth_scope",
            "token_ciphertext",
            "token_nonce",
            "token_aad",
            "token_version",
            "meta_data",
            "access_key_id",
            "secret_access_key",
        ] {
            assert!(
                !object.contains_key(forbidden),
                "projection leaked {forbidden}"
            );
        }
    }

    #[test]
    fn only_namespaced_supabase_subjects_project_to_supabase_users() {
        let user_id = Uuid::new_v4().to_string();
        assert_eq!(
            supabase_user_id_from_external_subject(&format!("supabase:{user_id}")),
            Some(user_id)
        );
        assert!(supabase_user_id_from_external_subject("anonymous:abc").is_none());
        assert!(supabase_user_id_from_external_subject("supabase:not-a-uuid").is_none());
    }

    #[test]
    fn cloud_oauth_pkce_is_url_safe_and_matches_its_verifier() {
        let pkce = new_oauth_pkce();

        assert_eq!(pkce.verifier.len(), 43);
        assert_eq!(pkce.challenge.len(), 43);
        assert!(pkce
            .verifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')));
        assert_eq!(
            pkce.challenge,
            URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.verifier.as_bytes()))
        );
    }

    #[test]
    fn provider_authorization_urls_request_offline_access_and_pkce() {
        let config = test_config();
        let redirect_uri = "https://api.sonusauris.app/oauth/callback";
        let challenge = "test-pkce-challenge";

        let google = reqwest::Url::parse(
            &authorization_url(
                CloudProvider::GoogleDrive,
                &config.google_oauth,
                redirect_uri,
                "google-state",
                Some(challenge),
            )
            .unwrap(),
        )
        .unwrap();
        let google_query = google.query_pairs().collect::<HashMap<_, _>>();
        assert_eq!(
            google_query.get("access_type").map(|v| v.as_ref()),
            Some("offline")
        );
        assert_eq!(
            google_query.get("prompt").map(|v| v.as_ref()),
            Some("consent")
        );
        assert_eq!(
            google_query
                .get("code_challenge_method")
                .map(|v| v.as_ref()),
            Some("S256")
        );
        assert_eq!(
            google_query.get("code_challenge").map(|v| v.as_ref()),
            Some(challenge)
        );

        let microsoft = reqwest::Url::parse(
            &authorization_url(
                CloudProvider::MicrosoftOneDrive,
                &config.microsoft_oauth,
                redirect_uri,
                "microsoft-state",
                Some(challenge),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(microsoft.host_str(), Some("login.microsoftonline.com"));
        assert!(microsoft.path().starts_with("/common/"));
        let microsoft_query = microsoft.query_pairs().collect::<HashMap<_, _>>();
        assert!(microsoft_query
            .get("scope")
            .is_some_and(|scope| scope.contains("offline_access")));
        assert!(microsoft_query
            .get("scope")
            .is_some_and(|scope| scope.contains("Files.ReadWrite.AppFolder")));

        let dropbox = reqwest::Url::parse(
            &authorization_url(
                CloudProvider::Dropbox,
                &config.dropbox_oauth,
                redirect_uri,
                "dropbox-state",
                Some(challenge),
            )
            .unwrap(),
        )
        .unwrap();
        let dropbox_query = dropbox.query_pairs().collect::<HashMap<_, _>>();
        assert_eq!(
            dropbox_query.get("token_access_type").map(|v| v.as_ref()),
            Some("offline")
        );
        assert_eq!(
            dropbox_query.get("code_challenge").map(|v| v.as_ref()),
            Some(challenge)
        );
    }

    #[tokio::test]
    async fn authorization_code_exchange_proves_the_pkce_verifier() {
        let (base_url, rx, handle) = spawn_json_server(
            r#"{"access_token":"linked-access","refresh_token":"linked-refresh","expires_in":3600,"token_type":"Bearer"}"#,
        );
        let mut config = test_config();
        config.google_oauth.token_url = Some(format!("{base_url}/oauth/token"));
        let state = test_state(config);

        let tokens = exchange_authorization_code(
            &state,
            CloudProvider::GoogleDrive,
            "provider-code",
            "https://api.sonusauris.app/oauth/callback",
            Some("server-held-verifier"),
        )
        .await
        .unwrap();

        assert_eq!(tokens.access_token, "linked-access");
        assert_eq!(tokens.refresh_token.as_deref(), Some("linked-refresh"));
        let request = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
        assert!(request.starts_with("POST /oauth/token HTTP/1.1"));
        assert!(request.contains("code=provider-code"));
        assert!(request.contains("code_verifier=server-held-verifier"));
        assert!(request.contains("grant_type=authorization_code"));
    }

    fn seal_test_connection(
        state: &mut AppState,
        provider: CloudProvider,
    ) -> CloudConnectionRecord {
        let sealer = CloudTokenSealer::from_base64_key(&BASE64_STANDARD.encode([7u8; 32])).unwrap();
        let mut connection = test_connection(provider);
        let sealed = sealer
            .seal(
                &connection.account_id,
                provider,
                &serde_json::to_vec(&test_token_set()).unwrap(),
            )
            .unwrap();
        connection.token_ciphertext = Some(sealed.ciphertext_b64);
        connection.token_nonce = Some(sealed.nonce_b64);
        connection.token_aad = Some(sealed.aad_tag);
        connection.token_version = Some(sealed.version);
        state.cloud_sealer = Some(sealer);
        connection
    }

    #[tokio::test]
    async fn disconnect_revokes_google_and_dropbox_authorizations_upstream() {
        let (google_url, google_rx, google_handle) = spawn_json_server("{}");
        let mut google_state = test_state(test_config());
        let google_connection = seal_test_connection(&mut google_state, CloudProvider::GoogleDrive);

        assert_eq!(
            revoke_provider_authorization(&google_state, &google_connection, Some(&google_url),)
                .await,
            Some(true)
        );
        let google_request = google_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        google_handle.join().unwrap();
        assert!(google_request.starts_with("POST / HTTP/1.1"));
        assert!(google_request.contains("token=test-refresh-token"));

        let (dropbox_url, dropbox_rx, dropbox_handle) = spawn_json_server("{}");
        let mut dropbox_state = test_state(test_config());
        let dropbox_connection = seal_test_connection(&mut dropbox_state, CloudProvider::Dropbox);

        assert_eq!(
            revoke_provider_authorization(&dropbox_state, &dropbox_connection, Some(&dropbox_url),)
                .await,
            Some(true)
        );
        let dropbox_request = dropbox_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        dropbox_handle.join().unwrap();
        assert!(dropbox_request.starts_with("POST / HTTP/1.1"));
        assert!(dropbox_request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-access-token"));
    }

    #[tokio::test]
    async fn onedrive_disconnect_is_local_without_revoking_all_microsoft_sessions() {
        let state = test_state(test_config());
        let connection = test_connection(CloudProvider::MicrosoftOneDrive);

        assert_eq!(
            revoke_provider_authorization(&state, &connection, None).await,
            None
        );
    }

    #[test]
    fn folder_path_rejects_unsafe_paths() {
        assert!(validate_folder_path(Some("../x".to_string())).is_err());
        assert!(validate_folder_path(Some("/absolute".to_string())).is_err());
        assert!(validate_folder_path(Some("sound-recorder\\bad".to_string())).is_err());
        assert_eq!(
            validate_folder_path(Some("sound-recorder/day".to_string())).unwrap(),
            "sound-recorder/day"
        );
    }

    #[test]
    fn query_escape_encodes_reserved_bytes() {
        assert_eq!(query_escape("a b/c?d"), "a%20b%2Fc%3Fd");
        assert_eq!(graph_path_escape("a b/c?d"), "a%20b/c%3Fd");
    }

    #[test]
    fn google_drive_file_name_keeps_destination_context() {
        assert_eq!(
            google_drive_file_name("sound-recorder/device=dev/session=s/segment-0000000001.m4a"),
            "sound-recorder__device=dev__session=s__segment-0000000001.m4a"
        );
        assert_eq!(google_drive_file_name("/"), "segment.m4a");
    }

    #[tokio::test]
    async fn google_drive_upload_uses_a_resumable_session_and_byte_ranges() {
        let (base_url, rx, handle) = spawn_google_resumable_server(r#"{"id":"google-file-1"}"#, 2);
        let mut config = test_config();
        config.google_drive_upload_url = format!("{base_url}/upload/drive/v3/files");
        let state = test_state(config);
        let mut connection = test_connection(CloudProvider::GoogleDrive);
        connection.root_folder_id = Some("drive-root-folder".to_string());
        let segment = test_segment();
        let job = test_job(
            CloudProvider::GoogleDrive,
            "sound-recorder/device=dev/session=s/segment-0000000001.m4a",
        );
        let file_id = upload_to_google_drive_in_chunks(
            &state,
            &connection,
            &segment,
            &job,
            b"ping".to_vec(),
            &test_token_set(),
            3,
        )
        .await
        .unwrap();
        assert_eq!(file_id, "google-file-1");
        let requests = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
        assert_eq!(requests.len(), 3);
        let start = &requests[0];
        let start_lower = start.to_ascii_lowercase();
        assert!(start.starts_with(
            "POST /upload/drive/v3/files?uploadType=resumable&fields=id,name,webViewLink HTTP/1.1"
        ));
        assert!(start_lower.contains("authorization: bearer test-access-token"));
        assert!(start_lower.contains("x-upload-content-length: 4"));
        assert!(start.contains("drive-root-folder"));
        assert!(start.contains("sound-recorder__device=dev__session=s__segment-0000000001.m4a"));

        let first_chunk = &requests[1];
        let second_chunk = &requests[2];
        assert!(first_chunk.starts_with("PUT /resumable/session-1 HTTP/1.1"));
        assert!(first_chunk
            .to_ascii_lowercase()
            .contains("content-range: bytes 0-2/4"));
        assert!(first_chunk.ends_with("pin"));
        assert!(second_chunk.starts_with("PUT /resumable/session-1 HTTP/1.1"));
        assert!(second_chunk
            .to_ascii_lowercase()
            .contains("content-range: bytes 3-3/4"));
        assert!(second_chunk.ends_with("g"));
        assert!(
            !first_chunk
                .to_ascii_lowercase()
                .contains("authorization: bearer"),
            "the bearer token must not be forwarded to the session URL"
        );
    }

    #[test]
    fn google_resumable_chunks_satisfy_the_provider_alignment() {
        assert_eq!(GOOGLE_RESUMABLE_CHUNK_BYTES % (256 * 1024), 0);
    }

    #[tokio::test]
    async fn microsoft_onedrive_upload_hits_configured_endpoint() {
        let (base_url, rx, handle) = spawn_json_server(r#"{"id":"onedrive-file-1"}"#);
        let mut config = test_config();
        config.microsoft_graph_base_url = base_url;
        let state = test_state(config);
        let segment = test_segment();
        let job = test_job(
            CloudProvider::MicrosoftOneDrive,
            "sound-recorder/device=dev/session=s/segment 0000000001.m4a",
        );
        let file_id = upload_to_microsoft_onedrive(
            &state,
            &segment,
            &job,
            b"ping".to_vec(),
            &test_token_set(),
        )
        .await
        .unwrap();
        assert_eq!(file_id, "onedrive-file-1");
        let request = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
        let request_lower = request.to_ascii_lowercase();
        assert!(request.starts_with(
            "PUT /me/drive/special/approot:/sound-recorder/device%3Ddev/session%3Ds/segment%200000000001.m4a:/content HTTP/1.1"
        ));
        assert!(request_lower.contains("authorization: bearer test-access-token"));
        assert!(request_lower.contains("content-type: audio/m4a"));
        assert!(request.contains("ping"));
    }

    #[tokio::test]
    async fn dropbox_upload_hits_configured_endpoint() {
        let (base_url, rx, handle) = spawn_json_server(
            r#"{"id":"id:dropbox-file-1","path_display":"/sound-recorder/a.m4a"}"#,
        );
        let mut config = test_config();
        config.dropbox_upload_url = format!("{base_url}/2/files/upload");
        let state = test_state(config);
        let segment = test_segment();
        let job = test_job(CloudProvider::Dropbox, "sound-recorder/a.m4a");
        let file_id =
            upload_to_dropbox(&state, &segment, &job, b"ping".to_vec(), &test_token_set())
                .await
                .unwrap();
        assert_eq!(file_id, "id:dropbox-file-1");
        let request = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
        let request_lower = request.to_ascii_lowercase();
        assert!(request.starts_with("POST /2/files/upload HTTP/1.1"));
        assert!(request_lower.contains("authorization: bearer test-access-token"));
        assert!(request_lower.contains("dropbox-api-arg:"));
        assert!(
            request.contains("\\/sound-recorder\\/a.m4a")
                || request.contains("/sound-recorder/a.m4a")
        );
        assert!(request.contains("ping"));
    }

    #[tokio::test]
    async fn dropbox_large_upload_uses_a_chunked_session() {
        let (base_url, rx, handle) = spawn_dropbox_session_server(2);
        let mut config = test_config();
        config.dropbox_upload_url = format!("{base_url}/2/files/upload");
        let state = test_state(config);
        let segment = test_segment();
        let job = test_job(CloudProvider::Dropbox, "sound-recorder/a.m4a");
        let file_id = upload_to_dropbox_session(
            &state,
            &segment,
            &job,
            b"ping".to_vec(),
            &test_token_set(),
            3,
        )
        .await
        .unwrap();

        assert_eq!(file_id, "id:dropbox-session-file-1");
        let requests = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[0].starts_with("POST /2/files/upload_session/start HTTP/1.1"));
        assert!(requests[0].contains("\"close\":false"));
        assert!(requests[1].starts_with("POST /2/files/upload_session/append_v2 HTTP/1.1"));
        assert!(requests[1].contains("\"offset\":0"));
        assert!(requests[1].ends_with("pin"));
        assert!(requests[2].starts_with("POST /2/files/upload_session/finish HTTP/1.1"));
        assert!(requests[2].contains("\"offset\":3"));
        assert!(requests[2].contains("\"path\":\"/sound-recorder/a.m4a\""));
        assert!(requests[2].ends_with("g"));
    }

    #[test]
    fn dropbox_switches_to_sessions_above_the_single_call_limit() {
        assert!(DROPBOX_SINGLE_UPLOAD_MAX_BYTES < MAX_CLOUD_COPY_MAX_BYTES as usize);
        assert_eq!(DROPBOX_SESSION_CHUNK_BYTES % (4 * 1024 * 1024), 0);
    }

    #[test]
    fn apple_icloud_copy_jobs_are_client_managed() {
        assert_eq!(CloudProvider::AppleICloud.link_mode(), "client_managed");
        assert!(!CloudProvider::AppleICloud.is_server_managed());
        assert_eq!(
            initial_cloud_copy_status(CloudProvider::AppleICloud),
            "waiting_client"
        );
        assert_eq!(
            initial_cloud_copy_status(CloudProvider::GoogleDrive),
            "pending"
        );
    }

    fn registration_request(external_subject: Option<&str>) -> RegisterDeviceRequest {
        RegisterDeviceRequest {
            platform: "ios".to_string(),
            install_id: "install-123".to_string(),
            device_label: None,
            app_version: None,
            os_version: None,
            external_subject: external_subject.map(ToString::to_string),
            display_name: None,
            legal_region: None,
            consent_version: "v1".to_string(),
            consent_accepted_at: None,
            recording_indicator_acknowledged: true,
            attestation: None,
        }
    }

    #[test]
    fn use_case_validation_defaults_and_rejects() {
        assert_eq!(validate_use_case(None).unwrap(), "security");
        assert_eq!(
            validate_use_case(Some("Music".to_string())).unwrap(),
            "music"
        );
        assert!(validate_use_case(Some("karaoke".to_string())).is_err());
    }

    #[test]
    fn supabase_identity_is_namespaced() {
        let identity = SupabaseIdentity {
            subject: "abc-123".to_string(),
            email: Some("a@b.co".to_string()),
        };
        assert_eq!(identity.external_subject(), "supabase:abc-123");
    }

    #[test]
    fn shared_auth_identity_is_namespaced() {
        let identity = SharedAuthIdentity {
            subject: "7bbbfce1-d3b0-41e3-ab93-2e4f4e62ba89".to_string(),
            email: Some("a@b.co".to_string()),
        };
        assert_eq!(
            identity.external_subject(),
            "shared-auth:7bbbfce1-d3b0-41e3-ab93-2e4f4e62ba89"
        );
    }

    #[test]
    fn shared_auth_url_accepts_https_and_cluster_http_only() {
        assert!(validate_shared_auth_url("https://auth.oresoftware.dev"));
        assert!(validate_shared_auth_url(
            "http://dd-shared-auth.dd.svc.cluster.local:8120"
        ));
        assert!(validate_shared_auth_url("http://127.0.0.1:8120"));
        for invalid in [
            "http://auth.example.com",
            "https://user:pass@auth.example.com",
            "https://auth.example.com/path",
            "https://auth.example.com?redirect=elsewhere",
        ] {
            assert!(!validate_shared_auth_url(invalid), "{invalid}");
        }
    }

    #[tokio::test]
    async fn shared_auth_introspection_requires_active_aal2_identity() {
        let issued_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let body: &'static str = Box::leak(
            format!(
                r#"{{"active":true,"sub":"7bbbfce1-d3b0-41e3-ab93-2e4f4e62ba89","email":"verified@example.com","email_verified":true,"aal":2,"acr":"urn:oresoftware:loa:2","amr":["federated","totp"],"iat":{issued_at}}}"#
            )
            .into_boxed_str(),
        );
        let (base_url, requests, handle) = spawn_json_server(body);
        let mut config = test_config();
        config.shared_auth = SharedAuthConfig {
            base_url: Some(base_url),
            introspect_secret: Some("shared-auth-test-introspection-secret-32-bytes".to_string()),
            required_aal: 2,
            validation_errors: Vec::new(),
        };
        let state = test_state(config);
        let identity = introspect_shared_auth(&state, "signed.shared.auth-token")
            .await
            .unwrap();
        assert_eq!(identity.subject, "7bbbfce1-d3b0-41e3-ab93-2e4f4e62ba89");
        assert_eq!(identity.email.as_deref(), Some("verified@example.com"));
        let request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(request.starts_with("POST /auth/introspect HTTP/1.1"));
        assert!(request.lines().any(|line| {
            line.eq_ignore_ascii_case(
                "authorization: Bearer shared-auth-test-introspection-secret-32-bytes",
            )
        }));
        assert!(request.contains(r#""token":"signed.shared.auth-token""#));
        handle.join().unwrap();

        let aal1 = r#"{"active":true,"sub":"7bbbfce1-d3b0-41e3-ab93-2e4f4e62ba89","email_verified":true,"aal":1}"#;
        let (base_url, _requests, handle) = spawn_json_server(aal1);
        let mut config = test_config();
        config.shared_auth = SharedAuthConfig {
            base_url: Some(base_url),
            introspect_secret: Some("shared-auth-test-introspection-secret-32-bytes".to_string()),
            required_aal: 2,
            validation_errors: Vec::new(),
        };
        assert!(matches!(
            introspect_shared_auth(&test_state(config), "signed.shared.auth-token").await,
            Err(ServiceError::MfaRequired)
        ));
        handle.join().unwrap();
    }

    #[test]
    fn public_registration_ignores_client_subject() {
        // The takeover fix: an unauthenticated caller cannot claim another
        // account by asserting its externalSubject.
        let req = registration_request(Some("supabase:victim"));
        let (subject, _) =
            resolve_registration_subject(&RegistrationTrust::Public, &req, "install-123").unwrap();
        assert_eq!(subject.as_deref(), Some("install:install-123"));
    }

    #[test]
    fn supabase_registration_uses_verified_subject() {
        let req = registration_request(Some("supabase:victim"));
        let trust = RegistrationTrust::Supabase(SupabaseIdentity {
            subject: "real-user".to_string(),
            email: Some("real@user.co".to_string()),
        });
        let (subject, display_name) =
            resolve_registration_subject(&trust, &req, "install-123").unwrap();
        assert_eq!(subject.as_deref(), Some("supabase:real-user"));
        assert_eq!(display_name.as_deref(), Some("real@user.co"));
    }

    #[test]
    fn shared_auth_registration_uses_verified_subject() {
        let req = registration_request(Some("shared-auth:victim"));
        let trust = RegistrationTrust::SharedAuth(SharedAuthIdentity {
            subject: "7bbbfce1-d3b0-41e3-ab93-2e4f4e62ba89".to_string(),
            email: Some("real@user.co".to_string()),
        });
        let (subject, display_name) =
            resolve_registration_subject(&trust, &req, "install-123").unwrap();
        assert_eq!(
            subject.as_deref(),
            Some("shared-auth:7bbbfce1-d3b0-41e3-ab93-2e4f4e62ba89")
        );
        assert_eq!(display_name.as_deref(), Some("real@user.co"));
    }

    #[test]
    fn trusted_server_registration_passes_subject_through() {
        let req = registration_request(Some("partner-tenant-7"));
        let (subject, _) =
            resolve_registration_subject(&RegistrationTrust::TrustedServer, &req, "install-123")
                .unwrap();
        assert_eq!(subject.as_deref(), Some("partner-tenant-7"));
    }

    #[test]
    fn supabase_provider_token_set_requires_access_token() {
        let mut req = CompleteCloudLinkRequest {
            provider: "google_drive".to_string(),
            state: "state".to_string(),
            authorization_code: None,
            redirect_uri: None,
            display_name: None,
            provider_account_id: None,
            root_folder_id: None,
            folder_path: None,
            client_managed_acknowledged: None,
            provider_access_token: None,
            provider_refresh_token: Some("refresh".to_string()),
            provider_token_expires_in: Some(3600),
            provider_token_type: Some("Bearer".to_string()),
            provider_token_scope: None,
            meta_data: None,
        };
        assert!(supabase_provider_token_set(&req).is_none());
        req.provider_access_token = Some("access".to_string());
        let token_set = supabase_provider_token_set(&req).unwrap();
        assert_eq!(token_set.access_token, "access");
        assert_eq!(token_set.refresh_token.as_deref(), Some("refresh"));
        assert!(token_set.expires_at.is_some());
    }

    #[test]
    fn metadata_has_size_limit() {
        let oversized = json!({ "x": "a".repeat(MAX_META_BYTES + 1) });
        assert!(validate_meta(Some(oversized)).is_err());
        assert!(validate_meta(Some(json!({ "ok": true }))).is_ok());
    }

    #[test]
    fn public_url_allowlist_requires_real_https_or_loopback() {
        assert!(is_safe_public_url("https://sound.example/listen/abc"));
        assert!(is_safe_public_url("http://localhost:8126/listen/abc"));
        assert!(is_safe_public_url("http://127.0.0.1:8126/listen/abc"));
        assert!(!is_safe_public_url(
            "http://localhost.evil.example/listen/abc"
        ));
        assert!(!is_safe_public_url("http://sound.example/listen/abc"));
        assert!(!is_safe_public_url("javascript:alert(1)"));
    }

    #[test]
    fn redirect_uri_rejects_lookalike_loopback() {
        let provider = CloudProvider::GoogleDrive;
        let any: &[String] = &[];
        assert!(validate_redirect_uri(
            provider,
            Some("https://app.example/oauth".to_string()),
            any
        )
        .is_ok());
        assert!(
            validate_redirect_uri(provider, Some("http://localhost:8080/cb".to_string()), any)
                .is_ok()
        );
        // The pre-fix prefix match accepted these lookalike hosts.
        assert!(validate_redirect_uri(
            provider,
            Some("http://localhost.evil.example/cb".to_string()),
            any
        )
        .is_err());
        assert!(validate_redirect_uri(
            provider,
            Some("http://127.0.0.1.evil.example/cb".to_string()),
            any
        )
        .is_err());
        assert!(
            validate_redirect_uri(provider, Some("http://example.com/cb".to_string()), any)
                .is_err()
        );
    }

    #[test]
    fn redirect_uri_allowlist_pins_to_known_callbacks() {
        let provider = CloudProvider::GoogleDrive;
        let allow = vec![
            "https://app.sonusauris.com/oauth/callback".to_string(),
            "https://app.sonusauris.com/oauth/onedrive".to_string(),
        ];
        // An allowed exact match passes.
        assert!(validate_redirect_uri(
            provider,
            Some("https://app.sonusauris.com/oauth/callback".to_string()),
            &allow,
        )
        .is_ok());
        // A different https host that would pass is_safe_public_url is now
        // rejected because it is not in the allow-list (open-redirect defense).
        assert!(validate_redirect_uri(
            provider,
            Some("https://attacker.example/oauth/callback".to_string()),
            &allow,
        )
        .is_err());
        // A path/case variant of an allowed entry is not an exact match.
        assert!(validate_redirect_uri(
            provider,
            Some("https://app.sonusauris.com/oauth/callback/extra".to_string()),
            &allow,
        )
        .is_err());
        // iCloud is client-managed and bypasses the URL allow-list entirely.
        assert!(validate_redirect_uri(CloudProvider::AppleICloud, None, &allow).is_ok());
    }

    #[test]
    fn hosted_oauth_callback_returns_encoded_code_and_state_to_the_app() {
        let target = cloud_oauth_app_callback(CloudOAuthCallbackQuery {
            code: Some("code with + & ?".to_string()),
            state: Some("sr_oauth_state".to_string()),
            ..Default::default()
        })
        .unwrap();
        let uri = reqwest::Url::parse(&target).unwrap();

        assert_eq!(uri.scheme(), "sonusauris");
        assert_eq!(uri.host_str(), Some("oauth"));
        assert_eq!(uri.path(), "/callback");
        let params = uri.query_pairs().collect::<HashMap<_, _>>();
        assert_eq!(
            params.get("state").map(|value| value.as_ref()),
            Some("sr_oauth_state")
        );
        assert_eq!(
            params.get("code").map(|value| value.as_ref()),
            Some("code with + & ?")
        );
    }

    #[test]
    fn hosted_oauth_callback_forwards_provider_errors_without_a_code() {
        let target = cloud_oauth_app_callback(CloudOAuthCallbackQuery {
            state: Some("sr_oauth_state".to_string()),
            error: Some("access_denied".to_string()),
            error_description: Some("The user cancelled.".to_string()),
            ..Default::default()
        })
        .unwrap();
        let uri = reqwest::Url::parse(&target).unwrap();
        let params = uri.query_pairs().collect::<HashMap<_, _>>();

        assert_eq!(
            params.get("error").map(|value| value.as_ref()),
            Some("access_denied")
        );
        assert_eq!(
            params.get("error_description").map(|value| value.as_ref()),
            Some("The user cancelled.")
        );
        assert!(!params.contains_key("code"));
    }

    #[test]
    fn manual_oauth_callback_displays_only_escaped_one_time_code() {
        let page = cloud_oauth_manual_page(CloudOAuthCallbackQuery {
            code: Some("<code>&secret".to_string()),
            state: Some("sr_oauth_state".to_string()),
            ..Default::default()
        })
        .unwrap();

        assert!(page.contains("&lt;code&gt;&amp;secret"));
        assert!(!page.contains("<code>&secret"));
        assert!(!page.contains("sr_oauth_state"));
        assert!(page.contains("Never send this code to another person"));
    }

    #[test]
    fn manual_oauth_callback_surfaces_denial_without_rendering_a_code_field() {
        let page = cloud_oauth_manual_page(CloudOAuthCallbackQuery {
            state: Some("sr_oauth_state".to_string()),
            error: Some("access_denied".to_string()),
            error_description: Some("<cancelled>".to_string()),
            ..Default::default()
        })
        .unwrap();

        assert!(page.contains("&lt;cancelled&gt;"));
        assert!(page.contains("access_denied"));
        assert!(!page.contains("<textarea"));
    }

    #[test]
    fn listen_alert_renders_chained_segment_urls() {
        let html = render_listen_alert(
            "alert-1",
            &json!({
                "trigger": "manual",
                "occurredAt": "2026-01-02T03:04:05Z",
                "startOffsetSeconds": 20,
                "downloadUrls": [
                    "https://downloads.example/segment-1.wav",
                    "https://downloads.example/segment-2.wav",
                    "http://localhost.evil.example/segment-3.wav"
                ]
            }),
        );
        assert!(html.contains("Segment <span id=\"segment-index\">1</span> of 2"));
        assert!(html.contains("https://downloads.example/segment-1.wav"));
        assert!(html.contains("https://downloads.example/segment-2.wav"));
        assert!(!html.contains("localhost.evil.example"));
        assert!(html.contains("loadSegment(startOffset)"));
    }

    #[tokio::test]
    async fn jwks_refresh_is_rate_limited() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc as StdArc;

        // One-shot JWKS endpoint that serves a (valid, empty) key set and counts
        // how many times it is actually fetched. It accepts a single connection
        // then exits, so any *second* outbound fetch would hit a refused port.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = StdArc::new(AtomicUsize::new(0));
        let hits_server = hits.clone();
        let handle = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                hits_server.fetch_add(1, Ordering::SeqCst);
                stream
                    .set_read_timeout(Some(Duration::from_millis(200)))
                    .ok();
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = r#"{"keys":[{"kty":"oct","k":"c2VjcmV0","alg":"HS256","kid":"test"}]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let verifier = SupabaseVerifier {
            audience: SUPABASE_DEFAULT_AUDIENCE.to_string(),
            issuer: None,
            jwt_secret: None,
            jwks_url: Some(format!("http://{addr}/jwks")),
            jwks_cache: RwLock::new(None),
            jwks_last_refresh: RwLock::new(None),
            jwks_refresh_lock: AsyncMutex::new(()),
        };
        let http = reqwest::Client::builder().build().unwrap();

        // First cache miss refreshes; the next two are throttled and must not
        // emit an outbound fetch (which is the JWKS-amplification DoS guard).
        assert!(verifier.try_refresh_jwks(&http).await.unwrap());
        assert!(!verifier.try_refresh_jwks(&http).await.unwrap());
        assert!(!verifier.try_refresh_jwks(&http).await.unwrap());
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "repeated unknown-kid lookups must collapse to a single JWKS fetch"
        );

        handle.join().unwrap();
    }

    #[test]
    fn storage_endpoint_detection_and_validation_cover_r2() {
        let account_id = "0123456789abcdef0123456789abcdef";
        assert!(is_valid_r2_account_id(account_id));
        assert!(!is_valid_r2_account_id("not-an-account-id"));
        let endpoint = format!("https://{account_id}.r2.cloudflarestorage.com");
        assert!(validate_service_url(&endpoint, true));
        assert_eq!(
            storage_backend_for_endpoint(Some(&endpoint)),
            ObjectStorageBackend::CloudflareR2
        );
        assert_eq!(
            storage_backend_for_endpoint(Some("https://minio.example.com")),
            ObjectStorageBackend::S3Compatible
        );
        assert!(!validate_service_url(
            "https://user:pass@storage.example.com",
            true
        ));
        assert!(!validate_service_url("http://storage.example.com", true));
    }

    #[test]
    fn boolean_configuration_rejects_ambiguous_values() {
        assert!(parse_bool_value("FLAG", " TRUE ").unwrap());
        assert!(!parse_bool_value("FLAG", "off").unwrap());
        assert!(parse_bool_value("SOUND_RECORDER_REQUIRE_SUPABASE", "tru").is_err());
        assert!(parse_bool_value("FLAG", "").is_err());
    }

    #[test]
    fn supabase_overrides_must_stay_on_the_project_origin() {
        assert!(urls_have_same_origin(
            "https://project.supabase.co",
            "https://project.supabase.co/auth/v1/.well-known/jwks.json"
        ));
        assert!(!urls_have_same_origin(
            "https://project.supabase.co",
            "https://attacker.example/auth/v1/.well-known/jwks.json"
        ));
        assert!(!urls_have_same_origin(
            "http://127.0.0.1:54321",
            "http://127.0.0.1:54322/auth/v1"
        ));
    }

    #[test]
    fn storage_metadata_is_server_owned_and_cutover_checked() {
        let mut config = test_config().s3;
        let meta = attach_storage_metadata(
            json!({
                STORAGE_FINGERPRINT_META_KEY: "client-forgery",
                RETENTION_DELETE_PENDING_META_KEY: true,
                "client": "kept"
            }),
            &config.backend_fingerprint,
        )
        .unwrap();
        assert_eq!(
            meta.get(STORAGE_FINGERPRINT_META_KEY)
                .and_then(Value::as_str),
            Some(config.backend_fingerprint.as_str())
        );
        assert!(meta.get(RETENTION_DELETE_PENDING_META_KEY).is_none());
        assert_eq!(meta.get("client").and_then(Value::as_str), Some("kept"));
        assert!(storage_record_is_compatible(
            &config,
            Some(&config.backend_fingerprint)
        ));
        assert!(!storage_record_is_compatible(&config, Some("old-backend")));
        assert!(!storage_record_is_compatible(&config, None));
        config.allow_unmarked_storage_history = true;
        assert!(storage_record_is_compatible(&config, None));

        assert!(storage_history_compatible(false, false, false));
        assert!(!storage_history_compatible(false, true, false));
        assert!(storage_history_compatible(false, true, true));
        assert!(!storage_history_compatible(true, false, true));

        assert!(!unmarked_history_acknowledgment(false, None, "current").unwrap());
        assert!(unmarked_history_acknowledgment(true, Some("current"), "current").unwrap());
        assert!(unmarked_history_acknowledgment(true, Some("old"), "current").is_err());
    }

    #[test]
    fn mirror_metadata_is_server_owned() {
        let config = test_config().s3;
        let meta = attach_storage_metadata(
            json!({
                MIRROR_STATE_META_KEY: "mirrored",
                MIRROR_BUCKET_META_KEY: "attacker-bucket",
                MIRROR_FINGERPRINT_META_KEY: "forged",
                MIRROR_ATTEMPTS_META_KEY: "not-a-number",
                MIRROR_CLAIM_ID_META_KEY: "stolen-claim",
                "client": "kept"
            }),
            &config.backend_fingerprint,
        )
        .unwrap();
        for key in [
            MIRROR_STATE_META_KEY,
            MIRROR_BUCKET_META_KEY,
            MIRROR_FINGERPRINT_META_KEY,
            MIRROR_ATTEMPTS_META_KEY,
            MIRROR_CLAIM_ID_META_KEY,
        ] {
            assert!(
                meta.get(key).is_none(),
                "client-supplied {key} must be stripped"
            );
        }
        assert_eq!(meta.get("client").and_then(Value::as_str), Some("kept"));
    }

    #[test]
    fn mirror_must_target_a_different_store_than_primary() {
        let primary = test_config().s3;
        let mut mirror = primary.clone();
        assert!(mirror_targets_conflict(&primary, &mirror));
        mirror.bucket = "backup-bucket".to_string();
        mirror.backend_fingerprint = storage_backend_fingerprint(
            mirror.backend,
            mirror.endpoint.as_deref(),
            &mirror.region,
            &mirror.bucket,
        );
        assert!(!mirror_targets_conflict(&primary, &mirror));
        // An unconfigured mirror never conflicts.
        mirror.bucket = String::new();
        assert!(!mirror_targets_conflict(&primary, &mirror));
    }

    #[test]
    fn mirror_retry_backoff_grows_and_caps() {
        assert_eq!(mirror_retry_backoff(0), ChronoDuration::seconds(60));
        assert_eq!(mirror_retry_backoff(1), ChronoDuration::seconds(120));
        assert_eq!(mirror_retry_backoff(3), ChronoDuration::seconds(480));
        assert_eq!(mirror_retry_backoff(6), ChronoDuration::seconds(3600));
        assert_eq!(mirror_retry_backoff(100), ChronoDuration::seconds(3600));
        assert_eq!(mirror_retry_backoff(-5), ChronoDuration::seconds(60));
    }

    #[test]
    fn mirror_probe_mode_reflects_configuration() {
        let mut mirror = test_config().mirror;
        assert_eq!(mirror_probe_mode(&mirror), "unconfigured");
        mirror.bucket = "backup-bucket".to_string();
        assert_eq!(mirror_probe_mode(&mirror), "head_probe_not_found_ok");
        mirror.readiness_object_key = Some("sound-recorder/segments/.sentinel".to_string());
        assert_eq!(mirror_probe_mode(&mirror), "head_object");
    }

    #[tokio::test]
    async fn sentinel_readiness_performs_remote_head_object() {
        let (endpoint, requests, handle) = spawn_json_server("");
        let mut config = test_config();
        config.s3.backend = ObjectStorageBackend::S3Compatible;
        config.s3.endpoint = Some(endpoint);
        config.s3.force_path_style = true;
        config.s3.send_sse_aes256 = false;
        config.s3.access_key_id = Some("test-access-key".to_string());
        config.s3.secret_access_key = Some("test-secret-key".to_string());
        config.s3.readiness_object_key = Some("sound-recorder/segments/.readiness".to_string());
        config.s3.backend_fingerprint = storage_backend_fingerprint(
            config.s3.backend,
            config.s3.endpoint.as_deref(),
            &config.s3.region,
            &config.s3.bucket,
        );
        let state = state_from_config(config).await;
        assert!(storage_is_ready(&state).await);
        let request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            request.starts_with("HEAD /test-bucket/sound-recorder/segments/.readiness HTTP/1.1")
        );
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn asymmetric_supabase_readiness_fetches_nonempty_jwks() {
        let body = r#"{"keys":[{"kty":"oct","k":"c2VjcmV0","alg":"HS256","kid":"test"}]}"#;
        let (project_url, requests, handle) = spawn_json_server(body);
        let mut config = test_config();
        config.supabase = SupabaseConfig {
            url: Some(project_url.clone()),
            jwt_secret: None,
            jwks_url: Some(format!("{project_url}/auth/v1/.well-known/jwks.json")),
            issuer: Some(format!("{project_url}/auth/v1")),
            audience: SUPABASE_DEFAULT_AUDIENCE.to_string(),
            publishable_key: Some("publishable-key".to_string()),
            service_role_key: Some("service-role-key".to_string()),
            validation_errors: Vec::new(),
        };
        let verifier = SupabaseVerifier::from_config(&config.supabase)
            .map(Arc::new)
            .unwrap();
        let mut state = test_state(config);
        state.supabase = Some(verifier);
        assert!(supabase_is_ready(&state).await);
        let request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(request.starts_with("GET /auth/v1/.well-known/jwks.json HTTP/1.1"));
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn legacy_supabase_readiness_probes_auth_health_with_api_key() {
        let (project_url, requests, handle) = spawn_json_server(r#"{"version":"test"}"#);
        let mut config = test_config();
        config.supabase = SupabaseConfig {
            url: Some(project_url.clone()),
            jwt_secret: Some("legacy-secret".to_string()),
            jwks_url: Some(format!("{project_url}/auth/v1/.well-known/jwks.json")),
            issuer: Some(format!("{project_url}/auth/v1")),
            audience: SUPABASE_DEFAULT_AUDIENCE.to_string(),
            publishable_key: Some("publishable-key".to_string()),
            service_role_key: Some("service-role-key".to_string()),
            validation_errors: Vec::new(),
        };
        let verifier = SupabaseVerifier::from_config(&config.supabase)
            .map(Arc::new)
            .unwrap();
        let mut state = test_state(config);
        state.supabase = Some(verifier);
        assert!(supabase_is_ready(&state).await);
        let request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(request.starts_with("GET /auth/v1/health HTTP/1.1"));
        assert!(request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("apikey: publishable-key")));
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn r2_presign_uses_auto_region_and_omits_unsupported_sse() {
        let account_id = "0123456789abcdef0123456789abcdef";
        let mut config = test_config();
        config.s3.backend = ObjectStorageBackend::CloudflareR2;
        config.s3.region = "auto".to_string();
        config.s3.endpoint = Some(format!("https://{account_id}.r2.cloudflarestorage.com"));
        config.s3.send_sse_aes256 = false;
        config.s3.access_key_id = Some("test-r2-access-key".to_string());
        config.s3.secret_access_key = Some("test-r2-secret-key".to_string());
        config.s3.backend_fingerprint = storage_backend_fingerprint(
            config.s3.backend,
            config.s3.endpoint.as_deref(),
            &config.s3.region,
            &config.s3.bucket,
        );
        let strict_state = state_from_config(config.clone()).await;
        assert!(
            !storage_is_ready(&strict_state).await,
            "production readiness must fail closed without a remote sentinel"
        );
        assert_eq!(
            strict_state.config.s3.readiness_probe_mode(),
            "remote_probe_not_configured"
        );
        config.s3.allow_signing_only_readiness = true;
        let state = state_from_config(config).await;
        assert!(
            storage_is_ready(&state).await,
            "explicit development opt-out may use signing-only readiness"
        );
        assert_eq!(state.config.s3.readiness_probe_mode(), "signing_dev_only");
        let transfer = presign_put(
            &state,
            "test-bucket",
            "sound-recorder/segments/test.m4a",
            "audio/mp4",
            Some(128),
            Utc::now() + ChronoDuration::minutes(5),
        )
        .await
        .unwrap();
        let url = reqwest::Url::parse(&transfer.url).unwrap();
        assert_eq!(
            url.host_str(),
            Some("test-bucket.0123456789abcdef0123456789abcdef.r2.cloudflarestorage.com")
        );
        let credential = url
            .query_pairs()
            .find(|(name, _)| name.eq_ignore_ascii_case("X-Amz-Credential"))
            .map(|(_, value)| value.into_owned())
            .unwrap();
        assert!(credential.contains("/auto/s3/aws4_request"));
        assert!(!transfer.headers.iter().any(|header| {
            header
                .name
                .eq_ignore_ascii_case("x-amz-server-side-encryption")
        }));
        assert!(transfer
            .headers
            .iter()
            .any(|header| header.name.eq_ignore_ascii_case("content-type")
                && header.value == "audio/mp4"));
    }

    #[test]
    fn uploaded_object_metadata_must_match_the_presign() {
        let verified = validate_stored_object_metadata(
            StoredObjectMetadata {
                content_length: Some(128),
                content_type: Some("audio/mp4"),
                etag: Some("\"abc123\""),
            },
            StoredObjectExpectation {
                content_type: "audio/mp4",
                presigned_byte_count: Some(128),
                reported_byte_count: Some(128),
                reported_etag: Some("abc123"),
                max_segment_bytes: 1024,
            },
        )
        .unwrap();
        assert_eq!(
            verified,
            VerifiedStoredObject {
                byte_count: 128,
                etag: "abc123".to_string()
            }
        );
        assert!(validate_stored_object_metadata(
            StoredObjectMetadata {
                content_length: Some(127),
                content_type: Some("audio/mp4"),
                etag: Some("abc123"),
            },
            StoredObjectExpectation {
                content_type: "audio/mp4",
                presigned_byte_count: Some(128),
                reported_byte_count: None,
                reported_etag: None,
                max_segment_bytes: 1024,
            },
        )
        .is_err());
        assert!(validate_stored_object_metadata(
            StoredObjectMetadata {
                content_length: Some(128),
                content_type: Some("text/plain"),
                etag: Some("abc123"),
            },
            StoredObjectExpectation {
                content_type: "audio/mp4",
                presigned_byte_count: Some(128),
                reported_byte_count: None,
                reported_etag: None,
                max_segment_bytes: 1024,
            },
        )
        .is_err());
        assert!(validate_stored_object_metadata(
            StoredObjectMetadata {
                content_length: Some(0),
                content_type: Some("audio/mp4"),
                etag: Some("abc123"),
            },
            StoredObjectExpectation {
                content_type: "audio/mp4",
                presigned_byte_count: None,
                reported_byte_count: None,
                reported_etag: None,
                max_segment_bytes: 1024,
            },
        )
        .is_err());
    }

    #[test]
    fn supabase_auth_contract_is_strict_and_case_insensitive() {
        assert_eq!(strip_bearer_scheme("bearer token-123"), Some("token-123"));
        assert_eq!(strip_bearer_scheme("BEARER   token-123"), Some("token-123"));
        assert_eq!(strip_bearer_scheme("Basic token-123"), None);
        assert!(is_supported_supabase_algorithm(Algorithm::HS256));
        assert!(is_supported_supabase_algorithm(Algorithm::RS256));
        assert!(is_supported_supabase_algorithm(Algorithm::ES256));
        assert!(!is_supported_supabase_algorithm(Algorithm::HS512));

        let mut config = SupabaseConfig {
            url: Some("https://project.supabase.co".to_string()),
            jwt_secret: None,
            jwks_url: Some("https://project.supabase.co/auth/v1/.well-known/jwks.json".to_string()),
            issuer: Some("https://project.supabase.co/auth/v1".to_string()),
            audience: SUPABASE_DEFAULT_AUDIENCE.to_string(),
            publishable_key: Some("publishable-key".to_string()),
            service_role_key: Some("service-role-key".to_string()),
            validation_errors: Vec::new(),
        };
        assert!(config.account_features_configured());
        config.service_role_key = None;
        assert!(!config.account_features_configured());
        config.service_role_key = Some("service-role-key".to_string());
        config
            .validation_errors
            .push("invalid Supabase URL".to_string());
        assert!(!config.account_features_configured());
    }

    #[test]
    fn user_settings_defaults_match_the_shared_contract() {
        let input = UserSettingsInput::default();
        assert!(input.validate().is_ok());
        let row = input.into_interface(
            "11111111-1111-1111-1111-111111111111".to_string(),
            "2026-07-13T00:00:00Z".to_string(),
        );
        assert_eq!(row.preferred_use_case, "security");
        assert_eq!(row.device_retention_hours, 100);
        assert_eq!(row.capture_sample_rate, 48_000);
        assert_eq!(row.quiet_sample_rate, 16_000);
        assert_eq!(row.user_id, "11111111-1111-1111-1111-111111111111");
    }

    #[test]
    fn user_settings_validation_rejects_cross_field_and_enum_drift() {
        let input = UserSettingsInput {
            preferred_use_case: "surveillance".to_string(),
            ..UserSettingsInput::default()
        };
        assert!(input.validate().is_err());

        let input = UserSettingsInput {
            quiet_sample_rate: 48_000,
            capture_sample_rate: 16_000,
            ..UserSettingsInput::default()
        };
        assert!(input.validate().is_err());

        let input = UserSettingsInput {
            device_retention_hours: 100,
            cloud_retention_hours: 99,
            ..UserSettingsInput::default()
        };
        assert!(input.validate().is_err());

        let input = UserSettingsInput {
            segment_minutes: 1,
            overlap_seconds: 60,
            ..UserSettingsInput::default()
        };
        assert!(input.validate().is_err());

        let input = UserSettingsInput {
            mic_sensitivity: f64::NAN,
            ..UserSettingsInput::default()
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn user_settings_upsert_payload_never_accepts_an_owner_id() {
        let value = serde_json::to_value(UserSettingsInput::default()).expect("serialize settings");
        let object = value.as_object().expect("settings object");
        assert!(object.contains_key("preferred_use_case"));
        assert!(!object.contains_key("user_id"));
        assert!(!object.contains_key("supabase_anon_key"));
        assert!(!object.contains_key("s3_access_key"));
    }

    #[test]
    fn storage_key_zero_pads_sequence_and_maps_extension() {
        // The sequence number is zero-padded to ten digits so object keys sort
        // lexicographically in the order segments were captured.
        assert_eq!(
            storage_key("sound-recorder/segments", 1, "audio/m4a"),
            "sound-recorder/segments/segment-0000000001.m4a"
        );
        assert_eq!(
            storage_key("p", 1234567890, "audio/wav"),
            "p/segment-1234567890.wav"
        );
        // Content-type detection is case-insensitive and substring-based, with
        // m4a as the catch-all default for unrecognized types.
        assert_eq!(extension_for_content_type("audio/WEBM"), "webm");
        assert_eq!(extension_for_content_type("audio/ogg; codecs=opus"), "opus");
        assert_eq!(extension_for_content_type("audio/mpeg"), "mp3");
        assert_eq!(extension_for_content_type("audio/3gpp"), "3gp");
        assert_eq!(
            extension_for_content_type("application/octet-stream"),
            "m4a"
        );
    }

    #[test]
    fn destination_key_derives_from_segment_file_name() {
        let mut segment = test_segment();
        segment.device_id = "dev-7".to_string();
        segment.session_id = "sess-9".to_string();
        segment.storage_key =
            "sound-recorder/segments/device=dev/session=s/segment-0000000042.wav".to_string();
        // The folder path is trimmed of surrounding slashes and the file name is
        // taken from the segment's own storage key.
        assert_eq!(
            destination_key("/sound-recorder/", &segment),
            "sound-recorder/device=dev-7/session=sess-9/segment-0000000042.wav"
        );
        // When the storage key has no trailing file name component, a stable
        // fallback name is used instead of producing an empty segment.
        segment.storage_key = "trailing-slash/".to_string();
        assert_eq!(
            destination_key("backup", &segment),
            "backup/device=dev-7/session=sess-9/segment.m4a"
        );
    }

    #[test]
    fn signed_ttl_floors_expired_urls_at_one_second() {
        // An already-expired (or exactly-now) expiry yields the minimum 1s TTL
        // rather than a zero or negative duration that a signer would reject.
        let past = Utc::now() - ChronoDuration::hours(1);
        assert_eq!(signed_ttl(past), Duration::from_secs(1));
        // A future expiry maps to a positive duration close to the remaining
        // time; allow slack for the Utc::now() call inside signed_ttl.
        let future = Utc::now() + ChronoDuration::seconds(300);
        let ttl = signed_ttl(future);
        assert!(
            ttl >= Duration::from_secs(290) && ttl <= Duration::from_secs(300),
            "unexpected ttl: {ttl:?}"
        );
    }
}

fn render_home(config: &Config) -> String {
    let ios = if config.ios_app_store_url.is_some() {
        r#"<a class="button primary" href="/download/ios">Download for iOS</a>"#
    } else {
        r#"<span class="button disabled">iOS coming soon</span>"#
    };
    let android = if config.android_play_store_url.is_some() {
        r#"<a class="button" href="/download/android">Download for Android</a>"#
    } else {
        r#"<span class="button disabled">Android coming soon</span>"#
    };
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Sound Recorder Dashcam</title>
  <style>
    :root {{ color-scheme: light; --bg:#f7f8fa; --ink:#17202a; --muted:#5f6b76; --line:#d8dee6; --panel:#fff; --blue:#205f8f; --green:#1f6b4b; --red:#a33a32; }}
    * {{ box-sizing:border-box; }}
    body {{ margin:0; background:var(--bg); color:var(--ink); font:15px/1.5 ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
    header, main, footer {{ width:min(1120px, calc(100% - 32px)); margin:0 auto; }}
    header {{ min-height:82px; display:flex; align-items:center; justify-content:space-between; gap:18px; border-bottom:1px solid var(--line); }}
    .brand {{ font-weight:800; font-size:18px; }}
    nav {{ display:flex; gap:14px; align-items:center; flex-wrap:wrap; }}
    a {{ color:var(--blue); text-decoration:none; }}
    .hero {{ min-height:calc(100vh - 150px); display:grid; grid-template-columns:minmax(0, 1.05fr) minmax(320px, .95fr); gap:42px; align-items:center; padding:34px 0; }}
    h1 {{ margin:0; font-size:clamp(36px, 6vw, 76px); line-height:.98; letter-spacing:0; max-width:820px; }}
    .lede {{ margin:22px 0 0; max-width:720px; color:var(--muted); font-size:18px; }}
    .actions {{ margin-top:28px; display:flex; gap:12px; flex-wrap:wrap; }}
    .button {{ display:inline-flex; align-items:center; justify-content:center; min-height:44px; padding:0 16px; border:1px solid var(--line); border-radius:8px; background:var(--panel); color:var(--ink); font-weight:700; }}
    .button.primary {{ background:var(--blue); border-color:var(--blue); color:white; }}
    .button.disabled {{ color:#7a828b; background:#eef1f4; }}
    .recorder {{ min-height:420px; border:1px solid var(--line); border-radius:8px; background:linear-gradient(180deg,#fff,#eef4f7); padding:22px; display:flex; flex-direction:column; justify-content:space-between; box-shadow:0 18px 50px rgba(21,39,54,.12); }}
    .status {{ display:flex; justify-content:space-between; align-items:center; gap:12px; color:var(--muted); font-size:13px; text-transform:uppercase; letter-spacing:.08em; }}
    .dot {{ width:12px; height:12px; border-radius:50%; background:var(--red); box-shadow:0 0 0 8px rgba(163,58,50,.12); }}
    .wave {{ height:190px; display:flex; align-items:center; gap:7px; border-block:1px solid var(--line); overflow:hidden; }}
    .wave span {{ flex:1; min-width:4px; border-radius:999px; background:var(--green); opacity:.82; }}
    .wave span:nth-child(3n) {{ height:32%; background:var(--blue); }}
    .wave span:nth-child(3n+1) {{ height:70%; }}
    .wave span:nth-child(4n) {{ height:90%; }}
    .wave span:nth-child(5n) {{ height:52%; }}
    .facts {{ display:grid; grid-template-columns:repeat(3, 1fr); gap:12px; }}
    .fact {{ border-top:1px solid var(--line); padding-top:12px; }}
    .fact strong {{ display:block; font-size:20px; }}
    .fact span {{ color:var(--muted); font-size:13px; }}
    section {{ border-top:1px solid var(--line); padding:30px 0; display:grid; grid-template-columns:260px minmax(0, 1fr); gap:28px; }}
    h2 {{ margin:0; font-size:22px; }}
    p {{ margin:0 0 12px; }}
    footer {{ color:var(--muted); padding:24px 0 40px; }}
    @media (max-width: 820px) {{
      header {{ align-items:flex-start; flex-direction:column; padding:18px 0; }}
      .hero {{ grid-template-columns:1fr; min-height:auto; }}
      .recorder {{ min-height:330px; }}
      section {{ grid-template-columns:1fr; }}
      .facts {{ grid-template-columns:1fr; }}
    }}
  </style>
</head>
<body>
  <header>
    <div class="brand">Sound Recorder Dashcam</div>
    <nav>
      <a href="/privacy">Privacy</a>
      <a href="/docs/api">API</a>
    </nav>
  </header>
  <main>
    <div class="hero">
      <div>
        <h1>Rolling audio memory for moments that need a record.</h1>
        <p class="lede">A mobile sound recorder backend for explicit, user-controlled recording with a {retention} hour rolling window, private object storage, and short-lived evidence export links.</p>
        <div class="actions">{ios}{android}</div>
      </div>
      <div class="recorder" aria-label="Recorder status preview">
        <div class="status"><span>Recording window</span><span class="dot" aria-hidden="true"></span></div>
        <div class="wave">{bars}</div>
        <div class="facts">
          <div class="fact"><strong>{retention}h</strong><span>rolling retention</span></div>
          <div class="fact"><strong>{segment}s</strong><span>default segments</span></div>
          <div class="fact"><strong>S3 API</strong><span>direct private upload</span></div>
        </div>
      </div>
    </div>
    <section>
      <h2>Built For Consent</h2>
      <div>
        <p>Registration records consent version, accepted timestamp, platform, and acknowledgement that the app shows an active recording indicator.</p>
        <p>The backend rejects device registration until registration auth or an explicit public-registration flag is configured.</p>
      </div>
    </section>
    <section>
      <h2>Evidence Export</h2>
      <div>
        <p>Audio segments stay private by default. The API exports a selected time range as short-lived object-storage download URLs and stores an audit event for each export.</p>
      </div>
    </section>
  </main>
  <footer>Generated API docs are available at <a href="/api/docs.json">/api/docs.json</a>.</footer>
</body>
</html>"#,
        retention = config.default_retention_hours,
        segment = config.default_segment_seconds,
        ios = ios,
        android = android,
        bars = (0..34)
            .map(|_| "<span></span>")
            .collect::<Vec<_>>()
            .join("")
    )
}

const PRIVACY_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Sound Recorder Dashcam Privacy</title>
  <style>
    body { margin:0; background:#f7f8fa; color:#17202a; font:15px/1.55 ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    main { width:min(860px, calc(100% - 32px)); margin:0 auto; padding:42px 0; }
    h1 { font-size:38px; line-height:1.05; letter-spacing:0; margin:0 0 18px; }
    h2 { margin-top:30px; }
    a { color:#205f8f; }
  </style>
</head>
<body>
  <main>
    <a href="/">Back</a>
    <h1>Privacy posture</h1>
    <p>This backend is designed for explicit personal recording, visible recording state, short-lived signed URLs, and a rolling retention window capped at 500 hours.</p>
    <h2>Consent</h2>
    <p>Mobile clients must record the consent version and acknowledgement that active recording is visible to the device owner before a device can register.</p>
    <h2>Storage</h2>
    <p>The service stores object keys and metadata in Postgres. Upload and download URLs are minted on demand and expire quickly.</p>
    <h2>Exports</h2>
    <p>Evidence exports are scoped by account, time range, and device token. Export activity is written to the audit table.</p>
  </main>
</body>
</html>"#;
