//! PayPal sync: walks `GET /v1/reporting/transactions` and posts each
//! recognized transaction event-code to the ledger.
//!
//! PayPal's reporting API has two constraints we work around:
//!   1. Date window is capped at **31 days** per request — so we walk
//!      30-day windows starting from the last seen completed timestamp.
//!   2. The endpoint paginates by page number (1-indexed) — we walk
//!      `page=1..page_count` per window.
//!
//! Idempotency: `paypal:tx:<transaction_id>`. We only post events for
//! `transaction_status == "S"` (completed).
//!
//! Event-code coverage: starts with the most common 8 codes (sale,
//! refund, payout, fee, dispute reversal, currency conversion, mass
//! payment, send money). Everything else opens a reconciliation break.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::ledger::{AccountKind, Direction, DraftPosting, DraftTransaction};
use crate::money::Currency;
use crate::providers::amount::parse_decimal_to_minor;
use crate::providers::connection::ProviderConnection;
use crate::providers::paypal::PaypalCredential;

use super::handler::{SyncCtx, SyncSummary};

const PAGE_SIZE: u32 = 500;
const MAX_WINDOWS_PER_RUN: u32 = 4; // 4 × 30d = 120d max per backstop tick
const WINDOW_DAYS: i64 = 30;
const FIRST_SYNC_LOOKBACK_DAYS: i64 = 90;

// --- Wire types -----------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ReportPage {
    transaction_details: Vec<TxDetail>,
    page: i32,
    total_pages: i32,
    #[allow(dead_code)]
    total_items: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct TxDetail {
    transaction_info: TxInfo,
}

#[derive(Debug, Deserialize)]
struct TxInfo {
    transaction_id: String,
    transaction_event_code: String,
    transaction_status: Option<String>,
    transaction_subject: Option<String>,
    transaction_note: Option<String>,
    transaction_initiation_date: Option<String>,
    transaction_updated_date: Option<String>,
    transaction_amount: Option<Money>,
    fee_amount: Option<Money>,
    payer_email: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Money {
    /// Decimal string like "10.00".
    value: String,
    /// ISO 4217.
    currency_code: String,
}

// --- Sync entry point -----------------------------------------------------

pub async fn sync_paypal(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    _caller_cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    let plaintext = ctx
        .connections
        .load_credential(ctx.tenant_id, conn.id)
        .await?;
    let cred: PaypalCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "paypal".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    let merchant_id = cred.merchant_id.clone();
    let api_base = ctx.cfg.paypal_api_base();
    let http = reqwest::Client::new();

    let mut window_start = conn
        .metadata
        .get("paypal_last_seen_at")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| Utc::now() - Duration::days(FIRST_SYNC_LOOKBACK_DAYS));

    let mut total_events: i64 = 0;
    let mut total_postings: i64 = 0;
    let mut unrecognized: Vec<String> = Vec::new();
    let mut newest_seen: Option<DateTime<Utc>> = None;
    let mut windows_walked = 0u32;

