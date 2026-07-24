use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::BufReader,
    sync::atomic::Ordering,
    time::Duration,
};

use serde_json::{json, Value};

use crate::{
    dispatch::remove_affinity_for_container,
    engine::remove_container,
    types::{AppState, Mount, PoolConfig, ServiceConfig},
    util::{
        clamp_i32_to_usize, command_vec_from_json, first_env, json_bool_field,
        json_string_field, json_u64_field, now_ms, safe_config_id, safe_container_image,
        safe_env_key, safe_env_value, safe_local_path, safe_nats_subject, safe_slug,
    },
};

pub(crate) fn pool_config_from_json(value: &Value) -> Result<PoolConfig, String> {
    let slug = json_string_field(value, "slug", "slug")
        .ok_or_else(|| "container pool config is missing slug".to_string())?;
    if !safe_slug(&slug) {
        return Err(format!("invalid app_config container pool slug: {slug}"));
    }
    let image = json_string_field(value, "image", "image")
        .ok_or_else(|| format!("container pool {slug} is missing image"))?;
    if !safe_container_image(&image) {
        return Err(format!("container pool {slug} has invalid image"));
    }
    let min_warm = json_u64_field(value, "minWarm", "min_warm")
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(1)
        .min(64);
    let max_warm = json_u64_field(value, "maxWarm", "max_warm")
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(min_warm.max(1))
        .max(min_warm)
        .min(128);
    let request_path = json_string_field(value, "requestPath", "request_path")
        .filter(|path| safe_local_path(path))
        .unwrap_or_else(|| "/invoke".to_string());
    let health_path = json_string_field(value, "healthPath", "health_path")
        .filter(|path| safe_local_path(path))
        .unwrap_or_else(|| "/healthz".to_string());
    let id = json_string_field(value, "id", "id").unwrap_or_else(|| slug.clone());
    if !safe_config_id(&id) {
        return Err(format!("container pool {slug} has invalid id"));
    }
    let nats_subject = json_string_field(value, "natsSubject", "nats_subject");
    if let Some(subject) = nats_subject.as_deref() {
        if !safe_nats_subject(subject) {
            return Err(format!("container pool {slug} has invalid nats_subject"));
        }
    }
    Ok(PoolConfig {
        id,
        slug: slug.clone(),
        display_name: json_string_field(value, "displayName", "display_name")
            .unwrap_or_else(|| slug.clone()),
        image,
        command: command_vec_from_json(value.get("command").cloned().unwrap_or_else(|| json!([]))),
        env: env_map_from_json(value.get("env").cloned().unwrap_or_else(|| json!({}))),
        request_path,
        health_path,
        container_port: json_u64_field(value, "containerPort", "container_port")
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(8080),
        min_warm,
        max_warm,
        max_concurrency_per_container: json_u64_field(
            value,
            "maxConcurrencyPerContainer",
            "max_concurrency_per_container",
        )
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(1)
        .clamp(1, 128),
        request_timeout: Duration::from_millis(
            json_u64_field(value, "requestTimeoutMs", "request_timeout_ms")
                .unwrap_or(30_000)
                .clamp(100, 900_000),
        ),
        idle_ttl: Duration::from_secs(
            json_u64_field(value, "idleTtlSeconds", "idle_ttl_seconds")
                .unwrap_or(900)
                .clamp(10, 86_400),
        ),
        nats_subject,
        read_only: json_bool_field(value, "readOnly", "read_only").unwrap_or(true),
        user: json_string_field(value, "user", "user")
            .filter(|value| {
                value.len() <= 64
                    && value
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '_' | '-'))
            })
            .unwrap_or_else(|| "10001:10001".to_string()),
        labels: value.get("labels").cloned().unwrap_or_else(|| json!([])),
        mounts: mounts_from_json(value, &slug)?,
        unconfined: json_bool_field(value, "unconfined", "unconfined").unwrap_or(false),
    })
}

fn pool_configs_from_app_config_value(value: Value) -> Result<Vec<PoolConfig>, String> {
    let pools = value
        .get("pools")
        .and_then(Value::as_array)
        .ok_or_else(|| "container pool app_config value must contain a pools array".to_string())?;
    pools.iter().map(pool_config_from_json).collect()
}

fn env_map_from_json(value: Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| {
                    if !safe_env_key(key) {
                        return None;
                    }
                    let value = value
                        .as_str()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| value.to_string());
                    safe_env_value(&value).then(|| (key.to_string(), value))
                })
                .collect()
        })
        .unwrap_or_default()
}

