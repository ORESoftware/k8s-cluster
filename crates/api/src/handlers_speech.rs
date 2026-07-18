//! STT, TTS, translation, the full speech-to-speech pipeline, and FFT-based
//! audio analysis.

use crate::audio_io::{decode_body, AudioParams};
use crate::error::ApiError;
use crate::metrics::Metrics;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use chrono::Utc;
use sea_orm::{ActiveModelTrait, Set};
use serde::Deserialize;
use serde_json::{json, Value};
use std::str::FromStr;
use t2v_core::audio::{encode_wav_pcm16, resample_linear};
use t2v_core::spectrum::analyze;
use t2v_core::vad::trim_silence;
use t2v_entity::{synthesis, transcription, translation};
use t2v_llm::{AudioFormat, Provider, SpeechRequest, TranslationRequest};
use uuid::Uuid;

/// Whisper wants 16 kHz mono; everything inbound is normalized to this.
const STT_SAMPLE_RATE: u32 = 16000;
/// OpenAI TTS rejects inputs beyond 4096 chars.
const MAX_TTS_CHARS: usize = 4096;
const MAX_TRANSLATE_CHARS: usize = 30_000;

const VAD_THRESHOLD_DB: f64 = -40.0;
const VAD_FRAME_MS: u32 = 20;
const VAD_PADDING_MS: u32 = 150;

pub static X_TRANSCRIPTION_ID: HeaderName = HeaderName::from_static("x-transcription-id");
pub static X_TRANSLATION_ID: HeaderName = HeaderName::from_static("x-translation-id");
pub static X_SYNTHESIS_ID: HeaderName = HeaderName::from_static("x-synthesis-id");

// ---------------------------------------------------------------------------
// Speech-to-text
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SttParams {
    #[serde(flatten)]
    pub audio: AudioParams,
    /// ISO-639-1 hint passed to Whisper.
    pub language: Option<String>,
    /// Trim leading/trailing silence before shipping to the provider
    /// (default true).
    pub trim: Option<bool>,
}

pub async fn stt(
    State(state): State<AppState>,
    Query(params): Query<SttParams>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    Metrics::bump(&state.metrics.stt_total);
    let clip = decode_body(&params.audio, &body)?;
    let duration_ms = (clip.duration_secs() * 1000.0) as i64;

    let (text, model, language, row_id) = run_stt(
        &state,
        clip.samples,
        clip.sample_rate,
        params.language.as_deref(),
        params.trim.unwrap_or(true),
        "upload",
        Some(duration_ms),
    )
    .await?;

    Ok(Json(json!({
        "ok": true,
        "transcription": {
            "id": row_id,
            "text": text,
            "model": model,
            "language": language,
            "durationMs": duration_ms,
            "sampleRate": clip.sample_rate,
        }
    })))
}

/// Shared STT path: VAD trim -> resample to 16 kHz -> WAV -> Whisper -> row.
async fn run_stt(
    state: &AppState,
    samples: Vec<f64>,
    sample_rate: u32,
    language: Option<&str>,
    trim: bool,
    source: &str,
    duration_ms: Option<i64>,
) -> Result<(String, String, Option<String>, Uuid), ApiError> {
    let samples = if trim {
        let trimmed = trim_silence(
            &samples,
            sample_rate,
            VAD_THRESHOLD_DB,
            VAD_FRAME_MS,
            VAD_PADDING_MS,
        );
        if trimmed.is_empty() {
            return Err(ApiError::unprocessable(
                "audio contains no signal above the silence threshold",
            ));
        }
        trimmed
    } else {
        samples
    };

    let resampled = resample_linear(&samples, sample_rate, STT_SAMPLE_RATE);
    let wav = encode_wav_pcm16(&resampled, STT_SAMPLE_RATE);
    let _permit = state.acquire_llm()?;
    let outcome = state.llm.transcribe(wav, language).await?;

    let row_id = Uuid::new_v4();
    transcription::ActiveModel {
        id: Set(row_id),
        source: Set(source.to_string()),
        provider: Set("openai".to_string()),
        model: Set(outcome.model.clone()),
        text: Set(outcome.text.clone()),
        language: Set(outcome
            .language
            .clone()
            .or_else(|| language.map(str::to_string))),
        sample_rate: Set(Some(sample_rate as i32)),
        duration_ms: Set(duration_ms),
        created_at: Set(Utc::now()),
    }
    .insert(&state.db)
    .await?;

    Ok((outcome.text, outcome.model, outcome.language, row_id))
}

