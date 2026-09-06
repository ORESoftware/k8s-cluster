//! Prometheus metrics with bounded labels.

use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, Response, StatusCode};
use axum::middleware::Next;
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry, TextEncoder,
};
use std::sync::Arc;
use std::time::Instant;

pub(crate) struct Metrics {
    registry: Registry,
    requests: IntCounterVec,
    request_duration: HistogramVec,
    requests_in_flight: IntGauge,
}

impl Metrics {
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();
        let requests = IntCounterVec::new(
            Opts::new(
                "threefa_web_http_requests_total",
                "HTTP requests handled by the 3FA web server.",
            ),
            &["method", "route", "status"],
        )?;
        let request_duration = HistogramVec::new(
            HistogramOpts::new(
                "threefa_web_http_request_duration_seconds",
                "3FA web server HTTP request latency in seconds.",
            )
            .buckets(vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 15.0,
            ]),
            &["method", "route"],
        )?;
        let requests_in_flight = IntGauge::new(
            "threefa_web_http_requests_in_flight",
            "HTTP requests currently handled by the 3FA web server.",
        )?;
        registry.register(Box::new(requests.clone()))?;
        registry.register(Box::new(request_duration.clone()))?;
        registry.register(Box::new(requests_in_flight.clone()))?;
        Ok(Self {
            registry,
            requests,
            request_duration,
            requests_in_flight,
        })
    }

    fn render(&self) -> Result<Vec<u8>, prometheus::Error> {
        let mut output = Vec::new();
        TextEncoder::new().encode(&self.registry.gather(), &mut output)?;
        Ok(output)
    }
}

pub(crate) async fn record_http(
    State(metrics): State<Arc<Metrics>>,
    request: Request,
    next: Next,
) -> Response<Body> {
    let method = request.method().as_str().to_owned();
    let route = metric_route(request.uri().path());
    let started = Instant::now();
    metrics.requests_in_flight.inc();
    let _in_flight = InFlightGuard(metrics.requests_in_flight.clone());
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

pub(crate) async fn prometheus(State(state): State<AppState>) -> Response<Body> {
    match state.metrics.render() {
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

struct InFlightGuard(IntGauge);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.dec();
    }
}

fn metric_route(path: &str) -> &'static str {
    match path {
        "/" => "/",
        "/login" => "/login",
        "/enroll" => "/enroll",
        "/enroll/verify" => "/enroll/verify",
        "/livez" => "/livez",
        "/healthz" => "/healthz",
        "/readyz" => "/readyz",
        "/metrics" => "/metrics",
        _ => "unmatched",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_or_secret_bearing_paths_use_one_bounded_label() {
        assert_eq!(metric_route("/enroll"), "/enroll");
        assert_eq!(metric_route("/users/alice"), "unmatched");
        assert_eq!(metric_route("/enroll/secret-value"), "unmatched");
    }

    #[test]
    fn registry_renders_all_http_metric_families() {
        let metrics = Metrics::new().expect("metrics registry");
        metrics
            .requests
            .with_label_values(&["GET", "/livez", "200"])
            .inc();
        metrics
            .request_duration
            .with_label_values(&["GET", "/livez"])
            .observe(0.01);
        let rendered = String::from_utf8(metrics.render().unwrap()).unwrap();
        for family in [
            "threefa_web_http_requests_total",
            "threefa_web_http_request_duration_seconds",
            "threefa_web_http_requests_in_flight",
        ] {
            assert!(rendered.contains(family), "missing {family}");
        }
        assert!(!rendered.contains("secret-value"));
    }
}