// Parse the optional `mounts` (alias `volumes`) array from a pool config. Each
// entry is `{source|volume, target|mountPath, readOnly?}`. Shape is validated
// here; host-path/writable *policy* is enforced at container start where the
// service config is available (`enforce_mount_policy`).
fn mounts_from_json(value: &Value, slug: &str) -> Result<Vec<Mount>, String> {
    let Some(items) = value.get("mounts").or_else(|| value.get("volumes")) else {
        return Ok(Vec::new());
    };
    if items.is_null() {
        return Ok(Vec::new());
    }
    let array = items
        .as_array()
        .ok_or_else(|| format!("container pool {slug} mounts must be an array"))?;
    if array.len() > 16 {
        return Err(format!(
            "container pool {slug} has too many mounts (max 16)"
        ));
    }
    let mut mounts = Vec::with_capacity(array.len());
    for item in array {
        let source = json_string_field(item, "source", "source")
            .or_else(|| json_string_field(item, "volume", "volume"))
            .ok_or_else(|| format!("container pool {slug} mount is missing source"))?;
        let target = json_string_field(item, "target", "target")
            .or_else(|| json_string_field(item, "mountPath", "mount_path"))
            .ok_or_else(|| format!("container pool {slug} mount is missing target"))?;
        if !safe_mount_source(&source) {
            return Err(format!(
                "container pool {slug} has invalid mount source: {source}"
            ));
        }
        if !safe_mount_target(&target) {
            return Err(format!(
                "container pool {slug} has invalid mount target: {target}"
            ));
        }
        if is_reserved_mount_target(&target) {
            return Err(format!(
                "container pool {slug} mount target {target} is reserved"
            ));
        }
        if mounts
            .iter()
            .any(|existing: &Mount| existing.target == target)
        {
            return Err(format!(
                "container pool {slug} has duplicate mount target {target}"
            ));
        }
        // Shared code/binaries default to read-only; writable needs an explicit
        // opt-in here and a service-level enable at start.
        let read_only = json_bool_field(item, "readOnly", "read_only").unwrap_or(true);
        mounts.push(Mount {
            source,
            target,
            read_only,
        });
    }
    Ok(mounts)
}

// A mount source is either a nerdctl/docker named volume or an absolute host
// path. ':' and ',' are excluded so the `-v src:dst:mode` argv element stays
// unambiguous; control chars / whitespace / backslash are rejected too.
fn safe_mount_source(input: &str) -> bool {
    if input.is_empty() || input.len() > 256 {
        return false;
    }
    if input
        .bytes()
        .any(|byte| byte <= 0x20 || byte == 0x7f || matches!(byte, b':' | b',' | b'\\'))
    {
        return false;
    }
    if input.starts_with('/') {
        // Require a canonical absolute path (no `//`) so allowlist prefix
        // matching is exact.
        safe_local_path(input) && !input.contains("//")
    } else {
        let bytes = input.as_bytes();
        bytes[0].is_ascii_alphanumeric()
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    }
}

fn safe_mount_target(input: &str) -> bool {
    safe_local_path(input) && !input.contains("//") && !input.contains(':') && !input.contains(',')
}

// Refuse to overmount the container root or the kernel pseudo-filesystems:
// these are never legitimate code-mount points and overmounting them can
// subvert isolation/observability inside the container.
fn is_reserved_mount_target(target: &str) -> bool {
    target == "/"
        || ["/proc", "/sys", "/dev"]
            .iter()
            .any(|reserved| path_has_prefix(target, reserved))
}

// Enforced at container start (has the service config). Named volumes are always
// allowed; absolute host paths must sit under an allowlisted prefix; writable
// mounts require the global opt-in. Fails closed with a clear operator message.
pub(crate) fn enforce_mount_policy(
    allowlist: &[String],
    allow_writable: bool,
    slug: &str,
    mount: &Mount,
) -> Result<(), String> {
    if !mount.read_only && !allow_writable {
        return Err(format!(
            "container pool {slug} requests writable mount {}; set \
             CONTAINER_POOL_ALLOW_WRITABLE_MOUNTS=true to permit",
            mount.target
        ));
    }
    if mount.source.starts_with('/') {
        let allowed = allowlist
            .iter()
            .any(|prefix| path_has_prefix(&mount.source, prefix));
        if !allowed {
            return Err(format!(
                "container pool {slug} host-path mount {} is not under any \
                 CONTAINER_POOL_MOUNT_SOURCE_ALLOWLIST prefix",
                mount.source
            ));
        }
    }
    Ok(())
}

// Prefix match on a path boundary so `/data` does not authorize `/database`.
fn path_has_prefix(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    path == prefix || path.starts_with(&format!("{prefix}/"))
}

