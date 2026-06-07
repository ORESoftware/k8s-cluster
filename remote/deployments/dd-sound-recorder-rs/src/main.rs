use std::{
    env,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use aws_config::meta::region::RegionProviderChain;
use aws_sdk_s3::{config::Region, presigning::PresigningConfig, types::ServerSideEncryption};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{
    engine::general_purpose::{STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounterVec, IntGauge, Opts, TextEncoder};
use rand::{rngs::OsRng, RngCore};
use reqwest::multipart::{Form, Part};
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
const MAX_META_BYTES: usize = 4096;
const MAX_CAPTURE_CLOCK_SKEW_SECONDS: i64 = 300;
const DEFAULT_OAUTH_STATE_TTL_SECONDS: u64 = 600;
const DEFAULT_CLOUD_COPY_BATCH_SIZE: i64 = 25;
const MAX_CLOUD_COPY_BATCH_SIZE: i64 = 100;
const DEFAULT_CLOUD_COPY_MAX_ATTEMPTS: i32 = 3;
const DEFAULT_CLOUD_COPY_MAX_BYTES: i64 = 25 * 1024 * 1024;
const MAX_CLOUD_COPY_MAX_BYTES: i64 = 200 * 1024 * 1024;
const DEFAULT_CLOUD_BACKFILL_SEGMENTS: i64 = 240;
const MAX_CLOUD_BACKFILL_SEGMENTS: i64 = 1000;
const GOOGLE_DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const MICROSOFT_ONEDRIVE_SCOPE: &str = "offline_access Files.ReadWrite.AppFolder";

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    s3: Option<aws_sdk_s3::Client>,
    http: reqwest::Client,
    cloud_sealer: Option<CloudTokenSealer>,
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
    oauth_state_ttl: Duration,
    cloud_copy_batch_size: i64,
    cloud_copy_max_attempts: i32,
    cloud_copy_max_bytes: i64,
    cloud_backfill_segments: i64,
    google_oauth: OAuthProviderConfig,
    microsoft_oauth: OAuthProviderConfig,
    google_drive_upload_url: String,
    microsoft_graph_base_url: String,
}

#[derive(Clone)]
struct S3StorageConfig {
    bucket: String,
    key_prefix: String,
    cdn_base_url: Option<String>,
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
    cloud_token_sealer_configured: bool,
    google_drive_configured: bool,
    microsoft_onedrive_configured: bool,
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
    cloud_copy_supported_providers: Vec<&'static str>,
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
    meta_data: Option<Value>,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudCopyDrainResult {
    job_id: String,
    provider: String,
    status: String,
    message: Option<String>,
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

fn env_duration_clamped(name: &str, default: u64, min: u64, max: u64) -> Duration {
    Duration::from_secs(env_u64(name, default).clamp(min, max))
}

fn env_i64_clamped(name: &str, default: i64, min: i64, max: i64) -> i64 {
    env_i64(name, default).clamp(min, max)
}

fn config_from_env() -> Config {
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
        google_oauth: OAuthProviderConfig {
            client_id: first_env(&["SOUND_RECORDER_GOOGLE_CLIENT_ID"]),
            client_secret: first_env(&["SOUND_RECORDER_GOOGLE_CLIENT_SECRET"]),
            authorization_url: first_env(&["SOUND_RECORDER_GOOGLE_AUTHORIZATION_URL"]),
            token_url: first_env(&["SOUND_RECORDER_GOOGLE_TOKEN_URL"]),
        },
        microsoft_oauth: OAuthProviderConfig {
            client_id: first_env(&["SOUND_RECORDER_MICROSOFT_CLIENT_ID"]),
            client_secret: first_env(&["SOUND_RECORDER_MICROSOFT_CLIENT_SECRET"]),
            authorization_url: first_env(&["SOUND_RECORDER_MICROSOFT_AUTHORIZATION_URL"]),
            token_url: first_env(&["SOUND_RECORDER_MICROSOFT_TOKEN_URL"]),
        },
        google_drive_upload_url: first_env(&["SOUND_RECORDER_GOOGLE_DRIVE_UPLOAD_URL"])
            .unwrap_or_else(|| "https://www.googleapis.com/upload/drive/v3/files".to_string()),
        microsoft_graph_base_url: first_env(&["SOUND_RECORDER_MICROSOFT_GRAPH_BASE_URL"])
            .unwrap_or_else(|| "https://graph.microsoft.com/v1.0".to_string()),
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

    AppState {
        config: Arc::new(config),
        s3,
        http,
        cloud_sealer,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CloudProvider {
    GoogleDrive,
    MicrosoftOneDrive,
    AppleICloud,
}

impl CloudProvider {
    fn parse(value: &str) -> Result<Self, ServiceError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "google_drive" | "googledrive" | "google" => Ok(Self::GoogleDrive),
            "microsoft_onedrive" | "onedrive" | "microsoft" => Ok(Self::MicrosoftOneDrive),
            "apple_icloud" | "icloud" | "apple" => Ok(Self::AppleICloud),
            _ => Err(ServiceError::BadRequest(
                "provider must be google_drive, microsoft_onedrive, or apple_icloud".to_string(),
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::GoogleDrive => "google_drive",
            Self::MicrosoftOneDrive => "microsoft_onedrive",
            Self::AppleICloud => "apple_icloud",
        }
    }

    fn link_mode(self) -> &'static str {
        match self {
            Self::AppleICloud => "client_managed",
            Self::GoogleDrive | Self::MicrosoftOneDrive => "server_oauth",
        }
    }

    fn required_scope(self) -> Option<&'static str> {
        match self {
            Self::GoogleDrive => Some(GOOGLE_DRIVE_SCOPE),
            Self::MicrosoftOneDrive => Some(MICROSOFT_ONEDRIVE_SCOPE),
            Self::AppleICloud => None,
        }
    }

    fn oauth_config<'a>(self, config: &'a Config) -> Option<&'a OAuthProviderConfig> {
        match self {
            Self::GoogleDrive => Some(&config.google_oauth),
            Self::MicrosoftOneDrive => Some(&config.microsoft_oauth),
            Self::AppleICloud => None,
        }
    }

    fn authorization_endpoint(self) -> Option<&'static str> {
        match self {
            Self::GoogleDrive => Some("https://accounts.google.com/o/oauth2/v2/auth"),
            Self::MicrosoftOneDrive => {
                Some("https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize")
            }
            Self::AppleICloud => None,
        }
    }

    fn token_endpoint(self) -> Option<&'static str> {
        match self {
            Self::GoogleDrive => Some("https://oauth2.googleapis.com/token"),
            Self::MicrosoftOneDrive => {
                Some("https://login.microsoftonline.com/consumers/oauth2/v2.0/token")
            }
            Self::AppleICloud => None,
        }
    }

    fn is_server_managed(self) -> bool {
        matches!(self, Self::GoogleDrive | Self::MicrosoftOneDrive)
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

fn validate_redirect_uri(
    provider: CloudProvider,
    value: Option<String>,
) -> Result<String, ServiceError> {
    if provider == CloudProvider::AppleICloud {
        return Ok("client-managed://apple-icloud".to_string());
    }
    let value = value.ok_or_else(|| {
        ServiceError::BadRequest("redirectUri is required for OAuth cloud links".to_string())
    })?;
    let uri = validate_nonempty(&value, "redirectUri", 512)?;
    let lower = uri.to_ascii_lowercase();
    if lower.starts_with("https://")
        || lower.starts_with("http://localhost")
        || lower.starts_with("http://127.0.0.1")
    {
        Ok(uri)
    } else {
        Err(ServiceError::BadRequest(
            "redirectUri must be https or local loopback http".to_string(),
        ))
    }
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
    match provider {
        CloudProvider::GoogleDrive => {
            params.push(("access_type", "offline".to_string()));
            params.push(("prompt", "consent".to_string()));
        }
        CloudProvider::MicrosoftOneDrive => {
            params.push(("response_mode", "query".to_string()));
        }
        CloudProvider::AppleICloud => {}
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
        ],
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
        cloud_token_sealer_configured: state.cloud_sealer.is_some(),
        google_drive_configured: state.config.google_oauth.client_id.is_some()
            && state.config.google_oauth.client_secret.is_some(),
        microsoft_onedrive_configured: state.config.microsoft_oauth.client_id.is_some()
            && state.config.microsoft_oauth.client_secret.is_some(),
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
            cloud_token_sealer_configured: state.cloud_sealer.is_some(),
            google_drive_configured: state.config.google_oauth.client_id.is_some()
                && state.config.google_oauth.client_secret.is_some(),
            microsoft_onedrive_configured: state.config.microsoft_oauth.client_id.is_some()
                && state.config.microsoft_oauth.client_secret.is_some(),
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
    let session_id = validate_uuid(&session_id, "sessionId")?;
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
    let session_id = validate_uuid(&session_id, "sessionId")?;
    let segment_id = validate_uuid(&segment_id, "segmentId")?;
    let sha256_hex = validate_sha256(req.sha256_hex)?;
    let etag = clean_string(req.etag, 160);
    let policy_row = client
        .query_opt(
            "select s.captured_started_at, s.duration_millis, us.max_segment_bytes
             from sound_recorder_segments s
             join sound_recorder_upload_sessions us on us.id = s.session_id
             where s.id = $1::uuid
               and s.session_id = $2::uuid
               and s.account_id = $3::uuid
               and s.device_id = $4::uuid
               and s.status <> 'deleted'",
            &[&segment_id, &session_id, &auth.account_id, &auth.device_id],
        )
        .await
        .map_err(db_error)?;
    let Some(policy_row) = policy_row else {
        return Err(ServiceError::NotFound("segment not found".to_string()));
    };
    let max_segment_bytes: i32 = policy_row.get("max_segment_bytes");
    if let Some(byte_count) = req.byte_count {
        if byte_count < 0 || byte_count > max_segment_bytes {
            return Err(ServiceError::BadRequest(format!(
                "byteCount must be between 0 and {max_segment_bytes}"
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
    let redirect_uri = validate_redirect_uri(provider, req.redirect_uri)?;
    let folder_path = validate_folder_path(req.folder_path)?;
    let root_folder_id = clean_optional_nonempty(req.root_folder_id, 512)?;
    let display_name = clean_string(req.display_name, 160);
    let meta_data = validate_meta(req.meta_data)?;
    let state_token = new_oauth_state();
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
        client_managed: provider == CloudProvider::AppleICloud,
    }))
}

async fn complete_cloud_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CompleteCloudLinkRequest>,
) -> Result<Json<CompleteCloudLinkResponse>, ServiceError> {
    let (auth, client) = authenticate_device(&state, &headers).await?;
    let provider = CloudProvider::parse(&req.provider)?;
    let state_token = validate_nonempty(&req.state, "state", 160)?;
    let state_hash = oauth_state_hash(&state.config, &state_token);
    let state_row = client
        .query_opt(
            "select id::text, redirect_uri, folder_path, meta_data
             from sound_recorder_oauth_states
             where account_id = $1::uuid
               and device_id = $2::uuid
               and provider = $3
               and state_hash = $4
               and status = 'pending'
               and expires_at > now()",
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
    let folder_path = validate_folder_path(
        req.folder_path
            .or_else(|| state_row.get::<_, Option<String>>("folder_path")),
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
        let authorization_code = validate_nonempty(
            req.authorization_code.as_deref().unwrap_or(""),
            "authorizationCode",
            4096,
        )?;
        let token_set =
            exchange_authorization_code(&state, provider, &authorization_code, &redirect_uri)
                .await?;
        let plaintext = serde_json::to_vec(&token_set)
            .map_err(|_| ServiceError::Internal("cloud token encode failed".to_string()))?;
        let sealed = sealer.seal(&auth.account_id, provider, &plaintext)?;
        (Some(sealed), token_set.expires_at, token_set.scope.clone())
    } else {
        if !req.client_managed_acknowledged.unwrap_or(false) {
            return Err(ServiceError::BadRequest(
                "clientManagedAcknowledged must be true for apple_icloud links".to_string(),
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
             where id = $1::uuid",
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
        json!({ "connectionId": connection_id }),
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
    let client = connect_postgres(&state.config).await?;
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
             where j.status = 'pending'
               and (j.locked_until is null or j.locked_until < now())
               and c.status = 'active'
               and c.link_mode = 'server_oauth'
               and s.status = 'uploaded'
             order by j.updated_at asc
             limit $1",
            &[&limit],
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
        let claimed_attempts = claim_cloud_copy_job(&client, &item.job.id).await?;
        let Some(attempts) = claimed_attempts else {
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
                completed += 1;
                mark_cloud_copy_job_success(&client, &item, &provider_file_id).await?;
                results.push(CloudCopyDrainResult {
                    job_id: item.job.id,
                    provider: item.job.provider,
                    status: "completed".to_string(),
                    message: None,
                });
            }
            Err(err) => {
                failed += 1;
                let message = service_error_message(&err);
                mark_cloud_copy_job_error(&client, &item.job.id, attempts, &message, &state.config)
                    .await?;
                results.push(CloudCopyDrainResult {
                    job_id: item.job.id,
                    provider: item.job.provider,
                    status: "failed".to_string(),
                    message: Some(message),
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

fn service_error_message(error: &ServiceError) -> String {
    let message = match error {
        ServiceError::BadRequest(message)
        | ServiceError::NotFound(message)
        | ServiceError::Conflict(message)
        | ServiceError::Unavailable(message)
        | ServiceError::Internal(message) => message.as_str(),
        ServiceError::Unauthorized => "unauthorized",
    };
    message.chars().take(500).collect()
}

async fn claim_cloud_copy_job(
    client: &tokio_postgres::Client,
    job_id: &str,
) -> Result<Option<i32>, ServiceError> {
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
               and status = 'pending'
               and (locked_until is null or locked_until < now())
             returning attempts",
            &[&job_id, &locked_until],
        )
        .await
        .map_err(db_error)?;
    Ok(row.map(|row| row.get("attempts")))
}

async fn process_cloud_copy_job(
    state: &AppState,
    client: &tokio_postgres::Client,
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
        CloudProvider::AppleICloud => Err(ServiceError::BadRequest(
            "apple_icloud is client managed".to_string(),
        )),
    }
}

async fn download_segment_bytes(
    state: &AppState,
    segment: &SegmentResponse,
) -> Result<Vec<u8>, ServiceError> {
    let s3 = state
        .s3
        .as_ref()
        .ok_or_else(|| ServiceError::Unavailable("S3 client is not configured".to_string()))?;
    let object = s3
        .get_object()
        .bucket(&segment.storage_bucket)
        .key(&segment.storage_key)
        .send()
        .await
        .map_err(|err| {
            error!(error = %err, segment_id = segment.id, "S3 segment download failed");
            ServiceError::Unavailable("S3 segment download failed".to_string())
        })?;
    let bytes = object.body.collect().await.map_err(|err| {
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
    let file_name = google_drive_file_name(&job.destination_key);
    let mut metadata = json!({
        "name": file_name,
        "description": format!("Sound recorder segment {}", segment.id)
    });
    if let Some(root_folder_id) = &connection.root_folder_id {
        metadata["parents"] = json!([root_folder_id]);
    }
    let metadata_part = Part::text(metadata.to_string())
        .mime_str("application/json")
        .map_err(|_| ServiceError::Internal("invalid metadata mime".to_string()))?;
    let file_part = Part::bytes(bytes)
        .file_name(file_name)
        .mime_str(&segment.content_type)
        .map_err(|_| ServiceError::BadRequest("invalid segment content type".to_string()))?;
    let form = Form::new()
        .part("metadata", metadata_part)
        .part("file", file_part);
    let url = append_query(
        &state.config.google_drive_upload_url,
        "uploadType=multipart&fields=id,name,webViewLink",
    );
    let response = state
        .http
        .post(url)
        .bearer_auth(&token_set.access_token)
        .multipart(form)
        .send()
        .await
        .map_err(|err| {
            error!(error = %err, segment_id = segment.id, "Google Drive upload request failed");
            ServiceError::Unavailable("Google Drive upload failed".to_string())
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

async fn mark_cloud_copy_job_success(
    client: &tokio_postgres::Client,
    item: &CloudCopyWorkItem,
    provider_file_id: &str,
) -> Result<(), ServiceError> {
    client
        .execute(
            "update sound_recorder_cloud_copy_jobs
             set status = 'completed',
                 provider_file_id = $2,
                 completed_at = now(),
                 locked_until = null,
                 last_error = null,
                 updated_at = now()
             where id = $1::uuid",
            &[&item.job.id, &provider_file_id],
        )
        .await
        .map_err(db_error)?;
    client
        .execute(
            "update sound_recorder_cloud_connections
             set last_sync_at = now(), updated_at = now()
             where id = $1::uuid",
            &[&item.connection.id],
        )
        .await
        .map_err(db_error)?;
    Ok(())
}

async fn mark_cloud_copy_job_error(
    client: &tokio_postgres::Client,
    job_id: &str,
    attempts: i32,
    message: &str,
    config: &Config,
) -> Result<(), ServiceError> {
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
    client
        .execute(
            "update sound_recorder_cloud_copy_jobs
             set status = $2,
                 locked_until = $3,
                 last_error = $4,
                 updated_at = now()
             where id = $1::uuid",
            &[&job_id, &status, &locked_until, &last_error],
        )
        .await
        .map_err(db_error)?;
    Ok(())
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
    if provider == CloudProvider::AppleICloud {
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
    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("grant_type", "authorization_code"),
    ];
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
    client: &tokio_postgres::Client,
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

async fn upsert_cloud_connection(
    client: &tokio_postgres::Client,
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
            .map(|row| row.get::<_, String>("id"))
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
    client: &tokio_postgres::Client,
    connection: &CloudConnectionRecord,
    segment: &SegmentResponse,
) -> Result<u64, ServiceError> {
    let provider = CloudProvider::parse(&connection.provider)?;
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
    client: &tokio_postgres::Client,
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
    client: &tokio_postgres::Client,
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
               and expires_at > now()
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

#[cfg(test)]
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

    fn spawn_json_server(
        body: &'static str,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
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

    fn test_config() -> Config {
        Config {
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
            },
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
            google_drive_upload_url: "https://www.googleapis.com/upload/drive/v3/files".to_string(),
            microsoft_graph_base_url: "https://graph.microsoft.com/v1.0".to_string(),
        }
    }

    fn test_state(config: Config) -> AppState {
        AppState {
            config: Arc::new(config),
            s3: None,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            cloud_sealer: None,
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
    async fn google_drive_upload_hits_configured_endpoint() {
        let (base_url, rx, handle) = spawn_json_server(r#"{"id":"google-file-1"}"#);
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
        let file_id = upload_to_google_drive(
            &state,
            &connection,
            &segment,
            &job,
            b"ping".to_vec(),
            &test_token_set(),
        )
        .await
        .unwrap();
        assert_eq!(file_id, "google-file-1");
        let request = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
        let request_lower = request.to_ascii_lowercase();
        assert!(request.starts_with(
            "POST /upload/drive/v3/files?uploadType=multipart&fields=id,name,webViewLink HTTP/1.1"
        ));
        assert!(request_lower.contains("authorization: bearer test-access-token"));
        assert!(request.contains("drive-root-folder"));
        assert!(request.contains("sound-recorder__device=dev__session=s__segment-0000000001.m4a"));
        assert!(request.contains("ping"));
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

    #[test]
    fn metadata_has_size_limit() {
        let oversized = json!({ "x": "a".repeat(MAX_META_BYTES + 1) });
        assert!(validate_meta(Some(oversized)).is_err());
        assert!(validate_meta(Some(json!({ "ok": true }))).is_ok());
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
