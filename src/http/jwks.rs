//! Publish our public JWKS so downstream services can verify minted tokens.

use axum::{extract::State, http::header, response::IntoResponse, Json};

use crate::state::AppState;

pub async fn jwks(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(
            header::CACHE_CONTROL,
            "public, max-age=300, stale-if-error=3600",
        )],
        Json(state.minter.jwks().as_json().clone()),
    )
}
