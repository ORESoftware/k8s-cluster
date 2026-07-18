use std::{sync::OnceLock, time::Duration, time::Instant};

use axum::{body::Body, extract::MatchedPath, http::Request, middleware::Next, response::Response};
use opentelemetry::{
    global,
    metrics::{Counter, Histogram, UpDownCounter},
    trace::TracerProvider as _,
    KeyValue,
};
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::{metrics::SdkMeterProvider, resource::Resource, trace::SdkTracerProvider};
use tracing::{field, Instrument};
use tracing_subscriber::{prelude::*, EnvFilter};

const DEFAULT_FILTER: &str = "info,akrion_web_server=info,tower_http=info";
const DEFAULT_OTLP_HTTP_BASE: &str = "http://127.0.0.1:4318";

pub(crate) struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.meter_provider.take() {
            let _ = provider.shutdown();
        }
        if let Some(provider) = self.tracer_provider.take() {
            let _ = provider.shutdown();
        }
    }
}

struct TelemetryConfig {
    enabled: bool,
    json_logs: bool,
    service_name: String,
    traces_endpoint: Option<String>,
    metrics_endpoint: Option<String>,
}

impl TelemetryConfig {
    fn from_env() -> Self {
        let generic_endpoint = env_string("AKRION_OTEL_EXPORTER_OTLP_ENDPOINT")
            .or_else(|| env_string("OTEL_EXPORTER_OTLP_ENDPOINT"));
        let endpoint_configured = generic_endpoint.is_some()
            || std::env::var_os("AKRION_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").is_some()
            || std::env::var_os("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").is_some()
            || std::env::var_os("AKRION_OTEL_EXPORTER_OTLP_METRICS_ENDPOINT").is_some()
            || std::env::var_os("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT").is_some();
        let base = generic_endpoint
            .as_deref()
            .unwrap_or(DEFAULT_OTLP_HTTP_BASE);

        Self {
            enabled: env_bool("AKRION_TELEMETRY_ENABLED", true),
            json_logs: env_bool("AKRION_LOG_JSON", false),
            service_name: env_string("AKRION_SERVICE_NAME")
                .unwrap_or_else(|| "akrion-web-server".to_string()),
            traces_endpoint: env_bool("AKRION_OTEL_TRACES", endpoint_configured).then(|| {
                env_string("AKRION_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
                    .or_else(|| env_string("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"))
                    .unwrap_or_else(|| signal_endpoint(base, "traces"))
            }),
            metrics_endpoint: env_bool("AKRION_OTEL_METRICS", endpoint_configured).then(|| {
                env_string("AKRION_OTEL_EXPORTER_OTLP_METRICS_ENDPOINT")
                    .or_else(|| env_string("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT"))
                    .unwrap_or_else(|| signal_endpoint(base, "metrics"))
            }),
        }
    }
}

pub(crate) fn init() -> TelemetryGuard {
    let config = TelemetryConfig::from_env();
    if !config.enabled {
        return TelemetryGuard {
            tracer_provider: None,
            meter_provider: None,
        };
    }

    let resource = telemetry_resource(&config.service_name);
    let tracer_provider = config.traces_endpoint.as_deref().and_then(|endpoint| {
        build_tracer_provider(endpoint, resource.clone())
            .map_err(|error| {
                eprintln!(
                    "akrion_telemetry_trace_init_failed service={} error={error}",
                    config.service_name
                );
            })
            .ok()
    });
    let meter_provider = config.metrics_endpoint.as_deref().and_then(|endpoint| {
        build_meter_provider(endpoint, resource)
            .map_err(|error| {
                eprintln!(
                    "akrion_telemetry_metric_init_failed service={} error={error}",
                    config.service_name
                );
            })
            .ok()
    });
    if let Some(provider) = meter_provider.as_ref() {
        global::set_meter_provider(provider.clone());
    }

    let filter = EnvFilter::try_from_env("AKRION_RUST_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let init_result = match (&tracer_provider, config.json_logs) {
        (Some(provider), true) => {
            let tracer = provider.tracer(config.service_name.clone());
            tracing_subscriber::registry()
                .with(filter)
                .with(json_log_layer())
                .with(tracing_opentelemetry::layer().with_tracer(tracer))
                .try_init()
        }
        (Some(provider), false) => {
            let tracer = provider.tracer(config.service_name.clone());
            tracing_subscriber::registry()
                .with(filter)
                .with(compact_log_layer())
                .with(tracing_opentelemetry::layer().with_tracer(tracer))
                .try_init()
        }
        (None, true) => tracing_subscriber::registry()
            .with(filter)
            .with(json_log_layer())
            .try_init(),
        (None, false) => tracing_subscriber::registry()
            .with(filter)
            .with(compact_log_layer())
            .try_init(),
    };
    if let Err(error) = init_result {
        eprintln!(
            "akrion_telemetry_subscriber_init_failed service={} error={error}",
            config.service_name
        );
    }

    tracing::info!(
        event = "akrion_telemetry_initialized",
        service.name = %config.service_name,
        log.format = if config.json_logs { "json" } else { "compact" },
        otel.traces = tracer_provider.is_some(),
        otel.metrics = meter_provider.is_some(),
        log.destination = "stdout",
    );

    TelemetryGuard {
        tracer_provider,
        meter_provider,
    }
}

fn build_tracer_provider(
    endpoint: &str,
    resource: Resource,
) -> Result<SdkTracerProvider, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(endpoint)
        .with_timeout(Duration::from_secs(3))
        .build()?;
    Ok(SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build())
}

