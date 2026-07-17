//! Coinflow — VASP-licensed pay-in / payout / FX orchestration.
//!
//! Coinflow (https://coinflow.cash, docs: https://docs.coinflow.cash) is a
//! Polish-registered VASP (Coinflow Sp.z.o.o., KRS:0001107350) that
//! processes card, ACH, Cash App, and crypto rails behind a single API.
//! Their VASP license is the reason this integration is interesting for
//! our tenants: they can move money through Coinflow without becoming a
//! money-services business themselves.
//!
//! Connection model (NOT OAuth):
//!
//!   * Tenant signs up at coinflow.cash and gets:
//!       - a `merchant_id` (used in URL paths + the `x-coinflow-auth-merchant-id` header)
//!       - an API key (used as the `Authorization` header value)
//!       - a webhook validation key (HMAC secret for incoming events)
//!   * Tenant pastes those into our dashboard. We POST to
//!     `/v1/tenants/{t}/connections/{conn_id}/attach-api-key` with a
//!     `CoinflowCredential` plaintext payload. We seal + store.
//!
//! At runtime we hit the production base `https://api.coinflow.cash`
//! (or `https://api-sandbox.coinflow.cash` for tests) with:
//!
//!   Authorization: <api_key>
//!   x-coinflow-auth-merchant-id: <merchant_id>
//!
//! For sync we walk `/api/merchant/webhooks` with date-range pagination
//! (idempotent because each event has a stable id we use as the source
//! event id in the ledger).

use chrono::{DateTime, TimeZone, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::{AppError, AppResult};
use crate::ledger::{Direction, DraftPosting, DraftTransaction};
use crate::money::Currency;

// --- Credential ------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoinflowCredential {
    pub api_key: String,
    pub merchant_id: String,
    /// "production" | "sandbox". Determines the base URL.
    #[serde(default = "default_env")]
    pub environment: String,
    /// HMAC secret Coinflow gives you in the Admin Dashboard
    /// (Developers → Webhooks → "Validation Key"). Optional because some
    /// tenants attach the API key first and configure webhooks later.
    pub webhook_validation_key: Option<String>,
}
fn default_env() -> String {
    "production".into()
}

impl CoinflowCredential {
    pub fn base_url(&self) -> &'static str {
        if self.environment.eq_ignore_ascii_case("sandbox") {
            "https://api-sandbox.coinflow.cash"
        } else {
            "https://api.coinflow.cash"
        }
    }
}

// --- API client ------------------------------------------------------------

pub struct CoinflowApi {
    cred: CoinflowCredential,
    http: reqwest::Client,
    base_url: String,
}

/// One row from `GET /api/merchant/webhooks`. We keep the fields we need
/// for normalization and stash the entire raw event in `raw` so we can
/// surface it on reconciliation breaks when we don't recognize the type.
#[derive(Clone, Debug, Deserialize)]
pub struct CoinflowWebhookEvent {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub payment_id: Option<String>,
    /// ISO 8601 timestamp from Coinflow.
    pub created_at: Option<String>,
    /// Amount in cents (Coinflow's standard minor-unit shape). Always
    /// positive; direction is determined by `event_type`.
    pub amount_cents: Option<i64>,
    pub currency: Option<String>,
    pub status: Option<String>,
    pub response_code: Option<i32>,
    /// Full payload of the underlying event Coinflow sent to our webhook,
    /// useful for recon breaks.
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct WebhookListResponse {
    data: Vec<CoinflowWebhookEvent>,
    #[serde(default)]
    page: Option<i32>,
    #[serde(default)]
    total_pages: Option<i32>,
    #[serde(default)]
    has_more: Option<bool>,
}

impl CoinflowApi {
    pub fn new(cred: CoinflowCredential) -> Self {
        let base_url = cred.base_url().to_string();
        Self {
            cred,
            http: reqwest::Client::new(),
            base_url,
        }
    }

