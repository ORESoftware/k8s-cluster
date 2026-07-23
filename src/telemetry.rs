//! OTLP tracing and Loki-compatible JSON logs.

use axum::http::{Request, Response};
use opentelemetry::global;
use opentelemetry::trace::{TraceContextExt, TracerProvider as _};
use opentelemetry::KeyValue;
use opentelemetry_http::HeaderExtractor;
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::{propagation::TraceContextPropagator, trace::Config, Resource};
use opentelemetry_semantic_conventions::resource as semconv;
use std::time::Duration;
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::trace::{DefaultOnRequest, MakeSpan, OnResponse, TraceLayer};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

const DEFAULT_OTLP_ENDPOINT: &str = "http://dd-otel-collector.observability.svc.cluster.local:4318";

pub(crate) struct Guard {
    provider: Option<opentelemetry_sdk::trace::TracerProvider>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            for result in provider.force_flush() {
                if let Err(error) = result {
                    eprintln!("threefa-web-telemetry: span flush failed: {error:?}");
                }
            }
            let _ = provider.shutdown();
            global::shutdown_tracer_provider();
        }
    }
}

pub(crate) fn init(service_name: &str) -> Guard {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info,hyper=warn"));
    let fmt_layer = tracing_subscriber::fmt::layer()
        .json()
        .flatten_event(true)
        .with_ansi(false)
        .with_current_span(true)
        // Keep the parent request trace ids visible in Loki when nested auth
        // exchange/verification spans emit an event.
        .with_span_list(true)
        .with_target(true);
    match build_provider(service_name) {
        Ok(provider) => {
            let tracer = provider.tracer("threefa-web-server");
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
                    eprintln!("threefa-web-telemetry: subscriber already installed: {error}");
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

pub(crate) fn http_trace_layer() -> TraceLayer<
    SharedClassifier<ServerErrorsAsFailures>,
    OtelMakeSpan,
    DefaultOnRequest,
    OtelOnResponse,
> {
    TraceLayer::new_for_http()
        .make_span_with(OtelMakeSpan)
        .on_response(OtelOnResponse)
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OtelMakeSpan;

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
pub(crate) struct OtelOnResponse;

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
    traces_endpoint_from_base(&base)
}

fn traces_endpoint_from_base(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with("/v1/traces") {
        base.to_owned()
    } else {
        format!("{base}/v1/traces")
    }
}

fn resource(service_name: &str) -> Resource {
    let mut attributes = vec![
        KeyValue::new(semconv::SERVICE_NAME, service_name.to_owned()),
        KeyValue::new(semconv::SERVICE_NAMESPACE, "3fa-app"),
        KeyValue::new(semconv::SERVICE_VERSION, env!("CARGO_PKG_VERSION")),
    ];
    for (env_name, key) in [
        ("DEPLOYMENT_ENVIRONMENT", semconv::DEPLOYMENT_ENVIRONMENT),
        ("POD_NAMESPACE", semconv::K8S_NAMESPACE_NAME),
        ("POD_NAME", semconv::K8S_POD_NAME),
        ("NODE_NAME", semconv::K8S_NODE_NAME),
        ("HOSTNAME", semconv::HOST_NAME),
    ] {
        if let Ok(value) = std::env::var(env_name) {
            let value = value.trim();
            if valid_resource_value(value) {
                attributes.push(KeyValue::new(key, value.to_owned()));
            }
        }
    }
    Resource::default().merge(&mut Resource::new(attributes))
}

fn valid_resource_value(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::{traces_endpoint_from_base, valid_resource_value};

    #[test]
    fn otlp_base_urls_normalize_to_the_trace_signal_path() {
        assert_eq!(
            traces_endpoint_from_base("http://collector:4318"),
            "http://collector:4318/v1/traces"
        );
        assert_eq!(
            traces_endpoint_from_base("http://collector:4318/"),
            "http://collector:4318/v1/traces"
        );
        assert_eq!(
            traces_endpoint_from_base("http://collector:4318/v1/traces"),
            "http://collector:4318/v1/traces"
        );
    }

    #[test]
    fn resource_values_are_bounded_and_single_line() {
        assert!(valid_resource_value("threefa"));
        assert!(!valid_resource_value(""));
        assert!(!valid_resource_value("line\nfeed"));
        assert!(!valid_resource_value(&"x".repeat(257)));
    }
}
