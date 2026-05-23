//! Coinbase Prime sync: walks the signed-request transactions endpoint
//! for the connected portfolio.
//!
//! Posting rules:
//!   - `USD`              → 1:1 → `clearing/coinbase_prime/usd`
//!   - `USDC`/`USDT`/`PYUSD`/`EURC` (stablecoins) → 1:1 to the fiat
//!     clearing account in the equivalent currency
//!   - All other crypto symbols → no ledger postings, just metadata
//!     stored on a `provider_balance_snapshots` row for observability
//!     (price oracles are out of scope for this push)
//!
//! Idempotency: `coinbase_prime:tx:<id>`.

use crate::error::{AppError, AppResult};
use crate::ledger::{AccountKind, Direction, DraftPosting, DraftTransaction};
use crate::money::Currency;
use crate::providers::amount::{parse_decimal_to_minor, stablecoin_to_fiat};
use crate::providers::coinbase::{CoinbaseCredential, CoinbasePrimeApi, PrimeTransaction};
use crate::providers::connection::ProviderConnection;

use super::handler::{SyncCtx, SyncSummary};

const PAGE_SIZE: u32 = 200;
const MAX_PAGES_PER_RUN: u32 = 6;
const CLEARING_PREFIX: &str = "clearing/coinbase_prime/";
const ACCT_FEES: &str = "expense/fees/coinbase_prime";

pub async fn sync_coinbase_prime(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    caller_cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    let plaintext = ctx
        .connections
        .load_credential(ctx.tenant_id, conn.id)
        .await?;
    let cred: CoinbaseCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "coinbase_prime".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    let api = CoinbasePrimeApi::new(cred)?;
    let portfolio_id = api.portfolio_id()?.to_string();

    let mut cursor: Option<String> = caller_cursor.map(str::to_string).or_else(|| {
        conn.last_sync_cursor
            .clone()
            .filter(|s| !s.is_empty())
    });
    let mut pages = 0u32;
    let mut total_events: i64 = 0;
    let mut total_postings: i64 = 0;
    let mut observability_only: i64 = 0;
    let mut has_more = true;

    while has_more && pages < MAX_PAGES_PER_RUN {
        let page = api
            .list_transactions(cursor.as_deref(), PAGE_SIZE)
            .await?;
        pages += 1;

        for tx in &page.transactions {
            if !is_terminal(&tx.status) {
                continue;
            }
            match post_one(ctx, &portfolio_id, tx).await {
                Ok(PostOutcome::Posted { n }) => {
                    total_events += 1;
                    total_postings += n as i64;
                }
                Ok(PostOutcome::Replayed) => total_events += 1,
                Ok(PostOutcome::ObservabilityOnly) => {
                    total_events += 1;
                    observability_only += 1;
                }
                Ok(PostOutcome::Skipped) => {}
                Err(e) => return Err(e),
            }
        }

        if let Some(p) = page.pagination {
            has_more = p.has_next && p.next_cursor.is_some();
            cursor = p.next_cursor;
        } else {
            has_more = false;
        }
    }

    let summary = format!(
        "coinbase_prime: portfolio={portfolio_id}; pages={pages}; \
         events {total_events}; postings {total_postings}; \
         observability_only={observability_only}; has_more={has_more}"
    );

    Ok(SyncSummary {
        new_postings: total_postings,
        events_processed: total_events,
        next_cursor: cursor,
        has_more,
        summary,
    })
}

pub(crate) fn is_terminal(status: &str) -> bool {
    matches!(
        status,
        "TRANSACTION_COMPLETED" | "TRANSACTION_FAILED" | "TRANSACTION_CANCELED"
    )
}

enum PostOutcome {
    Posted { n: usize },
    Replayed,
    ObservabilityOnly,
    Skipped,
}

