//! OpenAI-compatible `POST /v1/embeddings` implementation.
//!
//! This one struct backs every upstream that speaks the OpenAI embeddings
//! wire format: OpenAI itself, Mistral, Jina, Together, Fireworks, DeepInfra,
//! Nomic, Azure OpenAI, plus self-hosted Ollama and HF Text-Embeddings-
//! Inference. They differ only in base URL, default model, auth header, and
//! whether `dimensions` is accepted — all captured in [`OpenAiSpec`].

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    validate_input, EmbedRequest, EmbedResponse, Embedding, EmbeddingProvider, ProviderError,
    Usage,
};

#[derive(Clone, Copy)]
pub enum Auth {
    /// `Authorization: Bearer <key>` — the common case.
    Bearer,
    /// `api-key: <key>` — Azure OpenAI.
    ApiKeyHeader,
    /// No auth header — self-hosted Ollama / TEI.
    None,
}

#[derive(Clone)]
pub struct OpenAiSpec {
    pub id: &'static str,
    pub base_url: &'static str,
    pub default_model: &'static str,
    pub models: &'static [&'static str],
    /// Env var the API key is read from.
    pub key_env: &'static str,
    pub auth: Auth,
    /// Whether the upstream honors the `dimensions` field.
    pub supports_dimensions: bool,
    /// True for upstreams that need no key (self-hosted).
    pub keyless: bool,
}

/// The OpenAI-compatible roster. `base_url` already includes the path up to
/// (but not including) `/embeddings`.
pub fn default_specs() -> Vec<OpenAiSpec> {
    vec![
        OpenAiSpec {
            id: "openai",
            base_url: "https://api.openai.com/v1",
            default_model: "text-embedding-3-small",
            models: &["text-embedding-3-small", "text-embedding-3-large", "text-embedding-ada-002"],
            key_env: "OPENAI_API_KEY",
            auth: Auth::Bearer,
            supports_dimensions: true,
            keyless: false,
        },
        OpenAiSpec {
            id: "mistral",
            base_url: "https://api.mistral.ai/v1",
            default_model: "mistral-embed",
            models: &["mistral-embed"],
            key_env: "MISTRAL_API_KEY",
            auth: Auth::Bearer,
            supports_dimensions: false,
            keyless: false,
        },
        OpenAiSpec {
            id: "jina",
            base_url: "https://api.jina.ai/v1",
            default_model: "jina-embeddings-v3",
            models: &["jina-embeddings-v3", "jina-embeddings-v2-base-en"],
            key_env: "JINA_API_KEY",
            auth: Auth::Bearer,
            supports_dimensions: true,
            keyless: false,
        },
        OpenAiSpec {
            id: "together",
            base_url: "https://api.together.xyz/v1",
            default_model: "BAAI/bge-large-en-v1.5",
            models: &["BAAI/bge-large-en-v1.5", "togethercomputer/m2-bert-80M-8k-retrieval"],
            key_env: "TOGETHER_API_KEY",
            auth: Auth::Bearer,
            supports_dimensions: false,
            keyless: false,
        },
        OpenAiSpec {
            id: "fireworks",
            base_url: "https://api.fireworks.ai/inference/v1",
            default_model: "nomic-ai/nomic-embed-text-v1.5",
            models: &["nomic-ai/nomic-embed-text-v1.5"],
            key_env: "FIREWORKS_API_KEY",
            auth: Auth::Bearer,
            supports_dimensions: true,
            keyless: false,
        },
        OpenAiSpec {
            id: "deepinfra",
            base_url: "https://api.deepinfra.com/v1/openai",
            default_model: "BAAI/bge-large-en-v1.5",
            models: &["BAAI/bge-large-en-v1.5", "intfloat/e5-large-v2"],
            key_env: "DEEPINFRA_API_KEY",
            auth: Auth::Bearer,
            supports_dimensions: false,
            keyless: false,
        },
        OpenAiSpec {
            id: "nomic",
            base_url: "https://api-atlas.nomic.ai/v1",
            default_model: "nomic-embed-text-v1.5",
            models: &["nomic-embed-text-v1.5"],
            key_env: "NOMIC_API_KEY",
            auth: Auth::Bearer,
            supports_dimensions: true,
            keyless: false,
        },
        OpenAiSpec {
            id: "azure",
            // Set AZURE_OPENAI_BASE_URL to the full resource path; this default
            // is a placeholder that won't resolve without configuration.
            base_url: "https://YOUR-RESOURCE.openai.azure.com/openai/deployments/text-embedding-3-small",
            default_model: "text-embedding-3-small",
            models: &["text-embedding-3-small", "text-embedding-3-large"],
            key_env: "AZURE_OPENAI_API_KEY",
            auth: Auth::ApiKeyHeader,
            supports_dimensions: true,
            keyless: false,
        },
        OpenAiSpec {
            id: "tei",
            // Self-hosted HuggingFace Text Embeddings Inference, in-cluster.
            base_url: "http://dd-tei.ai-ml.svc.cluster.local/v1",
            default_model: "BAAI/bge-large-en-v1.5",
            models: &["BAAI/bge-large-en-v1.5"],
            key_env: "TEI_API_KEY",
            auth: Auth::None,
            supports_dimensions: false,
            keyless: true,
        },
        OpenAiSpec {
            id: "ollama",
            // Self-hosted Ollama, in-cluster.
            base_url: "http://dd-ollama.ai-ml.svc.cluster.local:11434/v1",
            default_model: "nomic-embed-text",
            models: &["nomic-embed-text", "mxbai-embed-large"],
            key_env: "OLLAMA_API_KEY",
            auth: Auth::None,
            supports_dimensions: false,
            keyless: true,
        },
    ]
}

