use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "schedule_kind", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ScheduleKind {
    Cron,
    Interval,
    OneShot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "job_run_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum JobRunStatus {
    Pending,
    Claimed,
    Succeeded,
    Failed,
    DeadLettered,
    Cancelled,
}

#[derive(Clone, Debug, Serialize)]
pub struct ScheduledJob {
    pub id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub kind: String,
    pub name: String,
    pub schedule_kind: ScheduleKind,
    pub cron_expr: Option<String>,
    pub interval_seconds: Option<i32>,
    pub one_shot_at: Option<DateTime<Utc>>,
    pub timezone: String,
    pub payload: serde_json::Value,
    pub enabled: bool,
    pub max_attempts: i32,
    pub retry_backoff_secs: i32,
    pub timeout_seconds: i32,
    pub next_run_at: DateTime<Utc>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct JobRun {
    pub id: i64,
    pub job_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub attempt: i32,
    pub status: JobRunStatus,
    pub scheduled_for: DateTime<Utc>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub claimed_by: Option<String>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i32>,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub idempotency_key: String,
}

/// Aggregate scheduler stats used by the admin dashboard.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct JobCounts {
    pub total: i64,
    pub enabled: i64,
    pub due_now: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateScheduledJob {
    pub kind: String,
    pub name: String,
    pub schedule_kind: ScheduleKind,
    pub cron_expr: Option<String>,
    pub interval_seconds: Option<i32>,
    pub one_shot_at: Option<DateTime<Utc>>,
    #[serde(default = "default_tz")]
    pub timezone: String,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: i32,
    #[serde(default = "default_retry_backoff")]
    pub retry_backoff_secs: i32,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: i32,
}

fn default_tz() -> String {
    "UTC".into()
}
fn default_enabled() -> bool {
    true
}
fn default_max_attempts() -> i32 {
    5
}
fn default_retry_backoff() -> i32 {
    30
}
fn default_timeout() -> i32 {
    300
}
