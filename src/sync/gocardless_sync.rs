//! GoCardless sync: paginated walk of `GET /payments?after=<id>` and
//! post each terminal-success payment to the ledger. Idempotent via
//! `gocardless:pmt:<id>` keys.

use chrono::{Duration, Utc};

use crate::error::{AppError, AppResult};
use crate::providers::connection::ProviderConnection;
use crate::providers::gocardless::{
    self, GoCardlessApi, GoCardlessCredential, GoCardlessPayment,
};

use super::handler::{SyncCtx, SyncSummary};

const PAGE_SIZE: u32 = 100;
const MAX_PAGES_PER_RUN: u32 = 6;
const FIRST_SYNC_LOOKBACK_DAYS: i64 = 180;

pub async fn sync_gocardless(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    caller_cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    let plaintext = ctx
        .connections
        .load_credential(ctx.tenant_id, conn.id)
        .await?;
    let cred: GoCardlessCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "gocardless".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    let api = GoCardlessApi::new(cred);

    let mut cursor: Option<String> = caller_cursor
        .map(str::to_string)
        .or_else(|| conn.last_sync_cursor.clone())
        .or_else(|| {
            conn.metadata
                .get("gocardless_cursor")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    let created_floor = if cursor.is_none() {
        Some(Utc::now() - Duration::days(FIRST_SYNC_LOOKBACK_DAYS))
    } else {
        None
    };

    let mut total_events: i64 = 0;
    let mut total_postings: i64 = 0;
    let mut skipped: i64 = 0;
    let mut has_more = false;

    for _ in 0..MAX_PAGES_PER_RUN {
        let (payments, next_after) = api
            .list_payments(PAGE_SIZE, cursor.as_deref(), created_floor)
            .await?;
        if payments.is_empty() {
            has_more = next_after.is_some();
            cursor = next_after;
            break;
        }

        for pmt in &payments {
            match post_one(ctx, pmt).await {
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
                                serde_json::json!({ "gocardless_cursor": id }),
                            )
                            .await;
                    }
                    return Err(e);
                }
            }
        }

        cursor = next_after;
        if cursor.is_none() {
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
                serde_json::json!({ "gocardless_cursor": id }),
            )
            .await?;
    }

    let summary = format!(
        "gocardless: processed {total_events} payments; \
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

async fn post_one(ctx: &SyncCtx<'_>, pmt: &GoCardlessPayment) -> AppResult<PostOutcome> {
    let norm = gocardless::normalize_payment(pmt, ctx.tenant_id)?;
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
