//! `dd-document-rs` — document manipulation service.
//!
//! A thin Rust/axum service in front of the Pandoc Haskell SDK, reached over FFI
//! through `libdd-pandoc-bridge.so` (see [`ffi`]). It converts documents between
//! Pandoc's text formats, round-trips through the Pandoc JSON AST, and reports
//! basic document structure — without spawning the `pandoc` CLI.

mod ffi;
mod image_ffi;

use std::{
    collections::{HashMap, VecDeque},
    env,
    error::Error,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use dd_nats_subject_defs::{RUNTIME_CRITICAL_EVENTS_SUBJECT, RUNTIME_EVENTS_SUBJECT};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Semaphore;

const SERVICE_NAME: &str = "dd-document-rs";
const SERVICE_NAMESPACE: &str = "remote-dev";
const LOG_SCHEMA: &str = "dd.log.v1";
const LOG_SCOPE: &str = "dd-document-rs";
const SCHEMA_VERSION: &str = "dd.document.v1";

const DEFAULT_CONVERT_SUBJECT: &str = "dd.remote.document.convert";
const DEFAULT_RESULT_SUBJECT: &str = "dd.remote.document.results";
const CONVERT_QUEUE_GROUP: &str = "dd-document-rs";
const JSON_FORMAT: &str = "json";

const DEFAULT_MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
// Streaming routes take raw binary bodies and carry a higher ceiling for the
// heavy/big-document work that the warm container-pool worker handles.
const DEFAULT_MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_CACHE_CAPACITY: usize = 256;
const DEFAULT_CACHE_MAX_ENTRY_BYTES: usize = 1024 * 1024;
const DEFAULT_IMAGE_CONCURRENCY: usize = 4;
const MAX_FORMAT_LEN: usize = 64;
const MAX_REQUEST_ID_LEN: usize = 128;
const MAX_RESIZE_GEOMETRY_LEN: usize = 32;
const MAX_BACKGROUND_LEN: usize = 32;

// Image encoders we allow on output. Decoded input coders are additionally
// screened by the Magick++ bridge and the shipped ImageMagick policy.xml.
const IMAGE_OUTPUT_FORMATS: &[&str] = &[
    "png", "jpeg", "jpg", "webp", "gif", "bmp", "tiff", "tif", "avif", "heic", "ico", "pnm",
    "ppm", "pgm", "pbm", "tga",
];

// Text-in / text-out Pandoc formats this bridge can drive. Format strings may
// carry extensions (e.g. `markdown+hard_line_breaks`); only the base name is
// matched against these lists.
const TEXT_READERS: &[&str] = &[
    "markdown", "markdown_strict", "markdown_phpextra", "markdown_mmd", "commonmark",
    "commonmark_x", "gfm", "html", "latex", "rst", "org", "mediawiki", "dokuwiki", "textile",
    "json", "native", "docbook", "jats", "man", "muse", "creole", "tikiwiki", "twiki", "vimwiki",
    "t2t", "haddock", "csv", "tsv", "bibtex", "biblatex", "ris", "endnotexml", "fb2", "opml",
    "jira", "ipynb", "typst", "djot", "rtf",
];
const TEXT_WRITERS: &[&str] = &[
    "markdown", "markdown_strict", "markdown_phpextra", "markdown_mmd", "commonmark",
    "commonmark_x", "gfm", "html", "html5", "html4", "latex", "beamer", "context", "rst", "org",
    "mediawiki", "dokuwiki", "zimwiki", "xwiki", "jira", "textile", "json", "native", "plain",
    "asciidoc", "asciidoctor", "texinfo", "man", "ms", "muse", "docbook", "docbook5", "jats",
    "opml", "ipynb", "typst", "djot", "markua", "tei", "fb2", "icml", "opendocument", "haddock",
    "rtf", "slidy", "slideous", "dzslides", "revealjs", "s5",
];
// Binary (non-text) Pandoc formats, handled via base64/stream endpoints.
// Readers Pandoc can parse from bytes; writers it can emit as bytes. (pdf needs
// an external engine and is handled separately, opt-in.)
const BINARY_READERS: &[&str] = &["docx", "odt", "epub"];
const BINARY_WRITERS: &[&str] = &["docx", "odt", "pptx", "epub", "epub2", "epub3"];

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    convert_subject: String,
    result_subject: String,
    event_subject: String,
    critical_event_subject: String,
    max_input_bytes: usize,
    max_output_bytes: usize,
    max_image_bytes: usize,
    max_stream_bytes: usize,
    bridge_enabled: bool,
    pandoc_version: Option<String>,
    image_enabled: bool,
    magick_version: Option<String>,
    pdf_enabled: bool,
    cache: Arc<Mutex<ConvCache>>,
    image_semaphore: Arc<Semaphore>,
    convert_semaphore: Arc<Semaphore>,
    metrics: Arc<Metrics>,
}

type CacheKey = [u8; 32];

/// Tiny capacity-bounded FIFO cache of conversion outputs, keyed by a SHA-256 of
/// the full request (from/to/standalone/metadata/content). Conversions are
/// deterministic + expensive, so this dedupes repeated work cheaply. A
/// cryptographic key prevents an attacker from crafting a colliding request that
/// would serve the wrong cached document.
struct ConvCache {
    capacity: usize,
    /// Per-entry byte cap so a few large outputs can't blow up memory
    /// (capacity * max_entry_bytes bounds total footprint).
    max_entry_bytes: usize,
    map: HashMap<CacheKey, Arc<Vec<u8>>>,
    order: VecDeque<CacheKey>,
}

