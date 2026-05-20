//! The rule evaluator runs as a scheduler job (`notifications.evaluate_rules`).
//!
//! For each tenant, it walks active rules and either:
//!   - evaluates the rule against the ledger (e.g. `balance_negative` checks
//!     every customer's AR account), OR
//!   - reacts to a recent change (e.g. `payment_received` watches new postings
//!     into `clearing/*`).
//!
//! For each match, the evaluator creates a dispatch row (subject to per-day
//! throttling) and immediately calls the channel driver. Failures land in
//! the dispatch row's `error` field and the scheduler's normal retry kicks in
//! the next time the evaluator job runs.

use async_trait::async_trait;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::scheduler::{JobContext, JobHandler, JobOutput};
use crate::shard::Region;

use super::channels::{deliver_email, deliver_slack, deliver_sms, deliver_webhook};
use super::service::NotificationService;
use super::types::*;

pub struct RuleEvaluatorJob {
    pool: PgPool,
    notifications: Arc<NotificationService>,
}

impl RuleEvaluatorJob {
    pub fn new(pool: PgPool, notifications: Arc<NotificationService>) -> Self {
        Self {
            pool,
            notifications,
        }
    }
}

#[async_trait]
impl JobHandler for RuleEvaluatorJob {
    async fn run(&self, ctx: &JobContext) -> AppResult<JobOutput> {
        // The job can be system-wide (tenant_id None -> iterate all tenants)
        // or tenant-scoped (tenant_id Some -> just that one).
        let tenant_ids: Vec<Uuid> = match ctx.tenant_id {
            Some(t) => vec![t],
            None => {
                sqlx::query_scalar(r#"SELECT id FROM tenants WHERE status = 'active'"#)
                    .fetch_all(&self.pool)
                    .await?
            }
        };

        let mut total_evaluated: i64 = 0;
        let mut total_dispatched: i64 = 0;
        let mut total_throttled: i64 = 0;
        let mut total_failed: i64 = 0;

        for tid in tenant_ids {
            let region = match self.tenant_region(tid).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(tenant = %tid, error = %e, "skip tenant: bad region");
                    continue;
                }
            };

            let rules = self.notifications.list_rules(tid).await?;
            for rule in rules {
                if !rule.enabled {
                    continue;
                }
                total_evaluated += 1;

                let matches = match rule.kind.as_str() {
                    "balance_negative" => self.eval_balance_negative(tid, &rule).await?,
                    "payment_overdue" => self.eval_payment_overdue(tid, &rule).await?,
                    "reconciliation_break_opened" => self.eval_recon_break(tid, &rule).await?,
                    other => {
                        tracing::warn!(rule = %rule.id, kind = %other,
                            "unknown notification rule kind; skipping");
                        Vec::new()
                    }
                };

                for m in matches {
                    if self
                        .notifications
                        .would_throttle(
                            rule.id,
                            m.target_resource.as_deref(),
                            rule.throttle_per_day,
                        )
                        .await?
                    {
                        total_throttled += 1;
                        continue;
                    }

                    let dispatch_id = self
                        .notifications
                        .create_dispatch(
                            rule.id,
                            tid,
                            region,
                            m.target_resource.as_deref(),
                            rule.channel,
                            &rule.target,
                            m.payload.clone(),
                        )
                        .await?;

                    let result = match rule.channel {
                        NotificationChannel::Webhook => {
                            deliver_webhook(&rule.target, &m.payload, None).await
                        }
                        NotificationChannel::Slack => {
                            deliver_slack(&rule.target, &m.payload, None).await
                        }
                        NotificationChannel::Email => {
                            deliver_email(&rule.target, &m.payload, None).await
                        }
                        NotificationChannel::Sms => {
                            deliver_sms(&rule.target, &m.payload, None).await
                        }
                    };

                    match result {
                        Ok(r) => {
                            self.notifications
                                .mark_dispatch_sent(dispatch_id, r.provider_message_id.as_deref())
                                .await?;
                            total_dispatched += 1;
                        }
                        Err(e) => {
                            self.notifications
                                .mark_dispatch_failed(dispatch_id, &e)
                                .await?;
                            total_failed += 1;
                        }
                    }
                }
            }
        }

        Ok(JobOutput::with_data(
            format!(
                "evaluated {total_evaluated} rules; dispatched {total_dispatched}; \
                 throttled {total_throttled}; failed {total_failed}"
            ),
            serde_json::json!({
                "evaluated": total_evaluated,
                "dispatched": total_dispatched,
                "throttled": total_throttled,
                "failed": total_failed,
            }),
        ))
    }
}

#[derive(Clone)]
struct Match {
    target_resource: Option<String>,
    payload: serde_json::Value,
}

impl RuleEvaluatorJob {
    async fn tenant_region(&self, tenant_id: Uuid) -> AppResult<Region> {
        let row = sqlx::query(r#"SELECT country_code, us_state FROM tenants WHERE id = $1"#)
            .bind(tenant_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("tenant {tenant_id}")))?;
        let cc: String = row.try_get("country_code")?;
        let st: Option<String> = row.try_get("us_state")?;
        Region::from_codes(&cc, st.as_deref()).map_err(|e| AppError::BadRequest(e.to_string()))
    }

