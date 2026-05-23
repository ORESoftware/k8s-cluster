//! Coinflow backstop sync: walks `/api/merchant/webhooks` (their own
//! delivery log) so we catch anything we missed from direct webhook
//! deliveries. Idempotent via `coinflow:evt:<event_id>` keys.

use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::providers::coinflow::{self, CoinflowApi, CoinflowCredential};
use crate::providers::connection::ProviderConnection;

use super::handler::{SyncCtx, SyncSummary};

const PAGE_SIZE: u32 = 50;
const MAX_PAGES_PER_RUN: u32 = 6;
/// On first sync (no cursor) we look back this far. Subsequent runs use
/// the saved `last_webhook_seen_at` as the start of the window.
const FIRST_SYNC_LOOKBACK_DAYS: i64 = 90;

pub async fn sync_coinflow(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    _caller_cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    // Decrypt credential.
    let plaintext = ctx
        .connections
        .load_credential(ctx.tenant_id, conn.id)
        .await?;
    let cred: CoinflowCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "coinflow".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    let merchant_id = cred.merchant_id.clone();
    let api = CoinflowApi::new(cred);

    let start_date = conn
        .metadata
        .get("coinflow_last_seen_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| Utc::now() - Duration::days(FIRST_SYNC_LOOKBACK_DAYS));
    let end_date = Utc::now();

    let mut total_events: i64 = 0;
    let mut total_postings: i64 = 0;
    let mut unrecognized: Vec<String> = Vec::new();
    let mut newest_seen: Option<chrono::DateTime<Utc>> = None;
    let mut has_more_overall = false;
    let mut page = 1u32;

    while page <= MAX_PAGES_PER_RUN {
        let (events, has_more) = api
            .list_webhook_activity(Some(start_date), Some(end_date), page, PAGE_SIZE)
            .await?;
        if events.is_empty() {
            has_more_overall = has_more;
            break;
        }

        for evt in &events {
            // Track newest timestamp we've seen so we advance the cursor
            // even when we don't recognize the event type.
            if let Some(ts) = evt
                .created_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc))
            {
                newest_seen = Some(match newest_seen {
                    Some(prev) if prev > ts => prev,
                    _ => ts,
                });
            }

            match post_one(ctx, conn, &merchant_id, evt).await {
                Ok(PostOutcome::Posted { n }) => {
                    total_events += 1;
                    total_postings += n as i64;
                }
                Ok(PostOutcome::Replayed) => {
                    total_events += 1;
                }
                Ok(PostOutcome::Unrecognized) => {
                    total_events += 1;
                    unrecognized.push(evt.id.clone());
                    open_recon_break(ctx, conn, evt).await.ok();
                }
                Err(e) => {
                    // Persist whatever cursor progress we have before bailing.
                    if let Some(ts) = newest_seen {
                        let _ = ctx
                            .connections
                            .merge_metadata(
                                conn.id,
                                serde_json::json!({
                                    "coinflow_last_seen_at": ts.to_rfc3339()
                                }),
                            )
                            .await;
                    }
                    return Err(e);
                }
            }
        }

        if !has_more {
            has_more_overall = false;
            break;
        }
        has_more_overall = true;
        page += 1;
    }

    if let Some(ts) = newest_seen {
        ctx.connections
            .merge_metadata(
                conn.id,
                serde_json::json!({
                    "coinflow_last_seen_at": ts.to_rfc3339()
                }),
            )
            .await?;
    }

    let summary = format!(
        "coinflow: processed {total_events} webhook events; \
         posted {total_postings} ledger postings; \
         unrecognized {} (raised recon breaks); has_more={}",
        unrecognized.len(),
        has_more_overall
    );

    Ok(SyncSummary {
        new_postings: total_postings,
        events_processed: total_events,
        next_cursor: newest_seen.map(|d| d.to_rfc3339()),
        has_more: has_more_overall,
        summary,
    })
}

enum PostOutcome {
    Posted { n: usize },
    Replayed,
    Unrecognized,
}

async fn post_one(
    ctx: &SyncCtx<'_>,
    _conn: &ProviderConnection,
    merchant_id: &str,
    event: &coinflow::CoinflowWebhookEvent,
) -> AppResult<PostOutcome> {
    let norm = coinflow::normalize_event(event, ctx.tenant_id, merchant_id)?;
    if !norm.recognized {
        return Ok(PostOutcome::Unrecognized);
    }
    if norm.draft.postings.is_empty() {
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

async fn open_recon_break(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    evt: &coinflow::CoinflowWebhookEvent,
) -> AppResult<()> {
    let exists: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM reconciliation_breaks
        WHERE tenant_id = $1 AND provider = $2::provider_kind AND external_ref = $3
        LIMIT 1
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(conn.provider.tag())
    .bind(&evt.id)
    .fetch_optional(ctx.pool)
    .await
    .ok()
    .flatten();
    if exists.is_some() {
        return Ok(());
    }

    let amount_text = evt.amount_cents.unwrap_or(0).to_string();
    let detail = serde_json::json!({
        "coinflow_event_id": evt.id,
        "coinflow_event_type": evt.event_type,
        "coinflow_payment_id": evt.payment_id,
        "coinflow_status": evt.status,
        "raw": evt.raw,
        "reason": "no posting template for this coinflow event type"
    });
    let _ = sqlx::query(
        r#"
        INSERT INTO reconciliation_breaks
            (tenant_id, shard_key, provider, connection_id, break_type,
             expected_minor, actual_minor, currency, external_ref, metadata)
        VALUES ($1, $2, $3::provider_kind, $4, 'unrecognized_provider_event',
                ($5)::NUMERIC(38,0), 0::NUMERIC(38,0), $6, $7, $8)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(shard_for(ctx.tenant_id, ctx.region))
    .bind(conn.provider.tag())
    .bind(conn.id)
    .bind(&amount_text)
    .bind(
        evt.currency
            .clone()
            .unwrap_or_else(|| "USD".into())
            .to_uppercase(),
    )
    .bind(&evt.id)
    .bind(&detail)
    .execute(ctx.pool)
    .await;
    Ok(())
}

fn shard_for(tenant_id: Uuid, region: crate::shard::Region) -> i64 {
    crate::shard::ShardKey::derive(tenant_id, region).0
}
