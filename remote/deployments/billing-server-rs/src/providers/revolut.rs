//! Revolut Business — multi-currency accounts, transactions, counterparties.
//!
//! Revolut Business is a UK/EU e-money institution and a Lithuanian
//! specialised bank (Revolut Bank UAB). For tenants who already use them
//! this gives us cross-border GBP/EUR/USD pay-in + payout observation
//! without our tenants needing their own MTL.
//!
//! Auth: we support the **API certificate + access token** path, which
//! Revolut calls "Production API access" — the tenant generates an
//! access token in their Revolut Business app, optionally rotating via
//! refresh token (we store both if provided). We do NOT yet support the
//! full JWT-client-assertion OAuth flow — that's a separate v2 push.
//!
//! Base URLs:
//!   production: https://b2b.revolut.com/api/1.0
//!   sandbox:    https://sandbox-b2b.revolut.com/api/1.0
//!
//! Endpoints we use:
//!   GET /accounts                        — list all tenant accounts (multi-ccy)
//!   GET /transactions?from=<ts>&count=N  — list transactions across all accounts

use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::{AppError, AppResult};
use crate::ledger::{AccountKind, Direction, DraftPosting, DraftTransaction};
use crate::money::Currency;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RevolutCredential {
    pub access_token: String,
    /// Optional — present when the tenant did OAuth (refresh-token flow).
    pub refresh_token: Option<String>,
    /// "production" | "sandbox".
    #[serde(default = "default_env")]
    pub environment: String,
    /// HMAC secret for webhook signature verification (Revolut-Signature
    /// header, format `v1=<hex>`).
    pub webhook_secret: Option<String>,
}
fn default_env() -> String {
    "production".into()
}

impl RevolutCredential {
    pub fn base_url(&self) -> &'static str {
        if self.environment.eq_ignore_ascii_case("sandbox") {
            "https://sandbox-b2b.revolut.com/api/1.0"
        } else {
            "https://b2b.revolut.com/api/1.0"
        }
    }
}

// --- Wire types -----------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
pub struct RevolutAccount {
    pub id: String,
    pub name: Option<String>,
    pub balance: f64,
    pub currency: String,
    pub state: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RevolutTransaction {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub state: Option<String>,
    pub created_at: Option<String>,
    pub completed_at: Option<String>,
    pub reference: Option<String>,
    pub merchant: Option<serde_json::Value>,
    pub legs: Option<Vec<RevolutLeg>>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RevolutLeg {
    pub leg_id: String,
    pub account_id: String,
    pub counterparty: Option<serde_json::Value>,
    /// Positive for credit to this account, negative for debit.
    pub amount: f64,
    pub currency: String,
    pub description: Option<String>,
    pub balance: Option<f64>,
}

// --- API client -----------------------------------------------------------

pub struct RevolutApi {
    cred: RevolutCredential,
    http: reqwest::Client,
    base_url: String,
}

impl RevolutApi {
    pub fn new(cred: RevolutCredential) -> Self {
        let base_url = cred.base_url().to_string();
        Self {
            cred,
            http: reqwest::Client::new(),
            base_url,
        }
    }

    #[cfg(test)]
    pub fn with_base_url_for_tests(cred: RevolutCredential, base_url: String) -> Self {
        Self {
            cred,
            http: reqwest::Client::new(),
            base_url,
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn list_accounts(&self) -> AppResult<Vec<RevolutAccount>> {
        let url = format!("{}/accounts", self.base_url());
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.cred.access_token)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "revolut".into(),
                message: format!("accounts HTTP: {e}"),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "revolut".into(),
            message: format!("accounts body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "revolut".into(),
                message: format!("accounts {status}: {}", String::from_utf8_lossy(&bytes)),
            });
        }
        let parsed: Vec<RevolutAccount> =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "revolut".into(),
                message: format!("accounts decode: {e}"),
            })?;
        Ok(parsed)
    }

    /// `GET /transactions?from=<ISO>&to=<ISO>&count=N` — Revolut returns
    /// transactions across **all** accounts owned by the merchant, with
    /// per-leg balance changes inside each transaction.
    pub async fn list_transactions(
        &self,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
        count: u32,
    ) -> AppResult<Vec<RevolutTransaction>> {
        let mut params: Vec<(&str, String)> = vec![("count", count.to_string())];
        if let Some(f) = from {
            params.push(("from", f.to_rfc3339()));
        }
        if let Some(t) = to {
            params.push(("to", t.to_rfc3339()));
        }
        let qs = serde_urlencoded::to_string(&params).map_err(|e| AppError::Provider {
            provider: "revolut".into(),
            message: format!("encode query: {e}"),
        })?;
        let url = format!("{}/transactions?{qs}", self.base_url());

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.cred.access_token)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "revolut".into(),
                message: format!("transactions HTTP: {e}"),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "revolut".into(),
            message: format!("transactions body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "revolut".into(),
                message: format!("transactions {status}: {}", String::from_utf8_lossy(&bytes)),
            });
        }
        let parsed: Vec<RevolutTransaction> =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "revolut".into(),
                message: format!("transactions decode: {e}"),
            })?;
        Ok(parsed)
    }
}

