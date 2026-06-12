//! `dd-ocr-rs` — optical character recognition service.
//!
//! A thin Rust/axum service that fronts several OCR engines behind one HTTP +
//! NATS API and a uniform response shape:
//!
//!   * a **local, open-source** engine — Tesseract (the Google-originated OCR
//!     engine) + Leptonica, reached over FFI through `cpp/tesseract_bridge.cpp`
//!     (see [`tesseract_ffi`]); images are decoded and Otsu-binarised first with
//!     the pure-Rust `image` + `imageproc` crates (see [`preprocess`]).
//!   * three **paid third-party** cloud backends (see [`cloud`]): Google Cloud
//!     Vision, AWS Textract (SigV4-signed), and Azure AI Vision Read.
//!
//! Callers pick an engine explicitly or let `auto` choose the first available
//! one. Each backend is enabled only when its toolchain / credentials exist, so
//! the service degrades to whatever is configured rather than failing to boot.

mod cloud;
mod preprocess;
mod tesseract_ffi;

use std::{
    env,
    error::Error,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use cloud::{AwsConfig, AzureConfig, CloudError, CloudOcr, GoogleConfig};
use dd_nats_subject_defs::{RUNTIME_CRITICAL_EVENTS_SUBJECT, RUNTIME_EVENTS_SUBJECT};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Semaphore;

const SERVICE_NAME: &str = "dd-ocr-rs";
const SERVICE_NAMESPACE: &str = "remote-dev";
const LOG_SCHEMA: &str = "dd.log.v1";
const LOG_SCOPE: &str = "dd-ocr-rs";
const SCHEMA_VERSION: &str = "dd.ocr.v1";

const DEFAULT_REQUEST_SUBJECT: &str = "dd.remote.ocr.requests";
const DEFAULT_RESULT_SUBJECT: &str = "dd.remote.ocr.results";
const OCR_QUEUE_GROUP: &str = "dd-ocr-rs";

const DEFAULT_MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_OCR_CONCURRENCY: usize = 4;
const DEFAULT_LANGUAGES: &str = "eng";
const DEFAULT_PSM: i32 = 3; // fully automatic page segmentation
const MAX_REQUEST_ID_LEN: usize = 128;
const MAX_LANGUAGES_LEN: usize = 64;

// ---------------------------------------------------------------------------
// Engine catalog
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Engine {
    Tesseract,
    Google,
    AwsTextract,
    Azure,
}

impl Engine {
    fn as_str(self) -> &'static str {
        match self {
            Engine::Tesseract => "tesseract",
            Engine::Google => "google",
            Engine::AwsTextract => "aws-textract",
            Engine::Azure => "azure",
        }
    }

    fn parse(raw: &str) -> Option<Engine> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "tesseract" | "local" => Some(Engine::Tesseract),
            "google" | "google-vision" | "gcv" => Some(Engine::Google),
            "aws" | "aws-textract" | "textract" => Some(Engine::AwsTextract),
            "azure" | "azure-vision" | "azure-read" => Some(Engine::Azure),
            _ => None,
        }
    }

    /// Preference order for `auto`: cheap-and-local first, then cloud.
    const AUTO_ORDER: [Engine; 4] = [
        Engine::Tesseract,
        Engine::Google,
        Engine::Azure,
        Engine::AwsTextract,
    ];
}

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    http: reqwest::Client,
    request_subject: String,
    result_subject: String,
    event_subject: String,
    critical_event_subject: String,
    max_image_bytes: usize,
    decode_limits: preprocess::DecodeLimits,
    default_languages: String,
    upscale_min_dim: u32,
    tesseract_enabled: bool,
    tesseract_version: Option<String>,
    google: Option<GoogleConfig>,
    aws: Option<AwsConfig>,
    azure: Option<AzureConfig>,
    ocr_semaphore: Arc<Semaphore>,
    nats_inflight: Arc<Semaphore>,
    http_inflight: Arc<Semaphore>,
    metrics: Arc<Metrics>,
}

impl AppState {
    fn engine_available(&self, engine: Engine) -> bool {
        match engine {
            Engine::Tesseract => self.tesseract_enabled,
            Engine::Google => self.google.is_some(),
            Engine::AwsTextract => self.aws.is_some(),
            Engine::Azure => self.azure.is_some(),
        }
    }

    /// First available engine in `auto` preference order.
    fn pick_auto(&self) -> Option<Engine> {
        Engine::AUTO_ORDER
            .into_iter()
            .find(|engine| self.engine_available(*engine))
    }
}

#[derive(Default)]
struct Metrics {
    ocr_requests_total: AtomicU64,
    ocr_errors_total: AtomicU64,
    tesseract_total: AtomicU64,
    google_total: AtomicU64,
    aws_total: AtomicU64,
    azure_total: AtomicU64,
    engine_unavailable_total: AtomicU64,
    image_rejected_total: AtomicU64,
    http_shed_total: AtomicU64,
    preprocess_errors_total: AtomicU64,
    nats_messages_total: AtomicU64,
    nats_payload_rejected_total: AtomicU64,
    nats_results_published_total: AtomicU64,
    nats_events_published_total: AtomicU64,
    nats_critical_events_published_total: AtomicU64,
    nats_publish_errors_total: AtomicU64,
    errors_total: AtomicU64,
}

