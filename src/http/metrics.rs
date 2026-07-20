//! Prometheus exposition. Uses the default registry; the process registers its
//! counters at first use. Kept intentionally small — the cluster scrapes this at
//! `dd-shared-auth.shared-auth.svc:8120/metrics`.

use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use prometheus::{Encoder, TextEncoder};

pub async fn metrics() -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let mut buffer = Vec::new();
    if encoder.encode(&prometheus::gather(), &mut buffer).is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "encode error").into_response();
    }
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, encoder.format_type())],
        buffer,
    )
        .into_response()
}
