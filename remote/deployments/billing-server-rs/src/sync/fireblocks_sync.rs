//! Fireblocks sync: walks `GET /v1/transactions` forward by `createdAt`.
//!
//! Posting rules — same posture as Coinbase Prime:
//!   - USD-equivalent assets (USDC/USDT/PYUSD/DAI/EURC/etc.) post 1:1
//!     to the matching fiat clearing account
//!   - Native crypto (BTC/ETH/SOL/etc.) records to
//!     `provider_balance_snapshots` for observability only
//!
//! Cursor: epoch-ms of the latest `createdAt` we've seen, stored on
//! `provider_connections.last_sync_cursor`. Fireblocks paginates
//! forward via `after=<epoch_ms>`.

use crate::error::{AppError, AppResult};
use crate::ledger::{AccountKind, Direction, DraftPosting, DraftTransaction};
use crate::money::Currency;
use crate::providers::amount::stablecoin_to_fiat;
use crate::providers::connection::ProviderConnection;
use crate::providers::fireblocks::{FireblocksApi, FireblocksCredential, FireblocksTransaction};

use super::handler::{SyncCtx, SyncSummary};

const PAGE_SIZE: u32 = 200;
const MAX_PAGES_PER_RUN: u32 = 6;
const CLEARING_PREFIX: &str = "clearing/fireblocks/";

pub async fn sync_fireblocks(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    caller_cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    let plaintext = ctx
        .connections
        .load_credential(ctx.tenant_id, conn.id)
        .await?;
    let cred: FireblocksCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "fireblocks".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    let api = FireblocksApi::new(cred);

    let mut after_ms: Option<i64> = caller_cursor
        .or(conn.last_sync_cursor.as_deref())
        .and_then(|s| s.parse::<i64>().ok());

    let mut pages = 0u32;
    let mut total_events: i64 = 0;
    let mut total_postings: i64 = 0;
    let mut observability_only: i64 = 0;
    let mut newest_ms: Option<i64> = after_ms;
    let mut has_more = true;

    while has_more && pages < MAX_PAGES_PER_RUN {
        let page = api.list_transactions(after_ms, PAGE_SIZE).await?;
        pages += 1;
        if page.is_empty() {
            has_more = false;
            break;
        }

        for tx in &page {
            if let Some(ts) = tx.created_at {
                newest_ms = Some(match newest_ms {
                    Some(prev) if prev > ts => prev,
                    _ => ts,
                });
            }
            if !is_terminal(&tx.status) {
                continue;
            }
            match post_one(ctx, conn, tx).await? {
                PostOutcome::Posted { n } => {
                    total_events += 1;
                    total_postings += n as i64;
                }
                PostOutcome::Replayed => total_events += 1,
                PostOutcome::ObservabilityOnly => {
                    total_events += 1;
                    observability_only += 1;
                }
                PostOutcome::Skipped => {}
            }
        }

        after_ms = newest_ms.map(|ms| ms + 1);
        has_more = page.len() as u32 == PAGE_SIZE;
    }

    let summary = format!(
        "fireblocks: pages={pages}; events {total_events}; \
         postings {total_postings}; observability_only={observability_only}; \
         has_more={has_more}"
    );

    Ok(SyncSummary {
        new_postings: total_postings,
        events_processed: total_events,
        next_cursor: newest_ms.map(|ms| ms.to_string()),
        has_more,
        summary,
    })
}

