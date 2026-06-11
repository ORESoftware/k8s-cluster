//! Thin Qdrant REST client (the `dd-qdrant` Helm release in the `ai-ml`
//! namespace). We talk HTTP/JSON rather than pulling the `qdrant-client`
//! crate so the dependency surface stays small and matches the rest of this
//! service's "everything is an HTTP call" shape.

use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum QdrantError {
    #[error("qdrant transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("qdrant returned {status}: {body}")]
    Upstream { status: u16, body: String },
}

pub struct Qdrant {
    base_url: String,
    api_key: Option<String>,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct CollectionExistsResult {
    exists: bool,
}

#[derive(Deserialize)]
struct CollectionExistsResponse {
    result: CollectionExistsResult,
}

/// One scored hit from a search.
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct ScoredPoint {
    pub id: Value,
    pub score: f32,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Deserialize)]
struct SearchResponse {
    result: Vec<ScoredPoint>,
}

impl Qdrant {
    pub fn new(base_url: String, api_key: Option<String>, http: reqwest::Client) -> Self {
        Self { base_url: base_url.trim_end_matches('/').to_string(), api_key, http }
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let mut rb = self.http.request(method, url);
        if let Some(key) = &self.api_key {
            rb = rb.header("api-key", key);
        }
        rb
    }

    async fn check(resp: reqwest::Response) -> Result<reqwest::Response, QdrantError> {
        let status = resp.status();
        if status.is_success() {
            Ok(resp)
        } else {
            let body = resp.text().await.unwrap_or_default();
            Err(QdrantError::Upstream { status: status.as_u16(), body })
        }
    }

    /// Create the collection if it does not already exist. `size` is the
    /// vector dimensionality; `distance` is one of Qdrant's metrics.
    pub async fn ensure_collection(
        &self,
        collection: &str,
        size: usize,
        distance: &str,
    ) -> Result<(), QdrantError> {
        let exists_resp = self
            .req(reqwest::Method::GET, &format!("/collections/{collection}/exists"))
            .send()
            .await?;
        let exists_resp = Self::check(exists_resp).await?;
        let parsed: CollectionExistsResponse = exists_resp.json().await?;
        if parsed.result.exists {
            return Ok(());
        }

        let resp = self
            .req(reqwest::Method::PUT, &format!("/collections/{collection}"))
            .json(&json!({
                "vectors": { "size": size, "distance": distance }
            }))
            .send()
            .await?;
        // A concurrent first-index can race us to create the same collection;
        // treat "already exists" (409) as success rather than a hard error.
        if resp.status() == reqwest::StatusCode::CONFLICT {
            return Ok(());
        }
        Self::check(resp).await?;
        Ok(())
    }

    /// Upsert a batch of points (id + vector + payload).
    pub async fn upsert(&self, collection: &str, points: Vec<Value>) -> Result<(), QdrantError> {
        let resp = self
            .req(
                reqwest::Method::PUT,
                &format!("/collections/{collection}/points?wait=true"),
            )
            .json(&json!({ "points": points }))
            .send()
            .await?;
        Self::check(resp).await?;
        Ok(())
    }

    /// Vector search returning the top `limit` hits with payloads.
    pub async fn search(
        &self,
        collection: &str,
        vector: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<ScoredPoint>, QdrantError> {
        let resp = self
            .req(
                reqwest::Method::POST,
                &format!("/collections/{collection}/points/search"),
            )
            .json(&json!({
                "vector": vector,
                "limit": limit,
                "with_payload": true,
            }))
            .send()
            .await?;
        let resp = Self::check(resp).await?;
        let parsed: SearchResponse = resp.json().await?;
        Ok(parsed.result)
    }

    /// Liveness probe used by `/readyz` to confirm the vector store is reachable.
    pub async fn healthz(&self) -> Result<(), QdrantError> {
        let resp = self.req(reqwest::Method::GET, "/healthz").send().await?;
        Self::check(resp).await?;
        Ok(())
    }

    /// List collection names.
    pub async fn list_collections(&self) -> Result<Vec<String>, QdrantError> {
        let resp = self.req(reqwest::Method::GET, "/collections").send().await?;
        let resp = Self::check(resp).await?;
        let parsed: ListCollectionsResponse = resp.json().await?;
        Ok(parsed.result.collections.into_iter().map(|c| c.name).collect())
    }

    /// Delete an entire collection. Idempotent: a missing collection is fine.
    pub async fn delete_collection(&self, collection: &str) -> Result<(), QdrantError> {
        let resp = self
            .req(reqwest::Method::DELETE, &format!("/collections/{collection}"))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        Self::check(resp).await?;
        Ok(())
    }

    /// Delete points by id from a collection.
    pub async fn delete_points(&self, collection: &str, ids: Vec<Value>) -> Result<(), QdrantError> {
        let resp = self
            .req(
                reqwest::Method::POST,
                &format!("/collections/{collection}/points/delete?wait=true"),
            )
            .json(&json!({ "points": ids }))
            .send()
            .await?;
        Self::check(resp).await?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct CollectionDescription {
    name: String,
}

#[derive(Deserialize)]
struct CollectionsList {
    collections: Vec<CollectionDescription>,
}

#[derive(Deserialize)]
struct ListCollectionsResponse {
    result: CollectionsList,
}
