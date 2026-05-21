//! Mercury — banking for tech startups.
//!
//! Mercury exposes a clean REST API (https://docs.mercury.com/reference)
//! that fits our Model A observer story perfectly: list workspace
//! accounts, list per-account transactions with cursor pagination, and
//! receive signed webhooks for real-time updates. We do NOT initiate
//! ACH/wire payments — Mercury initiation is left to the tenant's own
//! dashboard so we don't add money-movement liability.
//!
//! Auth: `Authorization: Bearer <api_key>` from Mercury Dashboard
//! (Settings → API). The API key is sealed via `attach-api-key` and
//! never leaves the server in plaintext.
//!
//! Base URL: `https://api.mercury.com/api/v1`
//!
//! Endpoints used:
//!   * `GET /accounts` — list every account on the workspace
//!   * `GET /account/{accountId}/transactions?limit=N&offset=N` — per-account txs
//!
//! Webhooks (configured in Mercury dashboard):
//!   X-Mercury-Signature: HMAC-SHA256 hex of `{timestamp}.{raw_body}`
//!   X-Mercury-Timestamp: unix seconds

use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::{AppError, AppResult};
use crate::ledger::{AccountKind, Direction, DraftPosting, DraftTransaction};
use crate::money::Currency;

const MERCURY_BASE: &str = "https://api.mercury.com/api/v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MercuryCredential {
    pub api_key: String,
    /// Optional list of `account_id`s to watch. Empty = all accounts on
    /// the workspace.
    #[serde(default)]
    pub watched_account_ids: Vec<String>,
    pub webhook_secret: Option<String>,
}

// --- Wire types -----------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
pub struct MercuryAccount {
    pub id: String,
    pub name: Option<String>,
    pub nickname: Option<String>,
    #[serde(rename = "type")]
    pub account_type: Option<String>,
    pub status: Option<String>,
    pub currency: Option<String>,
    /// Mercury returns balance as a decimal in major units (e.g. 12345.67).
    /// We don't trust floats for postings, so we only use this for the
    /// daily balance-snapshot reconciliation, never for ledger entries.
    pub available_balance: Option<f64>,
    pub current_balance: Option<f64>,
}

#[derive(Deserialize)]
struct AccountsResponse {
    accounts: Vec<MercuryAccount>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MercuryTransaction {
    pub id: String,
    /// Signed amount in major units: negative = outflow, positive = inflow.
    pub amount: f64,
    pub currency: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(rename = "postedAt")]
    pub posted_at: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    #[serde(rename = "counterpartyName")]
    pub counterparty_name: Option<String>,
    #[serde(rename = "counterpartyNickname")]
    pub counterparty_nickname: Option<String>,
    #[serde(rename = "bankDescription")]
    pub bank_description: Option<String>,
    #[serde(rename = "externalMemo")]
    pub external_memo: Option<String>,
    pub note: Option<String>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Deserialize)]
struct TransactionsResponse {
    transactions: Vec<MercuryTransaction>,
    #[serde(default)]
    total: Option<i64>,
}

// --- API client -----------------------------------------------------------

pub struct MercuryApi {
    cred: MercuryCredential,
    http: reqwest::Client,
}

impl MercuryApi {
    pub fn new(cred: MercuryCredential) -> Self {
        Self {
            cred,
            http: reqwest::Client::new(),
        }
    }

    pub async fn list_accounts(&self) -> AppResult<Vec<MercuryAccount>> {
        let url = format!("{MERCURY_BASE}/accounts");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.cred.api_key)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "mercury".into(),
                message: format!("accounts HTTP: {e}"),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "mercury".into(),
            message: format!("accounts body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "mercury".into(),
                message: format!(
                    "accounts {status}: {}",
                    String::from_utf8_lossy(&bytes)
                ),
            });
        }
        let parsed: AccountsResponse =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "mercury".into(),
                message: format!("accounts decode: {e}"),
            })?;
        Ok(parsed.accounts)
    }

    /// `GET /account/{id}/transactions?limit=N&offset=N&start=<date>` —
    /// returns transactions for one account. We page with `offset` because
    /// Mercury's per-account endpoint uses offset pagination.
    pub async fn list_transactions(
        &self,
        account_id: &str,
        limit: u32,
        offset: u32,
        start: Option<DateTime<Utc>>,
    ) -> AppResult<Vec<MercuryTransaction>> {
        let mut params: Vec<(&str, String)> = vec![
            ("limit", limit.to_string()),
            ("offset", offset.to_string()),
            ("order", "asc".to_string()),
        ];
        if let Some(s) = start {
            params.push(("start", s.format("%Y-%m-%d").to_string()));
        }
        let qs = serde_urlencoded::to_string(&params).map_err(|e| AppError::Provider {
            provider: "mercury".into(),
            message: format!("encode query: {e}"),
        })?;
        let url = format!("{MERCURY_BASE}/account/{account_id}/transactions?{qs}");

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.cred.api_key)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "mercury".into(),
                message: format!("transactions HTTP: {e}"),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "mercury".into(),
            message: format!("transactions body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "mercury".into(),
                message: format!(
                    "transactions {status}: {}",
                    String::from_utf8_lossy(&bytes)
                ),
            });
        }
        let parsed: TransactionsResponse =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "mercury".into(),
                message: format!("transactions decode: {e}"),
            })?;
        Ok(parsed.transactions)
    }
}

