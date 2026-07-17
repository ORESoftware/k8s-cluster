use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "gh_secret_sync_runs")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub repo: String,
    pub secret_name: String,
    pub value_sha256: String,
    pub status: String,
    pub detail: Option<String>,
    pub synced_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
