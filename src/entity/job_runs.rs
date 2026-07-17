//! SeaORM entity for `job_runs` — the durable history of every scheduler
//! attempt, unique per `(job_id, idempotency_key)`.
//! Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "job_runs")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub job_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub shard_key: i64,
    pub attempt: i32,
    #[sea_orm(
        column_type = r#"custom("job_run_status")"#,
        select_as = "text",
        save_as = "job_run_status"
    )]
    pub status: String,
    pub scheduled_for: DateTimeWithTimeZone,
    pub claimed_at: Option<DateTimeWithTimeZone>,
    /// Pod / worker id.
    pub claimed_by: Option<String>,
    pub finished_at: Option<DateTimeWithTimeZone>,
    pub duration_ms: Option<i32>,
    #[sea_orm(column_type = "JsonBinary")]
    pub output: Option<Json>,
    pub error: Option<String>,
    pub idempotency_key: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
