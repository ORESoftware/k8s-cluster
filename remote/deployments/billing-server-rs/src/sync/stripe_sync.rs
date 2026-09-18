//! Real Stripe sync: pulls `balance_transactions` and writes ledger postings.
//!
//! Pagination strategy:
//!   * Stripe orders newest-first. We persist the newest `id` we've ever
//!     processed in `provider_connections.metadata.stripe_balance_cursor`.
//!   * On each sync we call `GET /balance_transactions?ending_before=<cursor>`,
//!     which returns items *strictly newer* than the cursor.
//!   * We process in chronological order (reverse Stripe's response), post
//!     each as its own ledger Transaction, and advance the cursor to the
//!     newest id we successfully posted.
//!   * Up to 5 pages (500 txns) per sync run; if `has_more`, we surface
//!     `has_more=true` and let the next trigger continue.
//!
//! Idempotency:
//!   * Each ledger transaction's idempotency_key is `stripe:bt:<txn.id>`.
//!     The ledger layer short-circuits replays so re-processing the same
//!     Stripe id is a safe no-op.

use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::providers::connection::ProviderConnection;
use crate::providers::stripe::{self, BalanceTransaction, StripeApi, StripeCredential};

use super::handler::{SyncCtx, SyncSummary};

const MAX_PAGES_PER_RUN: u32 = 5;
const PAGE_SIZE: u32 = 100;

pub async fn sync_stripe(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    caller_cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    let secret = ctx
        .cfg
        .stripe_api_key()
        .ok_or_else(|| AppError::BadRequest("STRIPE_API_KEY not configured".into()))?;

    // Decrypt sealed credential to fetch the connected stripe_user_id (also
    // present on the connection row, but we re-check from the sealed payload
    // as the canonical source).
    let plaintext = ctx
        .connections
        .load_credential(ctx.tenant_id, conn.id)
        .await?;
    let cred: StripeCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "stripe".into(),
            message: format!("decode sealed credential: {e}"),
        })?;

    let api = StripeApi::new(
        secret.clone(),
        cred.stripe_user_id.clone(),
        ctx.cfg.stripe_api_version.clone(),
    );

    // Pick the cursor: caller-supplied (manual continuation) wins, else the
    // saved cursor in metadata, else None (first-ever sync).
    let saved_cursor = conn
        .metadata
        .get("stripe_balance_cursor")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let mut cursor: Option<String> = caller_cursor
        .map(|s| s.to_string())
        .or(saved_cursor.clone());

    let mut total_events: i64 = 0;
    let mut total_postings: i64 = 0;
    let mut unrecognized: Vec<String> = Vec::new();
    let mut pages = 0u32;
    let mut has_more_overall = false;
    let mut last_newest_id: Option<String> = cursor.clone();

    while pages < MAX_PAGES_PER_RUN {
        let (data, has_more) = api
            .list_balance_transactions(cursor.as_deref(), PAGE_SIZE)
            .await?;
        pages += 1;
        if data.is_empty() {
            has_more_overall = has_more;
            break;
        }

        // Stripe returns newest-first; process oldest-first so failure mid-
        // page leaves the cursor pointing at the latest successfully-posted
        // item rather than skipping ahead.
        let chronological: Vec<&BalanceTransaction> = data.iter().rev().collect();
        for bt in &chronological {
            match post_one(ctx, conn, &cred.stripe_user_id, bt).await {
                Ok(PostOutcome::Posted { postings }) => {
                    total_events += 1;
                    total_postings += postings as i64;
                    last_newest_id = Some(bt.id.clone());
                }
                Ok(PostOutcome::Replayed) => {
                    // Already in the ledger via prior sync; treat as success
                    // and advance the cursor so we don't keep re-fetching.
                    total_events += 1;
                    last_newest_id = Some(bt.id.clone());
                }
                Ok(PostOutcome::Unrecognized) => {
                    total_events += 1;
                    unrecognized.push(bt.id.clone());
                    last_newest_id = Some(bt.id.clone());
                    open_recon_break(ctx, conn, bt).await.ok();
                }
                Err(e) => {
                    // Persist the cursor we have so far before bailing.
                    if let Some(ref c) = last_newest_id {
                        let _ = ctx
                            .connections
                            .merge_metadata(
                                conn.tenant_id,
                                conn.id,
                                serde_json::json!({ "stripe_balance_cursor": c }),
                            )
                            .await;
                    }
                    return Err(e);
                }
            }
        }

        // Advance cursor to the newest seen so the next page is strictly newer.
        if let Some(newest) = data.first() {
            cursor = Some(newest.id.clone());
            last_newest_id = Some(newest.id.clone());
        }

        if !has_more {
            has_more_overall = false;
            break;
        }
        has_more_overall = true;
    }

    if let Some(ref c) = last_newest_id {
        ctx.connections
            .merge_metadata(
                conn.tenant_id,
                conn.id,
                serde_json::json!({ "stripe_balance_cursor": c }),
            )
            .await?;
    }

    let summary = format!(
        "stripe: processed {total_events} balance_transactions; \
         posted {total_postings} ledger postings; \
         unrecognized {} (raised recon breaks); has_more={}",
        unrecognized.len(),
        has_more_overall,
    );

    Ok(SyncSummary {
        new_postings: total_postings,
        events_processed: total_events,
        next_cursor: last_newest_id,
        has_more: has_more_overall,
        summary,
    })
}

