//! Reranking providers — the second-stage of a RAG pipeline. Given a query and
//! a candidate set, a cross-encoder reranker scores each candidate's relevance
//! far more accurately than embedding cosine alone. Cohere, Jina, and Voyage
//! all expose one; each has its own request/response shape, unified here behind
//! [`RerankProvider`] into a single `{index, score}` ranking.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::ProviderError;
use crate::config::Config;

/// A rerank request: score each `documents[i]` against `query`.
#[derive(Debug, Clone, Deserialize)]
pub struct RerankRequest {
    pub query: String,
    pub documents: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Return only the top N rankings (capped by the server's max_top_k).
    #[serde(default)]
    pub top_n: Option<usize>,
}

/// One scored candidate, `index` referring to the input `documents`.
#[derive(Debug, Clone, Serialize)]
pub struct Ranking {
    pub index: usize,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct RerankResponse {
    pub provider: String,
    pub model: String,
    pub results: Vec<Ranking>,
}

#[async_trait]
pub trait RerankProvider: Send + Sync {
    fn id(&self) -> &str;
    fn default_model(&self) -> &str;
    fn known_models(&self) -> &[&str];
    async fn rerank(&self, req: &RerankRequest) -> Result<RerankResponse, ProviderError>;
}

pub struct RerankRegistry {
    providers: BTreeMap<String, Arc<dyn RerankProvider>>,
    aliases: BTreeMap<String, String>,
}

impl RerankRegistry {
    pub fn from_config(cfg: &Config, http: reqwest::Client) -> Self {
        let mut providers: BTreeMap<String, Arc<dyn RerankProvider>> = BTreeMap::new();
        if let Some(key) = cfg.provider_key("COHERE_API_KEY") {
            providers.insert("cohere".into(), Arc::new(CohereRerank::new(key, http.clone())));
        }
        if let Some(key) = cfg.provider_key("JINA_API_KEY") {
            providers.insert("jina".into(), Arc::new(JinaRerank::new(key, http.clone())));
        }
        if let Some(key) = cfg.provider_key("VOYAGE_API_KEY") {
            providers.insert("voyage".into(), Arc::new(VoyageRerank::new(key, http.clone())));
        }
        let mut aliases = BTreeMap::new();
        if providers.contains_key("voyage") {
            // Same Anthropic→Voyage convention as the embedding side.
            aliases.insert("anthropic".to_string(), "voyage".to_string());
        }
        Self { providers, aliases }
    }

