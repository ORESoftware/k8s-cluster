use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    sync::atomic::Ordering,
    time::Duration,
};

use dd_nats_subject_defs::{container_pool_events_subject, container_pool_heartbeats_subject};
use tokio::time::{sleep, timeout};

use crate::{
    dispatch::{remove_affinity_for_container, target_url},
    engine::{engine_global_args, inspect_container_running, remove_container, run_command},
    pool_config::enforce_mount_policy,
    types::{
        AppState, ContainerStatus, PoolConfig, ServiceConfig, WarmContainer,
        MAX_HTTP_BODY_BYTES, SERVICE_NAME,
    },
    util::{duration_millis_u64, now_ms},
};

async fn allocate_container_slot(
    state: &AppState,
    pool_id: &str,
) -> Result<(PoolConfig, WarmContainer), String> {
    retire_stale_starting_containers(state, Some(pool_id)).await;

    let mut registry = state.registry.lock().await;
    let pool = registry
        .configs
        .get(pool_id)
        .cloned()
        .ok_or_else(|| format!("unknown container pool: {pool_id}"))?;
    let active = registry
        .containers
        .values()
        .filter(|container| container.pool_id == pool.id)
        .count();
    if active >= pool.max_warm {
        return Err(format!(
            "container pool {} is at max capacity ({})",
            pool.slug, pool.max_warm
        ));
    }

    let used_ports = registry
        .containers
        .values()
        .map(|container| container.port)
        .collect::<HashSet<_>>();
    let mut port = registry.next_port.max(state.config.port_start);
    let mut scanned = 0u32;
    while used_ports.contains(&port) {
        port = if port >= state.config.port_end {
            state.config.port_start
        } else {
            port + 1
        };
        scanned += 1;
        if scanned > u32::from(state.config.port_end - state.config.port_start) + 1 {
            return Err("container pool port range is exhausted".to_string());
        }
    }
    registry.next_port = if port >= state.config.port_end {
        state.config.port_start
    } else {
        port + 1
    };

    let name = format!(
        "dd-pool-{}-{}-{}",
        pool.slug,
        port,
        now_ms() % 1_000_000_000
    );
    let now = now_ms();
    let container = WarmContainer {
        name: name.clone(),
        pool_id: pool.id.clone(),
        pool_slug: pool.slug.clone(),
        affinity_key: None,
        port,
        status: ContainerStatus::Starting,
        in_flight: 0,
        launched_at_ms: now,
        last_used_at_ms: now,
        last_health_at_ms: None,
        last_healthy_at_ms: None,
        health_failure_count: 0,
        last_health_error: None,
        request_count: 0,
        failure_count: 0,
    };
    registry.containers.insert(name, container.clone());
    Ok((pool, container))
}

