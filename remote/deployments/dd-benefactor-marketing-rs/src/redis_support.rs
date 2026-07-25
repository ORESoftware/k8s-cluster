use std::sync::atomic::Ordering;

use serde_json::Value;
use tracing::warn;
use uuid::Uuid;

use crate::state::{AppState, SERVICE_NAME};
pub(crate) async fn redis_ping(state: &AppState) -> bool {
    if state.redis.is_none() {
        return false;
    }
    let mut guard = state.redis_connection.lock().await;
    if !ensure_redis_connection(state, &mut *guard).await {
        return false;
    }
    let result: redis::RedisResult<String> = {
        let conn = guard.as_mut().expect("redis connection ensured");
        redis::cmd("PING").query_async(conn).await
    };
    match result {
        Ok(_) => true,
        Err(err) => {
            *guard = None;
            record_redis_error(state, "ping", &err);
            false
        }
    }
}

pub(crate) async fn cache_get_json(state: &AppState, key: &str) -> Option<Value> {
    if state.redis.is_none() || state.cfg.cache_ttl_seconds == 0 {
        return None;
    }
    let mut guard = state.redis_connection.lock().await;
    if !ensure_redis_connection(state, &mut *guard).await {
        return None;
    }
    let result: redis::RedisResult<Option<String>> = {
        let conn = guard.as_mut().expect("redis connection ensured");
        redis::cmd("GET").arg(key).query_async(conn).await
    };
    match result {
        Ok(Some(body)) => match serde_json::from_str(&body) {
            Ok(value) => {
                state
                    .metrics
                    .cache_hits_total
                    .fetch_add(1, Ordering::Relaxed);
                Some(value)
            }
            Err(err) => {
                warn!(error = %err, key, "redis cache payload was not valid JSON");
                state
                    .metrics
                    .cache_misses_total
                    .fetch_add(1, Ordering::Relaxed);
                None
            }
        },
        Ok(None) => {
            state
                .metrics
                .cache_misses_total
                .fetch_add(1, Ordering::Relaxed);
            None
        }
        Err(err) => {
            *guard = None;
            record_redis_error(state, "cache_get", &err);
            None
        }
    }
}

pub(crate) async fn cache_set_json(state: &AppState, key: &str, value: &Value) {
    if state.redis.is_none() || state.cfg.cache_ttl_seconds == 0 {
        return;
    }
    let Ok(body) = serde_json::to_string(value) else {
        return;
    };
    let mut guard = state.redis_connection.lock().await;
    if !ensure_redis_connection(state, &mut *guard).await {
        return;
    }
    let result: redis::RedisResult<()> = {
        let conn = guard.as_mut().expect("redis connection ensured");
        redis::cmd("SETEX")
            .arg(key)
            .arg(state.cfg.cache_ttl_seconds)
            .arg(body)
            .query_async(conn)
            .await
    };
    if let Err(err) = result {
        *guard = None;
        record_redis_error(state, "cache_set", &err);
    }
}

async fn cache_delete(state: &AppState, key: &str) {
    if state.redis.is_none() {
        return;
    }
    let mut guard = state.redis_connection.lock().await;
    if !ensure_redis_connection(state, &mut *guard).await {
        return;
    }
    let result: redis::RedisResult<u64> = {
        let conn = guard.as_mut().expect("redis connection ensured");
        redis::cmd("DEL").arg(key).query_async(conn).await
    };
    match result {
        Ok(deleted) if deleted > 0 => {
            state
                .metrics
                .cache_invalidations_total
                .fetch_add(deleted, Ordering::Relaxed);
        }
        Ok(_) => {}
        Err(err) => {
            *guard = None;
            record_redis_error(state, "cache_delete", &err);
        }
    }
}

pub(crate) async fn redis_incr_with_ttl(state: &AppState, key: &str, ttl_seconds: i64) -> Option<i64> {
    let mut guard = state.redis_connection.lock().await;
    if !ensure_redis_connection(state, &mut *guard).await {
        return None;
    }
    let result: redis::RedisResult<i64> = {
        let conn = guard.as_mut().expect("redis connection ensured");
        redis::cmd("INCR").arg(key).query_async(conn).await
    };
    match result {
        Ok(count) => {
            if count == 1 {
                let expire_result: redis::RedisResult<bool> = {
                    let conn = guard.as_mut().expect("redis connection ensured");
                    redis::cmd("EXPIRE")
                        .arg(key)
                        .arg(ttl_seconds)
                        .query_async(conn)
                        .await
                };
                if let Err(err) = expire_result {
                    record_redis_error(state, "rate_limit_expire", &err);
                }
            }
            Some(count)
        }
        Err(err) => {
            *guard = None;
            record_redis_error(state, "rate_limit_incr", &err);
            None
        }
    }
}

pub(crate) async fn publish_job_event(state: &AppState, event_kind: &'static str, payload: Value) {
    if state.redis.is_none() {
        return;
    }
    let Ok(payload_body) = serde_json::to_string(&payload) else {
        return;
    };
    let mut guard = state.redis_connection.lock().await;
    if !ensure_redis_connection(state, &mut *guard).await {
        return;
    }
    let result: redis::RedisResult<String> = {
        let conn = guard.as_mut().expect("redis connection ensured");
        redis::cmd("XADD")
            .arg(&state.cfg.job_stream)
            .arg("*")
            .arg("service")
            .arg(SERVICE_NAME)
            .arg("eventKind")
            .arg(event_kind)
            .arg("payload")
            .arg(payload_body)
            .query_async(conn)
            .await
    };
    match result {
        Ok(_) => {
            state
                .metrics
                .redis_jobs_published_total
                .fetch_add(1, Ordering::Relaxed);
        }
        Err(err) => {
            *guard = None;
            record_redis_error(state, "stream_publish", &err);
        }
    }
}

async fn ensure_redis_connection(
    state: &AppState,
    connection: &mut Option<redis::aio::MultiplexedConnection>,
) -> bool {
    if connection.is_some() {
        return true;
    }
    let Some(client) = state.redis.as_ref() else {
        return false;
    };
    match client.get_multiplexed_async_connection().await {
        Ok(conn) => {
            *connection = Some(conn);
            true
        }
        Err(err) => {
            record_redis_error(state, "connect", &err);
            false
        }
    }
}

fn record_redis_error(state: &AppState, context: &'static str, err: &redis::RedisError) {
    state
        .metrics
        .redis_errors_total
        .fetch_add(1, Ordering::Relaxed);
    warn!(context, error = %err, "redis operation failed");
}

pub(crate) fn client_dashboard_cache_key(client_id: Uuid) -> String {
    format!("benefactor:marketing:client-dashboard:{client_id}")
}

pub(crate) fn record_mutation(state: &AppState) {
    state
        .metrics
        .mutations_total
        .fetch_add(1, Ordering::Relaxed);
}

pub(crate) async fn record_client_mutation(state: &AppState, client_id: Uuid) {
    record_mutation(state);
    cache_delete(state, &client_dashboard_cache_key(client_id)).await;
}