// ---------------------------------------------------------------------------
// Text-to-speech
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TtsRequest {
    pub text: String,
    pub voice: Option<String>,
    /// "wav" (default) or "mp3".
    pub format: Option<String>,
}

pub async fn tts(
    State(state): State<AppState>,
    Json(req): Json<TtsRequest>,
) -> Result<Response, ApiError> {
    Metrics::bump(&state.metrics.tts_total);
    let (bytes, format, row_id) =
        run_tts(&state, &req.text, req.voice, req.format.as_deref()).await?;

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, format.content_type().parse().unwrap());
    headers.insert(X_SYNTHESIS_ID.clone(), row_id.to_string().parse().unwrap());
    Ok((StatusCode::OK, headers, bytes).into_response())
}

async fn run_tts(
    state: &AppState,
    text: &str,
    voice: Option<String>,
    format: Option<&str>,
) -> Result<(Vec<u8>, AudioFormat, Uuid), ApiError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ApiError::bad_request("text must not be empty"));
    }
    if text.chars().count() > MAX_TTS_CHARS {
        return Err(ApiError::bad_request(format!(
            "text exceeds the {MAX_TTS_CHARS}-character TTS limit"
        )));
    }
    let format = match format {
        Some(f) => AudioFormat::from_str(f).map_err(ApiError::bad_request)?,
        None => AudioFormat::Wav,
    };

    let (bytes, voice_used, model) = state
        .llm
        .synthesize(&SpeechRequest {
            text: text.to_string(),
            voice,
            format,
        })
        .await?;

    let row_id = Uuid::new_v4();
    synthesis::ActiveModel {
        id: Set(row_id),
        text: Set(text.to_string()),
        voice: Set(voice_used),
        provider: Set("openai".to_string()),
        model: Set(model),
        format: Set(format.as_str().to_string()),
        audio_bytes: Set(bytes.len() as i64),
        created_at: Set(Utc::now()),
    }
    .insert(&state.db)
    .await?;

    Ok((bytes, format, row_id))
}

// ---------------------------------------------------------------------------
// Translation
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TranslateBody {
    pub text: String,
    pub target_lang: String,
    pub source_lang: Option<String>,
    /// "openai" | "gemini" | "anthropic" (default openai).
    pub provider: Option<String>,
}

pub async fn translate(
    State(state): State<AppState>,
    Json(req): Json<TranslateBody>,
) -> Result<Json<Value>, ApiError> {
    Metrics::bump(&state.metrics.translations_total);
    let row = run_translation(
        &state,
        &req.text,
        &req.target_lang,
        req.source_lang.as_deref(),
        req.provider.as_deref(),
    )
    .await?;
    Ok(Json(json!({ "ok": true, "translation": row })))
}

pub async fn run_translation(
    state: &AppState,
    text: &str,
    target_lang: &str,
    source_lang: Option<&str>,
    provider: Option<&str>,
) -> Result<translation::Model, ApiError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ApiError::bad_request("text must not be empty"));
    }
    if text.chars().count() > MAX_TRANSLATE_CHARS {
        return Err(ApiError::bad_request(format!(
            "text exceeds the {MAX_TRANSLATE_CHARS}-character translation limit"
        )));
    }
    let target_lang = target_lang.trim();
    if target_lang.is_empty() {
        return Err(ApiError::bad_request("target_lang must not be empty"));
    }
    let provider = match provider {
        Some(p) => Provider::from_str(p)?,
        None => Provider::OpenAi,
    };

    let outcome = state
        .llm
        .translate(&TranslationRequest {
            text: text.to_string(),
            source_lang: source_lang.map(str::to_string),
            target_lang: target_lang.to_string(),
            provider,
        })
        .await?;

    let row = translation::ActiveModel {
        id: Set(Uuid::new_v4()),
        source_text: Set(text.to_string()),
        translated_text: Set(outcome.translated_text.clone()),
        source_lang: Set(source_lang.map(str::to_string)),
        target_lang: Set(target_lang.to_string()),
        provider: Set(outcome.provider.as_str().to_string()),
        model: Set(outcome.model.clone()),
        latency_ms: Set(outcome.latency_ms),
        created_at: Set(Utc::now()),
    }
    .insert(&state.db)
    .await?;
    Ok(row)
}

