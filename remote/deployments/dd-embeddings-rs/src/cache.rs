//! Bounded in-memory embedding cache.
//!
//! Keyed by the exact `(provider, model, dimensions, input_type, text)` tuple
//! so a hit always returns the vector that text would have produced — no
//! hashing, hence no collision risk. To keep memory bounded we only cache
//! items whose text is below `max_item_bytes`, and evict FIFO once the entry
//! count hits `max_entries`. Re-embedding identical text (common in RAG and
//! repeated queries) then costs nothing and makes no provider call.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use crate::providers::{EmbedRequest, InputType};

pub struct EmbeddingCache {
    inner: Mutex<Inner>,
    max_entries: usize,
    max_item_bytes: usize,
    enabled: bool,
}

struct Inner {
    map: HashMap<String, Vec<f32>>,
    order: VecDeque<String>,
}

impl EmbeddingCache {
    pub fn new(max_entries: usize, max_item_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(Inner { map: HashMap::new(), order: VecDeque::new() }),
            max_entries,
            max_item_bytes,
            // A zero budget disables the cache entirely.
            enabled: max_entries > 0,
        }
    }

    fn key(provider: &str, model: &str, dims: Option<u32>, itype: InputType, text: &str) -> String {
        // \u{1} is not valid in provider/model ids or meaningful in text, so it
        // is a safe field separator.
        let it = match itype {
            InputType::Document => 'd',
            InputType::Query => 'q',
        };
        let dims = dims.map(|d| d.to_string()).unwrap_or_default();
        format!("{provider}\u{1}{model}\u{1}{dims}\u{1}{it}\u{1}{text}")
    }

    fn cacheable(&self, text: &str) -> bool {
        self.enabled && text.len() <= self.max_item_bytes
    }

    /// Look up the vector for one text, if present.
    pub fn get(&self, provider: &str, req: &EmbedRequest, model: &str, text: &str) -> Option<Vec<f32>> {
        if !self.cacheable(text) {
            return None;
        }
        let key = Self::key(provider, model, req.dimensions, req.input_type, text);
        self.inner.lock().unwrap().map.get(&key).cloned()
    }

    /// Store a freshly-computed vector.
    pub fn put(&self, provider: &str, req: &EmbedRequest, model: &str, text: &str, vector: &[f32]) {
        if !self.cacheable(text) {
            return;
        }
        let key = Self::key(provider, model, req.dimensions, req.input_type, text);
        let mut inner = self.inner.lock().unwrap();
        if inner.map.contains_key(&key) {
            return;
        }
        // FIFO eviction to stay under the entry budget.
        while inner.order.len() >= self.max_entries {
            if let Some(old) = inner.order.pop_front() {
                inner.map.remove(&old);
            } else {
                break;
            }
        }
        inner.map.insert(key.clone(), vector.to_vec());
        inner.order.push_back(key);
    }
}