pub struct OpenAiCompatible {
    spec: OpenAiSpec,
    api_key: String,
    http: reqwest::Client,
    base_url: String,
}

impl OpenAiCompatible {
    pub fn new(spec: OpenAiSpec, api_key: String, http: reqwest::Client) -> Self {
        // Allow a per-provider base-URL override (Azure resources, self-hosted
        // endpoints) via `<ID_UPPER>_BASE_URL`.
        let override_env = format!("{}_BASE_URL", spec.id.to_uppercase());
        let base_url = std::env::var(override_env).unwrap_or_else(|_| spec.base_url.to_string());
        Self { spec, api_key, http, base_url }
    }
}

#[derive(Deserialize)]
struct OpenAiEmbeddingItem {
    index: usize,
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    total_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct OpenAiEmbeddingsResponse {
    data: Vec<OpenAiEmbeddingItem>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[async_trait]
impl EmbeddingProvider for OpenAiCompatible {
    fn id(&self) -> &str {
        self.spec.id
    }

    fn default_model(&self) -> &str {
        self.spec.default_model
    }

    fn known_models(&self) -> &[&str] {
        self.spec.models
    }

    async fn embed(&self, req: &EmbedRequest) -> Result<EmbedResponse, ProviderError> {
        validate_input(req)?;
        let model = req.model.clone().unwrap_or_else(|| self.spec.default_model.to_string());

        let mut body = json!({
            "model": model,
            "input": req.input,
            "encoding_format": "float",
        });
        if self.spec.supports_dimensions {
            if let Some(dims) = req.dimensions {
                body["dimensions"] = Value::from(dims);
            }
        }

        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));
        let mut rb = self.http.post(&url).json(&body);
        rb = match self.spec.auth {
            Auth::Bearer => rb.bearer_auth(&self.api_key),
            Auth::ApiKeyHeader => rb.header("api-key", &self.api_key),
            Auth::None => rb,
        };

        let resp = rb.send().await.map_err(|e| ProviderError::Transport {
            provider: self.spec.id.to_string(),
            source: e,
        })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Upstream {
                provider: self.spec.id.to_string(),
                status: status.as_u16(),
                body: truncate(&body, 600),
            });
        }

        let parsed: OpenAiEmbeddingsResponse = resp
            .json()
            .await
            .map_err(|_| ProviderError::Decode(self.spec.id.to_string(), "data[].embedding"))?;

        let mut embeddings: Vec<Embedding> = parsed
            .data
            .into_iter()
            .map(|d| Embedding { index: d.index, vector: d.embedding })
            .collect();
        embeddings.sort_by_key(|e| e.index);

        let dimensions = embeddings.first().map(|e| e.vector.len()).unwrap_or(0);
        let usage = parsed.usage.map(|u| Usage {
            prompt_tokens: u.prompt_tokens,
            total_tokens: u.total_tokens,
        });

        Ok(EmbedResponse {
            provider: self.spec.id.to_string(),
            model,
            dimensions,
            embeddings,
            usage: usage.unwrap_or_default(),
        })
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Truncate on a char boundary so we never split a UTF-8 sequence.
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}