impl ConvCache {
    fn new(capacity: usize, max_entry_bytes: usize) -> Self {
        ConvCache {
            capacity,
            max_entry_bytes,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: &CacheKey) -> Option<Arc<Vec<u8>>> {
        self.map.get(key).cloned()
    }

    fn put(&mut self, key: CacheKey, value: Arc<Vec<u8>>) {
        if self.capacity == 0
            || value.len() > self.max_entry_bytes
            || self.map.contains_key(&key)
        {
            return;
        }
        while self.map.len() >= self.capacity {
            if let Some(evict) = self.order.pop_front() {
                self.map.remove(&evict);
            } else {
                break;
            }
        }
        self.order.push_back(key);
        self.map.insert(key, value);
    }
}

fn conversion_cache_key(
    from: &str,
    to: &str,
    standalone: bool,
    metadata: &Value,
    content: &[u8],
) -> CacheKey {
    use sha2::{Digest, Sha256};
    // Length-prefix every field so distinct requests can't alias (e.g.
    // from="a",to="b" vs from="ab",to="").
    let mut hasher = Sha256::new();
    let field = |h: &mut Sha256, bytes: &[u8]| {
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    };
    field(&mut hasher, from.as_bytes());
    field(&mut hasher, to.as_bytes());
    hasher.update([u8::from(standalone)]);
    field(&mut hasher, metadata.to_string().as_bytes());
    field(&mut hasher, content);
    hasher.finalize().into()
}

#[derive(Default)]
struct Metrics {
    conversions_total: AtomicU64,
    conversion_errors_total: AtomicU64,
    ast_reads_total: AtomicU64,
    ast_writes_total: AtomicU64,
    inspections_total: AtomicU64,
    validations_total: AtomicU64,
    format_rejections_total: AtomicU64,
    output_too_large_total: AtomicU64,
    cache_hits_total: AtomicU64,
    cache_misses_total: AtomicU64,
    pdf_total: AtomicU64,
    pdf_errors_total: AtomicU64,
    bridge_errors_total: AtomicU64,
    image_transforms_total: AtomicU64,
    image_identifies_total: AtomicU64,
    image_errors_total: AtomicU64,
    image_rejections_total: AtomicU64,
    nats_messages_total: AtomicU64,
    nats_payload_rejected_total: AtomicU64,
    nats_results_published_total: AtomicU64,
    nats_events_published_total: AtomicU64,
    nats_critical_events_published_total: AtomicU64,
    nats_publish_errors_total: AtomicU64,
    errors_total: AtomicU64,
}

// ---------------------------------------------------------------------------
// Small shared helpers (logging, env, time) — mirrors the other dd-*-rs
// services so the structured log shape stays consistent across the fleet.
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
        eprintln!("{line}");
    } else {
        println!("{line}");
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

// ---------------------------------------------------------------------------
// Format catalog helpers
// ---------------------------------------------------------------------------

/// Base format name without any `+ext`/`-ext` modifiers.
fn format_base(fmt: &str) -> &str {
    let end = fmt.find(['+', '-']).unwrap_or(fmt.len());
    &fmt[..end]
}

fn is_binary_format(fmt: &str) -> bool {
    matches!(
        format_base(fmt),
        "docx" | "odt" | "pptx" | "epub" | "epub2" | "epub3" | "pdf"
    )
}

fn is_text_reader(fmt: &str) -> bool {
    TEXT_READERS.contains(&format_base(fmt))
}

fn is_text_writer(fmt: &str) -> bool {
    TEXT_WRITERS.contains(&format_base(fmt))
}

fn is_known_reader(fmt: &str) -> bool {
    is_text_reader(fmt) || BINARY_READERS.contains(&format_base(fmt))
}

fn is_known_writer(fmt: &str) -> bool {
    is_text_writer(fmt) || BINARY_WRITERS.contains(&format_base(fmt))
}

/// Validate a `from`/`to` format. With `allow_binary` false, only text formats
/// pass (the JSON `/convert` path); with it true, binary formats pass too (the
/// base64 / streaming paths).
fn check_format(
    role: &str,
    fmt: &str,
    reader: bool,
    allow_binary: bool,
) -> Result<(), (String, Value)> {
    if fmt.is_empty() {
        return Err((format!("{role} format is required"), json!({ "role": role })));
    }
    if fmt.len() > MAX_FORMAT_LEN {
        return Err((
            format!("{role} format is too long"),
            json!({ "role": role, "maxLen": MAX_FORMAT_LEN }),
        ));
    }
    let known = match (reader, allow_binary) {
        (true, false) => is_text_reader(fmt),
        (true, true) => is_known_reader(fmt),
        (false, false) => is_text_writer(fmt),
        (false, true) => is_known_writer(fmt),
    };
    if !known {
        let hint = if !allow_binary && is_binary_format(fmt) {
            "; use /convert-binary or /stream/convert for binary formats"
        } else {
            ""
        };
        return Err((
            format!("{role} format '{fmt}' is not a recognized Pandoc format{hint}"),
            json!({ "role": role, "format": fmt }),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ConvertRequest {
    #[serde(default)]
    request_id: Option<String>,
    from: String,
    to: String,
    content: String,
    /// Wrap output in the format's default template (title page, head, etc.).
    #[serde(default)]
    standalone: Option<bool>,
    /// Document metadata (title/author/date/...) injected before writing.
    #[serde(default)]
    metadata: Option<Value>,
}

#[derive(Deserialize)]
struct ConvertBinaryRequest {
    #[serde(default)]
    request_id: Option<String>,
    from: String,
    to: String,
    /// Base64-encoded input bytes (text or binary formats).
    content_base64: String,
    #[serde(default)]
    standalone: Option<bool>,
    #[serde(default)]
    metadata: Option<Value>,
}

#[derive(Deserialize)]
struct PdfRequest {
    #[serde(default)]
    request_id: Option<String>,
    from: String,
    /// Base64-encoded input bytes. Output is always PDF.
    content_base64: String,
    #[serde(default)]
    standalone: Option<bool>,
    #[serde(default)]
    metadata: Option<Value>,
}

#[derive(Deserialize)]
struct ToAstRequest {
    #[serde(default)]
    request_id: Option<String>,
    from: String,
    content: String,
}

#[derive(Deserialize)]
struct FromAstRequest {
    #[serde(default)]
    request_id: Option<String>,
    to: String,
    content: String,
}

#[derive(Deserialize)]
struct InspectRequest {
    #[serde(default)]
    request_id: Option<String>,
    from: String,
    content: String,
}

fn request_id(input: Option<&String>, prefix: &str) -> String {
    let raw = input
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .unwrap_or(prefix);
    // Bound length and drop control characters so a caller-supplied id can't
    // bloat logs or inject newlines into structured output.
    raw.chars()
        .filter(|c| !c.is_control())
        .take(MAX_REQUEST_ID_LEN)
        .collect()
}

// ---------------------------------------------------------------------------
// Core conversion path (runs the blocking FFI on a worker thread)
// ---------------------------------------------------------------------------

/// Run a single Pandoc conversion off the async runtime. Returns the raw
/// converted bytes, or a `Response` error ready to return.
///
/// `allow_binary` admits binary formats (docx/odt/pptx/epub) for the base64 and
/// streaming paths; the plain JSON `/convert` path keeps it false (text only).
async fn run_conversion(
    state: &AppState,
    from: &str,
    to: &str,
    content: &[u8],
    standalone: bool,
    metadata: Value,
) -> Result<Arc<Vec<u8>>, Response> {
    let cap = state.max_stream_bytes;
    run_conversion_inner(state, from, to, content, standalone, metadata, true, cap).await
}

async fn run_conversion_text(
    state: &AppState,
    from: &str,
    to: &str,
    content: &[u8],
    standalone: bool,
    metadata: Value,
) -> Result<Arc<Vec<u8>>, Response> {
    let cap = state.max_input_bytes;
    run_conversion_inner(state, from, to, content, standalone, metadata, false, cap).await
}

#[allow(clippy::too_many_arguments)]
async fn run_conversion_inner(
    state: &AppState,
    from: &str,
    to: &str,
    content: &[u8],
    standalone: bool,
    metadata: Value,
    allow_binary: bool,
    input_cap: usize,
) -> Result<Arc<Vec<u8>>, Response> {
    if !state.bridge_enabled {
        state.metrics.bridge_errors_total.fetch_add(1, Ordering::Relaxed);
        return Err(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "pandoc bridge is not available in this build",
            json!({ "bridgeEnabled": false }),
        ));
    }
    if content.len() > input_cap {
        state
            .metrics
            .format_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(json_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "content exceeds the configured size limit",
            json!({ "maxInputBytes": input_cap, "contentBytes": content.len() }),
        ));
    }
    if let Err((message, details)) = check_format("from", from, true, allow_binary) {
        state
            .metrics
            .format_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(json_error(StatusCode::BAD_REQUEST, message, details));
    }
    if let Err((message, details)) = check_format("to", to, false, allow_binary) {
        state
            .metrics
            .format_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(json_error(StatusCode::BAD_REQUEST, message, details));
    }

    // Cache lookup (deterministic conversions).
    let key = conversion_cache_key(from, to, standalone, &metadata, content);
    if let Some(hit) = state
        .cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
    {
        state.metrics.cache_hits_total.fetch_add(1, Ordering::Relaxed);
        return Ok(hit);
    }
    state.metrics.cache_misses_total.fetch_add(1, Ordering::Relaxed);

    let (from_s, to_s, content_v) = (from.to_string(), to.to_string(), content.to_vec());
    // Bound concurrent CPU-heavy conversions (tokio's blocking pool alone would
    // allow hundreds, exhausting CPU/memory).
    let _permit = state.convert_semaphore.clone().acquire_owned().await;
    let outcome = tokio::task::spawn_blocking(move || {
        ffi::convert(&from_s, &to_s, &content_v, standalone, &metadata)
    })
    .await
    .map_err(|join_err| {
        json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "conversion worker failed",
            json!({ "error": join_err.to_string() }),
        )
    })?;

    match outcome {
        Ok(outcome) if outcome.ok => {
            let output = outcome.output.unwrap_or_default();
            if output.len() > state.max_output_bytes {
                state
                    .metrics
                    .output_too_large_total
                    .fetch_add(1, Ordering::Relaxed);
                return Err(json_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "converted document exceeds the configured output size limit",
                    json!({ "maxOutputBytes": state.max_output_bytes, "outputBytes": output.len() }),
                ));
            }
            state.metrics.conversions_total.fetch_add(1, Ordering::Relaxed);
            let output = Arc::new(output);
            state
                .cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .put(key, output.clone());
            Ok(output)
        }
        Ok(outcome) => {
            state
                .metrics
                .conversion_errors_total
                .fetch_add(1, Ordering::Relaxed);
            Err(json_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "pandoc could not convert the document",
                json!({ "pandocError": outcome.error.unwrap_or_default() }),
            ))
        }
        Err(error) => {
            state
                .metrics
                .bridge_errors_total
                .fetch_add(1, Ordering::Relaxed);
            Err(json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "pandoc bridge call failed",
                json!({ "error": error.to_string() }),
            ))
        }
    }
}

