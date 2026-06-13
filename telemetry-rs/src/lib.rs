//! Shared OpenTelemetry tracing + structured logging for the Rust services in
//! `k8s-cluster/remote`.
//!
//! Drop-in usage — three lines per service, no monkey-patching:
//!
//! ```ignore
//! #[tokio::main]
//! async fn main() {
//!     let _otel = dd_telemetry::init("dd-agent-worker-broker"); // hold for all of main
//!     // ...
//!     let app = Router::new()
//!         // ...routes...
//!         .layer(dd_telemetry::http_trace_layer());             // one span per request
//! }
//! ```
//!
//! Design:
//! - **Logs** are emitted as structured JSON on stdout (already shipped to Loki by
//!   promtail) via `tracing`, and carry the active `trace_id`/`span_id`.
//! - **Traces** (spans) are exported over OTLP/HTTP protobuf to the in-cluster
//!   collector (`dd-otel-collector.observability:4318`), which fans them out to
//!   Tempo + Jaeger.
//! - Everything is **explicit**: a real subscriber, a real exporter, a real tower
//!   layer. No runtime patching, no auto-instrumentation agent.
//! - Telemetry setup **never panics**: if the exporter can't be built the service
//!   still starts and logs normally (traces are simply disabled).

use std::time::Duration;

use http::Request;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_http::HeaderExtractor;
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::{propagation::TraceContextPropagator, trace::Config, Resource};
use opentelemetry_semantic_conventions::resource as semconv;
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::trace::{MakeSpan, TraceLayer};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// In-cluster OTel collector OTLP/HTTP base URL, used when
/// `OTEL_EXPORTER_OTLP_ENDPOINT` is not set in the environment.
const DEFAULT_OTLP_ENDPOINT: &str =
    "http://dd-otel-collector.observability.svc.cluster.local:4318";

/// Guard returned by [`init`]. Holds the tracer provider for the life of the process
/// and flushes any buffered spans when it is dropped (i.e. on shutdown). Bind it for
/// all of `main` — `let _otel = dd_telemetry::init(...)` — or spans may be lost.
#[must_use = "hold the OtelGuard for the lifetime of the process; dropping it flushes and shuts telemetry down"]
pub struct OtelGuard {
    provider: Option<opentelemetry_sdk::trace::TracerProvider>,
}

impl Drop for OtelGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            for result in provider.force_flush() {
                if let Err(error) = result {
                    // Use eprintln here: the subscriber may already be tearing down.
                    eprintln!("dd-telemetry: span flush on shutdown failed: {error:?}");
                }
            }
            let _ = provider.shutdown();
        }
        global::shutdown_tracer_provider();
    }
}

/// Initialise `tracing` (JSON logs to stdout) + OTLP span export for `service_name`
/// (e.g. `"dd-agent-worker-broker"`). Call once, as early in `main` as possible, and
/// keep the returned [`OtelGuard`] alive.
///
/// Reads, all optional: `RUST_LOG`, `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_SERVICE_NAME`,
/// `OTEL_RESOURCE_ATTRIBUTES`, and `POD_NAME`/`POD_NAMESPACE` (downward API) for the
/// `k8s.pod.name` / `k8s.namespace.name` resource attributes.
pub fn init(service_name: &str) -> OtelGuard {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "info,tower_http=info,hyper=warn,h2=warn,reqwest=warn,sqlx=warn,sea_orm=warn",
        )
    });

    // Structured JSON on stdout -> promtail -> Loki. `flatten_event` keeps log fields
    // at the top level; the current span (incl. trace_id/span_id from the otel layer)
    // is attached so logs correlate to traces.
    let fmt_layer = tracing_subscriber::fmt::layer()
        .json()
        .flatten_event(true)
        .with_current_span(true)
        .with_span_list(false);

    match build_tracer_provider(service_name) {
        Ok(provider) => {
            global::set_text_map_propagator(TraceContextPropagator::new());
            let tracer = provider.tracer("dd-telemetry");
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            global::set_tracer_provider(provider.clone());
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt_layer)
                .with(otel_layer)
                .init();
            tracing::info!(
                service = service_name,
                endpoint = %otlp_traces_endpoint(),
                "dd-telemetry initialised; OTLP trace export enabled"
            );
            OtelGuard {
                provider: Some(provider),
            }
        }
        Err(error) => {
            // Exporter unavailable (bad endpoint, etc.) — keep logging, drop traces.
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt_layer)
                .init();
            tracing::warn!(
                service = service_name,
                error = %error,
                "dd-telemetry: OTLP exporter unavailable; continuing with logs only"
            );
            OtelGuard { provider: None }
        }
    }
}

