//! The HTTP client. Every provider call is a plain reqwest request against
//! the provider's public REST endpoint; response extraction is factored into
//! pure functions so the JSON handling is unit-testable offline.

use crate::error::LlmError;
use crate::{
    AudioFormat, Provider, SpeechRequest, TranscriptionOutcome, TranslationOutcome,
    TranslationRequest,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const DEFAULT_OPENAI_BASE: &str = "https://api.openai.com";
const DEFAULT_GEMINI_BASE: &str = "https://generativelanguage.googleapis.com";
const DEFAULT_ANTHROPIC_BASE: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";

const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";
const DEFAULT_GEMINI_MODEL: &str = "gemini-2.0-flash";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-5";
const DEFAULT_STT_MODEL: &str = "whisper-1";
const DEFAULT_TTS_MODEL: &str = "tts-1";
const DEFAULT_TTS_VOICE: &str = "alloy";

/// How much provider error body to keep in our own error/logs.
const ERROR_BODY_LIMIT: usize = 600;

#[derive(Debug, Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    openai_key: Option<String>,
    gemini_key: Option<String>,
    anthropic_key: Option<String>,
    openai_base: String,
    gemini_base: String,
    anthropic_base: String,
    openai_model: String,
    gemini_model: String,
    anthropic_model: String,
    stt_model: String,
    tts_model: String,
    tts_voice: String,
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn env_or(name: &str, default: &str) -> String {
    env_nonempty(name).unwrap_or_else(|| default.to_string())
}

impl LlmClient {
    /// Build from environment. Missing API keys are tolerated at construction
    /// and reported per-request, so a deployment with only one provider key
    /// still serves the others.
    pub fn from_env() -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(180))
            .build()
            .expect("reqwest client construction cannot fail with static config");
        Self {
            http,
            openai_key: env_nonempty("OPENAI_API_KEY"),
            gemini_key: env_nonempty("GEMINI_API_KEY").or_else(|| env_nonempty("GOOGLE_API_KEY")),
            anthropic_key: env_nonempty("ANTHROPIC_API_KEY"),
            openai_base: env_or("OPENAI_BASE_URL", DEFAULT_OPENAI_BASE),
            gemini_base: env_or("GEMINI_BASE_URL", DEFAULT_GEMINI_BASE),
            anthropic_base: env_or("ANTHROPIC_BASE_URL", DEFAULT_ANTHROPIC_BASE),
            openai_model: env_or("OPENAI_MODEL", DEFAULT_OPENAI_MODEL),
            gemini_model: env_or("GEMINI_MODEL", DEFAULT_GEMINI_MODEL),
            anthropic_model: env_or("ANTHROPIC_MODEL", DEFAULT_ANTHROPIC_MODEL),
            stt_model: env_or("OPENAI_STT_MODEL", DEFAULT_STT_MODEL),
            tts_model: env_or("OPENAI_TTS_MODEL", DEFAULT_TTS_MODEL),
            tts_voice: env_or("OPENAI_TTS_VOICE", DEFAULT_TTS_VOICE),
        }
    }

    /// Providers that have an API key configured right now.
    pub fn configured_providers(&self) -> Vec<Provider> {
        Provider::ALL
            .into_iter()
            .filter(|p| match p {
                Provider::OpenAi => self.openai_key.is_some(),
                Provider::Gemini => self.gemini_key.is_some(),
                Provider::Anthropic => self.anthropic_key.is_some(),
            })
            .collect()
    }

    pub fn model_for(&self, provider: Provider) -> &str {
        match provider {
            Provider::OpenAi => &self.openai_model,
            Provider::Gemini => &self.gemini_model,
            Provider::Anthropic => &self.anthropic_model,
        }
    }

    // ------------------------------------------------------------------
    // Translation
    // ------------------------------------------------------------------

    pub async fn translate(
        &self,
        req: &TranslationRequest,
    ) -> Result<TranslationOutcome, LlmError> {
        let started = Instant::now();
        let prompt = translation_system_prompt(req.source_lang.as_deref(), &req.target_lang);
        let text = match req.provider {
            Provider::OpenAi => self.translate_openai(&prompt, &req.text).await?,
            Provider::Gemini => self.translate_gemini(&prompt, &req.text).await?,
            Provider::Anthropic => self.translate_anthropic(&prompt, &req.text).await?,
        };
        Ok(TranslationOutcome {
            translated_text: text.trim().to_string(),
            provider: req.provider,
            model: self.model_for(req.provider).to_string(),
            latency_ms: started.elapsed().as_millis() as i64,
        })
    }

    async fn translate_openai(&self, system: &str, text: &str) -> Result<String, LlmError> {
        let key = self
            .openai_key
            .as_ref()
            .ok_or(LlmError::MissingApiKey("OPENAI_API_KEY"))?;
        let body = json!({
            "model": self.openai_model,
            "temperature": 0.2,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": text },
            ],
        });
        let resp = self
            .http
            .post(format!("{}/v1/chat/completions", self.openai_base))
            .bearer_auth(key)
            .json(&body)
            .send()
            .await?;
        let value = check_json(resp, "openai").await?;
        extract_openai_chat_text(&value)
    }

    async fn translate_gemini(&self, system: &str, text: &str) -> Result<String, LlmError> {
        let key = self
            .gemini_key
            .as_ref()
            .ok_or(LlmError::MissingApiKey("GEMINI_API_KEY"))?;
        let body = json!({
            "systemInstruction": { "parts": [ { "text": system } ] },
            "contents": [ { "role": "user", "parts": [ { "text": text } ] } ],
            "generationConfig": { "temperature": 0.2 },
        });
        let url = format!(
            "{}/v1beta/models/{}:generateContent",
            self.gemini_base, self.gemini_model
        );
        // Key goes in a header, not the query string, so it never lands in logs.
        let resp = self
            .http
            .post(url)
            .header("x-goog-api-key", key)
            .json(&body)
            .send()
            .await?;
        let value = check_json(resp, "gemini").await?;
        extract_gemini_text(&value)
    }

    async fn translate_anthropic(&self, system: &str, text: &str) -> Result<String, LlmError> {
        let key = self
            .anthropic_key
            .as_ref()
            .ok_or(LlmError::MissingApiKey("ANTHROPIC_API_KEY"))?;
        let body = json!({
            "model": self.anthropic_model,
            "max_tokens": 4096,
            "system": system,
            "messages": [ { "role": "user", "content": text } ],
        });
        let resp = self
            .http
            .post(format!("{}/v1/messages", self.anthropic_base))
            .header("x-api-key", key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await?;
        let value = check_json(resp, "anthropic").await?;
        extract_anthropic_text(&value)
    }

    // ------------------------------------------------------------------
    // Speech-to-text (OpenAI Whisper)
    // ------------------------------------------------------------------

    pub async fn transcribe(
        &self,
        wav_bytes: Vec<u8>,
        language_hint: Option<&str>,
    ) -> Result<TranscriptionOutcome, LlmError> {
        let key = self
            .openai_key
            .as_ref()
            .ok_or(LlmError::MissingApiKey("OPENAI_API_KEY"))?;
        let part = reqwest::multipart::Part::bytes(wav_bytes)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .expect("static mime string is valid");
        let mut form = reqwest::multipart::Form::new()
            .text("model", self.stt_model.clone())
            .text("response_format", "json")
            .part("file", part);
        if let Some(lang) = language_hint {
            form = form.text("language", lang.to_string());
        }
        let resp = self
            .http
            .post(format!("{}/v1/audio/transcriptions", self.openai_base))
            .bearer_auth(key)
            .multipart(form)
            .send()
            .await?;
        let value = check_json(resp, "openai").await?;
        let text = value
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| LlmError::Parse {
                provider: "openai",
                detail: "transcription response missing 'text'".into(),
            })?
            .trim()
            .to_string();
        Ok(TranscriptionOutcome {
            text,
            model: self.stt_model.clone(),
            language: value
                .get("language")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    // ------------------------------------------------------------------
    // Text-to-speech (OpenAI)
    // ------------------------------------------------------------------

    /// Returns raw audio bytes in the requested container format, plus the
    /// voice and model actually used.
    pub async fn synthesize(
        &self,
        req: &SpeechRequest,
    ) -> Result<(Vec<u8>, String, String), LlmError> {
        let key = self
            .openai_key
            .as_ref()
            .ok_or(LlmError::MissingApiKey("OPENAI_API_KEY"))?;
        let voice = req.voice.clone().unwrap_or_else(|| self.tts_voice.clone());
        let body = json!({
            "model": self.tts_model,
            "input": req.text,
            "voice": voice,
            "response_format": req.format.as_str(),
        });
        let resp = self
            .http
            .post(format!("{}/v1/audio/speech", self.openai_base))
            .bearer_auth(key)
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(api_error("openai", status.as_u16(), body));
        }
        let bytes = resp.bytes().await?.to_vec();
        Ok((bytes, voice, self.tts_model.clone()))
    }

    /// The formats OpenAI TTS emits directly (kept here so callers don't
    /// hardcode provider capabilities).
    pub fn tts_formats(&self) -> [AudioFormat; 2] {
        [AudioFormat::Wav, AudioFormat::Mp3]
    }
}

/// Shared prompt for all three providers — output must be the bare translation.
pub fn translation_system_prompt(source_lang: Option<&str>, target_lang: &str) -> String {
    let source_clause = match source_lang {
        Some(lang) if !lang.trim().is_empty() && lang.trim() != "auto" => {
            format!("from {} ", lang.trim())
        }
        _ => "detecting the source language automatically ".to_string(),
    };
    format!(
        "You are a professional translation engine. Translate the user's text \
         {source_clause}into {target_lang}. Preserve meaning, tone, register, \
         formatting, and any markup. Do not add commentary, quotes, or notes. \
         Output ONLY the translated text."
    )
}

async fn check_json(resp: reqwest::Response, provider: &'static str) -> Result<Value, LlmError> {
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(api_error(provider, status.as_u16(), body));
    }
    Ok(resp.json::<Value>().await?)
}

