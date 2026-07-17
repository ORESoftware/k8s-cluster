//! External customer-scoped snapshot locks backed by fiducia.cloud.
//!
//! Billing-state reads and every ledger write that touches a customer account
//! contend on the same atomic Fiducia union-lock keys. PostgreSQL remains the
//! ledger source of truth and still uses transaction-scoped advisory locks for
//! idempotency; Fiducia provides cross-service leases and fencing.

use std::collections::BTreeSet;

use serde::Serialize;
use tokio::time::{Instant, sleep};
use uuid::Uuid;

use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::fiducia::FiduciaCoordinator;

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
    fencing_token: u64,
    resources: Vec<String>,
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
                let fencing_tokens = resources
                    .iter()
                    .map(|resource| CustomerFencingToken {
                        resource: resource.clone(),
                        token: grant.fencing_token,
                    })
                    .collect();
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
                        fencing_token: grant.fencing_token,
                        resources,
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

    pub async fn release(self) -> AppResult<()> {
        match self.inner {
            CustomerLockGuardInner::Disabled => Ok(()),
            CustomerLockGuardInner::Held(held) => {
                held.coordinator
                    .release_lock(&held.holder, held.fencing_token)
                    .await?;
                Ok(())
            }
        }
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
        let guard = broker
            .acquire_customers(Uuid::new_v4(), vec!["cus_1".into()], "test")
            .await
            .unwrap();
        let info = guard.info();
        assert!(!info.enabled);
        assert!(info.resources.is_empty());
        guard.release().await.unwrap();
    }
}
