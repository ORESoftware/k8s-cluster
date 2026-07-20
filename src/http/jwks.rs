//! Publish our public JWKS so downstream services can verify minted tokens.

use axum::{extract::State, Json};

use crate::state::AppState;

pub async fn jwks(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(state.minter.jwks().as_json().clone())
}