// ---------------------------------------------------------------------------
// Shared helpers (logging, env, time) — mirrors the sibling dd-*-rs services so
// the structured log shape stays consistent across the fleet.
// ---------------------------------------------------------------------------

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn now_unix_nano() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn env_opt(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn env_usize(key: &str, fallback: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(fallback)
}

fn severity_number(severity: &str) -> i32 {
    match severity {
        "FATAL" => 24,
        "ERROR" => 17,
        "WARN" => 13,
        "INFO" => 9,
        "DEBUG" => 5,
        _ => 1,
    }
}

fn structured_log_record(severity: &str, event_name: &str, body: &str, attributes: Value) -> Value {
    json!({
        "schema": LOG_SCHEMA,
        "time_unix_nano": now_unix_nano().to_string(),
        "severity_text": severity,
        "severity_number": severity_number(severity),
        "body": body,
        "resource_service_name": SERVICE_NAME,
        "resource_service_namespace": SERVICE_NAMESPACE,
        "scope_name": LOG_SCOPE,
        "event_name": event_name,
        "attributes": attributes,
    })
}

fn log_to(stderr: bool, severity: &str, event_name: &str, body: &str, attributes: Value) {
    let record = structured_log_record(severity, event_name, body, attributes);
    let line = serde_json::to_string(&record).unwrap_or_else(|error| {
        format!(
            "{{\"schema\":\"{LOG_SCHEMA}\",\"severity_text\":\"ERROR\",\"body\":\"structured log serialization failed\",\"resource_service_name\":\"{SERVICE_NAME}\",\"event_name\":\"structured-log-serialize-failed\",\"attributes\":{{\"error\":\"{error}\"}}}}"
        )
    });
    if stderr {
        tracing::error!("{line}");
    } else {
        tracing::info!("{line}");
    }
}

fn log_info(event_name: &str, body: &str, attributes: Value) {
    log_to(false, "INFO", event_name, body, attributes);
}

fn log_warn(event_name: &str, body: &str, attributes: Value) {
    log_to(true, "WARN", event_name, body, attributes);
}

fn log_error(event_name: &str, body: &str, attributes: Value) {
    log_to(true, "ERROR", event_name, body, attributes);
}

fn json_error(status: StatusCode, message: impl Into<String>, details: Value) -> Response {
    (
        status,
        Json(json!({
            "ok": false,
            "error": message.into(),
            "details": details,
            "generatedAtMs": now_ms(),
        })),
    )
        .into_response()
}

/// Bound + sanitise a caller-supplied request id so it can't bloat logs or
/// inject newlines into structured output.
fn request_id(input: Option<&str>, prefix: &str) -> String {
    let raw = input
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .unwrap_or(prefix);
    raw.chars()
        .filter(|c| !c.is_control())
        .take(MAX_REQUEST_ID_LEN)
        .collect()
}

/// Restrict the language string to the `[a-z0-9+_-]` Tesseract uses so it can't
/// smuggle paths or shell-ish characters toward the engine.
fn sanitize_languages(input: Option<&str>, fallback: &str) -> String {
    let raw = input
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .unwrap_or(fallback);
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '_' | '-'))
        .take(MAX_LANGUAGES_LEN)
        .collect();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

// ---------------------------------------------------------------------------
// Core OCR path
// ---------------------------------------------------------------------------

/// Validated knobs shared by the HTTP, stream, and NATS entry points.
struct OcrOptions {
    languages: String,
    document_mode: bool,
    binarize: bool,
    psm: i32,
}

struct OcrSuccess {
    engine: Engine,
    text: String,
    confidence: Option<f64>,
    lines: usize,
    width: Option<u32>,
    height: Option<u32>,
    binarized: bool,
}

struct OcrFailure {
    status: StatusCode,
    message: String,
    details: Value,
}

impl OcrFailure {
    fn new(status: StatusCode, message: impl Into<String>, details: Value) -> Self {
        OcrFailure {
            status,
            message: message.into(),
            details,
        }
    }

    fn into_response(self) -> Response {
        json_error(self.status, self.message, self.details)
    }
}

fn cloud_failure(engine: Engine, error: CloudError) -> OcrFailure {
    let (status, message) = match &error {
        CloudError::NotConfigured => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("{} engine is not configured", engine.as_str()),
        ),
        CloudError::Transport(_) | CloudError::Status(_, _) | CloudError::Parse(_) => {
            (StatusCode::BAD_GATEWAY, format!("{} backend failed", engine.as_str()))
        }
    };
    OcrFailure::new(
        status,
        message,
        json!({ "engine": engine.as_str(), "detail": error.to_string() }),
    )
}

/// Resolve the engine for a request: explicit name, or `auto`/empty to pick the
/// first available backend.
fn resolve_engine(state: &AppState, requested: Option<&str>) -> Result<Engine, OcrFailure> {
    let requested = requested.map(|s| s.trim()).filter(|s| !s.is_empty());
    match requested {
        None | Some("auto") => state.pick_auto().ok_or_else(|| {
            state.metrics.engine_unavailable_total.fetch_add(1, Ordering::Relaxed);
            OcrFailure::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "no OCR engine is configured",
                json!({ "engines": engine_availability(state) }),
            )
        }),
        Some(name) => {
            let engine = Engine::parse(name).ok_or_else(|| {
                OcrFailure::new(
                    StatusCode::BAD_REQUEST,
                    "unknown engine",
                    json!({ "requested": name, "supported": ["tesseract", "google", "aws-textract", "azure", "auto"] }),
                )
            })?;
            if !state.engine_available(engine) {
                state.metrics.engine_unavailable_total.fetch_add(1, Ordering::Relaxed);
                return Err(OcrFailure::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("{} engine is not available", engine.as_str()),
                    json!({ "engine": engine.as_str(), "engines": engine_availability(state) }),
                ));
            }
            Ok(engine)
        }
    }
}

