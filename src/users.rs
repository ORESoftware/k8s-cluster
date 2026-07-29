use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryResult,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::users;
use crate::error::{AppError, AppResult};
use crate::shard::{Region, ShardKey};

#[derive(Clone, Debug, Serialize)]
pub struct User {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub country_code: Option<String>,
    pub us_state: Option<String>,
    pub is_customer: bool,
    pub is_vendor: bool,
    pub external_refs: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateUser {
    pub email: String,
    pub display_name: Option<String>,
    pub country_code: Option<String>,
    pub us_state: Option<String>,
    #[serde(default)]
    pub is_customer: bool,
    #[serde(default)]
    pub is_vendor: bool,
    #[serde(default)]
    pub external_refs: serde_json::Value,
}

#[derive(Clone)]
pub struct UserService {
    pool: DatabaseConnection,
}

impl UserService {
    pub fn new(pool: DatabaseConnection) -> Self {
        Self { pool }
    }

    pub async fn upsert(
        &self,
        tenant_id: Uuid,
        tenant_region: Region,
        input: CreateUser,
    ) -> AppResult<User> {
        let shard = ShardKey::derive(tenant_id, tenant_region).0;
        let external_refs = if input.external_refs.is_null() {
            serde_json::json!({})
        } else {
            input.external_refs
        };

        // Raw SQL (SeaORM Statement): the DO UPDATE arm uses COALESCE over
        // EXCLUDED, boolean OR-merge, and the jsonb `||` merge operator —
        // none of which the entity OnConflict API can express without
        // hand-built expressions, so the proven statement is kept verbatim.
        let row = self
            .pool
            .query_one(crate::db::stmt(
                r#"
            INSERT INTO users
                (tenant_id, shard_key, email, display_name, country_code,
                 us_state, is_customer, is_vendor, external_refs)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (tenant_id, email) DO UPDATE
                SET display_name  = COALESCE(EXCLUDED.display_name, users.display_name),
                    country_code  = COALESCE(EXCLUDED.country_code, users.country_code),
                    us_state      = COALESCE(EXCLUDED.us_state,     users.us_state),
                    is_customer   = users.is_customer OR EXCLUDED.is_customer,
                    is_vendor     = users.is_vendor   OR EXCLUDED.is_vendor,
                    external_refs = users.external_refs || EXCLUDED.external_refs,
                    updated_at    = now()
            RETURNING id, tenant_id, email::TEXT AS email, display_name,
                      country_code, us_state, is_customer, is_vendor,
                      external_refs, created_at
            "#,
                [
                    tenant_id.into(),
                    shard.into(),
                    input.email.into(),
                    input.display_name.into(),
                    input.country_code.into(),
                    input.us_state.into(),
                    input.is_customer.into(),
                    input.is_vendor.into(),
                    external_refs.into(),
                ],
            ))
            .await?;
        let row = crate::db::require_row(row)?;

        row_to_user(&row)
    }

    pub async fn by_email(&self, tenant_id: Uuid, email: &str) -> AppResult<User> {
        // Raw SQL (SeaORM Statement): `$2::citext` keeps the lookup
        // case-insensitive; a bare text bind would degrade to the
        // case-sensitive text `=` operator.
        let row = self
            .pool
            .query_one(crate::db::stmt(
                r#"
            SELECT id, tenant_id, email::TEXT AS email, display_name,
                   country_code, us_state, is_customer, is_vendor,
                   external_refs, created_at
            FROM users
            WHERE tenant_id = $1 AND email = $2::citext
            "#,
                [tenant_id.into(), email.into()],
            ))
            .await?
            .ok_or_else(|| AppError::NotFound(format!("user {email}")))?;

        row_to_user(&row)
    }

    pub async fn by_id(&self, tenant_id: Uuid, id: Uuid) -> AppResult<User> {
        let model = users::Entity::find()
            .filter(users::Column::TenantId.eq(tenant_id))
            .filter(users::Column::Id.eq(id))
            .one(&self.pool)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("user {id}")))?;

        Ok(User {
            id: model.id,
            tenant_id: model.tenant_id,
            email: model.email,
            display_name: model.display_name,
            country_code: model.country_code,
            us_state: model.us_state,
            is_customer: model.is_customer,
            is_vendor: model.is_vendor,
            external_refs: model.external_refs,
            created_at: model.created_at.with_timezone(&Utc),
        })
    }
}

fn row_to_user(row: &QueryResult) -> AppResult<User> {
    Ok(User {
        id: row.try_get("", "id")?,
        tenant_id: row.try_get("", "tenant_id")?,
        email: row.try_get("", "email")?,
        display_name: row.try_get("", "display_name")?,
        country_code: row.try_get("", "country_code")?,
        us_state: row.try_get("", "us_state")?,
        is_customer: row.try_get("", "is_customer")?,
        is_vendor: row.try_get("", "is_vendor")?,
        external_refs: row.try_get("", "external_refs")?,
        created_at: row.try_get("", "created_at")?,
    })
}
