//! SeaORM entity for `tenant_locks` — tenant-scoped leases (the lock
//! primitive). Every acquire has a TTL so a crashed client cannot hold a
//! lock forever; renew/release require the opaque `lease_token`.
//! Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "tenant_locks")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub tenant_id: Uuid,
    pub shard_key: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub resource_key: String,
    pub lease_token: Uuid,
    pub holder: Option<String>,
    pub acquired_at: DateTimeWithTimeZone,
    pub expires_at: DateTimeWithTimeZone,
    #[sea_orm(column_type = "JsonBinary")]
    pub metadata: Json,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
