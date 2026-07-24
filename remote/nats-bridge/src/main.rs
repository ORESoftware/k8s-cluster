use axum::{
    extract::{Path, State, Json},
    http::StatusCode,
    routing::{get, post},
    Router,
};
use bytes::Bytes;
use std::net::SocketAddr;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use async_nats::Client;

#[derive(Clone)]
struct AppState {
    nats_client: Client,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nats_bridge=debug,info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string());
    tracing::info!("Connecting to NATS at {}", nats_url);
    
    // In a real k8s environment, the service might be available via DNS
    let nats_client = async_nats::connect(&nats_url).await.expect("Fatal: Could not connect to NATS");

    let state = AppState { nats_client };

    let app = Router::new()
        .route("/health", get(|| async { "OK" }))
        .route("/publish/:subject", post(publish_handler))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3004));
    tracing::info!("listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn publish_handler(
    Path(subject): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> Result<String, (StatusCode, String)> {
    let payload_bytes = Bytes::from(payload.to_string());
    
    match state.nats_client.publish(subject.clone(), payload_bytes).await {
        Ok(_) => {
            tracing::info!("Successfully published to subject: {}", subject);
            Ok(format!("Published to {}", subject))
        }
        Err(e) => {
            tracing::error!("Failed to publish to subject {}: {:?}", subject, e);
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to publish message".to_string()))
        }
    }
}
