use axum::{Json, response::IntoResponse};
use serde_json::json;

pub async fn handle_test() -> impl IntoResponse {
    // Scaffold for Android emulator testing
    tracing::info!("Starting Android headless emulator test");
    Json(json!({
        "status": "success",
        "message": "Android emulator test triggered",
        "platform": "android"
    }))
}