// --- Webhook signature verification ---------------------------------------

/// Revolut signs webhooks as `v1=<hex>` HMAC-SHA256 of
/// `{timestamp}.{raw_body}`, where `timestamp` is the `Revolut-Request-Timestamp`
/// header and the result lives in `Revolut-Signature`.
pub fn verify_webhook_signature(
    raw_body: &[u8],
    timestamp_header: &str,
    signature_header: &str,
    webhook_secret: &str,
) -> AppResult<()> {
    let provided = parse_provided(signature_header);
    let signed_payload = format!("{timestamp_header}.");
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(webhook_secret.as_bytes())
        .map_err(|e| AppError::Crypto(format!("hmac init: {e}")))?;
    Mac::update(&mut mac, signed_payload.as_bytes());
    Mac::update(&mut mac, raw_body);
    let expected = hex::encode(Mac::finalize(mac).into_bytes());
    if constant_time_eq_str(&provided, &expected) {
        Ok(())
    } else {
        Err(AppError::Unauthorized)
    }
}

fn parse_provided(header: &str) -> String {
    let h = header.trim();
    if let Some(rest) = h.strip_prefix("v1=") {
        return rest.to_string();
    }
    if let Some(rest) = h.strip_prefix("sha256=") {
        return rest.to_string();
    }
    h.to_string()
}

