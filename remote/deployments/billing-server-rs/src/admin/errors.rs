//! Error sanitization for the admin UI.
//!
//! Raw `AppError`/`sqlx::Error` messages can include schema names, SQL
//! fragments, file paths, and other internal detail that is fine in
//! server logs but should not be rendered into an operator's browser tab.
//! [`sanitized`] keeps a stable user-facing label per error class and
//! emits the full error via `tracing::warn!` so the operator can correlate
//! via a log query.

use maud::Markup;

use crate::error::AppError;

use super::layout::flash_error;

/// Render a user-safe error flash for an `AppError` and log the full
/// detail. The `label` is the action that failed (e.g. `"list connections"`)
/// and is used both in the UI message and as a structured log field.
pub fn sanitized(label: &str, err: &AppError) -> Markup {
    let kind = classify(err);
    tracing::warn!(
        admin.error.label = label,
        admin.error.kind = kind,
        admin.error.detail = %err,
        "admin: handler error"
    );
    flash_error(format!("{label}: {kind} — check server logs for details"))
}

fn classify(err: &AppError) -> &'static str {
    match err {
        AppError::NotFound(_) => "not found",
        AppError::BadRequest(_) => "bad request",
        AppError::Unauthorized => "unauthorized",
        AppError::Forbidden => "forbidden",
        AppError::Conflict(_) => "conflict",
        AppError::LedgerInvariant(_) => "ledger invariant violation",
        AppError::Provider { .. } => "provider error",
        AppError::ProviderRateLimited { .. } => "provider rate limited",
        AppError::Crypto(_) => "internal crypto error",
        AppError::Database(_) => "database error",
        AppError::Other(_) => "internal error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_covers_each_variant() {
        let cases: Vec<(AppError, &'static str)> = vec![
            (AppError::NotFound("x".into()), "not found"),
            (AppError::BadRequest("x".into()), "bad request"),
            (AppError::Unauthorized, "unauthorized"),
            (AppError::Forbidden, "forbidden"),
            (AppError::Conflict("x".into()), "conflict"),
            (AppError::LedgerInvariant("x".into()), "ledger invariant violation"),
            (
                AppError::Provider {
                    provider: "stripe".into(),
                    message: "boom".into(),
                },
                "provider error",
            ),
            (
                AppError::ProviderRateLimited {
                    provider: "stripe".into(),
                    retry_after_seconds: 1,
                    message: "boom".into(),
                },
                "provider rate limited",
            ),
            (AppError::Crypto("x".into()), "internal crypto error"),
            (AppError::Other(anyhow::anyhow!("x")), "internal error"),
        ];
        for (err, want) in cases {
            assert_eq!(classify(&err), want, "{err:?}");
        }
    }

    #[test]
    fn sanitized_does_not_leak_raw_error_text_into_html() {
        let raw_secret = "duplicate key value violates unique constraint \"users_pkey\" \
                          DETAIL: Key (id)=(abc) already exists.";
        let err = AppError::Conflict(raw_secret.into());
        let html = sanitized("create user", &err).into_string();
        assert!(html.contains("create user"));
        assert!(html.contains("conflict"));
        assert!(!html.contains(raw_secret));
        assert!(!html.contains("users_pkey"));
        assert!(!html.contains("DETAIL"));
    }
}
