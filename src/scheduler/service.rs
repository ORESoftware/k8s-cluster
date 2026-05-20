use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::shard::{Region, ShardKey};

use super::types::*;

#[derive(Clone)]
pub struct SchedulerService {
    pool: PgPool,
}

impl SchedulerService {
    pub fn new(pool: PgPool) -> Self {
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

        let row = sqlx::query(
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
                      schedule_kind AS "schedule_kind: ScheduleKind",
                      cron_expr, interval_seconds, one_shot_at, timezone,
                      payload, enabled, max_attempts, retry_backoff_secs,
                      timeout_seconds, next_run_at, last_run_at, created_at
            "#,
        )
        .bind(tenant_id)
        .bind(shard)
        .bind(&input.kind)
        .bind(&input.name)
        .bind(schedule_kind_tag(input.schedule_kind))
        .bind(&input.cron_expr)
        .bind(input.interval_seconds)
        .bind(input.one_shot_at)
        .bind(&input.timezone)
        .bind(&input.payload)
        .bind(input.enabled)
        .bind(input.max_attempts)
        .bind(input.retry_backoff_secs)
        .bind(input.timeout_seconds)
        .bind(next_run_at)
        .fetch_one(&self.pool)
        .await?;

        row_to_job(&row)
    }

    pub async fn list(&self, tenant_id: Option<Uuid>) -> AppResult<Vec<ScheduledJob>> {
        let rows = match tenant_id {
            Some(tid) => {
                sqlx::query(
                    r#"
                    SELECT id, tenant_id, shard_key, kind, name,
                           schedule_kind AS "schedule_kind: ScheduleKind",
                           cron_expr, interval_seconds, one_shot_at, timezone,
                           payload, enabled, max_attempts, retry_backoff_secs,
                           timeout_seconds, next_run_at, last_run_at, created_at
                    FROM scheduled_jobs
                    WHERE tenant_id = $1
                    ORDER BY kind, name
                    "#,
                )
                .bind(tid)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    r#"
                    SELECT id, tenant_id, shard_key, kind, name,
                           schedule_kind AS "schedule_kind: ScheduleKind",
                           cron_expr, interval_seconds, one_shot_at, timezone,
                           payload, enabled, max_attempts, retry_backoff_secs,
                           timeout_seconds, next_run_at, last_run_at, created_at
                    FROM scheduled_jobs
                    ORDER BY kind, name
                    "#,
                )
                .fetch_all(&self.pool)
                .await?
            }
        };

        rows.iter().map(row_to_job).collect()
    }

    pub async fn get(&self, id: Uuid) -> AppResult<ScheduledJob> {
        let row = sqlx::query(
            r#"
            SELECT id, tenant_id, shard_key, kind, name,
                   schedule_kind AS "schedule_kind: ScheduleKind",
                   cron_expr, interval_seconds, one_shot_at, timezone,
                   payload, enabled, max_attempts, retry_backoff_secs,
                   timeout_seconds, next_run_at, last_run_at, created_at
            FROM scheduled_jobs WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("scheduled_job {id}")))?;

        row_to_job(&row)
    }

    pub async fn set_enabled(&self, id: Uuid, enabled: bool) -> AppResult<()> {
        sqlx::query(r#"UPDATE scheduled_jobs SET enabled = $2, updated_at = now() WHERE id = $1"#)
            .bind(id)
            .bind(enabled)
            .execute(&self.pool)
            .await?;
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

    /// Force the next_run_at to "now" so the next dispatcher tick will pick it up.
    pub async fn run_now(&self, id: Uuid) -> AppResult<()> {
        sqlx::query(
            r#"UPDATE scheduled_jobs SET next_run_at = now(), updated_at = now() WHERE id = $1"#,
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_runs(&self, job_id: Uuid, limit: i64) -> AppResult<Vec<JobRun>> {
        let rows = sqlx::query(
            r#"
            SELECT id, job_id, tenant_id, attempt,
                   status AS "status: JobRunStatus",
                   scheduled_for, claimed_at, claimed_by, finished_at,
                   duration_ms, output, error, idempotency_key
            FROM job_runs
            WHERE job_id = $1
            ORDER BY scheduled_for DESC
            LIMIT $2
            "#,
        )
        .bind(job_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_run).collect()
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

fn row_to_job(row: &sqlx::postgres::PgRow) -> AppResult<ScheduledJob> {
    Ok(ScheduledJob {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        kind: row.try_get("kind")?,
        name: row.try_get("name")?,
        schedule_kind: row.try_get("schedule_kind")?,
        cron_expr: row.try_get("cron_expr")?,
        interval_seconds: row.try_get("interval_seconds")?,
        one_shot_at: row.try_get("one_shot_at")?,
        timezone: row.try_get("timezone")?,
        payload: row.try_get("payload")?,
        enabled: row.try_get("enabled")?,
        max_attempts: row.try_get("max_attempts")?,
        retry_backoff_secs: row.try_get("retry_backoff_secs")?,
        timeout_seconds: row.try_get("timeout_seconds")?,
        next_run_at: row.try_get("next_run_at")?,
        last_run_at: row.try_get("last_run_at")?,
        created_at: row.try_get("created_at")?,
    })
}

fn row_to_run(row: &sqlx::postgres::PgRow) -> AppResult<JobRun> {
    Ok(JobRun {
        id: row.try_get("id")?,
        job_id: row.try_get("job_id")?,
        tenant_id: row.try_get("tenant_id")?,
        attempt: row.try_get("attempt")?,
        status: row.try_get("status")?,
        scheduled_for: row.try_get("scheduled_for")?,
        claimed_at: row.try_get("claimed_at")?,
        claimed_by: row.try_get("claimed_by")?,
        finished_at: row.try_get("finished_at")?,
        duration_ms: row.try_get("duration_ms")?,
        output: row.try_get("output")?,
        error: row.try_get("error")?,
        idempotency_key: row.try_get("idempotency_key")?,
    })
}
