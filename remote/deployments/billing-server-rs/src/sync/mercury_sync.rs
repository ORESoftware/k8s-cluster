//! Mercury sync: list accounts, then for each account walk transactions
//! with offset pagination, posting each `sent`/`posted` transaction
//! through the ledger.
//!
//! Cursor model: per-account `(account_id, offset, last_seen_ts)`,
//! stored as a JSON object on the connection metadata under
//! `mercury_account_cursors`. This lets us resume mid-account without
//! re-walking a workspace with 50+ accounts.

use chrono::{Duration, Utc};
use serde_json::{Map, Value};

use crate::error::{AppError, AppResult};
use crate::providers::connection::ProviderConnection;
use crate::providers::mercury::{
    self, MercuryAccount, MercuryApi, MercuryCredential, MercuryTransaction,
};

use super::handler::{SyncCtx, SyncSummary};

const PAGE_SIZE: u32 = 100;
const MAX_PAGES_PER_ACCOUNT_PER_RUN: u32 = 5;
const FIRST_SYNC_LOOKBACK_DAYS: i64 = 180;

pub async fn sync_mercury(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    _caller_cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    let plaintext = ctx
        .connections
        .load_credential(ctx.tenant_id, conn.id)
        .await?;
    let cred: MercuryCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "mercury".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    let watched = cred.watched_account_ids.clone();
    let api = MercuryApi::new(cred);

    let accounts = api.list_accounts().await?;
    let accounts: Vec<MercuryAccount> = if watched.is_empty() {
        accounts
    } else {
        accounts
            .into_iter()
            .filter(|a| watched.iter().any(|w| w == &a.id))
            .collect()
    };

    let mut cursor_map: Map<String, Value> = conn
        .metadata
        .get("mercury_account_cursors")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut total_events: i64 = 0;
    let mut total_postings: i64 = 0;
    let mut skipped: i64 = 0;
    let mut has_more = false;

    for account in &accounts {
        let saved_offset = cursor_map
            .get(&account.id)
            .and_then(|v| v.get("offset"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        let mut offset = saved_offset;
        let start = Some(Utc::now() - Duration::days(FIRST_SYNC_LOOKBACK_DAYS));

        for _ in 0..MAX_PAGES_PER_ACCOUNT_PER_RUN {
            let txs = api
                .list_transactions(&account.id, PAGE_SIZE, offset, start)
                .await?;
            if txs.is_empty() {
                break;
            }

            for tx in &txs {
                match post_one(ctx, &account.id, tx).await {
                    Ok(PostOutcome::Posted { n }) => {
                        total_events += 1;
                        total_postings += n as i64;
                    }
                    Ok(PostOutcome::Replayed) => {
                        total_events += 1;
                    }
                    Ok(PostOutcome::Skipped) => {
                        skipped += 1;
                    }
                    Err(e) => {
                        cursor_map.insert(
                            account.id.clone(),
                            serde_json::json!({ "offset": offset }),
                        );
                        let _ = ctx
                            .connections
                            .merge_metadata(
                                conn.tenant_id,
                                conn.id,
                                serde_json::json!({
                                    "mercury_account_cursors": cursor_map,
                                }),
                            )
                            .await;
                        return Err(e);
                    }
                }
            }

            offset = offset.saturating_add(txs.len() as u32);
            if (txs.len() as u32) < PAGE_SIZE {
                break;
            }
            has_more = true;
        }

        cursor_map.insert(
            account.id.clone(),
            serde_json::json!({ "offset": offset }),
        );
    }

    ctx.connections
        .merge_metadata(
            conn.tenant_id,
            conn.id,
            serde_json::json!({
                "mercury_account_cursors": cursor_map,
                "mercury_known_accounts": accounts
                    .iter()
                    .map(|a| serde_json::json!({
                        "id": a.id,
                        "name": a.name,
                        "currency": a.currency,
                    }))
                    .collect::<Vec<_>>(),
            }),
        )
        .await?;

    let summary = format!(
        "mercury: walked {} accounts; \
         processed {total_events} txs; posted {total_postings} ledger postings; \
         skipped {skipped} (pending/failed/zero-amount); has_more={has_more}",
        accounts.len()
    );

    Ok(SyncSummary {
        new_postings: total_postings,
        events_processed: total_events,
        next_cursor: None,
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
    account_id: &str,
    tx: &MercuryTransaction,
) -> AppResult<PostOutcome> {
    let norm = mercury::normalize_transaction(tx, ctx.tenant_id, account_id)?;
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
