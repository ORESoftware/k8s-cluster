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
    #[error("conflict")]
    Conflict,
    #[error("gone")]
    Gone,
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
// Every other SeaORM error flows through this conversion and is logged only on
// the server side.
//
// The split below is the difference between "this deployment is broken" and
// "the database is briefly unreachable, back off and retry". PROTOCOL.md §8
// documents 503 for the latter, and clients, dashboards, and traces
// (`otel.status_code = ERROR` on 5xx) all key off it: folding an infrastructure
// blip into a 500 makes it indistinguishable from a service defect.

impl From<sea_orm::DbErr> for ApiError {
    fn from(error: sea_orm::DbErr) -> Self {
        if database_is_unreachable(&error) {
            tracing::warn!(error = %error, "database unavailable");
            return ApiError::Unavailable;
        }
        tracing::error!(error = %error, "database error");
        ApiError::Internal
    }
}

/// True when the error says the *database* could not be reached, rather than
/// that a query was wrong. Only these become 503; a genuine query/logic fault
/// stays an opaque 500 because retrying it would not help.
///
/// The variants are the ones SeaORM actually produces on this path (see
/// `sea_orm::driver::sqlx_common::sqlx_conn_acquire_err`): pool acquisition
/// failures become `ConnectionAcquire`, everything else at connect time becomes
/// `Conn`, and a connection that dies mid-statement surfaces as `Exec`/`Query`
/// wrapping a transport-level `sqlx::Error`.
fn database_is_unreachable(error: &sea_orm::DbErr) -> bool {
    use sea_orm::DbErr;

    match error {
        DbErr::Conn(_) | DbErr::ConnectionAcquire(_) => true,
        DbErr::Exec(runtime) | DbErr::Query(runtime) => runtime_error_is_transport(runtime),
        _ => false,
    }
}

fn runtime_error_is_transport(error: &sea_orm::RuntimeErr) -> bool {
    use sea_orm::{RuntimeErr, SqlxError};

    match error {
        RuntimeErr::SqlxError(error) => matches!(
            &**error,
            SqlxError::Io(_)
                | SqlxError::Tls(_)
                | SqlxError::PoolTimedOut
                | SqlxError::PoolClosed
                | SqlxError::WorkerCrashed
        ),
        // Generated inside SeaORM (e.g. "Disconnected"), not by the driver.
        RuntimeErr::Internal(message) => message.contains("Disconnected"),
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let code = match self {
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::BadRequest => StatusCode::BAD_REQUEST,
            ApiError::Conflict => StatusCode::CONFLICT,
            ApiError::Gone => StatusCode::GONE,
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
            (ApiError::Conflict, StatusCode::CONFLICT),
            (ApiError::Gone, StatusCode::GONE),
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
    fn query_errors_fold_to_opaque_internal() {
        // A genuine query/logic fault must surface as a leak-free 500, never a
        // detailed body — and never a 503, which would tell a client to retry
        // something that can only keep failing.
        for error in [
            sea_orm::DbErr::RecordNotFound("missing".to_owned()),
            sea_orm::DbErr::Custom("boom".to_owned()),
            sea_orm::DbErr::Json("bad column".to_owned()),
            sea_orm::DbErr::Query(sea_orm::RuntimeErr::Internal("syntax error".to_owned())),
        ] {
            let err: ApiError = error.into();
            assert!(matches!(err, ApiError::Internal));
            assert_eq!(
                err.into_response().status(),
                StatusCode::INTERNAL_SERVER_ERROR
            );
        }
    }

    #[test]
    fn connection_level_errors_become_503_service_unavailable() {
        // PROTOCOL.md §8: "503 | Database unavailable". These are exactly the
        // variants sea-orm produces when Postgres is unreachable or the pool
        // cannot hand out a connection.
        for error in [
            sea_orm::DbErr::ConnectionAcquire(sea_orm::ConnAcquireErr::Timeout),
            sea_orm::DbErr::ConnectionAcquire(sea_orm::ConnAcquireErr::ConnectionClosed),
            sea_orm::DbErr::Conn(sea_orm::RuntimeErr::Internal("Disconnected".to_owned())),
            sea_orm::DbErr::Conn(sea_orm::RuntimeErr::SqlxError(
                sea_orm::SqlxError::PoolClosed.into(),
            )),
            sea_orm::DbErr::Exec(sea_orm::RuntimeErr::SqlxError(
                sea_orm::SqlxError::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset))
                    .into(),
            )),
            sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(
                sea_orm::SqlxError::PoolTimedOut.into(),
            )),
        ] {
            let err: ApiError = error.into();
            assert!(matches!(err, ApiError::Unavailable));
            let response = err.into_response();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        }
    }
}
