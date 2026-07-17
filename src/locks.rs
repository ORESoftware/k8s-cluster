//! Tenant-scoped renewable leases.
//!
//! Fiducia leader-election leases are the cross-service authority when enabled:
//! campaign = acquire, renew = extend, resign = release. PostgreSQL keeps the
//! durable billing-facing mirror and append-only audit history. Every mirror
//! mutation runs in a transaction under a transaction-scoped advisory lock;
//! network calls are deliberately outside those transactions.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row, Transaction};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::fiducia::FiduciaCoordinator;
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
    fiducia: FiduciaCoordinator,
}

impl LockService {
    pub fn new(pool: PgPool, fiducia: FiduciaCoordinator) -> Self {
        Self { pool, fiducia }
    }

    /// Acquire a renewable lease. Fiducia wins the cross-service race first;
    /// the PostgreSQL mirror then commits atomically with its audit event. A DB
    /// failure is compensated by resigning the newly won Fiducia lease.
    pub async fn acquire(
        &self,
        tenant_id: Uuid,
        region: Region,
        actor: Option<&str>,
        req: AcquireRequest,
    ) -> AppResult<Lease> {
        validate_ttl(req.ttl_seconds)?;
        validate_resource_key(&req.resource)?;

        let token = Uuid::new_v4();
        let now = Utc::now();
        let name = fiducia_lease_name(tenant_id, &req.resource);
        let candidate = fiducia_candidate(tenant_id, token);
        let ttl_ms = ttl_ms(req.ttl_seconds);

        let remote = if self.fiducia.enabled() {
            let mut metadata = BTreeMap::new();
            metadata.insert("service".into(), "billing-server-rs".into());
            metadata.insert("tenant_id".into(), tenant_id.to_string());
            metadata.insert("resource".into(), req.resource.clone());
            if let Some(holder) = &req.holder {
                metadata.insert("holder".into(), holder.clone());
            }
            let Some(leadership) = self
                .fiducia
                .campaign_lease(&name, &candidate, ttl_ms, metadata)
                .await?
            else {
                return Err(AppError::Conflict(format!(
                    "lease '{}' is held in fiducia.cloud",
                    req.resource
                )));
            };
            Some(leadership)
        } else {
            None
        };

        let expires_at = match &remote {
            Some(leadership) => timestamp_millis(leadership.lease_expires_ms)?,
            None => now + Duration::seconds(req.ttl_seconds as i64),
        };
        let result = self
            .persist_acquire(tenant_id, region, actor, &req, token, now, expires_at)
            .await;

        if result.is_err() {
            if let Some(leadership) = &remote {
                self.compensate_resign(leadership, &name, &candidate, "acquire")
                    .await;
            }
        }
        result
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
        validate_resource_key(resource)?;

        // Avoid touching Fiducia for an obviously stale/foreign token.
        let local_exists = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM tenant_locks
                WHERE tenant_id = $1 AND resource_key = $2
                  AND lease_token = $3 AND expires_at > now()
            )
            "#,
        )
        .bind(tenant_id)
        .bind(resource)
        .bind(req.lease_token)
        .fetch_one(&self.pool)
        .await?;
        if !local_exists {
            return Err(AppError::Conflict(
                "lease not found, token mismatch, or already expired".into(),
            ));
        }

        let name = fiducia_lease_name(tenant_id, resource);
        let candidate = fiducia_candidate(tenant_id, req.lease_token);
        let ttl_ms = ttl_ms(req.ttl_seconds);
        let remote = if self.fiducia.enabled() {
            let leadership = self.current_remote_lease(&name, &candidate).await?;
            let fencing_token = u64::try_from(leadership.fencing_token)
                .map_err(|_| fiducia_protocol_error("negative lease fencing token"))?;
            let Some(renewed) = self
                .fiducia
                .renew_lease(&name, &candidate, fencing_token, ttl_ms)
                .await?
            else {
                return Err(AppError::Conflict(
                    "Fiducia lease expired, changed holder, or was already released".into(),
                ));
            };
            Some(renewed)
        } else {
            None
        };

        let new_expires = match &remote {
            Some(leadership) => timestamp_millis(leadership.lease_expires_ms)?,
            None => Utc::now() + Duration::seconds(req.ttl_seconds as i64),
        };
        let result = self
            .persist_renew(tenant_id, region, actor, resource, &req, new_expires)
            .await;

        if result.is_err() {
            // A remotely renewed lease without a durable local mirror is not a
            // valid billing lease. Resign it so the failure is fail-closed and
            // bounded instead of leaving an invisible holder.
            if let Some(leadership) = &remote {
                self.compensate_resign(leadership, &name, &candidate, "renew")
                    .await;
            }
        }
        result
    }

    /// Release a lease. Token mismatch remains a no-op success. PostgreSQL is
    /// committed first; if the subsequent Fiducia resign fails, its TTL bounds
    /// the stale remote hold and the caller receives an explicit provider error.
    pub async fn release(
        &self,
        tenant_id: Uuid,
        region: Region,
        actor: Option<&str>,
        resource: &str,
        req: ReleaseRequest,
    ) -> AppResult<()> {
        validate_resource_key(resource)?;
        let local_exists = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM tenant_locks
                WHERE tenant_id = $1 AND resource_key = $2 AND lease_token = $3
            )
            "#,
        )
        .bind(tenant_id)
        .bind(resource)
        .bind(req.lease_token)
        .fetch_one(&self.pool)
        .await?;
        if !local_exists {
            return Ok(());
        }

        let name = fiducia_lease_name(tenant_id, resource);
        let candidate = fiducia_candidate(tenant_id, req.lease_token);
        let remote = if self.fiducia.enabled() {
            let observed = self.fiducia.get_lease(&name).await?;
            observed
                .leadership
                .filter(|leadership| leadership.leader == candidate)
        } else {
            None
        };

        let deleted = self
            .persist_release(tenant_id, region, actor, resource, req.lease_token)
            .await?;
        if !deleted {
            return Ok(());
        }
        if let Some(leadership) = remote {
            let fencing_token = u64::try_from(leadership.fencing_token)
                .map_err(|_| fiducia_protocol_error("negative lease fencing token"))?;
            self.fiducia
                .resign_lease(&name, &candidate, fencing_token)
                .await?;
        }
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

        rows.iter()
            .map(|row| {
                Ok(LeaseRow {
                    resource_key: row.try_get("resource_key")?,
                    lease_token: row.try_get("lease_token")?,
                    holder: row.try_get("holder")?,
                    acquired_at: row.try_get("acquired_at")?,
                    expires_at: row.try_get("expires_at")?,
                    expired: row.try_get("expired")?,
                })
            })
            .collect()
    }

    /// Atomically delete old mirrors and write their expiry audit events. The
    /// previous two-autocommit implementation could record an expiry without
    /// deleting it (or delete rows inserted between statements).
    pub async fn sweep_expired(&self, keep_for_hours: i64) -> AppResult<u64> {
        let cutoff = Utc::now() - Duration::hours(keep_for_hours.max(0));
        let count: i64 = sqlx::query_scalar(
            r#"
            WITH expired AS (
                DELETE FROM tenant_locks
                WHERE expires_at <= $1
                RETURNING tenant_id, shard_key, resource_key, lease_token, holder
            ), audited AS (
                INSERT INTO tenant_lock_events
                    (tenant_id, shard_key, resource_key, lease_token, kind, holder)
                SELECT tenant_id, shard_key, resource_key, lease_token,
                       'expire'::lock_event_kind, holder
                FROM expired
                RETURNING 1
            )
            SELECT COUNT(*)::BIGINT FROM audited
            "#,
        )
        .bind(cutoff)
        .fetch_one(&self.pool)
        .await?;
        Ok(count as u64)
    }

    #[allow(clippy::too_many_arguments)]
    async fn persist_acquire(
        &self,
        tenant_id: Uuid,
        region: Region,
        actor: Option<&str>,
        req: &AcquireRequest,
        token: Uuid,
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> AppResult<Lease> {
        let shard = ShardKey::derive(tenant_id, region).0;
        let mut tx = self.pool.begin().await?;
        advisory_lock(&mut tx, tenant_id, &req.resource).await?;

        let previous = sqlx::query(
            r#"
            SELECT holder, acquired_at, expires_at
            FROM tenant_locks
            WHERE tenant_id = $1 AND resource_key = $2
            FOR UPDATE
            "#,
        )
        .bind(tenant_id)
        .bind(&req.resource)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some(existing) = &previous {
            let existing_expires: DateTime<Utc> = existing.try_get("expires_at")?;
            if existing_expires > now {
                let holder: Option<String> = existing.try_get("holder")?;
                tx.commit().await?;
                return Err(AppError::Conflict(format!(
                    "lock '{}' held by {} until {}",
                    req.resource,
                    holder.as_deref().unwrap_or("(unknown)"),
                    existing_expires.to_rfc3339()
                )));
            }
        }
        let kind = if previous.is_some() {
            "preempt"
        } else {
            "acquire"
        };

        let row = sqlx::query(
            r#"
            INSERT INTO tenant_locks
                (tenant_id, shard_key, resource_key, lease_token, holder,
                 acquired_at, expires_at, metadata)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (tenant_id, resource_key) DO UPDATE
                SET shard_key   = EXCLUDED.shard_key,
                    lease_token = EXCLUDED.lease_token,
                    holder      = EXCLUDED.holder,
                    acquired_at = EXCLUDED.acquired_at,
                    expires_at  = EXCLUDED.expires_at,
                    metadata    = EXCLUDED.metadata
            RETURNING lease_token, holder, acquired_at, expires_at, metadata
            "#,
        )
        .bind(tenant_id)
        .bind(shard)
        .bind(&req.resource)
        .bind(token)
        .bind(&req.holder)
        .bind(now)
        .bind(expires_at)
        .bind(&req.metadata)
        .fetch_one(&mut *tx)
        .await?;

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
            resource_key: req.resource.clone(),
            lease_token: token,
            holder: row.try_get("holder")?,
            acquired_at: row.try_get("acquired_at")?,
            expires_at: row.try_get("expires_at")?,
            metadata: row.try_get("metadata")?,
        })
    }

    async fn persist_renew(
        &self,
        tenant_id: Uuid,
        region: Region,
        actor: Option<&str>,
        resource: &str,
        req: &RenewRequest,
        new_expires: DateTime<Utc>,
    ) -> AppResult<Lease> {
        let shard = ShardKey::derive(tenant_id, region).0;
        let mut tx = self.pool.begin().await?;
        advisory_lock(&mut tx, tenant_id, resource).await?;

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

        let holder: Option<String> = row.try_get("holder")?;
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
        .bind(&holder)
        .bind(actor)
        .bind(req.ttl_seconds as i32)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(Lease {
            tenant_id,
            resource_key: resource.into(),
            lease_token: req.lease_token,
            holder,
            acquired_at: row.try_get("acquired_at")?,
            expires_at: row.try_get("expires_at")?,
            metadata: row.try_get("metadata")?,
        })
    }

    async fn persist_release(
        &self,
        tenant_id: Uuid,
        region: Region,
        actor: Option<&str>,
        resource: &str,
        lease_token: Uuid,
    ) -> AppResult<bool> {
        let shard = ShardKey::derive(tenant_id, region).0;
        let mut tx = self.pool.begin().await?;
        advisory_lock(&mut tx, tenant_id, resource).await?;
        let deleted = sqlx::query(
            r#"
            DELETE FROM tenant_locks
            WHERE tenant_id = $1 AND resource_key = $2 AND lease_token = $3
            "#,
        )
        .bind(tenant_id)
        .bind(resource)
        .bind(lease_token)
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
            .bind(lease_token)
            .bind(actor)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(deleted > 0)
    }

    async fn current_remote_lease(
        &self,
        name: &str,
        candidate: &str,
    ) -> AppResult<fiducia_interfaces::Leadership> {
        let observed = self.fiducia.get_lease(name).await?;
        let Some(leadership) = observed.leadership else {
            return Err(AppError::Conflict("Fiducia lease is not held".into()));
        };
        if leadership.leader != candidate {
            return Err(AppError::Conflict(
                "Fiducia lease fencing token belongs to another holder".into(),
            ));
        }
        Ok(leadership)
    }

    async fn compensate_resign(
        &self,
        leadership: &fiducia_interfaces::Leadership,
        name: &str,
        candidate: &str,
        operation: &str,
    ) {
        let fencing_token = match u64::try_from(leadership.fencing_token) {
            Ok(token) => token,
            Err(_) => {
                tracing::error!(lease = name, operation, "negative Fiducia fencing token");
                return;
            }
        };
        if let Err(err) = self
            .fiducia
            .resign_lease(name, candidate, fencing_token)
            .await
        {
            tracing::error!(
                error = %err,
                lease = name,
                operation,
                "failed to compensate Fiducia lease after Postgres failure"
            );
        }
    }
}

