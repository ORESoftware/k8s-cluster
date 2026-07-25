//! Explicit JSON logging and OTLP tracing. No runtime monkey-patching.

use axum::body::Body;
use axum::extract::Request as ExtractRequest;
use axum::http::{Request, Response};
use axum::middleware::Next;
use opentelemetry::global;
use opentelemetry::trace::{TraceContextExt, TracerProvider as _};
use opentelemetry::KeyValue;
use opentelemetry_http::HeaderExtractor;
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::{propagation::TraceContextPropagator, trace::Config, Resource};
use opentelemetry_semantic_conventions::resource as semconv;
use std::time::{Duration, Instant};
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::trace::{DefaultOnRequest, MakeSpan, OnResponse, TraceLayer};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

const DEFAULT_OTLP_ENDPOINT: &str = "http://dd-otel-collector.observability.svc.cluster.local:4318";

pub struct Guard {
    provider: Option<opentelemetry_sdk::trace::TracerProvider>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            for result in provider.force_flush() {
                if let Err(error) = result {
                    eprintln!("threefa-telemetry: span flush failed: {error:?}");
                }
            }
            let _ = provider.shutdown();
            global::shutdown_tracer_provider();
        }
    }
}

pub fn init(service_name: &str) -> Guard {
    // `sqlx::query` is where SeaORM's per-statement logging actually comes from
    // (sqlx-core/src/logger.rs), which is why `sea_orm=warn` never silenced it:
    // at INFO every statement the service issues was logged in full, drowning
    // the level and publishing the schema. Keep both directives — `sea_orm=warn`
    // still covers SeaORM's own chatter. Mirrored in deploy/k8s/deployment.yaml,
    // which is what production actually runs with.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("info,tower_http=info,hyper=warn,sea_orm=warn,sqlx::query=warn")
    });
    let fmt_layer = tracing_subscriber::fmt::layer()
        .json()
        .flatten_event(true)
        .with_ansi(false)
        .with_current_span(true)
        // Loki records retain the parent HTTP span's trace_id/span_id even when
        // an event is emitted from a nested shared-auth or database span.
        .with_span_list(true)
        .with_target(true);

    match build_provider(service_name) {
        Ok(provider) => {
            let tracer = provider.tracer("threefa-backend");
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            match tracing_subscriber::registry()
                .with(filter)
                .with(fmt_layer)
                .with(otel_layer)
                .try_init()
            {
                Ok(()) => {
                    global::set_text_map_propagator(TraceContextPropagator::new());
                    global::set_tracer_provider(provider.clone());
                    tracing::info!(
                        service.name = service_name,
                        service.namespace = "3fa-app",
                        otel.trace_exporter = true,
                        log.sink = "stdout/loki",
                        metrics.sink = "prometheus",
                        "telemetry initialized"
                    );
                    Guard {
                        provider: Some(provider),
                    }
                }
                Err(error) => {
                    eprintln!("threefa-telemetry: subscriber already installed: {error}");
                    let _ = provider.shutdown();
                    Guard { provider: None }
                }
            }
        }
        Err(error) => {
            if tracing_subscriber::registry()
                .with(filter)
                .with(fmt_layer)
                .try_init()
                .is_ok()
            {
                tracing::warn!(error = %error, "OTLP unavailable; continuing with JSON logs");
            }
            Guard { provider: None }
        }
    }
}

pub fn http_trace_layer() -> TraceLayer<
    SharedClassifier<ServerErrorsAsFailures>,
    OtelMakeSpan,
    DefaultOnRequest,
    OtelOnResponse,
> {
    TraceLayer::new_for_http()
        .make_span_with(OtelMakeSpan)
        .on_response(OtelOnResponse)
}

