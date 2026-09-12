//! Square — card / point-of-sale / online checkout (observer-mode).
//!
//! Connection: access token (bearer) plus environment. Square exposes a
//! single production host and a single sandbox host (no per-merchant
//! prefix), so the base is derived from `environment` alone.
//!
//! Webhook model: Square POSTs events and signs them with HMAC-SHA256 over
//! the concatenation of the **notification URL** and the raw request body,
//! keyed by the subscription's signature key, delivered base64 in the
//! `x-square-hmacsha256-signature` header. Because the URL participates in
//! the signature, the tenant stores the exact notification URL they
//! registered with Square alongside the credential. See
//! [`verify_webhook_signature`].
//!
//! Sync surface is stubbed in this pass (maturity = Stub): connection,
//! credential validation, and webhook verification are real; programmatic
//! payment/payout pulls land when a tenant contract maps to postings.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fmt;

use crate::error::{AppError, AppResult};
use crate::providers::amount::constant_time_eq_str;

#[derive(Clone, Serialize, Deserialize)]
pub struct SquareCredential {
    /// Square access token (bearer). High-entropy secret.
    pub access_token: String,
    /// "production" | "sandbox".
    #[serde(default = "default_env")]
    pub environment: String,
    /// Merchant id, when the tenant supplies it (used as the connection's
    /// external account id for webhook routing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merchant_id: Option<String>,
    /// Webhook subscription signature key. Optional until webhooks are set up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_signature_key: Option<String>,
    /// The exact notification URL registered with Square's webhook
    /// subscription. Required to verify signatures because Square signs
    /// `url + body`, not the body alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_notification_url: Option<String>,
}

fn default_env() -> String {
    "sandbox".to_string()
}

// Keep the access token / signature key out of logs.
impl fmt::Debug for SquareCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SquareCredential")
            .field("access_token", &"<redacted>")
            .field("environment", &self.environment)
            .field("merchant_id", &self.merchant_id)
            .field(
                "webhook_signature_key",
                &self.webhook_signature_key.as_ref().map(|_| "<redacted>"),
            )
            .field("webhook_notification_url", &self.webhook_notification_url)
            .finish()
    }
}

/// Verify a Square webhook signature. Square computes
/// `base64(HMAC-SHA256(signature_key, notification_url + body))`.
/// Constant-time compare against the `x-square-hmacsha256-signature` header.
pub fn verify_webhook_signature(
    notification_url: &str,
    body: &[u8],
    provided_sig_b64: &str,
    signature_key: &str,
) -> AppResult<()> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(signature_key.as_bytes())
        .map_err(|e| AppError::Crypto(format!("hmac init: {e}")))?;
    Mac::update(&mut mac, notification_url.as_bytes());
    Mac::update(&mut mac, body);
    let expected = B64.encode(Mac::finalize(mac).into_bytes());
    if constant_time_eq_str(provided_sig_b64.trim(), &expected) {
        Ok(())
    } else {
        Err(AppError::Unauthorized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_square_hmac_over_url_plus_body() {
        let url = "https://billing.example.com/v1/webhooks/square";
        let body = br#"{"type":"payment.updated"}"#;
        let key = "sq-sig-key";
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key.as_bytes()).unwrap();
        Mac::update(&mut mac, url.as_bytes());
        Mac::update(&mut mac, body);
        let sig = B64.encode(Mac::finalize(mac).into_bytes());
        verify_webhook_signature(url, body, &sig, key).unwrap();
    }

    #[test]
    fn rejects_when_url_differs() {
        let body = br#"{"type":"payment.updated"}"#;
        let key = "sq-sig-key";
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key.as_bytes()).unwrap();
        Mac::update(&mut mac, b"https://right.example/wh");
        Mac::update(&mut mac, body);
        let sig = B64.encode(Mac::finalize(mac).into_bytes());
        // Same body + key, but the verifier is told a different URL → reject.
        let err = verify_webhook_signature("https://wrong.example/wh", body, &sig, key).unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[test]
    fn redacted_debug_hides_secrets() {
        let cred = SquareCredential {
            access_token: "EAAA-secret".into(),
            environment: "sandbox".into(),
            merchant_id: Some("ML123".into()),
            webhook_signature_key: Some("sig-secret".into()),
            webhook_notification_url: Some("https://x/wh".into()),
        };
        let dbg = format!("{cred:?}");
        assert!(!dbg.contains("EAAA-secret"));
        assert!(!dbg.contains("sig-secret"));
        assert!(dbg.contains("ML123"));
    }
}
