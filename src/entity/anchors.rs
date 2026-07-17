//! SeaORM entity for `anchors` — Merkle roots over posting ranges committed
//! to Solana so anyone can independently verify a posting existed at a known
//! point in time. Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "anchors")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub tenant_id: Uuid,
    pub shard_key: i64,
    pub from_posting_id: i64,
    pub to_posting_id: i64,
    pub posting_count: i64,
    pub merkle_root: Vec<u8>,
    pub chain: String,
    pub tx_signature: Option<String>,
    pub slot: Option<i64>,
    pub finalized_at: Option<DateTimeWithTimeZone>,
    pub submitted_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
