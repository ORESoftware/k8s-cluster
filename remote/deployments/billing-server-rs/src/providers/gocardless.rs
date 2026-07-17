//! GoCardless — direct debit + open banking.
//!
//! GoCardless runs direct-debit collection across UK (BACS), EU (SEPA),
//! AU (BECS), US (ACH), plus an open-banking "Instant Bank Pay" rail.
//! Great fit for recurring B2B billing (lower fees than card, mandate
//! is owned by the tenant's GoCardless account, we just observe).
//!
//! Auth: OAuth 2.0 (Partner) or API access token (single-merchant).
//! Both are bearer-style; we accept either via the credential shape.
//! Base URL: https://api.gocardless.com (sandbox: api-sandbox.gocardless.com)
//! Headers required:
//!   Authorization: Bearer <token>
//!   GoCardless-Version: 2015-07-06
//!   Accept: application/json
//!
//! Endpoints used:
//!   * `GET /payments?after=<id>&limit=N&created_at[gte]=<ts>` — paginated
//!     payment list, oldest-to-newest if we sort by id ascending.
//!   * `GET /mandates?after=<id>&limit=N` — paginated mandate list (not
//!     yet wired into sync; useful for future recon).
//!
//! Webhook signature: `Webhook-Signature: <hex>` — raw HMAC-SHA256 of
//! the request body, no prefix. Verified inline; constant-time compare.

use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::{AppError, AppResult};
use crate::ledger::{AccountKind, Direction, DraftPosting, DraftTransaction};
use crate::money::Currency;

const GOCARDLESS_API_VERSION: &str = "2015-07-06";

/// GoCardless webhook verification: HMAC-SHA256 of raw body with the
/// secret configured in the GoCardless dashboard. The signature comes
/// in the `Webhook-Signature` header as raw hex (no prefix).
pub fn verify_webhook_signature(
    raw_body: &[u8],
    signature_header: &str,
    webhook_secret: &str,
) -> AppResult<()> {
    let provided = signature_header.trim();
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(webhook_secret.as_bytes())
        .map_err(|e| AppError::Crypto(format!("hmac init: {e}")))?;
    Mac::update(&mut mac, raw_body);
    let expected = hex::encode(Mac::finalize(mac).into_bytes());
    if constant_time_eq_str(provided, &expected) {
        Ok(())
    } else {
        Err(AppError::Unauthorized)
    }
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
mod gocardless_tests {
    use super::*;

    fn sign(body: &[u8], secret: &str) -> String {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes()).unwrap();
        Mac::update(&mut mac, body);
        hex::encode(Mac::finalize(mac).into_bytes())
    }

    #[test]
    fn verifies_gocardless_hmac() {
        let body = br#"{"events":[{"id":"EV1"}]}"#;
        let sig = sign(body, "shh");
        verify_webhook_signature(body, &sig, "shh").unwrap();
    }

    #[test]
    fn rejects_wrong_secret() {
        let body = br#"{"events":[]}"#;
        let sig = sign(body, "right");
        assert!(matches!(
            verify_webhook_signature(body, &sig, "wrong").unwrap_err(),
            AppError::Unauthorized
        ));
    }

    #[test]
    fn rejects_tampered_body() {
        let body = br#"{"events":[{"id":"A"}]}"#;
        let sig = sign(body, "k");
        assert!(matches!(
            verify_webhook_signature(b"{\"events\":[{\"id\":\"B\"}]}", &sig, "k").unwrap_err(),
            AppError::Unauthorized
        ));
    }

    #[test]
    fn whitespace_trimmed_from_header() {
        let body = br#"{"x":1}"#;
        let sig = sign(body, "k");
        let padded = format!("  {sig}  ");
        verify_webhook_signature(body, &padded, "k").unwrap();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoCardlessCredential {
    pub access_token: String,
    pub webhook_secret: Option<String>,
    /// "live" | "sandbox"
    #[serde(default = "default_env")]
    pub environment: String,
}
fn default_env() -> String {
    "live".into()
}

impl GoCardlessCredential {
    pub fn base_url(&self) -> &'static str {
        if self.environment.eq_ignore_ascii_case("sandbox") {
            "https://api-sandbox.gocardless.com"
        } else {
            "https://api.gocardless.com"
        }
    }
}

// --- Wire types -----------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
pub struct GoCardlessPayment {
    pub id: String,
    pub created_at: Option<String>,
    pub charge_date: Option<String>,
    /// In minor units (pence in GBP, cents in EUR, etc).
    pub amount: i64,
    /// In minor units. Refunded portion of the payment.
    #[serde(default)]
    pub amount_refunded: i64,
    pub currency: String,
    pub status: Option<String>,
    pub description: Option<String>,
    pub reference: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub links: Option<serde_json::Value>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Deserialize)]
