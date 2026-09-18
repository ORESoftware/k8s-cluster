//! Circle Mint sync: walks `GET /v1/businessAccount/transfers` cursor-paginated.
//!
//! Posting rules: Circle Mint balances are always USDC/EURC, both
//! 1:1-pegged by Circle's reserves. We post the full amount to the
//! matching fiat clearing account.
//!
//! Cursor: Circle's `pageAfter` is the id of the last transfer on the
//! previous page. We store the most-recent observed id on
//! `provider_connections.last_sync_cursor`.

use crate::error::{AppError, AppResult};
use crate::ledger::{AccountKind, Direction, DraftPosting, DraftTransaction};
use crate::money::Currency;
use crate::providers::amount::{parse_decimal_to_minor, stablecoin_to_fiat};
use crate::providers::circle::{CircleApi, CircleCredential, CircleTransfer};
use crate::providers::connection::ProviderConnection;

use super::handler::{SyncCtx, SyncSummary};

const PAGE_SIZE: u32 = 100;
const MAX_PAGES_PER_RUN: u32 = 8;
const CLEARING_PREFIX: &str = "clearing/circle/";

pub async fn sync_circle(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    caller_cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    let plaintext = ctx
        .connections
        .load_credential(ctx.tenant_id, conn.id)
        .await?;
    let cred: CircleCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "circle".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    let api = CircleApi::new(cred);

    let mut cursor: Option<String> = caller_cursor.map(str::to_string).or_else(|| {
        conn.last_sync_cursor.clone().filter(|s| !s.is_empty())
    });
    let mut pages = 0u32;
    let mut total_events: i64 = 0;
    let mut total_postings: i64 = 0;
    let mut has_more = true;

    while has_more && pages < MAX_PAGES_PER_RUN {
        let page = api.list_transfers(cursor.as_deref(), PAGE_SIZE).await?;
        pages += 1;
        if page.data.is_empty() {
            has_more = false;
            break;
        }
        // Circle returns transfers ordered most-recent-first by default,
        // which is the wrong direction for forward-cursor walking — but
        // pageAfter walks correctly when we feed it the last-id we saw.
        for t in &page.data {
            cursor = Some(t.id.clone());
            match post_one(ctx, t).await? {
                PostOutcome::Posted { n } => {
                    total_events += 1;
                    total_postings += n as i64;
                }
                PostOutcome::Replayed => total_events += 1,
                PostOutcome::Skipped => {}
            }
        }
        has_more = page.data.len() as u32 == PAGE_SIZE;
    }

    let summary = format!(
        "circle: pages={pages}; events {total_events}; \
         postings {total_postings}; has_more={has_more}"
    );

    Ok(SyncSummary {
        new_postings: total_postings,
        events_processed: total_events,
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

async fn post_one(
    ctx: &SyncCtx<'_>,
    t: &CircleTransfer,
) -> AppResult<PostOutcome> {
    if t.status.as_deref() != Some("complete") {
        return Ok(PostOutcome::Skipped);
    }
    let Some(amount) = t.amount.as_ref() else {
        return Ok(PostOutcome::Skipped);
    };
    let symbol = amount.currency.to_uppercase();
    let Some(fiat) = stablecoin_to_fiat(&symbol) else {
        return Ok(PostOutcome::Skipped);
    };
    let currency = Currency::new(fiat).map_err(|e| AppError::Provider {
        provider: "circle".into(),
        message: format!("unknown currency {fiat}: {e}"),
    })?;
    let cur = currency.as_str().to_string();
    let amount_minor = parse_decimal_to_minor(&amount.amount, "circle")?;
    let abs = amount_minor.unsigned_abs() as i128;
    if abs == 0 {
        return Ok(PostOutcome::Skipped);
    }

    let inflow = destination_is_circle_wallet(t);
    let clearing = format!("{CLEARING_PREFIX}{}", fiat.to_lowercase());
    let (cp_acct, cp_kind) = if inflow {
        ("revenue/circle/mint_deposits", AccountKind::Income)
    } else {
        ("expense/circle/mint_redemptions", AccountKind::Expense)
    };
    let (clearing_dir, cp_dir) = if inflow {
        (Direction::Debit, Direction::Credit)
    } else {
        (Direction::Credit, Direction::Debit)
    };

    let meta = serde_json::json!({
        "circle_transfer_id": t.id,
        "circle_status": t.status,
        "circle_create_date": t.create_date,
        "circle_source": t.source,
        "circle_destination": t.destination,
        "circle_transaction_hash": t.transaction_hash,
        "fiat_equivalent": fiat,
    });

    for (code, kind) in &[
        (clearing.as_str(), AccountKind::Asset),
        (cp_acct, cp_kind),
    ] {
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

    let draft = DraftTransaction {
        tenant_id: ctx.tenant_id,
        kind: format!(
            "circle.{}",
            if inflow { "deposit" } else { "redemption" }
        ),
        idempotency_key: format!("circle:transfer:{}", t.id),
        description: Some(format!(
            "circle transfer {} {} {} ({})",
            t.status.as_deref().unwrap_or("?"),
            amount.amount,
            symbol,
            t.id
        )),
        metadata: meta.clone(),
        postings: vec![
            DraftPosting {
                account_code: clearing,
                direction: clearing_dir,
                amount_minor: abs,
                currency: cur.clone(),
                source: "circle".into(),
                source_event_id: t.id.clone(),
                metadata: meta.clone(),
            },
            DraftPosting {
                account_code: cp_acct.into(),
                direction: cp_dir,
                amount_minor: abs,
                currency: cur,
                source: "circle".into(),
                source_event_id: format!("{}:cp", t.id),
                metadata: meta,
            },
        ],
    };
    match ctx.ledger.post_transaction(&draft, ctx.region).await {
        Ok(_) => Ok(PostOutcome::Posted { n: 2 }),
        Err(AppError::Conflict(_)) => Ok(PostOutcome::Replayed),
        Err(e) => Err(e),
    }
}

pub(crate) fn destination_is_circle_wallet(t: &CircleTransfer) -> bool {
    t.destination
        .as_ref()
        .and_then(|e| e.type_.as_deref())
        .map(|s| s.eq_ignore_ascii_case("wallet"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::circle::{CircleAmount, CircleEndpoint};

    fn transfer_with(dst_type: Option<&str>) -> CircleTransfer {
        CircleTransfer {
            id: "t1".into(),
            source: None,
            destination: dst_type.map(|t| CircleEndpoint {
                type_: Some(t.into()),
                id: None,
                chain: None,
                address: None,
            }),
            amount: Some(CircleAmount {
                amount: "100.00".into(),
                currency: "USD".into(),
            }),
            status: Some("complete".into()),
            create_date: None,
            transaction_hash: None,
        }
    }

    #[test]
    fn destination_wallet_lower_case() {
        assert!(destination_is_circle_wallet(&transfer_with(Some("wallet"))));
    }

    #[test]
    fn destination_wallet_case_insensitive() {
        // Circle docs use "wallet" (lowercase) but be defensive.
        assert!(destination_is_circle_wallet(&transfer_with(Some("Wallet"))));
        assert!(destination_is_circle_wallet(&transfer_with(Some("WALLET"))));
    }

    #[test]
    fn destination_blockchain_is_not_wallet() {
        // Outflow to an external chain address: type = "blockchain".
        assert!(!destination_is_circle_wallet(&transfer_with(Some(
            "blockchain"
        ))));
    }

    #[test]
    fn destination_missing_is_not_wallet() {
        assert!(!destination_is_circle_wallet(&transfer_with(None)));
    }
}
