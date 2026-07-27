use axum::{Json, response::IntoResponse};
use serde_json::json;

pub async fn handle_test() -> impl IntoResponse {
    // Scaffold for macOS headless testing
    tracing::info!("Starting macOS headless test");
    Json(json!({
        "status": "success",
        "message": "macOS test triggered",
        "platform": "macos"
    }))
}
