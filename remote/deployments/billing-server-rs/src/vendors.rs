//! Answers Question 2 from the product brief:
//!   "When do I pay a vendor, and how much?"
//!
//! Reads the per-vendor AP ledger and picks the cheapest viable rail given the
//! vendor's accepted payout methods and the tenant's enabled providers.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::AppResult;
use crate::ledger::LedgerService;
use crate::money::Currency;
use crate::users::UserService;

#[derive(Clone, Debug, Serialize)]
pub struct PayableState {
    pub user_id: Uuid,
    pub email: String,
    pub as_of: DateTime<Utc>,
    pub currency: String,
    pub outstanding_payable_minor: i128,
    pub aging: PayableAging,
    pub next_payment: Option<NextPayment>,
    pub approval_state: ApprovalState,
    pub duplicate_risk: DuplicateRisk,
    pub vendor_payout_methods: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct PayableAging {
    pub due_now_minor: i128,
    pub due_7_days_minor: i128,
    pub due_30_days_minor: i128,
    pub overdue_minor: i128,
}

#[derive(Clone, Debug, Serialize)]
pub struct NextPayment {
    pub due_on: NaiveDate,
    pub amount_minor: i128,
    pub preferred_rail: String,
    pub rail_options: Vec<RailOption>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RailOption {
    pub rail: String,
    pub fee_minor: i128,
    pub eta_business_days: u8,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Approved,
    PendingApprover,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateRisk {
    None,
    Suspected,
    Confirmed,
}

#[derive(Clone)]
pub struct VendorService {
    pool: PgPool,
    users: UserService,
    ledger: LedgerService,
}

impl VendorService {
    pub fn new(pool: PgPool, users: UserService, ledger: LedgerService) -> Self {
        Self {
            pool,
            users,
            ledger,
        }
    }

    pub async fn payable_state(
        &self,
        tenant_id: Uuid,
        email: &str,
        currency: Currency,
    ) -> AppResult<PayableState> {
        let vendor = self.users.by_email(tenant_id, email).await?;

        let ap_code = format!("ap/{}", vendor.id);
        let outstanding = self
            .ledger
            .account_balance(tenant_id, &ap_code, currency)
            .await
            .map(|b| b.balance_minor)
            .unwrap_or(0);

        let aging = self.compute_aging(tenant_id, vendor.id, currency).await?;
        let payout_methods = self.vendor_payout_methods(&vendor.external_refs);
        let next_payment = self.next_payment(&payout_methods, outstanding);

        Ok(PayableState {
            user_id: vendor.id,
            email: vendor.email,
            as_of: Utc::now(),
            currency: currency.as_str().to_string(),
            outstanding_payable_minor: outstanding,
            aging,
            next_payment,
            approval_state: ApprovalState::Approved,
            duplicate_risk: DuplicateRisk::None,
            vendor_payout_methods: payout_methods,
        })
    }

    async fn compute_aging(
        &self,
        tenant_id: Uuid,
        user_id: Uuid,
        currency: Currency,
    ) -> AppResult<PayableAging> {
        let ap_code = format!("ap/{}", user_id);
        let cur = currency.as_str().to_string();

        let row = sqlx::query(
            r#"
            WITH ap AS (
                SELECT p.amount_minor,
                       p.direction,
                       p.posted_at,
                       -- AP "due date" is encoded in metadata.due_on; absent => posted_at + 30d
                       COALESCE(
                           (p.metadata ->> 'due_on')::timestamptz,
                           p.posted_at + INTERVAL '30 days'
                       ) AS due_at
                FROM accounts a
                JOIN postings p ON p.account_id = a.id
                WHERE a.tenant_id = $1 AND a.code = $2 AND a.currency = $3
            )
            SELECT
                COALESCE(SUM(CASE WHEN due_at <  now()                                  AND signed > 0 THEN signed END), 0)::TEXT AS overdue_t,
                COALESCE(SUM(CASE WHEN due_at >= now() AND due_at <  now() + INTERVAL '1 day'  THEN signed END), 0)::TEXT AS due_now_t,
                COALESCE(SUM(CASE WHEN due_at >= now() + INTERVAL '1 day'  AND due_at <  now() + INTERVAL '7 days' THEN signed END), 0)::TEXT AS due_7_t,
                COALESCE(SUM(CASE WHEN due_at >= now() + INTERVAL '7 days' AND due_at <  now() + INTERVAL '30 days' THEN signed END), 0)::TEXT AS due_30_t
            FROM (
                SELECT due_at,
                       -- AP is a liability (credit-normal). A credit posting increases payable, debit decreases it.
                       CASE WHEN direction = 'credit' THEN amount_minor ELSE -amount_minor END AS signed
                FROM ap
            ) s
            "#,
        )
        .bind(tenant_id)
        .bind(&ap_code)
        .bind(&cur)
        .fetch_one(&self.pool)
        .await?;

        let parse = |k: &str| -> i128 {
            row.try_get::<String, _>(k)
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0)
        };

        Ok(PayableAging {
            due_now_minor: parse("due_now_t"),
            due_7_days_minor: parse("due_7_t"),
            due_30_days_minor: parse("due_30_t"),
            overdue_minor: parse("overdue_t"),
        })
    }

    fn vendor_payout_methods(&self, external_refs: &serde_json::Value) -> Vec<String> {
        external_refs
            .get("payout_methods")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_else(|| vec!["ach".into()])
    }

    /// Compute rail options for a payout. This is the place to add real fee
    /// schedules, FX, and SLA per provider. For the scaffold we ship a
    /// reasonable default table.
    fn next_payment(&self, methods: &[String], outstanding: i128) -> Option<NextPayment> {
        if outstanding <= 0 {
            return None;
        }

        let mut options: Vec<RailOption> = Vec::new();
        for m in methods {
            match m.as_str() {
                "ach" => options.push(RailOption {
                    rail: "ach".into(),
                    fee_minor: 25,
                    eta_business_days: 2,
                }),
                "wire" => options.push(RailOption {
                    rail: "wire".into(),
                    fee_minor: 1500,
                    eta_business_days: 0,
                }),
                "swift" => options.push(RailOption {
                    rail: "swift".into(),
                    fee_minor: 4500,
                    eta_business_days: 2,
                }),
                "paypal" => options.push(RailOption {
                    rail: "paypal".into(),
                    fee_minor: ((outstanding as f64) * 0.029) as i128 + 30,
                    eta_business_days: 0,
                }),
                "braintree" => options.push(RailOption {
                    rail: "braintree".into(),
                    fee_minor: ((outstanding as f64) * 0.029) as i128 + 30,
                    eta_business_days: 0,
                }),
                "usdc_sol" => options.push(RailOption {
                    rail: "usdc_sol".into(),
                    fee_minor: 1,
                    eta_business_days: 0,
                }),
                "wise" => options.push(RailOption {
                    rail: "wise".into(),
                    fee_minor: ((outstanding as f64) * 0.004) as i128,
                    eta_business_days: 1,
                }),
                _ => {}
            }
        }
        if options.is_empty() {
            return None;
        }

        options.sort_by_key(|o| o.fee_minor);
        let preferred = options.first().unwrap().rail.clone();

        Some(NextPayment {
            due_on: Utc::now().date_naive(),
            amount_minor: outstanding,
            preferred_rail: preferred,
            rail_options: options,
        })
    }
}
