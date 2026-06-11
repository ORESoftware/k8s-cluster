//! Shared application state handed to every handler.

use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::config::Limits;
use crate::embedder::Embedder;
use crate::metrics::Metrics;
use crate::providers::rerank::RerankRegistry;
use crate::providers::Registry;
use crate::rag::RagService;
use crate::search::SearchService;

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Registry>,
    /// Cache-aware embedding façade (used by the embed endpoint and RAG).
    pub embedder: Arc<Embedder>,
    pub rerank: Arc<RerankRegistry>,
    pub rag: Arc<RagService>,
    /// Postgres multi-signal search. `None` when no DATABASE_URL is configured.
    pub search: Option<Arc<SearchService>>,
    pub metrics: Arc<Metrics>,
    /// Optional API bearer token. `None` means the API is unauthenticated at
    /// this layer (an upstream gateway may still gate it).
    pub api_auth_bearer: Option<Arc<String>>,
    pub limits: Limits,
    /// Bounds concurrent cost-bearing requests; saturation sheds load with 503.
    pub inflight: Arc<Semaphore>,
}
