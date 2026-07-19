//! Explicit observability boundaries for the Kubernetes runtime.
//!
//! `tracing` JSON is written to the container stream for Promtail/Loki,
//! spans are exported through the shared OpenTelemetry collector, and metrics
//! remain available from the Prometheus-compatible `/metrics` endpoint.

use std::net::SocketAddr;

pub(crate) fn init() -> dd_telemetry::OtelGuard {
    init_for(crate::SERVICE_NAME)
}

pub(crate) fn init_for(service_name: &str) -> dd_telemetry::OtelGuard {
    dd_telemetry::init(service_name)
}

pub(crate) fn web_server_listening(
    http_address: SocketAddr,
    tcp_address: SocketAddr,
    persistence_enabled: bool,
    nats_enabled: bool,
    supabase_enabled: bool,
) {
    tracing::info!(
        service.name = "dd-fabrication-web-server",
        server.address = %http_address,
        network.peer.address = %tcp_address,
        db.client = "seaorm",
        persistence.enabled = persistence_enabled,
        messaging.system = "nats",
        messaging.enabled = nats_enabled,
        supabase.realtime.enabled = supabase_enabled,
        telemetry.logs = "stdout/loki",
        telemetry.traces = "otlp",
        telemetry.metrics = "prometheus",
        "fabrication web server listening"
    );
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
