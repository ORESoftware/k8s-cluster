//! Executable HTTP contract wrappers.
//!
//! Every function in this module is both the live Axum route registered by
//! `utoipa_axum::routes!` and the source of its OpenAPI operation. The wrapper
//! delegates directly to the established domain handler so the migration does
//! not fork speech, Vapi, history, persistence, or authentication behavior.

use crate::audio_io::AudioParams;
use crate::error::ApiError;
use crate::handlers_speech::{PipelineParams, SttParams, TranslateBody, TtsRequest};
use crate::history::HistoryParams;
use crate::openapi::{SharedApiDocs, OPENAPI_CONTENT_TYPE};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::Value;

fn bytes_response(bytes: Bytes, content_type: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type)], bytes).into_response()
}

#[utoipa::path(
    get,
    path = "/",
    operation_id = "getT2vServiceBanner",
    tag = "service",
    security(()),
    responses((status = 200, description = "Human-readable service and route summary", body = String, content_type = "text/plain"))
)]
pub async fn banner() -> &'static str {
    super::banner().await
}

#[utoipa::path(
    get,
    path = "/healthz",
    operation_id = "getT2vHealth",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Process liveness", body = String, content_type = "text/plain"))
)]
pub async fn healthz() -> &'static str {
    super::healthz().await
}

#[utoipa::path(
    get,
    path = "/readyz",
    operation_id = "getT2vReadiness",
    tag = "operations",
    security(()),
    responses(
        (status = 200, description = "Database reachable and service ready", body = String, content_type = "text/plain"),
        (status = 503, description = "Database unavailable", body = String, content_type = "text/plain")
    )
)]
pub async fn readyz(State(state): State<AppState>) -> Response {
    super::readyz(State(state)).await
}

#[utoipa::path(
    get,
    path = "/metrics",
    operation_id = "getT2vPrometheusMetrics",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Prometheus text exposition", body = String, content_type = "text/plain"))
)]
pub async fn metrics(State(state): State<AppState>) -> Response {
    super::metrics_handler(State(state)).await
}

#[utoipa::path(
    get,
    path = "/openapi.json",
    operation_id = "getT2vPublicOpenApi",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Fail-closed public OpenAPI 3.1 document", body = Value, content_type = "application/vnd.oai.openapi+json;version=3.1"))
)]
pub async fn public_openapi(Extension(docs): Extension<SharedApiDocs>) -> Response {
    bytes_response(docs.public_json.clone(), OPENAPI_CONTENT_TYPE)
}

#[utoipa::path(
    get,
    path = "/api/docs.json",
    operation_id = "getT2vPublicOpenApiCompatibilityAlias",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Compatibility alias for the public OpenAPI document", body = Value, content_type = "application/vnd.oai.openapi+json;version=3.1"))
)]
pub async fn public_openapi_alias(Extension(docs): Extension<SharedApiDocs>) -> Response {
    bytes_response(docs.public_json.clone(), OPENAPI_CONTENT_TYPE)
}

#[utoipa::path(
    get,
    path = "/api/docs",
    operation_id = "getT2vPublicApiReference",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Interactive Scalar reference for the public contract", body = String, content_type = "text/html"))
)]
pub async fn public_scalar(Extension(docs): Extension<SharedApiDocs>) -> Response {
    bytes_response(docs.public_scalar_html.clone(), "text/html; charset=utf-8")
}

#[utoipa::path(
    get,
    path = "/docs/api",
    operation_id = "getT2vPublicApiReferenceCompatibilityAlias",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Compatibility alias for the public Scalar reference", body = String, content_type = "text/html"))
)]
pub async fn public_scalar_alias(Extension(docs): Extension<SharedApiDocs>) -> Response {
    bytes_response(docs.public_scalar_html.clone(), "text/html; charset=utf-8")
}

#[utoipa::path(
    get,
    path = "/internal/openapi.json",
    operation_id = "getT2vInternalOpenApi",
    tag = "documentation",
    security(("server_auth" = [])),
    responses(
        (status = 200, description = "Complete internal OpenAPI 3.1 document", body = Value, content_type = "application/vnd.oai.openapi+json;version=3.1"),
        (status = 401, description = "Missing or invalid operator bearer token", body = Value),
        (status = 503, description = "Operator routes disabled because no server secret is configured", body = Value)
    )
)]
pub async fn internal_openapi(Extension(docs): Extension<SharedApiDocs>) -> Response {
    bytes_response(docs.internal_json.clone(), OPENAPI_CONTENT_TYPE)
}

#[utoipa::path(
    get,
    path = "/internal/docs/api",
    operation_id = "getT2vInternalApiReference",
    tag = "documentation",
    security(("server_auth" = [])),
    responses(
        (status = 200, description = "Interactive Scalar reference for the complete contract", body = String, content_type = "text/html"),
        (status = 401, description = "Missing or invalid operator bearer token", body = Value),
        (status = 503, description = "Operator routes disabled because no server secret is configured", body = Value)
    )
)]
pub async fn internal_scalar(Extension(docs): Extension<SharedApiDocs>) -> Response {
    bytes_response(docs.internal_scalar_html.clone(), "text/html; charset=utf-8")
}

