use canonical_web_server::{command, telemetry, SERVICE};
use tracing::Instrument as _;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _telemetry = telemetry::init(SERVICE, "canonical-cloud");
    let service_span = tracing::info_span!(
        "service.run",
        service.name = SERVICE,
        service.namespace = "canonical-cloud",
    );
    let command = std::env::args().nth(1);
    command::run(command.as_deref())
        .instrument(service_span)
        .await
}
