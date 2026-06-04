//! GitHub webhook signature verification.
//!
//! GitHub signs webhook bodies using HMAC-SHA256 and the shared secret you
//! register on the webhook. The hex digest is sent in the `X-Hub-Signature-256`
//! header, prefixed with `sha256=`.
//!
//! See <https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries>.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Verifies the given `X-Hub-Signature-256` header value (e.g. `sha256=...`)
/// against the HMAC-SHA256 of `body` keyed by `secret`. Comparison is
/// constant-time to avoid timing oracles.
pub fn verify_github_signature(secret: &[u8], body: &[u8], header_value: &str) -> bool {
    let Some(hex_digest) = header_value.strip_prefix("sha256=") else {
        return false;
    };

    let Ok(provided) = hex::decode(hex_digest) else {
        return false;
    };

    let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    let expected = mac.finalize().into_bytes();

    // HMAC-SHA256 output is always 32 bytes; length-mismatch must short-circuit
    // before the constant-time compare so we don't accidentally treat shorter
    // hex strings as valid.
    if provided.len() != expected.len() {
        return false;
    }

    provided.ct_eq(&expected).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(secret: &[u8], body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret).unwrap();
        mac.update(body);
        let digest = mac.finalize().into_bytes();
        format!("sha256={}", hex::encode(digest))
    }

    #[test]
    fn accepts_valid_signature() {
        let secret = b"It's a Secret to Everybody";
        let body = b"Hello, World!";
        let header = sign(secret, body);
        assert!(verify_github_signature(secret, body, &header));
    }

    #[test]
    fn rejects_wrong_secret() {
        let body = b"Hello, World!";
        let header = sign(b"right", body);
        assert!(!verify_github_signature(b"wrong", body, &header));
    }

    #[test]
    fn rejects_tampered_body() {
        let secret = b"shh";
        let header = sign(secret, b"original");
        assert!(!verify_github_signature(secret, b"tampered", &header));
    }

    #[test]
    fn rejects_wrong_prefix() {
        let secret = b"shh";
        let body = b"x";
        let mut mac = HmacSha256::new_from_slice(secret).unwrap();
        mac.update(body);
        let digest = hex::encode(mac.finalize().into_bytes());
        assert!(!verify_github_signature(
            secret,
            body,
            &format!("sha1={digest}")
        ));
        assert!(!verify_github_signature(secret, body, &digest));
    }

    #[test]
    fn rejects_invalid_hex() {
        let secret = b"shh";
        let body = b"x";
        assert!(!verify_github_signature(secret, body, "sha256=zzzz"));
    }

    #[test]
    fn rejects_short_digest() {
        let secret = b"shh";
        let body = b"x";
        assert!(!verify_github_signature(secret, body, "sha256=ab"));
    }
}
