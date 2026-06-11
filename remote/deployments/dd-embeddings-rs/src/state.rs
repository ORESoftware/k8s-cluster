//! Shared application state handed to every handler.

use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::config::Limits;
use crate::providers::Registry;
use crate::rag::RagService;

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Registry>,
    pub rag: Arc<RagService>,
    /// Optional API bearer token. `None` means the API is unauthenticated at
    /// this layer (an upstream gateway may still gate it).
    pub api_auth_bearer: Option<Arc<String>>,
    pub limits: Limits,
    /// Bounds concurrent cost-bearing requests; saturation sheds load with 503.
    pub inflight: Arc<Semaphore>,
}
