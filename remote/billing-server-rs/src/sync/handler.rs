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
use crate::providers::ProviderKind;
use crate::scheduler::{JobContext, JobHandler, JobOutput};
use crate::shard::Region;

use super::{coinflow_sync, stripe_sync};

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
}

impl ConnectionSyncJob {
    pub fn new(
        pool: sqlx::PgPool,
        cfg: Arc<Config>,
        ledger: LedgerService,
        locks: LockService,
        connections: ConnectionService,
    ) -> Self {
        Self { pool, cfg, ledger, locks, connections }
    }
}

#[async_trait]
impl JobHandler for ConnectionSyncJob {
    async fn run(&self, ctx: &JobContext) -> AppResult<JobOutput> {
        let payload: SyncPayload = serde_json::from_value(ctx.payload.clone())
            .map_err(|e| AppError::BadRequest(format!("invalid sync payload: {e}")))?;

        let tenant_id = ctx.tenant_id.ok_or_else(|| {
            AppError::BadRequest("sync.connection requires tenant_id".into())
        })?;
        let region = tenant_region(&self.pool, tenant_id).await?;

        let conn = self
            .connections
            .get(tenant_id, payload.connection_id)
            .await?;

        // Per-connection lease so concurrent triggers don't double-sync.
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

        let sctx = SyncCtx {
            pool: &self.pool,
            cfg: &self.cfg,
            ledger: &self.ledger,
            connections: &self.connections,
            tenant_id,
            region,
        };

        let sync_result = match conn.provider {
            ProviderKind::Stripe => stripe_sync::sync_stripe(&sctx, &conn, payload.cursor.as_deref()).await,
            ProviderKind::Coinflow => coinflow_sync::sync_coinflow(&sctx, &conn, payload.cursor.as_deref()).await,
            ProviderKind::Paypal => not_implemented(&conn).await,
            ProviderKind::Braintree => not_implemented(&conn).await,
            ProviderKind::CoinbaseCommerce | ProviderKind::CoinbasePrime => not_implemented(&conn).await,
            ProviderKind::PlaidBank => not_implemented(&conn).await,
            ProviderKind::SwiftWire => not_implemented(&conn).await,
            ProviderKind::AchDirect => not_implemented(&conn).await,
            ProviderKind::Wise => not_implemented(&conn).await,
            ProviderKind::SolanaWallet => not_implemented(&conn).await,
        };

        let _ = self
            .locks
            .release(
                tenant_id,
                region,
                Some(&format!("scheduler:{}", ctx.job_id)),
                &resource,
                ReleaseRequest { lease_token: lease.lease_token },
            )
            .await;

        match sync_result {
            Ok(summary) => {
                self.connections.mark_synced(conn.id).await?;
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
                let err = e.to_string();
                let _ = self.connections.mark_failed(conn.id, &err).await;
                Err(AppError::Provider {
                    provider: conn.provider.tag().to_string(),
                    message: err,
                })
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
    let row = sqlx::query(
        r#"SELECT country_code, us_state FROM tenants WHERE id = $1"#,
    )
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound(format!("tenant {tenant_id}")))?;
    let cc: String = row.try_get("country_code")?;
    let st: Option<String> = row.try_get("us_state")?;
    Region::from_codes(&cc, st.as_deref()).map_err(|e| AppError::BadRequest(e.to_string()))
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
