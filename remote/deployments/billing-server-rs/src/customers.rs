//! Answers Question 1 from the product brief:
//!   "When do I bill the customer, and for how much?"
//!
//! Reads the per-customer ledger projections and rolls them up into a single
//! `BillingState` response.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::AppResult;
use crate::ledger::LedgerService;
use crate::money::Currency;
use crate::users::UserService;

#[derive(Clone, Debug, Serialize)]
pub struct BillingState {
    pub user_id: Uuid,
    pub email: String,
    pub as_of: DateTime<Utc>,
    pub currency: String,
    pub outstanding_balance_minor: i128,
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
    ledger: LedgerService,
}

impl CustomerService {
    pub fn new(pool: PgPool, users: UserService, ledger: LedgerService) -> Self {
        Self {
            pool,
            users,
            ledger,
        }
    }

    pub async fn billing_state(
        &self,
        tenant_id: Uuid,
        email: &str,
        currency: Currency,
    ) -> AppResult<BillingState> {
        let user = self.users.by_email(tenant_id, email).await?;

        let ar_code = format!("ar/{}", user.id);
        let outstanding = self
            .ledger
            .account_balance(tenant_id, &ar_code, currency)
            .await
            .map(|b| b.balance_minor)
            .unwrap_or(0);

        let unallocated = self
            .ledger
            .account_balance(
                tenant_id,
                &format!("unallocated_cash/{}", user.id),
                currency,
            )
            .await
            .map(|b| b.balance_minor)
            .unwrap_or(0);

        let credit_memos = self
            .ledger
            .account_balance(tenant_id, &format!("credit_memo/{}", user.id), currency)
            .await
            .map(|b| b.balance_minor)
            .unwrap_or(0);

        let aging = self.compute_aging(tenant_id, user.id, currency).await?;
        let last_payment = self.last_payment(tenant_id, user.id, currency).await?;
        let recon = self.recon_status(tenant_id).await?;

        Ok(BillingState {
            user_id: user.id,
            email: user.email,
            as_of: Utc::now(),
            currency: currency.as_str().to_string(),
            outstanding_balance_minor: outstanding,
            aging,
            credit_memos_minor: credit_memos,
            unallocated_cash_minor: unallocated,
            next_bill: None, // populated by subscription/usage engine (out of scope for v0)
            last_payment,
            reconciliation_status: recon,
            as_of_confidence: Confidence::Finalized,
        })
    }

    async fn compute_aging(
        &self,
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
        .fetch_one(&self.pool)
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
        .fetch_optional(&self.pool)
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

    async fn recon_status(&self, tenant_id: Uuid) -> AppResult<ReconciliationStatus> {
        let count: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM reconciliation_breaks
               WHERE tenant_id = $1 AND status = 'open'"#,
        )
        .bind(tenant_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(if count == 0 {
            ReconciliationStatus::Clean
        } else {
            ReconciliationStatus::BreaksOpen
        })
    }
}
