//! Built-in system job handlers + boot-time registration.
//!
//! Run on startup once `AppState` is constructed. Handlers are registered
//! against the `HandlerRegistry`, then the `SchedulerRunner` is spawned and
//! also seeds the system jobs into the `scheduled_jobs` table if missing.

use async_trait::async_trait;
use std::sync::Arc;

use crate::error::AppResult;
use crate::locks::LockService;
use crate::notifications::evaluator::RuleEvaluatorJob;
use crate::scheduler::{
    CreateScheduledJob, HandlerRegistryBuilder, JobContext, JobHandler, JobOutput, ScheduleKind,
    SchedulerService,
};
use crate::solana::AnchorService;
use crate::state::AppState;
use crate::sync::ConnectionSyncJob;

pub fn build_registry(state: &AppState) -> crate::scheduler::HandlerRegistry {
    HandlerRegistryBuilder::new()
        .register(
            "system.lock_sweeper",
            Arc::new(LockSweeperJob::new(state.locks.clone())),
        )
        .register(
            "system.anchor_sweeper",
            Arc::new(AnchorSweeperJob::new(state.anchor.clone())),
        )
        .register(
            "notifications.evaluate_rules",
            Arc::new(RuleEvaluatorJob::new(
                state.pool.clone(),
                state.notifications.clone(),
                state.cfg.clone(),
            )),
        )
        .register(
            "tenant.webhook",
            Arc::new(TenantWebhookJob::new(state.cfg.clone())),
        )
        .register(
            "sync.connection",
            Arc::new(ConnectionSyncJob::new(
                state.pool.clone(),
                state.cfg.clone(),
                state.ledger.clone(),
                state.locks.clone(),
                state.connections.clone(),
                state.events.clone(),
                state.solana_client.clone(),
            )),
        )
        .build()
}

/// Idempotently insert the system job rows. Safe to call on every boot.
pub async fn seed_system_jobs(scheduler: &SchedulerService) -> AppResult<()> {
    let jobs = vec![
        CreateScheduledJob {
            kind: "system.lock_sweeper".into(),
            name: "default".into(),
            schedule_kind: ScheduleKind::Interval,
            cron_expr: None,
            interval_seconds: Some(300),
            one_shot_at: None,
            timezone: "UTC".into(),
            payload: serde_json::json!({ "keep_for_hours": 24 }),
            enabled: true,
            max_attempts: 3,
            retry_backoff_secs: 60,
            timeout_seconds: 60,
        },
        CreateScheduledJob {
            kind: "system.anchor_sweeper".into(),
            name: "default".into(),
            schedule_kind: ScheduleKind::Interval,
            cron_expr: None,
            interval_seconds: Some(60),
            one_shot_at: None,
            timezone: "UTC".into(),
            payload: serde_json::Value::Null,
            enabled: true,
            max_attempts: 5,
            retry_backoff_secs: 30,
            timeout_seconds: 120,
        },
        CreateScheduledJob {
            kind: "notifications.evaluate_rules".into(),
            name: "default".into(),
            schedule_kind: ScheduleKind::Interval,
            cron_expr: None,
            interval_seconds: Some(300),
            one_shot_at: None,
            timezone: "UTC".into(),
            payload: serde_json::Value::Null,
            enabled: true,
            max_attempts: 3,
            retry_backoff_secs: 60,
            timeout_seconds: 120,
        },
    ];

    for j in jobs {
        scheduler.create(None, None, j).await?;
    }
    Ok(())
}

// -- System handlers ---------------------------------------------------------

pub struct LockSweeperJob {
    locks: LockService,
}
impl LockSweeperJob {
    pub fn new(locks: LockService) -> Self {
        Self { locks }
    }
}
#[async_trait]
impl JobHandler for LockSweeperJob {
    async fn run(&self, ctx: &JobContext) -> AppResult<JobOutput> {
        let keep_for_hours = ctx
            .payload
            .get("keep_for_hours")
            .and_then(|v| v.as_i64())
            .unwrap_or(24);
        let n = self.locks.sweep_expired(keep_for_hours).await?;
        Ok(JobOutput::with_data(
            format!("swept {n} expired leases (older than {keep_for_hours}h)"),
            serde_json::json!({ "swept": n, "keep_for_hours": keep_for_hours }),
        ))
    }
}

pub struct AnchorSweeperJob {
    anchor: Arc<AnchorService>,
}
impl AnchorSweeperJob {
    pub fn new(anchor: Arc<AnchorService>) -> Self {
        Self { anchor }
    }
}
#[async_trait]
impl JobHandler for AnchorSweeperJob {
    async fn run(&self, _ctx: &JobContext) -> AppResult<JobOutput> {
        self.anchor.sweep_all_tenants().await?;
        Ok(JobOutput::ok("anchor sweep completed"))
    }
}

/// Outbound tenant webhook job: POSTs `payload` to the tenant's registered URL
/// with an HMAC signature, signed by the tenant's webhook secret.
///
/// Tenants register one of these for payroll, AP runs, end-of-month close,
/// etc. The platform doesn't execute the business logic — it just calls the
/// tenant on schedule with a signed payload they can verify came from us.
pub struct TenantWebhookJob {
    cfg: std::sync::Arc<crate::config::Config>,
}

impl TenantWebhookJob {
    pub fn new(cfg: std::sync::Arc<crate::config::Config>) -> Self {
        Self { cfg }
    }
}

#[async_trait]
impl JobHandler for TenantWebhookJob {
    async fn run(&self, ctx: &JobContext) -> AppResult<JobOutput> {
        let url = ctx
            .payload
            .get("webhook_url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::error::AppError::BadRequest(
                    "tenant.webhook payload requires 'webhook_url'".into(),
                )
            })?
            .to_string();
        let secret = ctx
            .payload
            .get("signing_secret")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let body = ctx
            .payload
            .get("body")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({ "ping": true }));

        let body_with_meta = serde_json::json!({
            "job_id": ctx.job_id,
            "tenant_id": ctx.tenant_id,
            "name": ctx.name,
            "attempt": ctx.attempt,
            "idempotency_key": ctx.idempotency_key,
            "body": body,
        });

        let res = crate::notifications::channels::deliver_webhook(
            &url,
            &body_with_meta,
            secret.as_deref(),
            self.cfg.block_private_outbound,
        )
        .await
        .map_err(|e| crate::error::AppError::Provider {
            provider: "tenant_webhook".into(),
            message: e,
        })?;

        Ok(JobOutput::with_data(
            format!("POST {url} -> {}", res.http_status),
            serde_json::json!({ "http_status": res.http_status }),
        ))
    }
}