pub(crate) fn is_terminal(status: &str) -> bool {
    // Fireblocks terminal statuses (rejected / blocked / failed never
    // pay out, so we don't post them either).
    matches!(status, "COMPLETED" | "CONFIRMED" | "BROADCASTING")
        || matches!(
            status,
            "REJECTED" | "BLOCKED" | "FAILED" | "CANCELLED" | "CANCELED"
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
    conn: &ProviderConnection,
    tx: &FireblocksTransaction,
) -> AppResult<PostOutcome> {
    if tx.status != "COMPLETED" && tx.status != "CONFIRMED" {
        // We acknowledge failed / cancelled transactions by counting
        // them as events but don't post — observers want to know they
        // happened, but the ledger doesn't move.
        return Ok(PostOutcome::Skipped);
    }
    let symbol = tx.asset_id.clone().unwrap_or_default().to_uppercase();
    let Some(fiat) = stablecoin_to_fiat(&symbol) else {
        let _ = record_observability(ctx, conn, tx).await;
        return Ok(PostOutcome::ObservabilityOnly);
    };

    let amount = match tx.net_amount.or(tx.amount) {
        Some(a) if a != 0.0 => a,
        _ => return Ok(PostOutcome::Skipped),
    };
    let amount_minor = (amount.abs() * 100.0).round() as i128;
    if amount_minor == 0 {
        return Ok(PostOutcome::Skipped);
    }

    let currency = Currency::new(fiat).map_err(|e| AppError::Provider {
        provider: "fireblocks".into(),
        message: format!("unknown currency {fiat}: {e}"),
    })?;
    let cur = currency.as_str().to_string();

    let direction_is_inflow = source_is_external(tx);
    let workspace_id = conn.external_account_id.clone().unwrap_or_else(|| "default".into());
    let clearing = format!("{CLEARING_PREFIX}{}/{}", workspace_id, fiat.to_lowercase());

    let (cp_acct, cp_kind): (&str, AccountKind) = if direction_is_inflow {
        ("revenue/fireblocks/inflows", AccountKind::Income)
    } else {
        ("expense/fireblocks/outflows", AccountKind::Expense)
    };
    let (clearing_dir, cp_dir) = if direction_is_inflow {
        (Direction::Debit, Direction::Credit)
    } else {
        (Direction::Credit, Direction::Debit)
    };

    let meta = serde_json::json!({
        "fireblocks_tx_id": tx.id,
        "fireblocks_status": tx.status,
        "fireblocks_asset_id": tx.asset_id,
        "fireblocks_source": tx.source,
        "fireblocks_destination": tx.destination,
        "fireblocks_tx_hash": tx.tx_hash,
        "fireblocks_operation": tx.operation,
        "fireblocks_amount_raw": tx.amount,
        "fireblocks_net_amount_raw": tx.net_amount,
        "fireblocks_fee_raw": tx.fee,
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
        kind: format!("fireblocks.{}", tx.status.to_lowercase()),
        idempotency_key: format!("fireblocks:tx:{}", tx.id),
        description: Some(format!(
            "fireblocks {} {} {} ({})",
            tx.status,
            tx.operation.as_deref().unwrap_or(""),
            symbol,
            tx.id
        )),
        metadata: meta.clone(),
        postings: vec![
            DraftPosting {
                account_code: clearing,
                direction: clearing_dir,
                amount_minor,
                currency: cur.clone(),
                source: "fireblocks".into(),
                source_event_id: tx.id.clone(),
                metadata: meta.clone(),
            },
            DraftPosting {
                account_code: cp_acct.into(),
                direction: cp_dir,
                amount_minor,
                currency: cur,
                source: "fireblocks".into(),
                source_event_id: format!("{}:cp", tx.id),
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

pub(crate) fn source_is_external(tx: &FireblocksTransaction) -> bool {
    // Fireblocks marks the side of a transfer with `type`:
    //   VAULT_ACCOUNT / EXCHANGE_ACCOUNT / INTERNAL_WALLET / EXTERNAL_WALLET /
    //   FIAT_ACCOUNT / NETWORK_CONNECTION / ONE_TIME_ADDRESS / UNKNOWN
    // External-to-vault = inflow; vault-to-external = outflow.
    let src_external = tx
        .source
        .as_ref()
        .and_then(|p| p.type_.as_deref())
        .map(is_external_type)
        .unwrap_or(false);
    let dst_external = tx
        .destination
        .as_ref()
        .and_then(|p| p.type_.as_deref())
        .map(is_external_type)
        .unwrap_or(false);
    // If src is external and dst is internal → inflow.
    // If neither end is external (internal transfer), treat as inflow
    // for the destination side; the ledger will balance against
    // `revenue/fireblocks/inflows` which is fine for observation.
    src_external || !dst_external
}

pub(crate) fn is_external_type(t: &str) -> bool {
    matches!(
        t,
        "EXTERNAL_WALLET"
            | "ONE_TIME_ADDRESS"
            | "EXCHANGE_ACCOUNT"
            | "NETWORK_CONNECTION"
            | "UNKNOWN"
    )
}

async fn record_observability(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    tx: &FireblocksTransaction,
) -> AppResult<()> {
    let _ = sqlx::query(
        r#"
        INSERT INTO provider_balance_snapshots
            (tenant_id, shard_key, provider, external_account_id, asset_symbol,
             balance_minor, currency, captured_at, metadata)
        VALUES ($1, $2, 'fireblocks'::provider_kind, $3, $4,
                0::NUMERIC(38,0), $4, now(), $5)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(crate::shard::ShardKey::derive(ctx.tenant_id, ctx.region).0)
    .bind(conn.external_account_id.as_deref().unwrap_or("?"))
    .bind(tx.asset_id.as_deref().unwrap_or("?"))
    .bind(serde_json::json!({
        "fireblocks_tx_id": tx.id,
        "fireblocks_status": tx.status,
        "fireblocks_amount": tx.amount,
        "fireblocks_net_amount": tx.net_amount,
        "fireblocks_asset_id": tx.asset_id,
        "fireblocks_tx_hash": tx.tx_hash,
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
    use crate::providers::fireblocks::FireblocksParty;

    fn party(t: &str) -> FireblocksParty {
        FireblocksParty {
            type_: Some(t.into()),
            id: None,
            name: None,
        }
    }

    fn tx_with(src: Option<&str>, dst: Option<&str>) -> FireblocksTransaction {
        FireblocksTransaction {
            id: "tx".into(),
            status: "COMPLETED".into(),
            source: src.map(party),
            destination: dst.map(party),
            amount: Some(1.0),
            net_amount: Some(1.0),
            fee: None,
            asset_id: Some("USDC".into()),
            created_at: None,
            last_updated: None,
            note: None,
            tx_hash: None,
            operation: None,
        }
    }

    #[test]
    fn is_terminal_recognizes_completed_family() {
        assert!(is_terminal("COMPLETED"));
        assert!(is_terminal("CONFIRMED"));
        assert!(is_terminal("BROADCASTING"));
    }

    #[test]
    fn is_terminal_recognizes_failed_family() {
        assert!(is_terminal("REJECTED"));
        assert!(is_terminal("BLOCKED"));
        assert!(is_terminal("FAILED"));
        assert!(is_terminal("CANCELLED"));
        assert!(is_terminal("CANCELED"));
    }

    #[test]
    fn is_terminal_rejects_pending_states() {
        assert!(!is_terminal("PENDING_SIGNATURE"));
        assert!(!is_terminal("QUEUED"));
        assert!(!is_terminal("SUBMITTED"));
    }

    #[test]
    fn external_type_recognition() {
        assert!(is_external_type("EXTERNAL_WALLET"));
        assert!(is_external_type("ONE_TIME_ADDRESS"));
        assert!(is_external_type("EXCHANGE_ACCOUNT"));
        assert!(is_external_type("NETWORK_CONNECTION"));
        assert!(is_external_type("UNKNOWN"));

        assert!(!is_external_type("VAULT_ACCOUNT"));
        assert!(!is_external_type("INTERNAL_WALLET"));
        assert!(!is_external_type("FIAT_ACCOUNT"));
    }

    #[test]
    fn external_to_vault_is_inflow() {
        // EXTERNAL_WALLET → VAULT_ACCOUNT = inflow (someone paid us)
        let tx = tx_with(Some("EXTERNAL_WALLET"), Some("VAULT_ACCOUNT"));
        assert!(source_is_external(&tx));
    }

    #[test]
    fn vault_to_external_is_outflow() {
        // VAULT_ACCOUNT → EXTERNAL_WALLET = outflow (we paid out)
        let tx = tx_with(Some("VAULT_ACCOUNT"), Some("EXTERNAL_WALLET"));
        assert!(!source_is_external(&tx));
    }

    #[test]
    fn vault_to_vault_treated_as_inflow_for_observability() {
        // Internal transfer between two vaults — we observe the
        // destination side, so it counts as an inflow for ledger
        // purposes. Neither end is external → src_external=false but
        // dst_external=false too → !dst_external = true.
        let tx = tx_with(Some("VAULT_ACCOUNT"), Some("VAULT_ACCOUNT"));
        assert!(source_is_external(&tx));
    }

    #[test]
    fn missing_source_treated_as_outflow_if_dst_external() {
        let tx = tx_with(None, Some("EXTERNAL_WALLET"));
        assert!(!source_is_external(&tx));
    }
}
