//! SeaORM entity for `dead_letter_jobs` — failures copied here after
//! `max_attempts` so they surface on the breaks/ops dashboard.
//! Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "dead_letter_jobs")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub job_id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub last_run_id: Option<i64>,
    pub final_attempt: i32,
    pub error: Option<String>,
    pub occurred_at: DateTimeWithTimeZone,
    pub acknowledged_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
