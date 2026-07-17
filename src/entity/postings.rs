//! SeaORM entity for `postings` — the append-only double-entry rows
//! (UPDATE/DELETE forbidden by trigger). `amount_minor` is `numeric(38, 0)`;
//! the ledger runtime reads/writes it as text-cast i128 (see
//! `src/ledger/service.rs`) because rust_decimal cannot represent the full
//! 38-digit domain — the `Decimal` type here mirrors the column for schema
//! accuracy. Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "postings")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub transaction_id: Uuid,
    pub tenant_id: Uuid,
    pub shard_key: i64,
    pub account_id: Uuid,
    #[sea_orm(
        column_type = r#"custom("posting_direction")"#,
        select_as = "text",
        save_as = "posting_direction"
    )]
    pub direction: String,
    #[sea_orm(column_type = "Decimal(Some((38, 0)))")]
    pub amount_minor: Decimal,
    #[sea_orm(column_type = "Char(Some(3))")]
    pub currency: String,
    pub source: String,
    pub source_event_id: String,
    pub posted_at: DateTimeWithTimeZone,
    #[sea_orm(column_type = "JsonBinary")]
    pub metadata: Json,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
