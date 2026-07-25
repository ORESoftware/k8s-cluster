use std::{collections::BTreeMap, sync::atomic::Ordering};

use reqwest::header::{HeaderName, HeaderValue};
use serde_json::{json, Value};
use tokio::time::timeout;

use crate::{
    lifecycle::{reconcile_pool, retire_container, start_one_for_pool},
    redis_lock::RedisLockGuard,
    types::{
        AppState, ContainerLease, ContainerStatus, DispatchRequest, DispatchResponse,
        PoolConfig, PoolRegistry, WarmContainer,
    },
    util::{duration_millis_u64, now_ms, safe_local_path},
};

pub(crate) fn pool_id_from_selector(registry: &PoolRegistry, selector: &str) -> Option<String> {
    if registry.configs.contains_key(selector) {
        Some(selector.to_string())
    } else {
        registry.slug_to_id.get(selector).cloned()
    }
}

fn normalized_affinity_key(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    let mut output = String::new();
    for ch in value.chars() {
        if output.len() >= 256 {
            break;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':') {
            output.push(ch);
        } else {
            output.push('-');
        }
    }
    let output = output.trim_matches('-').to_string();
    (!output.is_empty()).then_some(output)
}

fn affinity_map_key(pool_id: &str, affinity_key: &str) -> String {
    format!("{pool_id}:{affinity_key}")
}

async fn acquire_affinity_dispatch_lock(
    state: &AppState,
    selector: &str,
    affinity_key: Option<&str>,
) -> Result<Option<RedisLockGuard>, String> {
    let Some(affinity_key) = affinity_key else {
        return Ok(None);
    };
    let Some(redis_locks) = state.redis_locks.as_ref() else {
        return Ok(None);
    };
    let pool_id = {
        let registry = state.registry.lock().await;
        pool_id_from_selector(&registry, selector)
            .ok_or_else(|| format!("unknown container pool: {selector}"))?
    };
    redis_locks
        .acquire(&affinity_map_key(&pool_id, affinity_key))
        .await
        .map(Some)
}

pub(crate) fn remove_affinity_for_container(registry: &mut PoolRegistry, container_name: &str) {
    registry
        .affinity
        .retain(|_, mapped_name| mapped_name != container_name);
}

fn container_can_accept(pool: &PoolConfig, container: &WarmContainer) -> bool {
    !matches!(
        container.status,
        ContainerStatus::Starting | ContainerStatus::Draining | ContainerStatus::Unhealthy
    ) && container.in_flight < pool.max_concurrency_per_container
}

fn container_matches_affinity_request(
    container: &WarmContainer,
    affinity_key: &str,
    fresh_affinity: bool,
) -> bool {
    match container.affinity_key.as_deref() {
        Some(bound) => bound == affinity_key,
        None => !fresh_affinity || container.request_count == 0,
    }
}