fn api_error(provider: &'static str, status: u16, body: String) -> LlmError {
    let mut body = body;
    if body.len() > ERROR_BODY_LIMIT {
        body.truncate(ERROR_BODY_LIMIT);
        body.push('…');
    }
    LlmError::Api {
        provider,
        status,
        body,
    }
}

// ---------------------------------------------------------------------------
// Response extractors — pure, unit-tested offline.
// ---------------------------------------------------------------------------

pub(crate) fn extract_openai_chat_text(value: &Value) -> Result<String, LlmError> {
    value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| LlmError::Parse {
            provider: "openai",
            detail: "choices[0].message.content missing".into(),
        })
}

pub(crate) fn extract_gemini_text(value: &Value) -> Result<String, LlmError> {
    let parts = value
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .ok_or_else(|| LlmError::Parse {
            provider: "gemini",
            detail: "candidates[0].content.parts missing".into(),
        })?;
    let text: String = parts
        .iter()
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() {
        return Err(LlmError::Parse {
            provider: "gemini",
            detail: "no text parts in candidate".into(),
        });
    }
    Ok(text)
}

pub(crate) fn extract_anthropic_text(value: &Value) -> Result<String, LlmError> {
    let blocks = value
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| LlmError::Parse {
            provider: "anthropic",
            detail: "content array missing".into(),
        })?;
    let text: String = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() {
        return Err(LlmError::Parse {
            provider: "anthropic",
            detail: "no text blocks in content".into(),
        });
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_chat_extraction() {
        let v = json!({"choices": [{"message": {"role": "assistant", "content": "Hola"}}]});
        assert_eq!(extract_openai_chat_text(&v).unwrap(), "Hola");
        assert!(extract_openai_chat_text(&json!({"choices": []})).is_err());
    }

    #[test]
    fn gemini_extraction_joins_parts() {
        let v =
            json!({"candidates": [{"content": {"parts": [{"text": "Bon"}, {"text": "jour"}]}}]});
        assert_eq!(extract_gemini_text(&v).unwrap(), "Bonjour");
        assert!(extract_gemini_text(&json!({})).is_err());
    }

    #[test]
    fn anthropic_extraction_filters_text_blocks() {
        let v = json!({"content": [
            {"type": "thinking", "thinking": "…"},
            {"type": "text", "text": "Guten Tag"}
        ]});
        assert_eq!(extract_anthropic_text(&v).unwrap(), "Guten Tag");
        assert!(extract_anthropic_text(&json!({"content": []})).is_err());
    }

    #[test]
    fn prompt_mentions_languages() {
        let p = translation_system_prompt(Some("English"), "Spanish");
        assert!(p.contains("from English"));
        assert!(p.contains("into Spanish"));
        let auto = translation_system_prompt(None, "French");
        assert!(auto.contains("detecting the source language automatically"));
        let auto2 = translation_system_prompt(Some("auto"), "French");
        assert!(auto2.contains("detecting the source language automatically"));
    }

    #[test]
    fn api_error_truncates_huge_bodies() {
        let e = api_error("openai", 500, "x".repeat(5000));
        match e {
            LlmError::Api { body, .. } => assert!(body.len() < 700),
            _ => panic!("wrong variant"),
        }
    }
}