/// Run one OCR job end to end on the chosen engine.
async fn run_ocr(
    state: &AppState,
    engine: Engine,
    image: &[u8],
    opts: &OcrOptions,
) -> Result<OcrSuccess, OcrFailure> {
    if image.is_empty() {
        return Err(OcrFailure::new(
            StatusCode::BAD_REQUEST,
            "image is empty",
            json!({}),
        ));
    }
    if image.len() > state.max_image_bytes {
        state.metrics.image_rejected_total.fetch_add(1, Ordering::Relaxed);
        return Err(OcrFailure::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "image exceeds the configured size limit",
            json!({ "bytes": image.len(), "maxImageBytes": state.max_image_bytes }),
        ));
    }

    // Enforce the format allowlist up front (cheap, no decode, no permit held)
    // so junk/disallowed bytes are rejected before any CPU work or — critically
    // — before being forwarded to a paid third-party OCR API.
    if let Err(error) = preprocess::sniff_allowed(image) {
        state.metrics.image_rejected_total.fetch_add(1, Ordering::Relaxed);
        let status = match error {
            preprocess::PreprocessError::UnsupportedFormat(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        };
        return Err(OcrFailure::new(
            status,
            "unsupported or undecodable image",
            json!({ "detail": error.to_string() }),
        ));
    }

    // One permit caps concurrent CPU/socket work across every engine.
    let _permit = state
        .ocr_semaphore
        .acquire()
        .await
        .map_err(|_| OcrFailure::new(StatusCode::SERVICE_UNAVAILABLE, "service is shutting down", json!({})))?;

    state.metrics.ocr_requests_total.fetch_add(1, Ordering::Relaxed);

    let result = match engine {
        Engine::Tesseract => run_tesseract(state, image, opts).await,
        Engine::Google => run_google(state, image, opts).await,
        Engine::AwsTextract => run_aws(state, image).await,
        Engine::Azure => run_azure(state, image).await,
    };
    if result.is_err() {
        state.metrics.ocr_errors_total.fetch_add(1, Ordering::Relaxed);
    }
    result
}

async fn run_tesseract(
    state: &AppState,
    image: &[u8],
    opts: &OcrOptions,
) -> Result<OcrSuccess, OcrFailure> {
    state.metrics.tesseract_total.fetch_add(1, Ordering::Relaxed);

    let bytes = image.to_vec();
    let binarize = opts.binarize;
    let min_dim = state.upscale_min_dim;
    let limits = state.decode_limits;
    let prepared =
        tokio::task::spawn_blocking(move || preprocess::prepare(&bytes, binarize, min_dim, limits))
            .await
            .map_err(|e| OcrFailure::new(StatusCode::INTERNAL_SERVER_ERROR, "preprocess task failed", json!({ "detail": e.to_string() })))?;
    let prepared = prepared.map_err(|e| {
        state.metrics.preprocess_errors_total.fetch_add(1, Ordering::Relaxed);
        // 415 for a recognised-but-disallowed container; 422 for undecodable or
        // limit-tripping bytes.
        let status = match e {
            preprocess::PreprocessError::UnsupportedFormat(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        };
        OcrFailure::new(status, "could not decode the image", json!({ "detail": e.to_string() }))
    })?;

    let (png, width, height, binarized) =
        (prepared.png, prepared.width, prepared.height, prepared.binarized);
    let languages = opts.languages.clone();
    let psm = opts.psm;
    let outcome = tokio::task::spawn_blocking(move || tesseract_ffi::recognize(&png, &languages, psm))
        .await
        .map_err(|e| OcrFailure::new(StatusCode::INTERNAL_SERVER_ERROR, "ocr task failed", json!({ "detail": e.to_string() })))?;
    let outcome = outcome.map_err(|e| {
        OcrFailure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "tesseract recognition failed",
            json!({ "detail": e.to_string() }),
        )
    })?;

    let confidence = if outcome.confidence >= 0 {
        Some(outcome.confidence as f64 / 100.0)
    } else {
        None
    };
    let text = outcome.text.trim_end().to_string();
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    Ok(OcrSuccess {
        engine: Engine::Tesseract,
        text,
        confidence,
        lines,
        width: Some(width),
        height: Some(height),
        binarized,
    })
}

async fn run_google(
    state: &AppState,
    image: &[u8],
    opts: &OcrOptions,
) -> Result<OcrSuccess, OcrFailure> {
    state.metrics.google_total.fetch_add(1, Ordering::Relaxed);
    let cfg = state
        .google
        .as_ref()
        .ok_or_else(|| cloud_failure(Engine::Google, CloudError::NotConfigured))?;
    let ocr = cloud::google_vision(&state.http, cfg, image, opts.document_mode)
        .await
        .map_err(|e| cloud_failure(Engine::Google, e))?;
    Ok(cloud_success(Engine::Google, ocr))
}

async fn run_aws(state: &AppState, image: &[u8]) -> Result<OcrSuccess, OcrFailure> {
    state.metrics.aws_total.fetch_add(1, Ordering::Relaxed);
    let cfg = state
        .aws
        .as_ref()
        .ok_or_else(|| cloud_failure(Engine::AwsTextract, CloudError::NotConfigured))?;
    let ocr = cloud::aws_textract(&state.http, cfg, image)
        .await
        .map_err(|e| cloud_failure(Engine::AwsTextract, e))?;
    Ok(cloud_success(Engine::AwsTextract, ocr))
}

async fn run_azure(state: &AppState, image: &[u8]) -> Result<OcrSuccess, OcrFailure> {
    state.metrics.azure_total.fetch_add(1, Ordering::Relaxed);
    let cfg = state
        .azure
        .as_ref()
        .ok_or_else(|| cloud_failure(Engine::Azure, CloudError::NotConfigured))?;
    let ocr = cloud::azure_read(&state.http, cfg, image)
        .await
        .map_err(|e| cloud_failure(Engine::Azure, e))?;
    Ok(cloud_success(Engine::Azure, ocr))
}

