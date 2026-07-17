//! SeaORM entity for `users` — end-users / counterparties of a tenant,
//! uniquely identified by `(tenant_id, email)`. A user can be a customer (we
//! bill them), a vendor (we pay them), or both. Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub shard_key: i64,
    #[sea_orm(
        column_type = r#"custom("citext")"#,
        select_as = "text",
        save_as = "citext"
    )]
    pub email: String,
    pub display_name: Option<String>,
    #[sea_orm(column_type = "Char(Some(2))")]
    pub country_code: Option<String>,
    #[sea_orm(column_type = "Char(Some(2))")]
    pub us_state: Option<String>,
    pub is_customer: bool,
    pub is_vendor: bool,
    #[sea_orm(column_type = "JsonBinary")]
    pub external_refs: Json,
    #[sea_orm(column_type = "JsonBinary")]
    pub metadata: Json,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
