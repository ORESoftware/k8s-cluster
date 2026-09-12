//! Bridge.xyz — stablecoin (USDC) orchestration.
//!
//! Bridge.xyz (acquired by Stripe in October 2024) provides USDC
//! issuance, redemption, virtual accounts, and cross-border stablecoin
//! payouts under a regulated MTL framework. Same VASP-license leverage
//! story as Coinflow, but specifically for stablecoin rails.
//!
//! Auth: `Api-Key: <api_key>` header.
//! Base URL: production `https://api.bridge.xyz/v0`, sandbox
//! `https://api.sandbox.bridge.xyz/v0`.
//!
//! Endpoints used:
//!   * `GET /transfers?limit=N&starting_after=<id>` — paginated transfer
//!     list (newest-to-oldest by default; we reverse to post in order)
//!
//! Webhook signature:
//!   `X-Webhook-Signature: t=<ts_ms>,v0=<base64_rsa_sha256>`
//!   Bridge uses RSA-SHA256 PKCS1v15 with a per-account PEM public key
//!   delivered out-of-band. We do timestamp staleness checks (>10 min →
//!   reject) and structure the verifier, but the cryptographic signature
//!   step is deliberately left for a follow-up that adds the `rsa` crate
//!   — same posture as Plaid. Until then, signature_ok is recorded as
//!   `false` and the require_webhook_signatures flag in config controls
//!   whether unverified deliveries are rejected.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::ledger::{AccountKind, Direction, DraftPosting, DraftTransaction};
use crate::money::Currency;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeCredential {
    pub api_key: String,
    pub webhook_secret: Option<String>,
    /// PEM-encoded RSA public key Bridge delivers to the merchant
    /// out-of-band, used for webhook signature verification. Optional
    /// today — when present, future cryptographic verification will
    /// pick it up automatically.
    pub webhook_public_key_pem: Option<String>,
    /// "production" | "sandbox"
    #[serde(default = "default_env")]
    pub environment: String,
}
fn default_env() -> String {
    "production".into()
}

impl BridgeCredential {
    pub fn base_url(&self) -> &'static str {
        if self.environment.eq_ignore_ascii_case("sandbox") {
            "https://api.sandbox.bridge.xyz/v0"
        } else {
            "https://api.bridge.xyz/v0"
        }
    }
}

// --- Wire types -----------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
pub struct BridgeTransfer {
    pub id: String,
    pub state: Option<String>,
    pub amount: String,
    pub currency: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub customer_id: Option<String>,
    pub on_behalf_of: Option<String>,
    pub source: Option<serde_json::Value>,
    pub destination: Option<serde_json::Value>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Deserialize)]
struct TransfersResponse {
    data: Vec<BridgeTransfer>,
    #[serde(default)]
    count: Option<i64>,
}

// --- API client -----------------------------------------------------------

pub struct BridgeApi {
    cred: BridgeCredential,
    http: reqwest::Client,
    base_url: String,
}

impl BridgeApi {
    pub fn new(cred: BridgeCredential) -> Self {
        let base_url = cred.base_url().to_string();
        Self {
            cred,
            http: reqwest::Client::new(),
            base_url,
        }
    }

    #[cfg(test)]
    pub fn with_base_url_for_tests(cred: BridgeCredential, base_url: String) -> Self {
        Self {
            cred,
            http: reqwest::Client::new(),
            base_url,
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    /// `GET /transfers?limit=N&starting_after=<id>` — Bridge returns
    /// newest-to-oldest by default. We pass back `next_cursor = last
    /// transfer id` so the next page starts after it.
    pub async fn list_transfers(
        &self,
        limit: u32,
        starting_after: Option<&str>,
    ) -> AppResult<(Vec<BridgeTransfer>, Option<String>)> {
        let mut params: Vec<(&str, String)> = vec![("limit", limit.to_string())];
        if let Some(c) = starting_after {
            params.push(("starting_after", c.to_string()));
        }
        let qs = serde_urlencoded::to_string(&params).map_err(|e| AppError::Provider {
            provider: "bridge".into(),
            message: format!("encode query: {e}"),
        })?;
        let url = format!("{}/transfers?{qs}", self.base_url());

        let resp = self
            .http
            .get(&url)
            .header("Api-Key", &self.cred.api_key)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "bridge".into(),
                message: format!("transfers HTTP: {e}"),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "bridge".into(),
            message: format!("transfers body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "bridge".into(),
                message: format!("transfers {status}: {}", String::from_utf8_lossy(&bytes)),
            });
        }
        let parsed: TransfersResponse =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "bridge".into(),
                message: format!("transfers decode: {e}"),
            })?;

        let next = parsed.data.last().map(|t| t.id.clone());
        Ok((parsed.data, next))
    }
}

