//! Cohere `POST /v1/embed`.
//!
//! Cohere's own format: `texts` array, a required `input_type`, and vectors
//! returned under `embeddings.float`.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::{
    validate_input, EmbedRequest, EmbedResponse, Embedding, EmbeddingProvider, InputType,
    ProviderError, Usage,
};

const URL: &str = "https://api.cohere.com/v1/embed";
const DEFAULT_MODEL: &str = "embed-english-v3.0";
const MODELS: &[&str] = &["embed-english-v3.0", "embed-multilingual-v3.0", "embed-english-light-v3.0"];

pub struct Cohere {
    api_key: String,
    http: reqwest::Client,
}

impl Cohere {
    pub fn new(api_key: String, http: reqwest::Client) -> Self {
        Self { api_key, http }
    }
}

#[derive(Deserialize)]
struct CohereFloatEmbeddings {
    float: Vec<Vec<f32>>,
}

#[derive(Deserialize)]
struct CohereBilledUnits {
    #[serde(default)]
    input_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct CohereMetaBilled {
    #[serde(default)]
    billed_units: Option<CohereBilledUnits>,
}

#[derive(Deserialize)]
struct CohereMeta {
    #[serde(default)]
    billed_units: Option<CohereBilledUnits>,
    // Some responses nest under `meta.billed_units`; keep a fallback.
    #[serde(default)]
    meta: Option<CohereMetaBilled>,
}

#[derive(Deserialize)]
struct CohereResponse {
    embeddings: CohereFloatEmbeddings,
    #[serde(default)]
    meta: Option<CohereMeta>,
}

#[async_trait]
impl EmbeddingProvider for Cohere {
    fn id(&self) -> &str {
        "cohere"
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
            InputType::Query => "search_query",
            InputType::Document => "search_document",
        };

        let resp = self
            .http
            .post(URL)
            .bearer_auth(&self.api_key)
            .json(&json!({
                "model": model,
                "texts": req.input,
                "input_type": input_type,
                "embedding_types": ["float"],
            }))
            .send()
            .await
            .map_err(|e| ProviderError::Transport { provider: "cohere".into(), source: e })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Upstream {
                provider: "cohere".into(),
                status: status.as_u16(),
                body,
            });
        }

        let parsed: CohereResponse = resp
            .json()
            .await
            .map_err(|_| ProviderError::Decode("cohere".into(), "embeddings.float"))?;

        let embeddings: Vec<Embedding> = parsed
            .embeddings
            .float
            .into_iter()
            .enumerate()
            .map(|(index, vector)| Embedding { index, vector })
            .collect();
        let dimensions = embeddings.first().map(|e| e.vector.len()).unwrap_or(0);

        let prompt_tokens = parsed
            .meta
            .and_then(|m| m.billed_units.or(m.meta.and_then(|x| x.billed_units)))
            .and_then(|b| b.input_tokens);

        Ok(EmbedResponse {
            provider: "cohere".into(),
            model,
            dimensions,
            embeddings,
            usage: Usage { prompt_tokens, total_tokens: prompt_tokens },
        })
    }
}
