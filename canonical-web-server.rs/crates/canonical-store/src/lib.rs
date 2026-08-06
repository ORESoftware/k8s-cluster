pub mod entity;
pub mod migration;

use sea_orm::{
    prelude::DateTimeUtc, ConnectOptions, ConnectionTrait, Database, DatabaseBackend,
    DatabaseConnection, DatabaseTransaction, DbErr, Statement, TransactionTrait,
};
use std::time::Duration;
use uuid::Uuid;

/// Opens a bounded application pool without enabling SQL-value logging.
/// Process-specific configuration remains in `canonical-config`; all backend
/// services share these conservative connection timeouts.
pub async fn connect_database(
    database_url: &str,
    database_max_connections: u32,
) -> Result<DatabaseConnection, DbErr> {
    let mut options = ConnectOptions::new(database_url);
    options
        .max_connections(database_max_connections)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(10))
        .acquire_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(300))
        .sqlx_logging(false);

    Database::connect(options).await
}

/// Returns the database's coordination clock for PostgreSQL transactions.
///
/// Lease holders must compare and extend durable timestamps against one shared
/// clock; otherwise replica clock skew can make two processes believe they own
/// the same lease. SQLite is used only for local development and isolated tests,
/// so it intentionally keeps the process clock and avoids backend-specific SQL.
pub async fn database_now<C>(connection: &C) -> Result<DateTimeUtc, DbErr>
where
    C: ConnectionTrait,
{
    if connection.get_database_backend() != DatabaseBackend::Postgres {
        return Ok(DateTimeUtc::from(std::time::SystemTime::now()));
    }

    connection
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT CURRENT_TIMESTAMP AS database_now".to_owned(),
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("database clock query returned no row".into()))?
        .try_get("", "database_now")
}

/// The only database login accepted by the Supabase session revoker.
///
/// This role is bootstrapped with access to `web_session` only and must remain
/// a non-owner, non-`BYPASSRLS` login. Keeping the expected role in code makes
/// an accidentally privileged revoker URL fail closed on first use.
pub const SESSION_REVOKER_ROLE: &str = "canonical_session_revoker";

/// The only PostgreSQL login accepted by the customer-facing web process.
pub const RUNTIME_SERVER_ROLE: &str = "canonical_web_server";

/// The database login reserved for a separately deployed admin service.
pub const ADMIN_SERVER_ROLE: &str = "canonical_admin_server";

/// Authentication assurance carried by a Supabase access token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssuranceLevel {
    Aal1,
    Aal2,
}

/// Fails before migration or HTTP startup when PostgreSQL is connected through
/// an owner, superuser, admin, revoker, delegated role, or otherwise unexpected
/// credential. The role name alone is insufficient: PostgreSQL role attributes,
/// memberships, and ownership can drift after bootstrap, so startup rechecks the
/// complete runtime boundary against the live catalogs.
/// SQLite remains available for isolated tests and local development.
pub async fn verify_runtime_database_role(db: &DatabaseConnection) -> Result<(), DbErr> {
    verify_isolated_database_role(db, RUNTIME_SERVER_ROLE, "web runtime").await
}

/// Verifies the dedicated no-ingress session revoker login before work starts.
pub async fn verify_session_revoker_database_role(db: &DatabaseConnection) -> Result<(), DbErr> {
    verify_isolated_database_role(db, SESSION_REVOKER_ROLE, "session revoker").await
}

/// Verifies the separately deployed administrative service login.
pub async fn verify_admin_database_role(db: &DatabaseConnection) -> Result<(), DbErr> {
    verify_isolated_database_role(db, ADMIN_SERVER_ROLE, "admin runtime").await
}

