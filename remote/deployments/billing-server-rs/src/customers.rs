//! Answers Question 1 from the product brief:
//!   "When do I bill the customer, and for how much?"
//!
//! Reads the per-customer ledger projections and rolls them up into a single
//! `BillingState` response.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::customer_locks::{CustomerLockBroker, CustomerSnapshotLockInfo};
use crate::error::AppResult;
use crate::ledger::AccountKind;
use crate::money::Currency;
use crate::users::UserService;

#[derive(Clone, Debug, Serialize)]
pub struct BillingState {
    pub user_id: Uuid,
    pub email: String,
    pub as_of: DateTime<Utc>,
    pub currency: String,
    pub outstanding_balance_minor: i128,
    pub balance_components: Vec<BalanceComponent>,
    pub snapshot_lock: CustomerSnapshotLockInfo,
    pub aging: Aging,
    pub credit_memos_minor: i128,
    pub unallocated_cash_minor: i128,
    pub next_bill: Option<NextBill>,
    pub last_payment: Option<LastPayment>,
    pub reconciliation_status: ReconciliationStatus,
    pub as_of_confidence: Confidence,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct Aging {
    pub current_minor: i128,
    pub d1_30_minor: i128,
    pub d31_60_minor: i128,
    pub d61_90_minor: i128,
    pub over_90_minor: i128,
}

#[derive(Clone, Debug, Serialize)]
pub struct NextBill {
    pub due_on: chrono::NaiveDate,
    pub amount_minor: i128,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct LastPayment {
    pub received_on: DateTime<Utc>,
    pub amount_minor: i128,
    pub via: String,
    pub external_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct BalanceComponent {
    pub service: String,
    pub account_code: String,
    pub account_kind: AccountKind,
    pub balance_minor: i128,
    pub contribution_minor: i128,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationStatus {
    Clean,
    BreaksOpen,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Pending,
    Confirmed,
    Finalized,
}

#[derive(Clone)]
pub struct CustomerService {
    pool: PgPool,
    users: UserService,
    customer_locks: CustomerLockBroker,
}

impl CustomerService {
    pub fn new(pool: PgPool, users: UserService, customer_locks: CustomerLockBroker) -> Self {
        Self {
            pool,
            users,
            customer_locks,
        }
    }

    pub async fn billing_state(
        &self,
        tenant_id: Uuid,
        email: &str,
        currency: Currency,
    ) -> AppResult<BillingState> {
        let user = self.users.by_email(tenant_id, email).await?;
        let lock_guard = self
            .customer_locks
            .acquire_customer_uuid(tenant_id, user.id, "customers.billing_state")
            .await?;
        let snapshot_lock = lock_guard.info();

        let result = async {
            // The distributed lock excludes cooperating writers, while the
            // repeatable-read, read-only transaction guarantees every query in
            // this multi-query response observes one PostgreSQL snapshot.
            let mut tx = self.pool.begin().await?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
                .execute(&mut *tx)
                .await?;
            let components = self
                .customer_balance_components(&mut tx, tenant_id, user.id, currency)
                .await?;

            let outstanding = components.iter().map(|c| c.contribution_minor).sum();
            let unallocated =
                component_balance(&components, &format!("unallocated_cash/{}", user.id));
            let credit_memos = component_balance(&components, &format!("credit_memo/{}", user.id))
                + component_balance(&components, &format!("credit_memos/{}", user.id));

            let aging = self
                .compute_aging(&mut tx, tenant_id, user.id, currency)
                .await?;
            let last_payment = self
                .last_payment(&mut tx, tenant_id, user.id, currency)
                .await?;
            let recon = self.recon_status(&mut tx, tenant_id).await?;

            let state = BillingState {
                user_id: user.id,
                email: user.email,
                as_of: Utc::now(),
                currency: currency.as_str().to_string(),
                outstanding_balance_minor: outstanding,
                balance_components: components,
                snapshot_lock,
                aging,
                credit_memos_minor: credit_memos,
                unallocated_cash_minor: unallocated,
                next_bill: None, // populated by subscription/usage engine (out of scope for v0)
                last_payment,
                reconciliation_status: recon,
                as_of_confidence: Confidence::Finalized,
            };
            tx.commit().await?;
            Ok(state)
        }
        .await;

        if let Err(e) = lock_guard.release().await {
            tracing::warn!(error = %e, "failed to release customer billing-state lock");
        }
        result
    }

    async fn customer_balance_components(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        tenant_id: Uuid,
        user_id: Uuid,
        currency: Currency,
    ) -> AppResult<Vec<BalanceComponent>> {
        let cur = currency.as_str().to_string();
        let ar_code = format!("ar/{user_id}");
        let unallocated_code = format!("unallocated_cash/{user_id}");
        let credit_memo_code = format!("credit_memo/{user_id}");
        let credit_memos_code = format!("credit_memos/{user_id}");
        let customer_prefix = format!("customer/{user_id}/%");

        let rows = sqlx::query(
            r#"
            SELECT a.code,
                   a.kind,
                   COALESCE(SUM(
                       CASE
                           WHEN a.normal_side = 'debit'  AND p.direction = 'debit'  THEN  p.amount_minor
                           WHEN a.normal_side = 'debit'  AND p.direction = 'credit' THEN -p.amount_minor
                           WHEN a.normal_side = 'credit' AND p.direction = 'credit' THEN  p.amount_minor
                           WHEN a.normal_side = 'credit' AND p.direction = 'debit'  THEN -p.amount_minor
                       END
                   ), 0)::TEXT AS balance_t
            FROM accounts a
            LEFT JOIN postings p ON p.account_id = a.id
            WHERE a.tenant_id = $1
              AND a.currency = $2
              AND (
                    a.user_id = $3
                 OR a.code = $4
                 OR a.code = $5
                 OR a.code = $6
                 OR a.code = $7
                 OR a.code LIKE $8
              )
            GROUP BY a.code, a.kind
            ORDER BY a.code
            "#,
        )
        .bind(tenant_id)
        .bind(&cur)
        .bind(user_id)
        .bind(&ar_code)
        .bind(&unallocated_code)
        .bind(&credit_memo_code)
        .bind(&credit_memos_code)
        .bind(&customer_prefix)
        .fetch_all(&mut **tx)
        .await?;

        rows.iter()
            .map(|row| {
                let account_code: String = row.try_get("code")?;
                let account_kind: AccountKind = row.try_get("kind")?;
                let balance_text: String = row.try_get("balance_t")?;
                let balance_minor = balance_text.parse().unwrap_or(0);
                let contribution_minor =
                    customer_balance_contribution(account_kind, &account_code, balance_minor);
                Ok(BalanceComponent {
                    service: service_from_account_code(&account_code),
                    account_code,
                    account_kind,
                    balance_minor,
                    contribution_minor,
                })
            })
            .collect()
    }

    async fn compute_aging(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        tenant_id: Uuid,
        user_id: Uuid,
        currency: Currency,
    ) -> AppResult<Aging> {
        let ar_code = format!("ar/{}", user_id);
        let cur = currency.as_str().to_string();

        let row = sqlx::query(
            r#"
            WITH ar AS (
                SELECT p.amount_minor,
                       p.direction,
                       p.posted_at,
                       EXTRACT(EPOCH FROM (now() - p.posted_at))/86400 AS days_old
                FROM accounts a
                JOIN postings p ON p.account_id = a.id
                WHERE a.tenant_id = $1 AND a.code = $2 AND a.currency = $3
            )
            SELECT
                COALESCE(SUM(CASE WHEN days_old <  1                       THEN signed END), 0)::TEXT AS current_t,
                COALESCE(SUM(CASE WHEN days_old >= 1  AND days_old < 30    THEN signed END), 0)::TEXT AS d1_30_t,
                COALESCE(SUM(CASE WHEN days_old >= 30 AND days_old < 60    THEN signed END), 0)::TEXT AS d31_60_t,
                COALESCE(SUM(CASE WHEN days_old >= 60 AND days_old < 90    THEN signed END), 0)::TEXT AS d61_90_t,
                COALESCE(SUM(CASE WHEN days_old >= 90                      THEN signed END), 0)::TEXT AS over90_t
            FROM (
                SELECT days_old,
                       CASE WHEN direction = 'debit' THEN amount_minor ELSE -amount_minor END AS signed
                FROM ar
            ) s
            "#,
        )
        .bind(tenant_id)
        .bind(&ar_code)
        .bind(&cur)
        .fetch_one(&mut **tx)
        .await?;

        let parse = |k: &str| -> i128 {
            row.try_get::<String, _>(k)
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0)
        };

        Ok(Aging {
            current_minor: parse("current_t"),
            d1_30_minor: parse("d1_30_t"),
            d31_60_minor: parse("d31_60_t"),
            d61_90_minor: parse("d61_90_t"),
            over_90_minor: parse("over90_t"),
        })
    }

    async fn last_payment(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        tenant_id: Uuid,
        user_id: Uuid,
        currency: Currency,
    ) -> AppResult<Option<LastPayment>> {
        let ar_code = format!("ar/{}", user_id);
        let cur = currency.as_str().to_string();

        let row = sqlx::query(
            r#"
            SELECT p.amount_minor::TEXT AS amount_t, p.posted_at, p.source, p.source_event_id
            FROM accounts a
            JOIN postings p ON p.account_id = a.id
            WHERE a.tenant_id = $1
              AND a.code = $2
              AND a.currency = $3
              AND p.direction = 'credit'
            ORDER BY p.posted_at DESC
            LIMIT 1
            "#,
        )
        .bind(tenant_id)
        .bind(&ar_code)
        .bind(&cur)
        .fetch_optional(&mut **tx)
        .await?;

        Ok(row.map(|r| LastPayment {
            received_on: r.try_get("posted_at").unwrap_or_else(|_| Utc::now()),
            amount_minor: r
                .try_get::<String, _>("amount_t")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            via: r.try_get("source").unwrap_or_default(),
            external_id: r.try_get("source_event_id").unwrap_or_default(),
        }))
    }

    async fn recon_status(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        tenant_id: Uuid,
    ) -> AppResult<ReconciliationStatus> {
        let count: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM reconciliation_breaks
               WHERE tenant_id = $1 AND status = 'open'"#,
        )
        .bind(tenant_id)
        .fetch_one(&mut **tx)
        .await?;
        Ok(if count == 0 {
            ReconciliationStatus::Clean
        } else {
            ReconciliationStatus::BreaksOpen
        })
    }
}

fn component_balance(components: &[BalanceComponent], account_code: &str) -> i128 {
    components
        .iter()
        .find(|c| c.account_code == account_code)
        .map(|c| c.balance_minor)
        .unwrap_or(0)
}

fn customer_balance_contribution(
    kind: AccountKind,
    account_code: &str,
    balance_minor: i128,
) -> i128 {
    match kind {
        AccountKind::Receivable => balance_minor,
        AccountKind::Liability
            if account_code.starts_with("unallocated_cash/")
                || account_code.starts_with("credit_memo/")
                || account_code.starts_with("credit_memos/") =>
        {
            -balance_minor
        }
        _ => 0,
    }
}

fn service_from_account_code(account_code: &str) -> String {
    account_code
        .split('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contribution_nets_customer_liabilities_against_receivable() {
        assert_eq!(
            customer_balance_contribution(AccountKind::Receivable, "ar/cus_1", 1_000),
            1_000
        );
        assert_eq!(
            customer_balance_contribution(AccountKind::Liability, "unallocated_cash/cus_1", 250,),
            -250
        );
        assert_eq!(
            customer_balance_contribution(AccountKind::Liability, "credit_memo/cus_1", 100),
            -100
        );
        assert_eq!(
            customer_balance_contribution(AccountKind::Asset, "clearing/stripe/acct_1", 999),
            0
        );
    }

    #[test]
    fn service_uses_account_code_prefix() {
        assert_eq!(service_from_account_code("ar/cus_1"), "ar");
        assert_eq!(
            service_from_account_code("clearing/stripe/acct_1"),
            "clearing"
        );
        assert_eq!(service_from_account_code(""), "unknown");
    }
}
