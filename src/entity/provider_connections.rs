//! SeaORM entity for `provider_connections` — per-tenant provider auth. All
//! secret credential material is sealed (AES-256-GCM envelope in
//! `sealed_credential`) before it touches the database.
//! Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "provider_connections")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub shard_key: i64,
    #[sea_orm(
        column_type = r#"custom("provider_kind")"#,
        select_as = "text",
        save_as = "provider_kind"
    )]
    pub provider: String,
    #[sea_orm(
        column_type = r#"custom("provider_auth_kind")"#,
        select_as = "text",
        save_as = "provider_auth_kind"
    )]
    pub auth_kind: String,
    pub external_account_id: Option<String>,
    pub display_label: String,
    #[sea_orm(
        column_type = r#"custom("connection_status")"#,
        select_as = "text",
        save_as = "connection_status"
    )]
    pub status: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub sealed_credential: Option<Json>,
    pub kms_key_version: i32,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTimeWithTimeZone>,
    pub refreshed_at: Option<DateTimeWithTimeZone>,
    pub last_sync_at: Option<DateTimeWithTimeZone>,
    pub last_sync_cursor: Option<String>,
    pub last_error: Option<String>,
    #[sea_orm(column_type = "JsonBinary")]
    pub metadata: Json,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
