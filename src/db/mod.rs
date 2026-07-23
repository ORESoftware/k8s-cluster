//! Postgres-backed identity, credential, role, and session store.
//!
//! `db/schema.sql` is the declarative schema and this module executes DML only.
//! Postgres is authoritative. Supabase and future providers are represented by
//! rows in `provider_identities`, so adding an adapter does not change the core
//! user/session model.

use std::sync::Arc;

use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement,
    TransactionTrait,
};
use uuid::Uuid;

use crate::config::DbConfig;
use crate::error::AuthError;
use crate::supabase::VerifiedIdentity;

const IDENTITY_NAMESPACE: Uuid = Uuid::from_bytes([
    0x6e, 0x7f, 0x92, 0x29, 0x8a, 0xe2, 0x4b, 0x3f, 0x9a, 0xd7, 0x38, 0x0a, 0xb8, 0xdd, 0x73, 0x2b,
]);

#[derive(Clone, Debug)]
pub struct AuthenticatedIdentity {
    pub shared_user_id: Uuid,
    pub provider: String,
    pub provider_tenant: String,
    pub provider_subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub roles: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LocalCredential {
    pub identity: AuthenticatedIdentity,
    pub password_hash: String,
    pub locked: bool,
}

#[derive(Clone, Debug)]
pub struct SessionRecord {
    pub session_id: Uuid,
    pub identity: AuthenticatedIdentity,
    pub expires_at: chrono::DateTime<chrono::FixedOffset>,
}

#[derive(Clone)]
pub struct DbStore {
    db: Arc<DatabaseConnection>,
}

impl DbStore {
    pub async fn connect(config: &DbConfig) -> anyhow::Result<Self> {
        let mut options = ConnectOptions::new(config.url.clone());
        options
            .max_connections(config.max_connections)
            .min_connections(1)
            .connect_timeout(std::time::Duration::from_secs(5))
            .acquire_timeout(std::time::Duration::from_secs(5))
            .idle_timeout(std::time::Duration::from_secs(300))
            .sqlx_logging(false);
        let db = Database::connect(options).await?;
        Ok(Self { db: Arc::new(db) })
    }

    pub async fn ping(&self) -> Result<(), AuthError> {
        self.db.ping().await.map_err(|error| {
            tracing::warn!(%error, "Postgres readiness check failed");
            AuthError::Upstream
        })
    }

    /// Resolve a verified Supabase identity into the provider-neutral user
    /// namespace. We never auto-link accounts merely because emails match.
    pub async fn upsert_supabase_identity(
        &self,
        identity: &VerifiedIdentity,
    ) -> Result<AuthenticatedIdentity, AuthError> {
        self.upsert_external_identity(
            "supabase",
            &identity.project,
            &identity.supabase_user_id,
            identity.email.clone(),
            identity.email_verified,
            serde_json::json!({
                "phone": identity.phone,
                "role": identity.role,
                "user_metadata": identity.user_metadata,
                "app_metadata": identity.app_metadata,
            }),
        )
        .await
    }