fn build_meter_provider(
    endpoint: &str,
    resource: Resource,
) -> Result<SdkMeterProvider, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(endpoint)
        .with_timeout(Duration::from_secs(3))
        .build()?;
    Ok(SdkMeterProvider::builder()
        .with_resource(resource)
        .with_periodic_exporter(exporter)
        .build())
}

fn telemetry_resource(service_name: &str) -> Resource {
    let mut attributes = vec![
        KeyValue::new("service.namespace", "akrion"),
        KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
    ];
    push_env_attribute(&mut attributes, "AKRION_CLUSTER_NAME", "k8s.cluster.name");
    push_env_attribute(&mut attributes, "POD_NAMESPACE", "k8s.namespace.name");
    push_env_attribute(&mut attributes, "HOSTNAME", "k8s.pod.name");
    push_env_attribute(&mut attributes, "NODE_NAME", "k8s.node.name");
    push_env_attribute(
        &mut attributes,
        "AKRION_SOURCE_COMMIT",
        "service.instance.revision",
    );

    Resource::builder()
        .with_service_name(service_name.to_string())
        .with_attributes(attributes)
        .build()
}

fn push_env_attribute(attributes: &mut Vec<KeyValue>, env_name: &str, key: &'static str) {
    if let Some(value) = env_string(env_name) {
        attributes.push(KeyValue::new(key, value));
    }
}

fn signal_endpoint(base: &str, signal: &str) -> String {
    format!("{}/v1/{signal}", base.trim_end_matches('/'))
}

fn json_log_layer<S>() -> tracing_subscriber::fmt::Layer<
    S,
    tracing_subscriber::fmt::format::JsonFields,
    tracing_subscriber::fmt::format::Format<tracing_subscriber::fmt::format::Json>,
>
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
}

fn compact_log_layer<S>() -> tracing_subscriber::fmt::Layer<
    S,
    tracing_subscriber::fmt::format::DefaultFields,
    tracing_subscriber::fmt::format::Format<tracing_subscriber::fmt::format::Compact>,
>
where
    S: tracing::Subscriber + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    tracing_subscriber::fmt::layer()
        .compact()
        .with_ansi(false)
        .with_target(true)
}

struct HttpMetrics {
    requests: Counter<u64>,
    active_requests: UpDownCounter<i64>,
    duration: Histogram<f64>,
}

fn http_metrics() -> &'static HttpMetrics {
    static METRICS: OnceLock<HttpMetrics> = OnceLock::new();
    METRICS.get_or_init(|| {
        let meter = global::meter("akrion-web-server/http");
        HttpMetrics {
            requests: meter
                .u64_counter("http.server.request.count")
                .with_description("Completed HTTP server requests")
                .with_unit("{request}")
                .build(),
            active_requests: meter
                .i64_up_down_counter("http.server.active_requests")
                .with_description("Active HTTP server requests")
                .with_unit("{request}")
                .build(),
            duration: meter
                .f64_histogram("http.server.request.duration")
                .with_description("HTTP server request duration")
                .with_unit("s")
                .build(),
        }
    })
}

pub(crate) async fn record_http_metrics(request: Request<Body>, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().as_str().to_string();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or("unmatched")
        .to_string();
    let active_attributes = [
        KeyValue::new("http.request.method", method.clone()),
        KeyValue::new("http.route", route.clone()),
    ];
    let span = tracing::info_span!(
        "http.server.request",
        http.request.method = %method,
        http.route = %route,
        http.response.status_code = field::Empty,
    );
    let metrics = http_metrics();
    metrics.active_requests.add(1, &active_attributes);

    let response = next.run(request).instrument(span.clone()).await;
    span.record(
        "http.response.status_code",
        i64::from(response.status().as_u16()),
    );
    metrics.active_requests.add(-1, &active_attributes);
    let completed_attributes = [
        KeyValue::new("http.request.method", method),
        KeyValue::new("http.route", route),
        KeyValue::new(
            "http.response.status_code",
            i64::from(response.status().as_u16()),
        ),
    ];
    metrics.requests.add(1, &completed_attributes);
    metrics
        .duration
        .record(started.elapsed().as_secs_f64(), &completed_attributes);
    response
}

fn env_string(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_bool(name: &str, default: bool) -> bool {
    match env_string(name) {
        Some(value) => matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        None => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_otlp_endpoint_gets_signal_path() {
        assert_eq!(
            signal_endpoint("http://collector:4318/", "traces"),
            "http://collector:4318/v1/traces"
        );
    }
}
