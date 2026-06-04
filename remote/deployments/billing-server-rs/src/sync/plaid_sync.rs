//! Plaid sync: walks `POST /transactions/sync` (delta API) and posts
//! each *added* transaction to the ledger. Modified/removed events open
//! reconciliation breaks rather than mutating the ledger — reversing
//! posted transactions is intentionally manual.
//!
//! Cursor lives on `provider_connection.last_sync_cursor` (canonical
//! Plaid `next_cursor`). Idempotency via `plaid:tx:<transaction_id>`.

use crate::error::{AppError, AppResult};
use crate::ledger::{AccountKind, Direction, DraftPosting, DraftTransaction};
use crate::money::Currency;
use crate::providers::connection::ProviderConnection;
use crate::providers::plaid::{PlaidCredential, PlaidLink, PlaidSyncPage, PlaidTransaction};

use super::handler::{SyncCtx, SyncSummary};

const PAGE_SIZE: i32 = 500;
const MAX_PAGES_PER_RUN: u32 = 8;

const ACCT_PLAID_PREFIX: &str = "asset/plaid/";
const ACCT_INCOMING: &str = "income/plaid/unclassified";
const ACCT_OUTGOING: &str = "expense/plaid/unclassified";

pub async fn sync_plaid(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    caller_cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    let plaintext = ctx
        .connections
        .load_credential(ctx.tenant_id, conn.id)
        .await?;
    let cred: PlaidCredential = serde_json::from_slice(&plaintext)
        .map_err(|e| AppError::Provider {
            provider: "plaid".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    let link = PlaidLink::new(ctx.cfg);

    let mut cursor: Option<String> = caller_cursor.map(str::to_string).or_else(|| {
        conn.last_sync_cursor
            .clone()
            .filter(|s| !s.is_empty())
    });

    let mut total_added: i64 = 0;
    let mut total_postings: i64 = 0;
    let mut modified_count: i64 = 0;
    let mut removed_count: i64 = 0;
    let mut has_more = true;
    let mut pages = 0u32;

    while has_more && pages < MAX_PAGES_PER_RUN {
        let page: PlaidSyncPage = link
            .sync_transactions(&cred.access_token, cursor.as_deref(), PAGE_SIZE)
            .await?;
        pages += 1;

        for tx in &page.added {
            match post_added(ctx, tx).await {
                Ok(PostOutcome::Posted { n }) => {
                    total_added += 1;
                    total_postings += n as i64;
                }
                Ok(PostOutcome::Replayed) => total_added += 1,
                Ok(PostOutcome::Skipped) => {}
                // Bubble the error up WITHOUT advancing the cursor — the
                // job handler will mark the connection failed, the
                // scheduler will retry, and on retry we replay this
                // page. Posting is idempotent on `plaid:tx:<id>`, so
                // re-posting the items we already wrote is safe.
                Err(e) => return Err(e),
            }
        }
        for tx in &page.modified {
            modified_count += 1;
            let _ = open_modified_break(ctx, conn, tx).await;
        }
        for tx in &page.removed {
            removed_count += 1;
            let _ = open_removed_break(ctx, conn, tx).await;
        }

        cursor = Some(page.next_cursor.clone());
        has_more = page.has_more;
    }

    let summary = format!(
        "plaid: added {total_added}; modified {modified_count} (breaks raised); \
         removed {removed_count} (breaks raised); posted {total_postings}; \
         pages_walked={pages}; has_more={has_more}"
    );

    Ok(SyncSummary {
        new_postings: total_postings,
        events_processed: total_added + modified_count + removed_count,
        next_cursor: cursor,
        has_more,
        summary,
    })
}

enum PostOutcome {
    Posted { n: usize },
    Replayed,
    Skipped,
}

async fn post_added(ctx: &SyncCtx<'_>, tx: &PlaidTransaction) -> AppResult<PostOutcome> {
    // Skip pending transactions — they'll come back as `modified` with
    // `pending=false` when the bank confirms.
    if tx.pending.unwrap_or(false) {
        return Ok(PostOutcome::Skipped);
    }
    let currency_str = tx
        .iso_currency_code
        .clone()
        .or_else(|| tx.unofficial_currency_code.clone())
        .unwrap_or_else(|| "USD".into())
        .to_uppercase();
    let currency = Currency::new(&currency_str).map_err(|e| AppError::Provider {
        provider: "plaid".into(),
        message: format!("unknown currency {currency_str}: {e}"),
    })?;
    let cur = currency.as_str().to_string();

    let amount_minor = (tx.amount.abs() * 100.0).round() as i128;
    if amount_minor == 0 {
        return Ok(PostOutcome::Skipped);
    }

    let plaid_acct_code = format!("{ACCT_PLAID_PREFIX}{}", tx.account_id);
    let (plaid_dir, cp_acct, cp_kind) = match infer_plaid_direction(tx.amount) {
        PlaidDirection::Outflow => (Direction::Credit, ACCT_OUTGOING, AccountKind::Expense),
        PlaidDirection::Inflow => (Direction::Debit, ACCT_INCOMING, AccountKind::Income),
    };

    let meta = serde_json::json!({
        "plaid_tx_id": tx.transaction_id,
        "plaid_account_id": tx.account_id,
        "plaid_name": tx.name,
        "plaid_merchant_name": tx.merchant_name,
        "plaid_payment_channel": tx.payment_channel,
        "plaid_category": tx.category,
        "plaid_category_id": tx.category_id,
        "plaid_date": tx.date,
        "plaid_authorized_date": tx.authorized_date,
    });

    let draft = DraftTransaction {
        tenant_id: ctx.tenant_id,
        kind: "plaid.transaction".into(),
        idempotency_key: format!("plaid:tx:{}", tx.transaction_id),
        description: Some(format!(
            "plaid {} {} ({})",
            tx.merchant_name.as_deref().or(tx.name.as_deref()).unwrap_or("transaction"),
            tx.date.as_deref().unwrap_or(""),
            tx.transaction_id
        )),
        metadata: meta.clone(),
        postings: vec![
            DraftPosting {
                account_code: plaid_acct_code.clone(),
                direction: plaid_dir,
                amount_minor,
                currency: cur.clone(),
                source: "plaid".into(),
                source_event_id: tx.transaction_id.clone(),
                metadata: meta.clone(),
            },
            DraftPosting {
                account_code: cp_acct.into(),
                direction: plaid_dir.opposite(),
                amount_minor,
                currency: cur.clone(),
                source: "plaid".into(),
                source_event_id: format!("{}:cp", tx.transaction_id),
                metadata: meta,
            },
        ],
    };

    ctx.ledger
        .ensure_account(
            ctx.tenant_id,
            ctx.region,
            None,
            AccountKind::Asset,
            &plaid_acct_code,
            currency.clone(),
        )
        .await?;
    ctx.ledger
        .ensure_account(ctx.tenant_id, ctx.region, None, cp_kind, cp_acct, currency)
        .await?;

    match ctx.ledger.post_transaction(&draft, ctx.region).await {
        Ok(_) => Ok(PostOutcome::Posted { n: 2 }),
        Err(AppError::Conflict(_)) => Ok(PostOutcome::Replayed),
        Err(e) => Err(e),
    }
}

async fn open_modified_break(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    tx: &PlaidTransaction,
) -> AppResult<()> {
    let _ = sqlx::query(
        r#"
        INSERT INTO reconciliation_breaks
            (tenant_id, shard_key, provider, connection_id, break_type,
             expected_minor, actual_minor, currency, external_ref, metadata)
        VALUES ($1, $2, $3::provider_kind, $4, 'modified_transaction',
                0::NUMERIC(38,0), 0::NUMERIC(38,0), $5, $6, $7)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(crate::shard::ShardKey::derive(ctx.tenant_id, ctx.region).0)
    .bind(conn.provider.tag())
    .bind(conn.id)
    .bind(tx.iso_currency_code.clone().unwrap_or_else(|| "USD".into()))
    .bind(&tx.transaction_id)
    .bind(serde_json::json!({
        "plaid_tx_id": tx.transaction_id,
        "reason": "plaid /transactions/sync returned this in `modified`; \
                   manual reconciliation needed (we do not auto-reverse)",
    }))
    .execute(ctx.pool)
    .await;
    Ok(())
}

async fn open_removed_break(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    tx: &crate::providers::plaid::PlaidRemovedTx,
) -> AppResult<()> {
    let _ = sqlx::query(
        r#"
        INSERT INTO reconciliation_breaks
            (tenant_id, shard_key, provider, connection_id, break_type,
             expected_minor, actual_minor, currency, external_ref, metadata)
        VALUES ($1, $2, $3::provider_kind, $4, 'removed_transaction',
                0::NUMERIC(38,0), 0::NUMERIC(38,0), 'USD', $5, $6)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(crate::shard::ShardKey::derive(ctx.tenant_id, ctx.region).0)
    .bind(conn.provider.tag())
    .bind(conn.id)
    .bind(&tx.transaction_id)
    .bind(serde_json::json!({
        "plaid_tx_id": tx.transaction_id,
        "plaid_account_id": tx.account_id,
        "reason": "plaid /transactions/sync removed this transaction; \
                   manual reconciliation needed",
    }))
    .execute(ctx.pool)
    .await;
    Ok(())
}

/// Plaid's sign convention is inverted from ours:
///   - positive `amount` means money LEFT the account (outflow / debit)
///   - negative `amount` means money ENTERED the account (inflow / credit)
///
/// This is the most footgun-y aspect of the Plaid API and we test it
/// explicitly so it doesn't silently regress.
pub(crate) fn infer_plaid_direction(amount: f64) -> PlaidDirection {
    if amount > 0.0 {
        PlaidDirection::Outflow
    } else {
        PlaidDirection::Inflow
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlaidDirection {
    Inflow,
    Outflow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaid_positive_amount_is_outflow() {
        // Critical: Plaid documents that positive = outflow. Getting
        // this wrong silently corrupts every ledger we sync.
        assert_eq!(infer_plaid_direction(100.0), PlaidDirection::Outflow);
        assert_eq!(infer_plaid_direction(0.01), PlaidDirection::Outflow);
        assert_eq!(infer_plaid_direction(99999.99), PlaidDirection::Outflow);
    }

    #[test]
    fn plaid_negative_amount_is_inflow() {
        assert_eq!(infer_plaid_direction(-100.0), PlaidDirection::Inflow);
        assert_eq!(infer_plaid_direction(-0.01), PlaidDirection::Inflow);
    }

    #[test]
    fn plaid_zero_amount_treated_as_inflow_but_is_filtered_upstream() {
        // amount=0 is filtered earlier by `if amount_minor == 0
        // { return PostOutcome::Skipped }`, so the direction inference
        // for zero never reaches the ledger. We document the behavior
        // here just so the function's branch coverage is honest.
        assert_eq!(infer_plaid_direction(0.0), PlaidDirection::Inflow);
    }
}
