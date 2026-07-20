//! `shared_auth.users` upserts and reads. Runtime queries only (no compile-time
//! DB, no macros) so the crate builds without a live database.

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::config::DbConfig;
use crate::error::AuthError;
use crate::supabase::VerifiedIdentity;

/// A row of `shared_auth.users` after an upsert. `shared_user_id` is the stable
/// OreSoftware identifier we mint tokens against — distinct from the per-project
/// Supabase `sub`.
#[derive(Clone, Debug)]
pub struct MirroredUser {
    pub shared_user_id: Uuid,
    pub project: String,
    pub supabase_user_id: String,
    pub email: Option<String>,
    pub email_verified: bool,
}

#[derive(Clone)]
pub struct UserStore {
    pool: PgPool,
}

impl UserStore {
    pub async fn connect(config: &DbConfig) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .connect(&config.url)
            .await?;
        Ok(Self { pool })
    }

    /// Insert or update the mirror row for a verified identity, returning the
    /// stable `shared_user_id`. Keyed on `(project, supabase_user_id)`.
    pub async fn upsert_identity(
        &self,
        identity: &VerifiedIdentity,
    ) -> Result<MirroredUser, AuthError> {
        let row = sqlx::query(
            r#"
            insert into shared_auth.users
                (supabase_project, supabase_user_id, email, email_verified,
                 phone, user_metadata, app_metadata, last_seen_at)
            values ($1, $2, $3, $4, $5, $6, $7, now())
            on conflict (supabase_project, supabase_user_id) do update set
                email          = excluded.email,
                email_verified = excluded.email_verified,
                phone          = excluded.phone,
                user_metadata  = excluded.user_metadata,
                app_metadata   = excluded.app_metadata,
                updated_at     = now(),
                last_seen_at   = now()
            returning shared_user_id, supabase_project, supabase_user_id, email, email_verified
            "#,
        )
        .bind(&identity.project)
        .bind(&identity.supabase_user_id)
        .bind(&identity.email)
        .bind(identity.email_verified)
        .bind(&identity.phone)
        .bind(&identity.user_metadata)
        .bind(&identity.app_metadata)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "identity upsert failed");
            AuthError::Upstream
        })?;

        Ok(MirroredUser {
            shared_user_id: row.get("shared_user_id"),
            project: row.get("supabase_project"),
            supabase_user_id: row.get("supabase_user_id"),
            email: row.get("email"),
            email_verified: row.get("email_verified"),
        })
    }

    /// Look up a mirror row by stable id (used by `/auth/introspect`).
    pub async fn find_by_shared_id(
        &self,
        shared_user_id: Uuid,
    ) -> Result<Option<MirroredUser>, AuthError> {
        let row = sqlx::query(
            r#"
            select shared_user_id, supabase_project, supabase_user_id, email, email_verified
            from shared_auth.users
            where shared_user_id = $1
            "#,
        )
        .bind(shared_user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "user lookup failed");
            AuthError::Upstream
        })?;

        Ok(row.map(|row| MirroredUser {
            shared_user_id: row.get("shared_user_id"),
            project: row.get("supabase_project"),
            supabase_user_id: row.get("supabase_user_id"),
            email: row.get("email"),
            email_verified: row.get("email_verified"),
        }))
    }

    /// Liveness check for `/readyz`.
    pub async fn ping(&self) -> Result<(), AuthError> {
        sqlx::query("select 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(|_| AuthError::Upstream)
    }
}
