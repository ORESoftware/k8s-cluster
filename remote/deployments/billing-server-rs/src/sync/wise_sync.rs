//! Wise activity sync.
//!
//! Wise's activity list is useful for "something happened" detection, but its
//! `primaryAmount` fields are formatted display strings. We therefore raise
//! reconciliation breaks for unseen monetary activity and leave exact posting
//! to the balance-statement parser.

use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::providers::connection::ProviderConnection;
use crate::providers::wise::{WiseActivity, WiseApi, WiseCredential};

use super::handler::{SyncCtx, SyncSummary};

const PAGE_SIZE: u32 = 100;
const MAX_PAGES_PER_RUN: u32 = 5;
const FIRST_SYNC_LOOKBACK_DAYS: i64 = 90;

pub async fn sync_wise(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    caller_cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    let plaintext = ctx
        .connections
        .load_credential(ctx.tenant_id, conn.id)
        .await?;
    let cred: WiseCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "wise".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    let api = WiseApi::new(cred);

    let mut cursor = caller_cursor
        .map(str::to_string)
        .or_else(|| conn.last_sync_cursor.clone())
        .or_else(|| {
            conn.metadata
                .get("wise_activity_cursor")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    let since = if cursor.is_none() {
        Some(Utc::now() - Duration::days(FIRST_SYNC_LOOKBACK_DAYS))
    } else {
        None
    };
    let until = Some(Utc::now());

    let mut total_events = 0_i64;
    let mut breaks_opened = 0_i64;
    let mut has_more = false;

    for _ in 0..MAX_PAGES_PER_RUN {
        let (activities, next_cursor) = api
            .list_activities(cursor.as_deref(), since, until, PAGE_SIZE)
            .await?;
        if activities.is_empty() {
            has_more = next_cursor.is_some();
            cursor = next_cursor.or(cursor);
            break;
        }

        for activity in &activities {
            total_events += 1;
            if open_activity_break(ctx, conn, activity).await? {
                breaks_opened += 1;
            }
        }

        match next_cursor {
            Some(next) => {
                cursor = Some(next);
                has_more = true;
            }
            None => {
                has_more = false;
                break;
            }
        }
    }

    if let Some(ref c) = cursor {
        ctx.connections
            .merge_metadata(
                conn.tenant_id,
                conn.id,
                serde_json::json!({ "wise_activity_cursor": c }),
            )
            .await?;
    }

    Ok(SyncSummary {
        new_postings: 0,
        events_processed: total_events,
        next_cursor: cursor,
        has_more,
        summary: format!(
            "wise: scanned {total_events} activities; opened {breaks_opened} reconciliation breaks; statement parser required for postings; has_more={has_more}"
        ),
    })
}

async fn open_activity_break(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    activity: &WiseActivity,
) -> AppResult<bool> {
    let exists: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM reconciliation_breaks
        WHERE tenant_id = $1 AND provider = 'wise'::provider_kind AND external_ref = $2
        LIMIT 1
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(&activity.id)
    .fetch_optional(ctx.pool)
    .await
    .ok()
    .flatten();
    if exists.is_some() {
        return Ok(false);
    }

    let metadata = serde_json::json!({
        "wise_activity_id": activity.id,
        "wise_activity_type": activity.kind,
        "resource": activity.resource.as_ref().map(|r| serde_json::json!({
            "type": r.kind,
            "id": r.id,
        })),
        "status": activity.status,
        "primary_amount": activity.primary_amount,
        "secondary_amount": activity.secondary_amount,
        "created_on": activity.created_on,
        "updated_on": activity.updated_on,
        "raw": activity.raw,
        "reason": "Wise activity detected; balance statement parser must post exact ledger entries"
    });

    sqlx::query(
        r#"
        INSERT INTO reconciliation_breaks
            (tenant_id, shard_key, provider, connection_id, break_type,
             external_ref, metadata)
        VALUES ($1, $2, 'wise'::provider_kind, $3,
                'unposted_wise_activity', $4, $5)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(shard_for(ctx.tenant_id, ctx.region))
    .bind(conn.id)
    .bind(&activity.id)
    .bind(&metadata)
    .execute(ctx.pool)
    .await?;

    Ok(true)
}

fn shard_for(tenant_id: Uuid, region: crate::shard::Region) -> i64 {
    crate::shard::ShardKey::derive(tenant_id, region).0
}
