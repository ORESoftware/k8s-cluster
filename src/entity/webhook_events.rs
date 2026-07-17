//! SeaORM entity for `webhook_events` — raw inbound provider deliveries kept
//! for replay + audit. Bodies are sealed at rest into `payload_sealed`; the
//! legacy plaintext `payload` column is nullable and no longer written.
//! Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "webhook_events")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub connection_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    #[sea_orm(
        column_type = r#"custom("provider_kind")"#,
        select_as = "text",
        save_as = "provider_kind"
    )]
    pub provider: String,
    pub external_event_id: String,
    pub event_type: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub payload: Option<Json>,
    pub signature_ok: bool,
    pub processed_at: Option<DateTimeWithTimeZone>,
    pub process_error: Option<String>,
    pub received_at: DateTimeWithTimeZone,
    pub payload_sha256: Option<String>,
    pub verification_error: Option<String>,
    pub external_account_id: Option<String>,
    #[sea_orm(column_type = "JsonBinary")]
    pub payload_sealed: Option<Json>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
