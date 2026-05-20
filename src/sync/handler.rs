use async_trait::async_trait;
use serde::Deserialize;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::ledger::LedgerService;
use crate::locks::{AcquireRequest, LockService, ReleaseRequest};
use crate::providers::connection::{ConnectionService, ProviderConnection};
use crate::providers::solana::SolanaWalletCredential;
use crate::providers::{ConnectionStatus, ProviderKind};
use crate::scheduler::{JobContext, JobHandler, JobOutput};
use crate::shard::Region;
use crate::solana::SolanaClient;

use super::rate_limit::ProviderRateLimiter;

use super::{coinflow_sync, stripe_sync, wise_sync};

#[derive(Debug, Deserialize)]
struct SyncPayload {
    connection_id: Uuid,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    trigger: Option<String>,
}

/// Shared context passed to each provider's sync function.
pub struct SyncCtx<'a> {
    pub pool: &'a sqlx::PgPool,
    pub cfg: &'a Config,
    pub ledger: &'a LedgerService,
    pub connections: &'a ConnectionService,
    pub tenant_id: Uuid,
    pub region: Region,
}

pub struct ConnectionSyncJob {
    pool: sqlx::PgPool,
    cfg: Arc<Config>,
    ledger: LedgerService,
    locks: LockService,
    connections: ConnectionService,
    rate_limiter: ProviderRateLimiter,
    solana: SolanaClient,
}

impl ConnectionSyncJob {
    pub fn new(
        pool: sqlx::PgPool,
        cfg: Arc<Config>,
        ledger: LedgerService,
        locks: LockService,
        connections: ConnectionService,
        solana: SolanaClient,
    ) -> Self {
        let rate_limiter = ProviderRateLimiter::new(pool.clone());
        Self {
            pool,
            cfg,
            ledger,
            locks,
            connections,
            rate_limiter,
            solana,
        }
    }
}

#[async_trait]
impl JobHandler for ConnectionSyncJob {
    async fn run(&self, ctx: &JobContext) -> AppResult<JobOutput> {
        let payload: SyncPayload = serde_json::from_value(ctx.payload.clone())
            .map_err(|e| AppError::BadRequest(format!("invalid sync payload: {e}")))?;

        let tenant_id = ctx
            .tenant_id
            .ok_or_else(|| AppError::BadRequest("sync.connection requires tenant_id".into()))?;
        let region = tenant_region(&self.pool, tenant_id).await?;

        let conn = self
            .connections
            .get(tenant_id, payload.connection_id)
            .await?;

        if conn.status != ConnectionStatus::Active {
            return Err(AppError::BadRequest(format!(
                "connection {} is not active (status={:?})",
                conn.id, conn.status
            )));
        }

        // Acquire per-connection lease so concurrent triggers (webhook arrives
        // mid on-demand sync, etc.) don't double-sync the same window.
        let resource = format!("connection:{}", conn.id);
        let lease = self
            .locks
            .acquire(
                tenant_id,
                region,
                Some(&format!("scheduler:{}", ctx.job_id)),
                AcquireRequest {
                    resource: resource.clone(),
                    ttl_seconds: 600,
                    holder: Some(format!("sync.connection/{}", ctx.idempotency_key)),
                    metadata: serde_json::json!({
                        "trigger": payload.trigger.clone().unwrap_or_else(|| "scheduled".into()),
                        "job_id": ctx.job_id,
                    }),
                },
            )
            .await?;

        let sync_result = async {
            let reservation = self.rate_limiter.reserve(tenant_id, conn.provider).await?;
            if !reservation.allowed {
                return Err(AppError::ProviderRateLimited {
                    provider: conn.provider.tag().to_string(),
                    retry_after_seconds: reservation.retry_after_seconds,
                    message: format!(
                        "sync budget exhausted; retry after {}s",
                        reservation.retry_after_seconds
                    ),
                });
            }

            tracing::debug!(
                tenant = %tenant_id,
                connection_id = %conn.id,
                provider = conn.provider.tag(),
                remaining = reservation.remaining,
                window_start = %reservation.window_start,
                "reserved provider sync budget"
            );

            let sctx = SyncCtx {
                pool: &self.pool,
                cfg: &self.cfg,
                ledger: &self.ledger,
                connections: &self.connections,
                tenant_id,
                region,
            };
            let cursor = payload
                .cursor
                .as_deref()
                .or(conn.last_sync_cursor.as_deref());

            match conn.provider {
                ProviderKind::Stripe => stripe_sync::sync_stripe(&sctx, &conn, cursor).await,
                ProviderKind::Coinflow => coinflow_sync::sync_coinflow(&sctx, &conn, cursor).await,
                ProviderKind::Paypal => not_implemented(&conn).await,
                ProviderKind::Braintree => not_implemented(&conn).await,
                ProviderKind::CoinbaseCommerce | ProviderKind::CoinbasePrime => {
                    not_implemented(&conn).await
                }
                ProviderKind::PlaidBank => not_implemented(&conn).await,
                ProviderKind::SwiftWire => not_implemented(&conn).await,
                ProviderKind::AchDirect => not_implemented(&conn).await,
                ProviderKind::Wise => wise_sync::sync_wise(&sctx, &conn, cursor).await,
                ProviderKind::SolanaWallet => sync_solana(&self.solana, &conn, cursor).await,
            }
        }
        .await;

        let _ = self
            .locks
            .release(
                tenant_id,
                region,
                Some(&format!("scheduler:{}", ctx.job_id)),
                &resource,
                ReleaseRequest {
                    lease_token: lease.lease_token,
                },
            )
            .await;

        match sync_result {
            Ok(summary) => {
                self.connections
                    .mark_synced(conn.id, summary.next_cursor.as_deref())
                    .await?;
                Ok(JobOutput::with_data(
                    summary.summary,
                    serde_json::json!({
                        "connection_id": conn.id,
                        "provider": conn.provider.tag(),
                        "new_postings": summary.new_postings,
                        "events_processed": summary.events_processed,
                        "next_cursor": summary.next_cursor,
                        "has_more": summary.has_more,
                    }),
                ))
            }
            Err(e) => {
                let is_rate_limited = matches!(e, AppError::ProviderRateLimited { .. });
                let err = e.to_string();
                let _ = self.connections.mark_sync_failed(conn.id, &err).await;
                if is_rate_limited {
                    Err(e)
                } else {
                    Err(AppError::Provider {
                        provider: conn.provider.tag().to_string(),
                        message: err,
                    })
                }
            }
        }
    }
}

