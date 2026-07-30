//! Prometheus metrics. A dedicated registry (not the global default) so the
//! counters are explicit and testable. Scraped by the cluster Prometheus at
//! `dd-shared-auth.shared-auth.svc:8120/metrics` → Grafana.

use std::sync::Arc;

use prometheus::{Encoder, IntCounter, IntCounterVec, Opts, Registry, TextEncoder};

/// Owned set of metrics + their registry, shared via [`AppState`](crate::state::AppState).
#[derive(Clone)]
pub struct Metrics {
    registry: Arc<Registry>,
    /// Token exchanges by project and outcome (`ok` / `error`).
    pub exchanges: IntCounterVec,
    /// Supabase verification failures (bad signature, unknown issuer, expired…).
    pub verify_failures: IntCounter,
    /// Introspections of our own minted tokens by outcome.
    pub introspections: IntCounterVec,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();

        let exchanges = IntCounterVec::new(
            Opts::new(
                "shared_auth_exchanges_total",
                "Supabase→OreSoftware token exchanges.",
            ),
            &["project", "outcome"],
        )
        .expect("valid metric");
        let verify_failures = IntCounter::with_opts(Opts::new(
            "shared_auth_verify_failures_total",
            "Supabase token verification failures.",
        ))
        .expect("valid metric");
        let introspections = IntCounterVec::new(
            Opts::new(
                "shared_auth_introspections_total",
                "Introspections of OreSoftware-minted tokens.",
            ),
            &["outcome"],
        )
        .expect("valid metric");

        registry
            .register(Box::new(exchanges.clone()))
            .expect("register exchanges");
        registry
            .register(Box::new(verify_failures.clone()))
            .expect("register verify_failures");
        registry
            .register(Box::new(introspections.clone()))
            .expect("register introspections");

        Self {
            registry: Arc::new(registry),
            exchanges,
            verify_failures,
            introspections,
        }
    }

    /// Render the exposition text for `/metrics`.
    pub fn render(&self) -> (String, Vec<u8>) {
        let encoder = TextEncoder::new();
        let mut buffer = Vec::new();
        let content_type = encoder.format_type().to_string();
        if let Err(err) = encoder.encode(&self.registry.gather(), &mut buffer) {
            tracing::error!(error = %err, "metrics encode failed");
        }
        (content_type, buffer)
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposition_and_router_contract_are_stable_and_content_free() {
        let metrics = Metrics::new();
        metrics
            .exchanges
            .with_label_values(&["sonus-auris", "ok"])
            .inc();
        metrics.verify_failures.inc();
        metrics
            .introspections
            .with_label_values(&["rejected"])
            .inc();

        let (content_type, bytes) = metrics.render();
        assert!(content_type.starts_with("text/plain"));
        let body = String::from_utf8(bytes).expect("Prometheus exposition is UTF-8");
        for name in [
            "shared_auth_exchanges_total",
            "shared_auth_verify_failures_total",
            "shared_auth_introspections_total",
        ] {
            assert!(body.contains(name), "missing metric {name}");
        }
        for forbidden_label in [
            "email=",
            "identity=",
            "subject=",
            "sub=",
            "jwt=",
            "token=",
            "token_prefix=",
            "url=",
        ] {
            assert!(
                !body.contains(forbidden_label),
                "sensitive/unbounded label leaked: {forbidden_label}"
            );
        }

        let router_source = include_str!("http/mod.rs");
        assert!(router_source.contains(".route(\"/metrics\", get(metrics::metrics))"));
    }
}
