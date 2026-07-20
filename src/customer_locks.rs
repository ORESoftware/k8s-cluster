//! External customer-scoped snapshot locks backed by fiducia.cloud.
//!
//! Billing-state reads and every ledger write that touches a customer account
//! contend on the same atomic Fiducia union-lock keys. PostgreSQL remains the
//! ledger source of truth and still uses transaction-scoped advisory locks for
//! idempotency; Fiducia provides cross-service leases and fencing.

use std::collections::BTreeSet;

use sea_orm::{ConnectionTrait, DatabaseTransaction};
use serde::Serialize;
use tokio::time::{Instant, sleep};
use uuid::Uuid;

use crate::config::Config;
use crate::db::stmt;
use crate::error::{AppError, AppResult};
use crate::fiducia::{FiduciaCoordinator, FiduciaLockGrant};

#[derive(Clone, Debug)]
pub struct CustomerLockBroker {
    coordinator: FiduciaCoordinator,
    ttl_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomerSnapshotLockInfo {
    pub enabled: bool,
    pub broker_addr: Option<String>,
    pub resources: Vec<String>,
    pub fencing_tokens: Vec<CustomerFencingToken>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CustomerFencingToken {
    pub resource: String,
    pub token: u64,
}

enum CustomerLockGuardInner {
    Disabled,
    Held(Box<HeldCustomerLock>),
}

struct HeldCustomerLock {
    coordinator: FiduciaCoordinator,
    holder: String,
    ttl_ms: u64,
    lease_expires_ms: i64,
    resources: Vec<String>,
    /// The live grant, including per-key fencing tokens. Kept whole so release
    /// and fencing both use each key's own token rather than the scalar.
    grant: FiduciaLockGrant,
    fencing_tokens: Vec<CustomerFencingToken>,
}

pub struct CustomerLockGuard {
    broker_addr: Option<String>,
    inner: CustomerLockGuardInner,
}

impl CustomerLockBroker {
    pub fn from_config(cfg: &Config, coordinator: FiduciaCoordinator) -> Self {
        Self {
            coordinator,
            ttl_ms: cfg.fiducia_lock_ttl_ms,
        }
    }

    #[cfg(test)]
    pub fn disabled() -> Self {
        Self {
            coordinator: FiduciaCoordinator::disabled(),
            ttl_ms: 60_000,
        }
    }

    pub async fn acquire_customer_uuid(
        &self,
        tenant_id: Uuid,
        customer_id: Uuid,
        reason: &str,
    ) -> AppResult<CustomerLockGuard> {
        self.acquire_customers(tenant_id, vec![customer_id.to_string()], reason)
            .await
    }

    pub async fn acquire_customers(
        &self,
        tenant_id: Uuid,
        customer_ids: Vec<String>,
        reason: &str,
    ) -> AppResult<CustomerLockGuard> {
        let targets = normalized_customer_ids(customer_ids)?;
        if !self.coordinator.enabled() || targets.is_empty() {
            return Ok(CustomerLockGuard::disabled());
        }

        let resources = targets
            .into_iter()
            .map(|customer_id| customer_lock_key(tenant_id, &customer_id))
            .collect::<Vec<_>>();
        let holder = format!(
            "billing-customer:{}:{}:{}",
            tenant_id,
            std::process::id(),
            Uuid::new_v4()
        );
        let deadline = Instant::now() + self.coordinator.request_timeout();

        loop {
            if let Some(grant) = self
                .coordinator
                .acquire_lock(resources.clone(), &holder, self.ttl_ms)
                .await?
            {
                // Each key carries its own token in a union grant. Stamping the
                // scalar onto every resource (as this once did) misreports the
                // fence and strands member keys on release.
                let mut fencing_tokens = Vec::with_capacity(resources.len());
                for resource in &resources {
                    let Some(token) = grant.token_for(resource) else {
                        // The grant does not actually cover a key we asked for.
                        // Give the whole grant back rather than proceeding with
                        // an unfenced resource.
                        let _ = self.coordinator.release_grant(&holder, &grant).await;
                        return Err(AppError::Provider {
                            provider: "fiducia.cloud".into(),
                            message: format!(
                                "Fiducia grant omitted a fencing token for '{resource}'"
                            ),
                        });
                    };
                    fencing_tokens.push(CustomerFencingToken {
                        resource: resource.clone(),
                        token,
                    });
                }
                tracing::debug!(
                    reason,
                    holder,
                    fencing_token = grant.fencing_token,
                    lease_expires_ms = grant.lease_expires_ms,
                    resources = ?resources,
                    "acquired Fiducia customer lock"
                );
                return Ok(CustomerLockGuard {
                    broker_addr: Some(self.coordinator.base_url().to_string()),
                    inner: CustomerLockGuardInner::Held(Box::new(HeldCustomerLock {
                        coordinator: self.coordinator.clone(),
                        holder,
                        ttl_ms: self.ttl_ms,
                        lease_expires_ms: grant.lease_expires_ms,
                        resources,
                        grant,
                        fencing_tokens,
                    })),
                });
            }

            let now = Instant::now();
            if now >= deadline {
                return Err(AppError::Provider {
                    provider: "fiducia.cloud".into(),
                    message: format!(
                        "customer lock acquisition timed out for {reason} after {}ms",
                        self.coordinator.request_timeout().as_millis()
                    ),
                });
            }
            // Try-lock polling deliberately avoids Fiducia's durable wait queue:
            // if this caller times out, it must not be granted a lease later.
            sleep((deadline - now).min(std::time::Duration::from_millis(50))).await;
        }
    }
}

impl CustomerLockGuard {
    fn disabled() -> Self {
        Self {
            broker_addr: None,
            inner: CustomerLockGuardInner::Disabled,
        }
    }

    pub fn info(&self) -> CustomerSnapshotLockInfo {
        match &self.inner {
            CustomerLockGuardInner::Disabled => CustomerSnapshotLockInfo {
                enabled: false,
                broker_addr: None,
                resources: Vec::new(),
                fencing_tokens: Vec::new(),
            },
            CustomerLockGuardInner::Held(held) => CustomerSnapshotLockInfo {
                enabled: true,
                broker_addr: self.broker_addr.clone(),
                resources: held.resources.clone(),
                fencing_tokens: held.fencing_tokens.clone(),
            },
        }
    }

    /// Reacquire the exact same holder/key set immediately before a protected
    /// commit or snapshot handoff. Fiducia preserves the fencing token and
    /// extends the lease; a missing or different grant proves this guard lost
    /// authority and the caller must roll its database transaction back.
    pub async fn ensure_valid(&mut self) -> AppResult<()> {
        let CustomerLockGuardInner::Held(held) = &mut self.inner else {
            return Ok(());
        };
        let grant = held
            .coordinator
            .acquire_lock(held.resources.clone(), &held.holder, held.ttl_ms)
            .await?
            .ok_or_else(|| lost_lock_error("the lock is now held by another caller"))?;
        if grant.fencing_token != held.grant.fencing_token
            || grant.keys != held.resources
            || grant.per_key_tokens() != held.grant.per_key_tokens()
        {
            // This re-acquire created a *new* grant under our holder id whose
            // token we are about to reject. Releasing the old token would not
            // reach it, so hand the new grant back explicitly — otherwise it
            // sits held until the lease TTL expires.
            if let Err(err) = held.coordinator.release_grant(&held.holder, &grant).await {
                tracing::error!(
                    error = %err,
                    holder = held.holder,
                    "failed to release the superseding Fiducia grant after a lost fence"
                );
            }
            return Err(lost_lock_error(
                "the renewed grant did not match the original fencing token and resources",
            ));
        }
        held.lease_expires_ms = grant.lease_expires_ms;
        tracing::debug!(
            holder = held.holder,
            fencing_token = held.grant.fencing_token,
            lease_expires_ms = held.lease_expires_ms,
            resources = ?held.resources,
            "renewed Fiducia customer lock before protected handoff"
        );
        Ok(())
    }

    /// Assert this guard's fencing tokens against the durable fence table,
    /// inside the caller's transaction, immediately before the protected write
    /// commits.
    ///
    /// This is what makes the lease non-advisory. `ensure_valid` asks fiducia
    /// over the network, which is a TOCTOU — the answer is stale the moment it
    /// returns and cannot be made atomic with COMMIT. The fence row lives in
    /// the same database as the write, so a stale token loses deterministically
    /// even if this process still believes it holds the lease (expired TTL,
    /// cross-cluster partition, paused VM).
    ///
    /// Accepts a strictly higher token (a new grant) or the same token from the
    /// same holder (this grant doing a second write in its own term). A lower
    /// token, or an equal token from a different holder, is fenced out.
    pub async fn fence(&self, tx: &DatabaseTransaction, tenant_id: Uuid) -> AppResult<()> {
        let CustomerLockGuardInner::Held(held) = &self.inner else {
            return Ok(());
        };
        for fence in &held.fencing_tokens {
            let token = i64::try_from(fence.token).map_err(|_| {
                lost_lock_error("fencing token does not fit the durable fence column")
            })?;
            let advanced = tx
                .query_one(stmt(
                    r#"
                INSERT INTO fiducia_fences (tenant_id, fence_key, fencing_token, holder)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (tenant_id, fence_key) DO UPDATE
                    SET fencing_token = EXCLUDED.fencing_token,
                        holder        = EXCLUDED.holder,
                        observed_at   = now()
                    WHERE fiducia_fences.fencing_token < EXCLUDED.fencing_token
                       OR (fiducia_fences.fencing_token = EXCLUDED.fencing_token
                           AND fiducia_fences.holder IS NOT DISTINCT FROM EXCLUDED.holder)
                RETURNING fencing_token
                "#,
                    [
                        tenant_id.into(),
                        fence.resource.clone().into(),
                        token.into(),
                        held.holder.clone().into(),
                    ],
                ))
                .await?;

            if advanced.is_none() {
                // The ON CONFLICT ... WHERE predicate failed: a strictly higher
                // token has already committed against this key. We were fenced.
                return Err(lost_lock_error(&format!(
                    "a newer fencing token has already committed for '{}'; \
                     this lease is stale and the transaction must roll back",
                    fence.resource
                )));
            }
        }
        Ok(())
    }

    pub async fn release(self) -> AppResult<()> {
        match self.inner {
            CustomerLockGuardInner::Disabled => Ok(()),
            CustomerLockGuardInner::Held(held) => {
                // Release every member key with its own token; the scalar alone
                // would strand the rest of a union grant until its TTL.
                held.coordinator
                    .release_grant(&held.holder, &held.grant)
                    .await?;
                Ok(())
            }
        }
    }
}

fn lost_lock_error(message: &str) -> AppError {
    AppError::Provider {
        provider: "fiducia.cloud".into(),
        message: format!("customer lock authority was lost: {message}"),
    }
}

pub fn customer_lock_targets_from_account_code(account_code: &str) -> Option<String> {
    let mut parts = account_code.split('/');
    let prefix = parts.next()?;
    let target = parts.next()?;
    if !customer_lock_prefix(prefix) || target.is_empty() {
        return None;
    }
    Some(target.to_string())
}

pub fn normalized_customer_ids(customer_ids: Vec<String>) -> AppResult<Vec<String>> {
    let mut set = BTreeSet::new();
    for raw in customer_ids {
        let value = raw.trim();
        if value.is_empty() {
            continue;
        }
        validate_customer_id(value)?;
        set.insert(value.to_string());
    }
    Ok(set.into_iter().collect())
}

fn customer_lock_key(tenant_id: Uuid, customer_id: &str) -> String {
    format!("billing:customer:{tenant_id}:{customer_id}")
}

fn customer_lock_prefix(prefix: &str) -> bool {
    matches!(
        prefix,
        "ar" | "accounts_receivable"
            | "customer"
            | "unallocated_cash"
            | "credit_memo"
            | "credit_memos"
    )
}

fn validate_customer_id(value: &str) -> AppResult<()> {
    if value.len() > 128 {
        return Err(AppError::BadRequest(
            "customer lock id must be <= 128 bytes".into(),
        ));
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(AppError::BadRequest(
            "customer lock id must not contain control characters".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_code_targets_customer_accounts() {
        assert_eq!(
            customer_lock_targets_from_account_code("ar/111/revenue"),
            Some("111".into())
        );
        assert_eq!(
            customer_lock_targets_from_account_code("unallocated_cash/cus_123"),
            Some("cus_123".into())
        );
        assert_eq!(
            customer_lock_targets_from_account_code("credit_memos/cus_123"),
            Some("cus_123".into())
        );
        assert_eq!(
            customer_lock_targets_from_account_code("clearing/stripe/acct_1"),
            None
        );
        assert_eq!(customer_lock_targets_from_account_code("ar/"), None);
    }

    #[test]
    fn normalized_customer_ids_dedupes_and_sorts() {
        let ids = normalized_customer_ids(vec![" b ".into(), "a".into(), "b".into(), " ".into()])
            .unwrap();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }

    #[tokio::test]
    async fn disabled_broker_never_connects() {
        let broker = CustomerLockBroker::disabled();
        let mut guard = broker
            .acquire_customers(Uuid::new_v4(), vec!["cus_1".into()], "test")
            .await
            .unwrap();
        let info = guard.info();
        assert!(!info.enabled);
        assert!(info.resources.is_empty());
        guard.ensure_valid().await.unwrap();
        guard.release().await.unwrap();
    }

    #[test]
    fn lost_lock_error_is_actionable_without_credentials() {
        let error = lost_lock_error("fencing token changed");
        let rendered = error.to_string();
        assert!(rendered.contains("fiducia.cloud"));
        assert!(rendered.contains("authority was lost"));
        assert!(rendered.contains("fencing token changed"));
    }
}
