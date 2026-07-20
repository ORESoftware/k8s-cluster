//! Shared NATS connection policy for every Daedalus fabrication process.

use std::{error::Error, time::Duration};

use crate::{
    config::{env_bool, optional_env},
    secrets::SecretOverlay,
};

#[tracing::instrument(
    name = "messaging.connect",
    skip_all,
    err,
    fields(otel.kind = "client", messaging.system = "nats", service.name = service_name)
)]
pub(crate) async fn connect_nats(
    nats_url: &str,
    secrets: &SecretOverlay,
    service_name: &str,
) -> Result<async_nats::Client, Box<dyn Error + Send + Sync>> {
    let mut options = async_nats::ConnectOptions::new()
        .name(service_name)
        .retry_on_initial_connect()
        .ping_interval(Duration::from_secs(15))
        .connection_timeout(Duration::from_secs(10));
    if env_bool("NATS_REQUIRE_TLS", false) {
        options = options.require_tls(true);
    }
    if let Some(path) = optional_env("NATS_CREDENTIALS_FILE") {
        options = options
            .credentials_file(&path)
            .await
            .map_err(|error| format!("failed to read NATS credentials file {path}: {error}"))?;
    } else if let Some(token) = secrets.get("NATS_TOKEN") {
        options = options.token(token);
    } else if let Some(seed) = secrets.get("NATS_NKEY") {
        options = options.nkey(seed);
    }
    Ok(options.connect(nats_url).await?)
}

pub(crate) async fn connect_optional(
    secrets: &SecretOverlay,
    service_name: &str,
) -> Option<async_nats::Client> {
    let url = secrets.get("NATS_URL")?;
    match tokio::time::timeout(
        Duration::from_secs(12),
        connect_nats(&url, secrets, service_name),
    )
    .await
    {
        Ok(Ok(client)) => Some(client),
        Ok(Err(error)) => {
            tracing::error!(
                service.name = service_name,
                server.address = "nats",
                error = %error,
                "NATS connect failed"
            );
            None
        }
        Err(_) => {
            tracing::error!(
                service.name = service_name,
                server.address = "nats",
                "NATS connect timed out"
            );
            None
        }
    }
}