// ---------------------------------------------------------------------------
// Speech-to-speech pipeline: STT -> translate -> TTS
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PipelineParams {
    #[serde(flatten)]
    pub audio: AudioParams,
    pub target_lang: String,
    /// Source-language hint for Whisper (ISO-639-1).
    pub language: Option<String>,
    pub provider: Option<String>,
    pub voice: Option<String>,
    /// Output format: "wav" (default) or "mp3".
    pub out_format: Option<String>,
    /// "audio" (default) returns synthesized bytes; "json" returns the full
    /// pipeline record including base64 audio.
    pub respond: Option<String>,
}

pub async fn speech_to_speech(
    State(state): State<AppState>,
    Query(params): Query<PipelineParams>,
    body: Bytes,
) -> Result<Response, ApiError> {
    Metrics::bump(&state.metrics.pipeline_total);
    let clip = decode_body(&params.audio, &body)?;
    let duration_ms = (clip.duration_secs() * 1000.0) as i64;

    let (source_text, _, detected_lang, transcription_id) = run_stt(
        &state,
        clip.samples,
        clip.sample_rate,
        params.language.as_deref(),
        true,
        "pipeline",
        Some(duration_ms),
    )
    .await?;

    if source_text.is_empty() {
        return Err(ApiError::unprocessable("transcription produced no text"));
    }

    let translation_row = run_translation(
        &state,
        &source_text,
        &params.target_lang,
        params.language.as_deref().or(detected_lang.as_deref()),
        params.provider.as_deref(),
    )
    .await?;

    let (audio, format, synthesis_id) = run_tts(
        &state,
        &translation_row.translated_text,
        params.voice.clone(),
        params.out_format.as_deref(),
    )
    .await?;

    if params.respond.as_deref() == Some("json") {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&audio);
        return Ok(Json(json!({
            "ok": true,
            "transcriptionId": transcription_id,
            "sourceText": source_text,
            "translation": translation_row,
            "synthesisId": synthesis_id,
            "audioFormat": format.as_str(),
            "audioBase64": b64,
        }))
        .into_response());
    }

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, format.content_type().parse().unwrap());
    headers.insert(
        X_TRANSCRIPTION_ID.clone(),
        transcription_id.to_string().parse().unwrap(),
    );
    headers.insert(
        X_TRANSLATION_ID.clone(),
        translation_row.id.to_string().parse().unwrap(),
    );
    headers.insert(
        X_SYNTHESIS_ID.clone(),
        synthesis_id.to_string().parse().unwrap(),
    );
    Ok((StatusCode::OK, headers, audio).into_response())
}

// ---------------------------------------------------------------------------
// FFT analysis
// ---------------------------------------------------------------------------

pub async fn analyze_audio(
    State(state): State<AppState>,
    Query(params): Query<AudioParams>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    Metrics::bump(&state.metrics.analyze_total);
    let clip = decode_body(&params, &body)?;
    let analysis = analyze(&clip.samples, clip.sample_rate)?;

    Ok(Json(json!({
        "ok": true,
        "analysis": {
            "sampleRate": analysis.sample_rate,
            "numSamples": analysis.num_samples,
            "durationSecs": analysis.duration_secs,
            "rmsDbfs": analysis.rms_dbfs,
            "dominantFreqHz": analysis.dominant_freq_hz,
            "dominantMagnitude": analysis.dominant_magnitude,
            "spectralCentroidHz": analysis.spectral_centroid_hz,
            "dtmfDigits": analysis.dtmf_digits,
            "voicedDurationSecs": analysis.voiced_duration_secs,
            "sourceChannels": clip.source_channels,
            "spectrumPreview": analysis
                .spectrum_preview
                .iter()
                .map(|(f, m)| json!({ "freqHz": f, "magnitude": m }))
                .collect::<Vec<_>>(),
        }
    })))
}
