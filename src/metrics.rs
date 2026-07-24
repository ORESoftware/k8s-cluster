//! Prometheus counters on a dedicated registry (scraped at /metrics).

use std::sync::Arc;

use prometheus::{Encoder, IntCounterVec, Opts, Registry, TextEncoder};

#[derive(Clone)]
pub struct Metrics {
    registry: Arc<Registry>,
    /// Publishes through POST /publish, by outcome (ok|rejected|failed).
    pub publishes: IntCounterVec,
    /// NATS→webhook deliveries, by route subject and outcome (ok|failed).
    pub deliveries: IntCounterVec,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let publishes = IntCounterVec::new(
            Opts::new("shared_auth_bridge_publishes_total", "HTTP→NATS publishes."),
            &["outcome"],
        )
        .expect("valid metric");
        let deliveries = IntCounterVec::new(
            Opts::new(
                "shared_auth_bridge_deliveries_total",
                "NATS→webhook deliveries.",
            ),
            &["subject", "outcome"],
        )
        .expect("valid metric");
        registry
            .register(Box::new(publishes.clone()))
            .expect("register");
        registry
            .register(Box::new(deliveries.clone()))
            .expect("register");
        Self {
            registry: Arc::new(registry),
            publishes,
            deliveries,
        }
    }

    pub fn render(&self) -> (String, Vec<u8>) {
        let encoder = TextEncoder::new();
        let mut buffer = Vec::new();
        let content_type = encoder.format_type().to_string();
        if let Err(error) = encoder.encode(&self.registry.gather(), &mut buffer) {
            tracing::error!(%error, "metrics encode failed");
        }
        (content_type, buffer)
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}
