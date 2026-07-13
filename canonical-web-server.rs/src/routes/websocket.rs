use crate::{
    auth::{require_origin, Authenticated, CredentialSource},
    error::AppError,
    ws, AppState,
};
use axum::{
    extract::{ws::WebSocketUpgrade, State},
    http::HeaderMap,
    response::Response,
};

pub async fn upgrade(
    State(state): State<AppState>,
    Authenticated(actor): Authenticated,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Result<Response, AppError> {
    if actor.source == CredentialSource::SessionCookie {
        require_origin(&headers, &state)?;
    }
    let hub = state.hub.clone();
    let sessions = state.sessions.clone();
    Ok(websocket
        .max_message_size(64 * 1024)
        .max_frame_size(64 * 1024)
        .on_upgrade(move |socket| ws::serve(socket, actor, sessions, hub)))
}
