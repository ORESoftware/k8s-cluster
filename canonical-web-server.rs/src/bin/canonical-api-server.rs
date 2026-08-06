use canonical_web_server::{api_server, config::Config, telemetry};
use tracing::Instrument as _;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _telemetry = telemetry::init(api_server::SERVICE, "canonical-cloud");
    let service_span = tracing::info_span!(
        "service.run",
        service.name = api_server::SERVICE,
        service.namespace = "canonical-cloud",
    );
    api_server::run(Config::from_env()?)
        .instrument(service_span)
        .await?;
    Ok(())
}
