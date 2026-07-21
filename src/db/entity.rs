//! SeaORM entity for `shared_auth.users`. The schema is owned declaratively by
//! pg-defs (`db/schema.sql`); this mirrors it for reads/upserts only.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(schema_name = "shared_auth", table_name = "users")]
pub struct Model {
    /// Stable OreSoftware identity — the `sub` of tokens we mint. We generate it,
    /// so it is not auto-increment.
    #[sea_orm(primary_key, auto_increment = false)]
    pub shared_user_id: Uuid,
    pub supabase_project: String,
    pub supabase_user_id: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub phone: Option<String>,
    #[sea_orm(column_type = "Json")]
    pub user_metadata: Json,
    #[sea_orm(column_type = "Json")]
    pub app_metadata: Json,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    pub last_seen_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