#[utoipa::path(
    post,
    path = "/v1/stt",
    operation_id = "transcribeAudio",
    tag = "speech",
    security(()),
    params(
        ("format" = Option<String>, Query, description = "wav (default) or mulaw/ulaw/g711"),
        ("rate" = Option<u32>, Query, description = "Sample rate for raw mulaw input; defaults to 8000"),
        ("language" = Option<String>, Query, description = "Optional ISO-639-1 language hint"),
        ("trim" = Option<bool>, Query, description = "Trim leading and trailing silence; defaults true")
    ),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "WAV or raw G.711 mu-law audio bytes"),
    responses(
        (status = 200, description = "Persisted transcription result", body = Value),
        (status = 400, description = "Invalid audio or parameters", body = Value),
        (status = 422, description = "No signal above the silence threshold", body = Value),
        (status = 502, description = "Speech provider failed", body = Value),
        (status = 503, description = "Speech provider or concurrency capacity unavailable", body = Value)
    )
)]
pub async fn stt(
    State(state): State<AppState>,
    Query(params): Query<SttParams>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    crate::handlers_speech::stt(State(state), Query(params), body).await
}

#[utoipa::path(
    post,
    path = "/v1/analyze",
    operation_id = "analyzeAudioSpectrum",
    tag = "speech",
    security(()),
    params(
        ("format" = Option<String>, Query, description = "wav (default) or mulaw/ulaw/g711"),
        ("rate" = Option<u32>, Query, description = "Sample rate for raw mulaw input; defaults to 8000")
    ),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "WAV or raw G.711 mu-law audio bytes"),
    responses(
        (status = 200, description = "FFT, spectral centroid, DTMF, RMS, and voiced-duration analysis", body = Value),
        (status = 400, description = "Invalid audio or parameters", body = Value)
    )
)]
pub async fn analyze_audio(
    State(state): State<AppState>,
    Query(params): Query<AudioParams>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    crate::handlers_speech::analyze_audio(State(state), Query(params), body).await
}

#[utoipa::path(
    post,
    path = "/v1/speech-to-speech",
    operation_id = "translateSpeechToSpeech",
    tag = "speech",
    security(()),
    params(
        ("format" = Option<String>, Query, description = "wav (default) or mulaw/ulaw/g711"),
        ("rate" = Option<u32>, Query, description = "Sample rate for raw mulaw input; defaults to 8000"),
        ("target_lang" = String, Query, description = "Required translation target language"),
        ("language" = Option<String>, Query, description = "Optional source-language hint"),
        ("provider" = Option<String>, Query, description = "openai, gemini, or anthropic"),
        ("voice" = Option<String>, Query, description = "Optional synthesis voice"),
        ("out_format" = Option<String>, Query, description = "wav (default) or mp3"),
        ("respond" = Option<String>, Query, description = "audio (default) or json with base64 audio")
    ),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "Source speech audio bytes"),
    responses(
        (status = 200, description = "Translated audio bytes or JSON pipeline result"),
        (status = 400, description = "Invalid audio, provider, language, or output format", body = Value),
        (status = 422, description = "No transcribable speech", body = Value),
        (status = 502, description = "Speech or translation provider failed", body = Value),
        (status = 503, description = "Provider or concurrency capacity unavailable", body = Value)
    )
)]
pub async fn speech_to_speech(
    State(state): State<AppState>,
    Query(params): Query<PipelineParams>,
    body: Bytes,
) -> Result<Response, ApiError> {
    crate::handlers_speech::speech_to_speech(State(state), Query(params), body).await
}

#[utoipa::path(
    post,
    path = "/v1/tts",
    operation_id = "synthesizeSpeech",
    tag = "speech",
    security(()),
    request_body(content = Value, content_type = "application/json", description = "Object with text plus optional voice and wav/mp3 format"),
    responses(
        (status = 200, description = "Synthesized audio with X-Synthesis-Id response header"),
        (status = 400, description = "Invalid text, voice, or format", body = Value),
        (status = 502, description = "Speech provider failed", body = Value),
        (status = 503, description = "Speech provider or concurrency capacity unavailable", body = Value)
    )
)]
pub async fn tts(
    State(state): State<AppState>,
    Json(request): Json<TtsRequest>,
) -> Result<Response, ApiError> {
    crate::handlers_speech::tts(State(state), Json(request)).await
}

#[utoipa::path(
    post,
    path = "/v1/translate",
    operation_id = "translateText",
    tag = "translation",
    security(()),
    request_body(content = Value, content_type = "application/json", description = "Text, target_lang, optional source_lang, and optional provider"),
    responses(
        (status = 200, description = "Persisted translation result", body = Value),
        (status = 400, description = "Invalid text, language, or provider", body = Value),
        (status = 502, description = "Translation provider failed", body = Value),
        (status = 503, description = "Translation provider or concurrency capacity unavailable", body = Value)
    )
)]
pub async fn translate(
    State(state): State<AppState>,
    Json(request): Json<TranslateBody>,
) -> Result<Json<Value>, ApiError> {
    crate::handlers_speech::translate(State(state), Json(request)).await
}