fn row_string(row: &tokio_postgres::Row, column: &str) -> String {
    row.try_get::<_, String>(column).unwrap_or_default()
}

fn row_opt_string(row: &tokio_postgres::Row, column: &str) -> Option<String> {
    row.try_get::<_, Option<String>>(column)
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty())
}

fn row_i32(row: &tokio_postgres::Row, column: &str, fallback: i32) -> i32 {
    row.try_get::<_, i32>(column).unwrap_or(fallback)
}

fn row_value(row: &tokio_postgres::Row, column: &str, fallback: Value) -> Value {
    row.try_get::<_, Value>(column).unwrap_or(fallback)
}

fn add_rds_root_certificates(root_store: &mut rustls::RootCertStore) -> Result<(), String> {
    let mut reader = BufReader::new(&include_bytes!("../certs/rds-us-east-1-bundle.pem")[..]);
    let mut added = 0usize;

    for cert in rustls_pemfile::certs(&mut reader) {
        let cert = cert.map_err(|error| format!("failed to parse RDS CA certificate: {error}"))?;
        if root_store.add(cert).is_ok() {
            added += 1;
        }
    }

    if added == 0 {
        return Err("no RDS CA certificates loaded".to_string());
    }

    Ok(())
}

fn row_to_pool_config(row: &tokio_postgres::Row) -> Result<PoolConfig, String> {
    let id = row_string(row, "id");
    let slug = row_string(row, "slug");
    if !safe_slug(&slug) {
        return Err(format!("invalid container pool slug: {slug}"));
    }
    let image = row_string(row, "image");
    if !safe_container_image(&image) {
        return Err(format!("container pool {slug} has invalid image"));
    }
    if !safe_config_id(&id) {
        return Err(format!("container pool {slug} has invalid id"));
    }

    let min_warm = clamp_i32_to_usize(row_i32(row, "min_warm", 1), 1, 0, 64);
    let max_warm =
        clamp_i32_to_usize(row_i32(row, "max_warm", 1), min_warm.max(1), 1, 128).max(min_warm);
    let request_timeout_ms = row_i32(row, "request_timeout_ms", 30_000).clamp(100, 900_000);
    let idle_ttl_seconds = row_i32(row, "idle_ttl_seconds", 900).clamp(10, 86_400);
    let container_port = row_i32(row, "container_port", 8080).clamp(1, u16::MAX as i32) as u16;
    let max_concurrency_per_container =
        clamp_i32_to_usize(row_i32(row, "max_concurrency_per_container", 1), 1, 1, 128);
    let request_path = row_opt_string(row, "request_path").unwrap_or_else(|| "/invoke".to_string());
    if !safe_local_path(&request_path) {
        return Err(format!("container pool {slug} has invalid request_path"));
    }
    let health_path = row_opt_string(row, "health_path").unwrap_or_else(|| "/healthz".to_string());
    if !safe_local_path(&health_path) {
        return Err(format!("container pool {slug} has invalid health_path"));
    }

    let nats_subject = row_opt_string(row, "nats_subject");
    if let Some(subject) = nats_subject.as_deref() {
        if !safe_nats_subject(subject) {
            return Err(format!("container pool {slug} has invalid nats_subject"));
        }
    }

    // `mounts` column is optional; row_value falls back to `[]` if absent (no
    // migration required for the fallback table).
    let mounts = mounts_from_json(&row_value(row, "mounts", json!([])), &slug)?;

    Ok(PoolConfig {
        id,
        slug,
        display_name: row_opt_string(row, "display_name").unwrap_or_else(|| image.clone()),
        image,
        command: command_vec_from_json(row_value(row, "command", json!([]))),
        env: env_map_from_json(row_value(row, "env", json!({}))),
        request_path,
        health_path,
        container_port,
        min_warm,
        max_warm,
        max_concurrency_per_container,
        request_timeout: Duration::from_millis(request_timeout_ms as u64),
        idle_ttl: Duration::from_secs(idle_ttl_seconds as u64),
        nats_subject,
        read_only: true,
        user: "10001:10001".to_string(),
        labels: row_value(row, "labels", json!([])),
        mounts,
        unconfined: false,
    })
}

async fn connect_postgres(config: &ServiceConfig) -> Result<tokio_postgres::Client, String> {
    let database_url = config
        .database_url
        .as_deref()
        .ok_or_else(|| "container pool database URL is not configured".to_string())?;
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    add_rds_root_certificates(&mut root_store)?;
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
    let (client, connection) = tokio_postgres::connect(database_url, tls)
        .await
        .map_err(|error| error.to_string())?;

    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::error!("container pool postgres connection error: {error}");
        }
    });
    Ok(client)
}

