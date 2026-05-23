//! Revolut Business sync — walks `GET /transactions?from=<cursor>` and
//! posts each completed/processing transaction. Idempotent via
//! `revolut:tx:<id>` keys.

use chrono::{Duration, Utc};

use crate::error::{AppError, AppResult};
use crate::providers::connection::ProviderConnection;
use crate::providers::revolut::{self, RevolutApi, RevolutCredential, RevolutTransaction};

use super::handler::{SyncCtx, SyncSummary};

const PAGE_SIZE: u32 = 100;
const MAX_PAGES_PER_RUN: u32 = 4;
const FIRST_SYNC_LOOKBACK_DAYS: i64 = 90;

pub async fn sync_revolut(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    _caller_cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    let plaintext = ctx
        .connections
        .load_credential(ctx.tenant_id, conn.id)
        .await?;
    let cred: RevolutCredential = serde_json::from_slice(&plaintext)
        .map_err(|e| AppError::Provider {
            provider: "revolut".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    let api = RevolutApi::new(cred);

    let mut window_start = conn
        .metadata
        .get("revolut_last_seen_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| Utc::now() - Duration::days(FIRST_SYNC_LOOKBACK_DAYS));

    let mut total_events: i64 = 0;
    let mut total_postings: i64 = 0;
    let mut unrecognized: Vec<String> = Vec::new();
    let mut newest_seen: Option<chrono::DateTime<Utc>> = None;

    for _ in 0..MAX_PAGES_PER_RUN {
        let txs = api
            .list_transactions(Some(window_start), Some(Utc::now()), PAGE_SIZE)
            .await?;
        if txs.is_empty() {
            break;
        }

        let mut walked = 0;
        for tx in &txs {
            walked += 1;
            if let Some(ts) = tx
                .completed_at
                .as_deref()
                .or(tx.created_at.as_deref())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc))
            {
                newest_seen = Some(match newest_seen {
                    Some(prev) if prev > ts => prev,
                    _ => ts,
                });
            }

            match post_one(ctx, tx).await {
                Ok(PostOutcome::Posted { n }) => {
                    total_events += 1;
                    total_postings += n as i64;
                }
                Ok(PostOutcome::Replayed) => {
                    total_events += 1;
                }
                Ok(PostOutcome::Unrecognized) => {
                    total_events += 1;
                    unrecognized.push(tx.id.clone());
                }
                Err(e) => {
                    if let Some(ts) = newest_seen {
                        let _ = ctx
                            .connections
                            .merge_metadata(
                                conn.tenant_id,
                                conn.id,
                                serde_json::json!({
                                    "revolut_last_seen_at": ts.to_rfc3339()
                                }),
                            )
                            .await;
                    }
                    return Err(e);
                }
            }
        }

        if (walked as u32) < PAGE_SIZE {
            break;
        }
        // Advance the window past the newest tx we've seen on this page,
        // so the next page query starts right after it.
        if let Some(ts) = newest_seen {
            window_start = ts + Duration::milliseconds(1);
        }
    }

    if let Some(ts) = newest_seen {
        ctx.connections
            .merge_metadata(
                conn.tenant_id,
                conn.id,
                serde_json::json!({
                    "revolut_last_seen_at": ts.to_rfc3339()
                }),
            )
            .await?;
    }

    let summary = format!(
        "revolut: processed {total_events} transactions; \
         posted {total_postings} ledger postings; \
         unrecognized {}",
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

async fn post_one(ctx: &SyncCtx<'_>, tx: &RevolutTransaction) -> AppResult<PostOutcome> {
    let norm = revolut::normalize_transaction(tx, ctx.tenant_id)?;
    if !norm.recognized || norm.draft.postings.is_empty() {
        return Ok(PostOutcome::Unrecognized);
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
