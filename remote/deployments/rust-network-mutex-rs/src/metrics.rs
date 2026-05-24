//! Prometheus metrics for the broker. Counters and gauges are intentionally
//! plain `IntCounter`/`IntGauge` from the `prometheus` crate; runtime queries
//! to the broker (key count, holder count, queue depth) are pulled on-render
//! to keep metric writes off the broker's hot path.

use prometheus::{IntCounter, Registry, TextEncoder};

use crate::broker::Broker;

#[derive(Debug)]
pub struct Metrics {
    pub registry: Registry,
    pub requests_total: IntCounter,
    pub malformed_requests_total: IntCounter,
    pub auth_failures_total: IntCounter,
    pub tcp_connections_total: IntCounter,
    pub uds_connections_total: IntCounter,
    /// Number of accepted TCP sockets where `setsockopt(TCP_NODELAY, 1)`
    /// returned ok. Lets operators confirm the experiment from
    /// upstream `live-mutex#22` is wired through at runtime.
    pub tcp_nodelay_applied_total: IntCounter,
    /// Number of times we re-applied `TCP_QUICKACK = 1` after a
    /// frame read. Increments per request on Linux when the option
    /// is enabled; remains 0 on macOS / BSD where the syscall is
    /// a no-op.
    pub tcp_quickack_applied_total: IntCounter,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let requests_total = IntCounter::new(
            "dd_rust_network_mutex_requests_total",
            "Inbound requests handled (any transport).",
        )
        .unwrap();
        let malformed_requests_total = IntCounter::new(
            "dd_rust_network_mutex_malformed_requests_total",
            "Inbound requests rejected due to malformed JSON.",
        )
        .unwrap();
        let auth_failures_total = IntCounter::new(
            "dd_rust_network_mutex_auth_failures_total",
            "Connections rejected because the auth handshake failed.",
        )
        .unwrap();
        let tcp_connections_total = IntCounter::new(
            "dd_rust_network_mutex_tcp_connections_total",
            "Accepted TCP client connections.",
        )
        .unwrap();
        let uds_connections_total = IntCounter::new(
            "dd_rust_network_mutex_uds_connections_total",
            "Accepted Unix domain socket client connections.",
        )
        .unwrap();
        let tcp_nodelay_applied_total = IntCounter::new(
            "dd_rust_network_mutex_tcp_nodelay_applied_total",
            "Accepted TCP sockets that successfully had TCP_NODELAY set.",
        )
        .unwrap();
        let tcp_quickack_applied_total = IntCounter::new(
            "dd_rust_network_mutex_tcp_quickack_applied_total",
            "Reads after which TCP_QUICKACK was successfully (re-)applied (Linux only).",
        )
        .unwrap();
        registry.register(Box::new(requests_total.clone())).ok();
        registry
            .register(Box::new(malformed_requests_total.clone()))
            .ok();
        registry
            .register(Box::new(auth_failures_total.clone()))
            .ok();
        registry
            .register(Box::new(tcp_connections_total.clone()))
            .ok();
        registry
            .register(Box::new(uds_connections_total.clone()))
            .ok();
        registry
            .register(Box::new(tcp_nodelay_applied_total.clone()))
            .ok();
        registry
            .register(Box::new(tcp_quickack_applied_total.clone()))
            .ok();
        Self {
            registry,
            requests_total,
            malformed_requests_total,
            auth_failures_total,
            tcp_connections_total,
            uds_connections_total,
            tcp_nodelay_applied_total,
            tcp_quickack_applied_total,
        }
    }

    pub fn render(&self, broker: &Broker) -> String {
        let snapshot = broker.metrics();
        let encoder = TextEncoder::new();
        let mut buffer = Vec::new();
        let metric_families = self.registry.gather();
        let _ = prometheus::Encoder::encode(&encoder, &metric_families, &mut buffer);
        let mut body = String::from_utf8_lossy(&buffer).into_owned();
        body.push_str(&format!(
            concat!(
                "# HELP dd_rust_network_mutex_keys Number of distinct lock keys tracked by the broker.\n",
                "# TYPE dd_rust_network_mutex_keys gauge\n",
                "dd_rust_network_mutex_keys {}\n",
                "# HELP dd_rust_network_mutex_holders Total active lock holders (exclusive + readers + writers).\n",
                "# TYPE dd_rust_network_mutex_holders gauge\n",
                "dd_rust_network_mutex_holders {}\n",
                "# HELP dd_rust_network_mutex_waiters Total queued lock requests across all keys.\n",
                "# TYPE dd_rust_network_mutex_waiters gauge\n",
                "dd_rust_network_mutex_waiters {}\n",
                "# HELP dd_rust_network_mutex_clients Connected broker clients.\n",
                "# TYPE dd_rust_network_mutex_clients gauge\n",
                "dd_rust_network_mutex_clients {}\n",
                "# HELP dd_rust_network_mutex_pending_deadlines Holders currently registered in the periodic TTL deadline index.\n",
                "# TYPE dd_rust_network_mutex_pending_deadlines gauge\n",
                "dd_rust_network_mutex_pending_deadlines {}\n",
                "# HELP dd_rust_network_mutex_ttl_evictions_total Cumulative TTL-driven evictions performed by the periodic sweeper (upstream live-mutex#13).\n",
                "# TYPE dd_rust_network_mutex_ttl_evictions_total counter\n",
                "dd_rust_network_mutex_ttl_evictions_total {}\n",
                "# HELP dd_rust_network_mutex_max_concurrency_cap Effective per-key concurrency ceiling enforced by the broker (upstream live-mutex semaphore-style locks).\n",
                "# TYPE dd_rust_network_mutex_max_concurrency_cap gauge\n",
                "dd_rust_network_mutex_max_concurrency_cap {}\n",
                "# HELP dd_rust_network_mutex_concurrency_cap_clamps_total Cumulative `lock` requests whose `max` was clamped to the cap.\n",
                "# TYPE dd_rust_network_mutex_concurrency_cap_clamps_total counter\n",
                "dd_rust_network_mutex_concurrency_cap_clamps_total {}\n",
            ),
            snapshot.keys,
            snapshot.holders,
            snapshot.waiters,
            snapshot.clients,
            snapshot.pending_deadlines,
            snapshot.ttl_evictions_total,
            snapshot.max_concurrency_cap,
            snapshot.concurrency_cap_clamps_total,
        ));
        body
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}
