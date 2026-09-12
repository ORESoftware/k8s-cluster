//! Voyage AI `POST /v1/embeddings`.
//!
//! This is also the upstream behind the `anthropic` alias: Anthropic does not
//! offer a first-party embeddings endpoint and points users at Voyage in their
//! own docs, so "give me Anthropic embeddings" resolves here.
//!
//! Voyage is close to the OpenAI shape but adds an `input_type` of
//! `query`/`document` and an `output_dimension` knob, so it gets a small
//! dedicated impl rather than reusing the OpenAI adapter.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    validate_input, EmbedRequest, EmbedResponse, Embedding, EmbeddingProvider, InputType,
    ProviderError, Usage,
};

const URL: &str = "https://api.voyageai.com/v1/embeddings";
const DEFAULT_MODEL: &str = "voyage-3";
const MODELS: &[&str] = &["voyage-3", "voyage-3-lite", "voyage-code-3", "voyage-finance-2", "voyage-law-2"];

pub struct Voyage {
    api_key: String,
    http: reqwest::Client,
}

impl Voyage {
    pub fn new(api_key: String, http: reqwest::Client) -> Self {
        Self { api_key, http }
    }
}

#[derive(Deserialize)]
struct VoyageItem {
    index: usize,
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct VoyageUsage {
    #[serde(default)]
    total_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct VoyageResponse {
    data: Vec<VoyageItem>,
    #[serde(default)]
    usage: Option<VoyageUsage>,
}

#[async_trait]
impl EmbeddingProvider for Voyage {
    fn id(&self) -> &str {
        "voyage"
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
        let input_type = match req.input_type {
            InputType::Query => "query",
            InputType::Document => "document",
        };

        let mut body = json!({
            "model": model,
            "input": req.input,
            "input_type": input_type,
        });
        if let Some(dims) = req.dimensions {
            body["output_dimension"] = Value::from(dims);
        }

        let resp = self
            .http
            .post(URL)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport { provider: "voyage".into(), source: e })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Upstream {
                provider: "voyage".into(),
                status: status.as_u16(),
                body,
            });
        }

        let parsed: VoyageResponse = resp
            .json()
            .await
            .map_err(|_| ProviderError::Decode("voyage".into(), "data[].embedding"))?;

        let mut embeddings: Vec<Embedding> = parsed
            .data
            .into_iter()
            .map(|d| Embedding { index: d.index, vector: d.embedding })
            .collect();
        embeddings.sort_by_key(|e| e.index);
        let dimensions = embeddings.first().map(|e| e.vector.len()).unwrap_or(0);

        Ok(EmbedResponse {
            provider: "voyage".into(),
            model,
            dimensions,
            embeddings,
            usage: Usage {
                prompt_tokens: None,
                total_tokens: parsed.usage.and_then(|u| u.total_tokens),
            },
        })
    }
}