fn cloud_success(engine: Engine, ocr: CloudOcr) -> OcrSuccess {
    OcrSuccess {
        engine,
        text: ocr.text,
        confidence: ocr.confidence,
        lines: ocr.lines,
        width: None,
        height: None,
        binarized: false,
    }
}

fn success_json(rid: &str, success: &OcrSuccess) -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "requestId": rid,
        "engine": success.engine.as_str(),
        "text": success.text,
        "confidence": success.confidence,
        "lineCount": success.lines,
        "charCount": success.text.chars().count(),
        "width": success.width,
        "height": success.height,
        "binarized": success.binarized,
        "generatedAtMs": now_ms(),
    })
}

fn engine_availability(state: &AppState) -> Value {
    json!({
        "tesseract": state.tesseract_enabled,
        "google": state.google.is_some(),
        "aws-textract": state.aws.is_some(),
        "azure": state.azure.is_some(),
    })
}

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

async fn home() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "description": "Multi-backend OCR service (Tesseract + Google Vision / AWS Textract / Azure Read).",
        "endpoints": {
            "healthz": "/healthz",
            "status": "/status",
            "engines": "/engines",
            "capabilities": "/capabilities",
            "example": "/example",
            "ocr": "/ocr",
            "ocrStream": "/ocr/stream",
            "metrics": "/metrics",
            "docs": "/docs/api"
        }
    }))
}

async fn healthz() -> impl IntoResponse {
    Json(json!({ "ok": true, "service": SERVICE_NAME, "generatedAtMs": now_ms() }))
}

async fn status_http(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "engines": engine_availability(&state),
        "tesseractVersion": state.tesseract_version,
        "defaultLanguages": state.default_languages,
        "natsEnabled": state.nats.is_some(),
        "maxImageBytes": state.max_image_bytes,
        "generatedAtMs": now_ms(),
    }))
}

async fn engines_http(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "engines": [
            {
                "name": "tesseract",
                "kind": "local-open-source",
                "available": state.tesseract_enabled,
                "version": state.tesseract_version,
                "libraries": ["tesseract", "leptonica", "image", "imageproc"],
            },
            {
                "name": "google",
                "kind": "paid-cloud",
                "available": state.google.is_some(),
                "provider": "Google Cloud Vision",
            },
            {
                "name": "aws-textract",
                "kind": "paid-cloud",
                "available": state.aws.is_some(),
                "provider": "AWS Textract",
            },
            {
                "name": "azure",
                "kind": "paid-cloud",
                "available": state.azure.is_some(),
                "provider": "Azure AI Vision Read",
            }
        ],
        "auto": state.pick_auto().map(|e| e.as_str()),
        "generatedAtMs": now_ms(),
    }))
}

async fn capabilities_http(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "engines": engine_availability(&state),
        "inputFormats": ["png", "jpeg", "webp", "tiff", "bmp", "gif"],
        "maxImageBytes": state.max_image_bytes,
        "defaultLanguages": state.default_languages,
        "natsEnabled": state.nats.is_some(),
        "requestSubject": state.request_subject,
        "resultSubject": state.result_subject,
        "generatedAtMs": now_ms(),
    }))
}