async fn lease_container(
    state: &AppState,
    selector: &str,
    affinity_key: Option<&str>,
    fresh_affinity: bool,
) -> Result<ContainerLease, String> {
    let affinity_key = normalized_affinity_key(affinity_key);
    let pool_id = {
        let registry = state.registry.lock().await;
        pool_id_from_selector(&registry, selector)
            .ok_or_else(|| format!("unknown container pool: {selector}"))?
    };

    for _ in 0..2 {
        let maybe_lease = {
            let mut registry = state.registry.lock().await;
            let pool = registry
                .configs
                .get(&pool_id)
                .cloned()
                .ok_or_else(|| format!("unknown container pool: {selector}"))?;
            let candidate_name = if let Some(affinity_key) = affinity_key.as_deref() {
                let map_key = affinity_map_key(&pool.id, affinity_key);
                let mapped_name = registry.affinity.get(&map_key).cloned();
                let mapped_candidate = match mapped_name
                    .as_deref()
                    .and_then(|name| registry.containers.get(name))
                {
                    Some(container)
                        if container.pool_id == pool.id
                            && container_can_accept(&pool, container) =>
                    {
                        Some(container.name.clone())
                    }
                    Some(container)
                        if container.pool_id == pool.id
                            && !matches!(
                                container.status,
                                ContainerStatus::Draining | ContainerStatus::Unhealthy
                            ) =>
                    {
                        return Err(format!(
                                "affinity container {} for key {} is not ready (status {:?}, inFlight {})",
                                container.name, affinity_key, container.status, container.in_flight
                            ));
                    }
                    _ => None,
                };
                if mapped_candidate.is_none() {
                    if let Some(mapped_name) = mapped_name {
                        let clear_mapping = registry
                            .containers
                            .get(&mapped_name)
                            .map(|container| {
                                container.pool_id != pool.id
                                    || matches!(
                                        container.status,
                                        ContainerStatus::Draining | ContainerStatus::Unhealthy
                                    )
                            })
                            .unwrap_or(true);
                        if clear_mapping {
                            registry.affinity.remove(&map_key);
                        }
                    }
                }
                mapped_candidate.or_else(|| {
                    registry
                        .containers
                        .values()
                        .filter(|container| container.pool_id == pool.id)
                        .filter(|container| container_can_accept(&pool, container))
                        .filter(|container| {
                            container_matches_affinity_request(
                                container,
                                affinity_key,
                                fresh_affinity,
                            )
                        })
                        .min_by_key(|container| {
                            (
                                container.affinity_key.as_deref() != Some(affinity_key),
                                container.in_flight,
                                container.last_used_at_ms,
                            )
                        })
                        .map(|container| container.name.clone())
                })
            } else {
                registry
                    .containers
                    .values()
                    .filter(|container| container.pool_id == pool.id)
                    .filter(|container| container_can_accept(&pool, container))
                    .min_by_key(|container| (container.in_flight, container.last_used_at_ms))
                    .map(|container| container.name.clone())
            };
            candidate_name.and_then(|name| {
                let affinity = affinity_key.clone();
                if let Some(affinity_key) = affinity.as_deref() {
                    let map_key = affinity_map_key(&pool.id, affinity_key);
                    registry.affinity.insert(map_key, name.clone());
                }
                let container = registry.containers.get_mut(&name)?;
                if let Some(affinity_key) = affinity {
                    container.affinity_key = Some(affinity_key);
                }
                container.in_flight += 1;
                container.status = ContainerStatus::Busy;
                container.request_count += 1;
                container.last_used_at_ms = now_ms();
                Some(ContainerLease {
                    pool,
                    container: container.clone(),
                })
            })
        };
        if let Some(lease) = maybe_lease {
            return Ok(lease);
        }
        start_one_for_pool(state, &pool_id).await?;
    }

    Err(format!("no warm container available for pool {selector}"))
}

async fn release_container(state: &AppState, container_name: &str, failed: bool) {
    let mut registry = state.registry.lock().await;
    if let Some(container) = registry.containers.get_mut(container_name) {
        container.in_flight = container.in_flight.saturating_sub(1);
        if failed {
            container.failure_count += 1;
        }
        container.status = if container.in_flight == 0 {
            ContainerStatus::Idle
        } else {
            ContainerStatus::Busy
        };
        container.last_used_at_ms = now_ms();
    }
}

fn safe_dispatch_path(path: Option<&str>, fallback: &str) -> Result<String, String> {
    let path = path
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback);
    if !safe_local_path(path) {
        return Err("dispatch path must be a local absolute path".to_string());
    }
    Ok(path.to_string())
}

fn dispatch_body(request: &DispatchRequest) -> Value {
    request
        .payload
        .clone()
        .or_else(|| request.body.clone())
        .unwrap_or_else(|| json!({}))
}

fn request_id_from_request(request: &DispatchRequest) -> String {
    request
        .request_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("container-pool-request")
        .chars()
        .take(128)
        .collect()
}

pub(crate) fn target_url(container: &WarmContainer, path: &str) -> String {
    format!("http://127.0.0.1:{}{path}", container.port)
}

