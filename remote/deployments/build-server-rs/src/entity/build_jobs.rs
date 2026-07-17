use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "build_jobs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub status: String,
    pub job_kind: String,
    pub source: String,
    pub executor: String,
    pub repo_url: String,
    pub git_ref: Option<String>,
    pub image: String,
    pub request: Json,
    pub error: Option<String>,
    pub log_path: Option<String>,
    pub lock_key: Option<String>,
    pub fencing_token: Option<i64>,
    pub created_at: DateTimeWithTimeZone,
    pub started_at: Option<DateTimeWithTimeZone>,
    pub finished_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
