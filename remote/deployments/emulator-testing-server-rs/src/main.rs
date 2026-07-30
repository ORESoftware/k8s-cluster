mod platforms;

use axum::{
    routing::{get, post},
    Router, Json,
};
use serde_json::json;
use tokio::net::TcpListener;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "emulator_testing_server_rs=debug,info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Build the router
    let app = Router::new()
        .route("/health", get(|| async { Json(json!({"status": "ok"})) }))
        .route("/test/android", post(platforms::android::handle_test))
        .route("/test/ios", post(platforms::ios::handle_test))
        .route("/test/macos", post(platforms::macos::handle_test))
        .route("/test/windows", post(platforms::windows::handle_test));

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{}", port);
    
    tracing::info!("Server listening on {}", addr);
    
    let listener = TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
