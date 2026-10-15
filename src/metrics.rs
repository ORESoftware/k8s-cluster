//! Prometheus metrics for the broker. Counters and gauges are intentionally
//! plain `IntCounter`/`IntGauge` from the `prometheus` crate; runtime queries
//! to the broker (key count, holder count, queue depth) are pulled on-render
//! to keep metric writes off the broker's hot path.

use std::time::Duration;

use prometheus::{HistogramOpts, HistogramVec, IntCounter, IntGauge, Registry, TextEncoder};

use crate::broker::Broker;

const REQUEST_DURATION_ROUTES: &[&str] = &[
    "stream_frame",
    "http_acquire",
    "http_release",
    "http_rw_read",
    "http_rw_read_end",
    "http_rw_write",
    "http_rw_write_end",
    "http_lock_info",
    "http_ls",
    "raft_http_acquire",
    "raft_http_release",
];

const REQUEST_PAYLOAD_ROUTES: &[&str] = &["stream_frame"];

#[derive(Debug)]
pub struct Metrics {
    pub registry: Registry,
    pub requests_total: IntCounter,
    pub malformed_requests_total: IntCounter,
    pub auth_failures_total: IntCounter,
    pub tcp_connections_total: IntCounter,
    pub uds_connections_total: IntCounter,
    pub stream_connections_active: IntGauge,
    pub stream_connection_cap_drops_total: IntCounter,
    /// Number of accepted TCP sockets where `setsockopt(TCP_NODELAY, 1)`
    /// returned ok. Lets operators confirm the experiment from
    /// upstream `live-mutex#22` is wired through at runtime.
    pub tcp_nodelay_applied_total: IntCounter,
    /// Number of times we re-applied `TCP_QUICKACK = 1` after a
    /// frame read. Increments per request on Linux when the option
    /// is enabled; remains 0 on macOS / BSD where the syscall is
    /// a no-op.
    pub tcp_quickack_applied_total: IntCounter,
    /// End-to-end request latency for HTTP handlers and synchronous
    /// per-frame handling time for persistent TCP/UDS clients. Labels are
    /// deliberately fixed route names, never user-supplied keys or UUIDs.
    pub request_duration_seconds: HistogramVec,
    /// Newline-delimited JSON frame payload sizes seen on persistent
    /// TCP/UDS connections, labelled by the fixed transport route.
    pub request_payload_bytes: HistogramVec,
}

impl Metrics {
    pub fn new() -> Self {
        crate::routine_id!("ddl-routine-rKS5wwT_syifRxDnTm");
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
        let stream_connections_active = IntGauge::new(
            "dd_rust_network_mutex_stream_connections_active",
            "Currently active TCP/UDS stream client connections admitted below LMX_MAX_CONNECTIONS.",
        )
        .unwrap();
        let stream_connection_cap_drops_total = IntCounter::new(
            "dd_rust_network_mutex_stream_connection_cap_drops_total",
            "TCP/UDS stream client connections dropped because LMX_MAX_CONNECTIONS was reached.",
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
        let request_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "dd_rust_network_mutex_request_duration_seconds",
                "Request handling latency by fixed route name.",
            )
            .buckets(vec![
                0.000_1, 0.000_25, 0.000_5, 0.001, 0.002_5, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25,
                0.5, 1.0, 2.5, 5.0, 10.0,
            ]),
            &["route"],
        )
        .unwrap();
        let request_payload_bytes = HistogramVec::new(
            HistogramOpts::new(
                "dd_rust_network_mutex_request_payload_bytes",
                "Persistent TCP/UDS JSON frame payload size by fixed route name.",
            )
            .buckets(vec![
                64.0,
                128.0,
                256.0,
                512.0,
                1024.0,
                2048.0,
                4096.0,
                8192.0,
                16_384.0,
                65_536.0,
                262_144.0,
                1_048_576.0,
            ]),
            &["route"],
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
            .register(Box::new(stream_connections_active.clone()))
            .ok();
        registry
            .register(Box::new(stream_connection_cap_drops_total.clone()))
            .ok();
        registry
            .register(Box::new(tcp_nodelay_applied_total.clone()))
            .ok();
        registry
            .register(Box::new(tcp_quickack_applied_total.clone()))
            .ok();
        registry
            .register(Box::new(request_duration_seconds.clone()))
            .ok();
        registry
            .register(Box::new(request_payload_bytes.clone()))
            .ok();
        for route in REQUEST_DURATION_ROUTES {
            request_duration_seconds.with_label_values(&[route]);
        }
        for route in REQUEST_PAYLOAD_ROUTES {
            request_payload_bytes.with_label_values(&[route]);
        }
        Self {
            registry,
            requests_total,
            malformed_requests_total,
            auth_failures_total,
            tcp_connections_total,
            uds_connections_total,
            stream_connections_active,
            stream_connection_cap_drops_total,
            tcp_nodelay_applied_total,
            tcp_quickack_applied_total,
            request_duration_seconds,
            request_payload_bytes,
        }
    }

    pub fn observe_request_duration(&self, route: &'static str, elapsed: Duration) {
        crate::routine_id!("ddl-routine-metrics-observe-request-duration-1");
        self.request_duration_seconds
            .with_label_values(&[route])
            .observe(elapsed.as_secs_f64());
    }

    pub fn observe_request_payload_bytes(&self, route: &'static str, bytes: usize) {
        crate::routine_id!("ddl-routine-metrics-observe-request-payload-bytes-1");
        self.request_payload_bytes
            .with_label_values(&[route])
            .observe(bytes as f64);
    }

    pub fn render(&self, broker: &Broker) -> String {
        crate::routine_id!("ddl-routine-0Wb5ER7VYrf2fPU4LU");
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
                "# HELP dd_rust_network_mutex_fencing_watermark Strictly monotonic upper bound on every fencing token issued since broker start; seeds freshly-materialised LockState entries to preserve cross-prune monotonicity.\n",
                "# TYPE dd_rust_network_mutex_fencing_watermark counter\n",
                "dd_rust_network_mutex_fencing_watermark {}\n",
                "# HELP dd_rust_network_mutex_idle_keys_pruned_total Cumulative idle LockState entries reclaimed by the periodic empty-key prune sweep.\n",
                "# TYPE dd_rust_network_mutex_idle_keys_pruned_total counter\n",
                "dd_rust_network_mutex_idle_keys_pruned_total {}\n",
            ),
            snapshot.keys,
            snapshot.holders,
            snapshot.waiters,
            snapshot.clients,
            snapshot.pending_deadlines,
            snapshot.ttl_evictions_total,
            snapshot.max_concurrency_cap,
            snapshot.concurrency_cap_clamps_total,
            snapshot.fencing_watermark,
            snapshot.idle_keys_pruned_total,
        ));
        body
    }
}

impl Default for Metrics {
    fn default() -> Self {
        crate::routine_id!("ddl-routine-0kOF9HII9dTwi5WcsE");
        Self::new()
    }
}
