//! Cache-aware embedding façade shared by the `/api/embeddings` handler and
//! the RAG service. Resolves the provider, serves whatever it can from the
//! in-memory cache, and only sends the cache-miss texts to the upstream — so
//! repeated/identical text costs nothing and makes no provider call.

use std::sync::Arc;

use crate::cache::EmbeddingCache;
use crate::metrics::Metrics;
use crate::providers::{EmbedRequest, EmbedResponse, Embedding, ProviderError, Registry, Usage};

pub struct Embedder {
    registry: Arc<Registry>,
    cache: Arc<EmbeddingCache>,
    metrics: Arc<Metrics>,
}

impl Embedder {
    pub fn new(registry: Arc<Registry>, cache: Arc<EmbeddingCache>, metrics: Arc<Metrics>) -> Self {
        Self { registry, cache, metrics }
    }

    pub async fn embed(
        &self,
        provider_id: &str,
        req: &EmbedRequest,
    ) -> Result<EmbedResponse, ProviderError> {
        let provider = self.registry.resolve(provider_id)?;
        let model = req.model.clone().unwrap_or_else(|| provider.default_model().to_string());

        // Partition inputs into cache hits and misses, preserving order.
        let mut vectors: Vec<Option<Vec<f32>>> = Vec::with_capacity(req.input.len());
        let mut miss_texts: Vec<String> = Vec::new();
        let mut miss_positions: Vec<usize> = Vec::new();
        let mut hits = 0u64;

        for (i, text) in req.input.iter().enumerate() {
            match self.cache.get(provider.id(), req, &model, text) {
                Some(v) => {
                    hits += 1;
                    vectors.push(Some(v));
                }
                None => {
                    vectors.push(None);
                    miss_positions.push(i);
                    miss_texts.push(text.clone());
                }
            }
        }
        let misses = miss_texts.len() as u64;
        self.metrics.record_cache(hits, misses);

        let mut usage = Usage::default();
        if !miss_texts.is_empty() {
            // Only the misses go upstream.
            let sub = EmbedRequest {
                input: miss_texts.clone(),
                model: Some(model.clone()),
                dimensions: req.dimensions,
                input_type: req.input_type,
            };
            let resp = provider.embed(&sub).await?;
            self.metrics.record_provider(provider.id());
            usage = resp.usage;

            // resp.embeddings are index-ordered over the sub-batch; map back.
            let mut sub_vectors: Vec<Vec<f32>> = vec![Vec::new(); miss_texts.len()];
            for e in resp.embeddings {
                if let Some(slot) = sub_vectors.get_mut(e.index) {
                    *slot = e.vector;
                }
            }
            for (sub_idx, &orig_pos) in miss_positions.iter().enumerate() {
                let v = std::mem::take(&mut sub_vectors[sub_idx]);
                self.cache.put(provider.id(), req, &model, &miss_texts[sub_idx], &v);
                vectors[orig_pos] = Some(v);
            }
        }

        let embeddings: Vec<Embedding> = vectors
            .into_iter()
            .enumerate()
            .map(|(index, v)| Embedding { index, vector: v.unwrap_or_default() })
            .collect();
        let dimensions = embeddings.first().map(|e| e.vector.len()).unwrap_or(0);

        Ok(EmbedResponse {
            provider: provider.id().to_string(),
            model,
            dimensions,
            embeddings,
            usage,
        })
    }
}
