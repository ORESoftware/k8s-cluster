use std::time::Instant;

use axum::http::StatusCode;
use once_cell::sync::Lazy;
use prometheus::{IntCounterVec, IntGauge, Opts};

pub(crate) static STARTED_AT: Lazy<Instant> = Lazy::new(Instant::now);
static HTTP_REQUESTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "dd_runtime_http_requests_total",
            "HTTP requests observed by the dd remote runtime.",
        ),
        &["service", "method", "path", "status"],
    )
    .expect("failed to create dd_runtime_http_requests_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("failed to register dd_runtime_http_requests_total");
    counter
});
pub(crate) static UPTIME_SECONDS: Lazy<IntGauge> = Lazy::new(|| {
    let gauge = IntGauge::new(
        "dd_runtime_uptime_seconds",
        "Worker process uptime in seconds.",
    )
    .expect("failed to create dd_runtime_uptime_seconds");
    prometheus::default_registry()
        .register(Box::new(gauge.clone()))
        .expect("failed to register dd_runtime_uptime_seconds");
    gauge
});

pub(crate) fn record_request(method: &str, path: &str, status: StatusCode) {
    HTTP_REQUESTS
        .with_label_values(&["dd-remote-web-home", method, path, status.as_str()])
        .inc();
}
