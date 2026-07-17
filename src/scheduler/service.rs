use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect,
};
use uuid::Uuid;

use crate::db::{decode_enum, require_row, stmt};
use crate::entity::{job_runs, scheduled_jobs};
use crate::error::{AppError, AppResult};
use crate::shard::{Region, ShardKey};

use super::types::*;

#[derive(Clone)]
pub struct SchedulerService {
    pool: DatabaseConnection,
}

impl SchedulerService {
    pub fn new(pool: DatabaseConnection) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        tenant_id: Option<Uuid>,
        region: Option<Region>,
        input: CreateScheduledJob,
    ) -> AppResult<ScheduledJob> {
        validate_schedule(&input)?;

        let shard = match (tenant_id, region) {
            (Some(tid), Some(r)) => ShardKey::derive(tid, r).0,
            _ => 0,
        };

        let next_run_at = compute_next_run(
            input.schedule_kind,
            input.cron_expr.as_deref(),
            input.interval_seconds,
            input.one_shot_at,
            &input.timezone,
            Utc::now(),
        )?;

        // Raw SQL (SeaORM Statement): the DO UPDATE arm needs
        // `LEAST(scheduled_jobs.next_run_at, EXCLUDED.next_run_at)` and
        // `updated_at = now()`, which the entity OnConflict API cannot
        // express. Only change from the sqlx original: `schedule_kind` in
        // RETURNING gains a `::TEXT` cast (the driver cannot decode a
        // custom enum OID into `String`).
        let row = require_row(
            self.pool
                .query_one(stmt(
                    r#"
            INSERT INTO scheduled_jobs
                (tenant_id, shard_key, kind, name, schedule_kind, cron_expr,
                 interval_seconds, one_shot_at, timezone, payload, enabled,
                 max_attempts, retry_backoff_secs, timeout_seconds, next_run_at)
            VALUES ($1, $2, $3, $4, $5::schedule_kind, $6, $7, $8, $9, $10,
                    $11, $12, $13, $14, $15)
            ON CONFLICT (tenant_id, kind, name) DO UPDATE
                SET cron_expr          = EXCLUDED.cron_expr,
                    interval_seconds   = EXCLUDED.interval_seconds,
                    one_shot_at        = EXCLUDED.one_shot_at,
                    timezone           = EXCLUDED.timezone,
                    payload            = EXCLUDED.payload,
                    enabled            = EXCLUDED.enabled,
                    max_attempts       = EXCLUDED.max_attempts,
                    retry_backoff_secs = EXCLUDED.retry_backoff_secs,
                    timeout_seconds    = EXCLUDED.timeout_seconds,
                    next_run_at        = LEAST(scheduled_jobs.next_run_at, EXCLUDED.next_run_at),
                    updated_at         = now()
            RETURNING id, tenant_id, shard_key, kind, name,
                      schedule_kind::TEXT AS schedule_kind,
                      cron_expr, interval_seconds, one_shot_at, timezone,
                      payload, enabled, max_attempts, retry_backoff_secs,
                      timeout_seconds, next_run_at, last_run_at, created_at
            "#,
                    [
                        tenant_id.into(),
                        shard.into(),
                        input.kind.clone().into(),
                        input.name.clone().into(),
                        schedule_kind_tag(input.schedule_kind).into(),
                        input.cron_expr.clone().into(),
                        input.interval_seconds.into(),
                        input.one_shot_at.into(),
                        input.timezone.clone().into(),
                        input.payload.clone().into(),
                        input.enabled.into(),
                        input.max_attempts.into(),
                        input.retry_backoff_secs.into(),
                        input.timeout_seconds.into(),
                        next_run_at.into(),
                    ],
                ))
                .await?,
        )?;

        row_to_job(&row)
    }

    pub async fn list(&self, tenant_id: Option<Uuid>) -> AppResult<Vec<ScheduledJob>> {
        let mut query = scheduled_jobs::Entity::find();
        if let Some(tid) = tenant_id {
            query = query.filter(scheduled_jobs::Column::TenantId.eq(tid));
        }
        let models = query
            .order_by_asc(scheduled_jobs::Column::Kind)
            .order_by_asc(scheduled_jobs::Column::Name)
            .all(&self.pool)
            .await?;

        models.into_iter().map(model_to_job).collect()
    }

    /// Global lookup, only used by admin and the scheduler runner (which
    /// has already proven ownership via `job_runs.tenant_id`). API
    /// callers MUST go through [`Self::get_for_tenant`].
    pub async fn get(&self, id: Uuid) -> AppResult<ScheduledJob> {
        let model = scheduled_jobs::Entity::find_by_id(id)
            .one(&self.pool)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("scheduled_job {id}")))?;
        model_to_job(model)
    }

    /// Tenant-scoped lookup. `tenant_id=NULL` jobs (system jobs) are
    /// not visible through this accessor — those are admin-only.
    /// Returns NotFound (not Forbidden) on cross-tenant access to
    /// avoid leaking whether a given job UUID exists.
    pub async fn get_for_tenant(&self, tenant_id: Uuid, id: Uuid) -> AppResult<ScheduledJob> {
        let model = scheduled_jobs::Entity::find()
            .filter(scheduled_jobs::Column::Id.eq(id))
            .filter(scheduled_jobs::Column::TenantId.eq(tenant_id))
            .one(&self.pool)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("scheduled_job {id}")))?;
        model_to_job(model)
    }

    /// Admin-only: unscoped enable/disable. API callers must use
    /// [`Self::set_enabled_for_tenant`].
    pub async fn set_enabled(&self, id: Uuid, enabled: bool) -> AppResult<()> {
        self.pool
            .execute(stmt(
                r#"UPDATE scheduled_jobs SET enabled = $2, updated_at = now() WHERE id = $1"#,
                [id.into(), enabled.into()],
            ))
            .await?;
        Ok(())
    }

    /// Tenant-scoped enable/disable. Returns NotFound when the job
    /// belongs to a different tenant.
    pub async fn set_enabled_for_tenant(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        enabled: bool,
    ) -> AppResult<()> {
        let n = self
            .pool
            .execute(stmt(
                r#"
            UPDATE scheduled_jobs
            SET enabled = $3, updated_at = now()
            WHERE id = $1 AND tenant_id = $2
            "#,
                [id.into(), tenant_id.into(), enabled.into()],
            ))
            .await?
            .rows_affected();
        if n == 0 {
            return Err(AppError::NotFound(format!("scheduled_job {id}")));
        }
        Ok(())
    }

    /// Create a one-shot job that fires immediately. Used by on-demand API
    /// endpoints (e.g. "refresh this connection now"). The job auto-disables
    /// after success; failures retry per `max_attempts`. Each call gets a
    /// unique `name` so multiple concurrent on-demand requests are independent
    /// rows (intra-resource concurrency is controlled by the handler via the
    /// lock service).
    pub async fn enqueue_one_shot(
        &self,
        tenant_id: Uuid,
        region: Region,
        kind: impl Into<String>,
        name_prefix: impl Into<String>,
        payload: serde_json::Value,
    ) -> AppResult<ScheduledJob> {
        let req_id = Uuid::new_v4();
        let kind = kind.into();
        let name = format!("{}/{}", name_prefix.into(), req_id);
        self.create(
            Some(tenant_id),
            Some(region),
            CreateScheduledJob {
                kind,
                name,
                schedule_kind: ScheduleKind::OneShot,
                cron_expr: None,
                interval_seconds: None,
                one_shot_at: Some(Utc::now()),
                timezone: "UTC".into(),
                payload,
                enabled: true,
                max_attempts: 3,
                retry_backoff_secs: 15,
                timeout_seconds: 300,
            },
        )
        .await
    }

    /// Force the next_run_at to "now" so the next dispatcher tick will
    /// pick it up. Admin/system path — bypasses tenant ownership.
    pub async fn run_now(&self, id: Uuid) -> AppResult<()> {
        self.pool
            .execute(stmt(
                r#"UPDATE scheduled_jobs SET next_run_at = now(), updated_at = now() WHERE id = $1"#,
                [id.into()],
            ))
            .await?;
        Ok(())
    }

    /// Tenant-scoped variant of [`Self::run_now`].
    pub async fn run_now_for_tenant(&self, tenant_id: Uuid, id: Uuid) -> AppResult<()> {
        let n = self
            .pool
            .execute(stmt(
                r#"
            UPDATE scheduled_jobs
            SET next_run_at = now(), updated_at = now()
            WHERE id = $1 AND tenant_id = $2
            "#,
                [id.into(), tenant_id.into()],
            ))
            .await?
            .rows_affected();
        if n == 0 {
            return Err(AppError::NotFound(format!("scheduled_job {id}")));
        }
        Ok(())
    }

    /// Recent runs across all jobs for a tenant (or globally if `tenant_id`
    /// is `None`). Used by the admin dashboard for an at-a-glance health view.
    pub async fn recent_runs(&self, tenant_id: Option<Uuid>, limit: i64) -> AppResult<Vec<JobRun>> {
        let limit = limit.clamp(1, 500);
        let mut query = job_runs::Entity::find();
        if let Some(tid) = tenant_id {
            query = query.filter(job_runs::Column::TenantId.eq(tid));
        }
        let models = query
            .order_by_desc(job_runs::Column::ScheduledFor)
            .limit(limit as u64)
            .all(&self.pool)
            .await?;
        models.into_iter().map(model_to_run).collect()
    }

    /// Aggregate counts for the admin dashboard: `(total, enabled, due_now)`.
    pub async fn counts(&self, tenant_id: Option<Uuid>) -> AppResult<JobCounts> {
        // Raw SQL (SeaORM Statement): `COUNT(*) FILTER (WHERE ...)` aggregates.
        let row = match tenant_id {
            Some(tid) => require_row(
                self.pool
                    .query_one(stmt(
                        r#"
                    SELECT COUNT(*)                                AS total,
                           COUNT(*) FILTER (WHERE enabled)         AS enabled,
                           COUNT(*) FILTER (WHERE enabled
                                                  AND next_run_at <= now())
                                                                   AS due_now
                    FROM scheduled_jobs
                    WHERE tenant_id = $1
                    "#,
                        [tid.into()],
                    ))
                    .await?,
            )?,
            None => require_row(
                self.pool
                    .query_one(stmt(
                        r#"
                    SELECT COUNT(*)                                AS total,
                           COUNT(*) FILTER (WHERE enabled)         AS enabled,
                           COUNT(*) FILTER (WHERE enabled
                                                  AND next_run_at <= now())
                                                                   AS due_now
                    FROM scheduled_jobs
                    "#,
                        [],
                    ))
                    .await?,
            )?,
        };
        Ok(JobCounts {
            total: row.try_get("", "total")?,
            enabled: row.try_get("", "enabled")?,
            due_now: row.try_get("", "due_now")?,
        })
    }

    /// Admin-only: unscoped list_runs. Currently unused (admin
    /// dashboard surfaces job runs via [`Self::recent_runs`] instead);
    /// keep it for the planned per-job admin drill-down.
    #[allow(dead_code)]
    pub async fn list_runs(&self, job_id: Uuid, limit: i64) -> AppResult<Vec<JobRun>> {
        let models = job_runs::Entity::find()
            .filter(job_runs::Column::JobId.eq(job_id))
            .order_by_desc(job_runs::Column::ScheduledFor)
            .limit(limit.max(0) as u64)
            .all(&self.pool)
            .await?;
        models.into_iter().map(model_to_run).collect()
    }

    /// Tenant-scoped list_runs. Filters job_runs by both `job_id` AND
    /// `tenant_id` so a leaked job UUID does not let another tenant
    /// read its run history.
    pub async fn list_runs_for_tenant(
        &self,
        tenant_id: Uuid,
        job_id: Uuid,
        limit: i64,
    ) -> AppResult<Vec<JobRun>> {
        let models = job_runs::Entity::find()
            .filter(job_runs::Column::JobId.eq(job_id))
            .filter(job_runs::Column::TenantId.eq(tenant_id))
            .order_by_desc(job_runs::Column::ScheduledFor)
            .limit(limit.max(0) as u64)
            .all(&self.pool)
            .await?;
        models.into_iter().map(model_to_run).collect()
    }
}

