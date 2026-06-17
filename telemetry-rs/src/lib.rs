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
//! - Telemetry setup **does not panic** on a bad/unreachable collector (traces are
//!   simply disabled and the service logs normally) and is **idempotent** — a
//!   second `init` is a no-op rather than a double-install panic. Call it from
//!   within a Tokio runtime (e.g. under `#[tokio::main]`): the batch span exporter
//!   is installed via `install_batch(runtime::Tokio)`, which requires one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use http::{Request, Response};
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_http::HeaderExtractor;
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::{propagation::TraceContextPropagator, trace::Config, Resource};
use opentelemetry_semantic_conventions::resource as semconv;
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::trace::{DefaultOnRequest, MakeSpan, OnResponse, TraceLayer};
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
        // Only the guard that actually owns a provider tears telemetry down. An
        // inert guard (logs-only, or a redundant `init` call) must NOT call
        // `global::shutdown_tracer_provider`, or it would kill the real provider
        // installed by the owning guard.
        if let Some(provider) = self.provider.take() {
            for result in provider.force_flush() {
                if let Err(error) = result {
                    // Use eprintln here: the subscriber may already be tearing down.
                    eprintln!("dd-telemetry: span flush on shutdown failed: {error:?}");
                }
            }
            let _ = provider.shutdown();
            global::shutdown_tracer_provider();
        }
    }
}

/// Initialise `tracing` (JSON logs to stdout) + OTLP span export for `service_name`
/// (e.g. `"dd-agent-worker-broker"`). Call once, as early in `main` as possible, and
/// keep the returned [`OtelGuard`] alive.
///
/// Reads, all optional: `RUST_LOG`, `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_SERVICE_NAME`,
/// `OTEL_RESOURCE_ATTRIBUTES`, and `POD_NAME`/`POD_NAMESPACE` (downward API) for the
/// `k8s.pod.name` / `k8s.namespace.name` resource attributes.
/// Set once the first call has installed the global subscriber, so later calls
/// become inert no-ops instead of panicking on a double install.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

pub fn init(service_name: &str) -> OtelGuard {
    // Idempotent: only the first call wires telemetry up. A later call (a test
    // harness, or a service that calls `init` twice) returns an inert guard
    // rather than panicking inside `try_init` — honouring the never-panics
    // contract — and the inert guard's Drop is a no-op (see `OtelGuard::drop`).
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        eprintln!("dd-telemetry: init() called more than once; ignoring the later call");
        return OtelGuard { provider: None };
    }

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
            let tracer = provider.tracer("dd-telemetry");
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            // `try_init` (not `init`) so an already-installed subscriber yields an
            // error we handle, not a panic.
            match tracing_subscriber::registry()
                .with(filter)
                .with(fmt_layer)
                .with(otel_layer)
                .try_init()
            {
                Ok(()) => {
                    // Only touch global state once our subscriber owns the process.
                    global::set_text_map_propagator(TraceContextPropagator::new());
                    global::set_tracer_provider(provider.clone());
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
                    // A subscriber was already installed by something else; our
                    // layers are inactive, so tear the exporter back down.
                    eprintln!(
                        "dd-telemetry: a tracing subscriber was already installed; \
                         continuing without dd-telemetry ({error})"
                    );
                    let _ = provider.shutdown();
                    OtelGuard { provider: None }
                }
            }
        }
        Err(error) => {
            // Exporter unavailable (bad endpoint, etc.) — keep logging, drop traces.
            if tracing_subscriber::registry()
                .with(filter)
                .with(fmt_layer)
                .try_init()
                .is_ok()
            {
                tracing::warn!(
                    service = service_name,
                    error = %error,
                    "dd-telemetry: OTLP exporter unavailable; continuing with logs only"
                );
            } else {
                eprintln!(
                    "dd-telemetry: OTLP exporter unavailable ({error}) and a subscriber \
                     was already installed; dd-telemetry configured nothing"
                );
            }
            OtelGuard { provider: None }
        }
    }
}

