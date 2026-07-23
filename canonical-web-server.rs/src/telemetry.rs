//! Explicit OpenTelemetry and Loki-ready structured logging.
//!
//! Kubernetes stdout is the durable log path (Promtail -> Loki). When
//! `OTEL_EXPORTER_OTLP_ENDPOINT` is set, traces and metrics are exported to the
//! OpenTelemetry Collector; the cluster collector exposes those metrics to
//! Prometheus. No runtime APIs or framework internals are patched.

use std::time::Duration;

use axum::{
    extract::MatchedPath,
    http::{Request, Response},
    Router,
};
use opentelemetry::{
    global,
    metrics::{Counter, Histogram},
    trace::TraceContextExt as _,
    KeyValue,
};
use opentelemetry_http::HeaderExtractor;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    metrics::{PeriodicReader, SdkMeterProvider},
    propagation::TraceContextPropagator,
    trace::{SdkTracerProvider, Tracer},
    Resource,
};
use tower_http::trace::{MakeSpan, OnResponse, TraceLayer};
use tracing::{field, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

const EXPORT_TIMEOUT: Duration = Duration::from_secs(5);

/// Owns SDK providers so their final batches are flushed during shutdown.
pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        let tracer_provider = self.tracer_provider.take();
        let meter_provider = self.meter_provider.take();
        if tracer_provider.is_none() && meter_provider.is_none() {
            return;
        }

        if std::thread::spawn(move || {
            if let Some(provider) = meter_provider {
                let _ = provider.shutdown();
            }
            if let Some(provider) = tracer_provider {
                let _ = provider.shutdown();
            }
        })
        .join()
        .is_err()
        {
            eprintln!("telemetry: shutdown flush panicked; final batches may be incomplete");
        }
    }
}

/// Installs JSON stdout logs and optional OTLP trace and metric exporters.
///
/// Exporter configuration fails open to logs-only telemetry. Endpoint details
/// are never printed because OTLP URLs and headers can carry credentials.
pub fn init(service_name: &'static str, service_namespace: &'static str) -> TelemetryGuard {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "canonical_web_server=info,tower_http=info,hyper=warn,h2=warn,reqwest=warn,sea_orm=warn",
        )
    });
    let resource = resource(service_name, service_namespace);
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .filter(|value| !value.trim().is_empty());

    let (tracer_provider, tracer) = endpoint
        .as_deref()
        .and_then(|endpoint| build_tracer_provider(endpoint, resource.clone()).ok())
        .map_or((None, None), |(provider, tracer)| {
            global::set_tracer_provider(provider.clone());
            (Some(provider), Some(tracer))
        });

    let meter_provider = endpoint
        .as_deref()
        .and_then(|endpoint| build_meter_provider(endpoint, resource).ok());
    if let Some(provider) = meter_provider.as_ref() {
        global::set_meter_provider(provider.clone());
    }

    global::set_text_map_propagator(TraceContextPropagator::new());
    install_subscriber(filter, tracer);
    tracing::info!(
        service.name = service_name,
        service.namespace = service_namespace,
        otel.trace_exporter = tracer_provider.is_some(),
        otel.metric_exporter = meter_provider.is_some(),
        log.stream = "stdout",
        "web telemetry initialized"
    );

    TelemetryGuard {
        tracer_provider,
        meter_provider,
    }
}

fn build_tracer_provider(
    endpoint: &str,
    resource: Resource,
) -> Result<(SdkTracerProvider, Tracer), ()> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_timeout(EXPORT_TIMEOUT)
        .build()
        .map_err(|_| ())?;
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();
    use opentelemetry::trace::TracerProvider as _;
    let tracer = provider.tracer("canonical-web-server");
    Ok((provider, tracer))
}

fn build_meter_provider(endpoint: &str, resource: Resource) -> Result<SdkMeterProvider, ()> {
    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_timeout(EXPORT_TIMEOUT)
        .build()
        .map_err(|_| ())?;
    let reader = PeriodicReader::builder(exporter).build();
    Ok(SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(resource)
        .build())
}

fn install_subscriber(filter: EnvFilter, tracer: Option<Tracer>) {
    let result = match tracer {
        Some(tracer) => tracing_subscriber::registry()
            .with(filter)
            .with(stdout_json_layer())
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .try_init(),
        None => tracing_subscriber::registry()
            .with(filter)
            .with(stdout_json_layer())
            .try_init(),
    };
    if result.is_err() {
        eprintln!("telemetry: subscriber already initialized; keeping existing subscriber");
    }
}

fn stdout_json_layer<S>() -> impl tracing_subscriber::Layer<S>
where
    S: tracing::Subscriber + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    tracing_subscriber::fmt::layer()
        .json()
        .flatten_event(true)
        .with_ansi(false)
        .with_current_span(true)
        .with_span_list(true)
        .with_target(true)
        .with_writer(std::io::stdout)
}