struct PaymentsResponse {
    payments: Vec<GoCardlessPayment>,
    #[serde(default)]
    meta: Option<MetaWrap>,
}

#[derive(Deserialize)]
struct MetaWrap {
    #[serde(default)]
    cursors: Option<Cursors>,
}

#[derive(Deserialize)]
struct Cursors {
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    before: Option<String>,
}

// --- API client -----------------------------------------------------------

pub struct GoCardlessApi {
    cred: GoCardlessCredential,
    http: reqwest::Client,
    base_url: String,
}

impl GoCardlessApi {
    pub fn new(cred: GoCardlessCredential) -> Self {
        let base_url = cred.base_url().to_string();
        Self {
            cred,
            http: reqwest::Client::new(),
            base_url,
        }
    }

    #[cfg(test)]
    pub fn with_base_url_for_tests(cred: GoCardlessCredential, base_url: String) -> Self {
        Self {
            cred,
            http: reqwest::Client::new(),
            base_url,
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    /// `GET /payments?after=<id>&limit=N` — returns (payments, next_after).
    pub async fn list_payments(
        &self,
        limit: u32,
        after: Option<&str>,
        created_at_gte: Option<DateTime<Utc>>,
    ) -> AppResult<(Vec<GoCardlessPayment>, Option<String>)> {
        let mut params: Vec<(&str, String)> = vec![("limit", limit.to_string())];
        if let Some(c) = after {
            params.push(("after", c.to_string()));
        }
        if let Some(t) = created_at_gte {
            params.push(("created_at[gte]", t.to_rfc3339()));
        }
        let qs = serde_urlencoded::to_string(&params).map_err(|e| AppError::Provider {
            provider: "gocardless".into(),
            message: format!("encode query: {e}"),
        })?;
        let url = format!("{}/payments?{qs}", self.base_url());

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.cred.access_token)
            .header("GoCardless-Version", GOCARDLESS_API_VERSION)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "gocardless".into(),
                message: format!("payments HTTP: {e}"),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "gocardless".into(),
            message: format!("payments body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "gocardless".into(),
                message: format!("payments {status}: {}", String::from_utf8_lossy(&bytes)),
            });
        }
        let parsed: PaymentsResponse =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "gocardless".into(),
                message: format!("payments decode: {e}"),
            })?;

        let next = parsed
            .meta
            .as_ref()
            .and_then(|m| m.cursors.as_ref())
            .and_then(|c| c.after.clone());

        Ok((parsed.payments, next))
    }
}

// --- Normalization --------------------------------------------------------

#[derive(Clone, Debug)]
pub struct EnsureAccount {
    pub code: String,
    pub kind: AccountKind,
    pub currency: Currency,
}

pub struct NormalizedGoCardlessPayment {
    pub draft: DraftTransaction,
    pub accounts_to_ensure: Vec<EnsureAccount>,
    pub recognized: bool,
}

const ACCT_CLEARING: &str = "clearing/gocardless";
const ACCT_REVENUE: &str = "revenue/gocardless";
const ACCT_REFUNDS: &str = "expense/refunds/gocardless";

