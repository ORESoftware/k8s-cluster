//! Optional Redis/Valkey acceleration.
//!
//! The cache is deliberately non-authoritative. Postgres owns sessions and
//! roles; Redis provides distributed rate-limit counters and fast revocation
//! markers. Cache errors are observable and callers fall back to Postgres.

use redis::AsyncCommands;
use uuid::Uuid;

use crate::config::RedisConfig;

#[derive(Clone)]
pub struct Cache {
    connection: redis::aio::ConnectionManager,
    prefix: String,
}

impl Cache {
    pub async fn connect(config: &RedisConfig) -> anyhow::Result<Self> {
        let client = redis::Client::open(config.url.as_str())?;
        let connection = client.get_connection_manager().await?;
        Ok(Self {
            connection,
            prefix: config.key_prefix.trim_end_matches(':').to_owned(),
        })
    }

    pub async fn ping(&self) -> redis::RedisResult<()> {
        let mut connection = self.connection.clone();
        let _: String = redis::cmd("PING").query_async(&mut connection).await?;
        Ok(())
    }

    pub async fn mark_revoked(&self, session_id: Uuid, ttl_secs: u64) -> redis::RedisResult<()> {
        let mut connection = self.connection.clone();
        let key = format!("{}:revoked:{session_id}", self.prefix);
        connection.set_ex(key, "1", ttl_secs.max(60)).await
    }

    pub async fn is_revoked(&self, session_id: Uuid) -> redis::RedisResult<bool> {
        let mut connection = self.connection.clone();
        let key = format!("{}:revoked:{session_id}", self.prefix);
        connection.exists(key).await
    }

    /// Returns `true` while the bucket is within its allowance. Identifiers
    /// passed here must already be hashed; raw email/IP values never enter Redis.
    pub async fn allow(
        &self,
        bucket: &str,
        identifier_hash: &str,
        limit: u64,
        window_secs: u64,
    ) -> redis::RedisResult<bool> {
        let mut connection = self.connection.clone();
        let key = format!("{}:limit:{bucket}:{identifier_hash}", self.prefix);
        let count: u64 = connection.incr(&key, 1_u64).await?;
        if count == 1 {
            let _: bool = connection.expire(&key, window_secs as i64).await?;
        }
        Ok(count <= limit)
    }
}