fn payload_string<'a>(body: &'a Value, camel_key: &str, snake_key: &str) -> Option<&'a str> {
    body.get(camel_key)
        .or_else(|| body.get(snake_key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn normalized_repo_identity(value: &str) -> String {
    let mut repo = value.trim().trim_end_matches('/').trim_end_matches(".git");
    if let Some(rest) = repo.strip_prefix("git@github.com:") {
        repo = rest;
    } else if let Some(rest) = repo.strip_prefix("ssh://git@github.com/") {
        repo = rest;
    } else if let Some(rest) = repo.strip_prefix("https://github.com/") {
        repo = rest;
    } else if let Some(rest) = repo.strip_prefix("http://github.com/") {
        repo = rest;
    }
    repo.to_ascii_lowercase()
}

fn validate_repo_affinity(pool: &PoolConfig, body: &Value) -> Result<(), String> {
    let Some(configured_repo) = pool.env.get("DD_REPO_URL").map(String::as_str) else {
        return Ok(());
    };
    let Some(request_repo) = payload_string(body, "repo", "repo") else {
        return Ok(());
    };
    if normalized_repo_identity(configured_repo) != normalized_repo_identity(request_repo) {
        return Err(format!(
            "pool {} is configured for repo {configured_repo}, not {request_repo}",
            pool.slug
        ));
    }

    let configured_branch = pool
        .env
        .get("BASE_BRANCH")
        .or_else(|| pool.env.get("DD_REPO_REF"))
        .map(String::as_str)
        .unwrap_or("dev")
        .trim();
    let request_branch = payload_string(body, "baseBranch", "base_branch").unwrap_or("dev");
    if configured_branch != request_branch {
        return Err(format!(
            "pool {} is configured for baseBranch {configured_branch}, not {request_branch}",
            pool.slug
        ));
    }
    Ok(())
}

fn apply_forward_headers(
    mut builder: reqwest::RequestBuilder,
    headers: Option<&BTreeMap<String, String>>,
) -> reqwest::RequestBuilder {
    let Some(headers) = headers else {
        return builder;
    };
    for (key, value) in headers.iter().take(32) {
        if key.len() > 64 || value.len() > 8192 {
            continue;
        }
        let lower = key.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "authorization"
                | "cookie"
                | "host"
                | "connection"
                | "content-length"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
                | "x-agent-auth"
                | "x-container-pool-auth"
                | "x-server-auth"
        ) || lower.starts_with("proxy-")
        {
            continue;
        }
        let Ok(name) = HeaderName::from_bytes(key.as_bytes()) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_str(value) else {
            continue;
        };
        builder = builder.header(name, value);
    }
    builder
}

async fn read_limited_response_body(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(format!(
            "container response exceeded configured byte limit ({max_bytes})"
        ));
    }

    let body = response.bytes().await.map_err(|error| error.to_string())?;
    if body.len() > max_bytes {
        return Err(format!(
            "container response exceeded configured byte limit ({max_bytes})"
        ));
    }
    Ok(body.to_vec())
}

pub(crate) async fn dispatch_to_pool(
    state: &AppState,
    selector: &str,
    request: DispatchRequest,
) -> Result<DispatchResponse, String> {
    let affinity_key = normalized_affinity_key(request.affinity_key.as_deref());
    let fresh_affinity = request.fresh_affinity.unwrap_or(false) && affinity_key.is_some();
    let lock_guard =
        match acquire_affinity_dispatch_lock(state, selector, affinity_key.as_deref()).await {
            Ok(lock_guard) => lock_guard,
            Err(error) => {
                state
                    .metrics
                    .dispatch_failures_total
                    .fetch_add(1, Ordering::Relaxed);
                return Err(error);
            }
        };
    let result =
        dispatch_to_pool_inner(state, selector, request, affinity_key, fresh_affinity).await;
    if let Some(lock_guard) = lock_guard {
        if let Err(error) = lock_guard.release().await {
            tracing::error!("container pool redis affinity lock release failed: {error}");
        }
    }
    result
}

