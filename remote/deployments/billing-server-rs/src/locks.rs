//! Tenant-scoped lease service.
//!
//! Semantics:
//!   - `acquire`: atomically take a lease on `(tenant_id, resource_key)`.
//!     If an existing lease is unexpired, return 409 Conflict. If expired,
//!     preempt it (recorded as `preempt` in the audit log).
//!   - `renew`: extend the TTL. Requires the original `lease_token`. Fails
//!     if the lease was already preempted/expired; the caller must `acquire`
//!     again and re-validate whatever work they had in flight.
//!   - `release`: drop the lease. Token mismatch is a no-op success (the
//!     caller's intent — "this lock is gone for me" — is satisfied).
//!
//! Backed by Postgres; HA == PG HA. No separate distributed lock service.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::shard::{Region, ShardKey};

#[derive(Clone, Debug, Serialize)]
pub struct Lease {
    pub tenant_id: Uuid,
    pub resource_key: String,
    pub lease_token: Uuid,
    pub holder: Option<String>,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AcquireRequest {
    pub resource: String,
    pub ttl_seconds: u32,
    pub holder: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RenewRequest {
    pub lease_token: Uuid,
    pub ttl_seconds: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReleaseRequest {
    pub lease_token: Uuid,
}

#[derive(Clone, Debug, Serialize)]
pub struct LeaseRow {
    pub resource_key: String,
    pub lease_token: Uuid,
    pub holder: Option<String>,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub expired: bool,
}

#[derive(Clone)]
pub struct LockService {
    pool: PgPool,
}

impl LockService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Atomic acquire with TTL. Returns 409 (Conflict) if held + not expired.
    pub async fn acquire(
        &self,
        tenant_id: Uuid,
        region: Region,
        actor: Option<&str>,
        req: AcquireRequest,
    ) -> AppResult<Lease> {
        validate_ttl(req.ttl_seconds)?;
        validate_resource_key(&req.resource)?;

        let shard = ShardKey::derive(tenant_id, region).0;
        let token = Uuid::new_v4();
        let now = Utc::now();
        let expires = now + Duration::seconds(req.ttl_seconds as i64);

        let mut tx = self.pool.begin().await?;

        // Try INSERT; if a row exists, evaluate its expiry. If expired, preempt
        // it atomically. If not, return 409.
        let row = sqlx::query(
            r#"
            WITH ins AS (
                INSERT INTO tenant_locks
                    (tenant_id, shard_key, resource_key, lease_token, holder,
                     acquired_at, expires_at, metadata)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT (tenant_id, resource_key) DO UPDATE
                    SET lease_token = EXCLUDED.lease_token,
                        holder      = EXCLUDED.holder,
                        acquired_at = EXCLUDED.acquired_at,
                        expires_at  = EXCLUDED.expires_at,
                        metadata    = EXCLUDED.metadata
                    WHERE tenant_locks.expires_at <= EXCLUDED.acquired_at
                RETURNING lease_token, holder, acquired_at, expires_at, metadata
            )
            SELECT lease_token, holder, acquired_at, expires_at, metadata
            FROM ins
            "#,
        )
        .bind(tenant_id)
        .bind(shard)
        .bind(&req.resource)
        .bind(token)
        .bind(&req.holder)
        .bind(now)
        .bind(expires)
        .bind(&req.metadata)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(row) = row else {
            // Lock exists and is still valid for someone else.
            let existing = sqlx::query(
                r#"
                SELECT holder, acquired_at, expires_at
                FROM tenant_locks
                WHERE tenant_id = $1 AND resource_key = $2
                "#,
            )
            .bind(tenant_id)
            .bind(&req.resource)
            .fetch_one(&mut *tx)
            .await?;
            let holder: Option<String> = existing.try_get("holder")?;
            let exp: DateTime<Utc> = existing.try_get("expires_at")?;
            tx.commit().await?;
            return Err(AppError::Conflict(format!(
                "lock '{}' held by {} until {}",
                req.resource,
                holder.as_deref().unwrap_or("(unknown)"),
                exp.to_rfc3339()
            )));
        };

        // Audit. Distinguish acquire from preempt by checking whether the
        // returned acquired_at equals our `now` (we inserted) vs an earlier
        // time (we updated an expired row, which the WHERE clause permits).
        let acquired_at: DateTime<Utc> = row.try_get("acquired_at")?;
        let kind = if (acquired_at - now).num_milliseconds().abs() <= 1 {
            "acquire"
        } else {
            "preempt"
        };
        sqlx::query(
            r#"
            INSERT INTO tenant_lock_events
                (tenant_id, shard_key, resource_key, lease_token, kind,
                 holder, actor, ttl_seconds, metadata)
            VALUES ($1, $2, $3, $4, $5::lock_event_kind, $6, $7, $8, $9)
            "#,
        )
        .bind(tenant_id)
        .bind(shard)
        .bind(&req.resource)
        .bind(token)
        .bind(kind)
        .bind(&req.holder)
        .bind(actor)
        .bind(req.ttl_seconds as i32)
        .bind(&req.metadata)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(Lease {
            tenant_id,
            resource_key: req.resource,
            lease_token: token,
            holder: row.try_get("holder")?,
            acquired_at,
            expires_at: row.try_get("expires_at")?,
            metadata: row.try_get("metadata")?,
        })
    }

    pub async fn renew(
        &self,
        tenant_id: Uuid,
        region: Region,
        actor: Option<&str>,
        resource: &str,
        req: RenewRequest,
    ) -> AppResult<Lease> {
        validate_ttl(req.ttl_seconds)?;

        let shard = ShardKey::derive(tenant_id, region).0;
        let now = Utc::now();
        let new_expires = now + Duration::seconds(req.ttl_seconds as i64);

        let mut tx = self.pool.begin().await?;

        let row = sqlx::query(
            r#"
            UPDATE tenant_locks
            SET expires_at = $4
            WHERE tenant_id = $1
              AND resource_key = $2
              AND lease_token = $3
              AND expires_at > now()
            RETURNING lease_token, holder, acquired_at, expires_at, metadata
            "#,
        )
        .bind(tenant_id)
        .bind(resource)
        .bind(req.lease_token)
        .bind(new_expires)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(row) = row else {
            tx.commit().await?;
            return Err(AppError::Conflict(
                "lease not found, token mismatch, or already expired".into(),
            ));
        };

        sqlx::query(
            r#"
            INSERT INTO tenant_lock_events
                (tenant_id, shard_key, resource_key, lease_token, kind,
                 holder, actor, ttl_seconds)
            VALUES ($1, $2, $3, $4, 'renew'::lock_event_kind, $5, $6, $7)
            "#,
        )
        .bind(tenant_id)
        .bind(shard)
        .bind(resource)
        .bind(req.lease_token)
        .bind::<Option<String>>(row.try_get("holder")?)
        .bind(actor)
        .bind(req.ttl_seconds as i32)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(Lease {
            tenant_id,
            resource_key: resource.into(),
            lease_token: req.lease_token,
            holder: row.try_get("holder")?,
            acquired_at: row.try_get("acquired_at")?,
            expires_at: row.try_get("expires_at")?,
            metadata: row.try_get("metadata")?,
        })
    }

    /// Release a lease. Token mismatch is a no-op success.
    pub async fn release(
        &self,
        tenant_id: Uuid,
        region: Region,
        actor: Option<&str>,
        resource: &str,
        req: ReleaseRequest,
    ) -> AppResult<()> {
        let shard = ShardKey::derive(tenant_id, region).0;
        let mut tx = self.pool.begin().await?;

        let deleted = sqlx::query(
            r#"
            DELETE FROM tenant_locks
            WHERE tenant_id = $1 AND resource_key = $2 AND lease_token = $3
            "#,
        )
        .bind(tenant_id)
        .bind(resource)
        .bind(req.lease_token)
        .execute(&mut *tx)
        .await?
        .rows_affected();

        if deleted > 0 {
            sqlx::query(
                r#"
                INSERT INTO tenant_lock_events
                    (tenant_id, shard_key, resource_key, lease_token, kind, actor)
                VALUES ($1, $2, $3, $4, 'release'::lock_event_kind, $5)
                "#,
            )
            .bind(tenant_id)
            .bind(shard)
            .bind(resource)
            .bind(req.lease_token)
            .bind(actor)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn list(&self, tenant_id: Uuid) -> AppResult<Vec<LeaseRow>> {
        let rows = sqlx::query(
            r#"
            SELECT resource_key, lease_token, holder, acquired_at, expires_at,
                   (expires_at <= now()) AS expired
            FROM tenant_locks
            WHERE tenant_id = $1
            ORDER BY acquired_at DESC
            "#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|r| LeaseRow {
                resource_key: r.try_get("resource_key").unwrap_or_default(),
                lease_token: r.try_get("lease_token").unwrap_or(Uuid::nil()),
                holder: r.try_get("holder").unwrap_or(None),
                acquired_at: r.try_get("acquired_at").unwrap_or_else(|_| Utc::now()),
                expires_at: r.try_get("expires_at").unwrap_or_else(|_| Utc::now()),
                expired: r.try_get("expired").unwrap_or(false),
            })
            .collect())
    }

    /// Garbage collect leases that expired more than `keep_for_hours` ago.
    /// Recorded in the audit log as `expire` events for completeness.
    pub async fn sweep_expired(&self, keep_for_hours: i64) -> AppResult<u64> {
        let cutoff = Utc::now() - Duration::hours(keep_for_hours);

        // Record expire events first so the audit trail captures them.
        let _ = sqlx::query(
            r#"
            INSERT INTO tenant_lock_events
                (tenant_id, shard_key, resource_key, lease_token, kind, holder)
            SELECT tenant_id, shard_key, resource_key, lease_token,
                   'expire'::lock_event_kind, holder
            FROM tenant_locks
            WHERE expires_at <= $1
            "#,
        )
        .bind(cutoff)
        .execute(&self.pool)
        .await?;

        let n = sqlx::query(r#"DELETE FROM tenant_locks WHERE expires_at <= $1"#)
            .bind(cutoff)
            .execute(&self.pool)
            .await?
            .rows_affected();

        Ok(n)
    }
}

fn validate_ttl(ttl_seconds: u32) -> AppResult<()> {
    if ttl_seconds == 0 {
        return Err(AppError::BadRequest("ttl_seconds must be > 0".into()));
    }
    if ttl_seconds > 24 * 3600 {
        return Err(AppError::BadRequest(
            "ttl_seconds must be <= 86400 (24h); re-acquire if longer needed".into(),
        ));
    }
    Ok(())
}

fn validate_resource_key(key: &str) -> AppResult<()> {
    if key.is_empty() || key.len() > 256 {
        return Err(AppError::BadRequest(
            "resource key must be 1..=256 bytes".into(),
        ));
    }
    if key.chars().any(|c| c.is_control()) {
        return Err(AppError::BadRequest(
            "resource key must not contain control characters".into(),
        ));
    }
    Ok(())
}