/// Best-effort MIME type for a Pandoc output format (used by streaming routes).
fn output_content_type(to: &str) -> &'static str {
    match format_base(to) {
        "html" | "html5" | "html4" | "slidy" | "slideous" | "dzslides" | "revealjs" | "s5" => {
            "text/html; charset=utf-8"
        }
        "json" => "application/json",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "epub" | "epub2" | "epub3" => "application/epub+zip",
        "pdf" => "application/pdf",
        "rtf" => "application/rtf",
        _ => "text/plain; charset=utf-8",
    }
}

/// Decode bytes as UTF-8 text or fail with a 422 (for text-output responses).
fn output_as_text(bytes: &[u8]) -> Result<String, Response> {
    String::from_utf8(bytes.to_vec()).map_err(|_| {
        json_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "output is binary; use /convert-binary or /stream/convert",
            json!({}),
        )
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
        "description": "Pandoc-backed document manipulation service (Rust + Haskell FFI).",
        "endpoints": {
            "healthz": "/healthz",
            "status": "/status",
            "formats": "/formats",
            "capabilities": "/capabilities",
            "example": "/example",
            "convert": "/convert",
            "convertBinary": "/convert-binary",
            "convertPdf": "/convert-pdf",
            "streamConvert": "/stream/convert",
            "toAst": "/to-ast",
            "fromAst": "/from-ast",
            "inspect": "/inspect",
            "validate": "/validate",
            "imageConvert": "/image/convert",
            "imageTransform": "/image/transform",
            "imageIdentify": "/image/identify",
            "imageFormats": "/image/formats",
            "streamImage": "/stream/image",
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
        "bridgeEnabled": state.bridge_enabled,
        "pandocVersion": state.pandoc_version,
        "imageEnabled": state.image_enabled,
        "magickVersion": state.magick_version,
        "pdfEnabled": state.pdf_enabled,
        "natsEnabled": state.nats.is_some(),
        "maxInputBytes": state.max_input_bytes,
        "maxOutputBytes": state.max_output_bytes,
        "maxImageBytes": state.max_image_bytes,
        "maxStreamBytes": state.max_stream_bytes,
        "generatedAtMs": now_ms(),
    }))
}

async fn formats_http() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "readers": TEXT_READERS,
        "writers": TEXT_WRITERS,
        "binaryReaders": BINARY_READERS,
        "binaryWriters": BINARY_WRITERS,
        "note": "Text formats use /convert (JSON). Binary formats (docx/odt/pptx/epub) use /convert-binary (base64) or /stream/convert (raw). Format names may carry Pandoc extensions, e.g. markdown+hard_line_breaks.",
        "generatedAtMs": now_ms(),
    }))
}

async fn capabilities_http(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "bridgeEnabled": state.bridge_enabled,
        "pandocVersion": state.pandoc_version,
        "imageEnabled": state.image_enabled,
        "magickVersion": state.magick_version,
        "pdfEnabled": state.pdf_enabled,
        "maxInputBytes": state.max_input_bytes,
        "maxOutputBytes": state.max_output_bytes,
        "maxImageBytes": state.max_image_bytes,
        "readerCount": TEXT_READERS.len(),
        "writerCount": TEXT_WRITERS.len(),
        "imageFormatCount": IMAGE_OUTPUT_FORMATS.len(),
        "astFormat": JSON_FORMAT,
        "natsEnabled": state.nats.is_some(),
        "convertSubject": state.convert_subject,
        "resultSubject": state.result_subject,
        "generatedAtMs": now_ms(),
    }))
}

async fn example_http() -> impl IntoResponse {
    Json(json!({
        "schemaVersion": SCHEMA_VERSION,
        "requestId": "document-demo",
        "from": "markdown",
        "to": "html",
        "content": "# Hello\n\nA *small* Pandoc document with a [link](https://pandoc.org).\n"
    }))
}

async fn convert_http(
    State(state): State<AppState>,
    Json(req): Json<ConvertRequest>,
) -> Response {
    let rid = request_id(req.request_id.as_ref(), "document-convert");
    let standalone = req.standalone.unwrap_or(false);
    let metadata = req.metadata.clone().unwrap_or_else(|| json!({}));
    match run_conversion_text(&state, &req.from, &req.to, req.content.as_bytes(), standalone, metadata)
        .await
    {
        Ok(output) => match output_as_text(&output) {
            Ok(text) => Json(json!({
                "ok": true,
                "requestId": rid,
                "from": req.from,
                "to": req.to,
                "outputBytes": text.len(),
                "output": text,
                "generatedAtMs": now_ms(),
            }))
            .into_response(),
            Err(response) => response,
        },
        Err(response) => response,
    }
}

async fn convert_binary_http(
    State(state): State<AppState>,
    Json(req): Json<ConvertBinaryRequest>,
) -> Response {
    let rid = request_id(req.request_id.as_ref(), "document-convert-binary");
    let content = match BASE64.decode(req.content_base64.trim()) {
        Ok(bytes) => bytes,
        Err(error) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "content_base64 is not valid base64",
                json!({ "error": error.to_string() }),
            )
        }
    };
    let standalone = req.standalone.unwrap_or(false);
    let metadata = req.metadata.clone().unwrap_or_else(|| json!({}));
    match run_conversion(&state, &req.from, &req.to, &content, standalone, metadata).await {
        Ok(output) => Json(json!({
            "ok": true,
            "requestId": rid,
            "from": req.from,
            "to": req.to,
            "outputBytes": output.len(),
            "content_base64": BASE64.encode(output.as_slice()),
            "generatedAtMs": now_ms(),
        }))
        .into_response(),
        Err(response) => response,
    }
}

/// Render a document to PDF via the Typst engine (opt-in). Returns raw PDF
/// bytes, or a ready `Response` error. Shares the conversion cache.
async fn run_pdf(
    state: &AppState,
    from: &str,
    content: &[u8],
    standalone: bool,
    metadata: Value,
) -> Result<Arc<Vec<u8>>, Response> {
    if !state.bridge_enabled {
        state.metrics.bridge_errors_total.fetch_add(1, Ordering::Relaxed);
        return Err(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "pandoc bridge is not available in this build",
            json!({ "bridgeEnabled": false }),
        ));
    }
    if !state.pdf_enabled {
        return Err(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "pdf engine (typst) is not installed in this image; build with --build-arg PDF_ENGINE=typst",
            json!({ "pdfEnabled": false }),
        ));
    }
    if content.len() > state.max_stream_bytes {
        state.metrics.format_rejections_total.fetch_add(1, Ordering::Relaxed);
        return Err(json_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "content exceeds the configured size limit",
            json!({ "maxInputBytes": state.max_stream_bytes, "contentBytes": content.len() }),
        ));
    }
    if let Err((message, details)) = check_format("from", from, true, true) {
        state.metrics.format_rejections_total.fetch_add(1, Ordering::Relaxed);
        return Err(json_error(StatusCode::BAD_REQUEST, message, details));
    }

    let key = conversion_cache_key(from, "pdf", standalone, &metadata, content);
    if let Some(hit) = state.cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        state.metrics.cache_hits_total.fetch_add(1, Ordering::Relaxed);
        return Ok(hit);
    }
    state.metrics.cache_misses_total.fetch_add(1, Ordering::Relaxed);

    let (from_s, content_v) = (from.to_string(), content.to_vec());
    let _permit = state.convert_semaphore.clone().acquire_owned().await;
    let outcome = tokio::task::spawn_blocking(move || {
        ffi::make_pdf(&from_s, &content_v, standalone, &metadata)
    })
    .await
    .map_err(|join_err| {
        json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "pdf worker failed",
            json!({ "error": join_err.to_string() }),
        )
    })?;

    match outcome {
        Ok(outcome) if outcome.ok => {
            let output = outcome.output.unwrap_or_default();
            if output.len() > state.max_output_bytes {
                state.metrics.output_too_large_total.fetch_add(1, Ordering::Relaxed);
                return Err(json_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "generated pdf exceeds the configured output size limit",
                    json!({ "maxOutputBytes": state.max_output_bytes, "outputBytes": output.len() }),
                ));
            }
            state.metrics.pdf_total.fetch_add(1, Ordering::Relaxed);
            let output = Arc::new(output);
            state
                .cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .put(key, output.clone());
            Ok(output)
        }
        Ok(outcome) => {
            state.metrics.pdf_errors_total.fetch_add(1, Ordering::Relaxed);
            Err(json_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "pdf generation failed",
                json!({ "pandocError": outcome.error.unwrap_or_default() }),
            ))
        }
        Err(error) => {
            state.metrics.bridge_errors_total.fetch_add(1, Ordering::Relaxed);
            Err(json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "pdf bridge call failed",
                json!({ "error": error.to_string() }),
            ))
        }
    }
}

