use crate::{auth::SessionAuthenticated, error::AppError, views, AppState};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(dashboard))
        .route("/fragments/session", get(session_fragment))
        .fallback(not_found)
}

async fn dashboard(auth: Result<SessionAuthenticated, AppError>) -> Response {
    match auth {
        Ok(SessionAuthenticated(actor)) => views::dashboard(&actor).into_response(),
        Err(AppError::Unauthorized) => Redirect::to("/login").into_response(),
        Err(error) => error.into_response(),
    }
}

async fn session_fragment(auth: Result<SessionAuthenticated, AppError>) -> Response {
    match auth {
        Ok(SessionAuthenticated(actor)) => views::session_fragment(&actor).into_response(),
        Err(AppError::Unauthorized) => {
            let mut response = StatusCode::UNAUTHORIZED.into_response();
            response.headers_mut().insert(
                "hx-redirect",
                axum::http::HeaderValue::from_static("/login"),
            );
            response
        }
        Err(error) => error.into_response(),
    }
}

async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, views::html_not_found()).into_response()
}