// --- Webhook signature header parsing -------------------------------------

/// Result of parsing the `X-Webhook-Signature: t=<ts_ms>,v0=<sig>` header.
pub struct BridgeSignatureHeader {
    pub timestamp_ms: i64,
    pub signature_b64: String,
}

pub fn parse_signature_header(header: &str) -> AppResult<BridgeSignatureHeader> {
    let mut ts: Option<i64> = None;
    let mut sig: Option<String> = None;
    for part in header.split(',') {
        let p = part.trim();
        if let Some(rest) = p.strip_prefix("t=") {
            ts = rest.parse().ok();
        } else if let Some(rest) = p.strip_prefix("v0=") {
            sig = Some(rest.to_string());
        }
    }
    match (ts, sig) {
        (Some(timestamp_ms), Some(signature_b64)) => Ok(BridgeSignatureHeader {
            timestamp_ms,
            signature_b64,
        }),
        _ => Err(AppError::BadRequest(
            "bridge signature header must contain both t= and v0=".into(),
        )),
    }
}

/// Staleness check Bridge requires (timestamp not older than 10 minutes
/// in either direction). Returns Unauthorized on stale.
pub fn validate_timestamp_freshness(
    parsed: &BridgeSignatureHeader,
    max_age_seconds: i64,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let now_ms = now.timestamp_millis();
    let max_age_ms = max_age_seconds.saturating_mul(1000);
    if (now_ms - parsed.timestamp_ms).abs() > max_age_ms {
        return Err(AppError::Unauthorized);
    }
    Ok(())
}

/// Full RSA-SHA256 PKCS1v15 verification of a Bridge webhook.
///
/// Bridge signs `{timestamp_ms}.{raw_body}` with an RSA private key.
/// Each merchant has their own per-account public key (PEM) which we
/// receive via `BridgeCredential.webhook_public_key_pem`. The signature
/// in the `X-Webhook-Signature` header is base64-encoded.
///
/// Returns Ok(()) on success, Err(Unauthorized) on signature mismatch,
/// Err(Crypto) on a malformed PEM / signature.
pub fn verify_signature_rsa(
    raw_body: &[u8],
    parsed: &BridgeSignatureHeader,
    public_key_pem: &str,
) -> AppResult<()> {
    use base64::Engine as _;
    use rsa::RsaPublicKey;
    use rsa::pkcs1v15::{Signature, VerifyingKey};
    use rsa::pkcs8::DecodePublicKey;
    use rsa::sha2::Sha256;
    use rsa::signature::Verifier;

    let pubkey = RsaPublicKey::from_public_key_pem(public_key_pem.trim())
        .map_err(|e| AppError::Crypto(format!("bridge pubkey pem: {e}")))?;
    let verifying_key = VerifyingKey::<Sha256>::new(pubkey);

    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(parsed.signature_b64.as_bytes())
        .map_err(|e| AppError::Crypto(format!("bridge sig b64: {e}")))?;
    let signature = Signature::try_from(sig_bytes.as_slice())
        .map_err(|e| AppError::Crypto(format!("bridge sig: {e}")))?;

    let mut signed_payload = Vec::with_capacity(raw_body.len() + 24);
    signed_payload.extend_from_slice(parsed.timestamp_ms.to_string().as_bytes());
    signed_payload.push(b'.');
    signed_payload.extend_from_slice(raw_body);

    verifying_key
        .verify(&signed_payload, &signature)
        .map_err(|_| AppError::Unauthorized)
}

// --- Normalization --------------------------------------------------------

#[derive(Clone, Debug)]
pub struct EnsureAccount {
    pub code: String,
    pub kind: AccountKind,
    pub currency: Currency,
}

pub struct NormalizedBridgeTransfer {
    pub draft: DraftTransaction,
    pub accounts_to_ensure: Vec<EnsureAccount>,
    pub recognized: bool,
}