async fn convert_pdf_http(
    State(state): State<AppState>,
    Json(req): Json<PdfRequest>,
) -> Response {
    let rid = request_id(req.request_id.as_ref(), "document-convert-pdf");
    // Accept text content (content_base64 may carry UTF-8 text or binary input).
    let content = match BASE64.decode(req.content_base64.trim()) {
        Ok(bytes) => bytes,
        Err(error) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "content_base64 is not valid base64",
                json!({ "error": error.to_string() }),
            )
        }
    };
    let standalone = req.standalone.unwrap_or(true);
    let metadata = req.metadata.clone().unwrap_or_else(|| json!({}));
    match run_pdf(&state, &req.from, &content, standalone, metadata).await {
        Ok(output) => Json(json!({
            "ok": true,
            "requestId": rid,
            "from": req.from,
            "to": "pdf",
            "outputBytes": output.len(),
            "content_base64": BASE64.encode(output.as_slice()),
            "generatedAtMs": now_ms(),
        }))
        .into_response(),
        Err(response) => response,
    }
}

async fn to_ast_http(State(state): State<AppState>, Json(req): Json<ToAstRequest>) -> Response {
    let rid = request_id(req.request_id.as_ref(), "document-to-ast");
    match run_conversion_text(&state, &req.from, JSON_FORMAT, req.content.as_bytes(), false, json!({}))
        .await
    {
        Ok(output) => {
            state.metrics.ast_reads_total.fetch_add(1, Ordering::Relaxed);
            let ast: Value = serde_json::from_slice(&output).unwrap_or(Value::Null);
            Json(json!({
                "ok": true,
                "requestId": rid,
                "from": req.from,
                "ast": ast,
                "generatedAtMs": now_ms(),
            }))
            .into_response()
        }
        Err(response) => response,
    }
}

async fn from_ast_http(State(state): State<AppState>, Json(req): Json<FromAstRequest>) -> Response {
    let rid = request_id(req.request_id.as_ref(), "document-from-ast");
    match run_conversion_text(&state, JSON_FORMAT, &req.to, req.content.as_bytes(), false, json!({}))
        .await
    {
        Ok(output) => match output_as_text(&output) {
            Ok(text) => Json(json!({
                "ok": true,
                "requestId": rid,
                "to": req.to,
                "outputBytes": text.len(),
                "output": text,
                "generatedAtMs": now_ms(),
            }))
            .into_response(),
            Err(response) => response,
        },
        Err(response) => response,
    }
}

async fn inspect_http(State(state): State<AppState>, Json(req): Json<InspectRequest>) -> Response {
    let rid = request_id(req.request_id.as_ref(), "document-inspect");
    match run_conversion_text(&state, &req.from, JSON_FORMAT, req.content.as_bytes(), false, json!({}))
        .await
    {
        Ok(output) => {
            state.metrics.inspections_total.fetch_add(1, Ordering::Relaxed);
            let ast: Value = match serde_json::from_slice(&output) {
                Ok(value) => value,
                Err(error) => {
                    return json_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "pandoc returned an AST that could not be parsed",
                        json!({ "error": error.to_string() }),
                    )
                }
            };
            let report = inspect_ast(&ast);
            Json(json!({
                "ok": true,
                "requestId": rid,
                "from": req.from,
                "report": report,
                "generatedAtMs": now_ms(),
            }))
            .into_response()
        }
        Err(response) => response,
    }
}

async fn validate_http(State(state): State<AppState>, Json(req): Json<ConvertRequest>) -> Response {
    state.metrics.validations_total.fetch_add(1, Ordering::Relaxed);
    let rid = request_id(req.request_id.as_ref(), "document-validate");
    let mut errors = Vec::new();
    if let Err((message, _)) = check_format("from", &req.from, true, true) {
        errors.push(message);
    }
    if let Err((message, _)) = check_format("to", &req.to, false, true) {
        errors.push(message);
    }
    if req.content.len() > state.max_input_bytes {
        errors.push(format!(
            "content exceeds the configured size limit of {} bytes",
            state.max_input_bytes
        ));
    }
    Json(json!({
        "ok": errors.is_empty(),
        "requestId": rid,
        "from": req.from,
        "to": req.to,
        "contentBytes": req.content.len(),
        "errors": errors,
        "generatedAtMs": now_ms(),
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// Image manipulation (ImageMagick via the Magick++ C++ SDK)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ImageTransformRequest {
    #[serde(default)]
    request_id: Option<String>,
    /// Base64-encoded image bytes.
    content_base64: String,
    /// Target encoder, e.g. "png". Required for /image/convert.
    #[serde(default)]
    format: Option<String>,
    /// ImageMagick geometry, e.g. "200x200>".
    #[serde(default)]
    resize: Option<String>,
    /// Crop geometry, e.g. "100x100+10+10".
    #[serde(default)]
    crop: Option<String>,
    #[serde(default)]
    rotate: Option<f64>,
    #[serde(default)]
    quality: Option<i32>,
    /// Strip metadata/profiles. Defaults to true (privacy-preserving).
    #[serde(default)]
    strip: Option<bool>,
    /// Convert to grayscale.
    #[serde(default)]
    grayscale: Option<bool>,
    /// Apply EXIF orientation then clear the tag.
    #[serde(default)]
    auto_orient: Option<bool>,
    /// Flatten onto this background colour, e.g. "white" or "#ffffff".
    #[serde(default)]
    background: Option<String>,
}

#[derive(Deserialize)]
struct ImageIdentifyRequest {
    #[serde(default)]
    request_id: Option<String>,
    content_base64: String,
}

fn is_safe_geometry(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_RESIZE_GEOMETRY_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || "x%^!<>+-@. ".contains(c))
}

fn is_safe_color(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_BACKGROUND_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "#(),%. ".contains(c))
}

/// Decode + validate image inputs shared by the transform/convert handlers.
fn decode_image(state: &AppState, content_base64: &str) -> Result<Vec<u8>, Response> {
    if !state.image_enabled {
        state.metrics.image_errors_total.fetch_add(1, Ordering::Relaxed);
        return Err(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "image bridge is not available in this build",
            json!({ "imageEnabled": false }),
        ));
    }
    let bytes = BASE64.decode(content_base64.trim()).map_err(|error| {
        state
            .metrics
            .image_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        json_error(
            StatusCode::BAD_REQUEST,
            "content_base64 is not valid base64",
            json!({ "error": error.to_string() }),
        )
    })?;
    if bytes.is_empty() {
        return Err(json_error(
            StatusCode::BAD_REQUEST,
            "decoded image is empty",
            json!({}),
        ));
    }
    if bytes.len() > state.max_image_bytes {
        state
            .metrics
            .image_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(json_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "image exceeds the configured size limit",
            json!({ "maxImageBytes": state.max_image_bytes, "imageBytes": bytes.len() }),
        ));
    }
    Ok(bytes)
}

