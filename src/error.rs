use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({
            "ok": false,
            "error": self.message,
            "status": self.status.as_u16()
        }));
        (self.status, body).into_response()
    }
}

impl From<sea_orm::DbErr> for ApiError {
    fn from(error: sea_orm::DbErr) -> Self {
        match error {
            sea_orm::DbErr::RecordNotFound(_) => Self::new(StatusCode::NOT_FOUND, "row not found"),
            other => Self::internal(format!("database error: {other}")),
        }
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(error: reqwest::Error) -> Self {
        Self::new(
            StatusCode::BAD_GATEWAY,
            format!("upstream contract service error: {error}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_map_to_expected_status_codes() {
        assert_eq!(ApiError::bad_request("x").status, StatusCode::BAD_REQUEST);
        assert_eq!(ApiError::unauthorized("x").status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            ApiError::unavailable("x").status,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            ApiError::internal("x").status,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn display_uses_message_only() {
        let err = ApiError::bad_request("missing field: caseId");
        assert_eq!(err.to_string(), "missing field: caseId");
    }

    #[test]
    fn db_record_not_found_maps_to_404() {
        let err = ApiError::from(sea_orm::DbErr::RecordNotFound("cases".to_string()));
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        assert_eq!(err.message, "row not found");
    }

    #[test]
    fn other_db_errors_map_to_500_with_context() {
        let err = ApiError::from(sea_orm::DbErr::Custom("boom".to_string()));
        assert_eq!(err.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.message.contains("database error"));
        assert!(err.message.contains("boom"));
    }

    #[test]
    fn into_response_preserves_status() {
        let response = ApiError::unavailable("down").into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let response = ApiError::bad_request("nope").into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
