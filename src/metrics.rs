//! Prometheus metrics for the bounded public HTTP route set.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, Response, StatusCode};
use axum::middleware::Next;
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, Opts, Registry,
    TextEncoder,
};
use std::sync::Arc;
use std::time::Instant;

pub struct Metrics {
    registry: Registry,
    requests: IntCounterVec,
    request_duration: HistogramVec,
    requests_in_flight: IntGauge,
    database_queries: IntCounterVec,
    database_query_duration: HistogramVec,
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
        let requests_in_flight = IntGauge::new(
            "threefa_http_requests_in_flight",
            "HTTP requests currently being handled by the 3FA sync server.",
        )?;
        let database_queries = IntCounterVec::new(
            Opts::new(
                "threefa_database_queries_total",
                "SeaORM database operations completed by status.",
            ),
            &["status"],
        )?;
        let database_query_duration = HistogramVec::new(
            HistogramOpts::new(
                "threefa_database_query_duration_seconds",
                "SeaORM database operation latency in seconds.",
            )
            .buckets(vec![
                0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
            ]),
            &["status"],
        )?;
        let vault_conflicts = IntCounter::new(
            "threefa_vault_conflicts_total",
            "Vault pushes rejected because the server version was newer.",
        )?;

        registry.register(Box::new(requests.clone()))?;
        registry.register(Box::new(request_duration.clone()))?;
        registry.register(Box::new(requests_in_flight.clone()))?;
        registry.register(Box::new(database_queries.clone()))?;
        registry.register(Box::new(database_query_duration.clone()))?;
        registry.register(Box::new(vault_conflicts.clone()))?;

        Ok(Self {
            registry,
            requests,
            request_duration,
            requests_in_flight,
            database_queries,
            database_query_duration,
            vault_conflicts,
        })
    }

    pub fn render(&self) -> Result<Vec<u8>, prometheus::Error> {
        let mut output = Vec::new();
        TextEncoder::new().encode(&self.registry.gather(), &mut output)?;
        Ok(output)
    }

    pub fn observe_database_query(&self, elapsed: std::time::Duration, failed: bool) {
        let status = if failed { "error" } else { "ok" };
        self.database_queries.with_label_values(&[status]).inc();
        self.database_query_duration
            .with_label_values(&[status])
            .observe(elapsed.as_secs_f64());
    }
}

pub async fn record_http_metrics(
    State(metrics): State<Arc<Metrics>>,
    request: Request,
    next: Next,
) -> Response<Body> {
    let method = metric_method(request.method());
    let route = metric_route(request.uri().path());
    let started = Instant::now();
    metrics.requests_in_flight.inc();
    let _in_flight = InFlightGuard(metrics.requests_in_flight.clone());
    let response = next.run(request).await;
    let status = response.status().as_u16().to_string();

    metrics
        .requests
        .with_label_values(&[method, route, &status])
        .inc();
    metrics
        .request_duration
        .with_label_values(&[method, route])
        .observe(started.elapsed().as_secs_f64());
    response
}

struct InFlightGuard(IntGauge);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.dec();
    }
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

/// Collapse the request method to the bounded set this API actually serves.
///
/// The twin of [`metric_route`], and for the same reason: every distinct label
/// value is a permanent time series. HTTP allows any token as a method name, so
/// `FROBNICATE /livez` — unauthenticated, on an unauthenticated route, answered
/// 405 by the router but still counted here because this middleware sits
/// *outside* it — used to mint a new series in three metric families per
/// invented word. Anything outside the set collapses to `other`, exactly as an
/// unknown path collapses to `unmatched`.
pub(crate) fn metric_method(method: &axum::http::Method) -> &'static str {
    use axum::http::Method;
    match *method {
        Method::GET => "GET",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        Method::HEAD => "HEAD",
        Method::OPTIONS => "OPTIONS",
        Method::PATCH => "PATCH",
        _ => "other",
    }
}

pub(crate) fn metric_route(path: &str) -> &'static str {
    match path {
        "/v1/auth/shared" => "/v1/auth/shared",
        "/v1/auth/supabase" => "/v1/auth/supabase",
        "/v1/devices" => "/v1/devices",
        "/v1/devices/revoke" => "/v1/devices/revoke",
        "/v1/vault" => "/v1/vault",
        "/livez" => "/livez",
        "/healthz" => "/healthz",
        "/readyz" => "/readyz",
        // Served by the separate telemetry listener now, which is not wrapped in
        // this middleware. Kept so a stale scraper still hitting the public port
        // shows up as a labelled 404 instead of disappearing into "unmatched".
        "/metrics" => "/metrics",
        _ => "unmatched",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_paths_collapse_to_a_bounded_label() {
        assert_eq!(metric_route("/v1/vault"), "/v1/vault");
        assert_eq!(metric_route("/v1/auth/shared"), "/v1/auth/shared");
        assert_eq!(metric_route("/v1/auth/supabase"), "/v1/auth/supabase");
        assert_eq!(metric_route("/users/alice/private"), "unmatched");
        assert_eq!(metric_route("/v1/vault/account-secret"), "unmatched");
    }

    #[test]
    fn registry_exposes_http_database_and_domain_families() {
        let metrics = Metrics::new().expect("metrics registry");
        metrics
            .requests
            .with_label_values(&["GET", "/livez", "200"])
            .inc();
        metrics
            .request_duration
            .with_label_values(&["GET", "/livez"])
            .observe(0.01);
        metrics.observe_database_query(std::time::Duration::from_millis(3), false);
        metrics.vault_conflicts.inc();

        let rendered = String::from_utf8(metrics.render().unwrap()).unwrap();
        for family in [
            "threefa_http_requests_total",
            "threefa_http_request_duration_seconds",
            "threefa_http_requests_in_flight",
            "threefa_database_queries_total",
            "threefa_database_query_duration_seconds",
            "threefa_vault_conflicts_total",
        ] {
            assert!(rendered.contains(family), "missing {family}");
        }
        assert!(!rendered.contains("account-secret"));
    }
}