// --- Webhook signature verification ---------------------------------------

/// Mercury signs webhooks with HMAC-SHA256 over `{timestamp}.{raw_body}`,
/// where `timestamp` is `X-Mercury-Timestamp` and the result is in
/// `X-Mercury-Signature` as raw hex.
pub fn verify_webhook_signature(
    raw_body: &[u8],
    timestamp_header: &str,
    signature_header: &str,
    webhook_secret: &str,
) -> AppResult<()> {
    let provided = signature_header.trim();
    let signed_prefix = format!("{timestamp_header}.");
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(webhook_secret.as_bytes())
        .map_err(|e| AppError::Crypto(format!("hmac init: {e}")))?;
    Mac::update(&mut mac, signed_prefix.as_bytes());
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
    if ab.len() != bb.len() { return false; }
    let mut diff: u8 = 0;
    for (x, y) in ab.iter().zip(bb.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// --- Normalization --------------------------------------------------------

#[derive(Clone, Debug)]
pub struct EnsureAccount {
    pub code: String,
    pub kind: AccountKind,
    pub currency: Currency,
}

pub struct NormalizedMercuryTx {
    pub draft: DraftTransaction,
    pub accounts_to_ensure: Vec<EnsureAccount>,
    pub recognized: bool,
}

const ACCT_MERCURY_PREFIX: &str = "asset/mercury/";
const ACCT_INCOMING: &str = "income/mercury/unclassified";
const ACCT_OUTGOING: &str = "expense/mercury/unclassified";

/// Normalize one Mercury transaction into a balanced 2-leg posting:
///   * the mercury account is debited (inflow) or credited (outflow)
///   * the opposite side lands in an "unclassified" income/expense bucket
///     until the tenant categorizes it
///
/// We only post for `sent` or `posted` transactions; `pending`/`failed`
/// /`cancelled` are skipped so we don't write phantom entries.
pub fn normalize_transaction(
    tx: &MercuryTransaction,
    tenant_id: uuid::Uuid,
    account_id: &str,
) -> AppResult<NormalizedMercuryTx> {
    let status_ok = matches!(
        tx.status.as_deref(),
        Some("sent") | Some("posted") | Some("Sent") | Some("Posted") | None
    );

    let posted_at: DateTime<Utc> = tx
        .posted_at
        .as_deref()
        .or(tx.created_at.as_deref())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    let currency_str = tx.currency.clone().unwrap_or_else(|| "USD".into()).to_uppercase();
    let currency = Currency::new(&currency_str).map_err(|e| AppError::Provider {
        provider: "mercury".into(),
        message: format!("unknown currency {currency_str}: {e}"),
    })?;
    let cur = currency.as_str().to_string();

    let meta = serde_json::json!({
        "mercury_tx_id": tx.id,
        "mercury_account_id": account_id,
        "mercury_kind": tx.kind,
        "mercury_status": tx.status,
        "mercury_counterparty": tx.counterparty_name,
        "mercury_counterparty_nickname": tx.counterparty_nickname,
        "mercury_bank_description": tx.bank_description,
        "mercury_external_memo": tx.external_memo,
        "mercury_note": tx.note,
    });

    let mut postings: Vec<DraftPosting> = Vec::new();
    let mut accounts: Vec<EnsureAccount> = Vec::new();
    let mut recognized = false;

    let amount_minor = (tx.amount.abs() * 100.0).round() as i128;
    if status_ok && amount_minor > 0 {
        let mercury_acct_code = format!("{ACCT_MERCURY_PREFIX}{account_id}");

        let (mercury_direction, counterparty_acct, counterparty_kind) = if tx.amount > 0.0 {
            (Direction::Debit, ACCT_INCOMING, AccountKind::Income)
        } else {
            (Direction::Credit, ACCT_OUTGOING, AccountKind::Expense)
        };

        postings.push(DraftPosting {
            account_code: mercury_acct_code.clone(),
            direction: mercury_direction,
            amount_minor,
            currency: cur.clone(),
            source: "mercury".into(),
            source_event_id: tx.id.clone(),
            metadata: meta.clone(),
        });
        accounts.push(EnsureAccount {
            code: mercury_acct_code,
            kind: AccountKind::Asset,
            currency: currency.clone(),
        });

        postings.push(DraftPosting {
            account_code: counterparty_acct.into(),
            direction: mercury_direction.opposite(),
            amount_minor,
            currency: cur.clone(),
            source: "mercury".into(),
            source_event_id: format!("{}:cp", tx.id),
            metadata: meta.clone(),
        });
        accounts.push(EnsureAccount {
            code: counterparty_acct.into(),
            kind: counterparty_kind,
            currency,
        });
        recognized = true;
    }

    let description = Some(format!(
        "mercury {} {} ({}) at {}",
        tx.kind.as_deref().unwrap_or("transaction"),
        tx.counterparty_name.as_deref().unwrap_or(""),
        tx.id,
        posted_at.format("%Y-%m-%dT%H:%M:%SZ")
    ));

    Ok(NormalizedMercuryTx {
        draft: DraftTransaction {
            tenant_id,
            kind: format!(
                "mercury.{}",
                tx.kind.as_deref().unwrap_or("transaction")
            ),
            idempotency_key: format!("mercury:tx:{}", tx.id),
            description,
            metadata: meta,
            postings,
        },
        accounts_to_ensure: accounts,
        recognized,
    })
}