// Pure builder for the engine `run -d` argv (everything after the engine binary),
// extracted so it can be unit-tested across engines/runtimes/languages without a
// live daemon. Network/env are resolved by the caller and passed in; mount policy
// is enforced here so a disallowed mount fails the start with a clear error.
#[allow(clippy::too_many_arguments)]
fn build_run_args(
    config: &ServiceConfig,
    pool: &PoolConfig,
    container_name: &str,
    host_port: u16,
    container_env: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    let mut args = engine_global_args(config.engine, &config.containerd_namespace);
    args.push("run".to_string());
    args.push("-d".to_string());
    // Lower-level OCI runtime (runc/crun/Kata/gVisor) selection, engine-agnostic.
    if let Some(runtime) = config.oci_runtime.as_deref() {
        args.push("--runtime".to_string());
        args.push(runtime.to_string());
    }
    args.extend([
        "--name".to_string(),
        container_name.to_string(),
        "--label".to_string(),
        "dd.container-pool.managed=true".to_string(),
        "--label".to_string(),
        format!("dd.container-pool.pool={}", pool.slug),
        "--label".to_string(),
        format!("dd.container-pool.pool-id={}", pool.id),
        "--label".to_string(),
        format!("dd.container-pool.service={SERVICE_NAME}"),
        "--user".to_string(),
        pool.user.clone(),
        "--pids-limit".to_string(),
        config.pids_limit.to_string(),
        "--ulimit".to_string(),
        format!("nofile={limit}:{limit}", limit = config.nofile_limit),
    ]);
    // Pools that mount external code (the generic shared-volume case) are confined
    // by default — `--cap-drop ALL` + no-new-privileges — even when the service
    // defaults leave them off, since they run code the image did not bake in. A
    // pool can opt out with `unconfined: true` (falls back to the service flags;
    // does not grant extra privileges). Mount-less pools keep prior behavior.
    let mount_hardened = !pool.mounts.is_empty() && !pool.unconfined;
    if config.cap_drop_all || mount_hardened {
        args.push("--cap-drop".to_string());
        args.push("ALL".to_string());
    }
    if config.no_new_privileges || mount_hardened {
        args.push("--security-opt".to_string());
        args.push("no-new-privileges".to_string());
    }
    if pool.read_only {
        args.push("--read-only".to_string());
        args.push("--tmpfs".to_string());
        args.push("/tmp:rw,noexec,nosuid,size=64m".to_string());
    }
    if let Some(memory) = config.container_memory.as_deref() {
        args.push("--memory".to_string());
        args.push(memory.to_string());
    }
    if let Some(cpus) = config.container_cpus.as_deref() {
        args.push("--cpus".to_string());
        args.push(cpus.to_string());
    }
    if let Some(pull_policy) = config.pull_policy.as_deref() {
        args.push("--pull".to_string());
        args.push(pull_policy.to_string());
    }

    // Share code/binaries into the warm container (zero-copy) from a named volume
    // or allowlisted host path. Read-only by default; policy is enforced here.
    for mount in &pool.mounts {
        enforce_mount_policy(
            &config.mount_source_allowlist,
            config.allow_writable_mounts,
            &pool.slug,
            mount,
        )?;
        let mode = if mount.read_only { "ro" } else { "rw" };
        args.push("--volume".to_string());
        args.push(format!("{}:{}:{}", mount.source, mount.target, mode));
    }

    if config.network == "host" {
        args.push("--network".to_string());
        args.push("host".to_string());
        args.push("--env".to_string());
        args.push(format!("PORT={host_port}"));
    } else {
        args.push("--network".to_string());
        args.push(config.network.clone());
        args.push("--publish".to_string());
        args.push(format!("127.0.0.1:{}:{}", host_port, pool.container_port));
        args.push("--env".to_string());
        args.push(format!("PORT={}", pool.container_port));
    }

    for (key, value) in container_env {
        args.push("--env".to_string());
        args.push(format!("{key}={value}"));
    }
    args.push(pool.image.clone());
    args.extend(pool.command.clone());
    Ok(args)
}