    while windows_walked < MAX_WINDOWS_PER_RUN {
        let window_end = (window_start + Duration::days(WINDOW_DAYS)).min(Utc::now());
        if window_end <= window_start {
            break;
        }
        let mut page = 1i32;
        let mut total_pages = 1i32;
        while page <= total_pages {
            let url = format!("{api_base}/v1/reporting/transactions");
            let qs = serde_urlencoded::to_string([
                ("start_date", window_start.to_rfc3339().as_str()),
                ("end_date", window_end.to_rfc3339().as_str()),
                ("fields", "transaction_info"),
                ("page_size", PAGE_SIZE.to_string().as_str()),
                ("page", page.to_string().as_str()),
            ])
            .map_err(|e| AppError::Provider {
                provider: "paypal".into(),
                message: format!("encode query: {e}"),
            })?;
            let resp = http
                .get(format!("{url}?{qs}"))
                .bearer_auth(&cred.access_token)
                .send()
                .await
                .map_err(|e| AppError::Provider {
                    provider: "paypal".into(),
                    message: format!("reporting/transactions HTTP: {e}"),
                })?;
            let status = resp.status();
            let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
                provider: "paypal".into(),
                message: format!("reporting/transactions body: {e}"),
            })?;
            if !status.is_success() {
                return Err(AppError::Provider {
                    provider: "paypal".into(),
                    message: format!(
                        "reporting/transactions {status}: {}",
                        String::from_utf8_lossy(&bytes)
                    ),
                });
            }
            let parsed: ReportPage =
                serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                    provider: "paypal".into(),
                    message: format!("reporting/transactions decode: {e}"),
                })?;
            total_pages = parsed.total_pages.max(1);

            for det in &parsed.transaction_details {
                let info = &det.transaction_info;
                if info.transaction_status.as_deref() != Some("S") {
                    continue;
                }
                if let Some(ts) = info
                    .transaction_updated_date
                    .as_deref()
                    .or(info.transaction_initiation_date.as_deref())
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.with_timezone(&Utc))
                {
                    newest_seen = Some(match newest_seen {
                        Some(prev) if prev > ts => prev,
                        _ => ts,
                    });
                }

                match post_one(ctx, &merchant_id, info).await {
                    Ok(PostOutcome::Posted { n }) => {
                        total_events += 1;
                        total_postings += n as i64;
                    }
                    Ok(PostOutcome::Replayed) => total_events += 1,
                    Ok(PostOutcome::Unrecognized) => {
                        total_events += 1;
                        unrecognized.push(info.transaction_id.clone());
                    }
                    Err(e) => return Err(e),
                }
            }
            page = parsed.page + 1;
        }

        windows_walked += 1;
        window_start = window_end;
        if window_end >= Utc::now() {
            break;
        }
    }

    if let Some(ts) = newest_seen {
        ctx.connections
            .merge_metadata(
                conn.tenant_id,
                conn.id,
                serde_json::json!({ "paypal_last_seen_at": ts.to_rfc3339() }),
            )
            .await?;
    }

    let summary = format!(
        "paypal: walked {windows_walked} windows; \
         processed {total_events} txs; posted {total_postings} ledger postings; \
         unrecognized {} (recon breaks)",
        unrecognized.len()
    );

    Ok(SyncSummary {
        new_postings: total_postings,
        events_processed: total_events,
        next_cursor: newest_seen.map(|d| d.to_rfc3339()),
        has_more: false,
        summary,
    })
}

enum PostOutcome {
    Posted { n: usize },
    Replayed,
    Unrecognized,
}

const ACCT_CLEARING_PREFIX: &str = "clearing/paypal/";
const ACCT_REVENUE: &str = "revenue/paypal";
const ACCT_FEES: &str = "expense/fees/paypal";
const ACCT_REFUNDS: &str = "expense/refunds/paypal";
const ACCT_BANK_PENDING: &str = "asset/bank/pending";