fn resource(service_name: &str, service_namespace: &str) -> Resource {
    let mut attributes = vec![
        KeyValue::new("service.name", service_name.to_string()),
        KeyValue::new("service.namespace", service_namespace.to_string()),
        KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
    ];
    push_env_attribute(&mut attributes, "DEPLOYMENT_ENV", "deployment.environment");
    push_env_attribute(&mut attributes, "POD_NAMESPACE", "k8s.namespace.name");
    push_env_attribute(&mut attributes, "POD_NAME", "k8s.pod.name");
    push_env_attribute(&mut attributes, "NODE_NAME", "k8s.node.name");
    push_env_attribute(&mut attributes, "HOSTNAME", "host.name");

    if let Ok(raw) = std::env::var("OTEL_RESOURCE_ATTRIBUTES") {
        attributes
            .extend(resource_attribute_pairs(&raw).map(|(key, value)| KeyValue::new(key, value)));
    }
    Resource::builder_empty()
        .with_attributes(attributes)
        .build()
}

fn push_env_attribute(attributes: &mut Vec<KeyValue>, env_name: &str, key: &'static str) {
    if let Ok(value) = std::env::var(env_name) {
        let value = value.trim();
        if valid_attribute_value(value) {
            attributes.push(KeyValue::new(key, value.to_string()));
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
            && !matches!(key, "service.name" | "service.namespace")
        {
            Some((key.to_string(), value.to_string()))
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
        "authorization",
        "access_key",
        "bearer",
        "cookie",
        "credential",
        "connection_string",
        "database",
        "dsn",
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
        "api_key",
        "apikey",
        "uri",
        "url",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

/// Adds explicit HTTP spans and low-cardinality request metrics to the router.
pub fn instrument_http(app: Router) -> Router {
    let meter = global::meter("canonical-web-server");
    let completed = meter
        .u64_counter("http.server.request.count")
        .with_description("Number of HTTP responses completed")
        .with_unit("{request}")
        .build();
    let duration = meter
        .f64_histogram("http.server.request.duration")
        .with_description("HTTP request duration")
        .with_unit("s")
        .build();

    app.layer(
        TraceLayer::new_for_http()
            .make_span_with(HttpMakeSpan)
            .on_response(HttpOnResponse {
                completed,
                duration,
            }),
    )
}

#[derive(Clone, Copy, Debug, Default)]
struct HttpMakeSpan;

impl<B> MakeSpan<B> for HttpMakeSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        let method = request.method();
        let route = route_template(request);
        let request_id = request
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        let span = tracing::info_span!(
            "http.server.request",
            otel.name = %format!("{method} {route}"),
            otel.kind = "server",
            http.request.method = %method,
            http.route = route,
            http.response.status_code = field::Empty,
            otel.status_code = field::Empty,
            request.id = request_id,
            trace_id = field::Empty,
            span_id = field::Empty,
        );
        let parent = global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(request.headers()))
        });
        let _ = span.set_parent(parent);
        record_trace_context(&span);
        span
    }
}

fn route_template<B>(request: &Request<B>) -> &str {
    request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or("<unmatched>")
}

fn record_trace_context(span: &Span) {
    let context = span.context();
    let otel_span = context.span();
    let span_context = otel_span.span_context();
    if span_context.is_valid() {
        span.record("trace_id", span_context.trace_id().to_string());
        span.record("span_id", span_context.span_id().to_string());
    }
}

#[derive(Clone)]
struct HttpOnResponse {
    completed: Counter<u64>,
    duration: Histogram<f64>,
}

impl<B> OnResponse<B> for HttpOnResponse {
    fn on_response(self, response: &Response<B>, latency: Duration, span: &Span) {
        let status = response.status();
        span.record("http.response.status_code", status.as_u16() as u64);
        if status.is_server_error() {
            span.record("otel.status_code", "ERROR");
        }

        let attributes = [
            KeyValue::new("http.response.status_code", i64::from(status.as_u16())),
            KeyValue::new("http.response.status_class", status_class(status.as_u16())),
        ];
        self.completed.add(1, &attributes);
        self.duration.record(latency.as_secs_f64(), &attributes);
    }
}

fn status_class(status: u16) -> &'static str {
    match status {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_attributes_reject_secrets_and_identity_overrides() {
        let attributes = resource_attribute_pairs(
            "team=canonical,api.token=nope,service.name=spoof,cloud.region=us-east-1,db.connection_string=secret,database_url=secret,backup.dsn=secret",
        )
        .collect::<Vec<_>>();
        assert_eq!(
            attributes,
            vec![
                ("team".to_string(), "canonical".to_string()),
                ("cloud.region".to_string(), "us-east-1".to_string()),
            ]
        );
    }

    #[test]
    fn status_classes_are_bounded() {
        assert_eq!(status_class(204), "2xx");
        assert_eq!(status_class(404), "4xx");
        assert_eq!(status_class(503), "5xx");
        assert_eq!(status_class(700), "other");
    }

    #[test]
    fn unmatched_routes_never_export_the_raw_path() {
        let request = Request::builder()
            .uri("/password-reset/private-token")
            .body(())
            .unwrap();
        assert_eq!(route_template(&request), "<unmatched>");
    }
}