pub(crate) async fn start_one_for_pool(state: &AppState, pool_id: &str) -> Result<WarmContainer, String> {
    let (pool, mut container) = allocate_container_slot(state, pool_id).await?;

    let mut container_env = pool.env.clone();
    container_env
        .entry("DD_POOL_ID".to_string())
        .or_insert_with(|| pool.id.clone());
    container_env
        .entry("DD_POOL_SLUG".to_string())
        .or_insert_with(|| pool.slug.clone());
    container_env
        .entry("DD_POOL_CONTAINER_NAME".to_string())
        .or_insert_with(|| container.name.clone());
    container_env
        .entry("DD_POOL_MANAGER".to_string())
        .or_insert_with(|| SERVICE_NAME.to_string());
    container_env
        .entry("DD_POOL_REQUEST_PATH".to_string())
        .or_insert_with(|| pool.request_path.clone());
    container_env
        .entry("DD_POOL_HEALTH_PATH".to_string())
        .or_insert_with(|| pool.health_path.clone());
    container_env
        .entry("DD_POOL_CONTAINER_PORT".to_string())
        .or_insert_with(|| pool.container_port.to_string());
    container_env
        .entry("DD_POOL_MAX_BODY_BYTES".to_string())
        .or_insert_with(|| MAX_HTTP_BODY_BYTES.to_string());
    container_env
        .entry("DD_POOL_HANDLER_TIMEOUT_SECONDS".to_string())
        .or_insert_with(|| pool.request_timeout.as_secs().max(1).to_string());
    if let Some(nats_url) = state.config.nats_url.as_deref() {
        container_env
            .entry("NATS_URL".to_string())
            .or_insert_with(|| nats_url.to_string());
        container_env
            .entry("DD_POOL_NATS_EVENT_SUBJECT".to_string())
            .or_insert_with(|| container_pool_events_subject(&pool.slug));
        container_env
            .entry("DD_POOL_NATS_HEARTBEAT_SUBJECT".to_string())
            .or_insert_with(|| container_pool_heartbeats_subject(&pool.slug));
    }
    for key in &state.config.forward_env_keys {
        if container_env.contains_key(key) {
            continue;
        }
        if let Ok(value) = env::var(key) {
            if !value.is_empty() {
                container_env.insert(key.clone(), value);
            }
        }
    }

    let args = build_run_args(
        &state.config,
        &pool,
        &container.name,
        container.port,
        &container_env,
    )?;

    let container_run_timeout = state.config.nerdctl_run_timeout;
    let scrubbed_args = args
        .iter()
        .map(|arg| {
            // Match either env-name prefixes or value-bearing args whose
            // name reveals sensitivity. The substring checks below catch
            // any env name containing API_KEY/SECRET/DEPLOY_KEY/TOKEN
            // (covers AWS_SESSION_TOKEN, GITHUB_TOKEN, etc.) and the
            // explicit `AWS_` prefix scrubs AWS_ACCESS_KEY_ID, which
            // would otherwise slip past every substring rule.
            if arg.starts_with("GH_DEPLOY_KEY=")
                || arg.starts_with("SERVER_AUTH_SECRET=")
                || arg.starts_with("ANTHROPIC_API_KEY=")
                || arg.starts_with("OPENAI_API_KEY=")
                || arg.starts_with("CLAUDE_API_KEYS_JSON=")
                || arg.starts_with("OPENAI_API_KEYS_JSON=")
                || arg.starts_with("EVENT_INGEST_SECRET=")
                || arg.starts_with("GH_PAT=")
                || arg.starts_with("AWS_")
                || arg.contains("API_KEY")
                || arg.contains("SECRET")
                || arg.contains("DEPLOY_KEY")
                || arg.contains("TOKEN")
            {
                let prefix = arg.splitn(2, '=').next().unwrap_or("").to_string();
                format!("{prefix}=<redacted>")
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>();
    tracing::info!(
        "dd-container-pool {engine} run for {name}: {bin} {scrubbed_args:?}",
        engine = state.config.engine.label(),
        name = container.name,
        bin = state.config.engine_bin,
    );
    // Surface the *names* (not values) of the env keys that end up forwarded
    // into the warm worker. This makes silent-misconfig regressions obvious in
    // pod logs — e.g. when EVENT_INGEST_URL/EVENT_INGEST_SECRET are missing,
    // the dev-server's eventBus.startVercelIngest pipeline never starts and
    // task events never reach the websocket fanout.
    let mut env_keys: Vec<&str> = container_env.keys().map(String::as_str).collect();
    env_keys.sort_unstable();
    let event_ingest_url_present = container_env.contains_key("EVENT_INGEST_URL");
    let event_ingest_secret_present = container_env.contains_key("EVENT_INGEST_SECRET");
    let nats_url_present = container_env.contains_key("NATS_URL");
    let worker_fanout_secret_present = container_env.contains_key("WORKER_FANOUT_WS_SECRET")
        || container_env.contains_key("GLEAM_WORKER_WS_SECRET")
        || container_env.contains_key("GLEAM_BROADCAST_SECRET");
    tracing::info!(
        "dd-container-pool worker env for {name}: keys={env_keys:?} \
         event_ingest_url={event_ingest_url_present} \
         event_ingest_secret={event_ingest_secret_present} \
         nats_url={nats_url_present} \
         worker_fanout_secret={worker_fanout_secret_present}",
        name = container.name,
    );
    match run_command(&state.config.engine_bin, &args, container_run_timeout).await {
        Ok(output) => {
            let trimmed = output.trim();
            if !trimmed.is_empty() {
                tracing::debug!(
                    "dd-container-pool {engine} run -d output for {name}: {trimmed}",
                    engine = state.config.engine.label(),
                    name = container.name
                );
            }
            if let Err(error) = wait_container_ready(state, &pool, &container).await {
                let mut registry = state.registry.lock().await;
                registry.containers.remove(&container.name);
                remove_affinity_for_container(&mut registry, &container.name);
                drop(registry);
                if let Err(remove_error) = remove_container(state, &container.name).await {
                    tracing::error!(
                        "failed to remove unready warm container {}: {remove_error}",
                        container.name
                    );
                }
                return Err(error);
            }
            state
                .metrics
                .containers_started_total
                .fetch_add(1, Ordering::Relaxed);
            container.status = ContainerStatus::Idle;
            let mut registry = state.registry.lock().await;
            if let Some(stored) = registry.containers.get_mut(&container.name) {
                stored.status = ContainerStatus::Idle;
                stored.last_health_at_ms = Some(now_ms());
                stored.last_healthy_at_ms = Some(now_ms());
                stored.health_failure_count = 0;
                stored.last_health_error = None;
            }
            Ok(container)
        }
        Err(error) => {
            let mut registry = state.registry.lock().await;
            registry.containers.remove(&container.name);
            remove_affinity_for_container(&mut registry, &container.name);
            Err(error)
        }
    }
}

async fn wait_container_ready(
    state: &AppState,
    pool: &PoolConfig,
    container: &WarmContainer,
) -> Result<(), String> {
    let url = target_url(container, &pool.health_path);
    let started = tokio::time::Instant::now();
    loop {
        match timeout(Duration::from_millis(800), state.http.get(&url).send()).await {
            Ok(Ok(response)) if response.status().is_success() => return Ok(()),
            Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {}
        }
        if !inspect_container_running(state, &container.name).await? {
            return Err(format!(
                "container {} stopped before readiness at {url}",
                container.name
            ));
        }
        if started.elapsed() > state.config.container_start_timeout {
            return Err(format!(
                "container {} readiness timed out at {url}",
                container.name
            ));
        }
        sleep(Duration::from_millis(200)).await;
    }
}

async fn retire_stale_starting_containers(state: &AppState, pool_id: Option<&str>) {
    let now = now_ms();
    let candidates = {
        let registry = state.registry.lock().await;
        registry
            .containers
            .values()
            .filter(|container| container.in_flight == 0)
            .filter(|container| container.status == ContainerStatus::Starting)
            .filter(|container| pool_id.map(|id| id == container.pool_id).unwrap_or(true))
            .filter(|container| {
                Duration::from_millis(now.saturating_sub(container.launched_at_ms) as u64)
                    >= state.config.unhealthy_grace
            })
            .map(|container| container.name.clone())
            .collect::<Vec<_>>()
    };

    for name in candidates {
        match inspect_container_running(state, &name).await {
            Ok(false) => {
                retire_container(state, &name, "starting container is not running").await;
            }
            Ok(true) => {}
            Err(error) => {
                tracing::error!("failed to inspect starting warm container {name}: {error}");
            }
        }
    }
}

async fn probe_container_health(
    state: &AppState,
    pool: &PoolConfig,
    container: &WarmContainer,
) -> Result<(), String> {
    state
        .metrics
        .container_health_checks_total
        .fetch_add(1, Ordering::Relaxed);
    if !inspect_container_running(state, &container.name).await? {
        return Err("container is not running".to_string());
    }
    let url = target_url(container, &pool.health_path);
    match timeout(
        state.config.health_check_timeout,
        state.http.get(&url).send(),
    )
    .await
    {
        Ok(Ok(response)) if response.status().is_success() => Ok(()),
        Ok(Ok(response)) => Err(format!("health check returned {}", response.status())),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err(format!(
            "health check timed out after {}ms",
            duration_millis_u64(state.config.health_check_timeout)
        )),
    }
}

pub(crate) async fn retire_container(state: &AppState, name: &str, reason: &str) {
    let removed = {
        let mut registry = state.registry.lock().await;
        if let Some(container) = registry.containers.get_mut(name) {
            container.status = ContainerStatus::Unhealthy;
            container.last_health_at_ms = Some(now_ms());
            container.last_health_error = Some(reason.chars().take(512).collect());
        }
        let removed = registry.containers.remove(name).is_some();
        if removed {
            remove_affinity_for_container(&mut registry, name);
        }
        removed
    };
    if removed {
        state
            .metrics
            .containers_unhealthy_total
            .fetch_add(1, Ordering::Relaxed);
        if let Err(error) = remove_container(state, name).await {
            tracing::error!("failed to remove unhealthy warm container {name}: {error}");
        }
    }
}

async fn prune_unhealthy_containers(state: &AppState) {
    retire_stale_starting_containers(state, None).await;

    let now = now_ms();
    let candidates = {
        let registry = state.registry.lock().await;
        registry
            .containers
            .values()
            .filter(|container| container.in_flight == 0)
            .filter(|container| {
                !matches!(
                    container.status,
                    ContainerStatus::Starting | ContainerStatus::Draining
                )
            })
            .filter(|container| {
                container
                    .last_health_at_ms
                    .map(|last| {
                        Duration::from_millis(now.saturating_sub(last) as u64)
                            >= state.config.health_check_interval
                    })
                    .unwrap_or(true)
            })
            .filter_map(|container| {
                registry
                    .configs
                    .get(&container.pool_id)
                    .cloned()
                    .map(|pool| (pool, container.clone()))
            })
            .collect::<Vec<_>>()
    };

    for (pool, container) in candidates {
        let checked_at = now_ms();
        match probe_container_health(state, &pool, &container).await {
            Ok(()) => {
                let mut registry = state.registry.lock().await;
                if let Some(stored) = registry.containers.get_mut(&container.name) {
                    stored.status = ContainerStatus::Idle;
                    stored.last_health_at_ms = Some(checked_at);
                    stored.last_healthy_at_ms = Some(checked_at);
                    stored.health_failure_count = 0;
                    stored.last_health_error = None;
                }
            }
            Err(error) => {
                state
                    .metrics
                    .container_health_check_failures_total
                    .fetch_add(1, Ordering::Relaxed);
                let should_retire = {
                    let mut registry = state.registry.lock().await;
                    let Some(stored) = registry.containers.get_mut(&container.name) else {
                        continue;
                    };
                    stored.last_health_at_ms = Some(checked_at);
                    stored.health_failure_count = stored.health_failure_count.saturating_add(1);
                    stored.last_health_error = Some(error.chars().take(512).collect());
                    stored.status = ContainerStatus::Unhealthy;
                    let age = Duration::from_millis(
                        checked_at.saturating_sub(stored.launched_at_ms) as u64,
                    );
                    stored.in_flight == 0
                        && age >= state.config.unhealthy_grace
                        && stored.health_failure_count >= state.config.unhealthy_failure_threshold
                };
                if should_retire {
                    retire_container(state, &container.name, "health check failed").await;
                }
            }
        }
    }
}

pub(crate) async fn reconcile_pool(state: &AppState, pool_id: &str) -> Result<(), String> {
    loop {
        let deficit = {
            let registry = state.registry.lock().await;
            let Some(pool) = registry.configs.get(pool_id) else {
                return Ok(());
            };
            let active = registry
                .containers
                .values()
                .filter(|container| container.pool_id == pool.id)
                .count();
            let available_capacity = registry
                .containers
                .values()
                .filter(|container| container.pool_id == pool.id)
                .filter(|container| {
                    !matches!(
                        container.status,
                        ContainerStatus::Starting
                            | ContainerStatus::Draining
                            | ContainerStatus::Unhealthy
                    )
                })
                .map(|container| {
                    pool.max_concurrency_per_container
                        .saturating_sub(container.in_flight)
                })
                .sum::<usize>();
            let capacity_deficit = pool.min_warm.saturating_sub(available_capacity);
            capacity_deficit.min(pool.max_warm.saturating_sub(active))
        };
        if deficit == 0 {
            break;
        }
        start_one_for_pool(state, pool_id).await?;
    }
    Ok(())
}

pub(crate) async fn reconcile_all(state: &AppState) {
    prune_unhealthy_containers(state).await;

    let pool_ids = {
        let registry = state.registry.lock().await;
        registry.configs.keys().cloned().collect::<Vec<_>>()
    };
    for pool_id in pool_ids {
        if let Err(error) = reconcile_pool(state, &pool_id).await {
            tracing::error!("container pool reconcile failed for {pool_id}: {error}");
        }
    }

    let stale = {
        let mut registry = state.registry.lock().await;
        let mut per_pool_count = HashMap::<String, usize>::new();
        for container in registry.containers.values() {
            *per_pool_count.entry(container.pool_id.clone()).or_default() += 1;
        }
        let now = now_ms();
        let mut stale = Vec::new();
        let mut names = registry.containers.keys().cloned().collect::<Vec<_>>();
        names.sort();
        for name in names {
            let Some(container) = registry.containers.get(&name) else {
                continue;
            };
            if container.status == ContainerStatus::Busy || container.in_flight > 0 {
                continue;
            }
            let Some(pool) = registry.configs.get(&container.pool_id) else {
                stale.push(name.clone());
                continue;
            };
            let count = per_pool_count.get(&container.pool_id).copied().unwrap_or(0);
            let idle_for = Duration::from_millis((now - container.last_used_at_ms) as u64);
            if count > pool.max_warm || (count > pool.min_warm && idle_for > pool.idle_ttl) {
                stale.push(name.clone());
                if let Some(value) = per_pool_count.get_mut(&container.pool_id) {
                    *value = value.saturating_sub(1);
                }
            }
        }
        for name in &stale {
            if let Some(container) = registry.containers.get_mut(name) {
                container.status = ContainerStatus::Draining;
            }
            registry.containers.remove(name);
            remove_affinity_for_container(&mut registry, name);
        }
        stale
    };
    for name in stale {
        if let Err(error) = remove_container(state, &name).await {
            tracing::error!("failed to remove stale warm container {name}: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    use crate::{config::safe_oci_runtime, pool_config::pool_config_from_json, types::EngineKind};

    fn test_service_config() -> ServiceConfig {
        ServiceConfig {
            engine: EngineKind::Nerdctl,
            engine_bin: "/usr/local/bin/nerdctl".to_string(),
            oci_runtime: None,
            containerd_namespace: "k8s.io".to_string(),
            network: "host".to_string(),
            pull_policy: Some("never".to_string()),
            database_url: None,
            app_config_key: "container-pool.runtime-pools.v1".to_string(),
            app_config_scope: "default".to_string(),
            nats_url: None,
            nats_subject: "dd.remote.container_pool.requests".to_string(),
            nats_queue_group: "dd-container-pool".to_string(),
            nats_result_subject: "dd.remote.container_pool.results".to_string(),
            nats_max_payload_bytes: 1024,
            redis_url: None,
            redis_lock_prefix: "lock".to_string(),
            redis_lock_ttl: Duration::from_secs(1),
            redis_lock_wait_timeout: Duration::from_secs(1),
            redis_lock_retry_delay: Duration::from_millis(10),
            redis_lock_request_timeout: Duration::from_secs(1),
            worker_response_max_bytes: 1024,
            config_refresh: Duration::from_secs(1),
            reconcile_interval: Duration::from_secs(1),
            command_timeout: Duration::from_secs(1),
            nerdctl_run_timeout: Duration::from_secs(1),
            container_start_timeout: Duration::from_secs(1),
            health_check_interval: Duration::from_secs(1),
            health_check_timeout: Duration::from_millis(100),
            unhealthy_grace: Duration::from_secs(1),
            unhealthy_failure_threshold: 2,
            port_start: 12_000,
            port_end: 12_999,
            cleanup_on_start: false,
            server_auth_secret: None,
            container_memory: Some("256m".to_string()),
            container_cpus: Some("0.50".to_string()),
            forward_env_keys: Vec::new(),
            pids_limit: 4096,
            nofile_limit: 65536,
            cap_drop_all: true,
            no_new_privileges: true,
            mount_source_allowlist: Vec::new(),
            allow_writable_mounts: false,
        }
    }

    fn code_pool(slug: &str, image: &str, command: &[&str]) -> PoolConfig {
        let value = json!({
            "slug": slug,
            "image": image,
            "mounts": [{ "source": "dd-code", "target": "/opt/code", "readOnly": true }],
            "command": command,
        });
        pool_config_from_json(&value).expect("valid pool config")
    }

    fn contains_pair(args: &[String], a: &str, b: &str) -> bool {
        args.windows(2).any(|w| w[0] == a && w[1] == b)
    }

    fn tail_is(args: &[String], tail: &[&str]) -> bool {
        args.len() >= tail.len()
            && args[args.len() - tail.len()..]
                .iter()
                .zip(tail)
                .all(|(got, want)| got == want)
    }

    #[test]
    fn shared_volume_code_runs_eight_runtimes_zero_copy() {
        let config = test_service_config();
        // Code (not data) is shared read-only from one volume; the image only
        // supplies the runtime/libc. Covers multi-file trees (erlang ebin, java
        // classpath, node/python/ruby/bash sources) and single compiled binaries
        // (go, rust) — all zero-copy, no per-function image build.
        let runtimes: [(&str, &str, &[&str]); 8] = [
            (
                "nodejs-fn",
                "docker.io/library/dd-cp-nodejs:dev",
                &["node", "/opt/code/server.mjs"],
            ),
            (
                "python-fn",
                "docker.io/library/dd-cp-python3:dev",
                &["python3", "/opt/code/app.py"],
            ),
            (
                "ruby-fn",
                "docker.io/library/dd-cp-ruby:dev",
                &["ruby", "/opt/code/app.rb"],
            ),
            (
                "bash-fn",
                "docker.io/library/dd-cp-bash:dev",
                &["bash", "/opt/code/run.sh"],
            ),
            (
                "erlang-fn",
                "docker.io/library/dd-cp-erlang:dev",
                &[
                    "erl",
                    "-noshell",
                    "-pa",
                    "/opt/code/ebin",
                    "-s",
                    "myapp",
                    "start",
                ],
            ),
            (
                "golang-fn",
                "docker.io/library/dd-cp-golang:dev",
                &["/opt/code/bin/server"],
            ),
            (
                "rust-fn",
                "docker.io/library/dd-cp-rust:dev",
                &["/opt/code/bin/svc"],
            ),
            (
                "java-fn",
                "docker.io/library/dd-cp-java:dev",
                &["java", "-cp", "/opt/code/classes", "Main"],
            ),
        ];
        for (slug, image, command) in runtimes {
            let pool = code_pool(slug, image, command);
            let args = build_run_args(&config, &pool, "c1", 12_345, &BTreeMap::new())
                .unwrap_or_else(|error| panic!("{slug}: {error}"));
            assert!(
                contains_pair(&args, "--volume", "dd-code:/opt/code:ro"),
                "{slug} shared-volume mount missing: {args:?}"
            );
            let mut tail = vec![image];
            tail.extend_from_slice(command);
            assert!(
                tail_is(&args, &tail),
                "{slug} image+command tail wrong: {args:?}"
            );
            // Hardening still applies uniformly across runtimes.
            assert!(contains_pair(&args, "--cap-drop", "ALL"), "{slug} cap-drop");
            assert!(
                args.iter().any(|arg| arg == "--read-only"),
                "{slug} read-only"
            );
        }
    }

    #[test]
    fn engine_kind_controls_namespace_flag() {
        let pool = code_pool(
            "nodejs-fn",
            "docker.io/library/x:dev",
            &["node", "/opt/code/s.mjs"],
        );

        let nerd = test_service_config();
        let args = build_run_args(&nerd, &pool, "c1", 1, &BTreeMap::new()).unwrap();
        assert_eq!(
            &args[0..4],
            &["-n", "k8s.io", "run", "-d"],
            "nerdctl scopes to a namespace"
        );

        for engine in [EngineKind::Docker, EngineKind::Podman] {
            let mut config = test_service_config();
            config.engine = engine;
            let args = build_run_args(&config, &pool, "c1", 1, &BTreeMap::new()).unwrap();
            assert_eq!(&args[0..2], &["run", "-d"], "{engine:?} omits namespace");
            assert!(!args.iter().any(|arg| arg == "-n"), "{engine:?} has no -n");
        }
    }

    #[test]
    fn oci_runtime_passthrough_and_validation() {
        let pool = code_pool("svc", "docker.io/library/x:dev", &["/opt/code/bin/app"]);
        // runc/crun and the containerd Kata/gVisor handlers all flow through
        // --runtime under any engine.
        for runtime in [
            "runc",
            "crun",
            "runsc",
            "io.containerd.kata.v2",
            "io.containerd.runsc.v1",
        ] {
            let mut config = test_service_config();
            config.engine = EngineKind::Docker;
            config.oci_runtime = Some(runtime.to_string());
            let args = build_run_args(&config, &pool, "c1", 1, &BTreeMap::new()).unwrap();
            assert!(
                contains_pair(&args, "--runtime", runtime),
                "{runtime}: {args:?}"
            );
        }
        let config = test_service_config();
        let args = build_run_args(&config, &pool, "c1", 1, &BTreeMap::new()).unwrap();
        assert!(
            !args.iter().any(|arg| arg == "--runtime"),
            "no --runtime when unset"
        );

        assert!(safe_oci_runtime("crun"));
        assert!(safe_oci_runtime("io.containerd.kata.v2"));
        assert!(safe_oci_runtime("/usr/local/bin/crun"));
        assert!(!safe_oci_runtime("runc; rm -rf /"));
        assert!(!safe_oci_runtime("two words"));
        assert!(!safe_oci_runtime(""));
    }

    #[test]
    fn mounted_code_pools_are_confined_even_when_service_defaults_are_off() {
        // Service leaves the strict flags off (the current default).
        let mut config = test_service_config();
        config.cap_drop_all = false;
        config.no_new_privileges = false;

        // A pool that mounts external code must still be confined.
        let pool = code_pool("svc", "docker.io/library/x:dev", &["/opt/code/app"]);
        let args = build_run_args(&config, &pool, "c1", 1, &BTreeMap::new()).unwrap();
        assert!(
            contains_pair(&args, "--cap-drop", "ALL"),
            "mounted pool must drop caps: {args:?}"
        );
        assert!(
            contains_pair(&args, "--security-opt", "no-new-privileges"),
            "mounted pool must set no-new-privileges: {args:?}"
        );
    }

    #[test]
    fn unconfined_pool_opts_out_of_mount_hardening() {
        let mut config = test_service_config();
        config.cap_drop_all = false;
        config.no_new_privileges = false;

        let pool = pool_config_from_json(&json!({
            "slug": "svc",
            "image": "docker.io/library/x:dev",
            "mounts": [{ "source": "dd-code", "target": "/opt/code" }],
            "unconfined": true,
            "command": ["/opt/code/app"],
        }))
        .unwrap();
        let args = build_run_args(&config, &pool, "c1", 1, &BTreeMap::new()).unwrap();
        assert!(
            !args.iter().any(|a| a == "--cap-drop"),
            "unconfined opts out of cap-drop: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "--security-opt"),
            "unconfined opts out of no-new-privileges: {args:?}"
        );
    }

    #[test]
    fn mountless_pools_follow_service_security_defaults() {
        let mountless = pool_config_from_json(&json!({
            "slug": "svc",
            "image": "docker.io/library/x:dev",
            "command": ["/app"],
        }))
        .unwrap();

        // Service flags off => no strict flags (unchanged prior behavior).
        let mut off = test_service_config();
        off.cap_drop_all = false;
        off.no_new_privileges = false;
        let args = build_run_args(&off, &mountless, "c1", 1, &BTreeMap::new()).unwrap();
        assert!(!args.iter().any(|a| a == "--cap-drop"));
        assert!(!args.iter().any(|a| a == "--security-opt"));

        // Service flags on => strict flags, exactly as before.
        let args = build_run_args(
            &test_service_config(),
            &mountless,
            "c1",
            1,
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(contains_pair(&args, "--cap-drop", "ALL"));
        assert!(contains_pair(&args, "--security-opt", "no-new-privileges"));
    }

    #[test]
    fn bridge_network_publishes_host_port() {
        let pool = code_pool("svc", "docker.io/library/x:dev", &["/opt/code/app"]);
        let mut config = test_service_config();
        config.network = "bridge".to_string();
        let args = build_run_args(&config, &pool, "c1", 23_456, &BTreeMap::new()).unwrap();
        assert!(contains_pair(&args, "--network", "bridge"));
        assert!(contains_pair(&args, "--publish", "127.0.0.1:23456:8080"));
    }
}
