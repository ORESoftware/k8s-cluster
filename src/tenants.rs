use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryOrder, QuerySelect, Set,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::tenants;
use crate::error::{AppError, AppResult};
use crate::memberships::MembershipService;
use crate::shard::Region;

#[derive(Clone, Debug, Serialize)]
pub struct Tenant {
    pub id: Uuid,
    pub slug: String,
    pub display_name: String,
    pub country_code: String,
    pub us_state: Option<String>,
    pub base_currency: String,
    pub kms_key_id: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

impl Tenant {
    pub fn region(&self) -> AppResult<Region> {
        Region::from_codes(&self.country_code, self.us_state.as_deref())
            .map_err(|e| AppError::BadRequest(e.to_string()))
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateTenant {
    pub slug: String,
    pub display_name: String,
    pub country_code: String,
    pub us_state: Option<String>,
    pub base_currency: Option<String>,
    pub kms_key_id: Option<String>,
}

#[derive(Clone)]
pub struct TenantService {
    pool: DatabaseConnection,
}

impl TenantService {
    pub fn new(pool: DatabaseConnection) -> Self {
        Self { pool }
    }

    /// Legacy/admin provisioning without an end-user owner. Network API tenant
    /// creation uses [`Self::create_owned`] instead.
    pub async fn create(&self, input: CreateTenant) -> AppResult<Tenant> {
        self.insert_on(&self.pool, input).await
    }

    /// Create a tenant and its first Shared Auth owner in one transaction. A
    /// failure in either half rolls the whole operation back, so no network API
    /// call can produce an ownerless billing tenant.
    pub async fn create_owned(
        &self,
        input: CreateTenant,
        owner_shared_user_id: &str,
    ) -> AppResult<Tenant> {
        let transaction = self.pool.begin().await?;
        let tenant = self.insert_on(&transaction, input).await?;
        MembershipService::create_owner_on(&transaction, tenant.id, owner_shared_user_id).await?;
        transaction.commit().await?;
        Ok(tenant)
    }

    async fn insert_on<C>(&self, connection: &C, input: CreateTenant) -> AppResult<Tenant>
    where
        C: ConnectionTrait,
    {
        // Validate region early so we fail fast on bad country/state codes.
        let _ = Region::from_codes(&input.country_code, input.us_state.as_deref())
            .map_err(|e| AppError::BadRequest(e.to_string()))?;

        let base_currency = input.base_currency.unwrap_or_else(|| "USD".into());
        let kms_key_id = input.kms_key_id.unwrap_or_else(|| "kms/local-dev".into());

        let model = tenants::Entity::insert(tenants::ActiveModel {
            slug: Set(input.slug),
            display_name: Set(input.display_name),
            country_code: Set(input.country_code.to_uppercase()),
            us_state: Set(input.us_state.as_deref().map(|s| s.to_uppercase())),
            base_currency: Set(base_currency),
            kms_key_id: Set(kms_key_id),
            ..Default::default()
        })
        .exec_with_returning(connection)
        .await?;

        Ok(model_to_tenant(model))
    }

    pub async fn by_id(&self, id: Uuid) -> AppResult<Tenant> {
        let model = tenants::Entity::find_by_id(id)
            .one(&self.pool)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("tenant {id}")))?;
        Ok(model_to_tenant(model))
    }

    /// Lightweight pagination for the admin UI (most recent first).
    pub async fn list(&self, limit: i64) -> AppResult<Vec<Tenant>> {
        let limit = limit.clamp(1, 500);
        let models = tenants::Entity::find()
            .order_by_desc(tenants::Column::CreatedAt)
            .limit(limit as u64)
            .all(&self.pool)
            .await?;
        Ok(models.into_iter().map(model_to_tenant).collect())
    }

    pub async fn count(&self) -> AppResult<i64> {
        let count = tenants::Entity::find().count(&self.pool).await?;
        Ok(count as i64)
    }

    pub async fn by_slug(&self, slug: &str) -> AppResult<Tenant> {
        let row = self
            .pool
            .query_one(crate::db::stmt(
                r#"
            SELECT id, slug::TEXT AS slug, display_name, country_code,
                   us_state, base_currency, kms_key_id, status, created_at
            FROM tenants WHERE slug = $1::citext
            "#,
                [slug.into()],
            ))
            .await?
            .ok_or_else(|| AppError::NotFound(format!("tenant {slug}")))?;

        Ok(Tenant {
            id: row.try_get("", "id")?,
            slug: row.try_get("", "slug")?,
            display_name: row.try_get("", "display_name")?,
            country_code: row.try_get("", "country_code")?,
            us_state: row.try_get("", "us_state")?,
            base_currency: row.try_get("", "base_currency")?,
            kms_key_id: row.try_get("", "kms_key_id")?,
            status: row.try_get("", "status")?,
            created_at: row.try_get("", "created_at")?,
        })
    }
}

fn model_to_tenant(model: tenants::Model) -> Tenant {
    Tenant {
        id: model.id,
        slug: model.slug,
        display_name: model.display_name,
        country_code: model.country_code,
        us_state: model.us_state,
        base_currency: model.base_currency,
        kms_key_id: model.kms_key_id,
        status: model.status,
        created_at: model.created_at.with_timezone(&Utc),
    }
}
