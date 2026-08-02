//! Service-local device sync tokens.
//!
//! Human registration, login, and access-token verification are delegated to
//! github.com/shared-auth. After identity enrollment this service issues an
//! independent random token per device and stores only its SHA-256 digest.

use crate::entity::device;
use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::HeaderMap;
use base64::Engine;
use rand::RngCore;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

/// An authenticated device, resolved from a bearer token.
#[derive(Debug, Clone, Copy)]
pub struct AuthedDevice {
    pub account_id: Uuid,
    pub device_id: Uuid,
}

/// Authenticate from the request *head*, before any body is read.
///
/// This is the whole point of implementing [`FromRequestParts`] rather than
/// calling [`authenticate`] inside a handler: axum runs every
/// `FromRequestParts` extractor in declaration order and the single
/// `FromRequest` extractor (the body) last. Naming `AuthedDevice` before
/// `JsonBody<T>` in a handler signature therefore makes the credential check
/// strictly precede JSON deserialization, so an unauthenticated caller gets a
/// bare 401 instead of a body-shape error that enumerates the wire type — and
/// cannot force a multi-megabyte JSON parse per anonymous request.
impl FromRequestParts<AppState> for AuthedDevice {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        authenticate(state.database(), &parts.headers).await
    }
}

/// Generate a fresh bearer token (returned to the client once) and its stored
/// SHA-256 hex digest.
pub fn issue_token() -> (String, String) {
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    let hash = token_hash(&token);
    (token, hash)
}

pub fn token_hash(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

/// Extract the raw bearer token from an `Authorization: Bearer <token>` header.
///
/// A request carrying the header more than once is refused outright.
/// `Authorization` is a singleton field (RFC 9110 §11.6.2), and RFC 9110 §5.3
/// says a recipient of repeated field lines for a singleton field must either
/// combine them — meaningless for a credential — or treat the message as
/// malformed. `HeaderMap::get` would silently return the FIRST of them, which is
/// the dangerous third option: an intermediary that inspects or authorizes on
/// the LAST line (proxies and WAFs differ) would be reasoning about a different
/// credential than the one this service acts on, which is a standard
/// request-smuggling / authorization-bypass primitive. Ambiguous credential,
/// no credential: 401.
pub fn bearer(headers: &HeaderMap) -> Result<&str, ApiError> {
    let mut values = headers.get_all(axum::http::header::AUTHORIZATION).iter();
    let value = values.next().ok_or(ApiError::Unauthorized)?;
    if values.next().is_some() {
        return Err(ApiError::Unauthorized);
    }
    value
        .to_str()
        .ok()
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)
}

/// Resolve the `Authorization: Bearer <token>` header to an [`AuthedDevice`],
/// rejecting revoked or unknown tokens. Constant-ish: lookup is by token hash.
/// Also stamps the device's `last_seen_at` so an owner can spot stale devices;
/// the update is best-effort and never fails the request.
#[tracing::instrument(
    name = "auth.device_token.verify",
    skip_all,
    fields(auth.system = "threefa-device-token")
)]
pub async fn authenticate(
    db: &DatabaseConnection,
    headers: &HeaderMap,
) -> Result<AuthedDevice, ApiError> {
    let token = bearer(headers)?;

    let hash = token_hash(token);
    let row = device::Entity::find()
        .filter(device::Column::SyncTokenHash.eq(hash))
        .filter(device::Column::Revoked.eq(false))
        .one(db)
        .await?;

    let device = row.ok_or(ApiError::Unauthorized)?;
    let authed = AuthedDevice {
        account_id: device.account_id,
        device_id: device.id,
    };

    let mut active: device::ActiveModel = device.into();
    active.last_seen_at = Set(Some(OffsetDateTime::now_utc()));
    if let Err(error) = active.update(db).await {
        tracing::warn!(
            error = %error,
            "failed to update device last_seen_at"
        );
    }

    Ok(authed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_hash_is_stable_and_distinct() {
        let (tok, hash) = issue_token();
        assert_eq!(token_hash(&tok), hash);
        let (tok2, _) = issue_token();
        assert_ne!(tok, tok2);
    }

    #[test]
    fn token_hash_matches_known_sha256_vectors() {
        // SHA-256("") and SHA-256("abc") — pins the digest algorithm so a stored
        // sync_token_hash never silently changes meaning.
        assert_eq!(
            token_hash(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            token_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_single_bearer_header_is_read_and_a_repeated_one_is_refused() {
        use axum::http::header::AUTHORIZATION;
        use axum::http::HeaderValue;

        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer alpha"));
        assert_eq!(bearer(&headers).unwrap(), "alpha");

        // Two lines make the credential ambiguous: which one wins must not be
        // decided by header order, so neither does.
        headers.append(AUTHORIZATION, HeaderValue::from_static("Bearer beta"));
        assert!(matches!(bearer(&headers), Err(ApiError::Unauthorized)));

        // Including the case an intermediary is most likely to disagree on: a
        // junk line in front of a good credential, and the reverse.
        for pair in [["Bearer alpha", "garbage"], ["garbage", "Bearer alpha"]] {
            let mut headers = HeaderMap::new();
            for value in pair {
                headers.append(AUTHORIZATION, HeaderValue::from_str(value).unwrap());
            }
            assert!(matches!(bearer(&headers), Err(ApiError::Unauthorized)));
        }

        // No header at all is the same answer, so the two are not distinguishable.
        assert!(matches!(
            bearer(&HeaderMap::new()),
            Err(ApiError::Unauthorized)
        ));
    }

    #[test]
    fn issued_token_is_urlsafe_base64_of_32_bytes() {
        let (tok, hash) = issue_token();
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&tok)
            .expect("token must be url-safe base64 without padding");
        assert_eq!(raw.len(), 32);
        // Stored digest is lowercase hex of a SHA-256 (64 chars).
        assert_eq!(hash.len(), 64);
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(hash, hash.to_lowercase());
    }
}
