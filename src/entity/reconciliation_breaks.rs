//! SeaORM entity for `reconciliation_breaks` — drift between provider truth
//! and the ledger. Open breaks are unique per
//! `(provider, connection_id, break_type, external_ref)` via a partial index.
//! Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "reconciliation_breaks")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub tenant_id: Uuid,
    pub shard_key: i64,
    #[sea_orm(
        column_type = r#"custom("provider_kind")"#,
        select_as = "text",
        save_as = "provider_kind"
    )]
    pub provider: String,
    pub connection_id: Option<Uuid>,
    pub break_type: String,
    pub external_ref: Option<String>,
    pub transaction_id: Option<Uuid>,
    #[sea_orm(column_type = "Decimal(Some((38, 0)))")]
    pub expected_minor: Option<Decimal>,
    #[sea_orm(column_type = "Decimal(Some((38, 0)))")]
    pub actual_minor: Option<Decimal>,
    #[sea_orm(column_type = "Char(Some(3))")]
    pub currency: Option<String>,
    #[sea_orm(
        column_type = r#"custom("break_status")"#,
        select_as = "text",
        save_as = "break_status"
    )]
    pub status: String,
    pub notes: Option<String>,
    pub detected_at: DateTimeWithTimeZone,
    pub resolved_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(column_type = "JsonBinary")]
    pub metadata: Json,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