enum PostOutcome {
    Posted { postings: usize },
    Replayed,
    Unrecognized,
}

async fn post_one(
    ctx: &SyncCtx<'_>,
    _conn: &ProviderConnection,
    stripe_account_id: &str,
    bt: &BalanceTransaction,
) -> AppResult<PostOutcome> {
    let norm = stripe::normalize_balance_transaction(bt, ctx.tenant_id, stripe_account_id)?;

    if !norm.recognized {
        return Ok(PostOutcome::Unrecognized);
    }

    // Ensure all accounts exist for this currency.
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

    let postings_count = norm.draft.postings.len();
    match ctx.ledger.post_transaction(&norm.draft, ctx.region).await {
        Ok(tx_id) => {
            // If the returned tx_id matches an existing transaction (replay),
            // post_transaction returns it without inserting; we don't have a
            // way to distinguish here, so we report Posted with the count.
            // The user-facing tally is approximate by exactly the idempotency
            // count — fine for ops visibility.
            tracing::debug!(stripe_id=%bt.id, tx_id=%tx_id, "posted stripe balance_transaction");
            Ok(PostOutcome::Posted {
                postings: postings_count,
            })
        }
        Err(AppError::Conflict(_)) => Ok(PostOutcome::Replayed),
        Err(e) => Err(e),
    }
}

async fn open_recon_break(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    bt: &BalanceTransaction,
) -> AppResult<()> {
    let detail = serde_json::json!({
        "stripe_id": bt.id,
        "type": bt.kind,
        "amount": bt.amount,
        "fee": bt.fee,
        "currency": bt.currency,
        "status": bt.status,
        "created": bt.created,
        "reason": "no posting template for this stripe balance_transaction type"
    });
    let amount_text = bt.amount.to_string();

    // Skip duplicates: a break with the same (tenant, provider, external_ref)
    // means we've already surfaced this id; the cursor will advance past it
    // either way.
    let exists: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM reconciliation_breaks
        WHERE tenant_id = $1 AND provider = $2::provider_kind AND external_ref = $3
        LIMIT 1
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(conn.provider.tag())
    .bind(&bt.id)
    .fetch_optional(ctx.pool)
    .await
    .ok()
    .flatten();
    if exists.is_some() {
        return Ok(());
    }

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
    .bind(bt.currency.to_uppercase())
    .bind(&bt.id)
    .bind(&detail)
    .execute(ctx.pool)
    .await;

    ctx.events
        .publish_reconciliation_break(
            ctx.tenant_id,
            conn.provider.tag(),
            Some(conn.id),
            "unrecognized_provider_event",
            &bt.currency,
            &bt.id,
            Some(bt.amount as i128),
            Some(0),
        )
        .await;
    Ok(())
}

fn shard_for(tenant_id: Uuid, region: crate::shard::Region) -> i64 {
    crate::shard::ShardKey::derive(tenant_id, region).0
}