async fn post_one(
    ctx: &SyncCtx<'_>,
    portfolio_id: &str,
    tx: &PrimeTransaction,
) -> AppResult<PostOutcome> {
    if tx.status != "TRANSACTION_COMPLETED" {
        return Ok(PostOutcome::Skipped);
    }
    let symbol = tx
        .symbol
        .as_deref()
        .map(str::to_uppercase)
        .unwrap_or_default();
    let Some(fiat) = stablecoin_to_fiat(&symbol) else {
        let _ = record_observability(ctx, portfolio_id, tx).await;
        return Ok(PostOutcome::ObservabilityOnly);
    };

    let currency = Currency::new(fiat).map_err(|e| AppError::Provider {
        provider: "coinbase_prime".into(),
        message: format!("unknown currency {fiat}: {e}"),
    })?;
    let cur = currency.as_str().to_string();

    let amount_str = tx.amount.as_deref().unwrap_or("0");
    let amount_minor = parse_decimal_to_minor(amount_str, "coinbase_prime")?;
    if amount_minor == 0 {
        return Ok(PostOutcome::Skipped);
    }
    let abs = amount_minor.unsigned_abs() as i128;

    let clearing = format!("{CLEARING_PREFIX}{}", fiat.to_lowercase());

    // For DEPOSIT we receive into clearing (asset+). For WITHDRAWAL we
    // pay out from clearing (asset-). REWARD/FEE/INTERNAL fall to fees
    // bucket as a sign-preserving safety net.
    let kind = tx.type_.as_str();
    let (cp_acct, cp_kind, deposit_like): (&str, AccountKind, bool) = match kind {
        "DEPOSIT" => ("revenue/coinbase_prime/deposits", AccountKind::Income, true),
        "WITHDRAWAL" => ("expense/coinbase_prime/withdrawals", AccountKind::Expense, false),
        "FEE" => (ACCT_FEES, AccountKind::Expense, false),
        "REWARD" => ("revenue/coinbase_prime/rewards", AccountKind::Income, true),
        "INTERNAL" | "CONVERSION" => {
            let _ = record_observability(ctx, portfolio_id, tx).await;
            return Ok(PostOutcome::ObservabilityOnly);
        }
        _ => {
            let _ = record_observability(ctx, portfolio_id, tx).await;
            return Ok(PostOutcome::ObservabilityOnly);
        }
    };

    let meta = serde_json::json!({
        "coinbase_prime_tx_id": tx.id,
        "coinbase_prime_type": tx.type_,
        "coinbase_prime_symbol": symbol,
        "coinbase_prime_portfolio_id": portfolio_id,
        "coinbase_prime_completed_at": tx.completed_at,
        "coinbase_prime_destination": tx.destination,
        "coinbase_prime_network_fees": tx.network_fees,
        "fiat_equivalent": fiat,
    });

    let (clearing_dir, cp_dir) = if deposit_like {
        (Direction::Debit, Direction::Credit)
    } else {
        (Direction::Credit, Direction::Debit)
    };

    let postings = vec![
        DraftPosting {
            account_code: clearing.clone(),
            direction: clearing_dir,
            amount_minor: abs,
            currency: cur.clone(),
            source: "coinbase_prime".into(),
            source_event_id: tx.id.clone(),
            metadata: meta.clone(),
        },
        DraftPosting {
            account_code: cp_acct.into(),
            direction: cp_dir,
            amount_minor: abs,
            currency: cur.clone(),
            source: "coinbase_prime".into(),
            source_event_id: format!("{}:cp", tx.id),
            metadata: meta.clone(),
        },
    ];

    for (code, akind) in &[(clearing.as_str(), AccountKind::Asset), (cp_acct, cp_kind)] {
        ctx.ledger
            .ensure_account(
                ctx.tenant_id,
                ctx.region,
                None,
                *akind,
                code,
                currency.clone(),
            )
            .await?;
    }

    let len = postings.len();
    let draft = DraftTransaction {
        tenant_id: ctx.tenant_id,
        kind: format!("coinbase_prime.{}", kind.to_lowercase()),
        idempotency_key: format!("coinbase_prime:tx:{}", tx.id),
        description: Some(format!(
            "coinbase prime {} {symbol} {} ({})",
            kind,
            amount_str,
            tx.id
        )),
        metadata: meta,
        postings,
    };
    match ctx.ledger.post_transaction(&draft, ctx.region).await {
        Ok(_) => Ok(PostOutcome::Posted { n: len }),
        Err(AppError::Conflict(_)) => Ok(PostOutcome::Replayed),
        Err(e) => Err(e),
    }
}

async fn record_observability(
    ctx: &SyncCtx<'_>,
    portfolio_id: &str,
    tx: &PrimeTransaction,
) -> AppResult<()> {
    let _ = sqlx::query(
        r#"
        INSERT INTO provider_balance_snapshots
            (tenant_id, shard_key, provider, external_account_id, asset_symbol,
             balance_minor, currency, captured_at, metadata)
        VALUES ($1, $2, 'coinbase_prime'::provider_kind, $3, $4,
                0::NUMERIC(38,0), $4, now(), $5)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(crate::shard::ShardKey::derive(ctx.tenant_id, ctx.region).0)
    .bind(portfolio_id)
    .bind(tx.symbol.as_deref().unwrap_or("?"))
    .bind(serde_json::json!({
        "coinbase_prime_tx_id": tx.id,
        "coinbase_prime_type": tx.type_,
        "coinbase_prime_amount": tx.amount,
        "coinbase_prime_completed_at": tx.completed_at,
        "note": "non-fiat crypto: recorded for observability only \
                 (price oracle integration deferred)",
    }))
    .execute(ctx.pool)
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_only_accepts_completed_failed_canceled() {
        assert!(is_terminal("TRANSACTION_COMPLETED"));
        assert!(is_terminal("TRANSACTION_FAILED"));
        assert!(is_terminal("TRANSACTION_CANCELED"));
    }

    #[test]
    fn terminal_rejects_in_flight_states() {
        // These are statuses where the money hasn't moved yet (or might
        // still be reversed) — we MUST NOT post them to the ledger.
        assert!(!is_terminal("TRANSACTION_CREATED"));
        assert!(!is_terminal("TRANSACTION_REQUESTED"));
        assert!(!is_terminal("TRANSACTION_APPROVED"));
        assert!(!is_terminal("TRANSACTION_PROCESSING"));
    }

    #[test]
    fn terminal_rejects_unknown_states() {
        // If Coinbase Prime adds a new status we don't know about, we
        // default to "not terminal" — better to under-post and raise
        // a recon break than over-post and have to reverse later.
        assert!(!is_terminal("TRANSACTION_NEW_FUTURE_STATE"));
        assert!(!is_terminal(""));
    }
}