/// Validate transform parameters into an [`image_ffi::ImageOps`].
fn build_image_ops(
    state: &AppState,
    req: &ImageTransformRequest,
    require_format: bool,
) -> Result<image_ffi::ImageOps, Response> {
    let reject = |message: &str, details: Value| {
        state
            .metrics
            .image_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        json_error(StatusCode::BAD_REQUEST, message.to_string(), details)
    };

    if require_format && req.format.as_deref().unwrap_or("").is_empty() {
        return Err(reject("format is required", json!({})));
    }
    if let Some(format) = req.format.as_deref().filter(|f| !f.is_empty()) {
        let normalized = format.to_ascii_lowercase();
        if !IMAGE_OUTPUT_FORMATS.contains(&normalized.as_str()) {
            return Err(reject(
                "unsupported output image format",
                json!({ "format": format, "supported": IMAGE_OUTPUT_FORMATS }),
            ));
        }
    }
    if let Some(resize) = req.resize.as_deref().filter(|r| !r.is_empty()) {
        if !is_safe_geometry(resize) {
            return Err(reject(
                "resize geometry is invalid",
                json!({ "resize": resize, "maxLen": MAX_RESIZE_GEOMETRY_LEN }),
            ));
        }
    }
    if let Some(crop) = req.crop.as_deref().filter(|c| !c.is_empty()) {
        if !is_safe_geometry(crop) {
            return Err(reject(
                "crop geometry is invalid",
                json!({ "crop": crop, "maxLen": MAX_RESIZE_GEOMETRY_LEN }),
            ));
        }
    }
    if let Some(rotate) = req.rotate {
        if !rotate.is_finite() || rotate.abs() > 360.0 {
            return Err(reject(
                "rotate must be a finite number within [-360, 360]",
                json!({ "rotate": rotate }),
            ));
        }
    }
    if let Some(quality) = req.quality {
        if !(1..=100).contains(&quality) {
            return Err(reject(
                "quality must be within [1, 100]",
                json!({ "quality": quality }),
            ));
        }
    }
    if let Some(background) = req.background.as_deref().filter(|b| !b.is_empty()) {
        if !is_safe_color(background) {
            return Err(reject(
                "background colour is invalid",
                json!({ "background": background }),
            ));
        }
    }

    Ok(image_ffi::ImageOps {
        out_format: req.format.clone().filter(|f| !f.is_empty()),
        resize: req.resize.clone().filter(|r| !r.is_empty()),
        crop: req.crop.clone().filter(|c| !c.is_empty()),
        rotate_degrees: req.rotate.unwrap_or(0.0),
        quality: req.quality.unwrap_or(0),
        // Strip metadata by default; callers can opt out explicitly.
        strip: req.strip.unwrap_or(true),
        grayscale: req.grayscale.unwrap_or(false),
        auto_orient: req.auto_orient.unwrap_or(false),
        background: req.background.clone().filter(|b| !b.is_empty()),
    })
}

/// Run a Magick++ transform off the runtime (concurrency-bounded). Returns the
/// output bytes plus the bridge's identify report, or a ready `Response` error.
async fn image_process(
    state: &AppState,
    bytes: Vec<u8>,
    ops: image_ffi::ImageOps,
) -> Result<(Vec<u8>, Value), Response> {
    // Bound how many image ops run at once (MagickCore is heavy).
    let _permit = state.image_semaphore.clone().acquire_owned().await;
    let result = tokio::task::spawn_blocking(move || {
        let out = image_ffi::transform(&bytes, &ops)?;
        // Re-read the result for authoritative format/dimensions.
        let info = image_ffi::identify(&out).ok();
        Ok::<_, image_ffi::ImageError>((out, info))
    })
    .await;

    match result {
        Ok(Ok((out, info))) => {
            if out.len() > state.max_image_bytes {
                state
                    .metrics
                    .image_rejections_total
                    .fetch_add(1, Ordering::Relaxed);
                return Err(json_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "transformed image exceeds the configured size limit",
                    json!({ "maxImageBytes": state.max_image_bytes, "outputBytes": out.len() }),
                ));
            }
            state
                .metrics
                .image_transforms_total
                .fetch_add(1, Ordering::Relaxed);
            let info_value = info
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .unwrap_or(Value::Null);
            Ok((out, info_value))
        }
        Ok(Err(error)) => {
            state.metrics.image_errors_total.fetch_add(1, Ordering::Relaxed);
            Err(json_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "image transform failed",
                json!({ "error": error.to_string() }),
            ))
        }
        Err(join_err) => {
            state.metrics.image_errors_total.fetch_add(1, Ordering::Relaxed);
            Err(json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "image worker failed",
                json!({ "error": join_err.to_string() }),
            ))
        }
    }
}

async fn run_image_transform(
    state: AppState,
    req: ImageTransformRequest,
    require_format: bool,
    default_prefix: &str,
) -> Response {
    let rid = request_id(req.request_id.as_ref(), default_prefix);
    let bytes = match decode_image(&state, &req.content_base64) {
        Ok(bytes) => bytes,
        Err(response) => return response,
    };
    let ops = match build_image_ops(&state, &req, require_format) {
        Ok(ops) => ops,
        Err(response) => return response,
    };
    let input_bytes = bytes.len();
    match image_process(&state, bytes, ops).await {
        Ok((out, info)) => Json(json!({
            "ok": true,
            "requestId": rid,
            "inputBytes": input_bytes,
            "outputBytes": out.len(),
            "info": info,
            "content_base64": BASE64.encode(&out),
            "generatedAtMs": now_ms(),
        }))
        .into_response(),
        Err(response) => response,
    }
}

async fn image_convert_http(
    State(state): State<AppState>,
    Json(req): Json<ImageTransformRequest>,
) -> Response {
    run_image_transform(state, req, true, "image-convert").await
}

async fn image_transform_http(
    State(state): State<AppState>,
    Json(req): Json<ImageTransformRequest>,
) -> Response {
    run_image_transform(state, req, false, "image-transform").await
}

async fn image_identify_http(
    State(state): State<AppState>,
    Json(req): Json<ImageIdentifyRequest>,
) -> Response {
    let rid = request_id(req.request_id.as_ref(), "image-identify");
    let bytes = match decode_image(&state, &req.content_base64) {
        Ok(bytes) => bytes,
        Err(response) => return response,
    };
    let _permit = state.image_semaphore.clone().acquire_owned().await;
    let result = tokio::task::spawn_blocking(move || image_ffi::identify(&bytes)).await;
    match result {
        Ok(Ok(info)) => {
            state
                .metrics
                .image_identifies_total
                .fetch_add(1, Ordering::Relaxed);
            let info_value = serde_json::from_str::<Value>(&info).unwrap_or(Value::Null);
            Json(json!({
                "ok": true,
                "requestId": rid,
                "info": info_value,
                "generatedAtMs": now_ms(),
            }))
            .into_response()
        }
        Ok(Err(error)) => {
            state.metrics.image_errors_total.fetch_add(1, Ordering::Relaxed);
            json_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "image identify failed",
                json!({ "error": error.to_string() }),
            )
        }
        Err(join_err) => {
            state.metrics.image_errors_total.fetch_add(1, Ordering::Relaxed);
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "image worker failed",
                json!({ "error": join_err.to_string() }),
            )
        }
    }
}

async fn image_formats_http(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "imageEnabled": state.image_enabled,
        "magickVersion": state.magick_version,
        "outputFormats": IMAGE_OUTPUT_FORMATS,
        "maxImageBytes": state.max_image_bytes,
        "generatedAtMs": now_ms(),
    }))
}

// ---------------------------------------------------------------------------
// Streaming endpoints (raw binary I/O — no base64/JSON inflation)
//
// Designed for the heavy/big-document path served by a warm container-pool
// worker: the request body is the raw input, the response body is the raw
// output. Conversion params ride on headers since the body is opaque bytes.
// (Pandoc/Magick need the whole buffer, so this is binary transport with a high
// body ceiling, not chunk-by-chunk transform.)
// ---------------------------------------------------------------------------

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn header_bool(headers: &HeaderMap, name: &str) -> Option<bool> {
    header_str(headers, name)
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

fn image_content_type(fmt: Option<&str>) -> &'static str {
    match fmt.map(|f| f.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpeg") | Some("jpg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        Some("tiff") | Some("tif") => "image/tiff",
        Some("avif") => "image/avif",
        Some("heic") => "image/heic",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

async fn stream_convert_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(from) = header_str(&headers, "x-from").map(str::to_string) else {
        return json_error(StatusCode::BAD_REQUEST, "x-from header is required", json!({}));
    };
    let Some(to) = header_str(&headers, "x-to").map(str::to_string) else {
        return json_error(StatusCode::BAD_REQUEST, "x-to header is required", json!({}));
    };
    let standalone = header_bool(&headers, "x-standalone").unwrap_or(false);
    let metadata = header_str(&headers, "x-metadata")
        .and_then(|v| serde_json::from_str::<Value>(v).ok())
        .unwrap_or_else(|| json!({}));

    // `to: pdf` routes to the Typst engine path; everything else is runPure.
    let result = if format_base(&to) == "pdf" {
        run_pdf(&state, &from, &body, header_bool(&headers, "x-standalone").unwrap_or(true), metadata)
            .await
    } else {
        run_conversion(&state, &from, &to, &body, standalone, metadata).await
    };
    match result {
        Ok(output) => (
            [(header::CONTENT_TYPE, output_content_type(&to))],
            (*output).clone(),
        )
            .into_response(),
        Err(response) => response,
    }
}

async fn stream_image_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !state.image_enabled {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "image bridge is not available in this build",
            json!({ "imageEnabled": false }),
        );
    }
    if body.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "request body is empty", json!({}));
    }
    let req = ImageTransformRequest {
        request_id: None,
        content_base64: String::new(),
        format: header_str(&headers, "x-format").map(str::to_string),
        resize: header_str(&headers, "x-resize").map(str::to_string),
        crop: header_str(&headers, "x-crop").map(str::to_string),
        rotate: header_str(&headers, "x-rotate").and_then(|v| v.parse().ok()),
        quality: header_str(&headers, "x-quality").and_then(|v| v.parse().ok()),
        strip: header_bool(&headers, "x-strip"),
        grayscale: header_bool(&headers, "x-grayscale"),
        auto_orient: header_bool(&headers, "x-auto-orient"),
        background: header_str(&headers, "x-background").map(str::to_string),
    };
    let ops = match build_image_ops(&state, &req, false) {
        Ok(ops) => ops,
        Err(response) => return response,
    };
    match image_process(&state, body.to_vec(), ops).await {
        Ok((out, info)) => {
            // Prefer the requested format; fall back to the detected output format.
            let detected = info
                .get("format")
                .and_then(|f| f.as_str())
                .map(str::to_string);
            let ct = image_content_type(req.format.as_deref().or(detected.as_deref()));
            ([(header::CONTENT_TYPE, ct)], out).into_response()
        }
        Err(response) => response,
    }
}

