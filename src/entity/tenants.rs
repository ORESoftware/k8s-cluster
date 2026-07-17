//! SeaORM entity for `tenants` — the B2B customers of the billing server.
//! Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "tenants")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(
        column_type = r#"custom("citext")"#,
        select_as = "text",
        save_as = "citext"
    )]
    pub slug: String,
    pub display_name: String,
    #[sea_orm(column_type = "Char(Some(2))")]
    pub country_code: String,
    #[sea_orm(column_type = "Char(Some(2))")]
    pub us_state: Option<String>,
    #[sea_orm(column_type = "Char(Some(3))")]
    pub base_currency: String,
    pub kms_key_id: String,
    pub status: String,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
