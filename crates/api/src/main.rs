//! t2v-api — the JSON API server for the t2v-v2t platform.
//!
//! The runtime router and OpenAPI documents are assembled from the same
//! `utoipa_axum::routes!` declarations in the library. `--export-openapi`
//! exits before telemetry, database, provider clients, or sockets are touched.

use std::io::Write;
use std::net::SocketAddr;
use t2v_api::{app, db, openapi, openapi_documents, state::AppState};

fn port() -> u16 {
    std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8130)
}

fn export_scope() -> Result<Option<String>, String> {
    let mut scope = None;
    for argument in std::env::args().skip(1) {
        if let Some(value) = argument.strip_prefix("--export-openapi=") {
            if scope.replace(value.to_string()).is_some() {
                return Err("--export-openapi may be supplied only once".to_string());
            }
        } else {
            return Err(format!("unknown command-line argument: {argument}"));
        }
    }
    Ok(scope)
}

fn export_openapi(scope: &str) -> Result<(), Box<dyn std::error::Error>> {
    let documents = openapi_documents().map_err(std::io::Error::other)?;
    let document = match scope {
        "public" => &documents.public,
        "internal" => &documents.internal,
        other => {
            return Err(
                format!("unknown OpenAPI scope '{other}'; expected public or internal").into(),
            )
        }
    };
    let json = openapi::canonical_json(document)?;
    std::io::stdout().write_all(json.as_bytes())?;
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal;
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("t2v-api shutdown signal received");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(scope) = export_scope().map_err(std::io::Error::other)? {
        export_openapi(&scope)?;
        return Ok(());
    }

    // Keep the provider alive through graceful shutdown so queued traces,
    // metrics, and structured warnings are flushed before the pod exits.
    let _telemetry = fiducia_telemetry::init("dd-t2v-api");

    let db = db::connect_and_prepare().await?;
    let state = AppState::new(db);

    let providers: Vec<&str> = state
        .llm
        .configured_providers()
        .iter()
        .map(|p| p.as_str())
        .collect();
    tracing::info!(
        "t2v-api starting: translation providers configured = {:?}, vapi = {}",
        providers,
        state.vapi.is_configured()
    );

    let addr = SocketAddr::from(([0, 0, 0, 0], port()));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("t2v-api listening on http://{addr}");

    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}