async fn advisory_lock(
    tx: &mut Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    resource: &str,
) -> AppResult<()> {
    // The one-bigint advisory-lock namespace is independent from the ledger's
    // two-int idempotency namespace. PostgreSQL releases it automatically on
    // commit/rollback, including early error paths.
    let identity = format!("billing-lease:{tenant_id}:{resource}");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(identity)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn fiducia_lease_name(tenant_id: Uuid, resource: &str) -> String {
    format!("billing/tenant/{tenant_id}/{resource}")
}

fn fiducia_candidate(tenant_id: Uuid, lease_token: Uuid) -> String {
    format!("billing-server-rs:{tenant_id}:{lease_token}")
}

fn ttl_ms(ttl_seconds: u32) -> u64 {
    u64::from(ttl_seconds) * 1_000
}

fn timestamp_millis(value: i64) -> AppResult<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp_millis(value)
        .ok_or_else(|| fiducia_protocol_error("lease expiry is outside Chrono's timestamp range"))
}

fn fiducia_protocol_error(message: &str) -> AppError {
    AppError::Provider {
        provider: "fiducia.cloud".into(),
        message: message.into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fiducia_names_are_tenant_scoped_and_stable() {
        let tenant = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
        let token = Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap();
        assert_eq!(
            fiducia_lease_name(tenant, "invoice/42"),
            "billing/tenant/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/invoice/42"
        );
        assert_eq!(
            fiducia_candidate(tenant, token),
            "billing-server-rs:aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa:bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"
        );
    }

    #[test]
    fn ttl_validation_bounds_fiducia_leases() {
        assert!(validate_ttl(0).is_err());
        assert!(validate_ttl(1).is_ok());
        assert!(validate_ttl(86_400).is_ok());
        assert!(validate_ttl(86_401).is_err());
    }

    #[test]
    fn resource_validation_rejects_control_characters() {
        assert!(validate_resource_key("invoice/42").is_ok());
        assert!(validate_resource_key("invoice\n42").is_err());
    }
}