    async fn eval_balance_negative(
        &self,
        tenant_id: Uuid,
        rule: &NotificationRule,
    ) -> AppResult<Vec<Match>> {
        // Negative balance on any user's AR account (i.e., they paid more than
        // they owe — a credit balance on a debit-normal account).
        let rows = sqlx::query(
            r#"
            SELECT u.id AS user_id, u.email::TEXT AS email, a.code,
                   COALESCE(SUM(
                       CASE WHEN p.direction = 'debit' THEN p.amount_minor
                            ELSE -p.amount_minor END
                   ), 0)::TEXT AS balance_t
            FROM users u
            JOIN accounts a ON a.user_id = u.id AND a.tenant_id = u.tenant_id
            LEFT JOIN postings p ON p.account_id = a.id
            WHERE u.tenant_id = $1 AND a.kind = 'receivable'
            GROUP BY u.id, u.email, a.code
            HAVING COALESCE(SUM(
                       CASE WHEN p.direction = 'debit' THEN p.amount_minor
                            ELSE -p.amount_minor END
                   ), 0) < 0
            "#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let user_id: Uuid = r.try_get("user_id").unwrap_or(Uuid::nil());
                let email: String = r.try_get("email").unwrap_or_default();
                let balance: String = r.try_get("balance_t").unwrap_or_else(|_| "0".into());
                Match {
                    target_resource: Some(user_id.to_string()),
                    payload: serde_json::json!({
                        "summary": format!("Customer {email} has a negative balance ({balance} minor units)"),
                        "rule_kind": rule.kind,
                        "rule_name": rule.name,
                        "user_id": user_id,
                        "email": email,
                        "balance_minor": balance,
                    }),
                }
            })
            .collect())
    }

    async fn eval_payment_overdue(
        &self,
        tenant_id: Uuid,
        rule: &NotificationRule,
    ) -> AppResult<Vec<Match>> {
        let days = rule
            .params
            .get("days")
            .and_then(|v| v.as_i64())
            .unwrap_or(30);

        let rows = sqlx::query(
            r#"
            SELECT u.id AS user_id, u.email::TEXT AS email,
                   COALESCE(SUM(
                       CASE WHEN p.direction = 'debit' THEN p.amount_minor
                            ELSE -p.amount_minor END
                   ), 0)::TEXT AS overdue_t
            FROM users u
            JOIN accounts a ON a.user_id = u.id AND a.tenant_id = u.tenant_id
            JOIN postings p ON p.account_id = a.id
            WHERE u.tenant_id = $1 AND a.kind = 'receivable'
              AND p.posted_at < now() - ($2 || ' days')::interval
            GROUP BY u.id, u.email
            HAVING COALESCE(SUM(
                       CASE WHEN p.direction = 'debit' THEN p.amount_minor
                            ELSE -p.amount_minor END
                   ), 0) > 0
            "#,
        )
        .bind(tenant_id)
        .bind(days.to_string())
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let user_id: Uuid = r.try_get("user_id").unwrap_or(Uuid::nil());
                let email: String = r.try_get("email").unwrap_or_default();
                let overdue: String = r.try_get("overdue_t").unwrap_or_else(|_| "0".into());
                Match {
                    target_resource: Some(user_id.to_string()),
                    payload: serde_json::json!({
                        "summary": format!("Customer {email} has overdue balance ({overdue} minor units, > {days} days)"),
                        "rule_kind": rule.kind,
                        "rule_name": rule.name,
                        "user_id": user_id,
                        "email": email,
                        "overdue_minor": overdue,
                        "days": days,
                    }),
                }
            })
            .collect())
    }

    async fn eval_recon_break(
        &self,
        tenant_id: Uuid,
        rule: &NotificationRule,
    ) -> AppResult<Vec<Match>> {
        let rows = sqlx::query(
            r#"
            SELECT id, break_type, expected_minor::TEXT AS expected_t,
                   actual_minor::TEXT AS actual_t, currency, provider::TEXT AS provider_t
            FROM reconciliation_breaks
            WHERE tenant_id = $1 AND status = 'open'
              AND detected_at > now() - interval '1 hour'
            "#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let id: i64 = r.try_get("id").unwrap_or(0);
                let bt: String = r.try_get("break_type").unwrap_or_default();
                let prov: String = r.try_get("provider_t").unwrap_or_default();
                Match {
                    target_resource: Some(format!("break:{id}")),
                    payload: serde_json::json!({
                        "summary": format!("Reconciliation break opened: {bt} on {prov}"),
                        "rule_kind": rule.kind,
                        "rule_name": rule.name,
                        "break_id": id,
                        "break_type": bt,
                        "provider": prov,
                        "expected_minor": r.try_get::<String, _>("expected_t").ok(),
                        "actual_minor": r.try_get::<String, _>("actual_t").ok(),
                        "currency": r.try_get::<Option<String>, _>("currency").ok().flatten(),
                    }),
                }
            })
            .collect())
    }
}
