//! Coinbase Commerce backstop sync.
//!
//! Walks `GET /charges?starting_after=<cursor>` paginated and posts each
//! COMPLETED charge through the ledger. Idempotent via
//! `coinbase_commerce:chg:<charge_id>` keys.
//!
//! Crypto-amount postings are intentionally skipped here — they're not
//! generally useful as a ledger entry (huge fractional precision, volatile
//! rate). The fiat-equivalent `local` amount is what we post; the crypto
//! data lives in metadata for tenants who want to surface it.

use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::providers::coinbase::{self, CoinbaseCharge, CoinbaseCommerceApi, CoinbaseCredential};
use crate::providers::connection::ProviderConnection;

use super::handler::{SyncCtx, SyncSummary};

const PAGE_SIZE: u32 = 50;
const MAX_PAGES_PER_RUN: u32 = 6;

pub async fn sync_coinbase_commerce(
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
            provider: "coinbase_commerce".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    let api = CoinbaseCommerceApi::new(cred);

    let mut cursor: Option<String> = caller_cursor
        .map(str::to_string)
        .or_else(|| conn.last_sync_cursor.clone())
        .or_else(|| {
            conn.metadata
                .get("coinbase_commerce_cursor")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });

    let mut total_events: i64 = 0;
    let mut total_postings: i64 = 0;
    let mut skipped: Vec<String> = Vec::new();
    let mut last_charge_id: Option<String> = None;
    let mut has_more = false;

    for _ in 0..MAX_PAGES_PER_RUN {
        let (charges, next_cursor) = api.list_charges(PAGE_SIZE, cursor.as_deref()).await?;
        if charges.is_empty() {
            has_more = next_cursor.is_some();
            cursor = next_cursor;
            break;
        }

        for chg in &charges {
            last_charge_id = Some(chg.id.clone());
            match post_one(ctx, chg).await {
                Ok(PostOutcome::Posted { n }) => {
                    total_events += 1;
                    total_postings += n as i64;
                }
                Ok(PostOutcome::Replayed) => {
                    total_events += 1;
                }
                Ok(PostOutcome::Skipped) => {
                    skipped.push(chg.id.clone());
                }
                Err(e) => {
                    if let Some(id) = &last_charge_id {
                        let _ = ctx
                            .connections
                            .merge_metadata(
                                conn.id,
                                serde_json::json!({
                                    "coinbase_commerce_cursor": id
                                }),
                            )
                            .await;
                    }
                    return Err(e);
                }
            }
        }

        cursor = next_cursor;
        if cursor.is_none() {
            break;
        }
        has_more = true;
    }

    if let Some(id) = &last_charge_id {
        ctx.connections
            .merge_metadata(
                conn.id,
                serde_json::json!({ "coinbase_commerce_cursor": id }),
            )
            .await?;
    }

    let summary = format!(
        "coinbase_commerce: processed {total_events} charges; \
         posted {total_postings} ledger postings; \
         skipped {} non-completed; has_more={}",
        skipped.len(),
        has_more
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

async fn post_one(ctx: &SyncCtx<'_>, charge: &CoinbaseCharge) -> AppResult<PostOutcome> {
    let norm = coinbase::normalize_charge(charge, ctx.tenant_id)?;
    if !norm.recognized || norm.draft.postings.is_empty() {
        return Ok(PostOutcome::Skipped);
    }
    for a in &norm.accounts_to_ensure {
        ctx.ledger
            .ensure_account(
                ctx.tenant_id,
                ctx.region,
                None,
                a.kind,
                &a.code,
                a.currency.clone(),
            )
            .await?;
    }
    let n = norm.draft.postings.len();
    match ctx.ledger.post_transaction(&norm.draft, ctx.region).await {
        Ok(_) => Ok(PostOutcome::Posted { n }),
        Err(AppError::Conflict(_)) => Ok(PostOutcome::Replayed),
        Err(e) => Err(e),
    }
}

#[allow(dead_code)]
fn _silence_uuid(_: Uuid) {}
