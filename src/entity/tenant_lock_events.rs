//! SeaORM entity for `tenant_lock_events` — append-only audit trail of every
//! lease acquire/renew/release/preempt/expire (the SOC 2 control surface for
//! the lock feature). Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "tenant_lock_events")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub tenant_id: Uuid,
    pub shard_key: i64,
    pub resource_key: String,
    pub lease_token: Option<Uuid>,
    #[sea_orm(
        column_type = r#"custom("lock_event_kind")"#,
        select_as = "text",
        save_as = "lock_event_kind"
    )]
    pub kind: String,
    pub holder: Option<String>,
    /// API caller (e.g. "tenant-api-key:tak_abc").
    pub actor: Option<String>,
    pub ttl_seconds: Option<i32>,
    pub occurred_at: DateTimeWithTimeZone,
    #[sea_orm(column_type = "JsonBinary")]
    pub metadata: Json,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
