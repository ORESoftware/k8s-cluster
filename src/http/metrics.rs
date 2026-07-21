//! Prometheus exposition from the app's own registry (see [`crate::metrics`]).

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;

use crate::state::AppState;

pub async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let (content_type, body) = state.metrics.render();
    (StatusCode::OK, [(header::CONTENT_TYPE, content_type)], body)
}