// ---------------------------------------------------------------------------
// AST inspection (document structure report)
// ---------------------------------------------------------------------------

fn inspect_ast(ast: &Value) -> Value {
    let blocks = ast.get("blocks").and_then(|b| b.as_array());
    let mut block_counts: std::collections::BTreeMap<String, u64> = Default::default();
    let mut headers: Vec<Value> = Vec::new();
    let mut text = String::new();

    if let Some(blocks) = blocks {
        for block in blocks {
            if let Some(kind) = block.get("t").and_then(|t| t.as_str()) {
                *block_counts.entry(kind.to_string()).or_insert(0) += 1;
                if kind == "Header" {
                    if let Some(c) = block.get("c").and_then(|c| c.as_array()) {
                        let level = c.first().and_then(|l| l.as_u64()).unwrap_or(0);
                        let title = c
                            .get(2)
                            .map(|inlines| collect_text(inlines))
                            .unwrap_or_default();
                        headers.push(json!({ "level": level, "text": title.trim() }));
                    }
                }
            }
            collect_text_into(block, &mut text);
            // Keep adjacent blocks from running together (e.g. a heading and the
            // following paragraph) so word counts stay accurate.
            text.push('\n');
        }
    }

    let word_count = text.split_whitespace().count();
    json!({
        "blockCount": blocks.map(|b| b.len()).unwrap_or(0),
        "blockCounts": block_counts,
        "headers": headers,
        "wordCount": word_count,
        "characterCount": text.chars().count(),
        "hasMeta": ast
            .get("meta")
            .map(|m| m.as_object().map(|o| !o.is_empty()).unwrap_or(false))
            .unwrap_or(false),
        "pandocApiVersion": ast.get("pandoc-api-version"),
    })
}

fn collect_text(value: &Value) -> String {
    let mut out = String::new();
    collect_text_into(value, &mut out);
    out
}

