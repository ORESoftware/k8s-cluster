//! RAG indexing service: embed documents with any configured provider and
//! upsert them into Qdrant, then embed a query and retrieve nearest neighbors.
//!
//! This is the glue that turns the embedding gateway into a usable retrieval
//! layer. The embedding provider and Qdrant are both just HTTP dependencies,
//! so this module is mostly orchestration + id/payload bookkeeping.

pub mod qdrant;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::providers::{EmbedRequest, InputType, ProviderError, Registry};
use qdrant::{Qdrant, QdrantError, ScoredPoint};

#[derive(Debug, thiserror::Error)]
pub enum RagError {
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Qdrant(#[from] QdrantError),
    #[error("no documents provided")]
    NoDocuments,
    #[error("provider returned {got} vectors for {expected} documents")]
    CountMismatch { got: usize, expected: usize },
}

/// A document to index. `id` is optional — when omitted we derive a stable
/// UUIDv5 from the text so re-indexing the same content updates in place.
#[derive(Debug, Deserialize)]
pub struct Document {
    #[serde(default)]
    pub id: Option<String>,
    pub text: String,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Deserialize)]
pub struct IndexRequest {
    pub collection: String,
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub dimensions: Option<u32>,
    /// Qdrant distance metric: `Cosine` (default), `Dot`, or `Euclid`.
    #[serde(default = "default_distance")]
    pub distance: String,
    pub documents: Vec<Document>,
}

fn default_distance() -> String {
    "Cosine".to_string()
}

#[derive(Debug, Serialize)]
pub struct IndexResponse {
    pub collection: String,
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
    pub indexed: usize,
}

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub collection: String,
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub dimensions: Option<u32>,
    pub query: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}

fn default_top_k() -> usize {
    5
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub collection: String,
    pub provider: String,
    pub model: String,
    pub matches: Vec<ScoredPoint>,
}

/// Namespace for deriving stable point ids from document text or caller ids.
const DOC_NAMESPACE: Uuid = Uuid::from_u128(0x7e1d_2b40_9c3a_4f88_b6e2_0a51_77c4_d901);

/// Map a caller-supplied id (or, absent one, the document text) onto a valid
/// Qdrant point id. Returns `(point_id_value, source_id)` where `source_id` is
/// the caller's original id if any. A UUID or unsigned-int id passes through;
/// anything else is hashed into a stable UUIDv5 so the same id always lands on
/// the same point.
fn normalize_point_id(id: Option<&str>, text: &str) -> (Value, Option<String>) {
    match id {
        Some(raw) => {
            let point = if Uuid::parse_str(raw).is_ok() {
                json!(raw)
            } else if let Ok(n) = raw.parse::<u64>() {
                json!(n)
            } else {
                json!(Uuid::new_v5(&DOC_NAMESPACE, raw.as_bytes()).to_string())
            };
            (point, Some(raw.to_string()))
        }
        None => (
            json!(Uuid::new_v5(&DOC_NAMESPACE, text.as_bytes()).to_string()),
            None,
        ),
    }
}

pub struct RagService {
    registry: Arc<Registry>,
    qdrant: Arc<Qdrant>,
}

impl RagService {
    pub fn new(registry: Arc<Registry>, qdrant: Arc<Qdrant>) -> Self {
        Self { registry, qdrant }
    }

    pub async fn index(&self, req: IndexRequest) -> Result<IndexResponse, RagError> {
        if req.documents.is_empty() {
            return Err(RagError::NoDocuments);
        }
        let provider = self.registry.resolve(&req.provider)?;

        // Documents are embedded as documents, not queries.
        let embed_req = EmbedRequest {
            input: req.documents.iter().map(|d| d.text.clone()).collect(),
            model: req.model.clone(),
            dimensions: req.dimensions,
            input_type: InputType::Document,
        };
        let result = provider.embed(&embed_req).await?;

        if result.embeddings.len() != req.documents.len() {
            return Err(RagError::CountMismatch {
                got: result.embeddings.len(),
                expected: req.documents.len(),
            });
        }

        self.qdrant
            .ensure_collection(&req.collection, result.dimensions, &req.distance)
            .await?;

        let points: Vec<Value> = req
            .documents
            .iter()
            .zip(result.embeddings.iter())
            .map(|(doc, emb)| {
                // Qdrant only accepts unsigned-int or UUID point ids. Normalize
                // whatever the caller gave us into one of those (deterministically,
                // so re-indexing the same id updates in place) and keep the
                // original id in the payload as `source_id` for round-tripping.
                let (id, source_id) = normalize_point_id(doc.id.as_deref(), &doc.text);
                json!({
                    "id": id,
                    "vector": emb.vector,
                    "payload": {
                        "text": doc.text,
                        "metadata": doc.metadata,
                        "source_id": source_id,
                        "provider": result.provider,
                        "model": result.model,
                    }
                })
            })
            .collect();

        let indexed = points.len();
        self.qdrant.upsert(&req.collection, points).await?;

        Ok(IndexResponse {
            collection: req.collection,
            provider: result.provider,
            model: result.model,
            dimensions: result.dimensions,
            indexed,
        })
    }

    /// Readiness passthrough: is the vector store reachable?
    pub async fn qdrant_health(&self) -> Result<(), QdrantError> {
        self.qdrant.healthz().await
    }

    pub async fn search(&self, req: SearchRequest) -> Result<SearchResponse, RagError> {
        let provider = self.registry.resolve(&req.provider)?;

        // The query side must use query intent so asymmetric models line up.
        let embed_req = EmbedRequest {
            input: vec![req.query.clone()],
            model: req.model.clone(),
            dimensions: req.dimensions,
            input_type: InputType::Query,
        };
        let result = provider.embed(&embed_req).await?;
        let vector = result
            .embeddings
            .into_iter()
            .next()
            .map(|e| e.vector)
            .unwrap_or_default();

        let matches = self.qdrant.search(&req.collection, vector, req.top_k).await?;

        Ok(SearchResponse {
            collection: req.collection,
            provider: result.provider,
            model: result.model,
            matches,
        })
    }
}
