//! SeaORM entity for `notification_rules` — "when condition X is true for
//! entity Y in tenant T, send a message via channel Z to target W".
//! Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "notification_rules")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub shard_key: i64,
    pub kind: String,
    pub name: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub params: Json,
    #[sea_orm(
        column_type = r#"custom("notification_channel")"#,
        select_as = "text",
        save_as = "notification_channel"
    )]
    pub channel: String,
    pub target: String,
    /// Per-channel auth/signing material, sealed in the same envelope shape
    /// as provider credentials.
    #[sea_orm(column_type = "JsonBinary")]
    pub sealed_credential: Option<Json>,
    pub template_id: Option<String>,
    pub throttle_per_day: i32,
    pub enabled: bool,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
