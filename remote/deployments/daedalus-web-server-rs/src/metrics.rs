//! Prometheus metrics, exposed in the text format at `/metrics`.

use prometheus::{IntCounter, Registry, TextEncoder};

pub(crate) struct Metrics {
    registry: Registry,
    authorized: IntCounter,
    rejected: IntCounter,
}

impl Metrics {
    pub(crate) fn new() -> Self {
        let registry = Registry::new();
        let authorized = IntCounter::new(
            "daedalus_web_authorized_requests_total",
            "Requests that presented a verified, allow-listed Supabase identity.",
        )
        .expect("static metric definition");
        let rejected = IntCounter::new(
            "daedalus_web_rejected_requests_total",
            "Requests rejected at the authorization boundary.",
        )
        .expect("static metric definition");
        registry
            .register(Box::new(authorized.clone()))
            .expect("static metric registration");
        registry
            .register(Box::new(rejected.clone()))
            .expect("static metric registration");
        Self {
            registry,
            authorized,
            rejected,
        }
    }

    pub(crate) fn record_authorized(&self) {
        self.authorized.inc();
    }

    pub(crate) fn record_rejected(&self) {
        self.rejected.inc();
    }

    pub(crate) fn encode(&self) -> String {
        TextEncoder::new()
            .encode_to_string(&self.registry.gather())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_are_registered_and_encode() {
        let metrics = Metrics::new();
        metrics.record_authorized();
        metrics.record_rejected();
        let encoded = metrics.encode();
        assert!(encoded.contains("daedalus_web_authorized_requests_total 1"));
        assert!(encoded.contains("daedalus_web_rejected_requests_total 1"));
    }
}
