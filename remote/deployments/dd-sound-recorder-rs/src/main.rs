use std::{
    env,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use aws_config::meta::region::RegionProviderChain;
use aws_sdk_s3::{config::Region, presigning::PresigningConfig, types::ServerSideEncryption};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounterVec, IntGauge, Opts, TextEncoder};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_postgres::Row;
use tracing::{error, info, warn};
use uuid::Uuid;

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
const DEFAULT_SESSION_TTL_HOURS: i64 = 24;
const MAX_TIMELINE_LIMIT: i64 = 500;
const MAX_EXPORT_SEGMENTS: i64 = 240;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    s3: Option<aws_sdk_s3::Client>,
}

#[derive(Clone)]
struct Config {
    database_url: Option<String>,
    server_auth_secret: Option<String>,
    token_pepper: String,
    token_pepper_configured: bool,
    registration_bearer: Option<String>,
    allow_public_device_registration: bool,
    s3: S3StorageConfig,
    ios_app_store_url: Option<String>,
    android_play_store_url: Option<String>,
    default_retention_hours: i32,
    upload_url_ttl: Duration,
    download_url_ttl: Duration,
    session_ttl_hours: i64,
    default_segment_seconds: i32,
    max_segment_seconds: i32,
    max_segment_bytes: i32,
}