/// Emit exactly one structured INFO event per completed response.
///
/// Without this the log stream held 5xx and nothing else. `OtelOnResponse`
/// *records* the status onto the span (for OTLP) but emits no event;
/// tower_http's `DefaultOnRequest` logs at DEBUG, which the shipped
/// `RUST_LOG=info,…` drops; so the only surviving emitter was the default
/// `on_failure`, and `ServerErrorsAsFailures` classifies only 5xx as a failure.
/// A 401 storm (credential rotation gone wrong), a 404 storm (a client on the
/// wrong base path) and a 429 storm (a device stuck retrying) — the three
/// incidents most likely to be reported as "sync stopped working" — were
/// therefore invisible in Loki, and `init()` above documents OTLP being
/// unavailable as a supported degraded mode, in which nothing recorded them at
/// all.
///
/// Layered *inside* [`http_trace_layer`] so the event inherits the request span
/// and its trace/span ids, joining logs to traces.
///
/// What it deliberately does not carry: no header of any kind (the one on this
/// API is a credential), no query string, no body, and the *bounded* route label
/// rather than the raw URI, so a path that ever grows an id cannot smuggle one
/// into a log line.
pub async fn log_http_response(request: ExtractRequest, next: Next) -> Response<Body> {
    let method = crate::metrics::metric_method(request.method());
    let route = crate::metrics::metric_route(request.uri().path());
    let started = Instant::now();
    let response = next.run(request).await;
    let latency = started.elapsed();
    tracing::info!(
        http.request.method = method,
        http.route = route,
        http.response.status_code = response.status().as_u16(),
        http.server.request.duration_ms = latency.as_secs_f64() * 1_000.0,
        "http request"
    );
    response
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OtelMakeSpan;

impl<B> MakeSpan<B> for OtelMakeSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        let method = request.method();
        let path = request.uri().path();
        let span = tracing::info_span!(
            "http.server.request",
            otel.name = %format!("{method} {path}"),
            otel.kind = "server",
            http.request.method = %method,
            url.path = %path,
            http.response.status_code = tracing::field::Empty,
            http.server.request.duration_ms = tracing::field::Empty,
            otel.status_code = tracing::field::Empty,
            trace_id = tracing::field::Empty,
            span_id = tracing::field::Empty,
        );
        let parent = global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(request.headers()))
        });
        span.set_parent(parent);
        let context = span.context();
        let context_span = context.span();
        let span_context = context_span.span_context();
        if span_context.is_valid() {
            span.record("trace_id", tracing::field::display(span_context.trace_id()));
            span.record("span_id", tracing::field::display(span_context.span_id()));
        }
        span
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OtelOnResponse;

impl<B> OnResponse<B> for OtelOnResponse {
    fn on_response(self, response: &Response<B>, latency: Duration, span: &Span) {
        span.record(
            "http.response.status_code",
            response.status().as_u16() as u64,
        );
        span.record(
            "http.server.request.duration_ms",
            latency.as_secs_f64() * 1_000.0,
        );
        span.record(
            "otel.status_code",
            if response.status().is_server_error() {
                "ERROR"
            } else {
                "OK"
            },
        );
    }
}

fn build_provider(
    service_name: &str,
) -> Result<opentelemetry_sdk::trace::TracerProvider, opentelemetry::trace::TraceError> {
    let exporter = opentelemetry_otlp::new_exporter()
        .http()
        .with_endpoint(traces_endpoint())
        .with_protocol(Protocol::HttpBinary)
        .with_timeout(Duration::from_secs(5));
    opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(exporter)
        .with_trace_config(Config::default().with_resource(resource(service_name)))
        .install_batch(opentelemetry_sdk::runtime::Tokio)
}

fn traces_endpoint() -> String {
    if let Ok(value) = std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") {
        if !value.trim().is_empty() {
            return value.trim_end_matches('/').to_owned();
        }
    }
    let base = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_OTLP_ENDPOINT.to_owned());
    let base = base.trim_end_matches('/');
    if base.ends_with("/v1/traces") {
        base.to_owned()
    } else {
        format!("{base}/v1/traces")
    }
}

