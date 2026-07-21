//! `shared_auth.users` upserts and reads via SeaORM. No DDL — the schema is owned
//! by pg-defs (`db/schema.sql`); this only inserts/updates/reads.

use std::sync::Arc;

use sea_orm::sea_query::OnConflict;
use sea_orm::ActiveValue::Set;
use sea_orm::{ConnectOptions, Database, DatabaseConnection, EntityTrait};
use uuid::Uuid;

use crate::config::DbConfig;
use crate::error::AuthError;
use crate::supabase::VerifiedIdentity;

use super::entity::{self, ActiveModel, Column, Entity};

/// A row of `shared_auth.users` after an upsert. `shared_user_id` is the stable
/// OreSoftware identifier we mint tokens against.
#[derive(Clone, Debug)]
pub struct MirroredUser {
    pub shared_user_id: Uuid,
    pub project: String,
    pub supabase_user_id: String,
    pub email: Option<String>,
    pub email_verified: bool,
}

impl From<entity::Model> for MirroredUser {
    fn from(m: entity::Model) -> Self {
        Self {
            shared_user_id: m.shared_user_id,
            project: m.supabase_project,
            supabase_user_id: m.supabase_user_id,
            email: m.email,
            email_verified: m.email_verified,
        }
    }
}

#[derive(Clone)]
pub struct UserStore {
    db: Arc<DatabaseConnection>,
}

impl UserStore {
    pub async fn connect(config: &DbConfig) -> anyhow::Result<Self> {
        let mut opts = ConnectOptions::new(config.url.clone());
        opts.max_connections(config.max_connections)
            .sqlx_logging(false);
        let db = Database::connect(opts).await?;
        Ok(Self { db: Arc::new(db) })
    }

    /// For tests: wrap an already-built connection (e.g. a `MockDatabase`).
    #[cfg(test)]
    pub fn from_connection(db: DatabaseConnection) -> Self {
        Self { db: Arc::new(db) }
    }

    /// Insert or update the mirror row for a verified identity, returning the
    /// stable `shared_user_id`. Keyed on `(supabase_project, supabase_user_id)`;
    /// on conflict every mutable field is refreshed but `shared_user_id` is left
    /// untouched, so a returning caller always gets the same stable id.
    pub async fn upsert_identity(
        &self,
        identity: &VerifiedIdentity,
    ) -> Result<MirroredUser, AuthError> {
        let now = chrono::Utc::now().fixed_offset();
        let model = ActiveModel {
            shared_user_id: Set(Uuid::new_v4()),
            supabase_project: Set(identity.project.clone()),
            supabase_user_id: Set(identity.supabase_user_id.clone()),
            email: Set(identity.email.clone()),
            email_verified: Set(identity.email_verified),
            phone: Set(identity.phone.clone()),
            user_metadata: Set(identity.user_metadata.clone()),
            app_metadata: Set(identity.app_metadata.clone()),
            created_at: Set(now),
            updated_at: Set(now),
            last_seen_at: Set(now),
        };

        let on_conflict = OnConflict::columns([Column::SupabaseProject, Column::SupabaseUserId])
            .update_columns([
                Column::Email,
                Column::EmailVerified,
                Column::Phone,
                Column::UserMetadata,
                Column::AppMetadata,
                Column::UpdatedAt,
                Column::LastSeenAt,
            ])
            .to_owned();

        let row = Entity::insert(model)
            .on_conflict(on_conflict)
            .exec_with_returning(self.db.as_ref())
            .await
            .map_err(|err| {
                tracing::error!(error = %err, "identity upsert failed");
                AuthError::Upstream
            })?;

        Ok(row.into())
    }

    /// Look up a mirror row by stable id (used by `/auth/introspect`).
    pub async fn find_by_shared_id(
        &self,
        shared_user_id: Uuid,
    ) -> Result<Option<MirroredUser>, AuthError> {
        Entity::find_by_id(shared_user_id)
            .one(self.db.as_ref())
            .await
            .map(|opt| opt.map(MirroredUser::from))
            .map_err(|err| {
                tracing::error!(error = %err, "user lookup failed");
                AuthError::Upstream
            })
    }

    /// Liveness check for `/readyz`.
    pub async fn ping(&self) -> Result<(), AuthError> {
        self.db
            .as_ref()
            .ping()
            .await
            .map_err(|_| AuthError::Upstream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase};

    fn model(project: &str, sub: &str) -> entity::Model {
        entity::Model {
            shared_user_id: Uuid::from_u128(42),
            supabase_project: project.into(),
            supabase_user_id: sub.into(),
            email: Some("a@b.co".into()),
            email_verified: true,
            phone: None,
            user_metadata: serde_json::json!({}),
            app_metadata: serde_json::json!({}),
            created_at: chrono::Utc::now().fixed_offset(),
            updated_at: chrono::Utc::now().fixed_offset(),
            last_seen_at: chrono::Utc::now().fixed_offset(),
        }
    }

    fn identity(project: &str, sub: &str) -> VerifiedIdentity {
        VerifiedIdentity {
            project: project.into(),
            supabase_user_id: sub.into(),
            email: Some("a@b.co".into()),
            email_verified: true,
            phone: None,
            role: None,
            user_metadata: serde_json::Value::Null,
            app_metadata: serde_json::Value::Null,
        }
    }

    #[tokio::test]
    async fn upsert_returns_stable_shared_id() {
        let row = model("fiducia-cloud", "sub-1");
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([vec![row.clone()]])
            .into_connection();
        let store = UserStore::from_connection(db);

        let mirrored = store
            .upsert_identity(&identity("fiducia-cloud", "sub-1"))
            .await
            .unwrap();
        assert_eq!(mirrored.shared_user_id, row.shared_user_id);
        assert_eq!(mirrored.project, "fiducia-cloud");
        assert_eq!(mirrored.supabase_user_id, "sub-1");
        assert!(mirrored.email_verified);
    }

    #[tokio::test]
    async fn find_by_shared_id_maps_row() {
        let row = model("3fa-app", "sub-9");
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([vec![row.clone()]])
            .into_connection();
        let store = UserStore::from_connection(db);

        let found = store.find_by_shared_id(row.shared_user_id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().project, "3fa-app");
    }

    #[tokio::test]
    async fn find_missing_returns_none() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([Vec::<entity::Model>::new()])
            .into_connection();
        let store = UserStore::from_connection(db);
        assert!(store
            .find_by_shared_id(Uuid::from_u128(7))
            .await
            .unwrap()
            .is_none());
    }
}