async fn fetch_pool_configs_from_postgres(
    config: &ServiceConfig,
) -> Result<Vec<PoolConfig>, String> {
    let client = connect_postgres(config).await?;
    match fetch_pool_configs_from_app_config(&client, config).await {
        Ok(Some(configs)) => return Ok(configs),
        Ok(None) => {}
        Err(error) => {
            tracing::error!(
                "container pool app_config lookup failed, falling back to container_pool_configs: {error}"
            );
        }
    }
    fetch_pool_configs_from_table(&client).await
}

async fn fetch_pool_configs_from_app_config(
    client: &tokio_postgres::Client,
    config: &ServiceConfig,
) -> Result<Option<Vec<PoolConfig>>, String> {
    let rows = client
        .query(
            r#"
            select value
            from app_config
            where scope = $1
              and key = $2
              and status = 'active'
              and is_soft_deleted = false
            order by updated_at desc
            limit 1
            "#,
            &[&config.app_config_scope, &config.app_config_key],
        )
        .await
        .map_err(|error| error.to_string())?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let value = row_value(row, "value", json!({}));
    let configs = pool_configs_from_app_config_value(value)?;
    Ok(Some(configs))
}

async fn fetch_pool_configs_from_table(
    client: &tokio_postgres::Client,
) -> Result<Vec<PoolConfig>, String> {
    let rows = client
        .query(
            r#"
            select
              id::text as id,
              slug,
              display_name,
              image,
              command,
              env,
              request_path,
              health_path,
              container_port,
              min_warm,
              max_warm,
              max_concurrency_per_container,
              request_timeout_ms,
              idle_ttl_seconds,
              nats_subject,
              labels
            from container_pool_configs
            where status = 'active'
              and is_soft_deleted = false
            order by slug asc
            "#,
            &[],
        )
        .await
        .map_err(|error| error.to_string())?;

    let mut configs = Vec::with_capacity(rows.len());
    for row in rows {
        configs.push(row_to_pool_config(&row)?);
    }
    Ok(configs)
}

fn fallback_pool_configs_from_env() -> Result<Vec<PoolConfig>, String> {
    let Some(raw) = first_env(&["CONTAINER_POOL_CONFIG_JSON"]) else {
        return Ok(Vec::new());
    };
    let value = serde_json::from_str::<Value>(&raw).map_err(|error| error.to_string())?;
    let items = value
        .as_array()
        .ok_or_else(|| "CONTAINER_POOL_CONFIG_JSON must be a JSON array".to_string())?;
    let mut configs = Vec::with_capacity(items.len());
    for item in items {
        configs.push(pool_config_from_json(item)?);
    }
    Ok(configs)
}

async fn fetch_pool_configs(config: &ServiceConfig) -> Result<Vec<PoolConfig>, String> {
    if config.database_url.is_some() {
        fetch_pool_configs_from_postgres(config).await
    } else {
        fallback_pool_configs_from_env()
    }
}

pub(crate) async fn refresh_pool_configs(state: &AppState) -> Result<(), String> {
    let configs = fetch_pool_configs(&state.config).await?;
    let mut next_configs = HashMap::new();
    let mut next_slugs = HashMap::new();
    for config in configs {
        next_slugs.insert(config.slug.clone(), config.id.clone());
        next_configs.insert(config.id.clone(), config);
    }

    let removed_names = {
        let mut registry = state.registry.lock().await;
        let removed_pool_ids = registry
            .configs
            .keys()
            .filter(|pool_id| !next_configs.contains_key(*pool_id))
            .cloned()
            .collect::<HashSet<_>>();
        let removed_names = registry
            .containers
            .values()
            .filter(|container| removed_pool_ids.contains(&container.pool_id))
            .map(|container| container.name.clone())
            .collect::<Vec<_>>();
        for name in &removed_names {
            registry.containers.remove(name);
            remove_affinity_for_container(&mut registry, name);
        }
        registry.configs = next_configs;
        registry.slug_to_id = next_slugs;
        if registry.next_port == 0 {
            registry.next_port = state.config.port_start;
        }
        registry.last_config_error = None;
        registry.last_config_refresh_ms = Some(now_ms());
        removed_names
    };

    state
        .metrics
        .config_refresh_total
        .fetch_add(1, Ordering::Relaxed);
    for name in removed_names {
        if let Err(error) = remove_container(state, &name).await {
            tracing::error!("failed to remove container for deleted pool {name}: {error}");
        }
    }
    Ok(())
}

