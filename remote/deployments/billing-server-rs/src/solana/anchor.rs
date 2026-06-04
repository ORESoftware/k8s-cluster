//! Anchor service — periodically publishes Merkle roots of new postings to
//! Solana, then records the (tx_signature, slot) in the `anchors` table.
//!
//! Cadence is driven by two knobs (per tenant, configurable later):
//!   - max postings since last anchor (default 10_000)
//!   - max time since last anchor (default 60 seconds)
//!
//! Plus per-transaction `commit_critical: true` -> anchor immediately.
//!
//! Crucially: anchoring is NEVER on the critical path of `post_transaction`.
//! Posting always returns to the caller after the PG insert; the anchor job
//! runs in a background tokio task.

use chrono::Utc;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::AppResult;

use super::client::SolanaClient;
use super::merkle::{merkle_root, posting_leaf_hash};

pub struct AnchorService {
    pool: PgPool,
    solana: SolanaClient,
    pub max_batch: i64,
    pub max_age_seconds: i64,
}

#[derive(Clone, Debug)]
pub struct AnchorResult {
    pub anchor_id: i64,
    pub tenant_id: Uuid,
    pub from_posting_id: i64,
    pub to_posting_id: i64,
    pub posting_count: i64,
    pub merkle_root_hex: String,
    pub tx_signature: Option<String>,
    pub slot: Option<i64>,
}

impl AnchorService {
    pub fn new(pool: PgPool, solana: SolanaClient) -> Self {
        Self {
            pool,
            solana,
            max_batch: 10_000,
            max_age_seconds: 60,
        }
    }

    /// Compute and submit the next anchor batch for one tenant.
    /// Returns None if there are no new postings to anchor.
    pub async fn anchor_tenant_once(&self, tenant_id: Uuid) -> AppResult<Option<AnchorResult>> {
        let last_to_id: Option<i64> =
            sqlx::query_scalar(r#"SELECT MAX(to_posting_id) FROM anchors WHERE tenant_id = $1"#)
                .bind(tenant_id)
                .fetch_one(&self.pool)
                .await?;
        let start_after = last_to_id.unwrap_or(0);

        let rows = sqlx::query(
            r#"
            SELECT p.id, p.transaction_id, p.account_id,
                   p.direction::TEXT AS direction_t,
                   p.amount_minor::TEXT AS amount_t,
                   p.currency,
                   p.source, p.source_event_id,
                   p.posted_at,
                   EXTRACT(EPOCH FROM p.posted_at)::BIGINT * 1000 AS posted_unix_ms
            FROM postings p
            WHERE p.tenant_id = $1 AND p.id > $2
            ORDER BY p.id ASC
            LIMIT $3
            "#,
        )
        .bind(tenant_id)
        .bind(start_after)
        .bind(self.max_batch)
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok(None);
        }

        // Build leaves
        let mut leaves: Vec<[u8; 32]> = Vec::with_capacity(rows.len());
        let from_id: i64 = rows.first().unwrap().try_get("id")?;
        let mut to_id: i64 = from_id;
        for row in &rows {
            let posting_id: i64 = row.try_get("id")?;
            to_id = posting_id;
            let txid: Uuid = row.try_get("transaction_id")?;
            let acct_id: Uuid = row.try_get("account_id")?;
            let dir: String = row.try_get("direction_t")?;
            let amt: String = row.try_get("amount_t")?;
            let cur: String = row.try_get("currency")?;
            let src: String = row.try_get("source")?;
            let src_evt: String = row.try_get("source_event_id")?;
            let posted_unix_ms: i64 = row.try_get("posted_unix_ms")?;

            leaves.push(posting_leaf_hash(
                posting_id,
                txid,
                acct_id,
                &dir,
                &amt,
                &cur,
                &src,
                &src_evt,
                posted_unix_ms,
            ));
        }

        let root = merkle_root(&leaves);
        let count = leaves.len() as i64;

        // Submit on-chain.
        //
        // TODO(real impl): build a Solana transaction containing a Memo
        // instruction (program id MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr)
        // with payload = serde_json::to_vec(&{tenant_id, from_id, to_id,
        // count, root_hex}). Sign with the anchoring keypair from config,
        // send via sendTransaction, then watch for finalized via getSignature-
        // Statuses. For now we INSERT the anchor row without signature/slot
        // and a background "finalizer" job (not yet written) would fill it in.
        let _ = &self.solana; // silence unused-field warning until impl lands
        let tx_signature: Option<String> = None;
        let slot: Option<i64> = None;

        let root_hex = hex::encode(root);

        let anchor_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO anchors
                (tenant_id, shard_key, from_posting_id, to_posting_id,
                 posting_count, merkle_root, chain, tx_signature, slot, submitted_at)
            SELECT $1, p.shard_key, $2, $3, $4, $5, 'solana', $6, $7, now()
            FROM postings p
            WHERE p.id = $2
            ON CONFLICT (tenant_id, from_posting_id, to_posting_id)
                DO UPDATE SET submitted_at = now()
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(from_id)
        .bind(to_id)
        .bind(count)
        .bind(root.as_slice())
        .bind(&tx_signature)
        .bind(slot)
        .fetch_one(&self.pool)
        .await?;

        Ok(Some(AnchorResult {
            anchor_id,
            tenant_id,
            from_posting_id: from_id,
            to_posting_id: to_id,
            posting_count: count,
            merkle_root_hex: root_hex,
            tx_signature,
            slot,
        }))
    }

    /// Run the anchor loop forever. Sleeps `max_age_seconds` between sweeps.
    pub async fn run_forever(self: std::sync::Arc<Self>) {
        let sleep = std::time::Duration::from_secs(self.max_age_seconds as u64);
        loop {
            if let Err(e) = self.sweep_all_tenants().await {
                tracing::error!(error = %e, "anchor sweep failed");
            }
            tokio::time::sleep(sleep).await;
        }
    }

    pub async fn sweep_all_tenants(&self) -> AppResult<()> {
        let tenants: Vec<Uuid> =
            sqlx::query_scalar(r#"SELECT id FROM tenants WHERE status = 'active'"#)
                .fetch_all(&self.pool)
                .await?;

        for tid in tenants {
            match self.anchor_tenant_once(tid).await {
                Ok(Some(res)) => {
                    tracing::info!(
                        tenant = %tid,
                        anchor_id = res.anchor_id,
                        count = res.posting_count,
                        root = %res.merkle_root_hex,
                        "anchored postings"
                    );
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(tenant = %tid, error = %e, "anchor failed"),
            }
        }
        let _ = Utc::now();
        Ok(())
    }
}
