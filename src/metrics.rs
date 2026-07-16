//! Prometheus metrics for the bounded public HTTP route set.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, Response, StatusCode};
use axum::middleware::Next;
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, Opts, Registry, TextEncoder,
};
use std::sync::Arc;
use std::time::Instant;

pub struct Metrics {
    registry: Registry,
    requests: IntCounterVec,
    request_duration: HistogramVec,
    pub vault_conflicts: IntCounter,
}

impl Metrics {
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();
        let requests = IntCounterVec::new(
            Opts::new(
                "threefa_http_requests_total",
                "HTTP requests handled by the 3FA sync server.",
            ),
            &["method", "route", "status"],
        )?;
        let request_duration = HistogramVec::new(
            HistogramOpts::new(
                "threefa_http_request_duration_seconds",
                "3FA HTTP request latency in seconds.",
            )
            .buckets(vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 15.0,
            ]),
            &["method", "route"],
        )?;
        let vault_conflicts = IntCounter::new(
            "threefa_vault_conflicts_total",
            "Vault pushes rejected because the server version was newer.",
        )?;

        registry.register(Box::new(requests.clone()))?;
        registry.register(Box::new(request_duration.clone()))?;
        registry.register(Box::new(vault_conflicts.clone()))?;

        Ok(Self {
            registry,
            requests,
            request_duration,
            vault_conflicts,
        })
    }

    pub fn render(&self) -> Result<Vec<u8>, prometheus::Error> {
        let mut output = Vec::new();
        TextEncoder::new().encode(&self.registry.gather(), &mut output)?;
        Ok(output)
    }
}

pub async fn record_http_metrics(
    State(metrics): State<Arc<Metrics>>,
    request: Request,
    next: Next,
) -> Response<Body> {
    let method = request.method().as_str().to_owned();
    let route = metric_route(request.uri().path());
    let started = Instant::now();
    let response = next.run(request).await;
    let status = response.status().as_u16().to_string();

    metrics
        .requests
        .with_label_values(&[&method, route, &status])
        .inc();
    metrics
        .request_duration
        .with_label_values(&[&method, route])
        .observe(started.elapsed().as_secs_f64());
    response
}

pub fn response(metrics: &Metrics) -> Response<Body> {
    match metrics.render() {
        Ok(body) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, TextEncoder::new().format_type())
            .body(Body::from(body))
            .expect("valid metrics response"),
        Err(error) => {
            tracing::error!(error = %error, "failed to encode Prometheus metrics");
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("metrics unavailable"))
                .expect("valid metrics error response")
        }
    }
}

fn metric_route(path: &str) -> &'static str {
    match path {
        "/v1/register" => "/v1/register",
        "/v1/login" => "/v1/login",
        "/v1/devices/revoke" => "/v1/devices/revoke",
        "/v1/vault" => "/v1/vault",
        "/livez" => "/livez",
        "/healthz" => "/healthz",
        "/readyz" => "/readyz",
        "/metrics" => "/metrics",
        _ => "unmatched",
    }
}