async fn verify_isolated_database_role(
    db: &DatabaseConnection,
    expected_role: &str,
    process_name: &str,
) -> Result<(), DbErr> {
    if db.get_database_backend() != DatabaseBackend::Postgres {
        return Ok(());
    }
    let row = db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            r#"
            SELECT
              r.rolname::text AS database_role,
              session_user::text AS login_role,
              r.rolcanlogin AS can_login,
              r.rolsuper AS is_superuser,
              r.rolcreatedb AS can_create_database,
              r.rolcreaterole AS can_create_role,
              r.rolinherit AS inherits_roles,
              r.rolreplication AS can_replicate,
              r.rolbypassrls AS bypasses_rls,
              EXISTS (
                SELECT 1
                FROM pg_auth_members memberships
                WHERE memberships.member = r.oid
                   OR memberships.roleid = r.oid
              ) AS has_memberships_or_members,
              EXISTS (
                SELECT 1
                FROM pg_database database_object
                WHERE database_object.datname = current_database()
                  AND database_object.datdba = r.oid
              ) AS owns_current_database,
              EXISTS (
                SELECT 1
                FROM pg_namespace schema_object
                WHERE schema_object.nspname IN ('public', 'auth')
                  AND schema_object.nspowner = r.oid
              ) AS owns_application_schema,
              EXISTS (
                SELECT 1
                FROM pg_class relation_object
                JOIN pg_namespace relation_schema
                  ON relation_schema.oid = relation_object.relnamespace
                WHERE relation_schema.nspname IN ('public', 'auth')
                  AND relation_object.relowner = r.oid
              ) AS owns_application_relation,
              EXISTS (
                SELECT 1
                FROM pg_proc function_object
                JOIN pg_namespace function_schema
                  ON function_schema.oid = function_object.pronamespace
                WHERE function_schema.nspname IN ('public', 'auth')
                  AND function_object.proowner = r.oid
              ) AS owns_application_function
            FROM pg_roles r
            WHERE r.rolname = current_user
            "#
            .to_owned(),
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("database role query returned no row".into()))?;
    let role = row.try_get::<String>("", "database_role")?;
    let login_role = row.try_get::<String>("", "login_role")?;
    if role != expected_role || login_role != expected_role {
        return Err(DbErr::Custom(format!(
            "{process_name} requires database login and current role {expected_role}; connected as login {login_role} with current role {role}"
        )));
    }

    let mut unsafe_properties = Vec::new();
    if !row.try_get::<bool>("", "can_login")? {
        unsafe_properties.push("NOLOGIN");
    }
    for (column, description) in [
        ("is_superuser", "SUPERUSER"),
        ("can_create_database", "CREATEDB"),
        ("can_create_role", "CREATEROLE"),
        ("inherits_roles", "INHERIT"),
        ("can_replicate", "REPLICATION"),
        ("bypasses_rls", "BYPASSRLS"),
        ("has_memberships_or_members", "role membership/member"),
        ("owns_current_database", "current database ownership"),
        ("owns_application_schema", "public/auth schema ownership"),
        (
            "owns_application_relation",
            "public/auth relation ownership",
        ),
        (
            "owns_application_function",
            "public/auth function ownership",
        ),
    ] {
        if row.try_get::<bool>("", column)? {
            unsafe_properties.push(description);
        }
    }
    if !unsafe_properties.is_empty() {
        return Err(DbErr::Custom(format!(
            "{process_name} database role {expected_role} violates its least-privilege boundary: {}",
            unsafe_properties.join(", ")
        )));
    }
    Ok(())
}

/// Starts the transaction used by every user-owned repository operation.
///
/// Supabase's `auth.uid()` reads the transaction-local
/// `request.jwt.claim.sub` setting. `request.jwt.claims` is installed as well
/// for policies that use `auth.jwt()`. A direct SeaORM connection does not set
/// either value automatically, so PostgreSQL requests install the validated
/// actor before touching an RLS-protected table. SQLite is supported only for
/// local tests and skips this PostgreSQL-specific step.
pub async fn begin_user_transaction(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
) -> Result<DatabaseTransaction, sea_orm::DbErr> {
    let transaction = db.begin().await?;
    if db.get_database_backend() == DatabaseBackend::Postgres {
        let claims = serde_json::json!({
            "sub": user_id,
            "role": "authenticated"
        })
        .to_string();
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r#"
                SELECT
                  set_config('request.jwt.claims', $1, true),
                  set_config('request.jwt.claim.sub', $2, true),
                  set_config('request.jwt.claim.role', 'authenticated', true)
                "#,
                [claims.into(), user_id.to_string().into()],
            ))
            .await?;
    }
    Ok(transaction)
}

