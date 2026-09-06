use std::time::Duration;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::{trace::SdkTracerProvider, Resource};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

const DEFAULT_OTLP_ENDPOINT: &str = "http://dd-otel-collector.observability.svc.cluster.local:4318";

pub struct Guard(Option<SdkTracerProvider>);

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(provider) = self.0.take() {
            if let Err(error) = provider.force_flush() {
                eprintln!("shared-auth-nats-bridge telemetry flush failed: {error:?}");
            }
            let _ = provider.shutdown();
        }
    }
}

pub fn init(service_name: &str) -> Guard {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,reqwest=warn"));
    // MCP owns stdout. JSON logs must go to stderr or they corrupt the protocol.
    let logs = tracing_subscriber::fmt::layer()
        .json()
        .flatten_event(true)
        .with_writer(std::io::stderr);

    match provider(service_name) {
        Ok(provider) => {
            let tracer = provider.tracer("shared-auth-nats-bridge");
            let otel = tracing_opentelemetry::layer().with_tracer(tracer);
            if tracing_subscriber::registry()
                .with(filter)
                .with(logs)
                .with(otel)
                .try_init()
                .is_ok()
            {
                tracing::info!("OpenTelemetry trace export enabled");
                Guard(Some(provider))
            } else {
                let _ = provider.shutdown();
                Guard(None)
            }
        }
        Err(error) => {
            let fallback_filter = EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,reqwest=warn"));
            let fallback_logs = tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_writer(std::io::stderr);
            let _ = tracing_subscriber::registry()
                .with(fallback_filter)
                .with(fallback_logs)
                .try_init();
            tracing::warn!(%error, "OTLP exporter unavailable; using structured logs only");
            Guard(None)
        }
    }
}

fn provider(
    service_name: &str,
) -> Result<SdkTracerProvider, opentelemetry_otlp::ExporterBuildError> {
    let endpoint = traces_endpoint();
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .with_protocol(Protocol::HttpBinary)
        .with_timeout(Duration::from_secs(5))
        .build()?;
    let service = std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| service_name.to_owned());
    let resource = Resource::builder().with_service_name(service).build();
    Ok(SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build())
}

fn traces_endpoint() -> String {
    if let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") {
        if !endpoint.trim().is_empty() {
            return endpoint.trim_end_matches('/').to_owned();
        }
    }
    let base = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| DEFAULT_OTLP_ENDPOINT.to_owned());
    let base = base.trim_end_matches('/');
    if base.ends_with("/v1/traces") {
        base.to_owned()
    } else {
        format!("{base}/v1/traces")
    }
}