/// A [`tower_http`] layer that opens one tracing span per inbound HTTP request,
/// naming it `"{METHOD} {path}"`, extracting any upstream W3C `traceparent` so the
/// span links into the caller's trace. Add to your axum `Router` via `.layer(...)`.
pub fn http_trace_layer() -> TraceLayer<SharedClassifier<ServerErrorsAsFailures>, OtelMakeSpan> {
    TraceLayer::new_for_http().make_span_with(OtelMakeSpan)
}

/// [`MakeSpan`] implementation used by [`http_trace_layer`]. Public so the layer's
/// concrete return type is nameable by callers.
#[derive(Clone, Copy, Debug, Default)]
pub struct OtelMakeSpan;

impl<B> MakeSpan<B> for OtelMakeSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        let method = request.method();
        let path = request.uri().path();
        // `otel.name` overrides the exported span name; `otel.kind = server` marks
        // this as the inbound side of an RPC for Tempo/Jaeger.
        let span = tracing::info_span!(
            "http_request",
            otel.name = %format!("{method} {path}"),
            otel.kind = "server",
            http.request.method = %method,
            url.path = %path,
            http.response.status_code = tracing::field::Empty,
        );
        // Link to the upstream trace if the caller propagated W3C context.
        let parent = global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(request.headers()))
        });
        span.set_parent(parent);
        span
    }
}

/// Resolved OTLP/HTTP traces endpoint (base + `/v1/traces`). The OTLP HTTP exporter
/// uses a programmatically-set endpoint verbatim, so we append the signal path here.
fn otlp_traces_endpoint() -> String {
    let base = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_OTLP_ENDPOINT.to_string());
    format!("{}/v1/traces", base.trim_end_matches('/'))
}

fn build_tracer_provider(
    service_name: &str,
) -> Result<opentelemetry_sdk::trace::TracerProvider, opentelemetry::trace::TraceError> {
    let exporter = opentelemetry_otlp::new_exporter()
        .http()
        .with_endpoint(otlp_traces_endpoint())
        .with_protocol(Protocol::HttpBinary)
        .with_timeout(Duration::from_secs(5));

    opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(exporter)
        .with_trace_config(Config::default().with_resource(resource(service_name)))
        .install_batch(opentelemetry_sdk::runtime::Tokio)
}

/// Build the OTel `Resource`. `Resource::default()` already folds in `OTEL_SERVICE_NAME`
/// and `OTEL_RESOURCE_ATTRIBUTES` from the environment; we overlay an explicit
/// `service.name` fallback plus the k8s pod/namespace from the downward-API env vars.
fn resource(service_name: &str) -> Resource {
    let service =
        std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| service_name.to_string());
    let mut attributes = vec![KeyValue::new(semconv::SERVICE_NAME, service)];

    if let Some(namespace) = first_env(&["POD_NAMESPACE", "K8S_NAMESPACE_NAME"]) {
        attributes.push(KeyValue::new(semconv::K8S_NAMESPACE_NAME, namespace));
    }
    if let Some(pod) = first_env(&["POD_NAME", "K8S_POD_NAME", "HOSTNAME"]) {
        attributes.push(KeyValue::new(semconv::K8S_POD_NAME, pod));
    }

    Resource::default().merge(&mut Resource::new(attributes))
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}