/// Starts a narrowly scoped session-revocation transaction.
///
/// PostgreSQL callers must connect through `canonical_session_revoker`; using
/// the customer runtime role, table owner, or a `BYPASSRLS` login is rejected.
/// The fixed task name is installed transaction-locally for database auditing
/// and accidental raw-query prevention, while user JWT settings are cleared.
/// It is not a secret capability: the exact isolated role and its narrow grants
/// are the authorization boundary. SQLite is supported only for focused tests
/// and skips PostgreSQL role/settings checks.
pub async fn begin_session_revocation_transaction(
    db: &sea_orm::DatabaseConnection,
) -> Result<DatabaseTransaction, DbErr> {
    verify_session_revoker_database_role(db).await?;
    let transaction = db.begin().await?;
    if db.get_database_backend() == DatabaseBackend::Postgres {
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r#"
                SELECT
                  set_config('canonical.system_task', $1, true),
                  set_config('request.jwt.claims', '{}', true),
                  set_config('request.jwt.claim.sub', '', true),
                  set_config('request.jwt.claim.role', 'system', true)
                "#,
                ["session_revocation".into()],
            ))
            .await?;
    }
    Ok(transaction)
}

/// Starts a transaction for a separately deployed administrative service.
///
/// Administrative work is accepted only from the dedicated non-owner database
/// login and an actor whose access token has reached AAL2. The actor and AAL
/// are installed transaction-locally so capability functions derive identity
/// from `auth.uid()` instead of accepting a spoofable caller-supplied actor.
pub async fn begin_admin_transaction(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
    assurance_level: AssuranceLevel,
) -> Result<DatabaseTransaction, DbErr> {
    if assurance_level != AssuranceLevel::Aal2 {
        return Err(DbErr::Custom(
            "administrative transactions require Supabase AAL2".into(),
        ));
    }

    verify_admin_database_role(db).await?;
    let transaction = db.begin().await?;
    if db.get_database_backend() == DatabaseBackend::Postgres {
        let claims = serde_json::json!({
            "sub": user_id,
            "role": "authenticated",
            "aal": "aal2"
        })
        .to_string();
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r#"
                SELECT
                  set_config('request.jwt.claims', $1, true),
                  set_config('request.jwt.claim.sub', $2, true),
                  set_config('request.jwt.claim.role', 'authenticated', true),
                  set_config('request.jwt.claim.aal', 'aal2', true)
                "#,
                [claims.into(), user_id.to_string().into()],
            ))
            .await?;
    }
    Ok(transaction)
}

#[cfg(test)]
mod tests {
    use super::{
        begin_admin_transaction, begin_session_revocation_transaction, database_now,
        verify_runtime_database_role, AssuranceLevel,
    };
    use sea_orm::{prelude::DateTimeUtc, Database};
    use std::time::SystemTime;
    use uuid::Uuid;

    #[tokio::test]
    async fn sqlite_session_revocation_transaction_is_explicit_and_committable() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        verify_runtime_database_role(&db).await.unwrap();
        let transaction = begin_session_revocation_transaction(&db).await.unwrap();
        transaction.commit().await.unwrap();
    }

    #[tokio::test]
    async fn admin_transaction_requires_aal2_even_in_test_databases() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        let actor_id = Uuid::new_v4();
        assert!(begin_admin_transaction(&db, actor_id, AssuranceLevel::Aal1)
            .await
            .is_err());

        let transaction = begin_admin_transaction(&db, actor_id, AssuranceLevel::Aal2)
            .await
            .unwrap();
        transaction.commit().await.unwrap();
    }

    #[tokio::test]
    async fn sqlite_database_now_is_bounded_by_the_local_clock() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        let before = DateTimeUtc::from(SystemTime::now());
        let observed = database_now(&db).await.unwrap();
        let after = DateTimeUtc::from(SystemTime::now());

        assert!(observed >= before);
        assert!(observed <= after);
    }
}
