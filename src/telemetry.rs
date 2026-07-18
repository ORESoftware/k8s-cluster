//! Explicit JSON logging and OTLP tracing. No runtime monkey-patching.

use axum::http::{Request, Response};
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
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
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info,hyper=warn,sqlx=warn"));
    let fmt_layer = tracing_subscriber::fmt::layer()
        .json()
        .flatten_event(true)
        .with_current_span(true)
        .with_span_list(false);

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
                        service = service_name,
                        endpoint = %traces_endpoint(),
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

#[derive(Clone, Copy, Debug, Default)]
pub struct OtelMakeSpan;

impl<B> MakeSpan<B> for OtelMakeSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        let method = request.method();
        let path = request.uri().path();
        let span = tracing::info_span!(
            "http_request",
            otel.name = %format!("{method} {path}"),
            otel.kind = "server",
            http.request.method = %method,
            url.path = %path,
            http.response.status_code = tracing::field::Empty,
        );
        let parent = global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(request.headers()))
        });
        span.set_parent(parent);
        span
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OtelOnResponse;

impl<B> OnResponse<B> for OtelOnResponse {
    fn on_response(self, response: &Response<B>, _latency: Duration, span: &Span) {
        span.record(
            "http.response.status_code",
            response.status().as_u16() as u64,
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
    let service = std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| service_name.to_owned());
    let mut attributes = vec![KeyValue::new(semconv::SERVICE_NAME, service)];
    if let Ok(namespace) = std::env::var("POD_NAMESPACE") {
        attributes.push(KeyValue::new(semconv::K8S_NAMESPACE_NAME, namespace));
    }
    if let Ok(pod) = std::env::var("POD_NAME") {
        attributes.push(KeyValue::new(semconv::K8S_POD_NAME, pod));
    }
    Resource::default().merge(&mut Resource::new(attributes))
}
