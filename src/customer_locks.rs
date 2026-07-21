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
    /// Build a guard holding `tokens` for `resources`, without contacting
    /// fiducia. Lets the fence semantics be tested against a real database,
    /// which is where they actually live — the guard's own state is only an
    /// input to the SQL predicate.
    #[cfg(test)]
    fn held_for_tests(resources: Vec<String>, holder: &str, tokens: &[u64]) -> Self {
        let fencing_tokens = resources
            .iter()
            .zip(tokens)
            .map(|(resource, token)| CustomerFencingToken {
                resource: resource.clone(),
                token: *token,
            })
            .collect::<Vec<_>>();
        let grant = FiduciaLockGrant {
            fencing_token: tokens.first().copied().unwrap_or(1),
            lease_expires_ms: 0,
            keys: resources.clone(),
            fencing_tokens: fencing_tokens
                .iter()
                .map(|f| (f.resource.clone(), f.token))
                .collect(),
        };
        Self {
            broker_addr: None,
            inner: CustomerLockGuardInner::Held(Box::new(HeldCustomerLock {
                coordinator: FiduciaCoordinator::disabled(),
                holder: holder.to_string(),
                ttl_ms: 60_000,
                lease_expires_ms: 0,
                resources,
                grant,
                fencing_tokens,
            })),
        }
    }

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
    use sea_orm::TransactionTrait;

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

    /// Connect to the throwaway fence database, or skip.
    ///
    /// Gated on an env var so the suite stays green without a database; set
    /// BILLING_TEST_DATABASE_URL to a database loaded from schema/schema.sql
    /// to actually exercise these.
    async fn fence_db() -> Option<sea_orm::DatabaseConnection> {
        let Ok(url) = std::env::var("BILLING_TEST_DATABASE_URL") else {
            // Announce the skip. A silently-passing fence test is worse than no
            // test: it reports green while proving nothing.
            eprintln!(
                "SKIPPED: fence tests need BILLING_TEST_DATABASE_URL \
                 (a database loaded from schema/schema.sql)"
            );
            return None;
        };
        match sea_orm::Database::connect(url).await {
            Ok(db) => Some(db),
            Err(err) => panic!(
                "BILLING_TEST_DATABASE_URL is set but unusable, so the fence \
                 tests would silently pass without testing anything: {err}"
            ),
        }
    }

    async fn seed_tenant(db: &sea_orm::DatabaseConnection, tenant_id: Uuid) {
        db.execute(stmt(
            r#"INSERT INTO tenants (id, slug, display_name, country_code, kms_key_id)
               VALUES ($1, $2, 'Fence Test', 'US', 'test-key')
               ON CONFLICT DO NOTHING"#,
            [tenant_id.into(), tenant_id.to_string().into()],
        ))
        .await
        .expect("seed tenant");
    }

    /// Apply one guard's fence in its own committed transaction.
    async fn fence_once(
        db: &sea_orm::DatabaseConnection,
        guard: &CustomerLockGuard,
        tenant_id: Uuid,
    ) -> AppResult<()> {
        let tx = db.begin().await.expect("begin");
        let result = guard.fence(&tx, tenant_id).await;
        if result.is_ok() {
            tx.commit().await.expect("commit");
        } else {
            tx.rollback().await.expect("rollback");
        }
        result
    }

    #[tokio::test]
    async fn fence_rejects_a_stale_lease_but_admits_the_holders_own_term() {
        let Some(db) = fence_db().await else { return };
        let tenant_id = Uuid::new_v4();
        seed_tenant(&db, tenant_id).await;
        let key = format!("billing:customer:{tenant_id}:c1");

        let holder_a = CustomerLockGuard::held_for_tests(vec![key.clone()], "holder-A", &[5]);
        let holder_b = CustomerLockGuard::held_for_tests(vec![key.clone()], "holder-B", &[9]);
        let replay = CustomerLockGuard::held_for_tests(vec![key.clone()], "holder-C", &[9]);

        // First grant establishes the fence.
        fence_once(&db, &holder_a, tenant_id).await.unwrap();
        // The same holder writing twice in its own term is legitimate: fiducia
        // preserves the token across a re-acquire, so a strict `>` would reject
        // the holder that actually owns the lease.
        fence_once(&db, &holder_a, tenant_id).await.unwrap();
        // A newer grant advances the fence.
        fence_once(&db, &holder_b, tenant_id).await.unwrap();

        // THE POINT: holder-A's lease expired and holder-B took over. A stale
        // holder that still believes it owns the lock must not commit. Before
        // the fence existed, this write landed silently alongside holder-B's.
        let stale = fence_once(&db, &holder_a, tenant_id).await;
        assert!(
            stale.is_err(),
            "a stale fencing token was allowed to commit"
        );
        assert!(
            stale
                .unwrap_err()
                .to_string()
                .contains("newer fencing token")
        );

        // A different holder replaying the current token is also refused.
        assert!(fence_once(&db, &replay, tenant_id).await.is_err());
    }

    #[tokio::test]
    async fn fence_guards_every_key_of_a_union_grant_independently() {
        let Some(db) = fence_db().await else { return };
        let tenant_id = Uuid::new_v4();
        seed_tenant(&db, tenant_id).await;
        let key_a = format!("billing:customer:{tenant_id}:a");
        let key_b = format!("billing:customer:{tenant_id}:b");

        // A union grant carries a DIFFERENT token per key.
        let union = CustomerLockGuard::held_for_tests(
            vec![key_a.clone(), key_b.clone()],
            "holder-union",
            &[4, 8],
        );
        fence_once(&db, &union, tenant_id).await.unwrap();

        // Someone advances only the second key.
        let advance_b = CustomerLockGuard::held_for_tests(vec![key_b.clone()], "holder-next", &[9]);
        fence_once(&db, &advance_b, tenant_id).await.unwrap();

        // The union holder is now stale for key_b even though key_a is still
        // fine — and must be refused as a whole, not partially applied.
        let stale_union = fence_once(&db, &union, tenant_id).await;
        assert!(
            stale_union.is_err(),
            "union grant committed while stale on one member key"
        );

        // key_a must not have been advanced by the refused attempt.
        let row = db
            .query_one(stmt(
                "SELECT fencing_token FROM fiducia_fences WHERE tenant_id = $1 AND fence_key = $2",
                [tenant_id.into(), key_a.clone().into()],
            ))
            .await
            .unwrap()
            .expect("key_a fence row");
        let token: i64 = row.try_get_by_index(0).unwrap();
        assert_eq!(token, 4, "a refused fence must not advance sibling keys");
    }

    #[tokio::test]
    async fn fence_is_scoped_per_tenant() {
        let Some(db) = fence_db().await else { return };
        let tenant_a = Uuid::new_v4();
        let tenant_b = Uuid::new_v4();
        seed_tenant(&db, tenant_a).await;
        seed_tenant(&db, tenant_b).await;
        // Same fence_key string under two tenants must not collide.
        let key = "billing:customer:shared:c1".to_string();
        let high = CustomerLockGuard::held_for_tests(vec![key.clone()], "holder", &[99]);
        let low = CustomerLockGuard::held_for_tests(vec![key], "holder", &[2]);

        fence_once(&db, &high, tenant_a).await.unwrap();
        // Tenant B starts fresh; tenant A's high token must not fence it out.
        fence_once(&db, &low, tenant_b).await.unwrap();
    }

    #[tokio::test]
    async fn disabled_guard_fence_is_a_noop() {
        let Some(db) = fence_db().await else { return };
        let tenant_id = Uuid::new_v4();
        // A disabled guard has no tokens; fencing must succeed without writing.
        let tx = db.begin().await.unwrap();
        CustomerLockGuard::disabled()
            .fence(&tx, tenant_id)
            .await
            .expect("disabled fence is a no-op");
        tx.commit().await.unwrap();
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
