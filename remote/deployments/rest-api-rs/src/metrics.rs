use std::time::Instant;

use axum::{
    http::{header, StatusCode},
    response::IntoResponse,
};
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounterVec, IntGauge, Opts, TextEncoder};

use crate::lambdas::image_builder_dependencies_ready;
use crate::shared::image_builder_role;

pub(crate) static STARTED_AT: Lazy<Instant> = Lazy::new(Instant::now);
pub(crate) static HTTP_REQUESTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "dd_remote_rest_api_http_requests_total",
            "HTTP requests observed by the dd remote REST API.",
        ),
        &["method", "path", "status"],
    )
    .expect("failed to create dd_remote_rest_api_http_requests_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("failed to register dd_remote_rest_api_http_requests_total");
    counter
});
pub(crate) static UPTIME_SECONDS: Lazy<IntGauge> = Lazy::new(|| {
    let gauge = IntGauge::new(
        "dd_remote_rest_api_uptime_seconds",
        "REST API process uptime in seconds.",
    )
    .expect("failed to create dd_remote_rest_api_uptime_seconds");
    prometheus::default_registry()
        .register(Box::new(gauge.clone()))
        .expect("failed to register dd_remote_rest_api_uptime_seconds");
    gauge
});

pub(crate) fn record_request(method: &str, path: &str, status: StatusCode) {
    HTTP_REQUESTS
        .with_label_values(&[method, path, status.as_str()])
        .inc();
}

pub(crate) async fn metrics() -> impl IntoResponse {
    record_request("GET", "/metrics", StatusCode::OK);
    UPTIME_SECONDS.set(STARTED_AT.elapsed().as_secs() as i64);

    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder
        .encode(&metric_families, &mut buffer)
        .expect("failed to encode prometheus metrics");

    if image_builder_role() {
        let ready = image_builder_dependencies_ready();
        buffer.extend_from_slice(
            b"# HELP dd_image_builder_build_info Image builder process metadata.\n\
# TYPE dd_image_builder_build_info gauge\n\
dd_image_builder_build_info{service=\"dd-image-builder\"} 1\n",
        );
        buffer.extend_from_slice(
            format!(
                "# HELP dd_image_builder_dependencies_ready Whether auth, Postgres, containerd, nerdctl, buildctl, and buildkit are available.\n\
                 # TYPE dd_image_builder_dependencies_ready gauge\n\
                 dd_image_builder_dependencies_ready {}\n",
                u8::from(ready)
            )
            .as_bytes(),
        );
    }

    (
        [(header::CONTENT_TYPE, encoder.format_type().to_string())],
        buffer,
    )
}
