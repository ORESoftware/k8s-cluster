//! Adyen — global card acquiring + payments platform (observer-mode).
//!
//! Connection: API key (`X-API-Key` header) plus a merchant account and an
//! environment-specific API base. Adyen issues a distinct live endpoint
//! prefix per merchant, so the live base is supplied by the tenant; sandbox
//! ("test") uses the shared checkout-test host.
//!
//! Webhook model: Adyen POSTs batched **notification items**. Each item is
//! signed with an HMAC-SHA256 over a `:`-joined field string, keyed by the
//! merchant's HMAC key (hex-encoded in the Adyen dashboard) and delivered
//! base64 in `additionalData.hmacSignature`. We verify that exact scheme —
//! see [`verify_item_signature`] and [`signing_string`].
//!
//! Sync surface is intentionally stubbed in this pass (maturity = Stub): the
//! connection, credential validation, and webhook verification are real;
//! programmatic report pulls land when a tenant contract maps cleanly to
//! postings.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fmt;

use crate::error::{AppError, AppResult};
use crate::providers::amount::constant_time_eq_str;

#[derive(Clone, Serialize, Deserialize)]
pub struct AdyenCredential {
    /// Adyen API key (`X-API-Key`). High-entropy secret.
    pub api_key: String,
    /// Merchant account code (e.g. `AcmeCorpECOM`).
    pub merchant_account: String,
    /// "production" | "sandbox".
    #[serde(default = "default_env")]
    pub environment: String,
    /// Live endpoint base for this merchant (required in production; Adyen
    /// gives each merchant a unique prefix). Ignored in sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_base_url: Option<String>,
    /// HMAC key (hex) from the Adyen webhook config, used to verify
    /// notification signatures. Optional until webhooks are configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac_key_hex: Option<String>,
}

fn default_env() -> String {
    "sandbox".to_string()
}

// Never let the API key / HMAC key reach a log line.
impl fmt::Debug for AdyenCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdyenCredential")
            .field("api_key", &"<redacted>")
            .field("merchant_account", &self.merchant_account)
            .field("environment", &self.environment)
            .field("api_base_url", &self.api_base_url)
            .field("hmac_key_hex", &self.hmac_key_hex.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl AdyenCredential {
    pub fn is_production(&self) -> bool {
        self.environment.eq_ignore_ascii_case("production")
            || self.environment.eq_ignore_ascii_case("live")
    }
}

// =========================================================================
// Wire types (minimal — webhook notification envelope)
// =========================================================================

/// One Adyen notification item's signed fields. Extracted from
/// `notificationItems[].NotificationRequestItem`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AdyenNotificationItem {
    #[serde(rename = "pspReference", default)]
    pub psp_reference: String,
    #[serde(rename = "originalReference", default)]
    pub original_reference: String,
    #[serde(rename = "merchantAccountCode", default)]
    pub merchant_account_code: String,
    #[serde(rename = "merchantReference", default)]
    pub merchant_reference: String,
    #[serde(rename = "eventCode", default)]
    pub event_code: String,
    #[serde(default)]
    pub success: String,
    #[serde(default)]
    pub amount: Option<AdyenAmount>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct AdyenAmount {
    #[serde(default)]
    pub value: i64,
    #[serde(default)]
    pub currency: String,
}

impl AdyenNotificationItem {
    fn amount_value(&self) -> String {
        self.amount.as_ref().map(|a| a.value).unwrap_or(0).to_string()
    }
    fn amount_currency(&self) -> &str {
        self.amount.as_ref().map(|a| a.currency.as_str()).unwrap_or("")
    }
}

/// Build the exact `:`-joined string Adyen signs. Order is fixed by Adyen and
/// must not change: pspReference, originalReference, merchantAccountCode,
/// merchantReference, amount.value, amount.currency, eventCode, success.
/// Any literal `:` or `\` in a field is backslash-escaped per Adyen's spec.
pub fn signing_string(item: &AdyenNotificationItem) -> String {
    let parts = [
        item.psp_reference.as_str(),
        item.original_reference.as_str(),
        item.merchant_account_code.as_str(),
        item.merchant_reference.as_str(),
        &item.amount_value(),
        item.amount_currency(),
        item.event_code.as_str(),
        item.success.as_str(),
    ];
    parts
        .iter()
        .map(|p| p.replace('\\', "\\\\").replace(':', "\\:"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Verify one notification item's `additionalData.hmacSignature` against the
/// merchant HMAC key (hex). Constant-time compare.
pub fn verify_item_signature(
    item: &AdyenNotificationItem,
    provided_sig_b64: &str,
    hmac_key_hex: &str,
) -> AppResult<()> {
    let key = hex::decode(hmac_key_hex.trim())
        .map_err(|e| AppError::Crypto(format!("adyen hmac key not hex: {e}")))?;
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&key)
        .map_err(|e| AppError::Crypto(format!("hmac init: {e}")))?;
    Mac::update(&mut mac, signing_string(item).as_bytes());
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

    fn sample_item() -> AdyenNotificationItem {
        AdyenNotificationItem {
            psp_reference: "7914073381342284".into(),
            original_reference: String::new(),
            merchant_account_code: "AcmeCorpECOM".into(),
            merchant_reference: "order-123".into(),
            event_code: "AUTHORISATION".into(),
            success: "true".into(),
            amount: Some(AdyenAmount {
                value: 1999,
                currency: "USD".into(),
            }),
        }
    }

    #[test]
    fn signing_string_is_fixed_order_and_colon_joined() {
        let s = signing_string(&sample_item());
        assert_eq!(
            s,
            "7914073381342284::AcmeCorpECOM:order-123:1999:USD:AUTHORISATION:true"
        );
    }

    #[test]
    fn signing_string_escapes_colons_and_backslashes() {
        let mut item = sample_item();
        item.merchant_reference = "a:b\\c".into();
        let s = signing_string(&item);
        assert!(s.contains("a\\:b\\\\c"), "got: {s}");
    }

    #[test]
    fn verifies_adyen_item_hmac_roundtrip() {
        let item = sample_item();
        // Key is hex per Adyen's dashboard format.
        let key_hex = "deadbeefcafef00d";
        let key = hex::decode(key_hex).unwrap();
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&key).unwrap();
        Mac::update(&mut mac, signing_string(&item).as_bytes());
        let sig = B64.encode(Mac::finalize(mac).into_bytes());
        verify_item_signature(&item, &sig, key_hex).unwrap();
    }

    #[test]
    fn rejects_bad_adyen_hmac() {
        let item = sample_item();
        let err = verify_item_signature(&item, "AAAA", "deadbeef").unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[test]
    fn rejects_non_hex_key() {
        let item = sample_item();
        let err = verify_item_signature(&item, "AAAA", "nothex!!").unwrap_err();
        assert!(matches!(err, AppError::Crypto(_)));
    }

    #[test]
    fn redacted_debug_hides_secrets() {
        let cred = AdyenCredential {
            api_key: "AQE-secret".into(),
            merchant_account: "AcmeCorpECOM".into(),
            environment: "sandbox".into(),
            api_base_url: None,
            hmac_key_hex: Some("deadbeef".into()),
        };
        let dbg = format!("{cred:?}");
        assert!(!dbg.contains("AQE-secret"));
        assert!(!dbg.contains("deadbeef"));
        assert!(dbg.contains("AcmeCorpECOM"));
    }
}
