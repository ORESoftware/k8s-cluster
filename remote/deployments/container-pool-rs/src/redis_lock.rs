use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use tokio::time::{sleep, timeout};

use crate::{
    types::SERVICE_NAME,
    util::{duration_millis_u64, now_ms},
};

static LOCK_TOKEN_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub(crate) struct RedisLockManager {
    client: redis::Client,
    key_prefix: String,
    ttl: Duration,
    wait_timeout: Duration,
    retry_delay: Duration,
    request_timeout: Duration,
}

pub(crate) struct RedisLockGuard {
    manager: RedisLockManager,
    key: String,
    token: String,
}

impl RedisLockManager {
    pub(crate) fn new(
        redis_url: &str,
        key_prefix: String,
        ttl: Duration,
        wait_timeout: Duration,
        retry_delay: Duration,
        request_timeout: Duration,
    ) -> Result<Self, String> {
        let client = redis::Client::open(redis_url)
            .map_err(|error| format!("invalid container pool redis url: {error}"))?;
        Ok(Self {
            client,
            key_prefix: key_prefix.trim_matches(':').to_string(),
            ttl,
            wait_timeout,
            retry_delay,
            request_timeout,
        })
    }

    fn lock_key(&self, suffix: &str) -> String {
        format!("{}:{suffix}", self.key_prefix)
    }

    pub(crate) async fn acquire(&self, suffix: &str) -> Result<RedisLockGuard, String> {
        let key = self.lock_key(suffix);
        let token = next_lock_token();
        let started = tokio::time::Instant::now();
        let mut last_error = None::<String>;
        loop {
            match self.try_acquire(&key, &token).await {
                Ok(true) => {
                    return Ok(RedisLockGuard {
                        manager: self.clone(),
                        key,
                        token,
                    });
                }
                Ok(false) => {}
                Err(error) => last_error = Some(error),
            }
            if started.elapsed() >= self.wait_timeout {
                let waited_ms = duration_millis_u64(started.elapsed());
                let detail = last_error
                    .map(|error| format!("; last redis error: {error}"))
                    .unwrap_or_default();
                return Err(format!(
                    "timed out after {waited_ms}ms waiting for container affinity lock {key}{detail}"
                ));
            }
            sleep(self.retry_delay).await;
        }
    }

    async fn try_acquire(&self, key: &str, token: &str) -> Result<bool, String> {
        let mut connection = timeout(
            self.request_timeout,
            self.client.get_multiplexed_async_connection(),
        )
        .await
        .map_err(|_| {
            format!(
                "redis connection timed out after {}ms",
                duration_millis_u64(self.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        let ttl_ms = duration_millis_u64(self.ttl).max(1);
        let response: Option<String> = timeout(
            self.request_timeout,
            redis::cmd("SET")
                .arg(key)
                .arg(token)
                .arg("NX")
                .arg("PX")
                .arg(ttl_ms)
                .query_async(&mut connection),
        )
        .await
        .map_err(|_| {
            format!(
                "redis SET NX timed out after {}ms",
                duration_millis_u64(self.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        Ok(response
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("OK")))
    }
}

impl RedisLockGuard {
    pub(crate) async fn release(self) -> Result<bool, String> {
        let mut connection = timeout(
            self.manager.request_timeout,
            self.manager.client.get_multiplexed_async_connection(),
        )
        .await
        .map_err(|_| {
            format!(
                "redis connection timed out after {}ms",
                duration_millis_u64(self.manager.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        let _: String = timeout(
            self.manager.request_timeout,
            redis::cmd("WATCH")
                .arg(&self.key)
                .query_async(&mut connection),
        )
        .await
        .map_err(|_| {
            format!(
                "redis WATCH timed out after {}ms",
                duration_millis_u64(self.manager.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        let current: Option<String> = timeout(
            self.manager.request_timeout,
            redis::cmd("GET")
                .arg(&self.key)
                .query_async(&mut connection),
        )
        .await
        .map_err(|_| {
            format!(
                "redis GET timed out after {}ms",
                duration_millis_u64(self.manager.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        if current.as_deref() != Some(self.token.as_str()) {
            let _: String = timeout(
                self.manager.request_timeout,
                redis::cmd("UNWATCH").query_async(&mut connection),
            )
            .await
            .map_err(|_| {
                format!(
                    "redis UNWATCH timed out after {}ms",
                    duration_millis_u64(self.manager.request_timeout)
                )
            })?
            .map_err(|error| error.to_string())?;
            return Ok(false);
        }
        let _: String = timeout(
            self.manager.request_timeout,
            redis::cmd("MULTI").query_async(&mut connection),
        )
        .await
        .map_err(|_| {
            format!(
                "redis MULTI timed out after {}ms",
                duration_millis_u64(self.manager.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        let _: String = timeout(
            self.manager.request_timeout,
            redis::cmd("DEL")
                .arg(&self.key)
                .query_async(&mut connection),
        )
        .await
        .map_err(|_| {
            format!(
                "redis DEL timed out after {}ms",
                duration_millis_u64(self.manager.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        let deleted: Option<Vec<i64>> = timeout(
            self.manager.request_timeout,
            redis::cmd("EXEC").query_async(&mut connection),
        )
        .await
        .map_err(|_| {
            format!(
                "redis EXEC timed out after {}ms",
                duration_millis_u64(self.manager.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        Ok(deleted
            .and_then(|values| values.first().copied())
            .unwrap_or_default()
            > 0)
    }
}

fn next_lock_token() -> String {
    let seq = LOCK_TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{SERVICE_NAME}:{}:{}:{seq}", std::process::id(), now_ms())
}