    pub async fn upsert_external_identity(
        &self,
        provider: &str,
        provider_tenant: &str,
        provider_subject: &str,
        email: Option<String>,
        email_verified: bool,
        metadata: serde_json::Value,
    ) -> Result<AuthenticatedIdentity, AuthError> {
        let transaction = self.db.begin().await.map_err(db_error)?;

        let existing = transaction
            .query_one_raw(statement(
                "SELECT shared_user_id FROM shared_auth.provider_identities \
                 WHERE provider = $1 AND provider_tenant = $2 AND provider_subject = $3",
                vec![
                    provider.to_owned().into(),
                    provider_tenant.to_owned().into(),
                    provider_subject.to_owned().into(),
                ],
            ))
            .await
            .map_err(db_error)?;

        let stable_name = format!("{provider}\0{provider_tenant}\0{provider_subject}");
        let shared_user_id = existing
            .as_ref()
            .and_then(|row| row.try_get("", "shared_user_id").ok())
            .unwrap_or_else(|| Uuid::new_v5(&IDENTITY_NAMESPACE, stable_name.as_bytes()));

        transaction
            .execute_raw(statement(
                "INSERT INTO shared_auth.principals (shared_user_id, last_seen_at) \
                 VALUES ($1, now()) \
                 ON CONFLICT (shared_user_id) DO UPDATE \
                 SET last_seen_at = now(), updated_at = now()",
                vec![shared_user_id.into()],
            ))
            .await
            .map_err(db_error)?;

        let row = transaction
            .query_one_raw(statement(
                "INSERT INTO shared_auth.provider_identities \
                    (shared_user_id, provider, provider_tenant, provider_subject, email, \
                     email_verified, metadata, last_seen_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, now()) \
                 ON CONFLICT (provider, provider_tenant, provider_subject) DO UPDATE SET \
                    email = EXCLUDED.email, \
                    email_verified = EXCLUDED.email_verified, \
                    metadata = EXCLUDED.metadata, \
                    updated_at = now(), last_seen_at = now() \
                 RETURNING shared_user_id, provider, provider_tenant, provider_subject, \
                           email, email_verified",
                vec![
                    shared_user_id.into(),
                    provider.to_owned().into(),
                    provider_tenant.to_owned().into(),
                    provider_subject.to_owned().into(),
                    email.into(),
                    email_verified.into(),
                    metadata.into(),
                ],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Internal)?;

        transaction.commit().await.map_err(db_error)?;
        let mut resolved = identity_from_row(&row)?;
        resolved.roles = self.roles_for(shared_user_id).await?;
        Ok(resolved)
    }

    pub async fn create_local_user(
        &self,
        email: &str,
        display_name: Option<&str>,
        password_hash: &str,
    ) -> Result<AuthenticatedIdentity, AuthError> {
        let transaction = self.db.begin().await.map_err(db_error)?;
        let shared_user_id = Uuid::new_v4();
        let subject = shared_user_id.to_string();

        let inserted = transaction
            .execute_raw(statement(
                "INSERT INTO shared_auth.principals \
                    (shared_user_id, email, email_verified, display_name) \
                 VALUES ($1, $2, false, $3)",
                vec![
                    shared_user_id.into(),
                    email.to_owned().into(),
                    display_name.map(str::to_owned).into(),
                ],
            ))
            .await;
        if let Err(error) = inserted {
            tracing::info!(%error, "local registration conflicted");
            return Err(AuthError::Conflict);
        }

        transaction
            .execute_raw(statement(
                "INSERT INTO shared_auth.provider_identities \
                    (shared_user_id, provider, provider_tenant, provider_subject, email) \
                 VALUES ($1, 'local', 'default', $2, $3)",
                vec![
                    shared_user_id.into(),
                    subject.clone().into(),
                    email.to_owned().into(),
                ],
            ))
            .await
            .map_err(db_error)?;
        transaction
            .execute_raw(statement(
                "INSERT INTO shared_auth.local_credentials (shared_user_id, password_hash) \
                 VALUES ($1, $2)",
                vec![shared_user_id.into(), password_hash.to_owned().into()],
            ))
            .await
            .map_err(db_error)?;
        transaction
            .execute_raw(statement(
                "INSERT INTO shared_auth.roles (shared_user_id, role_name) VALUES ($1, 'user')",
                vec![shared_user_id.into()],
            ))
            .await
            .map_err(db_error)?;
        transaction.commit().await.map_err(db_error)?;

        Ok(AuthenticatedIdentity {
            shared_user_id,
            provider: "local".into(),
            provider_tenant: "default".into(),
            provider_subject: subject,
            email: Some(email.to_owned()),
            email_verified: false,
            roles: vec!["user".into()],
        })
    }

    pub async fn local_credential(
        &self,
        normalized_email: &str,
    ) -> Result<Option<LocalCredential>, AuthError> {
        let row = self
            .db
            .query_one_raw(statement(
                "SELECT u.shared_user_id, 'local' AS provider, 'default' AS provider_tenant, \
                        u.shared_user_id::text AS provider_subject, u.email, u.email_verified, \
                        c.password_hash, \
                        (c.locked_until IS NOT NULL AND c.locked_until > now()) AS locked \
                 FROM shared_auth.principals u \
                 JOIN shared_auth.local_credentials c USING (shared_user_id) \
                 WHERE lower(u.email) = $1 AND u.status = 'active'",
                vec![normalized_email.to_owned().into()],
            ))
            .await
            .map_err(db_error)?;
        let Some(row) = row else { return Ok(None) };
        let mut identity = identity_from_row(&row)?;
        identity.roles = self.roles_for(identity.shared_user_id).await?;
        Ok(Some(LocalCredential {
            identity,
            password_hash: row.try_get("", "password_hash").map_err(db_error)?,
            locked: row.try_get("", "locked").map_err(db_error)?,
        }))
    }

    pub async fn record_login_result(
        &self,
        shared_user_id: Uuid,
        success: bool,
    ) -> Result<(), AuthError> {
        let sql = if success {
            "UPDATE shared_auth.local_credentials SET failed_attempts = 0, locked_until = NULL, \
             updated_at = now() WHERE shared_user_id = $1"
        } else {
            "UPDATE shared_auth.local_credentials SET \
             failed_attempts = failed_attempts + 1, \
             locked_until = CASE WHEN failed_attempts + 1 >= 5 \
                                 THEN now() + interval '15 minutes' ELSE locked_until END, \
             updated_at = now() WHERE shared_user_id = $1"
        };
        self.db
            .execute_raw(statement(sql, vec![shared_user_id.into()]))
            .await
            .map_err(db_error)?;
        Ok(())
    }

    pub async fn create_session(
        &self,
        identity: AuthenticatedIdentity,
        refresh_token_hash: &str,
        expires_at: chrono::DateTime<chrono::FixedOffset>,
        rotated_from: Option<Uuid>,
    ) -> Result<SessionRecord, AuthError> {
        let session_id = Uuid::new_v4();
        self.db
            .execute_raw(statement(
                "INSERT INTO shared_auth.sessions \
                    (session_id, shared_user_id, refresh_token_hash, provider, provider_tenant, \
                     provider_subject, expires_at, rotated_from) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                vec![
                    session_id.into(),
                    identity.shared_user_id.into(),
                    refresh_token_hash.to_owned().into(),
                    identity.provider.clone().into(),
                    identity.provider_tenant.clone().into(),
                    identity.provider_subject.clone().into(),
                    expires_at.into(),
                    rotated_from.into(),
                ],
            ))
            .await
            .map_err(db_error)?;
        Ok(SessionRecord {
            session_id,
            identity,
            expires_at,
        })
    }