async fn dispatch_to_pool_inner(
    state: &AppState,
    selector: &str,
    request: DispatchRequest,
    affinity_key: Option<String>,
    fresh_affinity: bool,
) -> Result<DispatchResponse, String> {
    let started = now_ms();
    let lease = lease_container(state, selector, affinity_key.as_deref(), fresh_affinity).await?;
    let path = match safe_dispatch_path(request.path.as_deref(), &lease.pool.request_path) {
        Ok(path) => path,
        Err(error) => {
            release_container(state, &lease.container.name, true).await;
            state
                .metrics
                .dispatch_failures_total
                .fetch_add(1, Ordering::Relaxed);
            return Err(error);
        }
    };
    let url = target_url(&lease.container, &path);
    let body = dispatch_body(&request);
    if let Err(error) = validate_repo_affinity(&lease.pool, &body) {
        release_container(state, &lease.container.name, true).await;
        state
            .metrics
            .dispatch_failures_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(error);
    }
    let request_id = request_id_from_request(&request);
    let backfill_state = state.clone();
    let backfill_pool_id = lease.pool.id.clone();
    tokio::spawn(async move {
        if let Err(error) = reconcile_pool(&backfill_state, &backfill_pool_id).await {
            tracing::error!("container pool backfill failed for {backfill_pool_id}: {error}");
        }
    });

    let mut send = apply_forward_headers(state.http.post(&url), request.headers.as_ref());
    if let Some(secret) = state.config.server_auth_secret.as_deref() {
        send = send
            .header("x-server-auth", secret)
            .header("x-container-pool-auth", secret)
            .header("x-agent-auth", secret);
    }
    let send = send.json(&body);
    let response = timeout(lease.pool.request_timeout, send.send()).await;
    let mut retire_reason = None::<String>;
    let result = match response {
        Ok(Ok(response)) => {
            let status = response.status();
            match read_limited_response_body(response, state.config.worker_response_max_bytes).await
            {
                Ok(bytes) => {
                    let body = serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|_| {
                        json!({
                            "text": String::from_utf8_lossy(&bytes).chars().take(256 * 1024).collect::<String>()
                        })
                    });
                    Ok(DispatchResponse {
                        ok: status.is_success(),
                        request_id,
                        pool_id: lease.pool.id.clone(),
                        pool_slug: lease.pool.slug.clone(),
                        affinity_key: affinity_key.clone(),
                        container_name: lease.container.name.clone(),
                        container_port: lease.container.port,
                        target_url: url,
                        status: status.as_u16(),
                        body,
                        elapsed_ms: now_ms().saturating_sub(started),
                    })
                }
                Err(error) => {
                    let message = error.to_string();
                    retire_reason = Some(message.clone());
                    Err(message)
                }
            }
        }
        Ok(Err(error)) => {
            let message = error.to_string();
            retire_reason = Some(message.clone());
            Err(message)
        }
        Err(_) => {
            let message = format!(
                "container dispatch timed out after {}ms",
                duration_millis_u64(lease.pool.request_timeout)
            );
            retire_reason = Some(message.clone());
            Err(message)
        }
    };

    let failed = result.as_ref().map(|response| !response.ok).unwrap_or(true);
    if let Some(reason) = retire_reason.as_deref() {
        retire_container(state, &lease.container.name, reason).await;
    } else {
        release_container(state, &lease.container.name, failed).await;
    }
    if failed {
        state
            .metrics
            .dispatch_failures_total
            .fetch_add(1, Ordering::Relaxed);
    } else {
        state.metrics.dispatch_total.fetch_add(1, Ordering::Relaxed);
    }
    if retire_reason.is_some() {
        let refill_state = state.clone();
        let refill_pool_id = lease.pool.id.clone();
        tokio::spawn(async move {
            if let Err(error) = reconcile_pool(&refill_state, &refill_pool_id).await {
                tracing::error!("container pool refill failed for {refill_pool_id}: {error}");
            }
        });
    }
    result
}

pub(crate) fn pool_selector_from_request(
    request: &DispatchRequest,
    subject: Option<&str>,
    state: &PoolRegistry,
) -> Option<String> {
    request
        .pool_id
        .as_deref()
        .or(request.pool_slug.as_deref())
        .map(ToString::to_string)
        .or_else(|| {
            subject.and_then(|subject| {
                state
                    .configs
                    .values()
                    .find(|config| config.nats_subject.as_deref() == Some(subject))
                    .map(|config| config.id.clone())
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_container(affinity_key: Option<&str>, request_count: u64) -> WarmContainer {
        WarmContainer {
            name: "dd-pool-test-1".to_string(),
            pool_id: "pool-1".to_string(),
            pool_slug: "nodejs-chat-claude-k8s-cluster-dev".to_string(),
            affinity_key: affinity_key.map(str::to_string),
            port: 31001,
            status: ContainerStatus::Idle,
            in_flight: 0,
            launched_at_ms: 1,
            last_used_at_ms: 1,
            last_health_at_ms: None,
            last_healthy_at_ms: None,
            health_failure_count: 0,
            last_health_error: None,
            request_count,
            failure_count: 0,
        }
    }

    #[test]
    fn redis_affinity_lock_key_uses_normalized_thread_affinity() {
        let affinity_key = normalized_affinity_key(Some(" thread 1 / bad chars "))
            .expect("normalized affinity key");

        assert_eq!(affinity_key, "thread-1---bad-chars");
        assert_eq!(
            affinity_map_key("nodejs-chat-claude-k8s-cluster-dev", &affinity_key),
            "nodejs-chat-claude-k8s-cluster-dev:thread-1---bad-chars"
        );
    }

    #[test]
    fn fresh_affinity_does_not_reuse_unbound_used_container() {
        let new_thread = "47fc0453-5af1-4807-821e-5b24c4839398";
        let used_unbound = test_container(None, 1);
        let clean_unbound = test_container(None, 0);
        let same_thread = test_container(Some(new_thread), 7);
        let other_thread = test_container(Some("11111111-1111-4111-8111-111111111111"), 3);

        assert!(!container_matches_affinity_request(
            &used_unbound,
            new_thread,
            true
        ));
        assert!(container_matches_affinity_request(
            &clean_unbound,
            new_thread,
            true
        ));
        assert!(container_matches_affinity_request(
            &same_thread,
            new_thread,
            true
        ));
        assert!(!container_matches_affinity_request(
            &other_thread,
            new_thread,
            true
        ));
        assert!(container_matches_affinity_request(
            &used_unbound,
            new_thread,
            false
        ));
    }
}
