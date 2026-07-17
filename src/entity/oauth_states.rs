//! SeaORM entity for `oauth_states` — the anti-CSRF nonce store for the
//! OAuth handshake; rows are single-use (delete-returning) and expire.
//! Schema: `schema/schema.sql`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "oauth_states")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub state: String,
    pub tenant_id: Uuid,
    #[sea_orm(
        column_type = r#"custom("provider_kind")"#,
        select_as = "text",
        save_as = "provider_kind"
    )]
    pub provider: String,
    pub return_to: Option<String>,
    pub pkce_verifier: Option<String>,
    pub expires_at: DateTimeWithTimeZone,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
