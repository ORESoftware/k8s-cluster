use crate::{
    auth::{require_csrf, require_origin, Authenticated, CredentialSource},
    error::AppError,
    routes::health,
    sync::{self, MutationBatch, PullQuery},
    AppState,
};
use axum::{
    extract::{DefaultBodyLimit, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};

pub fn router() -> Router<AppState> {
    let v1 = Router::new()
        .route("/health", get(health::health))
        .route("/info", get(health::info))
        .route("/me", get(me))
        .route("/sync/changes", get(pull))
        .route("/sync/mutations", post(push))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .fallback(not_found);

    Router::new()
        // Compatibility aliases retained while clients move to /api/v1.
        .route("/health", get(health::health))
        .route("/info", get(health::info))
        .nest("/v1", v1)
        .fallback(not_found)
}

async fn me(Authenticated(actor): Authenticated) -> Json<crate::auth::AuthContext> {
    Json(actor)
}

async fn pull(
    State(state): State<AppState>,
    Authenticated(actor): Authenticated,
    Query(query): Query<PullQuery>,
) -> Result<Json<sync::PullResponse>, AppError> {
    Ok(Json(sync::pull_changes(&state, &actor, query).await?))
}

async fn push(
    State(state): State<AppState>,
    Authenticated(actor): Authenticated,
    headers: HeaderMap,
    Json(batch): Json<MutationBatch>,
) -> Result<Json<sync::MutationBatchResponse>, AppError> {
    if actor.source == CredentialSource::SessionCookie {
        require_origin(&headers, &state)?;
        require_csrf(&actor, &headers, None)?;
    }
    Ok(Json(sync::push_mutations(&state, &actor, batch).await?))
}

async fn not_found() -> Response {
    AppError::NotFound.into_response()
}
