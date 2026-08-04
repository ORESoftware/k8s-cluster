use std::{env, fmt, net::SocketAddr};

use push_notification_server::{
    ApiState, ContactApiState, NatsConfig, application_router, canonical_json,
    contact_registry_from_env, openapi_document, provider_registry_from_env,
    public_openapi_document, request_authenticator_from_env, run_nats_consumer,
};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenApiScope {
    Internal,
    Public,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArgumentError(String);

impl fmt::Display for ArgumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ArgumentError {}

/// Run the notification service process.
///
/// Keeping orchestration in a module makes the binary entrypoint a thin shell
/// and lets argument and address parsing be tested without initializing
/// telemetry, provider credentials, NATS, or sockets.
pub(crate) async fn run<I>(args: I) -> Result<(), Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = String>,
{
    let args = args.into_iter().collect::<Vec<_>>();
    if let Some(scope) = export_openapi_scope(&args)? {
        let openapi = match scope {
            OpenApiScope::Internal => openapi_document(),
            OpenApiScope::Public => public_openapi_document()?,
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

fn export_openapi_scope(args: &[String]) -> Result<Option<OpenApiScope>, ArgumentError> {
    let mut values = args.iter().filter_map(|argument| {
        argument
            .strip_prefix("--export-openapi=")
            .map(str::to_owned)
    });

    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(ArgumentError(
            "--export-openapi may be specified only once".to_owned(),
        ));
    }

    match value.as_str() {
        "internal" => Ok(Some(OpenApiScope::Internal)),
        "public" => Ok(Some(OpenApiScope::Public)),
        other => Err(ArgumentError(format!(
            "unsupported OpenAPI scope: {other}"
        ))),
    }
}

fn bind_address() -> Result<SocketAddr, std::net::AddrParseError> {
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_owned());
    let port = env::var("PORT").unwrap_or_else(|_| "8121".to_owned());
    parse_bind_address(&host, &port)
}

fn parse_bind_address(host: &str, port: &str) -> Result<SocketAddr, std::net::AddrParseError> {
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
    fn export_scope_is_explicit_and_unambiguous() {
        let public = vec!["server".to_owned(), "--export-openapi=public".to_owned()];
        assert_eq!(
            export_openapi_scope(&public).expect("public scope"),
            Some(OpenApiScope::Public)
        );

        let internal = vec![
            "server".to_owned(),
            "--export-openapi=internal".to_owned(),
        ];
        assert_eq!(
            export_openapi_scope(&internal).expect("internal scope"),
            Some(OpenApiScope::Internal)
        );

        let normal = vec!["server".to_owned()];
        assert_eq!(export_openapi_scope(&normal).expect("normal run"), None);
    }

    #[test]
    fn export_scope_rejects_unknown_or_duplicate_values() {
        let unknown = vec!["server".to_owned(), "--export-openapi=partner".to_owned()];
        assert!(export_openapi_scope(&unknown).is_err());

        let duplicate = vec![
            "server".to_owned(),
            "--export-openapi=public".to_owned(),
            "--export-openapi=internal".to_owned(),
        ];
        assert!(export_openapi_scope(&duplicate).is_err());
    }

    #[test]
    fn bind_address_parser_preserves_the_runtime_contract() {
        let address = parse_bind_address("0.0.0.0", "8121").expect("valid default address");
        assert_eq!(address, "0.0.0.0:8121".parse().expect("socket address"));
        assert!(parse_bind_address("0.0.0.0", "not-a-port").is_err());
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
