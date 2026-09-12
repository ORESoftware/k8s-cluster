//! Minimal, dependency-free Prometheus metrics.
//!
//! Explicit counters incremented by the handlers (no runtime-wide
//! instrumentation / monkey-patching, per the repo observability contract),
//! rendered as Prometheus text exposition at `/metrics`. The cluster's
//! prometheus scraper is allow-listed to this pod in the NetworkPolicy.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Default)]
pub struct Metrics {
    pub requests_total: AtomicU64,
    pub errors_total: AtomicU64,
    pub cache_hits_total: AtomicU64,
    pub cache_misses_total: AtomicU64,
    /// Per-route request counts, keyed by a static route label.
    by_route: Mutex<BTreeMap<&'static str, u64>>,
    /// Per-provider embedding-request counts.
    by_provider: Mutex<BTreeMap<String, u64>>,
}

impl Metrics {
    pub fn record_request(&self, route: &'static str) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        *self.by_route.lock().unwrap().entry(route).or_insert(0) += 1;
    }

    pub fn record_error(&self) {
        self.errors_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_provider(&self, provider: &str) {
        *self
            .by_provider
            .lock()
            .unwrap()
            .entry(provider.to_string())
            .or_insert(0) += 1;
    }

    pub fn record_cache(&self, hits: u64, misses: u64) {
        self.cache_hits_total.fetch_add(hits, Ordering::Relaxed);
        self.cache_misses_total.fetch_add(misses, Ordering::Relaxed);
    }

    /// Render Prometheus text exposition format (v0.0.4).
    pub fn render(&self) -> String {
        let mut out = String::with_capacity(1024);

        let counter = |out: &mut String, name: &str, help: &str, val: u64| {
            out.push_str(&format!("# HELP {name} {help}\n# TYPE {name} counter\n{name} {val}\n"));
        };

        counter(&mut out, "embeddings_requests_total", "Total API requests handled.", self.requests_total.load(Ordering::Relaxed));
        counter(&mut out, "embeddings_errors_total", "Total requests that returned an error.", self.errors_total.load(Ordering::Relaxed));
        counter(&mut out, "embeddings_cache_hits_total", "Embedding cache hits.", self.cache_hits_total.load(Ordering::Relaxed));
        counter(&mut out, "embeddings_cache_misses_total", "Embedding cache misses.", self.cache_misses_total.load(Ordering::Relaxed));

        out.push_str("# HELP embeddings_route_requests_total Requests by route.\n# TYPE embeddings_route_requests_total counter\n");
        for (route, n) in self.by_route.lock().unwrap().iter() {
            out.push_str(&format!("embeddings_route_requests_total{{route=\"{route}\"}} {n}\n"));
        }

        out.push_str("# HELP embeddings_provider_requests_total Embedding requests by provider.\n# TYPE embeddings_provider_requests_total counter\n");
        for (provider, n) in self.by_provider.lock().unwrap().iter() {
            out.push_str(&format!(
                "embeddings_provider_requests_total{{provider=\"{}\"}} {n}\n",
                sanitize_label(provider)
            ));
        }

        out
    }
}

/// Keep provider ids label-safe (they're already `[a-z0-9-]`, but be defensive).
fn sanitize_label(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}
