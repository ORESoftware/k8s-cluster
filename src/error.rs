use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

const MAX_PUBLIC_MESSAGE_CHARS: usize = 512;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("ledger invariant violated: {0}")]
    LedgerInvariant(String),

    #[error("provider error ({provider}): {message}")]
    Provider { provider: String, message: String },

    #[error("provider rate limited ({provider}): retry after {retry_after_seconds}s: {message}")]
    ProviderRateLimited {
        provider: String,
        retry_after_seconds: i64,
        message: String,
    },

    #[error("crypto error: {0}")]
    Crypto(String),

    #[error("database error: {0}")]
    Database(#[from] sea_orm::DbErr),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Serialize)]
struct ErrBody<'a> {
    error: &'a str,
    message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let retry_after = self.retry_after_seconds();
        let message = self.client_message();
        let (status, code) = match &self {
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            AppError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            AppError::LedgerInvariant(_) => {
                tracing::warn!("ledger invariant rejected a request");
                (StatusCode::UNPROCESSABLE_ENTITY, "ledger_invariant")
            }
            AppError::Provider { provider, .. } => {
                tracing::error!(provider = %provider, "upstream provider request failed");
                (StatusCode::BAD_GATEWAY, "provider_error")
            }
            AppError::ProviderRateLimited {
                provider,
                retry_after_seconds,
                ..
            } => {
                tracing::warn!(
                    provider = %provider,
                    retry_after_seconds = *retry_after_seconds,
                    "upstream provider rate limited a request"
                );
                (StatusCode::TOO_MANY_REQUESTS, "provider_rate_limited")
            }
            AppError::Crypto(_) => {
                tracing::error!("cryptographic operation failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "crypto_error")
            }
            AppError::Database(_) => {
                // Database errors may contain SQL text or bound values. Keep the
                // category observable without copying the raw error into logs or
                // the HTTP response.
                tracing::error!("database operation failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "database_error")
            }
            AppError::Other(_) => {
                tracing::error!("internal operation failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
        };

        let mut response = (
            status,
            Json(ErrBody {
                error: code,
                message,
            }),
        )
            .into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        );
        if let Some(seconds) = retry_after
            && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

impl AppError {
    pub fn retry_after_seconds(&self) -> Option<i64> {
        match self {
            AppError::ProviderRateLimited {
                retry_after_seconds,
                ..
            } => Some((*retry_after_seconds).clamp(1, 3600)),
            _ => None,
        }
    }

    fn client_message(&self) -> String {
        match self {
            AppError::NotFound(message) => public_message("not found", message),
            AppError::BadRequest(message) => public_message("bad request", message),
            AppError::Unauthorized => "authentication required".to_owned(),
            AppError::Forbidden => "forbidden".to_owned(),
            AppError::Conflict(message) => public_message("conflict", message),
            AppError::LedgerInvariant(_) => "ledger invariant rejected the operation".to_owned(),
            AppError::Provider { .. } => "upstream provider request failed".to_owned(),
            AppError::ProviderRateLimited { .. } => {
                "upstream provider rate limited the request".to_owned()
            }
            AppError::Crypto(_) => "cryptographic operation failed".to_owned(),
            AppError::Database(_) | AppError::Other(_) => "internal server error".to_owned(),
        }
    }
}

fn public_message(prefix: &str, detail: &str) -> String {
    let detail = detail
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_PUBLIC_MESSAGE_CHARS)
        .collect::<String>();
    if detail.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}: {detail}")
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use serde_json::Value;

    async fn response_json(error: AppError) -> (StatusCode, axum::http::HeaderMap, Value) {
        let response = error.into_response();
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let json = serde_json::from_slice(&body).unwrap();
        (status, headers, json)
    }

    #[tokio::test]
    async fn internal_error_details_never_reach_the_client() {
        let cases = [
            (
                AppError::LedgerInvariant("ledger-secret-123".to_owned()),
                "ledger-secret-123",
                "ledger invariant rejected the operation",
            ),
            (
                AppError::Provider {
                    provider: "stripe".to_owned(),
                    message: "provider-secret-123".to_owned(),
                },
                "provider-secret-123",
                "upstream provider request failed",
            ),
            (
                AppError::Crypto("crypto-secret-123".to_owned()),
                "crypto-secret-123",
                "cryptographic operation failed",
            ),
            (
                AppError::Database(sea_orm::DbErr::Custom("database-secret-123".to_owned())),
                "database-secret-123",
                "internal server error",
            ),
            (
                AppError::Other(anyhow::anyhow!("internal-secret-123")),
                "internal-secret-123",
                "internal server error",
            ),
        ];

        for (error, secret, expected_message) in cases {
            let (_, headers, body) = response_json(error).await;
            let rendered = body.to_string();
            assert!(!rendered.contains(secret));
            assert_eq!(body["message"], expected_message);
            assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
        }
    }

    #[tokio::test]
    async fn provider_rate_limit_is_redacted_and_retry_after_is_clamped() {
        let (status, headers, body) = response_json(AppError::ProviderRateLimited {
            provider: "stripe".to_owned(),
            retry_after_seconds: 99_999,
            message: "upstream-secret-123".to_owned(),
        })
        .await;

        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(headers.get(header::RETRY_AFTER).unwrap(), "3600");
        assert_eq!(body["message"], "upstream provider rate limited the request");
        assert!(!body.to_string().contains("upstream-secret-123"));
    }

    #[tokio::test]
    async fn public_validation_messages_are_control_free_and_bounded() {
        let detail = format!("line-one\n{}\0", "x".repeat(MAX_PUBLIC_MESSAGE_CHARS + 100));
        let (_, headers, body) = response_json(AppError::BadRequest(detail)).await;
        let message = body["message"].as_str().unwrap();

        assert!(message.starts_with("bad request: line-one"));
        assert!(!message.contains('\n'));
        assert!(!message.contains('\0'));
        assert!(message.chars().count() <= "bad request: ".chars().count() + MAX_PUBLIC_MESSAGE_CHARS);
        assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    }
}
