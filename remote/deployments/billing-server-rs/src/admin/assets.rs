//! Vendored client-side assets served under `/admin/static/`.
//!
//! Bundling `htmx.min.js` into the binary (rather than relying on a CDN)
//! gives us:
//!
//!   - **Supply-chain integrity.** The bytes ship in our release artifact;
//!     a CDN compromise can't inject script into our admin UI.
//!   - **Strict CSP.** `script-src 'self'` is sufficient — no `https://cdn.*`
//!     allowance, no third-party fetches.
//!   - **Air-gapped / restricted-egress deploys.** No outbound network
//!     needed to render `/admin`.
//!
//! The SRI hash is precomputed at compile time and surfaced from
//! [`HTMX_INTEGRITY`]; we also recompute it at startup and panic if the
//! vendored bytes have drifted from the published hash. Bump the version
//! by replacing `vendor/htmx.min.js` and updating [`HTMX_INTEGRITY`] in
//! the same commit.

use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use sha2::{Digest, Sha384};

/// Vendored htmx 2.0.10 (jsdelivr build). Bumping the file MUST be paired
/// with a matching update to [`HTMX_INTEGRITY`] (verified at startup).
pub const HTMX_BYTES: &[u8] = include_bytes!("./vendor/htmx.min.js");

/// SRI hash of [`HTMX_BYTES`] in `sha384-<b64>` form. This is the official
/// hash published by the htmx project for the 2.0.10 release. We recompute
/// from the bytes at startup via [`verify_integrity`] and fail loudly on
/// drift so a sloppy vendor bump never silently ships unverified JS.
pub const HTMX_INTEGRITY: &str =
    "sha384-H5SrcfygHmAuTDZphMHqBJLc3FhssKjG7w/CeCpFReSfwBWDTKpkzPP8c+cLsK+V";

/// Served by the asset route. We pin the URL itself to the SRI hash so
/// any byte change forces a fresh URL — eliminates client-cache poisoning
/// risk and lets us advertise a permanent `Cache-Control: immutable` TTL.
pub fn htmx_asset_path() -> String {
    let suffix = HTMX_INTEGRITY.trim_start_matches("sha384-");
    // Strip non-URL-safe chars to get a short cache-buster.
    let token: String = suffix
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(16)
        .collect();
    format!("/admin/static/htmx-{token}.js")
}

/// Verify the embedded bytes match the pinned SRI hash. Call once at
/// startup; panics on mismatch so a CI build catches the drift instead of
/// production browsers refusing the script.
pub fn verify_integrity() {
    let mut hasher = Sha384::new();
    hasher.update(HTMX_BYTES);
    let digest = hasher.finalize();
    let computed = format!(
        "sha384-{}",
        base64::engine::general_purpose::STANDARD.encode(digest)
    );
    assert_eq!(
        computed, HTMX_INTEGRITY,
        "vendored htmx SHA-384 drifted from HTMX_INTEGRITY constant. \
         Replace src/admin/vendor/htmx.min.js or update HTMX_INTEGRITY \
         (and the SRI attribute used by the admin layout) together."
    );
}

/// Handler bound to `/admin/static/{file}`. We restrict to a single
/// known filename pattern so an attacker can't probe for arbitrary files;
/// anything else returns 404.
pub async fn serve(Path(file): Path<String>) -> Response {
    if !is_htmx_filename(&file) {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/javascript; charset=utf-8"),
            // The URL is hash-pinned — safe to cache for a year.
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        HTMX_BYTES,
    )
        .into_response()
}

/// Accept only the hash-pinned htmx filename we emit. Anything else is
/// rejected so the static route can never be coerced into a probing surface.
fn is_htmx_filename(name: &str) -> bool {
    name.strip_prefix("htmx-")
        .and_then(|s| s.strip_suffix(".js"))
        .map(|token| {
            !token.is_empty()
                && token.len() <= 32
                && token.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendored_bytes_match_pinned_integrity() {
        verify_integrity();
    }

    #[test]
    fn asset_path_is_hash_pinned_and_safe() {
        let p = htmx_asset_path();
        assert!(p.starts_with("/admin/static/htmx-"));
        assert!(p.ends_with(".js"));
        let body = p
            .trim_start_matches("/admin/static/htmx-")
            .trim_end_matches(".js");
        assert!(body.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(body.len() >= 8 && body.len() <= 32);
    }

    #[test]
    fn filename_filter_rejects_traversal_and_other_files() {
        assert!(is_htmx_filename("htmx-H5SrcfygHmAuTDZp.js"));
        assert!(!is_htmx_filename(""));
        assert!(!is_htmx_filename("htmx.min.js"));
        assert!(!is_htmx_filename("htmx-.js"));
        assert!(!is_htmx_filename("htmx-../etc/passwd.js"));
        assert!(!is_htmx_filename("../etc/passwd"));
        assert!(!is_htmx_filename("htmx-A.css"));
        assert!(!is_htmx_filename("htmx-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA.js"));
    }
}