pub(crate) async fn record_config_error(state: &AppState, error: String) {
    state
        .metrics
        .config_refresh_failures_total
        .fetch_add(1, Ordering::Relaxed);
    let mut registry = state.registry.lock().await;
    registry.last_config_error = Some(error);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_source_accepts_named_volumes_and_rejects_path_smuggling() {
        assert!(safe_mount_source("dd-code"));
        assert!(safe_mount_source("dd_code.v1-2"));
        assert!(safe_mount_source("/srv/lambda-bin"));
        // Path-classification escapes and `-v` delimiter smuggling.
        assert!(!safe_mount_source("../etc"));
        assert!(!safe_mount_source("./code"));
        assert!(!safe_mount_source("/srv/../etc"));
        assert!(!safe_mount_source("/srv//code"));
        assert!(!safe_mount_source("vol:/etc"));
        assert!(!safe_mount_source("vol,extra"));
        assert!(!safe_mount_source("bad name"));
        assert!(!safe_mount_source(""));
    }

    #[test]
    fn mount_target_must_be_safe_and_unreserved() {
        assert!(safe_mount_target("/opt/code"));
        assert!(!safe_mount_target("/opt/../etc"));
        assert!(!safe_mount_target("relative"));
        assert!(!safe_mount_target("/opt:/code"));
        assert!(is_reserved_mount_target("/"));
        assert!(is_reserved_mount_target("/proc"));
        assert!(is_reserved_mount_target("/proc/sys"));
        assert!(is_reserved_mount_target("/dev/shm"));
        assert!(!is_reserved_mount_target("/opt/code"));
        // Boundary: /devices is not under /dev.
        assert!(!is_reserved_mount_target("/devices"));
    }

    #[test]
    fn path_prefix_matches_on_boundary() {
        assert!(path_has_prefix("/srv/code", "/srv/code"));
        assert!(path_has_prefix("/srv/code/bin", "/srv/code/"));
        assert!(!path_has_prefix("/srv/codex", "/srv/code"));
        assert!(!path_has_prefix("/srv", "/srv/code"));
    }

    #[test]
    fn mounts_from_json_parses_validates_and_defaults_read_only() {
        let value = json!({
            "mounts": [
                { "source": "dd-code", "target": "/opt/code" },
                { "volume": "dd-bin", "mountPath": "/opt/bin", "readOnly": false }
            ]
        });
        let mounts = mounts_from_json(&value, "svc").expect("valid mounts");
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0].target, "/opt/code");
        assert!(mounts[0].read_only, "defaults to read-only");
        assert!(!mounts[1].read_only);

        assert!(mounts_from_json(&json!({}), "svc").unwrap().is_empty());
        assert!(mounts_from_json(&json!({ "mounts": null }), "svc")
            .unwrap()
            .is_empty());
        assert!(mounts_from_json(&json!({ "mounts": "x" }), "svc").is_err());
        assert!(
            mounts_from_json(
                &json!({ "mounts": [
                    { "source": "a", "target": "/opt/code" },
                    { "source": "b", "target": "/opt/code" }
                ] }),
                "svc"
            )
            .is_err(),
            "duplicate target must be rejected"
        );
        assert!(
            mounts_from_json(
                &json!({ "mounts": [{ "source": "a", "target": "/proc" }] }),
                "svc"
            )
            .is_err(),
            "reserved target must be rejected"
        );
    }

    #[test]
    fn enforce_mount_policy_gates_host_paths_and_writes() {
        let allowlist = vec!["/srv/code".to_string()];

        let named_ro = Mount {
            source: "dd-code".into(),
            target: "/opt/code".into(),
            read_only: true,
        };
        assert!(enforce_mount_policy(&allowlist, false, "svc", &named_ro).is_ok());

        let host_allowed = Mount {
            source: "/srv/code/bin".into(),
            target: "/opt/bin".into(),
            read_only: true,
        };
        assert!(enforce_mount_policy(&allowlist, false, "svc", &host_allowed).is_ok());

        // Host path outside the allowlist is rejected.
        let host_denied = Mount {
            source: "/etc".into(),
            target: "/opt/etc".into(),
            read_only: true,
        };
        assert!(enforce_mount_policy(&allowlist, false, "svc", &host_denied).is_err());
        // ...and rejected outright when no allowlist is configured.
        assert!(enforce_mount_policy(&[], false, "svc", &host_allowed).is_err());

        // Writable needs the explicit opt-in.
        let writable = Mount {
            source: "dd-code".into(),
            target: "/opt/code".into(),
            read_only: false,
        };
        assert!(enforce_mount_policy(&allowlist, false, "svc", &writable).is_err());
        assert!(enforce_mount_policy(&allowlist, true, "svc", &writable).is_ok());
    }
}
