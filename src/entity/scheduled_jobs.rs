//! SeaORM entity for `scheduled_jobs` — the durable scheduler catalog of
//! *what should run when* (cron / interval / one-shot). `tenant_id` is
//! optional so system jobs live in the same table as tenant jobs.
//! Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "scheduled_jobs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub shard_key: i64,
    pub kind: String,
    pub name: String,
    #[sea_orm(
        column_type = r#"custom("schedule_kind")"#,
        select_as = "text",
        save_as = "schedule_kind"
    )]
    pub schedule_kind: String,
    pub cron_expr: Option<String>,
    pub interval_seconds: Option<i32>,
    pub one_shot_at: Option<DateTimeWithTimeZone>,
    pub timezone: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub payload: Json,
    pub enabled: bool,
    pub max_attempts: i32,
    pub retry_backoff_secs: i32,
    pub timeout_seconds: i32,
    pub next_run_at: DateTimeWithTimeZone,
    pub last_run_at: Option<DateTimeWithTimeZone>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
