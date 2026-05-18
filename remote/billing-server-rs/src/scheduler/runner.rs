use chrono::Utc;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::error::AppResult;

use super::handler::{HandlerRegistry, JobContext};
use super::service::compute_next_run;
use super::types::{JobRunStatus, ScheduleKind};

/// The dispatcher loop. Spawn this once per pod; PG-level SKIP LOCKED makes
/// running N pods safe — each due job is claimed exactly once per tick.
pub struct SchedulerRunner {
    pool: PgPool,
    handlers: HandlerRegistry,
    worker_id: String,
    poll_interval: Duration,
}

impl SchedulerRunner {
    pub fn new(pool: PgPool, handlers: HandlerRegistry) -> Self {
        let worker_id = format!(
            "{}-{}",
            std::env::var("HOSTNAME").unwrap_or_else(|_| "local".into()),
            std::process::id()
        );
        Self {
            pool,
            handlers,
            worker_id,
            poll_interval: Duration::from_secs(5),
        }
    }

    pub fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    pub async fn run_forever(self: Arc<Self>) {
        tracing::info!(
            worker = %self.worker_id,
            kinds = ?self.handlers.known_kinds(),
            "scheduler runner started"
        );
        loop {
            match self.tick().await {
                Ok(n) if n > 0 => {
                    tracing::debug!(claimed = n, "scheduler tick");
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::error!(error = %e, "scheduler tick failed");
                }
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// One sweep: claim up to N due jobs, run each, advance schedules.
    pub async fn tick(&self) -> AppResult<usize> {
        let batch_size: i64 = 32;

        let claims = sqlx::query(
            r#"
            WITH due AS (
                SELECT id
                FROM scheduled_jobs
                WHERE enabled = true AND next_run_at <= now()
                ORDER BY next_run_at ASC
                FOR UPDATE SKIP LOCKED
                LIMIT $1
            )
            UPDATE scheduled_jobs sj
            SET next_run_at = now() + (sj.timeout_seconds || ' seconds')::interval,
                last_run_at = now()
            FROM due
            WHERE sj.id = due.id
            RETURNING sj.id, sj.tenant_id, sj.kind, sj.name, sj.payload,
                      sj.schedule_kind AS "schedule_kind: ScheduleKind",
                      sj.cron_expr, sj.interval_seconds, sj.one_shot_at,
                      sj.timezone, sj.max_attempts, sj.retry_backoff_secs,
                      sj.timeout_seconds
            "#,
        )
        .bind(batch_size)
        .fetch_all(&self.pool)
        .await?;

        let n = claims.len();
        for row in claims {
            let job_id: Uuid = row.try_get("id")?;
            let tenant_id: Option<Uuid> = row.try_get("tenant_id")?;
            let kind: String = row.try_get("kind")?;
            let name: String = row.try_get("name")?;
            let payload: serde_json::Value = row.try_get("payload")?;
            let max_attempts: i32 = row.try_get("max_attempts")?;
            let retry_backoff_secs: i32 = row.try_get("retry_backoff_secs")?;
            let sched_kind: ScheduleKind = row.try_get("schedule_kind")?;
            let cron_expr: Option<String> = row.try_get("cron_expr")?;
            let interval_seconds: Option<i32> = row.try_get("interval_seconds")?;
            let one_shot_at: Option<chrono::DateTime<Utc>> = row.try_get("one_shot_at")?;
            let timezone: String = row.try_get("timezone")?;

            self.dispatch_one(
                job_id, tenant_id, kind, name, payload,
                max_attempts, retry_backoff_secs,
                sched_kind, cron_expr.as_deref(), interval_seconds,
                one_shot_at, &timezone,
            ).await;
        }

        Ok(n)
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_one(
        &self,
        job_id: Uuid,
        tenant_id: Option<Uuid>,
        kind: String,
        name: String,
        payload: serde_json::Value,
        max_attempts: i32,
        retry_backoff_secs: i32,
        sched_kind: ScheduleKind,
        cron_expr: Option<&str>,
        interval_seconds: Option<i32>,
        one_shot_at: Option<chrono::DateTime<Utc>>,
        timezone: &str,
    ) {
        // Determine current attempt number from prior failed runs since last success.
        let attempt = self.next_attempt(job_id).await.unwrap_or(1);
        let scheduled_for = Utc::now();
        let idem_key = format!("{job_id}/{}/{attempt}", scheduled_for.timestamp());

        let run_id = match self.insert_run(
            job_id, tenant_id, attempt, scheduled_for, &idem_key,
        ).await {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(job_id = %job_id, error = %e, "failed to insert job_run");
                return;
            }
        };

        let start = std::time::Instant::now();
        let handler = self.handlers.get(&kind);

        let outcome: AppResult<super::handler::JobOutput> = match handler {
            None => Err(crate::error::AppError::BadRequest(format!(
                "no registered handler for kind={kind}"
            ))),
            Some(h) => {
                let ctx = JobContext {
                    pool: self.pool.clone(),
                    job_id,
                    tenant_id,
                    kind: kind.clone(),
                    name: name.clone(),
                    payload: payload.clone(),
                    attempt,
                    idempotency_key: idem_key.clone(),
                };
                h.run(&ctx).await
            }
        };

        let duration_ms = start.elapsed().as_millis() as i32;

        match outcome {
            Ok(out) => {
                let _ = self.mark_run_succeeded(run_id, duration_ms, &out).await;
                let next = compute_next_run(
                    sched_kind, cron_expr, interval_seconds, one_shot_at, timezone, Utc::now(),
                );
                let next = next.unwrap_or_else(|_| Utc::now() + chrono::Duration::minutes(5));

                if sched_kind == ScheduleKind::OneShot {
                    let _ = sqlx::query(
                        r#"UPDATE scheduled_jobs
                           SET enabled = false, next_run_at = $2, updated_at = now()
                           WHERE id = $1"#,
                    ).bind(job_id).bind(next).execute(&self.pool).await;
                } else {
                    let _ = sqlx::query(
                        r#"UPDATE scheduled_jobs SET next_run_at = $2, updated_at = now()
                           WHERE id = $1"#,
                    ).bind(job_id).bind(next).execute(&self.pool).await;
                }
            }
            Err(e) => {
                let err_str = e.to_string();
                let _ = self.mark_run_failed(run_id, duration_ms, &err_str).await;

                if attempt >= max_attempts {
                    let _ = self.dead_letter(job_id, tenant_id, run_id, attempt, &err_str).await;
                    let next = compute_next_run(
                        sched_kind, cron_expr, interval_seconds, one_shot_at, timezone, Utc::now(),
                    );
                    let next = next.unwrap_or_else(|_| Utc::now() + chrono::Duration::hours(1));
                    let _ = sqlx::query(
                        r#"UPDATE scheduled_jobs SET next_run_at = $2, updated_at = now()
                           WHERE id = $1"#,
                    ).bind(job_id).bind(next).execute(&self.pool).await;
                } else {
                    let backoff = exponential_backoff(retry_backoff_secs, attempt);
                    let retry_at = Utc::now() + chrono::Duration::seconds(backoff as i64);
                    let _ = sqlx::query(
                        r#"UPDATE scheduled_jobs SET next_run_at = $2, updated_at = now()
                           WHERE id = $1"#,
                    ).bind(job_id).bind(retry_at).execute(&self.pool).await;
                }
            }
        }
    }

    async fn next_attempt(&self, job_id: Uuid) -> AppResult<i32> {
        // attempt = (consecutive failed runs since last success) + 1
        let row = sqlx::query(
            r#"
            WITH last_success AS (
                SELECT MAX(scheduled_for) AS ts
                FROM job_runs WHERE job_id = $1 AND status = 'succeeded'
            )
            SELECT COUNT(*)::INT AS fails
            FROM job_runs r, last_success ls
            WHERE r.job_id = $1
              AND r.status = 'failed'
              AND (ls.ts IS NULL OR r.scheduled_for > ls.ts)
            "#,
        )
        .bind(job_id)
        .fetch_one(&self.pool)
        .await?;
        let fails: i32 = row.try_get("fails")?;
        Ok(fails + 1)
    }

    async fn insert_run(
        &self,
        job_id: Uuid,
        tenant_id: Option<Uuid>,
        attempt: i32,
        scheduled_for: chrono::DateTime<Utc>,
        idem: &str,
    ) -> AppResult<i64> {
        let id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO job_runs
                (job_id, tenant_id, attempt, status, scheduled_for,
                 claimed_at, claimed_by, idempotency_key)
            VALUES ($1, $2, $3, 'claimed'::job_run_status, $4, now(), $5, $6)
            RETURNING id
            "#,
        )
        .bind(job_id)
        .bind(tenant_id)
        .bind(attempt)
        .bind(scheduled_for)
        .bind(&self.worker_id)
        .bind(idem)
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    async fn mark_run_succeeded(
        &self,
        run_id: i64,
        duration_ms: i32,
        out: &super::handler::JobOutput,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE job_runs
            SET status = 'succeeded'::job_run_status,
                finished_at = now(),
                duration_ms = $2,
                output = $3
            WHERE id = $1
            "#,
        )
        .bind(run_id)
        .bind(duration_ms)
        .bind(serde_json::to_value(out).unwrap_or(serde_json::Value::Null))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mark_run_failed(
        &self,
        run_id: i64,
        duration_ms: i32,
        error: &str,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE job_runs
            SET status = 'failed'::job_run_status,
                finished_at = now(),
                duration_ms = $2,
                error = $3
            WHERE id = $1
            "#,
        )
        .bind(run_id)
        .bind(duration_ms)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn dead_letter(
        &self,
        job_id: Uuid,
        tenant_id: Option<Uuid>,
        last_run_id: i64,
        final_attempt: i32,
        error: &str,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            INSERT INTO dead_letter_jobs
                (job_id, tenant_id, last_run_id, final_attempt, error)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(job_id)
        .bind(tenant_id)
        .bind(last_run_id)
        .bind(final_attempt)
        .bind(error)
        .execute(&self.pool)
        .await?;

        // Mark the run row as dead-lettered for clear status semantics.
        sqlx::query(
            r#"UPDATE job_runs SET status = 'dead_lettered'::job_run_status WHERE id = $1"#,
        )
        .bind(last_run_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn exponential_backoff(base_secs: i32, attempt: i32) -> i32 {
    let capped = attempt.min(10);
    let factor: i64 = 1i64 << (capped - 1).max(0);
    let secs = (base_secs as i64).saturating_mul(factor).min(3600);
    secs as i32
}