    pub fn resolve(&self, id: &str) -> Result<&Arc<dyn RerankProvider>, ProviderError> {
        let canonical = self.aliases.get(id).map(String::as_str).unwrap_or(id);
        self.providers.get(canonical).ok_or_else(|| {
            if self.providers.is_empty() {
                ProviderError::NotConfigured(id.to_string())
            } else {
                ProviderError::Unknown(id.to_string())
            }
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn RerankProvider>> {
        self.providers.values()
    }

    pub fn aliases(&self) -> &BTreeMap<String, String> {
        &self.aliases
    }

    pub fn len(&self) -> usize {
        self.providers.len()
    }
}

// --- shared helpers ---------------------------------------------------------

async fn post_json(
    http: &reqwest::Client,
    provider: &str,
    url: &str,
    key: &str,
    body: Value,
) -> Result<reqwest::Response, ProviderError> {
    let resp = http
        .post(url)
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .map_err(|e| ProviderError::Transport { provider: provider.into(), source: e })?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(ProviderError::Upstream { provider: provider.into(), status: status.as_u16(), body });
    }
    Ok(resp)
}

/// Cohere and Voyage both return `{... : [{ index, relevance_score }]}`; only
/// the top-level array key differs.
#[derive(Deserialize)]
struct ScoredIndex {
    index: usize,
    relevance_score: f32,
}

fn to_rankings(items: Vec<ScoredIndex>) -> Vec<Ranking> {
    items.into_iter().map(|r| Ranking { index: r.index, score: r.relevance_score }).collect()
}

// --- Cohere -----------------------------------------------------------------

pub struct CohereRerank {
    api_key: String,
    http: reqwest::Client,
}
impl CohereRerank {
    fn new(api_key: String, http: reqwest::Client) -> Self {
        Self { api_key, http }
    }
}

#[derive(Deserialize)]
struct CohereRerankResponse {
    results: Vec<ScoredIndex>,
}

#[async_trait]
impl RerankProvider for CohereRerank {
    fn id(&self) -> &str {
        "cohere"
    }
    fn default_model(&self) -> &str {
        "rerank-v3.5"
    }
    fn known_models(&self) -> &[&str] {
        &["rerank-v3.5", "rerank-english-v3.0", "rerank-multilingual-v3.0"]
    }
    async fn rerank(&self, req: &RerankRequest) -> Result<RerankResponse, ProviderError> {
        let model = req.model.clone().unwrap_or_else(|| self.default_model().to_string());
        let mut body = json!({ "model": model, "query": req.query, "documents": req.documents });
        if let Some(n) = req.top_n {
            body["top_n"] = json!(n);
        }
        let resp = post_json(&self.http, "cohere", "https://api.cohere.com/v2/rerank", &self.api_key, body).await?;
        let parsed: CohereRerankResponse = resp
            .json()
            .await
            .map_err(|_| ProviderError::Decode("cohere".into(), "results[].relevance_score"))?;
        Ok(RerankResponse { provider: "cohere".into(), model, results: to_rankings(parsed.results) })
    }
}

// --- Jina -------------------------------------------------------------------

pub struct JinaRerank {
    api_key: String,
    http: reqwest::Client,
}
impl JinaRerank {
    fn new(api_key: String, http: reqwest::Client) -> Self {
        Self { api_key, http }
    }
}

#[derive(Deserialize)]
struct JinaRerankResponse {
    results: Vec<ScoredIndex>,
}

#[async_trait]
impl RerankProvider for JinaRerank {
    fn id(&self) -> &str {
        "jina"
    }
    fn default_model(&self) -> &str {
        "jina-reranker-v2-base-multilingual"
    }
    fn known_models(&self) -> &[&str] {
        &["jina-reranker-v2-base-multilingual", "jina-colbert-v2"]
    }
    async fn rerank(&self, req: &RerankRequest) -> Result<RerankResponse, ProviderError> {
        let model = req.model.clone().unwrap_or_else(|| self.default_model().to_string());
        let mut body = json!({ "model": model, "query": req.query, "documents": req.documents });
        if let Some(n) = req.top_n {
            body["top_n"] = json!(n);
        }
        let resp = post_json(&self.http, "jina", "https://api.jina.ai/v1/rerank", &self.api_key, body).await?;
        let parsed: JinaRerankResponse = resp
            .json()
            .await
            .map_err(|_| ProviderError::Decode("jina".into(), "results[].relevance_score"))?;
        Ok(RerankResponse { provider: "jina".into(), model, results: to_rankings(parsed.results) })
    }
}

// --- Voyage -----------------------------------------------------------------

pub struct VoyageRerank {
    api_key: String,
    http: reqwest::Client,
}
impl VoyageRerank {
    fn new(api_key: String, http: reqwest::Client) -> Self {
        Self { api_key, http }
    }
}

#[derive(Deserialize)]
struct VoyageRerankResponse {
    data: Vec<ScoredIndex>,
}

#[async_trait]
impl RerankProvider for VoyageRerank {
    fn id(&self) -> &str {
        "voyage"
    }
    fn default_model(&self) -> &str {
        "rerank-2"
    }
    fn known_models(&self) -> &[&str] {
        &["rerank-2", "rerank-2-lite"]
    }
    async fn rerank(&self, req: &RerankRequest) -> Result<RerankResponse, ProviderError> {
        let model = req.model.clone().unwrap_or_else(|| self.default_model().to_string());
        let mut body = json!({ "model": model, "query": req.query, "documents": req.documents });
        if let Some(n) = req.top_n {
            body["top_k"] = json!(n); // Voyage calls it top_k
        }
        let resp = post_json(&self.http, "voyage", "https://api.voyageai.com/v1/rerank", &self.api_key, body).await?;
        let parsed: VoyageRerankResponse = resp
            .json()
            .await
            .map_err(|_| ProviderError::Decode("voyage".into(), "data[].relevance_score"))?;
        Ok(RerankResponse { provider: "voyage".into(), model, results: to_rankings(parsed.data) })
    }
}
