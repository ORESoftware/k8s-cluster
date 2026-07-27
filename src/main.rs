use std::{env, net::SocketAddr};

use axum::{routing::get, Json, Router};
use serde_json::{json, Value};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("push_notification_server=info,tower_http=info")),
        )
        .init();

    let address = bind_address()?;
    let app = app();
    let listener = tokio::net::TcpListener::bind(address).await?;

    tracing::info!(%address, "push notification server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

fn app() -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
}

async fn healthz() -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": "push-notification-server"
    }))
}

async fn readyz() -> Json<Value> {
    Json(json!({
        "ok": true,
        "providers": {
            "fcm": false,
            "apns": false,
            "expo": false,
            "webpush": false
        }
    }))
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
        use tokio::signal::unix::{signal, SignalKind};
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
    use super::*;

    #[test]
    fn default_bind_address_is_valid() {
        let address: SocketAddr = "0.0.0.0:8121".parse().expect("valid default address");
        assert_eq!(address.port(), 8121);
    }

    #[test]
    fn router_can_be_constructed() {
        let _ = app();
    }
}
