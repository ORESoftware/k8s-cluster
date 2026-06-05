//! External customer-scoped snapshot locks.
//!
//! The broker is `live-mutex-rs`/`dd-rust-network-mutex`, reached over the
//! live-mutex TCP protocol. This is intentionally not an in-process mutex:
//! billing-state reads and all ledger writes that touch customer accounts use
//! the same broker keys so pods agree on one customer snapshot at a time.

use std::collections::BTreeSet;
use std::time::Duration;

use live_mutex_client::{Client, ClientError, LockOpts};
use serde::Serialize;
use uuid::Uuid;

use crate::config::Config;
use crate::error::{AppError, AppResult};

#[derive(Clone, Debug)]
pub struct CustomerLockBroker {
    enabled: bool,
    addr: String,
    ttl_ms: u64,
    request_timeout: Duration,
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
    Single {
        client: Client,
        resource: String,
        lock_uuid: String,
        fencing_token: Option<u64>,
    },
    Many {
        client: Client,
        resources: Vec<String>,
        lock_uuid: String,
        fencing_tokens: Vec<CustomerFencingToken>,
    },
}

pub struct CustomerLockGuard {
    broker_addr: Option<String>,
    inner: CustomerLockGuardInner,
}

impl CustomerLockBroker {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            enabled: cfg.customer_snapshot_lock_enabled,
            addr: cfg.live_mutex_addr.clone(),
            ttl_ms: cfg.live_mutex_lock_ttl_ms,
            request_timeout: Duration::from_millis(cfg.live_mutex_request_timeout_ms),
        }
    }

    #[cfg(test)]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            addr: "disabled".into(),
            ttl_ms: 0,
            request_timeout: Duration::from_millis(1),
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
        if !self.enabled || targets.is_empty() {
            return Ok(CustomerLockGuard::disabled());
        }

        let resources = targets
            .into_iter()
            .map(|customer_id| customer_lock_key(tenant_id, &customer_id))
            .collect::<Vec<_>>();

        let client = self.connect(reason).await?;
        if resources.len() == 1 {
            let resource = resources.into_iter().next().expect("len checked");
            let opts = LockOpts {
                ttl_ms: Some(self.ttl_ms),
                max: Some(1),
            };
            let grant =
                tokio::time::timeout(self.request_timeout, client.acquire(&resource, Some(opts)))
                    .await
                    .map_err(|_| self.timeout_error("acquire", reason))?
                    .map_err(|e| self.client_error("acquire", reason, e))?;

            Ok(CustomerLockGuard {
                broker_addr: Some(self.addr.clone()),
                inner: CustomerLockGuardInner::Single {
                    client,
                    resource,
                    lock_uuid: grant.lock_uuid,
                    fencing_token: grant.fencing_token,
                },
            })
        } else {
            let resource_refs = resources.iter().map(String::as_str).collect::<Vec<_>>();
            let grant = tokio::time::timeout(
                self.request_timeout,
                client.acquire_many(&resource_refs, Some(self.ttl_ms)),
            )
            .await
            .map_err(|_| self.timeout_error("acquire_many", reason))?
            .map_err(|e| self.client_error("acquire_many", reason, e))?;

            let fencing_tokens = grant
                .fencing_tokens
                .iter()
                .map(|(resource, token)| CustomerFencingToken {
                    resource: resource.clone(),
                    token: *token,
                })
                .collect::<Vec<_>>();

            Ok(CustomerLockGuard {
                broker_addr: Some(self.addr.clone()),
                inner: CustomerLockGuardInner::Many {
                    client,
                    resources,
                    lock_uuid: grant.lock_uuid,
                    fencing_tokens,
                },
            })
        }
    }

    async fn connect(&self, reason: &str) -> AppResult<Client> {
        tokio::time::timeout(
            self.request_timeout,
            Client::connect_with_timeout(&self.addr, self.request_timeout),
        )
        .await
        .map_err(|_| self.timeout_error("connect", reason))?
        .map_err(|e| self.client_error("connect", reason, e))
    }

    fn timeout_error(&self, op: &str, reason: &str) -> AppError {
        AppError::Provider {
            provider: "live_mutex_rs".into(),
            message: format!(
                "{op} timed out after {}ms for {reason} at {}",
                self.request_timeout.as_millis(),
                self.addr
            ),
        }
    }

    fn client_error(&self, op: &str, reason: &str, err: ClientError) -> AppError {
        AppError::Provider {
            provider: "live_mutex_rs".into(),
            message: format!("{op} failed for {reason} at {}: {err}", self.addr),
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
            CustomerLockGuardInner::Single {
                resource,
                fencing_token,
                ..
            } => CustomerSnapshotLockInfo {
                enabled: true,
                broker_addr: self.broker_addr.clone(),
                resources: vec![resource.clone()],
                fencing_tokens: fencing_token
                    .map(|token| CustomerFencingToken {
                        resource: resource.clone(),
                        token,
                    })
                    .into_iter()
                    .collect(),
            },
            CustomerLockGuardInner::Many {
                resources,
                fencing_tokens,
                ..
            } => CustomerSnapshotLockInfo {
                enabled: true,
                broker_addr: self.broker_addr.clone(),
                resources: resources.clone(),
                fencing_tokens: fencing_tokens.clone(),
            },
        }
    }

    pub async fn release(self) -> AppResult<()> {
        match self.inner {
            CustomerLockGuardInner::Disabled => Ok(()),
            CustomerLockGuardInner::Single {
                client,
                resource,
                lock_uuid,
                ..
            } => client
                .release(&resource, &lock_uuid, false)
                .await
                .map_err(release_error),
            CustomerLockGuardInner::Many {
                client, lock_uuid, ..
            } => client.release_many(&lock_uuid).await.map_err(release_error),
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

fn release_error(err: ClientError) -> AppError {
    AppError::Provider {
        provider: "live_mutex_rs".into(),
        message: format!("release failed: {err}"),
    }
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