fn validate_schedule(s: &CreateScheduledJob) -> AppResult<()> {
    match s.schedule_kind {
        ScheduleKind::Cron => {
            let expr = s.cron_expr.as_deref().ok_or_else(|| {
                AppError::BadRequest("cron_expr required for schedule_kind=cron".into())
            })?;
            cron::Schedule::try_from(expr)
                .map_err(|e| AppError::BadRequest(format!("invalid cron_expr {expr:?}: {e}")))?;
        }
        ScheduleKind::Interval => {
            let secs = s.interval_seconds.ok_or_else(|| {
                AppError::BadRequest("interval_seconds required for schedule_kind=interval".into())
            })?;
            if secs < 1 || secs > 365 * 24 * 3600 {
                return Err(AppError::BadRequest(
                    "interval_seconds must be 1..=31536000".into(),
                ));
            }
        }
        ScheduleKind::OneShot => {
            if s.one_shot_at.is_none() {
                return Err(AppError::BadRequest(
                    "one_shot_at required for schedule_kind=one_shot".into(),
                ));
            }
        }
    }
    if !s.timezone.is_empty() {
        let _: chrono_tz::Tz = s
            .timezone
            .parse()
            .map_err(|_| AppError::BadRequest(format!("unknown IANA timezone: {}", s.timezone)))?;
    }
    Ok(())
}

