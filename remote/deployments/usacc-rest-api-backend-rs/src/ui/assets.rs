//! Vendored client-side assets served under `{base}/app/static/`.
//!
//! HTMX is bundled into the binary rather than pulled from a CDN so the
//! console renders with no outbound network, ships its JS in our own
//! release artifact (no CDN-compromise injection surface), and can run
//! under a strict `script-src 'self'` CSP.
//!
//! The SRI hash is pinned in [`HTMX_INTEGRITY`] and recomputed from the
//! embedded bytes at startup by [`verify_integrity`], which panics on
//! drift so a sloppy vendor bump fails the build rather than the browser.

use axum::extract::Path;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use sha2::{Digest, Sha384};

/// Vendored htmx 2.0.10 (jsdelivr build). Bumping the file MUST be paired
/// with a matching update to [`HTMX_INTEGRITY`] (verified at startup).
pub const HTMX_BYTES: &[u8] = include_bytes!("./vendor/htmx.min.js");

/// SRI hash of [`HTMX_BYTES`] in `sha384-<b64>` form — the hash published
/// by the htmx project for the 2.0.10 release.
pub const HTMX_INTEGRITY: &str =
    "sha384-H5SrcfygHmAuTDZphMHqBJLc3FhssKjG7w/CeCpFReSfwBWDTKpkzPP8c+cLsK+V";

/// Relative (un-prefixed) asset path. The URL is pinned to the SRI hash so
/// any byte change forces a fresh URL and lets us advertise an immutable,
/// year-long cache TTL. Callers prefix it with the configured base path.
pub fn htmx_asset_suffix() -> String {
    let suffix = HTMX_INTEGRITY.trim_start_matches("sha384-");
    let token: String = suffix
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(16)
        .collect();
    format!("/app/static/htmx-{token}.js")
}

/// Verify the embedded bytes match the pinned SRI hash. Call once at
/// startup; panics on mismatch so CI catches drift instead of production
/// browsers refusing the script.
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
        "vendored htmx SHA-384 drifted from HTMX_INTEGRITY. Replace \
         src/ui/vendor/htmx.min.js or update HTMX_INTEGRITY (and the SRI \
         attribute in ui::layout) together."
    );
}

/// Handler bound to `/app/static/{file}`. Restricted to the single
/// hash-pinned filename we emit so the route can't be probed for arbitrary
/// files.
pub async fn serve(Path(file): Path<String>) -> Response {
    if !is_htmx_filename(&file) {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        HTMX_BYTES,
    )
        .into_response()
}

/// Accept only the hash-pinned htmx filename we emit.
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
    fn asset_suffix_is_hash_pinned_and_safe() {
        let p = htmx_asset_suffix();
        assert!(p.starts_with("/app/static/htmx-"));
        assert!(p.ends_with(".js"));
        let body = p
            .trim_start_matches("/app/static/htmx-")
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
    }
}
