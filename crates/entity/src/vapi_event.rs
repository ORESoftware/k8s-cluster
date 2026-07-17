//! Raw Vapi webhook events, kept for audit/debugging (payload truncated at
//! the API layer before insert).

use sea_orm::entity::prelude::*;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize)]
#[sea_orm(table_name = "vapi_events")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub vapi_call_id: Option<String>,
    pub event_type: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub payload: Json,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
