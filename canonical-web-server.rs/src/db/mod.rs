pub mod entity;
pub mod migration;

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseTransaction, Statement, TransactionTrait};
use uuid::Uuid;

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
            .execute(Statement::from_sql_and_values(
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
