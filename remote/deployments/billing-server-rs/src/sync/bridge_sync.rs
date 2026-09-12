//! Bridge.xyz sync: walk `GET /transfers?starting_after=<id>` and post
//! each terminal-state transfer to the ledger. Idempotent via
//! `bridge:tr:<id>` keys.

use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::providers::bridge::{self, BridgeApi, BridgeCredential, BridgeTransfer};
use crate::providers::connection::ProviderConnection;

use super::handler::{SyncCtx, SyncSummary};

const PAGE_SIZE: u32 = 100;
const MAX_PAGES_PER_RUN: u32 = 5;

pub async fn sync_bridge(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    caller_cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    let plaintext = ctx
        .connections
        .load_credential(ctx.tenant_id, conn.id)
        .await?;
    let cred: BridgeCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "bridge".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    let api = BridgeApi::new(cred);

    let mut cursor: Option<String> = caller_cursor
        .map(str::to_string)
        .or_else(|| conn.last_sync_cursor.clone())
        .or_else(|| {
            conn.metadata
                .get("bridge_cursor")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });

    let mut total_events: i64 = 0;
    let mut total_postings: i64 = 0;
    let mut skipped: i64 = 0;
    let mut has_more = false;

    for _ in 0..MAX_PAGES_PER_RUN {
        let (transfers, next_cursor) = api.list_transfers(PAGE_SIZE, cursor.as_deref()).await?;
        if transfers.is_empty() {
            has_more = false;
            break;
        }

        for tr in &transfers {
            match post_one(ctx, tr).await {
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
                    if let Some(id) = &cursor {
                        let _ = ctx
                            .connections
                            .merge_metadata(
                                conn.tenant_id,
                                conn.id,
                                serde_json::json!({ "bridge_cursor": id }),
                            )
                            .await;
                    }
                    return Err(e);
                }
            }
        }

        cursor = next_cursor;
        if cursor.is_none() || (transfers.len() as u32) < PAGE_SIZE {
            has_more = false;
            break;
        }
        has_more = true;
    }

    if let Some(id) = &cursor {
        ctx.connections
            .merge_metadata(
                conn.tenant_id,
                conn.id,
                serde_json::json!({ "bridge_cursor": id }),
            )
            .await?;
    }

    let summary = format!(
        "bridge: processed {total_events} transfers; \
         posted {total_postings} ledger postings; \
         skipped {skipped} non-terminal; has_more={has_more}"
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

async fn post_one(ctx: &SyncCtx<'_>, tr: &BridgeTransfer) -> AppResult<PostOutcome> {
    let norm = bridge::normalize_transfer(tr, ctx.tenant_id)?;
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
