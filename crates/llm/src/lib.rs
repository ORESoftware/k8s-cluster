//! t2v-llm — raw-HTTP clients for the AI providers.
//!
//! No provider SDKs (fleet convention, same as the Vapi screener's reqwest
//! pattern): OpenAI, Google Gemini, and Anthropic are called directly at
//! their REST endpoints. Base URLs are env-overridable so tests and
//! air-gapped deployments can point at stand-ins.
//!
//! Capabilities:
//! * translation between natural languages via any of the three providers
//! * speech-to-text via OpenAI Whisper (`/v1/audio/transcriptions`)
//! * text-to-speech via OpenAI (`/v1/audio/speech`)

mod client;
mod error;

pub use client::LlmClient;
pub use error::LlmError;

use std::str::FromStr;

/// Which LLM performs a translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAi,
    Gemini,
    Anthropic,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::OpenAi => "openai",
            Provider::Gemini => "gemini",
            Provider::Anthropic => "anthropic",
        }
    }

    pub const ALL: [Provider; 3] = [Provider::OpenAi, Provider::Gemini, Provider::Anthropic];
}

impl FromStr for Provider {
    type Err = LlmError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openai" | "chatgpt" | "gpt" => Ok(Provider::OpenAi),
            "gemini" | "google" => Ok(Provider::Gemini),
            "anthropic" | "claude" => Ok(Provider::Anthropic),
            other => Err(LlmError::UnknownProvider(other.to_string())),
        }
    }
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct TranslationRequest {
    pub text: String,
    /// None = let the model detect the source language.
    pub source_lang: Option<String>,
    pub target_lang: String,
    pub provider: Provider,
}

#[derive(Debug, Clone)]
pub struct TranslationOutcome {
    pub translated_text: String,
    pub provider: Provider,
    pub model: String,
    pub latency_ms: i64,
}

#[derive(Debug, Clone)]
pub struct TranscriptionOutcome {
    pub text: String,
    pub model: String,
    /// Language reported by the STT provider, when present.
    pub language: Option<String>,
}

/// Output container format for TTS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    Wav,
    Mp3,
}

impl AudioFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            AudioFormat::Wav => "wav",
            AudioFormat::Mp3 => "mp3",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            AudioFormat::Wav => "audio/wav",
            AudioFormat::Mp3 => "audio/mpeg",
        }
    }
}

impl FromStr for AudioFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "wav" => Ok(AudioFormat::Wav),
            "mp3" => Ok(AudioFormat::Mp3),
            other => Err(format!("unsupported audio format '{other}' (wav|mp3)")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpeechRequest {
    pub text: String,
    /// Provider voice id; None uses the configured default.
    pub voice: Option<String>,
    pub format: AudioFormat,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_aliases_parse() {
        assert_eq!("ChatGPT".parse::<Provider>().unwrap(), Provider::OpenAi);
        assert_eq!("claude".parse::<Provider>().unwrap(), Provider::Anthropic);
        assert_eq!("google".parse::<Provider>().unwrap(), Provider::Gemini);
        assert!("mistral".parse::<Provider>().is_err());
    }

    #[test]
    fn audio_format_parses_and_maps_content_type() {
        assert_eq!("WAV".parse::<AudioFormat>().unwrap(), AudioFormat::Wav);
        assert_eq!(AudioFormat::Mp3.content_type(), "audio/mpeg");
        assert!("ogg".parse::<AudioFormat>().is_err());
    }
}