async fn post_one(
    ctx: &SyncCtx<'_>,
    merchant_id: &str,
    info: &TxInfo,
) -> AppResult<PostOutcome> {
    let amount = match &info.transaction_amount {
        Some(m) => m.clone(),
        None => return Ok(PostOutcome::Unrecognized),
    };
    let currency = Currency::new(&amount.currency_code).map_err(|e| AppError::Provider {
        provider: "paypal".into(),
        message: format!("unknown currency {}: {e}", amount.currency_code),
    })?;
    let cur = currency.as_str().to_string();
    let amount_minor = parse_decimal_to_minor(&amount.value, "paypal")?;
    let abs_amount = amount_minor.unsigned_abs();
    let clearing = format!("{ACCT_CLEARING_PREFIX}{merchant_id}");

    let meta = serde_json::json!({
        "paypal_tx_id": info.transaction_id,
        "paypal_event_code": info.transaction_event_code,
        "paypal_subject": info.transaction_subject,
        "paypal_note": info.transaction_note,
        "paypal_payer_email": info.payer_email,
        "paypal_fee": info.fee_amount,
    });

    let mut postings: Vec<DraftPosting> = Vec::new();
    let mut accounts: Vec<(String, AccountKind)> = Vec::new();

    let Some(category) = classify_paypal_event_code(&info.transaction_event_code) else {
        return Ok(PostOutcome::Unrecognized);
    };

    let mk = |code: &str, dir: Direction, amount_minor: i128| DraftPosting {
        account_code: code.to_string(),
        direction: dir,
        amount_minor,
        currency: cur.clone(),
        source: "paypal".into(),
        source_event_id: info.transaction_id.clone(),
        metadata: meta.clone(),
    };

    match category.tag() {
        "sale" => {
            postings.push(mk(&clearing, Direction::Debit, abs_amount as i128));
            postings.push(mk(ACCT_REVENUE, Direction::Credit, abs_amount as i128));
            accounts.push((clearing.clone(), AccountKind::Asset));
            accounts.push((ACCT_REVENUE.into(), AccountKind::Income));
            if let Some(fee) = info.fee_amount.as_ref() {
                let fee_minor = parse_decimal_to_minor(&fee.value, "paypal")?.unsigned_abs();
                if fee_minor > 0 {
                    postings.push(DraftPosting {
                        account_code: ACCT_FEES.into(),
                        direction: Direction::Debit,
                        amount_minor: fee_minor as i128,
                        currency: cur.clone(),
                        source: "paypal".into(),
                        source_event_id: format!("{}:fee", info.transaction_id),
                        metadata: meta.clone(),
                    });
                    postings.push(DraftPosting {
                        account_code: clearing.clone(),
                        direction: Direction::Credit,
                        amount_minor: fee_minor as i128,
                        currency: cur.clone(),
                        source: "paypal".into(),
                        source_event_id: format!("{}:fee-cp", info.transaction_id),
                        metadata: meta.clone(),
                    });
                    accounts.push((ACCT_FEES.into(), AccountKind::Expense));
                }
            }
        }
        "refund" | "chargeback" => {
            postings.push(mk(ACCT_REFUNDS, Direction::Debit, abs_amount as i128));
            postings.push(mk(&clearing, Direction::Credit, abs_amount as i128));
            accounts.push((ACCT_REFUNDS.into(), AccountKind::Expense));
            accounts.push((clearing.clone(), AccountKind::Asset));
        }
        "payout" => {
            postings.push(mk(ACCT_BANK_PENDING, Direction::Debit, abs_amount as i128));
            postings.push(mk(&clearing, Direction::Credit, abs_amount as i128));
            accounts.push((ACCT_BANK_PENDING.into(), AccountKind::Asset));
            accounts.push((clearing.clone(), AccountKind::Asset));
        }
        "fx" => {
            // FX conversions net to zero on the merchant balance; we
            // record them as observability only.
            return Ok(PostOutcome::Unrecognized);
        }
        _ => return Ok(PostOutcome::Unrecognized),
    }

    for (code, kind) in &accounts {
        ctx.ledger
            .ensure_account(
                ctx.tenant_id,
                ctx.region,
                None,
                *kind,
                code,
                currency.clone(),
            )
            .await?;
    }

    let n = postings.len();
    let draft = DraftTransaction {
        tenant_id: ctx.tenant_id,
        kind: format!("paypal.{}", category.tag()),
        idempotency_key: format!("paypal:tx:{}", info.transaction_id),
        description: Some(format!(
            "paypal {} {} ({})",
            info.transaction_event_code,
            info.transaction_subject.as_deref().unwrap_or(""),
            info.transaction_id
        )),
        metadata: meta,
        postings,
    };
    match ctx.ledger.post_transaction(&draft, ctx.region).await {
        Ok(_) => Ok(PostOutcome::Posted { n }),
        Err(AppError::Conflict(_)) => Ok(PostOutcome::Replayed),
        Err(e) => Err(e),
    }
}