async fn example_http() -> impl IntoResponse {
    // A 1x1 PNG — smallest valid payload that exercises the decode path.
    const TINY_PNG: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGP4DwABAQEAsTj2FAAAAABJRU5ErkJggg==";
    Json(json!({
        "schemaVersion": SCHEMA_VERSION,
        "requestId": "ocr-demo",
        "engine": "auto",
        "languages": "eng",
        "documentMode": true,
        "binarize": true,
        "imageBase64": TINY_PNG,
        "note": "POST this to /ocr, or send raw image bytes to /ocr/stream?engine=auto&lang=eng.",
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OcrRequest {
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    engine: Option<String>,
    /// Base64-encoded image bytes.
    image_base64: String,
    #[serde(default)]
    languages: Option<String>,
    /// Dense-document detection (Google) — defaults to true.
    #[serde(default)]
    document_mode: Option<bool>,
    /// Otsu-binarise before the local engine — defaults to true.
    #[serde(default)]
    binarize: Option<bool>,
    /// Tesseract page-segmentation mode (0..13).
    #[serde(default)]
    psm: Option<i32>,
}

fn options_from_request(
    state: &AppState,
    languages: Option<&str>,
    document_mode: Option<bool>,
    binarize: Option<bool>,
    psm: Option<i32>,
) -> OcrOptions {
    OcrOptions {
        languages: sanitize_languages(languages, &state.default_languages),
        document_mode: document_mode.unwrap_or(true),
        binarize: binarize.unwrap_or(true),
        psm: psm.filter(|p| (0..=13).contains(p)).unwrap_or(DEFAULT_PSM),
    }
}

/// Admission control: cap total concurrent HTTP OCR requests so waiters can't
/// pile up each holding a decoded image in memory. Returns a 503 when full; the
/// permit is held for the request's lifetime.
fn admit_http(state: &AppState) -> Result<tokio::sync::OwnedSemaphorePermit, Box<Response>> {
    state.http_inflight.clone().try_acquire_owned().map_err(|_| {
        state.metrics.http_shed_total.fetch_add(1, Ordering::Relaxed);
        Box::new(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "server is at capacity; retry shortly",
            json!({}),
        ))
    })
}

async fn ocr_http(State(state): State<AppState>, Json(req): Json<OcrRequest>) -> Response {
    let _permit = match admit_http(&state) {
        Ok(permit) => permit,
        Err(response) => return *response,
    };
    let rid = request_id(req.request_id.as_deref(), "ocr-http");
    // Resolve the engine before decoding the (potentially large) base64 body so
    // a bad/unavailable engine is rejected without the allocation.
    let engine = match resolve_engine(&state, req.engine.as_deref()) {
        Ok(engine) => engine,
        Err(failure) => return failure.into_response(),
    };
    let image = match BASE64.decode(req.image_base64.as_bytes()) {
        Ok(bytes) => bytes,
        Err(error) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "imageBase64 is not valid base64",
                json!({ "detail": error.to_string() }),
            );
        }
    };
    let opts = options_from_request(
        &state,
        req.languages.as_deref(),
        req.document_mode,
        req.binarize,
        req.psm,
    );
    finish_http(&state, &rid, engine, &image, &opts).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamQuery {
    #[serde(default)]
    engine: Option<String>,
    #[serde(default, alias = "languages")]
    lang: Option<String>,
    #[serde(default, alias = "request_id")]
    request_id: Option<String>,
    #[serde(default)]
    document_mode: Option<bool>,
    #[serde(default)]
    binarize: Option<bool>,
    #[serde(default)]
    psm: Option<i32>,
}

/// Raw-bytes entry point: the request body is the image; knobs ride the query
/// string. Convenient for piping a file straight in (`--data-binary @scan.png`).
async fn ocr_stream_http(
    State(state): State<AppState>,
    Query(query): Query<StreamQuery>,
    body: Bytes,
) -> Response {
    let _permit = match admit_http(&state) {
        Ok(permit) => permit,
        Err(response) => return *response,
    };
    let rid = request_id(query.request_id.as_deref(), "ocr-stream");
    let engine = match resolve_engine(&state, query.engine.as_deref()) {
        Ok(engine) => engine,
        Err(failure) => return failure.into_response(),
    };
    let opts = options_from_request(
        &state,
        query.lang.as_deref(),
        query.document_mode,
        query.binarize,
        query.psm,
    );
    finish_http(&state, &rid, engine, &body, &opts).await
}

async fn finish_http(
    state: &AppState,
    rid: &str,
    engine: Engine,
    image: &[u8],
    opts: &OcrOptions,
) -> Response {
    match run_ocr(state, engine, image, opts).await {
        Ok(success) => {
            let payload = success_json(rid, &success);
            publish_event(state, "ocr.recognize", rid, true).await;
            Json(payload).into_response()
        }
        Err(failure) => {
            // Server-side observability for failures (the client only sees the
            // HTTP response). Detail is redacted of credentials upstream.
            log_warn(
                "ocr-request-failed",
                "OCR request failed.",
                json!({
                    "requestId": rid,
                    "engine": engine.as_str(),
                    "status": failure.status.as_u16(),
                    "error": failure.message,
                    "details": failure.details,
                }),
            );
            publish_event(state, "ocr.recognize", rid, false).await;
            failure.into_response()
        }
    }
}

fn metrics_body(state: &AppState) -> String {
    let m = &state.metrics;
    let g = |v: &AtomicU64| v.load(Ordering::Relaxed);
    let mut out = String::new();
    let mut line = |name: &str, help: &str, value: u64| {
        out.push_str(&format!("# HELP {name} {help}\n# TYPE {name} counter\n{name} {value}\n"));
    };
    line("ddocr_requests_total", "Total OCR requests accepted.", g(&m.ocr_requests_total));
    line("ddocr_errors_total", "Total OCR requests that failed.", g(&m.ocr_errors_total));
    line("ddocr_engine_tesseract_total", "OCR requests routed to Tesseract.", g(&m.tesseract_total));
    line("ddocr_engine_google_total", "OCR requests routed to Google Vision.", g(&m.google_total));
    line("ddocr_engine_aws_total", "OCR requests routed to AWS Textract.", g(&m.aws_total));
    line("ddocr_engine_azure_total", "OCR requests routed to Azure Read.", g(&m.azure_total));
    line("ddocr_engine_unavailable_total", "Requests rejected for an unavailable engine.", g(&m.engine_unavailable_total));
    line("ddocr_image_rejected_total", "Images rejected (empty/too large/disallowed format).", g(&m.image_rejected_total));
    line("ddocr_http_shed_total", "HTTP requests shed at the admission cap (503).", g(&m.http_shed_total));
    line("ddocr_preprocess_errors_total", "Image decode/preprocess failures.", g(&m.preprocess_errors_total));
    line("ddocr_nats_messages_total", "NATS request messages received.", g(&m.nats_messages_total));
    line("ddocr_nats_payload_rejected_total", "NATS payloads rejected.", g(&m.nats_payload_rejected_total));
    line("ddocr_nats_results_published_total", "NATS result messages published.", g(&m.nats_results_published_total));
    line("ddocr_nats_events_published_total", "NATS lifecycle events published.", g(&m.nats_events_published_total));
    line("ddocr_nats_critical_events_published_total", "NATS critical events published.", g(&m.nats_critical_events_published_total));
    line("ddocr_nats_publish_errors_total", "NATS publish failures.", g(&m.nats_publish_errors_total));
    line("ddocr_internal_errors_total", "Internal (serialise/etc) errors.", g(&m.errors_total));
    out
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    (
        [("content-type", "text/plain; version=0.0.4")],
        metrics_body(&state),
    )
}

// ---------------------------------------------------------------------------
// NATS worker
// ---------------------------------------------------------------------------

async fn publish_value(state: &AppState, subject: &str, payload: &Value, kind: &str) {
    let Some(nats) = &state.nats else { return };
    let bytes = match serde_json::to_vec(payload) {
        Ok(bytes) => bytes,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            log_error(
                "ocr-nats-serialize-failed",
                "Could not serialize a NATS payload.",
                json!({ "kind": kind, "error": error.to_string() }),
            );
            return;
        }
    };
    match nats.publish(subject.to_string(), bytes.into()).await {
        Ok(_) => match kind {
            "result" => {
                state.metrics.nats_results_published_total.fetch_add(1, Ordering::Relaxed);
            }
            "critical" => {
                state.metrics.nats_critical_events_published_total.fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                state.metrics.nats_events_published_total.fetch_add(1, Ordering::Relaxed);
            }
        },
        Err(error) => {
            state.metrics.nats_publish_errors_total.fetch_add(1, Ordering::Relaxed);
            log_error(
                "ocr-nats-publish-failed",
                "NATS publish failed.",
                json!({ "subject": subject, "kind": kind, "error": error.to_string() }),
            );
        }
    }
}

async fn publish_event(state: &AppState, event_type: &str, request_id: &str, ok: bool) {
    if state.nats.is_none() {
        return;
    }
    let event = json!({
        "schema": "dd.event.v1",
        "service": SERVICE_NAME,
        "eventType": event_type,
        "requestId": request_id,
        "ok": ok,
        "generatedAtMs": now_ms(),
    });
    publish_value(state, &state.event_subject.clone(), &event, "event").await;
    if !ok {
        publish_value(state, &state.critical_event_subject.clone(), &event, "critical").await;
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NatsOcrRequest {
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    engine: Option<String>,
    image_base64: String,
    #[serde(default)]
    languages: Option<String>,
    #[serde(default)]
    document_mode: Option<bool>,
    #[serde(default)]
    binarize: Option<bool>,
    #[serde(default)]
    psm: Option<i32>,
}

async fn run_nats_loop(state: AppState) {
    let Some(nats) = state.nats.clone() else { return };
    loop {
        let subscription = nats
            .queue_subscribe(state.request_subject.clone(), OCR_QUEUE_GROUP.to_string())
            .await;
        let mut subscription = match subscription {
            Ok(subscription) => subscription,
            Err(error) => {
                log_error(
                    "ocr-nats-subscribe-failed",
                    "OCR service could not subscribe to requests; retrying in 5s.",
                    json!({ "subject": state.request_subject, "error": error.to_string() }),
                );
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        log_info(
            "ocr-nats-subscribed",
            "Listening for OCR requests over NATS.",
            json!({ "subject": state.request_subject, "queueGroup": OCR_QUEUE_GROUP }),
        );

        while let Some(message) = subscription.next().await {
            state.metrics.nats_messages_total.fetch_add(1, Ordering::Relaxed);
            if message.payload.len() > state.max_image_bytes / 3 * 4 + 64 * 1024 {
                state.metrics.nats_payload_rejected_total.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let request: NatsOcrRequest = match serde_json::from_slice(&message.payload) {
                Ok(request) => request,
                Err(error) => {
                    state.metrics.nats_payload_rejected_total.fetch_add(1, Ordering::Relaxed);
                    log_warn(
                        "ocr-nats-bad-payload",
                        "Discarded an unparseable OCR request.",
                        json!({ "error": error.to_string() }),
                    );
                    continue;
                }
            };
            // Bound in-flight jobs: a permit per spawned task caps how much
            // decoded-image memory can be queued, and sheds load (drops the
            // message) rather than letting a NATS flood OOM the pod.
            let permit = match state.nats_inflight.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    state.metrics.nats_payload_rejected_total.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };
            let state = state.clone();
            tokio::spawn(async move {
                let _permit = permit;
                handle_nats_request(&state, request).await;
            });
        }

        log_warn(
            "ocr-nats-subscription-ended",
            "OCR request subscription ended; re-subscribing in 5s.",
            json!({ "subject": state.request_subject }),
        );
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn handle_nats_request(state: &AppState, request: NatsOcrRequest) {
    let rid = request_id(request.request_id.as_deref(), "ocr-nats");
    let image = match BASE64.decode(request.image_base64.as_bytes()) {
        Ok(bytes) => bytes,
        Err(error) => {
            state.metrics.nats_payload_rejected_total.fetch_add(1, Ordering::Relaxed);
            log_warn(
                "ocr-nats-bad-image",
                "OCR request carried invalid base64 image data.",
                json!({ "requestId": rid, "error": error.to_string() }),
            );
            return;
        }
    };

    let (ok, payload) = match resolve_engine(state, request.engine.as_deref()) {
        Ok(engine) => {
            let opts = options_from_request(
                state,
                request.languages.as_deref(),
                request.document_mode,
                request.binarize,
                request.psm,
            );
            match run_ocr(state, engine, &image, &opts).await {
                Ok(success) => (true, success_json(&rid, &success)),
                Err(failure) => {
                    log_warn(
                        "ocr-nats-request-failed",
                        "OCR request over NATS failed.",
                        json!({
                            "requestId": rid,
                            "engine": engine.as_str(),
                            "status": failure.status.as_u16(),
                            "error": failure.message,
                        }),
                    );
                    (
                        false,
                        json!({
                            "ok": false,
                            "schemaVersion": SCHEMA_VERSION,
                            "requestId": rid,
                            "engine": engine.as_str(),
                            "error": failure.message,
                            "details": failure.details,
                            "generatedAtMs": now_ms(),
                        }),
                    )
                }
            }
        }
        Err(failure) => (
            false,
            json!({
                "ok": false,
                "schemaVersion": SCHEMA_VERSION,
                "requestId": rid,
                "error": failure.message,
                "details": failure.details,
                "generatedAtMs": now_ms(),
            }),
        ),
    };

    publish_value(state, &state.result_subject.clone(), &payload, "result").await;
    publish_event(state, "ocr.recognize", &rid, ok).await;
}

// ---------------------------------------------------------------------------
// Docs + lifecycle
// ---------------------------------------------------------------------------

async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl IntoResponse {
    (
        [("content-type", "application/json")],
        include_str!("../generated/api-docs.json"),
    )
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

fn load_google() -> Option<GoogleConfig> {
    env_opt("GOOGLE_VISION_API_KEY").map(|api_key| GoogleConfig { api_key })
}

/// A valid AWS region is lowercase alphanumerics + hyphens (e.g. `us-east-1`).
/// Validated because it is interpolated into the Textract hostname and the
/// SigV4 scope; a malformed value could otherwise redirect the signed request.
fn is_valid_aws_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 32
        && region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn load_aws() -> Option<AwsConfig> {
    let access_key_id = env_opt("AWS_ACCESS_KEY_ID")?;
    let secret_access_key = env_opt("AWS_SECRET_ACCESS_KEY")?;
    let region = env_value("AWS_REGION", "us-east-1");
    if !is_valid_aws_region(&region) {
        log_warn(
            "ocr-aws-region-rejected",
            "AWS_REGION is malformed; the aws-textract engine stays disabled.",
            json!({}),
        );
        return None;
    }
    Some(AwsConfig {
        access_key_id,
        secret_access_key,
        session_token: env_opt("AWS_SESSION_TOKEN"),
        region,
    })
}

/// The Azure endpoint is operator-supplied and interpolated into the request
/// URL; require a clean `https://host` with no path/query/fragment or
/// whitespace so it can't be steered to an attacker-chosen target.
fn is_safe_azure_endpoint(endpoint: &str) -> bool {
    let lower = endpoint.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("https://") else {
        return false;
    };
    endpoint.len() <= 256
        && !rest.is_empty()
        && !endpoint.chars().any(|c| c.is_whitespace() || c.is_control())
        // Only scheme://host[:port] — reject anything that adds a path, query,
        // fragment, userinfo, or extra scheme.
        && !rest.contains(['/', '?', '#', '@', '\\'])
}

fn load_azure() -> Option<AzureConfig> {
    let endpoint = env_opt("AZURE_VISION_ENDPOINT")?;
    let key = env_opt("AZURE_VISION_KEY")?;
    // Require a clean https host so the subscription key is never sent in clear
    // text and the request can't be steered off a plain-HTTP/redirect/path trick.
    if !is_safe_azure_endpoint(&endpoint) {
        log_warn(
            "ocr-azure-endpoint-rejected",
            "AZURE_VISION_ENDPOINT is not a clean https host; the azure engine stays disabled.",
            json!({}),
        );
        return None;
    }
    Some(AzureConfig { endpoint, key })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _otel = dd_telemetry::init("dd-ocr-rs");

    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8123");
    let max_image_bytes = env_usize("OCR_MAX_IMAGE_BYTES", DEFAULT_MAX_IMAGE_BYTES);
    let default_languages = sanitize_languages(env_opt("OCR_DEFAULT_LANGUAGES").as_deref(), DEFAULT_LANGUAGES);
    let upscale_min_dim = env::var("OCR_UPSCALE_MIN_DIM")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let default_concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 16);
    let ocr_concurrency = env_usize("OCR_CONCURRENCY", default_concurrency.max(DEFAULT_OCR_CONCURRENCY));
    // Total concurrent in-flight HTTP OCR requests (running + queued on the
    // engine semaphore). Bounds buffered decoded-image memory; excess sheds 503.
    let max_inflight = env_usize("OCR_MAX_INFLIGHT", ocr_concurrency.saturating_mul(8).max(16));
    let request_timeout = env_usize("OCR_HTTP_TIMEOUT_SECS", 30) as u64;

    // Decode guards for the local path: cap pixel dimensions and the decoder's
    // intermediate allocation so a small "bomb" payload can't exhaust memory.
    // Conservative relative to the pod memory limit: a 10k-px side is already
    // far larger than any real document scan, and the 256MB decode-allocation
    // cap keeps a few concurrent jobs well inside the container budget.
    let max_image_dim = env::var("OCR_MAX_IMAGE_DIM")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(10_000);
    let max_decode_alloc =
        env_usize("OCR_MAX_DECODE_ALLOC_BYTES", 256 * 1024 * 1024) as u64;
    let decode_limits = preprocess::DecodeLimits {
        max_dim: max_image_dim,
        max_alloc_bytes: max_decode_alloc,
    };

    let request_subject = env_value("OCR_REQUEST_SUBJECT", DEFAULT_REQUEST_SUBJECT);
    let result_subject = env_value("OCR_RESULT_SUBJECT", DEFAULT_RESULT_SUBJECT);
    let event_subject = env_value("OCR_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT);
    let critical_event_subject =
        env_value("NATS_CRITICAL_EVENT_SUBJECT", RUNTIME_CRITICAL_EVENTS_SUBJECT);

    let tesseract_enabled = tesseract_ffi::tesseract_enabled();
    let tesseract_version = if tesseract_enabled {
        tesseract_ffi::version()
    } else {
        None
    };

    let google = load_google();
    let aws = load_aws();
    let azure = load_azure();

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(request_timeout))
        .connect_timeout(Duration::from_secs(10))
        // Cloud backends are fixed HTTPS endpoints: forbid plaintext and don't
        // chase redirects (defends the operator-supplied Azure endpoint against
        // a downgrade/SSRF redirect).
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("dd-ocr-rs/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| -> Box<dyn Error> { format!("failed to build HTTP client: {e}").into() })?;

    let nats_url = env_opt("NATS_URL");
    let nats = match nats_url {
        Some(url) => match async_nats::connect(url.clone()).await {
            Ok(client) => Some(client),
            Err(error) => {
                log_error(
                    "ocr-nats-connect-failed",
                    "OCR service failed to connect to NATS.",
                    json!({ "url": url, "error": error.to_string() }),
                );
                None
            }
        },
        None => None,
    };

    let state = AppState {
        nats,
        http,
        request_subject,
        result_subject,
        event_subject,
        critical_event_subject,
        max_image_bytes,
        decode_limits,
        default_languages,
        upscale_min_dim,
        tesseract_enabled,
        tesseract_version,
        google,
        aws,
        azure,
        ocr_semaphore: Arc::new(Semaphore::new(ocr_concurrency.max(1))),
        // Cap queued NATS jobs (and therefore buffered decoded-image memory) at
        // a few times the active concurrency; excess messages are shed.
        nats_inflight: Arc::new(Semaphore::new(ocr_concurrency.saturating_mul(4).max(4))),
        http_inflight: Arc::new(Semaphore::new(max_inflight)),
        metrics: Arc::new(Metrics::default()),
    };

    log_info(
        "ocr-service-starting",
        "OCR service runtime configuration loaded.",
        json!({
            "tesseractEnabled": state.tesseract_enabled,
            "tesseractVersion": state.tesseract_version,
            "googleConfigured": state.google.is_some(),
            "awsConfigured": state.aws.is_some(),
            "azureConfigured": state.azure.is_some(),
            "maxImageBytes": state.max_image_bytes,
            "defaultLanguages": state.default_languages,
            "ocrConcurrency": ocr_concurrency,
            "requestSubject": state.request_subject,
            "resultSubject": state.result_subject,
            "natsEnabled": state.nats.is_some(),
        }),
    );

    if state.nats.is_some() {
        tokio::spawn(run_nats_loop(state.clone()));
    }

    // JSON body limit covers a base64-encoded image (~4/3 the raw bytes) plus
    // envelope slack; the raw stream route carries the same ceiling on bytes.
    let json_limit = state.max_image_bytes / 3 * 4 + 64 * 1024;
    let stream_routes = Router::new()
        .route("/ocr/stream", post(ocr_stream_http))
        .layer(DefaultBodyLimit::max(state.max_image_bytes + 64 * 1024));

    let app = Router::new()
        .route("/", get(home))
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/status", get(status_http))
        .route("/engines", get(engines_http))
        .route("/capabilities", get(capabilities_http))
        .route("/example", get(example_http))
        .route("/ocr", post(ocr_http))
        .layer(DefaultBodyLimit::max(json_limit))
        .merge(stream_routes)
        .with_state(state)
        .merge(dd_runtime_config_client::router());
    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let address: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    log_info(
        "ocr-service-listening",
        "OCR service HTTP listener is ready.",
        json!({ "address": address.to_string() }),
    );
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_parsing_accepts_aliases() {
        assert_eq!(Engine::parse("tesseract"), Some(Engine::Tesseract));
        assert_eq!(Engine::parse("LOCAL"), Some(Engine::Tesseract));
        assert_eq!(Engine::parse("google-vision"), Some(Engine::Google));
        assert_eq!(Engine::parse("textract"), Some(Engine::AwsTextract));
        assert_eq!(Engine::parse("azure-read"), Some(Engine::Azure));
        assert_eq!(Engine::parse("nope"), None);
    }

    #[test]
    fn languages_are_sanitised() {
        assert_eq!(sanitize_languages(Some("eng+deu"), "eng"), "eng+deu");
        assert_eq!(sanitize_languages(Some("../../etc/passwd"), "eng"), "etcpasswd");
        assert_eq!(sanitize_languages(Some("  "), "eng"), "eng");
        assert_eq!(sanitize_languages(None, "eng"), "eng");
        assert_eq!(sanitize_languages(Some(";rm -rf"), "eng"), "rm-rf");
    }

    #[test]
    fn request_id_is_bounded_and_clean() {
        let raw = format!("a{}\nb", "x".repeat(500));
        let id = request_id(Some(&raw), "fallback");
        assert!(id.len() <= MAX_REQUEST_ID_LEN);
        assert!(!id.contains('\n'));
        assert_eq!(request_id(None, "fallback"), "fallback");
    }

    #[test]
    fn auto_order_prefers_local_then_cloud() {
        assert_eq!(Engine::AUTO_ORDER[0], Engine::Tesseract);
        assert_eq!(Engine::AUTO_ORDER.last().copied(), Some(Engine::AwsTextract));
    }

    #[test]
    fn aws_region_validation() {
        assert!(is_valid_aws_region("us-east-1"));
        assert!(is_valid_aws_region("ap-southeast-2"));
        assert!(!is_valid_aws_region(""));
        assert!(!is_valid_aws_region("us-east-1.evil.com"));
        assert!(!is_valid_aws_region("us east 1"));
        assert!(!is_valid_aws_region("US-EAST-1"));
        assert!(!is_valid_aws_region(&"a".repeat(33)));
    }

    #[test]
    fn azure_endpoint_validation() {
        assert!(is_safe_azure_endpoint("https://my-vision.cognitiveservices.azure.com"));
        assert!(is_safe_azure_endpoint("https://host:443"));
        assert!(!is_safe_azure_endpoint("http://my-vision.cognitiveservices.azure.com"));
        assert!(!is_safe_azure_endpoint("https://evil.com/redirect?x=1"));
        assert!(!is_safe_azure_endpoint("https://user@evil.com"));
        assert!(!is_safe_azure_endpoint("https://host/path"));
        assert!(!is_safe_azure_endpoint("https://host\n"));
        assert!(!is_safe_azure_endpoint("ftp://host"));
        assert!(!is_safe_azure_endpoint("https://"));
    }
}