/// Walk a Pandoc AST fragment pulling out rendered text. `Str` nodes contribute
/// their string; `Space`/`SoftBreak`/`LineBreak` contribute a space.
fn collect_text_into(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_text_into(item, out);
            }
        }
        Value::Object(map) => {
            match map.get("t").and_then(|t| t.as_str()) {
                Some("Str") => {
                    if let Some(s) = map.get("c").and_then(|c| c.as_str()) {
                        out.push_str(s);
                    }
                }
                Some("Space") | Some("SoftBreak") | Some("LineBreak") => out.push(' '),
                _ => {
                    if let Some(c) = map.get("c") {
                        collect_text_into(c, out);
                    }
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

fn metrics_body(state: &AppState) -> String {
    let m = &state.metrics;
    format!(
        concat!(
            "# HELP dd_document_rs_conversions_total Successful conversions.\n",
            "# TYPE dd_document_rs_conversions_total counter\n",
            "dd_document_rs_conversions_total {}\n",
            "# HELP dd_document_rs_conversion_errors_total Conversions rejected by pandoc.\n",
            "# TYPE dd_document_rs_conversion_errors_total counter\n",
            "dd_document_rs_conversion_errors_total {}\n",
            "# HELP dd_document_rs_ast_reads_total Documents parsed to the JSON AST.\n",
            "# TYPE dd_document_rs_ast_reads_total counter\n",
            "dd_document_rs_ast_reads_total {}\n",
            "# HELP dd_document_rs_ast_writes_total Documents rendered from the JSON AST.\n",
            "# TYPE dd_document_rs_ast_writes_total counter\n",
            "dd_document_rs_ast_writes_total {}\n",
            "# HELP dd_document_rs_inspections_total Document structure inspections.\n",
            "# TYPE dd_document_rs_inspections_total counter\n",
            "dd_document_rs_inspections_total {}\n",
            "# HELP dd_document_rs_validations_total Validation-only requests.\n",
            "# TYPE dd_document_rs_validations_total counter\n",
            "dd_document_rs_validations_total {}\n",
            "# HELP dd_document_rs_format_rejections_total Requests rejected on format/size policy.\n",
            "# TYPE dd_document_rs_format_rejections_total counter\n",
            "dd_document_rs_format_rejections_total {}\n",
            "# HELP dd_document_rs_output_too_large_total Conversions rejected for oversized output.\n",
            "# TYPE dd_document_rs_output_too_large_total counter\n",
            "dd_document_rs_output_too_large_total {}\n",
            "# HELP dd_document_rs_cache_hits_total Conversion cache hits.\n",
            "# TYPE dd_document_rs_cache_hits_total counter\n",
            "dd_document_rs_cache_hits_total {}\n",
            "# HELP dd_document_rs_cache_misses_total Conversion cache misses.\n",
            "# TYPE dd_document_rs_cache_misses_total counter\n",
            "dd_document_rs_cache_misses_total {}\n",
            "# HELP dd_document_rs_pdf_total Successful PDF generations.\n",
            "# TYPE dd_document_rs_pdf_total counter\n",
            "dd_document_rs_pdf_total {}\n",
            "# HELP dd_document_rs_pdf_errors_total PDF generation failures.\n",
            "# TYPE dd_document_rs_pdf_errors_total counter\n",
            "dd_document_rs_pdf_errors_total {}\n",
            "# HELP dd_document_rs_bridge_errors_total Pandoc FFI bridge failures.\n",
            "# TYPE dd_document_rs_bridge_errors_total counter\n",
            "dd_document_rs_bridge_errors_total {}\n",
            "# HELP dd_document_rs_image_transforms_total Successful image transforms.\n",
            "# TYPE dd_document_rs_image_transforms_total counter\n",
            "dd_document_rs_image_transforms_total {}\n",
            "# HELP dd_document_rs_image_identifies_total Image identify calls.\n",
            "# TYPE dd_document_rs_image_identifies_total counter\n",
            "dd_document_rs_image_identifies_total {}\n",
            "# HELP dd_document_rs_image_errors_total Image processing failures.\n",
            "# TYPE dd_document_rs_image_errors_total counter\n",
            "dd_document_rs_image_errors_total {}\n",
            "# HELP dd_document_rs_image_rejections_total Image requests rejected on policy.\n",
            "# TYPE dd_document_rs_image_rejections_total counter\n",
            "dd_document_rs_image_rejections_total {}\n",
            "# HELP dd_document_rs_nats_messages_total NATS convert messages received.\n",
            "# TYPE dd_document_rs_nats_messages_total counter\n",
            "dd_document_rs_nats_messages_total {}\n",
            "# HELP dd_document_rs_nats_payload_rejected_total NATS payloads rejected before conversion.\n",
            "# TYPE dd_document_rs_nats_payload_rejected_total counter\n",
            "dd_document_rs_nats_payload_rejected_total {}\n",
            "# HELP dd_document_rs_nats_published_total NATS messages published by kind.\n",
            "# TYPE dd_document_rs_nats_published_total counter\n",
            "dd_document_rs_nats_published_total{{subject_kind=\"result\"}} {}\n",
            "dd_document_rs_nats_published_total{{subject_kind=\"event\"}} {}\n",
            "dd_document_rs_nats_published_total{{subject_kind=\"critical\"}} {}\n",
            "# HELP dd_document_rs_nats_publish_errors_total NATS publish errors.\n",
            "# TYPE dd_document_rs_nats_publish_errors_total counter\n",
            "dd_document_rs_nats_publish_errors_total {}\n",
            "# HELP dd_document_rs_errors_total Internal errors.\n",
            "# TYPE dd_document_rs_errors_total counter\n",
            "dd_document_rs_errors_total {}\n",
            "# HELP dd_document_rs_bridge_enabled Pandoc bridge availability (1=yes).\n",
            "# TYPE dd_document_rs_bridge_enabled gauge\n",
            "dd_document_rs_bridge_enabled {}\n",
            "# HELP dd_document_rs_image_enabled Image (Magick++) bridge availability (1=yes).\n",
            "# TYPE dd_document_rs_image_enabled gauge\n",
            "dd_document_rs_image_enabled {}\n",
        ),
        m.conversions_total.load(Ordering::Relaxed),
        m.conversion_errors_total.load(Ordering::Relaxed),
        m.ast_reads_total.load(Ordering::Relaxed),
        m.ast_writes_total.load(Ordering::Relaxed),
        m.inspections_total.load(Ordering::Relaxed),
        m.validations_total.load(Ordering::Relaxed),
        m.format_rejections_total.load(Ordering::Relaxed),
        m.output_too_large_total.load(Ordering::Relaxed),
        m.cache_hits_total.load(Ordering::Relaxed),
        m.cache_misses_total.load(Ordering::Relaxed),
        m.pdf_total.load(Ordering::Relaxed),
        m.pdf_errors_total.load(Ordering::Relaxed),
        m.bridge_errors_total.load(Ordering::Relaxed),
        m.image_transforms_total.load(Ordering::Relaxed),
        m.image_identifies_total.load(Ordering::Relaxed),
        m.image_errors_total.load(Ordering::Relaxed),
        m.image_rejections_total.load(Ordering::Relaxed),
        m.nats_messages_total.load(Ordering::Relaxed),
        m.nats_payload_rejected_total.load(Ordering::Relaxed),
        m.nats_results_published_total.load(Ordering::Relaxed),
        m.nats_events_published_total.load(Ordering::Relaxed),
        m.nats_critical_events_published_total.load(Ordering::Relaxed),
        m.nats_publish_errors_total.load(Ordering::Relaxed),
        m.errors_total.load(Ordering::Relaxed),
        u8::from(state.bridge_enabled),
        u8::from(state.image_enabled),
    )
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
                "document-nats-serialize-failed",
                "Could not serialize a NATS payload.",
                json!({ "kind": kind, "error": error.to_string() }),
            );
            return;
        }
    };
    match nats.publish(subject.to_string(), bytes.into()).await {
        Ok(_) => match kind {
            "result" => {
                state
                    .metrics
                    .nats_results_published_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            "critical" => {
                state
                    .metrics
                    .nats_critical_events_published_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                state
                    .metrics
                    .nats_events_published_total
                    .fetch_add(1, Ordering::Relaxed);
            }
        },
        Err(error) => {
            state
                .metrics
                .nats_publish_errors_total
                .fetch_add(1, Ordering::Relaxed);
            log_error(
                "document-nats-publish-failed",
                "NATS publish failed.",
                json!({ "subject": subject, "kind": kind, "error": error.to_string() }),
            );
        }
    }
}

async fn publish_event(state: &AppState, event_type: &str, request_id: &str, ok: bool) {
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

async fn run_nats_loop(state: AppState) {
    let Some(nats) = state.nats.clone() else { return };
    loop {
    let subscription = nats
        .queue_subscribe(state.convert_subject.clone(), CONVERT_QUEUE_GROUP.to_string())
        .await;
    let mut subscription = match subscription {
        Ok(subscription) => subscription,
        Err(error) => {
            log_error(
                "document-nats-subscribe-failed",
                "Document service could not subscribe to convert requests; retrying in 5s.",
                json!({ "subject": state.convert_subject, "error": error.to_string() }),
            );
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            continue;
        }
    };
    log_info(
        "document-nats-subscribed",
        "Listening for document convert requests over NATS.",
        json!({ "subject": state.convert_subject, "queueGroup": CONVERT_QUEUE_GROUP }),
    );

    while let Some(message) = subscription.next().await {
        state.metrics.nats_messages_total.fetch_add(1, Ordering::Relaxed);
        if message.payload.len() > state.max_input_bytes {
            state
                .metrics
                .nats_payload_rejected_total
                .fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let request: ConvertRequest = match serde_json::from_slice(&message.payload) {
            Ok(request) => request,
            Err(error) => {
                state
                    .metrics
                    .nats_payload_rejected_total
                    .fetch_add(1, Ordering::Relaxed);
                log_warn(
                    "document-nats-bad-payload",
                    "Discarded an unparseable convert request.",
                    json!({ "error": error.to_string() }),
                );
                continue;
            }
        };
        let rid = request_id(request.request_id.as_ref(), "document-nats");
        let standalone = request.standalone.unwrap_or(false);
        let metadata = request.metadata.clone().unwrap_or_else(|| json!({}));
        let result = run_conversion(
            &state,
            &request.from,
            &request.to,
            request.content.as_bytes(),
            standalone,
            metadata,
        )
        .await;
        let (ok, payload) = match result {
            Ok(output) => {
                // Text output goes back as `output`; binary as base64.
                let body = match String::from_utf8(output.to_vec()) {
                    Ok(text) => json!({ "output": text }),
                    Err(_) => json!({ "outputBase64": BASE64.encode(output.as_slice()) }),
                };
                (
                    true,
                    json!({
                        "ok": true,
                        "requestId": rid,
                        "from": request.from,
                        "to": request.to,
                        "outputBytes": output.len(),
                        "body": body,
                        "generatedAtMs": now_ms(),
                    }),
                )
            }
            Err(_) => (
                false,
                json!({
                    "ok": false,
                    "requestId": rid,
                    "from": request.from,
                    "to": request.to,
                    "error": "conversion failed; see service logs and HTTP API for details",
                    "generatedAtMs": now_ms(),
                }),
            ),
        };
        publish_value(&state, &state.result_subject.clone(), &payload, "result").await;
        publish_event(&state, "document.convert", &rid, ok).await;
    }
    log_warn(
        "document-nats-subscription-ended",
        "Document convert subscription ended; re-subscribing in 5s.",
        json!({ "subject": state.convert_subject }),
    );
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8122");
    let max_input_bytes = env_usize("DOCUMENT_MAX_INPUT_BYTES", DEFAULT_MAX_INPUT_BYTES);
    let max_output_bytes = env_usize("DOCUMENT_MAX_OUTPUT_BYTES", DEFAULT_MAX_OUTPUT_BYTES);
    let max_image_bytes = env_usize("DOCUMENT_MAX_IMAGE_BYTES", DEFAULT_MAX_IMAGE_BYTES);
    let max_stream_bytes = env_usize("DOCUMENT_MAX_STREAM_BYTES", DEFAULT_MAX_STREAM_BYTES);
    let cache_capacity = env::var("DOCUMENT_CACHE_CAPACITY")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_CACHE_CAPACITY);
    let cache_max_entry_bytes =
        env_usize("DOCUMENT_CACHE_MAX_ENTRY_BYTES", DEFAULT_CACHE_MAX_ENTRY_BYTES);
    let image_concurrency = env_usize("DOCUMENT_IMAGE_CONCURRENCY", DEFAULT_IMAGE_CONCURRENCY);
    let default_convert_concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 16);
    let convert_concurrency =
        env_usize("DOCUMENT_CONVERT_CONCURRENCY", default_convert_concurrency);
    let convert_subject = env_value("DOCUMENT_CONVERT_SUBJECT", DEFAULT_CONVERT_SUBJECT);
    let result_subject = env_value("DOCUMENT_RESULT_SUBJECT", DEFAULT_RESULT_SUBJECT);
    let event_subject = env_value("DOCUMENT_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT);
    let critical_event_subject =
        env_value("NATS_CRITICAL_EVENT_SUBJECT", RUNTIME_CRITICAL_EVENTS_SUBJECT);

    let bridge_enabled = ffi::bridge_enabled();
    let pandoc_version = if bridge_enabled {
        match ffi::version() {
            Ok(version) => Some(version),
            Err(error) => {
                log_error(
                    "document-bridge-version-failed",
                    "Pandoc bridge is linked but did not report a version.",
                    json!({ "error": error.to_string() }),
                );
                None
            }
        }
    } else {
        None
    };

    let image_enabled = image_ffi::image_enabled();
    let magick_version = if image_enabled {
        image_ffi::version()
    } else {
        None
    };

    // PDF needs the typst binary on PATH (opt-in in the image).
    let pdf_engine = env_value("DOCUMENT_PDF_ENGINE", "typst");
    let pdf_enabled = bridge_enabled
        && std::process::Command::new(&pdf_engine)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

    let nats_url = env::var("NATS_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let nats = match nats_url {
        Some(url) => match async_nats::connect(url.clone()).await {
            Ok(client) => Some(client),
            Err(error) => {
                log_error(
                    "document-nats-connect-failed",
                    "Document service failed to connect to NATS.",
                    json!({ "url": url, "error": error.to_string() }),
                );
                None
            }
        },
        None => None,
    };

    let state = AppState {
        nats,
        convert_subject,
        result_subject,
        event_subject,
        critical_event_subject,
        max_input_bytes,
        max_output_bytes,
        max_image_bytes,
        max_stream_bytes,
        bridge_enabled,
        pandoc_version,
        image_enabled,
        magick_version,
        pdf_enabled,
        cache: Arc::new(Mutex::new(ConvCache::new(cache_capacity, cache_max_entry_bytes))),
        image_semaphore: Arc::new(Semaphore::new(image_concurrency.max(1))),
        convert_semaphore: Arc::new(Semaphore::new(convert_concurrency.max(1))),
        metrics: Arc::new(Metrics::default()),
    };

    log_info(
        "document-service-starting",
        "Document service runtime configuration loaded.",
        json!({
            "bridgeEnabled": state.bridge_enabled,
            "pandocVersion": state.pandoc_version,
            "imageEnabled": state.image_enabled,
            "magickVersion": state.magick_version,
            "pdfEnabled": state.pdf_enabled,
            "maxInputBytes": state.max_input_bytes,
            "maxOutputBytes": state.max_output_bytes,
            "maxImageBytes": state.max_image_bytes,
            "convertSubject": state.convert_subject,
            "resultSubject": state.result_subject,
            "eventSubject": state.event_subject,
            "criticalEventSubject": state.critical_event_subject,
            "natsEnabled": state.nats.is_some(),
        }),
    );

    if state.nats.is_some() {
        tokio::spawn(run_nats_loop(state.clone()));
    }

    let app = Router::new()
        .route("/", get(home))
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/status", get(status_http))
        .route("/formats", get(formats_http))
        .route("/capabilities", get(capabilities_http))
        .route("/example", get(example_http))
        .route("/convert", post(convert_http))
        .route("/convert-binary", post(convert_binary_http))
        .route("/convert-pdf", post(convert_pdf_http))
        .route("/to-ast", post(to_ast_http))
        .route("/from-ast", post(from_ast_http))
        .route("/inspect", post(inspect_http))
        .route("/validate", post(validate_http))
        .route("/image/convert", post(image_convert_http))
        .route("/image/transform", post(image_transform_http))
        .route("/image/identify", post(image_identify_http))
        .route("/image/formats", get(image_formats_http))
        // JSON/base64 routes: limit covers a document or a base64-encoded image
        // (~4/3 the raw bytes), plus JSON envelope slack.
        .layer(DefaultBodyLimit::max(
            max_input_bytes
                .max(max_image_bytes / 3 * 4)
                .saturating_add(64 * 1024),
        ));

    // Raw streaming routes carry their own, higher body ceiling.
    let stream_routes = Router::new()
        .route("/stream/convert", post(stream_convert_http))
        .route("/stream/image", post(stream_image_http))
        .layer(DefaultBodyLimit::max(max_stream_bytes));

    let app = app
        .merge(stream_routes)
        .with_state(state)
        .merge(dd_runtime_config_client::router());
    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let address: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    log_info(
        "document-service-listening",
        "Document service HTTP listener is ready.",
        json!({ "address": address.to_string() }),
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_base_strips_extensions() {
        assert_eq!(format_base("markdown+hard_line_breaks"), "markdown");
        assert_eq!(format_base("gfm-raw_html"), "gfm");
        assert_eq!(format_base("html"), "html");
    }

    #[test]
    fn binary_formats_are_rejected() {
        assert!(is_binary_format("docx"));
        assert!(is_binary_format("pdf"));
        assert!(!is_binary_format("markdown"));
        // Binary rejected on the text path, allowed on the binary path.
        assert!(check_format("to", "docx", false, false).is_err());
        assert!(check_format("to", "docx", false, true).is_ok());
        assert!(check_format("from", "docx", true, true).is_ok());
        // pdf is not a pure writer even on the binary path.
        assert!(check_format("to", "pdf", false, true).is_err());
    }

    #[test]
    fn known_text_formats_pass() {
        assert!(check_format("from", "markdown", true, false).is_ok());
        assert!(check_format("to", "html5", false, false).is_ok());
        assert!(check_format("from", "not-a-format", true, false).is_err());
    }

    #[test]
    fn geometry_and_color_validation() {
        assert!(is_safe_geometry("200x200>"));
        assert!(is_safe_geometry("50%"));
        assert!(!is_safe_geometry("200x200; rm -rf /"));
        assert!(!is_safe_geometry(&"9".repeat(MAX_RESIZE_GEOMETRY_LEN + 1)));
        assert!(is_safe_color("white"));
        assert!(is_safe_color("#ff00aa"));
        assert!(!is_safe_color("url(http://evil)"));
    }

    #[test]
    fn image_output_format_allowlist() {
        assert!(IMAGE_OUTPUT_FORMATS.contains(&"png"));
        assert!(IMAGE_OUTPUT_FORMATS.contains(&"webp"));
        assert!(!IMAGE_OUTPUT_FORMATS.contains(&"pdf"));
        assert!(!IMAGE_OUTPUT_FORMATS.contains(&"svg"));
    }

    #[test]
    fn cache_key_is_unambiguous_and_deterministic() {
        let m = json!({});
        let k1 = conversion_cache_key("a", "b", false, &m, b"x");
        let k2 = conversion_cache_key("a", "b", false, &m, b"x");
        assert_eq!(k1, k2, "same request must hash equal");
        // Field boundaries must not alias.
        assert_ne!(
            conversion_cache_key("a", "b", false, &m, b"x"),
            conversion_cache_key("ab", "", false, &m, b"x")
        );
        assert_ne!(
            conversion_cache_key("a", "b", false, &m, b"x"),
            conversion_cache_key("a", "b", true, &m, b"x")
        );
        assert_ne!(
            conversion_cache_key("a", "b", false, &m, b"x"),
            conversion_cache_key("a", "b", false, &json!({"t": 1}), b"x")
        );
    }

    #[test]
    fn cache_respects_capacity_and_entry_size() {
        let mut cache = ConvCache::new(2, 8);
        let k = |n: u8| [n; 32];
        cache.put(k(1), Arc::new(vec![0u8; 4]));
        cache.put(k(2), Arc::new(vec![0u8; 4]));
        assert!(cache.get(&k(1)).is_some());
        cache.put(k(3), Arc::new(vec![0u8; 4])); // evicts k(1) (FIFO)
        assert!(cache.get(&k(1)).is_none());
        assert!(cache.get(&k(3)).is_some());
        // Oversized entry is not cached.
        cache.put(k(4), Arc::new(vec![0u8; 9]));
        assert!(cache.get(&k(4)).is_none());
    }

    #[test]
    fn request_id_is_bounded_and_clean() {
        let raw = format!("a{}\nb", "x".repeat(500));
        let id = request_id(Some(&raw), "fallback");
        assert!(id.len() <= MAX_REQUEST_ID_LEN);
        assert!(!id.contains('\n'));
    }

    #[test]
    fn inspect_counts_headers_and_words() {
        let ast = json!({
            "pandoc-api-version": [1, 23],
            "meta": {},
            "blocks": [
                { "t": "Header", "c": [1, ["h", [], []], [
                    { "t": "Str", "c": "Hello" }, { "t": "Space" }, { "t": "Str", "c": "World" }
                ]] },
                { "t": "Para", "c": [ { "t": "Str", "c": "one" }, { "t": "Space" }, { "t": "Str", "c": "two" } ] }
            ]
        });
        let report = inspect_ast(&ast);
        assert_eq!(report["blockCount"], 2);
        assert_eq!(report["headers"][0]["level"], 1);
        assert_eq!(report["headers"][0]["text"], "Hello World");
        assert_eq!(report["wordCount"], 4);
    }
}
