//! Explicit observability boundaries for the Kubernetes runtime.
//!
//! `tracing` JSON is written to the container stream for Promtail/Loki,
//! spans are exported through the shared OpenTelemetry collector, and metrics
//! remain available from the Prometheus-compatible `/metrics` endpoint.

use std::net::SocketAddr;

pub(crate) fn init() -> dd_telemetry::OtelGuard {
    dd_telemetry::init(crate::SERVICE_NAME)
}

pub(crate) fn server_listening(address: SocketAddr, persistence_enabled: bool, nats_enabled: bool) {
    tracing::info!(
        service.name = crate::SERVICE_NAME,
        server.address = %address,
        db.client = "seaorm",
        persistence.enabled = persistence_enabled,
        messaging.system = "nats",
        messaging.enabled = nats_enabled,
        telemetry.logs = "stdout/loki",
        telemetry.traces = "otlp",
        telemetry.metrics = "prometheus",
        "fabrication server listening"
    );
}