/// Compute the next firing time for a job in the given timezone.
pub fn compute_next_run(
    kind: ScheduleKind,
    cron_expr: Option<&str>,
    interval_seconds: Option<i32>,
    one_shot_at: Option<DateTime<Utc>>,
    timezone: &str,
    after: DateTime<Utc>,
) -> AppResult<DateTime<Utc>> {
    match kind {
        ScheduleKind::Cron => {
            let tz: chrono_tz::Tz = timezone
                .parse()
                .map_err(|_| AppError::BadRequest(format!("unknown timezone {timezone}")))?;
            let expr = cron_expr.ok_or_else(|| AppError::BadRequest("missing cron_expr".into()))?;
            let sched = cron::Schedule::try_from(expr)
                .map_err(|e| AppError::BadRequest(format!("invalid cron_expr: {e}")))?;
            let after_tz = after.with_timezone(&tz);
            let next = sched
                .after(&after_tz)
                .next()
                .ok_or_else(|| AppError::BadRequest("cron has no next firing".into()))?;
            Ok(next.with_timezone(&Utc))
        }
        ScheduleKind::Interval => {
            let secs = interval_seconds.unwrap_or(60).max(1);
            Ok(after + chrono::Duration::seconds(secs as i64))
        }
        ScheduleKind::OneShot => {
            // Returned as-is; once a one-shot fires, the runner sets enabled=false.
            Ok(one_shot_at.unwrap_or(after))
        }
    }
}

