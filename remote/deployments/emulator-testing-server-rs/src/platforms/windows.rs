use axum::{Json, response::IntoResponse};
use serde_json::json;

pub async fn handle_test() -> impl IntoResponse {
    // Scaffold for Windows headless testing
    tracing::info!("Starting Windows headless test");
    Json(json!({
        "status": "success",
        "message": "Windows test triggered",
        "platform": "windows"
    }))
}