#[utoipa::path(
    post,
    path = "/vapi/webhook",
    operation_id = "receiveVapiWebhook",
    tag = "vapi",
    security(("vapi_secret" = [])),
    params(("x-vapi-secret" = Option<String>, Header, description = "Configured Vapi callback secret; required unless explicit insecure development mode is enabled")),
    request_body(content = Value, content_type = "application/json", description = "Vapi assistant-request, tool-calls, status-update, or end-of-call-report message"),
    responses(
        (status = 200, description = "Webhook acknowledged or tool-call results returned", body = Value),
        (status = 400, description = "Malformed JSON or tool-call shape", body = Value),
        (status = 401, description = "Invalid or missing Vapi secret", body = Value)
    )
)]
pub async fn vapi_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    crate::handlers_vapi::webhook(State(state), headers, body).await
}

#[utoipa::path(
    get,
    path = "/v1/history/transcriptions",
    operation_id = "listTranscriptionHistory",
    tag = "history",
    security(("server_auth" = [])),
    params(("limit" = Option<u64>, Query, description = "Newest rows to return, clamped to 1..=200")),
    responses(
        (status = 200, description = "Recent transcription rows and total count", body = Value),
        (status = 401, description = "Invalid operator bearer token", body = Value),
        (status = 503, description = "Operator routes disabled or database unavailable", body = Value)
    )
)]
pub async fn transcription_history(
    State(state): State<AppState>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Value>, ApiError> {
    crate::history::transcriptions(State(state), Query(params)).await
}

#[utoipa::path(
    get,
    path = "/v1/history/translations",
    operation_id = "listTranslationHistory",
    tag = "history",
    security(("server_auth" = [])),
    params(("limit" = Option<u64>, Query, description = "Newest rows to return, clamped to 1..=200")),
    responses(
        (status = 200, description = "Recent translation rows and total count", body = Value),
        (status = 401, description = "Invalid operator bearer token", body = Value),
        (status = 503, description = "Operator routes disabled or database unavailable", body = Value)
    )
)]
pub async fn translation_history(
    State(state): State<AppState>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Value>, ApiError> {
    crate::history::translations(State(state), Query(params)).await
}

#[utoipa::path(
    get,
    path = "/v1/history/syntheses",
    operation_id = "listSynthesisHistory",
    tag = "history",
    security(("server_auth" = [])),
    params(("limit" = Option<u64>, Query, description = "Newest rows to return, clamped to 1..=200")),
    responses(
        (status = 200, description = "Recent synthesis rows and total count", body = Value),
        (status = 401, description = "Invalid operator bearer token", body = Value),
        (status = 503, description = "Operator routes disabled or database unavailable", body = Value)
    )
)]
pub async fn synthesis_history(
    State(state): State<AppState>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Value>, ApiError> {
    crate::history::syntheses(State(state), Query(params)).await
}

#[utoipa::path(
    get,
    path = "/v1/history/vapi-calls",
    operation_id = "listVapiCallHistory",
    tag = "history",
    security(("server_auth" = [])),
    params(("limit" = Option<u64>, Query, description = "Newest rows to return, clamped to 1..=200")),
    responses(
        (status = 200, description = "Recent Vapi call rows and total count", body = Value),
        (status = 401, description = "Invalid operator bearer token", body = Value),
        (status = 503, description = "Operator routes disabled or database unavailable", body = Value)
    )
)]
pub async fn vapi_call_history(
    State(state): State<AppState>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Value>, ApiError> {
    crate::history::vapi_calls(State(state), Query(params)).await
}

#[utoipa::path(
    post,
    path = "/vapi/call",
    operation_id = "createVapiCall",
    tag = "vapi",
    security(("server_auth" = [])),
    request_body(content = Value, content_type = "application/json", description = "Vapi create-call payload passed through to the configured provider"),
    responses(
        (status = 200, description = "Created Vapi call", body = Value),
        (status = 401, description = "Invalid operator bearer token", body = Value),
        (status = 502, description = "Vapi provider failed", body = Value),
        (status = 503, description = "Operator or Vapi client is not configured", body = Value)
    )
)]
pub async fn create_vapi_call(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    crate::handlers_vapi::create_call(State(state), Json(body)).await
}

#[utoipa::path(
    get,
    path = "/vapi/call/{id}",
    operation_id = "getVapiCall",
    tag = "vapi",
    security(("server_auth" = [])),
    params(("id" = String, Path, description = "Vapi call identifier")),
    responses(
        (status = 200, description = "Current Vapi call state", body = Value),
        (status = 401, description = "Invalid operator bearer token", body = Value),
        (status = 502, description = "Vapi provider failed", body = Value),
        (status = 503, description = "Operator or Vapi client is not configured", body = Value)
    )
)]
pub async fn get_vapi_call(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    crate::handlers_vapi::get_call(State(state), Path(id)).await
}
