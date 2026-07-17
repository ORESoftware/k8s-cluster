//! SeaORM entity for `accounts` — the ledger primitive; multiple accounts
//! per user (ar/cus_x, ap/ven_y, clearing/stripe, cash/chase, ...).
//! Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "accounts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub shard_key: i64,
    pub user_id: Option<Uuid>,
    #[sea_orm(
        column_type = r#"custom("account_kind")"#,
        select_as = "text",
        save_as = "account_kind"
    )]
    pub kind: String,
    #[sea_orm(
        column_type = r#"custom("account_normal_side")"#,
        select_as = "text",
        save_as = "account_normal_side"
    )]
    pub normal_side: String,
    pub code: String,
    #[sea_orm(column_type = "Char(Some(3))")]
    pub currency: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub metadata: Json,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
