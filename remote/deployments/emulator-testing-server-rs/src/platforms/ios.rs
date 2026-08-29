use axum::{Json, response::IntoResponse};
use serde_json::json;

pub async fn handle_test() -> impl IntoResponse {
    // Scaffold for iOS emulator testing
    tracing::info!("Starting iOS headless emulator test");
    Json(json!({
        "status": "success",
        "message": "iOS emulator test triggered",
        "platform": "ios"
    }))
}
