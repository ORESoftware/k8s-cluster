//! A Vapi.ai call, upserted from webhook lifecycle events keyed by Vapi's
//! call id.

use sea_orm::entity::prelude::*;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize)]
#[sea_orm(table_name = "vapi_calls")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(unique)]
    pub vapi_call_id: String,
    /// Last observed status ("queued", "ringing", "in-progress", "ended", …).
    pub status: String,
    pub ended_reason: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub transcript: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub summary: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