#[derive(Clone)]
struct S3StorageConfig {
    bucket: String,
    key_prefix: String,
    cdn_base_url: Option<String>,
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
        let (status, error, message) = match self {
            ServiceError::BadRequest(message) => (StatusCode::BAD_REQUEST, "bad_request", message),
            ServiceError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "authentication required".to_string(),
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
    retention_hours: i32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    ok: bool,
    service: &'static str,
    mode: &'static str,
    postgres_configured: bool,
    s3_configured: bool,
    token_pepper_configured: bool,
    registration_configured: bool,
    server_auth_configured: bool,
    retention_hours: i32,
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
struct MobilePolicy {
    retention_hours: i32,
    default_segment_seconds: i32,
    max_segment_seconds: i32,
    max_segment_bytes: i32,
    upload_url_ttl_seconds: u64,
    download_url_ttl_seconds: u64,
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
}

struct SessionPolicy {
    status: String,
    storage_bucket: String,
    storage_prefix: String,
    content_type: String,
    codec: Option<String>,
    segment_duration_seconds: i32,
    max_segment_bytes: i32,
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
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

fn config_from_env() -> Config {
    let token_pepper = first_env(&[
        "SOUND_RECORDER_DEVICE_TOKEN_PEPPER",
        "SOUND_RECORDER_SERVER_AUTH_SECRET",
        "SERVER_AUTH_SECRET",
    ]);
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

    Config {
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
        allow_public_device_registration: env_bool(
            "SOUND_RECORDER_ALLOW_PUBLIC_DEVICE_REGISTRATION",
            false,
        ),
        s3: S3StorageConfig {
            bucket: first_env(&["SOUND_RECORDER_S3_BUCKET", "S3_BUCKET"]).unwrap_or_default(),
            key_prefix: first_env(&["SOUND_RECORDER_S3_KEY_PREFIX", "S3_KEY_PREFIX"])
                .unwrap_or_else(|| "sound-recorder/segments".to_string()),
            cdn_base_url: first_env(&[
                "SOUND_RECORDER_CDN_BASE_URL",
                "SOUND_RECORDER_S3_PUBLIC_BASE_URL",
                "S3_PUBLIC_BASE_URL",
            ]),
        },
        ios_app_store_url: first_env(&["SOUND_RECORDER_IOS_APP_STORE_URL"]),
        android_play_store_url: first_env(&["SOUND_RECORDER_ANDROID_PLAY_STORE_URL"]),
        default_retention_hours,
        upload_url_ttl: Duration::from_secs(env_u64(
            "SOUND_RECORDER_UPLOAD_URL_TTL_SECONDS",
            DEFAULT_UPLOAD_URL_TTL_SECONDS,
        )),
        download_url_ttl: Duration::from_secs(env_u64(
            "SOUND_RECORDER_DOWNLOAD_URL_TTL_SECONDS",
            DEFAULT_DOWNLOAD_URL_TTL_SECONDS,
        )),
        session_ttl_hours: env_i64(
            "SOUND_RECORDER_SESSION_TTL_HOURS",
            DEFAULT_SESSION_TTL_HOURS,
        ),
        default_segment_seconds,
        max_segment_seconds,
        max_segment_bytes,
    }
}

async fn state_from_config(config: Config) -> AppState {
    let s3 = if !config.s3.bucket.is_empty() {
        let region = first_env(&[
            "SOUND_RECORDER_S3_REGION",
            "S3_REGION",
            "AWS_REGION",
            "AWS_DEFAULT_REGION",
        ]);
        let region_provider = RegionProviderChain::first_try(region.map(Region::new))
            .or_default_provider()
            .or_else("us-east-1");
        let shared_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(region_provider)
            .load()
            .await;
        let mut builder = aws_sdk_s3::config::Builder::from(&shared_config);
        if let Some(endpoint) = first_env(&["SOUND_RECORDER_S3_ENDPOINT", "S3_ENDPOINT"]) {
            builder = builder.endpoint_url(endpoint);
        }
        Some(aws_sdk_s3::Client::from_conf(builder.build()))
    } else {
        None
    };

    AppState {
        config: Arc::new(config),
        s3,
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

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
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

fn normalize_platform(value: &str) -> Result<String, ServiceError> {
    let platform = value.trim().to_ascii_lowercase();
    if matches!(platform.as_str(), "ios" | "android") {
        Ok(platform)
    } else {
        Err(ServiceError::BadRequest(
            "platform must be ios or android".to_string(),
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
        Some(value) if value.is_object() => Ok(value),
        Some(_) => Err(ServiceError::BadRequest(
            "metaData must be a JSON object".to_string(),
        )),
    }
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

fn policy(config: &Config, retention_hours: i32) -> MobilePolicy {
    MobilePolicy {
        retention_hours,
        default_segment_seconds: config.default_segment_seconds,
        max_segment_seconds: config.max_segment_seconds,
        max_segment_bytes: config.max_segment_bytes,
        upload_url_ttl_seconds: config.upload_url_ttl.as_secs(),
        download_url_ttl_seconds: config.download_url_ttl.as_secs(),
    }
}

async fn connect_postgres(config: &Config) -> Result<tokio_postgres::Client, ServiceError> {
    let database_url = config.database_url.as_deref().ok_or_else(|| {
        ServiceError::Unavailable("sound recorder database is not configured".to_string())
    })?;
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
    let (client, connection) = tokio_postgres::connect(database_url, tls)
        .await
        .map_err(|err| {
            error!(error = %err, "postgres connect failed");
            ServiceError::Unavailable("postgres connection failed".to_string())
        })?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            error!(error = %err, "postgres connection task failed");
        }
    });
    Ok(client)
}

fn db_error(error: tokio_postgres::Error) -> ServiceError {
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

fn require_registration_auth(config: &Config, headers: &HeaderMap) -> Result<(), ServiceError> {
    if let Some(expected) = config.registration_bearer.as_deref() {
        let provided = bearer_token(headers).unwrap_or("");
        if !provided.is_empty() && const_time_eq(provided.as_bytes(), expected.as_bytes()) {
            return Ok(());
        }
        return Err(ServiceError::Unauthorized);
    }
    if config.allow_public_device_registration {
        Ok(())
    } else {
        Err(ServiceError::Unavailable(
            "device registration is disabled until SOUND_RECORDER_REGISTRATION_BEARER or SOUND_RECORDER_ALLOW_PUBLIC_DEVICE_REGISTRATION is configured".to_string(),
        ))
    }
}

async fn authenticate_device(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(DeviceAuth, tokio_postgres::Client), ServiceError> {
    if !state.config.token_pepper_configured {
        return Err(ServiceError::Unavailable(
            "device token pepper is not configured".to_string(),
        ));
    }
    let token = bearer_token(headers).ok_or(ServiceError::Unauthorized)?;
    let token_hash = hash_secret(token, &state.config.token_pepper);
    let client = connect_postgres(&state.config).await?;
    let row = client
        .query_opt(
            "select d.id::text as device_id, d.account_id::text as account_id, a.retention_hours
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
    client: &tokio_postgres::Client,
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
    client: &tokio_postgres::Client,
    config: &Config,
    req: &RegisterDeviceRequest,
    legal_region: Option<&str>,
) -> Result<(String, i32), ServiceError> {
    if let Some(external_subject) = req
        .external_subject
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let external_subject = validate_nonempty(external_subject, "externalSubject", 240)?;
        if let Some(row) = client
            .query_opt(
                "select id::text, retention_hours
                 from sound_recorder_accounts
                 where external_subject = $1 and status <> 'deleted'",
                &[&external_subject],
            )
            .await
            .map_err(db_error)?
        {
            let account_id: String = row.get("id");
            let display_name = clean_string(req.display_name.clone(), 160);
            let _ = client
                .execute(
                    "update sound_recorder_accounts
                     set display_name = coalesce($2, display_name),
                         legal_region = coalesce($3, legal_region),
                         updated_at = now()
                     where id = $1::uuid",
                    &[&account_id, &display_name, &legal_region],
                )
                .await;
            return Ok((account_id, row.get("retention_hours")));
        }

        let account_id = Uuid::new_v4().to_string();
        let display_name = clean_string(req.display_name.clone(), 160);
        let row = client
            .query_one(
                "insert into sound_recorder_accounts
                  (id, external_subject, display_name, legal_region, retention_hours)
                 values ($1::uuid, $2, $3, $4, $5)
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
        return Ok((row.get("id"), row.get("retention_hours")));
    }

    let account_id = Uuid::new_v4().to_string();
    let display_name = clean_string(req.display_name.clone(), 160);
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
        s3_configured: state.s3.is_some() && !state.config.s3.bucket.is_empty(),
        token_pepper_configured: state.config.token_pepper_configured,
        registration_configured: state.config.registration_bearer.is_some()
            || state.config.allow_public_device_registration,
        server_auth_configured: state.config.server_auth_secret.is_some(),
        retention_hours: state.config.default_retention_hours,
    })
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let ready = state.config.database_url.is_some()
        && state.s3.is_some()
        && !state.config.s3.bucket.is_empty()
        && state.config.token_pepper_configured
        && (state.config.registration_bearer.is_some()
            || state.config.allow_public_device_registration)
        && state.config.server_auth_secret.is_some();
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
            s3_configured: state.s3.is_some() && !state.config.s3.bucket.is_empty(),
            token_pepper_configured: state.config.token_pepper_configured,
            registration_configured: state.config.registration_bearer.is_some()
                || state.config.allow_public_device_registration,
            server_auth_configured: state.config.server_auth_secret.is_some(),
            retention_hours: state.config.default_retention_hours,
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

async fn register_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegisterDeviceRequest>,
) -> Result<Json<RegisterDeviceResponse>, ServiceError> {
    require_registration_auth(&state.config, &headers)?;
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
    let device_label = clean_string(req.device_label.clone(), 160);
    let app_version = clean_string(req.app_version.clone(), 80);
    let os_version = clean_string(req.os_version.clone(), 80);
    let consent_accepted_at = req.consent_accepted_at.unwrap_or_else(Utc::now);
    let attestation = validate_meta(req.attestation.clone())?;
    let token = new_device_token();
    let token_hash = hash_secret(&token, &state.config.token_pepper);
    let token_last4 = last4(&token);

    let client = connect_postgres(&state.config).await?;
    let (account_id, retention_hours) =
        find_or_create_account(&client, &state.config, &req, legal_region.as_deref()).await?;
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

async fn create_upload_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateUploadSessionRequest>,
) -> Result<Json<CreateUploadSessionResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
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
    let meta_data = validate_meta(req.meta_data)?;
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
               started_at, last_heartbeat_at, expires_at, client_timezone, legal_region, meta_data)
             values
              ($1::uuid, $2::uuid, $3::uuid, $4, $5, $6, $7,
               $8, $9, $10, $11, $12, $12, $13, $14, $15, $16)
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
            "contentType": content_type
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
    let session = load_session_policy(&client, &auth, &session_id).await?;
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
        if byte_count < 0 || byte_count > session.max_segment_bytes {
            return Err(ServiceError::BadRequest(format!(
                "byteCount must be between 0 and {}",
                session.max_segment_bytes
            )));
        }
    }
    let sha256_hex = validate_sha256(req.sha256_hex)?;
    let meta_data = validate_meta(req.meta_data)?;
    let retention_cutoff = Utc::now()
        .checked_sub_signed(ChronoDuration::hours(auth.retention_hours as i64))
        .unwrap_or_else(Utc::now);
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
    let upload_expires_at = Utc::now()
        .checked_add_signed(chrono_duration_from_std(state.config.upload_url_ttl)?)
        .unwrap_or_else(Utc::now);
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
             where sound_recorder_segments.status <> 'uploaded'
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

async fn complete_segment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, segment_id)): Path<(String, String)>,
    Json(req): Json<CompleteSegmentRequest>,
) -> Result<Json<CompleteSegmentResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let sha256_hex = validate_sha256(req.sha256_hex)?;
    let etag = clean_string(req.etag, 160);
    if let Some(byte_count) = req.byte_count {
        if !(0..=MAX_SEGMENT_BYTES).contains(&byte_count) {
            return Err(ServiceError::BadRequest(
                "byteCount is outside allowed range".to_string(),
            ));
        }
    }
    let row = client
        .query_opt(
            "update sound_recorder_segments
             set status = 'uploaded',
                 etag = coalesce($5, etag),
                 byte_count = coalesce($6, byte_count),
                 sha256_hex = coalesce($7, sha256_hex),
                 captured_ended_at = coalesce($8, captured_ended_at),
                 uploaded_at = now(),
                 updated_at = now()
             where id = $1::uuid
               and session_id = $2::uuid
               and account_id = $3::uuid
               and device_id = $4::uuid
               and status <> 'deleted'
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
                &etag,
                &req.byte_count,
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
    audit_event(
        &client,
        Some(&auth.account_id),
        Some(&auth.device_id),
        "sound_recorder.segment.completed",
        json!({
            "sessionId": session_id,
            "segmentId": segment_id,
            "byteCount": req.byte_count
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
    if let Some(device_id) = &req.device_id {
        if !uuid_like(device_id) {
            return Err(ServiceError::BadRequest(
                "deviceId must be a UUID".to_string(),
            ));
        }
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
    let rows = if let Some(device_id) = &req.device_id {
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
                &req.device_id,
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

async fn retention_sweep(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<RetentionSweepResponse>, ServiceError> {
    require_internal_auth(&state.config, &headers)?;
    let client = connect_postgres(&state.config).await?;
    let expired = client
        .execute(
            "update sound_recorder_segments
             set status = 'expired', updated_at = now()
             where status in ('pending', 'uploaded') and expires_at < now()",
            &[],
        )
        .await
        .map_err(db_error)?;
    audit_event(
        &client,
        None,
        None,
        "sound_recorder.retention.swept",
        json!({ "expiredSegments": expired }),
    )
    .await;
    record_request("POST", "/internal/retention/sweep", StatusCode::OK);
    Ok(Json(RetentionSweepResponse {
        ok: true,
        expired_segments: expired,
    }))
}

async fn load_session_policy(
    client: &tokio_postgres::Client,
    auth: &DeviceAuth,
    session_id: &str,
) -> Result<SessionPolicy, ServiceError> {
    let row = client
        .query_opt(
            "select account_id::text, device_id::text, status, storage_bucket, storage_prefix,
                    content_type, codec, segment_duration_seconds, max_segment_bytes
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
    Ok(SessionPolicy {
        status: row.get("status"),
        storage_bucket: row.get("storage_bucket"),
        storage_prefix: row.get("storage_prefix"),
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
        .content_type(content_type)
        .server_side_encryption(ServerSideEncryption::Aes256);
    if let Some(byte_count) = byte_count {
        request = request.content_length(byte_count as i64);
    }
    let presigned = request.presigned(presigning_config).await.map_err(|err| {
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
    let presigned = s3
        .get_object()
        .bucket(bucket)
        .key(key)
        .presigned(presigning_config)
        .await
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

fn uuid_like(value: &str) -> bool {
    Uuid::parse_str(value).is_ok()
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(home))
        .route("/privacy", get(privacy))
        .route("/download/ios", get(download_ios))
        .route("/download/android", get(download_android))
        .route("/api/mobile/v1/devices/register", post(register_device))
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
        .route("/internal/retention/sweep", post(retention_sweep))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .with_state(state)
}

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dd_sound_recorder_rs=info,tower_http=warn".into()),
        )
        .without_time()
        .init();

    let config = config_from_env();
    if !config.token_pepper_configured {
        warn!("SOUND_RECORDER_DEVICE_TOKEN_PEPPER is not configured; device tokens will not survive process restart");
    }
    if config.registration_bearer.is_none() && !config.allow_public_device_registration {
        warn!("device registration is disabled until registration auth is configured");
    }
    let host = first_env(&["HOST"]).unwrap_or_else(|| "0.0.0.0".to_string());
    let port = env_u64("PORT", DEFAULT_PORT as u64) as u16;
    let state = state_from_config(config).await;

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("HOST/PORT must form a socket address");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind dd-sound-recorder-rs");
    info!("dd-sound-recorder-rs listening on http://{addr}");
    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("axum server crashed");
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
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
        <p class="lede">A mobile sound recorder backend for explicit, user-controlled recording with a {retention} hour rolling window, encrypted S3 storage, and short-lived evidence export links.</p>
        <div class="actions">{ios}{android}</div>
      </div>
      <div class="recorder" aria-label="Recorder status preview">
        <div class="status"><span>Recording window</span><span class="dot" aria-hidden="true"></span></div>
        <div class="wave">{bars}</div>
        <div class="facts">
          <div class="fact"><strong>{retention}h</strong><span>rolling retention</span></div>
          <div class="fact"><strong>{segment}s</strong><span>default segments</span></div>
          <div class="fact"><strong>S3</strong><span>presigned upload</span></div>
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
        <p>Audio segments stay private by default. The API exports a selected time range as short-lived S3 download URLs and stores an audit event for each export.</p>
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
