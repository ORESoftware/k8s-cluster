//! Coarse API error type. Deliberately leaks nothing — no account-existence,
//! no crypto detail — to a caller (see `ProtocolError` in `shared`).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("bad request")]
    BadRequest,
    #[error("too many requests")]
    TooManyRequests,
    // Returned when human-identity enrollment is disabled because shared-auth
    // has no configured base URL on this deployment.
    #[error("not implemented")]
    NotImplemented,
    // Authentication authority could not decide; this is intentionally not a
    // 401 because an upstream outage is not evidence that a token is invalid.
    #[error("service unavailable")]
    Unavailable,
    #[error("internal error")]
    Internal,
}

// Note: a Postgres unique violation is folded to a coarse 409 at registration.
// Every other SeaORM error flows through this conversion to an opaque 500 and
// is logged only on the server side.

impl From<sea_orm::DbErr> for ApiError {
    fn from(error: sea_orm::DbErr) -> Self {
        tracing::error!(error = %error, "database error");
        ApiError::Internal
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let code = match self {
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::BadRequest => StatusCode::BAD_REQUEST,
            ApiError::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            ApiError::NotImplemented => StatusCode::NOT_IMPLEMENTED,
            ApiError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        // Body intentionally minimal.
        (code, self.to_string()).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_maps_to_its_status_code() {
        let cases = [
            (ApiError::Unauthorized, StatusCode::UNAUTHORIZED),
            (ApiError::BadRequest, StatusCode::BAD_REQUEST),
            (ApiError::TooManyRequests, StatusCode::TOO_MANY_REQUESTS),
            (ApiError::NotImplemented, StatusCode::NOT_IMPLEMENTED),
            (ApiError::Unavailable, StatusCode::SERVICE_UNAVAILABLE),
            (ApiError::Internal, StatusCode::INTERNAL_SERVER_ERROR),
        ];
        for (err, expected) in cases {
            assert_eq!(err.into_response().status(), expected);
        }
    }

    #[test]
    fn database_errors_fold_to_opaque_internal() {
        // Any DB error must surface as a leak-free 500, never a detailed body.
        let err: ApiError = sea_orm::DbErr::RecordNotFound("missing".to_owned()).into();
        assert!(matches!(err, ApiError::Internal));
        assert_eq!(
            err.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
