//! Google Gemini embeddings (`:batchEmbedContents` on generativelanguage).
//!
//! Distinct wire format from OpenAI: contents are wrapped objects, the model
//! is in the URL path, the key is a query param, and retrieval intent is the
//! `taskType` field.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    validate_input, EmbedRequest, EmbedResponse, Embedding, EmbeddingProvider, InputType,
    ProviderError, Usage,
};

const BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const DEFAULT_MODEL: &str = "text-embedding-004";
const MODELS: &[&str] = &["text-embedding-004", "embedding-001"];

pub struct Gemini {
    api_key: String,
    http: reqwest::Client,
}

impl Gemini {
    pub fn new(api_key: String, http: reqwest::Client) -> Self {
        Self { api_key, http }
    }
}

#[derive(Deserialize)]
struct GeminiValues {
    values: Vec<f32>,
}

#[derive(Deserialize)]
struct GeminiBatchResponse {
    embeddings: Vec<GeminiValues>,
}

#[async_trait]
impl EmbeddingProvider for Gemini {
    fn id(&self) -> &str {
        "gemini"
    }

    fn default_model(&self) -> &str {
        DEFAULT_MODEL
    }

    fn known_models(&self) -> &[&str] {
        MODELS
    }

    async fn embed(&self, req: &EmbedRequest) -> Result<EmbedResponse, ProviderError> {
        validate_input(req)?;
        let model = req.model.clone().unwrap_or_else(|| DEFAULT_MODEL.to_string());
        // Gemini is the one provider that puts the model in the request URL
        // path, so it gets a strict charset check the loose global validator
        // doesn't enforce — no `/`, `:`, `?`, `#`, etc. that could alter the
        // path or inject a query string.
        if !model.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
            return Err(ProviderError::InvalidModel(
                "gemini".into(),
                "Gemini model names may contain only [A-Za-z0-9._-]".into(),
            ));
        }
        let task_type = match req.input_type {
            InputType::Query => "RETRIEVAL_QUERY",
            InputType::Document => "RETRIEVAL_DOCUMENT",
        };

        // Gemini wants the model id prefixed with `models/` in each request.
        let model_path = format!("models/{model}");
        let requests: Vec<Value> = req
            .input
            .iter()
            .map(|text| {
                let mut r = json!({
                    "model": model_path,
                    "content": { "parts": [{ "text": text }] },
                    "taskType": task_type,
                });
                if let Some(dims) = req.dimensions {
                    r["outputDimensionality"] = Value::from(dims);
                }
                r
            })
            .collect();

        let url = format!("{BASE_URL}/{model_path}:batchEmbedContents");
        let resp = self
            .http
            .post(&url)
            // Pass the key as a header, not a `?key=` query param, so it can't
            // leak through URL-level logging, proxies, or error surfaces.
            .header("x-goog-api-key", &self.api_key)
            .json(&json!({ "requests": requests }))
            .send()
            .await
            .map_err(|e| ProviderError::Transport { provider: "gemini".into(), source: e })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Upstream {
                provider: "gemini".into(),
                status: status.as_u16(),
                body,
            });
        }

        let parsed: GeminiBatchResponse = resp
            .json()
            .await
            .map_err(|_| ProviderError::Decode("gemini".into(), "embeddings[].values"))?;

        let embeddings: Vec<Embedding> = parsed
            .embeddings
            .into_iter()
            .enumerate()
            .map(|(index, e)| Embedding { index, vector: e.values })
            .collect();
        let dimensions = embeddings.first().map(|e| e.vector.len()).unwrap_or(0);

        Ok(EmbedResponse {
            provider: "gemini".into(),
            model,
            dimensions,
            embeddings,
            usage: Usage::default(),
        })
    }
}