pub struct SyncSummary {
    pub new_postings: i64,
    pub events_processed: i64,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub summary: String,
}

async fn tenant_region(pool: &sqlx::PgPool, tenant_id: Uuid) -> AppResult<Region> {
    let row = sqlx::query(r#"SELECT country_code, us_state FROM tenants WHERE id = $1"#)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("tenant {tenant_id}")))?;
    let cc: String = row.try_get("country_code")?;
    let st: Option<String> = row.try_get("us_state")?;
    Region::from_codes(&cc, st.as_deref()).map_err(|e| AppError::BadRequest(e.to_string()))
}

async fn sync_solana(
    solana: &SolanaClient,
    conn: &ProviderConnection,
    cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    // Solana provider is read-only by design (observer model). The "sync"
    // here is reading recent USDC/SOL transactions for tracked wallets and
    // recording them in the ledger.
    let wallet = solana_wallet_from_connection(conn)?;
    let signatures = solana
        .get_signatures_for_address(&wallet.pubkey_b58, None, cursor, 100)
        .await?;
    let next_cursor = signatures
        .first()
        .map(|info| info.signature.clone())
        .or_else(|| cursor.map(str::to_string));

    Ok(SyncSummary {
        new_postings: 0,
        events_processed: signatures.len() as i64,
        next_cursor,
        has_more: false,
        summary: format!(
            "solana wallet {} finalized signatures scanned; SPL posting parser pending",
            wallet.pubkey_b58
        ),
    })
}

fn solana_wallet_from_connection(conn: &ProviderConnection) -> AppResult<SolanaWalletCredential> {
    if let Ok(wallet) = serde_json::from_value::<SolanaWalletCredential>(conn.metadata.clone()) {
        validate_solana_pubkey(&wallet.pubkey_b58)?;
        return Ok(wallet);
    }

    let pubkey_b58 = conn.external_account_id.clone().ok_or_else(|| {
        AppError::BadRequest(
            "solana_wallet connection requires metadata.pubkey_b58 or external_account_id".into(),
        )
    })?;
    validate_solana_pubkey(&pubkey_b58)?;
    Ok(SolanaWalletCredential {
        pubkey_b58,
        tracked_mints: vec!["USDC".into(), "SOL".into()],
    })
}

fn validate_solana_pubkey(pubkey_b58: &str) -> AppResult<()> {
    let bytes = bs58::decode(pubkey_b58)
        .into_vec()
        .map_err(|e| AppError::BadRequest(format!("invalid Solana wallet pubkey: {e}")))?;
    if bytes.len() != 32 {
        return Err(AppError::BadRequest(
            "Solana wallet pubkey must decode to 32 bytes".into(),
        ));
    }
    Ok(())
}

async fn not_implemented(conn: &ProviderConnection) -> AppResult<SyncSummary> {
    Err(AppError::Provider {
        provider: conn.provider.tag().to_string(),
        message: format!(
            "{} sync not implemented yet; webhook events still flow",
            conn.provider.tag()
        ),
    })
}