const ACCT_BRIDGE_CLEARING: &str = "clearing/bridge";
const ACCT_BRIDGE_USDC: &str = "asset/bridge/usdc";
const ACCT_BRIDGE_FEES: &str = "expense/fees/bridge";

/// Normalize a Bridge transfer into a balanced draft. We only post for
/// terminal states (`payment_processed`, `payment_submitted`, `funds_received`,
/// `completed`); in-flight states stay observability-only so we don't
/// double-post when the transfer settles later.
///
/// Stablecoin amounts come back as decimal strings — we convert to minor
/// units (USDC is 1:1 with USD for ledger purposes; tenants who want
/// the raw on-chain amount in 6-decimal precision get it via the
/// metadata stash).
pub fn normalize_transfer(
    tr: &BridgeTransfer,
    tenant_id: uuid::Uuid,
) -> AppResult<NormalizedBridgeTransfer> {
    let terminal_states = ["payment_processed", "funds_received", "completed"];
    let is_terminal = tr
        .state
        .as_deref()
        .map(|s| terminal_states.contains(&s))
        .unwrap_or(false);

    let mut postings: Vec<DraftPosting> = Vec::new();
    let mut accounts: Vec<EnsureAccount> = Vec::new();
    let mut recognized = false;

    let posted_at: DateTime<Utc> = tr
        .updated_at
        .as_deref()
        .or(tr.created_at.as_deref())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    let cur_upper = tr.currency.to_uppercase();
    let ledger_currency = if cur_upper == "USDC" || cur_upper == "USDT" {
        "USD".to_string()
    } else {
        cur_upper.clone()
    };
    let currency = Currency::new(&ledger_currency).map_err(|e| AppError::Provider {
        provider: "bridge".into(),
        message: format!("unknown currency {ledger_currency}: {e}"),
    })?;
    let cur = currency.as_str().to_string();
    let amount_minor = parse_amount_to_minor(&tr.amount)?;

    let meta = serde_json::json!({
        "bridge_transfer_id": tr.id,
        "bridge_state": tr.state,
        "bridge_currency_raw": tr.currency,
        "bridge_amount_raw": tr.amount,
        "bridge_customer_id": tr.customer_id,
        "bridge_on_behalf_of": tr.on_behalf_of,
        "bridge_source": tr.source,
        "bridge_destination": tr.destination,
    });

    if is_terminal && amount_minor > 0 {
        // Posting template: tenant USDC balance increases (asset) and
        // clearing decreases. For an *outgoing* transfer the direction
        // is inverted. We pick direction from `source`/`destination`
        // structure: if `destination.payment_rail == "bridge_internal"`
        // we treat as inbound (mint); otherwise outbound.
        let outbound = tr
            .destination
            .as_ref()
            .and_then(|v| v.get("payment_rail"))
            .and_then(|v| v.as_str())
            .map(|s| !s.eq_ignore_ascii_case("bridge_internal"))
            .unwrap_or(false);

        let (usdc_dir, clearing_dir) = if outbound {
            (Direction::Credit, Direction::Debit)
        } else {
            (Direction::Debit, Direction::Credit)
        };

        postings.push(DraftPosting {
            account_code: ACCT_BRIDGE_USDC.into(),
            direction: usdc_dir,
            amount_minor,
            currency: cur.clone(),
            source: "bridge".into(),
            source_event_id: tr.id.clone(),
            metadata: meta.clone(),
        });
        accounts.push(EnsureAccount {
            code: ACCT_BRIDGE_USDC.into(),
            kind: AccountKind::Asset,
            currency: currency.clone(),
        });

        postings.push(DraftPosting {
            account_code: ACCT_BRIDGE_CLEARING.into(),
            direction: clearing_dir,
            amount_minor,
            currency: cur.clone(),
            source: "bridge".into(),
            source_event_id: format!("{}:cp", tr.id),
            metadata: meta.clone(),
        });
        accounts.push(EnsureAccount {
            code: ACCT_BRIDGE_CLEARING.into(),
            kind: AccountKind::Asset,
            currency,
        });
        recognized = true;
    }

    // Silence the warning if neither branch fires.
    let _ = ACCT_BRIDGE_FEES;

    let description = Some(format!(
        "bridge transfer {} ({} {}) at {}",
        tr.id,
        tr.amount,
        tr.currency,
        posted_at.format("%Y-%m-%dT%H:%M:%SZ")
    ));

    Ok(NormalizedBridgeTransfer {
        draft: DraftTransaction {
            tenant_id,
            kind: format!(
                "bridge.transfer.{}",
                tr.state.as_deref().unwrap_or("unknown")
            ),
            idempotency_key: format!("bridge:tr:{}", tr.id),
            description,
            metadata: meta,
            postings,
        },
        accounts_to_ensure: accounts,
        recognized,
    })
}

