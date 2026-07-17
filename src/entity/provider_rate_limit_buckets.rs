//! SeaORM entity for `provider_rate_limit_buckets` — distributed provider
//! request budgets, keyed by `(tenant_id, provider, window_start,
//! window_seconds)`. Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "provider_rate_limit_buckets")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub tenant_id: Uuid,
    #[sea_orm(
        primary_key,
        auto_increment = false,
        column_type = r#"custom("provider_kind")"#,
        select_as = "text",
        save_as = "provider_kind"
    )]
    pub provider: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub window_start: DateTimeWithTimeZone,
    #[sea_orm(primary_key, auto_increment = false)]
    pub window_seconds: i32,
    pub request_limit: i32,
    pub requests_used: i32,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