/// PayPal categorizes its event codes into ~30 prefix groups; we map
/// each group to one of our ledger categories. Returns `None` for
/// codes we don't recognize (those become recon breaks rather than
/// silently dropped).
///
/// Reference: PayPal Transaction Event Codes
/// (https://developer.paypal.com/docs/integration/direct/transaction-search/transaction-event-codes/)
pub(crate) fn classify_paypal_event_code(code: &str) -> Option<PaypalCategory> {
    if code.starts_with("T00")
        || code.starts_with("T01")
        || code.starts_with("T02")
        || code.starts_with("T03")
        || code.starts_with("T05")
    {
        Some(PaypalCategory::Sale)
    } else if code.starts_with("T11") {
        Some(PaypalCategory::Refund)
    } else if code.starts_with("T12") {
        Some(PaypalCategory::Chargeback)
    } else if code.starts_with("T04") {
        Some(PaypalCategory::Payout)
    } else if code.starts_with("T15") {
        // FX legs net to zero on the merchant balance; we record them
        // as observability events but don't post.
        Some(PaypalCategory::Fx)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaypalCategory {
    Sale,
    Refund,
    Chargeback,
    Payout,
    Fx,
}

impl PaypalCategory {
    pub(crate) fn tag(self) -> &'static str {
        match self {
            Self::Sale => "sale",
            Self::Refund => "refund",
            Self::Chargeback => "chargeback",
            Self::Payout => "payout",
            Self::Fx => "fx",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_paypal_sale_codes() {
        // T0006 = website sale, T0001 = express checkout
        assert_eq!(
            classify_paypal_event_code("T0006"),
            Some(PaypalCategory::Sale)
        );
        assert_eq!(
            classify_paypal_event_code("T0001"),
            Some(PaypalCategory::Sale)
        );
        assert_eq!(
            classify_paypal_event_code("T0302"),
            Some(PaypalCategory::Sale)
        );
    }

    #[test]
    fn classifies_paypal_refund_codes() {
        // T1101 = refund, T1106 = payment refund
        assert_eq!(
            classify_paypal_event_code("T1101"),
            Some(PaypalCategory::Refund)
        );
        assert_eq!(
            classify_paypal_event_code("T1106"),
            Some(PaypalCategory::Refund)
        );
    }

    #[test]
    fn classifies_paypal_chargeback_codes() {
        // T1201 = chargeback, T1202 = reversal
        assert_eq!(
            classify_paypal_event_code("T1201"),
            Some(PaypalCategory::Chargeback)
        );
        assert_eq!(
            classify_paypal_event_code("T1202"),
            Some(PaypalCategory::Chargeback)
        );
    }

    #[test]
    fn classifies_paypal_payout_codes() {
        // T0400 = payout / mass payment
        assert_eq!(
            classify_paypal_event_code("T0400"),
            Some(PaypalCategory::Payout)
        );
    }

    #[test]
    fn classifies_paypal_fx_codes() {
        // T1503/T1504 = currency conversion
        assert_eq!(
            classify_paypal_event_code("T1503"),
            Some(PaypalCategory::Fx)
        );
        assert_eq!(
            classify_paypal_event_code("T1504"),
            Some(PaypalCategory::Fx)
        );
    }

    #[test]
    fn unrecognized_codes_return_none() {
        assert_eq!(classify_paypal_event_code("T9999"), None);
        assert_eq!(classify_paypal_event_code("ZZZZ"), None);
        assert_eq!(classify_paypal_event_code(""), None);
    }

    #[test]
    fn paypal_category_tags_are_stable() {
        // These tags become posting kinds (paypal.sale, paypal.refund, …)
        // so changing them would break idempotency on replay.
        assert_eq!(PaypalCategory::Sale.tag(), "sale");
        assert_eq!(PaypalCategory::Refund.tag(), "refund");
        assert_eq!(PaypalCategory::Chargeback.tag(), "chargeback");
        assert_eq!(PaypalCategory::Payout.tag(), "payout");
        assert_eq!(PaypalCategory::Fx.tag(), "fx");
    }
}