/// Bridge sends amounts as decimal strings like "100.00" or "100.123456".
/// We convert to minor units (cents). For tokens with >2-decimal precision
/// we truncate to 2 — full precision is preserved in metadata.
fn parse_amount_to_minor(amount: &str) -> AppResult<i128> {
    let cleaned = amount.trim().replace([',', '_'], "");
    let parts: Vec<&str> = cleaned.split('.').collect();
    match parts.as_slice() {
        [w] => {
            let whole: i128 = w.parse().map_err(|e| AppError::Provider {
                provider: "bridge".into(),
                message: format!("amount {w}: {e}"),
            })?;
            Ok(whole * 100)
        }
        [w, f] => {
            let whole: i128 = w.parse().map_err(|e| AppError::Provider {
                provider: "bridge".into(),
                message: format!("amount whole {w}: {e}"),
            })?;
            let frac_str = if f.len() >= 2 {
                f[..2].to_string()
            } else {
                format!("{f}{}", "0".repeat(2 - f.len()))
            };
            let frac: i128 = frac_str.parse().map_err(|e| AppError::Provider {
                provider: "bridge".into(),
                message: format!("amount frac {frac_str}: {e}"),
            })?;
            Ok(whole * 100 + frac)
        }
        _ => Err(AppError::Provider {
            provider: "bridge".into(),
            message: format!("malformed amount {amount}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use chrono::Duration;
    // The rsa crate is pinned to rand_core 0.6 and re-exports `OsRng`
    // from that version. The workspace's top-level `rand` is 0.10, so
    // we must use the rsa-flavored RNG to satisfy its trait bounds.
    use rsa::RsaPrivateKey;
    use rsa::pkcs1v15::SigningKey;
    use rsa::pkcs8::EncodePublicKey;
    use rsa::rand_core::OsRng;
    use rsa::sha2::Sha256;
    use rsa::signature::{RandomizedSigner, SignatureEncoding};

    /// Generate a small RSA keypair for tests (1024 bits is fine here
    /// — we're not protecting anything, just exercising the verify path).
    /// We keep the private key in-memory rather than PEM-roundtripping
    /// it so we don't need to pull in the `DecodePrivateKey` trait.
    fn make_test_keypair() -> (RsaPrivateKey, String) {
        let private = RsaPrivateKey::new(&mut OsRng, 1024).unwrap();
        let public_pem = private
            .to_public_key()
            .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap();
        (private, public_pem)
    }

    fn sign_for_bridge(private: &RsaPrivateKey, ts_ms: i64, body: &[u8]) -> String {
        let signing_key = SigningKey::<Sha256>::new(private.clone());
        let mut payload = ts_ms.to_string().into_bytes();
        payload.push(b'.');
        payload.extend_from_slice(body);
        let sig = signing_key.sign_with_rng(&mut OsRng, &payload);
        base64::engine::general_purpose::STANDARD.encode(sig.to_bytes())
    }

    #[test]
    fn parses_signature_header_round_trip() {
        let h = parse_signature_header("t=1716423000000, v0=abc==").unwrap();
        assert_eq!(h.timestamp_ms, 1716423000000);
        assert_eq!(h.signature_b64, "abc==");
    }

    #[test]
    fn parses_signature_header_order_insensitive() {
        let h = parse_signature_header("v0=zz==,t=42").unwrap();
        assert_eq!(h.timestamp_ms, 42);
        assert_eq!(h.signature_b64, "zz==");
    }

    #[test]
    fn rejects_missing_pieces() {
        assert!(parse_signature_header("t=42").is_err());
        assert!(parse_signature_header("v0=abc").is_err());
        assert!(parse_signature_header("").is_err());
    }

    #[test]
    fn freshness_accepts_recent() {
        let now = Utc::now();
        let parsed = BridgeSignatureHeader {
            timestamp_ms: now.timestamp_millis() - 30_000, // 30s ago
            signature_b64: "x".into(),
        };
        assert!(validate_timestamp_freshness(&parsed, 600, now).is_ok());
    }

    #[test]
    fn freshness_rejects_stale() {
        let now = Utc::now();
        let parsed = BridgeSignatureHeader {
            timestamp_ms: (now - Duration::seconds(700)).timestamp_millis(),
            signature_b64: "x".into(),
        };
        let err = validate_timestamp_freshness(&parsed, 600, now).unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[test]
    fn freshness_rejects_future() {
        // Symmetric: a timestamp from the FUTURE outside the tolerance
        // is also rejected (clock-skew attack).
        let now = Utc::now();
        let parsed = BridgeSignatureHeader {
            timestamp_ms: (now + Duration::seconds(700)).timestamp_millis(),
            signature_b64: "x".into(),
        };
        assert!(validate_timestamp_freshness(&parsed, 600, now).is_err());
    }

    #[test]
    fn rsa_verify_accepts_genuine_signature() {
        let (private, pub_pem) = make_test_keypair();
        let body = br#"{"event":"transfer.completed","id":"tr_1"}"#;
        let ts = Utc::now().timestamp_millis();
        let sig_b64 = sign_for_bridge(&private, ts, body);
        let parsed = BridgeSignatureHeader {
            timestamp_ms: ts,
            signature_b64: sig_b64,
        };
        verify_signature_rsa(body, &parsed, &pub_pem).unwrap();
    }

    #[test]
    fn rsa_verify_rejects_tampered_body() {
        let (private, pub_pem) = make_test_keypair();
        let body = br#"{"amount":100}"#;
        let ts = Utc::now().timestamp_millis();
        let sig_b64 = sign_for_bridge(&private, ts, body);
        let parsed = BridgeSignatureHeader {
            timestamp_ms: ts,
            signature_b64: sig_b64,
        };
        // Attacker swaps the body but keeps the signature.
        let tampered = br#"{"amount":99999}"#;
        let err = verify_signature_rsa(tampered, &parsed, &pub_pem).unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[test]
    fn rsa_verify_rejects_tampered_timestamp() {
        let (private, pub_pem) = make_test_keypair();
        let body = br#"{"event":"x"}"#;
        let ts = Utc::now().timestamp_millis();
        let sig_b64 = sign_for_bridge(&private, ts, body);
        // Same signature, but attacker shifts the timestamp — the
        // signed payload doesn't match, so verify must fail.
        let parsed = BridgeSignatureHeader {
            timestamp_ms: ts + 1,
            signature_b64: sig_b64,
        };
        assert!(matches!(
            verify_signature_rsa(body, &parsed, &pub_pem).unwrap_err(),
            AppError::Unauthorized
        ));
    }

    #[test]
    fn rsa_verify_rejects_signature_from_different_keypair() {
        let (_a_priv, pub_a) = make_test_keypair();
        let (b_priv, _b_pub) = make_test_keypair();
        let body = br#"{"event":"x"}"#;
        let ts = Utc::now().timestamp_millis();
        let sig_b64 = sign_for_bridge(&b_priv, ts, body);
        let parsed = BridgeSignatureHeader {
            timestamp_ms: ts,
            signature_b64: sig_b64,
        };
        // Signed with B but verified against A — must fail.
        assert!(matches!(
            verify_signature_rsa(body, &parsed, &pub_a).unwrap_err(),
            AppError::Unauthorized
        ));
    }

    #[test]
    fn rsa_verify_rejects_malformed_pem() {
        let parsed = BridgeSignatureHeader {
            timestamp_ms: 0,
            signature_b64: "AA==".into(),
        };
        let err = verify_signature_rsa(b"x", &parsed, "not a pem at all").unwrap_err();
        assert!(matches!(err, AppError::Crypto(_)));
    }

    #[test]
    fn rsa_verify_rejects_malformed_sig() {
        let (_private, pub_pem) = make_test_keypair();
        let parsed = BridgeSignatureHeader {
            timestamp_ms: 0,
            signature_b64: "@@@not base64@@@".into(),
        };
        let err = verify_signature_rsa(b"x", &parsed, &pub_pem).unwrap_err();
        assert!(matches!(err, AppError::Crypto(_)));
    }
}