/// Normalize a GoCardless payment into a balanced draft. We only post
/// for terminal-success states (`paid_out`, `confirmed`); intermediate
/// states (`pending_submission`, `submitted`) stay observability-only
/// so we don't double-post when the payment settles.
///
/// Templates:
///   * paid_out / confirmed: clearing/gocardless DR  revenue/gocardless CR
///   * If `amount_refunded > 0`, additional draft: expense/refunds/gocardless DR
///     clearing/gocardless CR for the refunded amount.
pub fn normalize_payment(
    pmt: &GoCardlessPayment,
    tenant_id: uuid::Uuid,
) -> AppResult<NormalizedGoCardlessPayment> {
    let terminal_ok = matches!(pmt.status.as_deref(), Some("paid_out") | Some("confirmed"));

    let posted_at: DateTime<Utc> = pmt
        .charge_date
        .as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .map(|d| Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap_or_default()))
        .or_else(|| {
            pmt.created_at
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc))
        })
        .unwrap_or_else(Utc::now);

    let currency = Currency::new(&pmt.currency).map_err(|e| AppError::Provider {
        provider: "gocardless".into(),
        message: format!("unknown currency {}: {e}", pmt.currency),
    })?;
    let cur = currency.as_str().to_string();

    let meta = serde_json::json!({
        "gocardless_payment_id": pmt.id,
        "gocardless_status": pmt.status,
        "gocardless_reference": pmt.reference,
        "gocardless_description": pmt.description,
        "gocardless_links": pmt.links,
        "gocardless_metadata": pmt.metadata,
    });

    let mut postings: Vec<DraftPosting> = Vec::new();
    let mut accounts: Vec<EnsureAccount> = Vec::new();
    let mut recognized = false;

    if terminal_ok && pmt.amount > 0 {
        let net_minor = (pmt.amount - pmt.amount_refunded).max(0) as i128;
        if net_minor > 0 {
            postings.push(DraftPosting {
                account_code: ACCT_CLEARING.into(),
                direction: Direction::Debit,
                amount_minor: net_minor,
                currency: cur.clone(),
                source: "gocardless".into(),
                source_event_id: pmt.id.clone(),
                metadata: meta.clone(),
            });
            postings.push(DraftPosting {
                account_code: ACCT_REVENUE.into(),
                direction: Direction::Credit,
                amount_minor: net_minor,
                currency: cur.clone(),
                source: "gocardless".into(),
                source_event_id: format!("{}:rev", pmt.id),
                metadata: meta.clone(),
            });
            accounts.push(EnsureAccount {
                code: ACCT_CLEARING.into(),
                kind: AccountKind::Asset,
                currency: currency.clone(),
            });
            accounts.push(EnsureAccount {
                code: ACCT_REVENUE.into(),
                kind: AccountKind::Income,
                currency: currency.clone(),
            });
            recognized = true;
        }

        if pmt.amount_refunded > 0 {
            let refunded_minor = pmt.amount_refunded as i128;
            postings.push(DraftPosting {
                account_code: ACCT_REFUNDS.into(),
                direction: Direction::Debit,
                amount_minor: refunded_minor,
                currency: cur.clone(),
                source: "gocardless".into(),
                source_event_id: format!("{}:refund", pmt.id),
                metadata: meta.clone(),
            });
            postings.push(DraftPosting {
                account_code: ACCT_CLEARING.into(),
                direction: Direction::Credit,
                amount_minor: refunded_minor,
                currency: cur.clone(),
                source: "gocardless".into(),
                source_event_id: format!("{}:refund-cp", pmt.id),
                metadata: meta.clone(),
            });
            accounts.push(EnsureAccount {
                code: ACCT_REFUNDS.into(),
                kind: AccountKind::Expense,
                currency: currency.clone(),
            });
            recognized = true;
        }
    }

    let description = Some(format!(
        "gocardless payment {} ({}) at {}",
        pmt.id,
        pmt.status.as_deref().unwrap_or("unknown"),
        posted_at.format("%Y-%m-%d")
    ));

    Ok(NormalizedGoCardlessPayment {
        draft: DraftTransaction {
            tenant_id,
            kind: format!(
                "gocardless.payment.{}",
                pmt.status.as_deref().unwrap_or("unknown")
            ),
            idempotency_key: format!("gocardless:pmt:{}", pmt.id),
            description,
            metadata: meta,
            postings,
        },
        accounts_to_ensure: accounts,
        recognized,
    })
}

use chrono::TimeZone;
