//! Embedding-provider abstraction.
//!
//! Every upstream is reduced to one trait, [`EmbeddingProvider`], that takes a
//! normalized [`EmbedRequest`] and returns a normalized [`EmbedResponse`].
//! The bulk of the industry exposes an OpenAI-shaped `POST /embeddings`, so
//! [`openai::OpenAiCompatible`] covers most of the roster; the few that don't
//! (Google Gemini, Cohere, Voyage) get their own modules.

pub mod cohere;
pub mod gemini;
pub mod openai;
pub mod voyage;

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider `{0}` is not configured (missing API key?)")]
    NotConfigured(String),
    #[error("unknown provider `{0}`")]
    Unknown(String),
    #[error("empty input: at least one non-empty string is required")]
    EmptyInput,
    #[error("transport error talking to {provider}: {source}")]
    Transport {
        provider: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("upstream {provider} returned {status}: {body}")]
    Upstream {
        provider: String,
        status: u16,
        body: String,
    },
    #[error("could not decode {provider} response: {0}")]
    Decode(String, &'static str),
}

/// What a caller asks for, independent of which upstream serves it.
#[derive(Debug, Clone, Deserialize)]
pub struct EmbedRequest {
    /// One or more texts to embed.
    pub input: Vec<String>,
    /// Override the provider's default model. Optional.
    #[serde(default)]
    pub model: Option<String>,
    /// Requested output dimensionality. Honored only by providers/models that
    /// support Matryoshka truncation (OpenAI v3, Voyage, Gemini, Nomic, ...);
    /// silently ignored elsewhere.
    #[serde(default)]
    pub dimensions: Option<u32>,
    /// Retrieval hint. Some providers (Voyage, Cohere, Gemini, Nomic) embed
    /// queries and documents into different sub-spaces and need to be told
    /// which side this text is. Defaults to `Document` for indexing safety.
    #[serde(default)]
    pub input_type: InputType,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    #[default]
    Document,
    Query,
}

/// One embedding vector plus its position in the request.
#[derive(Debug, Clone, Serialize)]
pub struct Embedding {
    pub index: usize,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Usage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
}

/// Normalized result returned to callers.
#[derive(Debug, Clone, Serialize)]
pub struct EmbedResponse {
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
    pub embeddings: Vec<Embedding>,
    pub usage: Usage,
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Stable provider id used in the API (`openai`, `voyage`, `gemini`, ...).
    fn id(&self) -> &str;
    /// Model used when the request doesn't override it.
    fn default_model(&self) -> &str;
    /// Models this provider advertises (for `/api/providers`); not exhaustive.
    fn known_models(&self) -> &[&str];
    async fn embed(&self, req: &EmbedRequest) -> Result<EmbedResponse, ProviderError>;
}

/// Holds every provider that booted with credentials present.
pub struct Registry {
    providers: BTreeMap<String, Arc<dyn EmbeddingProvider>>,
    /// Soft aliases (`anthropic` -> `voyage`) resolved before lookup.
    aliases: BTreeMap<String, String>,
}

impl Registry {
    /// Build the roster from config. A provider is registered only if its API
    /// key is present (or it's keyless, like a self-hosted Ollama/TEI). This
    /// is intentionally permissive: the pod boots fine with zero providers and
    /// you wire them up by adding keys to the secret later.
    pub fn from_config(cfg: &Config, http: reqwest::Client) -> Self {
        let mut providers: BTreeMap<String, Arc<dyn EmbeddingProvider>> = BTreeMap::new();

        let mut add = |p: Arc<dyn EmbeddingProvider>| {
            providers.insert(p.id().to_string(), p);
        };

        // --- OpenAI-compatible upstreams (the majority) -------------------
        for spec in openai::default_specs() {
            if let Some(key) = cfg.provider_key(spec.key_env) {
                add(Arc::new(openai::OpenAiCompatible::new(spec, key, http.clone())));
            } else if spec.keyless {
                // Self-hosted (Ollama, HF TEI) — no key required.
                add(Arc::new(openai::OpenAiCompatible::new(spec, String::new(), http.clone())));
            }
        }

        // --- Bespoke wire formats ----------------------------------------
        if let Some(key) = cfg.provider_key("GEMINI_API_KEY") {
            add(Arc::new(gemini::Gemini::new(key, http.clone())));
        }
        if let Some(key) = cfg.provider_key("COHERE_API_KEY") {
            add(Arc::new(cohere::Cohere::new(key, http.clone())));
        }
        if let Some(key) = cfg.provider_key("VOYAGE_API_KEY") {
            add(Arc::new(voyage::Voyage::new(key, http.clone())));
        }

        // Anthropic has no embeddings API; route the alias to Voyage, which is
        // what Anthropic officially recommends. The alias only resolves if
        // Voyage actually registered.
        let mut aliases = BTreeMap::new();
        if providers.contains_key("voyage") {
            aliases.insert("anthropic".to_string(), "voyage".to_string());
        }

        Self { providers, aliases }
    }

    pub fn resolve<'a>(&self, id: &'a str) -> Result<&Arc<dyn EmbeddingProvider>, ProviderError> {
        let canonical = self.aliases.get(id).map(String::as_str).unwrap_or(id);
        self.providers
            .get(canonical)
            .ok_or_else(|| match self.providers.is_empty() {
                true => ProviderError::NotConfigured(id.to_string()),
                false => ProviderError::Unknown(id.to_string()),
            })
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn EmbeddingProvider>> {
        self.providers.values()
    }

    pub fn aliases(&self) -> &BTreeMap<String, String> {
        &self.aliases
    }

    pub fn len(&self) -> usize {
        self.providers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

/// Shared helper: reject input that is empty or all-blank before we spend a
/// network round trip on it.
pub fn validate_input(req: &EmbedRequest) -> Result<(), ProviderError> {
    if req.input.is_empty() || req.input.iter().all(|s| s.trim().is_empty()) {
        return Err(ProviderError::EmptyInput);
    }
    Ok(())
}