fn schedule_kind_tag(k: ScheduleKind) -> &'static str {
    match k {
        ScheduleKind::Cron => "cron",
        ScheduleKind::Interval => "interval",
        ScheduleKind::OneShot => "one_shot",
    }
}

fn model_to_job(m: scheduled_jobs::Model) -> AppResult<ScheduledJob> {
    Ok(ScheduledJob {
        id: m.id,
        tenant_id: m.tenant_id,
        kind: m.kind,
        name: m.name,
        schedule_kind: decode_enum("schedule_kind", &m.schedule_kind)?,
        cron_expr: m.cron_expr,
        interval_seconds: m.interval_seconds,
        one_shot_at: m.one_shot_at.map(|t| t.with_timezone(&Utc)),
        timezone: m.timezone,
        payload: m.payload,
        enabled: m.enabled,
        max_attempts: m.max_attempts,
        retry_backoff_secs: m.retry_backoff_secs,
        timeout_seconds: m.timeout_seconds,
        next_run_at: m.next_run_at.with_timezone(&Utc),
        last_run_at: m.last_run_at.map(|t| t.with_timezone(&Utc)),
        created_at: m.created_at.with_timezone(&Utc),
    })
}

fn model_to_run(m: job_runs::Model) -> AppResult<JobRun> {
    Ok(JobRun {
        id: m.id,
        job_id: m.job_id,
        tenant_id: m.tenant_id,
        attempt: m.attempt,
        status: decode_enum("status", &m.status)?,
        scheduled_for: m.scheduled_for.with_timezone(&Utc),
        claimed_at: m.claimed_at.map(|t| t.with_timezone(&Utc)),
        claimed_by: m.claimed_by,
        finished_at: m.finished_at.map(|t| t.with_timezone(&Utc)),
        duration_ms: m.duration_ms,
        output: m.output,
        error: m.error,
        idempotency_key: m.idempotency_key,
    })
}

fn row_to_job(row: &sea_orm::QueryResult) -> AppResult<ScheduledJob> {
    let kind_label: String = row.try_get("", "schedule_kind")?;
    Ok(ScheduledJob {
        id: row.try_get("", "id")?,
        tenant_id: row.try_get("", "tenant_id")?,
        kind: row.try_get("", "kind")?,
        name: row.try_get("", "name")?,
        schedule_kind: decode_enum("schedule_kind", &kind_label)?,
        cron_expr: row.try_get("", "cron_expr")?,
        interval_seconds: row.try_get("", "interval_seconds")?,
        one_shot_at: row.try_get("", "one_shot_at")?,
        timezone: row.try_get("", "timezone")?,
        payload: row.try_get("", "payload")?,
        enabled: row.try_get("", "enabled")?,
        max_attempts: row.try_get("", "max_attempts")?,
        retry_backoff_secs: row.try_get("", "retry_backoff_secs")?,
        timeout_seconds: row.try_get("", "timeout_seconds")?,
        next_run_at: row.try_get("", "next_run_at")?,
        last_run_at: row.try_get("", "last_run_at")?,
        created_at: row.try_get("", "created_at")?,
    })
}
