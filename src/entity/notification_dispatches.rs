//! SeaORM entity for `notification_dispatches` — one row per outbound send;
//! per-day throttling and dedupe run against this table.
//! Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "notification_dispatches")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub rule_id: Uuid,
    pub tenant_id: Uuid,
    pub shard_key: i64,
    /// The thing the dispatch is about, e.g. user_id or invoice_id.
    pub target_resource: Option<String>,
    #[sea_orm(
        column_type = r#"custom("notification_channel")"#,
        select_as = "text",
        save_as = "notification_channel"
    )]
    pub channel: String,
    pub target: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub payload: Json,
    #[sea_orm(
        column_type = r#"custom("notification_dispatch_status")"#,
        select_as = "text",
        save_as = "notification_dispatch_status"
    )]
    pub status: String,
    pub provider_message_id: Option<String>,
    pub error: Option<String>,
    pub sent_at: Option<DateTimeWithTimeZone>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
