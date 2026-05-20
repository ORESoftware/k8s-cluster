use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
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
    pool: PgPool,
}

impl TenantService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, input: CreateTenant) -> AppResult<Tenant> {
        // Validate region early so we fail fast on bad country/state codes.
        let _ = Region::from_codes(&input.country_code, input.us_state.as_deref())
            .map_err(|e| AppError::BadRequest(e.to_string()))?;

        let base_currency = input.base_currency.unwrap_or_else(|| "USD".into());
        let kms_key_id = input.kms_key_id.unwrap_or_else(|| "kms/local-dev".into());

        let row = sqlx::query(
            r#"
            INSERT INTO tenants
                (slug, display_name, country_code, us_state, base_currency, kms_key_id)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, slug::TEXT AS slug, display_name, country_code,
                      us_state, base_currency, kms_key_id, status, created_at
            "#,
        )
        .bind(&input.slug)
        .bind(&input.display_name)
        .bind(&input.country_code.to_uppercase())
        .bind(input.us_state.as_deref().map(|s| s.to_uppercase()))
        .bind(&base_currency)
        .bind(&kms_key_id)
        .fetch_one(&self.pool)
        .await?;

        row_to_tenant(&row)
    }

    pub async fn by_id(&self, id: Uuid) -> AppResult<Tenant> {
        let row = sqlx::query(
            r#"
            SELECT id, slug::TEXT AS slug, display_name, country_code,
                   us_state, base_currency, kms_key_id, status, created_at
            FROM tenants WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("tenant {id}")))?;

        row_to_tenant(&row)
    }

    pub async fn by_slug(&self, slug: &str) -> AppResult<Tenant> {
        let row = sqlx::query(
            r#"
            SELECT id, slug::TEXT AS slug, display_name, country_code,
                   us_state, base_currency, kms_key_id, status, created_at
            FROM tenants WHERE slug = $1::citext
            "#,
        )
        .bind(slug)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("tenant {slug}")))?;

        row_to_tenant(&row)
    }
}

fn row_to_tenant(row: &sqlx::postgres::PgRow) -> AppResult<Tenant> {
    Ok(Tenant {
        id: row.try_get("id")?,
        slug: row.try_get("slug")?,
        display_name: row.try_get("display_name")?,
        country_code: row.try_get("country_code")?,
        us_state: row.try_get("us_state")?,
        base_currency: row.try_get("base_currency")?,
        kms_key_id: row.try_get("kms_key_id")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
    })
}
