use std::{env, net::SocketAddr};

use push_notification_server::{
    ApiState, ContactApiState, NatsConfig, application_router, canonical_json,
    contact_registry_from_env, openapi_document, provider_registry_from_env,
    public_openapi_document, request_authenticator_from_env, run_nats_consumer,
};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(scope) = export_openapi_scope() {
        let openapi = match scope.as_str() {
            "internal" => openapi_document(),
            "public" => public_openapi_document()?,
            other => return Err(format!("unsupported OpenAPI scope: {other}").into()),
        };
        print!("{}", canonical_json(&openapi)?);
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                EnvFilter::new("push_notification_server=info,tower_http=info")
            }),
        )
        .init();

    let address = bind_address()?;
    let registry = provider_registry_from_env()?;
    let contact_registry = contact_registry_from_env()?;
    let authenticator = request_authenticator_from_env()?;

    if let Some(nats_config) = NatsConfig::from_env()? {
        let nats_registry = registry.clone();
        tokio::spawn(async move {
            if let Err(error) = run_nats_consumer(nats_config, nats_registry).await {
                tracing::error!(%error, "JetStream push ingestion stopped");
            }
        });
    } else {
        tracing::info!("JetStream push ingestion disabled because NATS_URL is not configured");
    }

    let app = application_router(
        ApiState::new(registry, authenticator.clone()),
        ContactApiState::new(contact_registry, authenticator.clone()),
        authenticator,
    )?;
    let listener = tokio::net::TcpListener::bind(address).await?;

    tracing::info!(%address, "notification server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

fn export_openapi_scope() -> Option<String> {
    env::args().find_map(|argument| {
        argument
            .strip_prefix("--export-openapi=")
            .map(ToOwned::to_owned)
    })
}

fn bind_address() -> Result<SocketAddr, std::net::AddrParseError> {
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_owned());
    let port = env::var("PORT").unwrap_or_else(|_| "8121".to_owned());
    format!("{host}:{port}").parse()
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut stream) = signal(SignalKind::terminate()) {
            let _ = stream.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use push_notification_server::{
        ContactProviderRegistry, DenyAllAuthenticator, ProviderRegistry,
    };

    use super::*;

    #[test]
    fn default_bind_address_is_valid() {
        let address: SocketAddr = "0.0.0.0:8121".parse().expect("valid default address");
        assert_eq!(address.port(), 8121);
    }

    #[test]
    fn routers_can_be_constructed_without_runtime_credentials() {
        let authenticator = Arc::new(DenyAllAuthenticator);
        let _ = push_notification_server::application_router(
            ApiState::new(ProviderRegistry::new(), authenticator.clone()),
            ContactApiState::new(ContactProviderRegistry::new(), authenticator.clone()),
            authenticator,
        )
        .expect("application router");
    }
}
