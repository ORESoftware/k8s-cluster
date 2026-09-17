//! SeaORM-backed persistence adapter for the existing reviewed SQL.
//!
//! Schema changes are applied out of band by the cluster. SeaORM owns pool,
//! TLS, transactions, binding, and row decoding here; keeping the SQL visible
//! preserves the service's existing query contracts during modularization.

use std::time::Duration;

use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection,
    DatabaseTransaction, DbErr, QueryResult, Statement, TransactionTrait, TryGetable, Value,
};

/// A value that can be bound to a SeaORM statement.
pub trait DbParam: Sync {
    fn to_value(&self) -> Value;
    fn null_value() -> Value
    where
        Self: Sized;
}

macro_rules! impl_db_param {
    ($type:ty, $null:expr) => {
        impl DbParam for $type {
            fn to_value(&self) -> Value {
                self.clone().into()
            }

            fn null_value() -> Value {
                $null
            }
        }
    };
}

impl DbParam for &str {
    fn to_value(&self) -> Value {
        Value::from(*self)
    }

    fn null_value() -> Value {
        Value::String(None)
    }
}

impl<T> DbParam for Option<T>
where
    T: DbParam,
{
    fn to_value(&self) -> Value {
        self.as_ref()
            .map(DbParam::to_value)
            .unwrap_or_else(T::null_value)
    }

    fn null_value() -> Value {
        T::null_value()
    }
}

impl_db_param!(String, Value::String(None));
impl_db_param!(bool, Value::Bool(None));
impl_db_param!(i16, Value::SmallInt(None));
impl_db_param!(i32, Value::Int(None));
impl_db_param!(i64, Value::BigInt(None));
impl_db_param!(u16, Value::SmallUnsigned(None));
impl_db_param!(u32, Value::Unsigned(None));
impl_db_param!(f32, Value::Float(None));
impl_db_param!(f64, Value::Double(None));
impl_db_param!(Vec<u8>, Value::Bytes(None));
impl_db_param!(serde_json::Value, Value::Json(None));
impl_db_param!(DateTime<Utc>, Value::ChronoDateTimeUtc(None));
impl_db_param!(
    Vec<String>,
    Value::Array(sea_orm::sea_query::ArrayType::String, None)
);

/// Row wrapper with the same typed `get("column")` ergonomics used by the
/// original postgres client.
pub struct Row(QueryResult);

impl Row {
    pub fn get<T>(&self, column: &str) -> T
    where
        T: TryGetable,
    {
        self.0.try_get("", column).unwrap_or_else(|error| {
            panic!("database column {column:?} could not be decoded: {error}")
        })
    }
}

/// Cloneable SeaORM connection pool handle.
#[derive(Clone)]
pub struct DbClient {
    connection: DatabaseConnection,
}

impl DbClient {
    pub async fn connect(database_url: &str, max_connections: u32) -> Result<Self, DbErr> {
        let mut options = ConnectOptions::new(database_url.to_string());
        options
            .max_connections(max_connections)
            .min_connections(0)
            .connect_lazy(true)
            .connect_timeout(Duration::from_secs(5))
            .acquire_timeout(Duration::from_secs(5))
            .idle_timeout(Duration::from_secs(10 * 60))
            .sqlx_logging(false);
        let connection = Database::connect(options).await?;
        Ok(Self { connection })
    }

    pub async fn query_one(
        &self,
        sql: &str,
        params: &[&(dyn DbParam + Sync)],
    ) -> Result<Row, DbErr> {
        query_one(&self.connection, sql, params).await
    }

    pub async fn simple_query(&self, sql: &str) -> Result<(), DbErr> {
        self.connection
            .query_one(Statement::from_string(DatabaseBackend::Postgres, sql))
            .await?
            .ok_or_else(|| DbErr::RecordNotFound("query returned no rows".to_string()))?;
        Ok(())
    }

    pub async fn query_opt(
        &self,
        sql: &str,
        params: &[&(dyn DbParam + Sync)],
    ) -> Result<Option<Row>, DbErr> {
        query_opt(&self.connection, sql, params).await
    }

    pub async fn query(
        &self,
        sql: &str,
        params: &[&(dyn DbParam + Sync)],
    ) -> Result<Vec<Row>, DbErr> {
        query(&self.connection, sql, params).await
    }

    pub async fn execute(&self, sql: &str, params: &[&(dyn DbParam + Sync)]) -> Result<u64, DbErr> {
        execute(&self.connection, sql, params).await
    }

    pub async fn transaction(&self) -> Result<DbTransaction, DbErr> {
        Ok(DbTransaction {
            transaction: Some(self.connection.begin().await?),
        })
    }
}

/// Owned SeaORM transaction. `commit` consumes it so accidental reuse is not
/// possible after the database has accepted the commit.
pub struct DbTransaction {
    transaction: Option<DatabaseTransaction>,
}

impl DbTransaction {
    fn connection(&self) -> &DatabaseTransaction {
        self.transaction
            .as_ref()
            .expect("transaction is unavailable after commit")
    }

    pub async fn execute(&self, sql: &str, params: &[&(dyn DbParam + Sync)]) -> Result<u64, DbErr> {
        execute(self.connection(), sql, params).await
    }

    pub async fn commit(mut self) -> Result<(), DbErr> {
        self.transaction
            .take()
            .expect("transaction is unavailable after commit")
            .commit()
            .await
    }
}

fn statement(sql: &str, params: &[&(dyn DbParam + Sync)]) -> Statement {
    Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        params.iter().map(|value| value.to_value()),
    )
}

async fn query_one<C>(
    connection: &C,
    sql: &str,
    params: &[&(dyn DbParam + Sync)],
) -> Result<Row, DbErr>
where
    C: ConnectionTrait,
{
    connection
        .query_one(statement(sql, params))
        .await?
        .map(Row)
        .ok_or_else(|| DbErr::RecordNotFound("query returned no rows".to_string()))
}

async fn query_opt<C>(
    connection: &C,
    sql: &str,
    params: &[&(dyn DbParam + Sync)],
) -> Result<Option<Row>, DbErr>
where
    C: ConnectionTrait,
{
    Ok(connection.query_one(statement(sql, params)).await?.map(Row))
}

async fn query<C>(
    connection: &C,
    sql: &str,
    params: &[&(dyn DbParam + Sync)],
) -> Result<Vec<Row>, DbErr>
where
    C: ConnectionTrait,
{
    Ok(connection
        .query_all(statement(sql, params))
        .await?
        .into_iter()
        .map(Row)
        .collect())
}

async fn execute<C>(
    connection: &C,
    sql: &str,
    params: &[&(dyn DbParam + Sync)],
) -> Result<u64, DbErr>
where
    C: ConnectionTrait,
{
    Ok(connection
        .execute(statement(sql, params))
        .await?
        .rows_affected())
}