    #[cfg(test)]
    pub fn with_base_url_for_tests(cred: CoinflowCredential, base_url: String) -> Self {
        Self {
            cred,
            http: reqwest::Client::new(),
            base_url,
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    /// `GET /api/merchant/webhooks` with date-range + pagination.
    /// Returns (events, has_more).
    pub async fn list_webhook_activity(
        &self,
        start_date: Option<DateTime<Utc>>,
        end_date: Option<DateTime<Utc>>,
        page: u32,
        limit: u32,
    ) -> AppResult<(Vec<CoinflowWebhookEvent>, bool)> {
        let mut params: Vec<(&str, String)> =
            vec![("page", page.to_string()), ("limit", limit.to_string())];
        if let Some(d) = start_date {
            params.push(("startDate", d.to_rfc3339()));
        }
        if let Some(d) = end_date {
            params.push(("endDate", d.to_rfc3339()));
        }
        let qs = serde_urlencoded::to_string(&params).map_err(|e| AppError::Provider {
            provider: "coinflow".into(),
            message: format!("encode query: {e}"),
        })?;
        let url = format!("{}/api/merchant/webhooks?{qs}", self.base_url());

        let resp = self
            .http
            .get(&url)
            .header("Authorization", &self.cred.api_key)
            .header("x-coinflow-auth-merchant-id", &self.cred.merchant_id)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "coinflow".into(),
                message: format!("merchant/webhooks HTTP: {e}"),
            })?;

        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "coinflow".into(),
            message: format!("merchant/webhooks body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "coinflow".into(),
                message: format!(
                    "merchant/webhooks {status}: {}",
                    String::from_utf8_lossy(&bytes)
                ),
            });
        }

        let parsed: WebhookListResponse =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "coinflow".into(),
                message: format!("merchant/webhooks decode: {e}"),
            })?;

        let has_more = parsed.has_more.unwrap_or({
            matches!(
                (parsed.page, parsed.total_pages),
                (Some(p), Some(t)) if p < t
            )
        });

        Ok((parsed.data, has_more))
    }
}

// --- Webhook signature verification ----------------------------------------

/// Verify the signature Coinflow sends on incoming webhooks.
///
/// Coinflow's webhook authenticity model uses an HMAC-SHA256 signature
/// computed over the raw request body, keyed by the "Validation Key" the
/// merchant configured in the Admin Dashboard. The signature is sent in
/// a request header (typically `x-coinflow-signature` or similar; exact
/// header name should be confirmed against the merchant's Coinflow
/// dashboard for your environment).
///
/// We accept the signature as a hex-encoded HMAC of the raw body. If
/// Coinflow's actual format diverges (e.g. they send it base64 or
/// prefixed `v1=`), extend `parse_provided` accordingly — the rest of
/// the verifier is constant-time and won't need to change.
pub fn verify_webhook_signature(
    payload: &[u8],
    signature_header: &str,
    validation_key: &str,
) -> AppResult<()> {
    let provided = parse_provided(signature_header);
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(validation_key.as_bytes())
        .map_err(|e| AppError::Crypto(format!("hmac init: {e}")))?;
    Mac::update(&mut mac, payload);
    let expected_bytes = Mac::finalize(mac).into_bytes();
    let expected_hex = hex::encode(expected_bytes);
    if constant_time_eq_str(&provided, &expected_hex) {
        Ok(())
    } else {
        Err(AppError::Unauthorized)
    }
}