/// A [`tower_http`] layer that opens one tracing span per inbound HTTP request,
/// naming it `"{METHOD} {path}"`, extracting any upstream W3C `traceparent` so the
/// span links into the caller's trace, and recording the response status onto the
/// span when it completes. Add to your axum `Router` via `.layer(...)`.
pub fn http_trace_layer(
) -> TraceLayer<SharedClassifier<ServerErrorsAsFailures>, OtelMakeSpan, DefaultOnRequest, OtelOnResponse>
{
    TraceLayer::new_for_http()
        .make_span_with(OtelMakeSpan)
        .on_response(OtelOnResponse)
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

/// [`OnResponse`] implementation used by [`http_trace_layer`]: records the final
/// HTTP status onto the request span, populating the `http.response.status_code`
/// field that [`OtelMakeSpan`] declares empty. Public so the layer's concrete
/// return type is nameable by callers.
#[derive(Clone, Copy, Debug, Default)]
pub struct OtelOnResponse;

impl<B> OnResponse<B> for OtelOnResponse {
    fn on_response(self, response: &Response<B>, _latency: Duration, span: &Span) {
        // Cast to u64: `tracing` records integer fields as i64/u64.
        span.record("http.response.status_code", response.status().as_u16() as u64);
    }
}

/// Resolved OTLP/HTTP traces endpoint. The OTLP HTTP exporter uses a
/// programmatically-set endpoint verbatim (it does not append the signal path),
/// so we resolve the full URL here, following the OTel env-var spec.
fn otlp_traces_endpoint() -> String {
    resolve_traces_endpoint(
        env_nonempty("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").as_deref(),
        env_nonempty("OTEL_EXPORTER_OTLP_ENDPOINT").as_deref(),
    )
}

/// Apply the OTel endpoint precedence: a per-signal `..._TRACES_ENDPOINT` is used
/// as-is; otherwise the base endpoint gets the `/v1/traces` signal path appended
/// (and we never append it twice). Falls back to the in-cluster collector.
fn resolve_traces_endpoint(traces_specific: Option<&str>, base: Option<&str>) -> String {
    if let Some(full) = traces_specific {
        return full.trim_end_matches('/').to_string();
    }
    let base = base.unwrap_or(DEFAULT_OTLP_ENDPOINT).trim_end_matches('/');
    if base.ends_with("/v1/traces") {
        base.to_string()
    } else {
        format!("{base}/v1/traces")
    }
}

/// First non-empty (trimmed) value of `key` from the environment.
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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
    keys.iter().find_map(|key| env_nonempty(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_defaults_to_in_cluster_collector() {
        assert_eq!(
            resolve_traces_endpoint(None, None),
            format!("{DEFAULT_OTLP_ENDPOINT}/v1/traces")
        );
    }

    #[test]
    fn endpoint_appends_signal_path_once() {
        assert_eq!(
            resolve_traces_endpoint(None, Some("http://collector:4318")),
            "http://collector:4318/v1/traces"
        );
        // A trailing slash and an already-present signal path are both handled.
        assert_eq!(
            resolve_traces_endpoint(None, Some("http://collector:4318/")),
            "http://collector:4318/v1/traces"
        );
        assert_eq!(
            resolve_traces_endpoint(None, Some("http://collector:4318/v1/traces")),
            "http://collector:4318/v1/traces"
        );
    }

    #[test]
    fn per_signal_endpoint_wins_and_is_verbatim() {
        // OTEL_EXPORTER_OTLP_TRACES_ENDPOINT takes precedence and is not suffixed.
        assert_eq!(
            resolve_traces_endpoint(Some("http://other:4318/v1/traces"), Some("http://base:4318")),
            "http://other:4318/v1/traces"
        );
    }

    #[test]
    fn http_trace_layer_type_composes() {
        // Ensures make_span + on_response wire together and the nameable return
        // type is correct (a compile/type check, no runtime needed).
        let _layer = http_trace_layer();
    }
}