fn resource(service_name: &str) -> Resource {
    let service = std::env::var("OTEL_SERVICE_NAME")
        .ok()
        .filter(|value| valid_attribute_value(value.trim()))
        .unwrap_or_else(|| service_name.to_owned());
    let mut attributes = vec![
        KeyValue::new(semconv::SERVICE_NAME, service),
        KeyValue::new(semconv::SERVICE_NAMESPACE, "3fa-app"),
        KeyValue::new(semconv::SERVICE_VERSION, env!("CARGO_PKG_VERSION")),
    ];
    push_env_attribute(
        &mut attributes,
        "DEPLOYMENT_ENVIRONMENT",
        semconv::DEPLOYMENT_ENVIRONMENT,
    );
    push_env_attribute(
        &mut attributes,
        "POD_NAMESPACE",
        semconv::K8S_NAMESPACE_NAME,
    );
    push_env_attribute(&mut attributes, "POD_NAME", semconv::K8S_POD_NAME);
    push_env_attribute(&mut attributes, "NODE_NAME", semconv::K8S_NODE_NAME);
    push_env_attribute(&mut attributes, "HOSTNAME", semconv::HOST_NAME);
    if let Ok(raw) = std::env::var("OTEL_RESOURCE_ATTRIBUTES") {
        attributes
            .extend(resource_attribute_pairs(&raw).map(|(key, value)| KeyValue::new(key, value)));
    }
    Resource::default().merge(&mut Resource::new(attributes))
}

fn push_env_attribute(attributes: &mut Vec<KeyValue>, env_name: &str, key: &'static str) {
    if let Ok(value) = std::env::var(env_name) {
        let value = value.trim();
        if valid_attribute_value(value) {
            attributes.push(KeyValue::new(key, value.to_owned()));
        }
    }
}

fn resource_attribute_pairs(raw: &str) -> impl Iterator<Item = (String, String)> + '_ {
    raw.split(',').filter_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        let key = key.trim();
        let value = value.trim();
        if valid_attribute_key(key)
            && valid_attribute_value(value)
            && !sensitive_attribute_key(key)
            && !matches!(
                key,
                "service.name" | "service.namespace" | "service.version"
            )
        {
            Some((key.to_owned(), value.to_owned()))
        } else {
            None
        }
    })
}

fn valid_attribute_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_attribute_value(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn sensitive_attribute_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['-', '.'], "_");
    [
        "api_key",
        "apikey",
        "authorization",
        "bearer",
        "cookie",
        "credential",
        "email",
        "jwt",
        "passphrase",
        "passwd",
        "password",
        "private_key",
        "pwd",
        "secret",
        "session",
        "signing_key",
        "token",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::resource_attribute_pairs;

    #[test]
    fn resource_attributes_reject_secrets_and_identity_overrides() {
        let attributes = resource_attribute_pairs(
            "team=simulation,api.token=nope,service.name=spoof,cloud.region=us-east-1",
        )
        .collect::<Vec<_>>();
        assert_eq!(
            attributes,
            vec![
                ("team".to_owned(), "simulation".to_owned()),
                ("cloud.region".to_owned(), "us-east-1".to_owned()),
            ]
        );
    }

    #[test]
    fn resource_attributes_reject_controls_and_oversized_values() {
        let long = "x".repeat(257);
        let raw = format!("good=value,bad=line\nfeed,long={long}");
        assert_eq!(
            resource_attribute_pairs(&raw).collect::<Vec<_>>(),
            vec![("good".to_owned(), "value".to_owned())]
        );
    }

    #[test]
    fn resource_attributes_reject_secret_key_variants() {
        let attributes = resource_attribute_pairs(
            "db.password=nope,session-id=nope,ApiKey=nope,cloud.zone=us-east-1a",
        )
        .collect::<Vec<_>>();
        assert_eq!(
            attributes,
            vec![("cloud.zone".to_owned(), "us-east-1a".to_owned())]
        );
    }
}