fn parse_provided(header: &str) -> String {
    // Tolerate "<hex>", "v1=<hex>", or "sha256=<hex>" shapes.
    let trimmed = header.trim();
    if let Some(rest) = trimmed.strip_prefix("v1=") {
        return rest.to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("sha256=") {
        return rest.to_string();
    }
    trimmed.to_string()
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

// --- Normalization: CoinflowWebhookEvent -> ledger DraftTransaction --------

pub struct NormalizedCoinflowEvent {
    pub draft: DraftTransaction,
    pub accounts_to_ensure: Vec<EnsureAccount>,
    pub recognized: bool,
}

#[derive(Clone, Debug)]
pub struct EnsureAccount {
    pub code: String,
    pub kind: crate::ledger::AccountKind,
    pub currency: Currency,
}

const ACCT_CLEARING_PREFIX: &str = "clearing/coinflow/";
const ACCT_REVENUE: &str = "revenue/coinflow";
const ACCT_FEES: &str = "expense/fees/coinflow";
const ACCT_REFUNDS: &str = "expense/refunds/coinflow";
const ACCT_BANK_PENDING: &str = "asset/bank/pending";

/// Normalize a Coinflow webhook event into a balanced double-entry draft.
///
/// Posting templates by `event_type`. Names follow Coinflow's documented
/// webhook event taxonomy (card payments, ACH transfers, refunds,
/// chargebacks, withdrawals, etc.); unknown types raise reconciliation
/// breaks rather than getting silently dropped.
pub fn normalize_event(
    event: &CoinflowWebhookEvent,
    tenant_id: uuid::Uuid,
    merchant_id: &str,
) -> AppResult<NormalizedCoinflowEvent> {
    let amount_cents = event.amount_cents.unwrap_or(0);
    let currency_str = event
        .currency
        .clone()
        .unwrap_or_else(|| "USD".into())
        .to_uppercase();
    let currency = Currency::new(&currency_str).map_err(|e| AppError::Provider {
        provider: "coinflow".into(),
        message: format!("unknown currency {currency_str}: {e}"),
    })?;
    let cur = currency.as_str().to_string();
    let clearing_code = format!("{ACCT_CLEARING_PREFIX}{merchant_id}");
    let posted_at: DateTime<Utc> = event
        .created_at
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    let idempotency_key = format!("coinflow:evt:{}", event.id);
    let description = Some(format!(
        "coinflow {} {} ({}) at {}",
        event.event_type,
        signed_amount_human(amount_cents, &cur),
        event.id,
        posted_at.format("%Y-%m-%dT%H:%M:%SZ"),
    ));
    let meta = serde_json::json!({
        "coinflow_event_id": event.id,
        "coinflow_event_type": event.event_type,
        "coinflow_payment_id": event.payment_id,
        "coinflow_status": event.status,
        "coinflow_response_code": event.response_code,
        "coinflow_merchant": merchant_id,
        "coinflow_raw": event.raw,
    });

    let mut postings: Vec<DraftPosting> = Vec::new();
    let mut accounts: Vec<EnsureAccount> = Vec::new();
    let mut recognized = true;

    let mk_posting = |account_code: &str, direction: Direction, amount: i128| DraftPosting {
        account_code: account_code.to_string(),
        direction,
        amount_minor: amount,
        currency: cur.clone(),
        source: "coinflow".into(),
        source_event_id: event.id.clone(),
        metadata: meta.clone(),
    };

    let abs = (amount_cents.unsigned_abs()) as i128;

    match event.event_type.as_str() {
        // Successful pay-in (card / Cash App / ACH).
        "cardPayment.succeeded"
        | "cashAppPayment.succeeded"
        | "achPayment.succeeded"
        | "cryptoPayment.succeeded" => {
            postings.push(mk_posting(&clearing_code, Direction::Debit, abs));
            postings.push(mk_posting(ACCT_REVENUE, Direction::Credit, abs));
            accounts.push(EnsureAccount {
                code: clearing_code.clone(),
                kind: crate::ledger::AccountKind::Asset,
                currency: currency.clone(),
            });
            accounts.push(EnsureAccount {
                code: ACCT_REVENUE.into(),
                kind: crate::ledger::AccountKind::Income,
                currency: currency.clone(),
            });
        }
        // Fee debited from clearing.
        "fee.posted" | "platformFee.posted" => {
            postings.push(mk_posting(ACCT_FEES, Direction::Debit, abs));
            postings.push(mk_posting(&clearing_code, Direction::Credit, abs));
            accounts.push(EnsureAccount {
                code: ACCT_FEES.into(),
                kind: crate::ledger::AccountKind::Expense,
                currency: currency.clone(),
            });
            accounts.push(EnsureAccount {
                code: clearing_code.clone(),
                kind: crate::ledger::AccountKind::Asset,
                currency: currency.clone(),
            });
        }
        // Refund issued to a customer.
        "refund.succeeded" | "refund.posted" => {
            postings.push(mk_posting(ACCT_REFUNDS, Direction::Debit, abs));
            postings.push(mk_posting(&clearing_code, Direction::Credit, abs));
            accounts.push(EnsureAccount {
                code: ACCT_REFUNDS.into(),
                kind: crate::ledger::AccountKind::Expense,
                currency: currency.clone(),
            });
            accounts.push(EnsureAccount {
                code: clearing_code.clone(),
                kind: crate::ledger::AccountKind::Asset,
                currency: currency.clone(),
            });
        }
        // Money moving from Coinflow wallet out to bank / crypto recipient.
        "withdrawal.succeeded" | "payout.succeeded" => {
            postings.push(mk_posting(ACCT_BANK_PENDING, Direction::Debit, abs));
            postings.push(mk_posting(&clearing_code, Direction::Credit, abs));
            accounts.push(EnsureAccount {
                code: ACCT_BANK_PENDING.into(),
                kind: crate::ledger::AccountKind::Asset,
                currency: currency.clone(),
            });
            accounts.push(EnsureAccount {
                code: clearing_code.clone(),
                kind: crate::ledger::AccountKind::Asset,
                currency: currency.clone(),
            });
        }
        // Withdrawal failure: money returns to clearing.
        "withdrawal.failed" | "payout.failed" => {
            postings.push(mk_posting(&clearing_code, Direction::Debit, abs));
            postings.push(mk_posting(ACCT_BANK_PENDING, Direction::Credit, abs));
            accounts.push(EnsureAccount {
                code: clearing_code.clone(),
                kind: crate::ledger::AccountKind::Asset,
                currency: currency.clone(),
            });
            accounts.push(EnsureAccount {
                code: ACCT_BANK_PENDING.into(),
                kind: crate::ledger::AccountKind::Asset,
                currency: currency.clone(),
            });
        }
        _ => {
            recognized = false;
        }
    }

    let draft = DraftTransaction {
        tenant_id,
        kind: format!("coinflow.{}", event.event_type),
        idempotency_key,
        description,
        metadata: meta,
        postings,
    };

    Ok(NormalizedCoinflowEvent {
        draft,
        accounts_to_ensure: accounts,
        recognized,
    })
}

fn signed_amount_human(amount_cents: i64, currency: &str) -> String {
    let sign = if amount_cents < 0 { "-" } else { "" };
    let abs = amount_cents.unsigned_abs();
    let whole = abs / 100;
    let frac = abs % 100;
    format!("{sign}{whole}.{frac:02} {currency}")
}

// Silence: TimeZone import becomes unused if posted_at parsing changes shape.
#[allow(dead_code)]
fn _silence_tz(_: chrono::Utc) -> Option<chrono::Utc> {
    None
}

// Compile-time hint that TimeZone is genuinely referenced (for future use
// when we read Unix timestamps from Coinflow's API).
#[allow(dead_code)]
fn _silence_timezone() {
    let _ = chrono::Utc.timestamp_opt(0, 0).single();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_coinflow_hmac() {
        let payload = br#"{"id":"evt_1","type":"cardPayment.succeeded"}"#;
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(b"validation_key").unwrap();
        Mac::update(&mut mac, payload);
        let sig = hex::encode(Mac::finalize(mac).into_bytes());

        verify_webhook_signature(payload, &format!("v1={sig}"), "validation_key").unwrap();
    }

    #[test]
    fn rejects_bad_coinflow_hmac() {
        let err = verify_webhook_signature(b"{}", "v1=deadbeef", "validation_key").unwrap_err();

        assert!(matches!(err, AppError::Unauthorized));
    }
}
