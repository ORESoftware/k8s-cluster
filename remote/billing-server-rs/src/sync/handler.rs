use async_trait::async_trait;
use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::locks::{AcquireRequest, LockService, ReleaseRequest};
use crate::providers::connection::ConnectionService;
use crate::providers::ProviderKind;
use crate::scheduler::{JobContext, JobHandler, JobOutput};
use crate::shard::Region;

#[derive(Debug, Deserialize)]
struct SyncPayload {
    connection_id: Uuid,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    trigger: Option<String>,
}

pub struct ConnectionSyncJob {
    pool: sqlx::PgPool,
    locks: LockService,
    connections: ConnectionService,
}

impl ConnectionSyncJob {
    pub fn new(
        pool: sqlx::PgPool,
        locks: LockService,
        connections: ConnectionService,
    ) -> Self {
        Self { pool, locks, connections }
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

        // Per-provider sync. All stubbed today — return a "not-implemented"
        // structured result so the contract is real but obvious.
        let sync_result = match conn.provider {
            ProviderKind::Stripe           => sync_stripe(&conn, payload.cursor.as_deref()).await,
            ProviderKind::Paypal           => sync_paypal(&conn, payload.cursor.as_deref()).await,
            ProviderKind::Braintree        => sync_braintree(&conn, payload.cursor.as_deref()).await,
            ProviderKind::CoinbaseCommerce => sync_coinbase(&conn, payload.cursor.as_deref()).await,
            ProviderKind::CoinbasePrime    => sync_coinbase(&conn, payload.cursor.as_deref()).await,
            ProviderKind::PlaidBank        => sync_plaid(&conn, payload.cursor.as_deref()).await,
            ProviderKind::SwiftWire        => sync_swift(&conn, payload.cursor.as_deref()).await,
            ProviderKind::AchDirect        => sync_ach(&conn, payload.cursor.as_deref()).await,
            ProviderKind::Wise             => sync_wise(&conn, payload.cursor.as_deref()).await,
            ProviderKind::SolanaWallet     => sync_solana(&conn, payload.cursor.as_deref()).await,
        };

        // Always release the lease before returning.
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
                        "provider": provider_tag(conn.provider),
                        "new_postings": summary.new_postings,
                        "events_processed": summary.events_processed,
                        "next_cursor": summary.next_cursor,
                    }),
                ))
            }
            Err(e) => {
                let err = e.to_string();
                let _ = self.connections.mark_failed(conn.id, &err).await;
                Err(AppError::Provider {
                    provider: provider_tag(conn.provider).to_string(),
                    message: err,
                })
            }
        }
    }
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

fn provider_tag(p: ProviderKind) -> &'static str {
    p.tag()
}

// -- Per-provider sync stubs -------------------------------------------------
//
// Stable contract: each returns `SyncSummary { new_postings, events_processed,
// next_cursor, summary }` on success, or `AppError` on failure.
//
// Real impls land per provider. Stripe is the reference target for the next
// turn (`balance_transactions` poller normalized into double-entry postings
// against `clearing/stripe/<external_account_id>`).

pub struct SyncSummary {
    pub new_postings: i64,
    pub events_processed: i64,
    pub next_cursor: Option<String>,
    pub summary: String,
}

use crate::providers::connection::ProviderConnection;

async fn sync_stripe(
    _conn: &ProviderConnection,
    _cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    // TODO(next turn): Stripe balance_transactions poller.
    not_implemented("stripe")
}

async fn sync_paypal(
    _conn: &ProviderConnection,
    _cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    not_implemented("paypal")
}

async fn sync_braintree(
    _conn: &ProviderConnection,
    _cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    not_implemented("braintree")
}

async fn sync_coinbase(
    _conn: &ProviderConnection,
    _cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    not_implemented("coinbase")
}

async fn sync_plaid(
    _conn: &ProviderConnection,
    _cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    not_implemented("plaid")
}

async fn sync_swift(
    _conn: &ProviderConnection,
    _cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    not_implemented("swift")
}

async fn sync_ach(
    _conn: &ProviderConnection,
    _cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    not_implemented("ach")
}

async fn sync_wise(
    _conn: &ProviderConnection,
    _cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    not_implemented("wise")
}

async fn sync_solana(
    _conn: &ProviderConnection,
    _cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    // Solana provider is read-only by design (observer model). The "sync"
    // here is reading recent USDC/SOL transactions for tracked wallets and
    // recording them in the ledger.
    not_implemented("solana_read_only")
}

fn not_implemented(provider: &str) -> AppResult<SyncSummary> {
    Err(AppError::Provider {
        provider: provider.to_string(),
        message: format!("{provider} sync not implemented yet; webhook events still flow"),
    })
}
