//! SeaORM entity for `provider_balance_snapshots` — per-merchant wallet
//! balance snapshots (reserved for the wallet-balance reconciler).
//! Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "provider_balance_snapshots")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub tenant_id: Uuid,
    pub shard_key: i64,
    pub connection_id: Uuid,
    #[sea_orm(column_type = "Char(Some(3))")]
    pub currency: String,
    #[sea_orm(column_type = "Decimal(Some((38, 0)))")]
    pub available_minor: Decimal,
    #[sea_orm(column_type = "Decimal(Some((38, 0)))")]
    pub pending_minor: Decimal,
    pub apy_bps: Option<i32>,
    pub snapshot_at: DateTimeWithTimeZone,
    #[sea_orm(column_type = "JsonBinary")]
    pub raw: Json,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