fn constant_time_eq_str(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in ab.iter().zip(bb.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(body: &[u8], ts: &str, secret: &str) -> String {
        let signed_payload = format!("{ts}.");
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes()).unwrap();
        Mac::update(&mut mac, signed_payload.as_bytes());
        Mac::update(&mut mac, body);
        hex::encode(Mac::finalize(mac).into_bytes())
    }

    #[test]
    fn verifies_revolut_v1_signature() {
        let body = br#"{"event":"transaction.state_changed","id":"42"}"#;
        let ts = "1716423000";
        let sig = sign(body, ts, "topsecret");
        let header = format!("v1={sig}");
        verify_webhook_signature(body, ts, &header, "topsecret").unwrap();
    }

    #[test]
    fn accepts_sha256_prefix_alias() {
        let body = br#"{"id":"x"}"#;
        let ts = "1";
        let sig = sign(body, ts, "k");
        let header = format!("sha256={sig}");
        verify_webhook_signature(body, ts, &header, "k").unwrap();
    }

    #[test]
    fn accepts_bare_hex_signature() {
        let body = br#"{"id":"x"}"#;
        let ts = "1";
        let sig = sign(body, ts, "k");
        verify_webhook_signature(body, ts, &sig, "k").unwrap();
    }

    #[test]
    fn rejects_wrong_secret() {
        let body = br#"{"id":"x"}"#;
        let ts = "1";
        let sig = sign(body, ts, "right");
        let header = format!("v1={sig}");
        let err = verify_webhook_signature(body, ts, &header, "wrong").unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[test]
    fn rejects_tampered_body() {
        let body = br#"{"amount":1}"#;
        let ts = "1";
        let sig = sign(body, ts, "k");
        let header = format!("v1={sig}");
        // Verify against a different body — must fail.
        let err = verify_webhook_signature(b"{\"amount\":99}", ts, &header, "k").unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[test]
    fn rejects_tampered_timestamp() {
        let body = br#"{"id":"x"}"#;
        let sig = sign(body, "1000", "k");
        let header = format!("v1={sig}");
        let err = verify_webhook_signature(body, "1001", &header, "k").unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }
}

// --- Normalization --------------------------------------------------------

#[derive(Clone, Debug)]
pub struct EnsureAccount {
    pub code: String,
    pub kind: AccountKind,
    pub currency: Currency,
}

pub struct NormalizedRevolutTx {
    pub draft: DraftTransaction,
    pub accounts_to_ensure: Vec<EnsureAccount>,
    pub recognized: bool,
}

const ACCT_REVOLUT_PREFIX: &str = "asset/revolut/";
const ACCT_REVENUE: &str = "revenue/revolut";
const ACCT_EXPENSE_FEES: &str = "expense/fees/revolut";
const ACCT_EXPENSE_PAYOUTS: &str = "expense/payouts/revolut";
const ACCT_TRANSFERS_HOLDING: &str = "asset/transit/revolut";

/// Normalize one Revolut transaction into a balanced draft.
///
/// Strategy: each `RevolutLeg` is one side of the ledger entry. A simple
/// "topup" (deposit from external) becomes `clearing/revolut/<account_id>
/// DR, revenue/revolut CR`. A "transfer" between two Revolut accounts is
/// internal and uses the same Revolut asset accounts on both legs.
pub fn normalize_transaction(
    tx: &RevolutTransaction,
    tenant_id: uuid::Uuid,
) -> AppResult<NormalizedRevolutTx> {
    let mut postings: Vec<DraftPosting> = Vec::new();
    let mut accounts: Vec<EnsureAccount> = Vec::new();
    let mut recognized = false;

    let posted_at: DateTime<Utc> = tx
        .completed_at
        .as_deref()
        .or(tx.created_at.as_deref())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    let meta = serde_json::json!({
        "revolut_tx_id": tx.id,
        "revolut_tx_type": tx.kind,
        "revolut_state": tx.state,
        "revolut_reference": tx.reference,
        "revolut_merchant": tx.merchant,
    });

    if !matches!(
        tx.state.as_deref(),
        Some("completed") | Some("processing") | None
    ) {
        return Ok(NormalizedRevolutTx {
            draft: DraftTransaction {
                tenant_id,
                kind: format!("revolut.{}", tx.kind),
                idempotency_key: format!("revolut:tx:{}", tx.id),
                description: None,
                metadata: meta,
                postings,
            },
            accounts_to_ensure: accounts,
            recognized: false,
        });
    }

    if let Some(legs) = &tx.legs {
        for leg in legs {
            let amount_minor = (leg.amount.abs() * 100.0).round() as i128;
            if amount_minor == 0 {
                continue;
            }
            let currency = Currency::new(&leg.currency).map_err(|e| AppError::Provider {
                provider: "revolut".into(),
                message: format!("unknown currency {}: {e}", leg.currency),
            })?;
            let cur = currency.as_str().to_string();
            let revolut_acct_code = format!("{ACCT_REVOLUT_PREFIX}{}", leg.account_id);

            let direction = if leg.amount > 0.0 {
                Direction::Debit
            } else {
                Direction::Credit
            };
            let opposite = direction.opposite();

            postings.push(DraftPosting {
                account_code: revolut_acct_code.clone(),
                direction,
                amount_minor,
                currency: cur.clone(),
                source: "revolut".into(),
                source_event_id: leg.leg_id.clone(),
                metadata: meta.clone(),
            });
            accounts.push(EnsureAccount {
                code: revolut_acct_code,
                kind: AccountKind::Asset,
                currency: currency.clone(),
            });

            // Counterparty side — chosen by transaction kind.
            let (cp_account, cp_kind): (&str, AccountKind) = match tx.kind.as_str() {
                "card_payment" | "card_refund" | "topup" => (ACCT_REVENUE, AccountKind::Income),
                "fee" => (ACCT_EXPENSE_FEES, AccountKind::Expense),
                "atm" | "transfer" | "exchange" => (ACCT_TRANSFERS_HOLDING, AccountKind::Asset),
                "card_credit" => (ACCT_REVENUE, AccountKind::Income),
                "payout" | "withdrawal" => (ACCT_EXPENSE_PAYOUTS, AccountKind::Expense),
                _ => (ACCT_TRANSFERS_HOLDING, AccountKind::Asset),
            };
            postings.push(DraftPosting {
                account_code: cp_account.to_string(),
                direction: opposite,
                amount_minor,
                currency: cur.clone(),
                source: "revolut".into(),
                source_event_id: format!("{}:cp", leg.leg_id),
                metadata: meta.clone(),
            });
            accounts.push(EnsureAccount {
                code: cp_account.to_string(),
                kind: cp_kind,
                currency,
            });
            recognized = true;
        }
    }

    let description = Some(format!(
        "revolut {} {} at {}",
        tx.kind,
        tx.id,
        posted_at.format("%Y-%m-%dT%H:%M:%SZ")
    ));

    Ok(NormalizedRevolutTx {
        draft: DraftTransaction {
            tenant_id,
            kind: format!("revolut.{}", tx.kind),
            idempotency_key: format!("revolut:tx:{}", tx.id),
            description,
            metadata: meta,
            postings,
        },
        accounts_to_ensure: accounts,
        recognized,
    })
}