    /// Atomically consume a refresh token and replace it. A replay sees the old
    /// row as revoked and is rejected, preventing parallel refresh reuse.
    pub async fn rotate_session(
        &self,
        old_hash: &str,
        new_hash: &str,
        new_expires_at: chrono::DateTime<chrono::FixedOffset>,
    ) -> Result<SessionRecord, AuthError> {
        let transaction = self.db.begin().await.map_err(db_error)?;
        let row = transaction
            .query_one_raw(statement(
                "SELECT s.session_id, s.shared_user_id, s.provider, s.provider_tenant, \
                        s.provider_subject, COALESCE(pi.email, u.email) AS email, \
                        COALESCE(pi.email_verified, u.email_verified) AS email_verified \
                 FROM shared_auth.sessions s \
                 JOIN shared_auth.principals u USING (shared_user_id) \
                 LEFT JOIN shared_auth.provider_identities pi \
                   ON pi.shared_user_id = s.shared_user_id AND pi.provider = s.provider \
                  AND pi.provider_tenant = s.provider_tenant \
                  AND pi.provider_subject = s.provider_subject \
                 WHERE s.refresh_token_hash = $1 AND s.revoked_at IS NULL \
                   AND s.expires_at > now() AND u.status = 'active' \
                 FOR UPDATE OF s",
                vec![old_hash.to_owned().into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Unauthorized)?;
        let old_session_id: Uuid = row.try_get("", "session_id").map_err(db_error)?;
        let mut identity = identity_from_row(&row)?;

        let revoked = transaction
            .execute_raw(statement(
                "UPDATE shared_auth.sessions SET revoked_at = now(), updated_at = now() \
                 WHERE session_id = $1 AND revoked_at IS NULL",
                vec![old_session_id.into()],
            ))
            .await
            .map_err(db_error)?;
        if revoked.rows_affected() != 1 {
            return Err(AuthError::Unauthorized);
        }

        let new_session_id = Uuid::new_v4();
        transaction
            .execute_raw(statement(
                "INSERT INTO shared_auth.sessions \
                    (session_id, shared_user_id, refresh_token_hash, provider, provider_tenant, \
                     provider_subject, expires_at, rotated_from) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                vec![
                    new_session_id.into(),
                    identity.shared_user_id.into(),
                    new_hash.to_owned().into(),
                    identity.provider.clone().into(),
                    identity.provider_tenant.clone().into(),
                    identity.provider_subject.clone().into(),
                    new_expires_at.into(),
                    old_session_id.into(),
                ],
            ))
            .await
            .map_err(db_error)?;
        transaction.commit().await.map_err(db_error)?;
        identity.roles = self.roles_for(identity.shared_user_id).await?;
        Ok(SessionRecord {
            session_id: new_session_id,
            identity,
            expires_at: new_expires_at,
        })
    }

    pub async fn revoke_by_refresh_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<Uuid>, AuthError> {
        let row = self
            .db
            .query_one_raw(statement(
                "UPDATE shared_auth.sessions SET revoked_at = now(), updated_at = now() \
                 WHERE refresh_token_hash = $1 AND revoked_at IS NULL RETURNING session_id",
                vec![token_hash.to_owned().into()],
            ))
            .await
            .map_err(db_error)?;
        Ok(row.and_then(|row| row.try_get("", "session_id").ok()))
    }

    pub async fn revoke_by_session_id(&self, session_id: Uuid) -> Result<(), AuthError> {
        self.db
            .execute_raw(statement(
                "UPDATE shared_auth.sessions SET revoked_at = COALESCE(revoked_at, now()), \
                 updated_at = now() WHERE session_id = $1",
                vec![session_id.into()],
            ))
            .await
            .map_err(db_error)?;
        Ok(())
    }

    pub async fn session_is_active(&self, session_id: Uuid) -> Result<bool, AuthError> {
        let row = self
            .db
            .query_one_raw(statement(
                "SELECT EXISTS (SELECT 1 FROM shared_auth.sessions s \
                 JOIN shared_auth.principals u USING (shared_user_id) \
                 WHERE s.session_id = $1 AND s.revoked_at IS NULL \
                   AND s.expires_at > now() AND u.status = 'active') AS active",
                vec![session_id.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::Internal)?;
        row.try_get("", "active").map_err(db_error)
    }

    pub async fn replace_roles(
        &self,
        shared_user_id: Uuid,
        roles: &[String],
    ) -> Result<(), AuthError> {
        let transaction = self.db.begin().await.map_err(db_error)?;
        transaction
            .execute_raw(statement(
                "DELETE FROM shared_auth.roles WHERE shared_user_id = $1",
                vec![shared_user_id.into()],
            ))
            .await
            .map_err(db_error)?;
        for role in roles {
            transaction
                .execute_raw(statement(
                    "INSERT INTO shared_auth.roles (shared_user_id, role_name) VALUES ($1, $2)",
                    vec![shared_user_id.into(), role.clone().into()],
                ))
                .await
                .map_err(db_error)?;
        }
        transaction.commit().await.map_err(db_error)?;
        Ok(())
    }

    pub async fn record_webhook_event(
        &self,
        event_id: Uuid,
        provider: &str,
        event_type: &str,
        payload_sha256: &str,
    ) -> Result<bool, AuthError> {
        let result = self
            .db
            .execute_raw(statement(
                "INSERT INTO shared_auth.webhook_events \
                    (event_id, provider, event_type, payload_sha256) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT (event_id) DO NOTHING",
                vec![
                    event_id.into(),
                    provider.to_owned().into(),
                    event_type.to_owned().into(),
                    payload_sha256.to_owned().into(),
                ],
            ))
            .await
            .map_err(db_error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn roles_for(&self, shared_user_id: Uuid) -> Result<Vec<String>, AuthError> {
        let rows = self
            .db
            .query_all_raw(statement(
                "SELECT role_name FROM shared_auth.roles WHERE shared_user_id = $1 \
                 ORDER BY role_name",
                vec![shared_user_id.into()],
            ))
            .await
            .map_err(db_error)?;
        rows.into_iter()
            .map(|row| row.try_get("", "role_name").map_err(db_error))
            .collect()
    }
}

fn identity_from_row(row: &sea_orm::QueryResult) -> Result<AuthenticatedIdentity, AuthError> {
    Ok(AuthenticatedIdentity {
        shared_user_id: row.try_get("", "shared_user_id").map_err(db_error)?,
        provider: row.try_get("", "provider").map_err(db_error)?,
        provider_tenant: row.try_get("", "provider_tenant").map_err(db_error)?,
        provider_subject: row.try_get("", "provider_subject").map_err(db_error)?,
        email: row.try_get("", "email").map_err(db_error)?,
        email_verified: row.try_get("", "email_verified").map_err(db_error)?,
        roles: Vec::new(),
    })
}

fn statement(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

fn db_error<E: std::fmt::Display>(error: E) -> AuthError {
    tracing::error!(%error, "shared-auth database operation failed");
    AuthError::Upstream
}
