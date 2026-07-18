use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "web_session")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id_hash: String,
    pub user_id: Uuid,
    pub email: String,
    pub supabase_session_id: Option<Uuid>,
    pub encrypted_access_token: String,
    pub encrypted_refresh_token: String,
    pub access_expires_at: DateTimeUtc,
    pub refresh_lease_id: Option<Uuid>,
    pub refresh_lease_expires_at: Option<DateTimeUtc>,
    pub csrf_token: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
    pub expires_at: DateTimeUtc,
    pub revoked_at: Option<DateTimeUtc>,
    pub revocation_pending_at: Option<DateTimeUtc>,
    pub revocation_next_attempt_at: Option<DateTimeUtc>,
    pub revocation_attempts: i32,
    pub upstream_revoked_at: Option<DateTimeUtc>,
    pub revocation_abandoned_at: Option<DateTimeUtc>,
    pub revocation_failure_kind: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
