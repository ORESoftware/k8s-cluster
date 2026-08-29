use std::time::Duration;

use dd_nats_subject_defs::{CONTAINER_POOL_REQUESTS_SUBJECT, CONTAINER_POOL_RESULTS_SUBJECT};

use crate::{
    types::{
        EngineKind, ServiceConfig, DEFAULT_REDIS_LOCK_PREFIX, MAX_NATS_PAYLOAD_BYTES,
        MAX_WORKER_RESPONSE_BYTES, SERVICE_NAME,
    },
    util::{
        env_bool, env_u16, env_u64, env_usize, env_value, first_env, safe_env_key,
        safe_local_path, safe_nats_queue_group, safe_network_name, safe_resource_value,
    },
};

pub(crate) fn service_config_from_env() -> ServiceConfig {
    let port_start = env_u16("CONTAINER_POOL_PORT_START", 12_000);
    let port_end = env_u16("CONTAINER_POOL_PORT_END", 12_999).max(port_start);
    let network = env_value("CONTAINER_POOL_NETWORK", "host");
    let engine_raw = env_value("CONTAINER_POOL_ENGINE", "nerdctl");
    let engine = EngineKind::parse(&engine_raw);
    let nats_queue_group_raw = env_value("CONTAINER_POOL_NATS_QUEUE_GROUP", "dd-container-pool");
    let nats_queue_group = if safe_nats_queue_group(&nats_queue_group_raw) {
        nats_queue_group_raw
    } else {
        tracing::warn!("invalid CONTAINER_POOL_NATS_QUEUE_GROUP; using dd-container-pool");
        "dd-container-pool".to_string()
    };
    if !engine_raw.trim().is_empty() && !engine_value_recognized(&engine_raw) {
        tracing::warn!(
            "{SERVICE_NAME} warning: unrecognized CONTAINER_POOL_ENGINE={engine_raw:?}; defaulting to nerdctl"
        );
    }
    let oci_runtime =
        match classify_oci_runtime(first_env(&["CONTAINER_POOL_OCI_RUNTIME"]).as_deref()) {
            Ok(value) => value,
            Err(message) => {
                tracing::warn!(
                "{SERVICE_NAME} warning: {message}; containers will use the engine default OCI \
                 runtime (no --runtime) — this may be weaker isolation than intended"
            );
                None
            }
        };
    ServiceConfig {
        engine,
        engine_bin: first_env(&["CONTAINER_POOL_ENGINE_BIN", "CONTAINER_POOL_NERDCTL_BIN"])
            .filter(|bin| !bin.trim().is_empty())
            .unwrap_or_else(|| engine.default_bin().to_string()),
        oci_runtime,
        containerd_namespace: env_value("CONTAINER_POOL_CONTAINERD_NAMESPACE", "k8s.io"),
        network: if safe_network_name(&network) {
            network
        } else {
            "host".to_string()
        },
        pull_policy: first_env(&["CONTAINER_POOL_PULL_POLICY"]).and_then(|value| {
            matches!(value.as_str(), "always" | "missing" | "never").then_some(value)
        }),
        database_url: first_env(&[
            "CONTAINER_POOL_DATABASE_URL",
            "AGENT_TASKS_RDS_DATABASE_URL",
            "RDS_DATABASE_URL",
            "DATABASE_URL",
        ]),
        app_config_key: env_value(
            "CONTAINER_POOL_APP_CONFIG_KEY",
            "container-pool.runtime-pools.v1",
        ),
        app_config_scope: env_value("CONTAINER_POOL_APP_CONFIG_SCOPE", "default"),
        nats_url: first_env(&["NATS_URL"]),
        nats_subject: env_value(
            "CONTAINER_POOL_NATS_SUBJECT",
            CONTAINER_POOL_REQUESTS_SUBJECT,
        ),
        nats_queue_group,
        nats_result_subject: env_value(
            "CONTAINER_POOL_NATS_RESULT_SUBJECT",
            CONTAINER_POOL_RESULTS_SUBJECT,
        ),
        nats_max_payload_bytes: env_usize(
            "CONTAINER_POOL_NATS_MAX_PAYLOAD_BYTES",
            MAX_NATS_PAYLOAD_BYTES,
        )
        .min(16 * 1024 * 1024),
        redis_url: first_env(&["CONTAINER_POOL_REDIS_URL", "REDIS_URL"]),
        redis_lock_prefix: env_value(
            "CONTAINER_POOL_REDIS_LOCK_PREFIX",
            DEFAULT_REDIS_LOCK_PREFIX,
        ),
        redis_lock_ttl: Duration::from_secs(env_u64("CONTAINER_POOL_REDIS_LOCK_TTL_SECONDS", 600)),
        redis_lock_wait_timeout: Duration::from_secs(env_u64(
            "CONTAINER_POOL_REDIS_LOCK_WAIT_TIMEOUT_SECONDS",
            420,
        )),
        redis_lock_retry_delay: Duration::from_millis(env_u64(
            "CONTAINER_POOL_REDIS_LOCK_RETRY_MS",
            250,
        )),
        redis_lock_request_timeout: Duration::from_millis(env_u64(
            "CONTAINER_POOL_REDIS_LOCK_REQUEST_TIMEOUT_MS",
            800,
        )),
        worker_response_max_bytes: env_usize(
            "CONTAINER_POOL_WORKER_RESPONSE_MAX_BYTES",
            MAX_WORKER_RESPONSE_BYTES,
        )
        .min(16 * 1024 * 1024),
        config_refresh: Duration::from_secs(env_u64("CONTAINER_POOL_CONFIG_REFRESH_SECONDS", 30)),
        reconcile_interval: Duration::from_secs(env_u64("CONTAINER_POOL_RECONCILE_SECONDS", 10)),
        command_timeout: Duration::from_secs(env_u64("CONTAINER_POOL_COMMAND_TIMEOUT_SECONDS", 30)),
        nerdctl_run_timeout: Duration::from_secs(env_u64(
            "CONTAINER_POOL_NERDCTL_RUN_TIMEOUT_SECONDS",
            180,
        )),
        container_start_timeout: Duration::from_secs(env_u64(
            "CONTAINER_POOL_START_TIMEOUT_SECONDS",
            15,
        )),
        health_check_interval: Duration::from_secs(env_u64(
            "CONTAINER_POOL_HEALTH_CHECK_SECONDS",
            10,
        )),
        health_check_timeout: Duration::from_millis(env_u64(
            "CONTAINER_POOL_HEALTH_TIMEOUT_MS",
            1_000,
        )),
        unhealthy_grace: Duration::from_secs(env_u64("CONTAINER_POOL_UNHEALTHY_GRACE_SECONDS", 5)),
        unhealthy_failure_threshold: env_u64("CONTAINER_POOL_UNHEALTHY_FAILURE_THRESHOLD", 2)
            .clamp(1, 10),
        port_start,
        port_end,
        cleanup_on_start: env_bool("CONTAINER_POOL_CLEANUP_ON_START", true),
        server_auth_secret: first_env(&[
            "CONTAINER_POOL_AUTH_SECRET",
            "SERVER_AUTH_SECRET",
            "REMOTE_DEV_SERVER_SECRET",
        ]),
        container_memory: first_env(&["CONTAINER_POOL_CONTAINER_MEMORY"])
            .filter(|value| safe_resource_value(value)),
        container_cpus: first_env(&["CONTAINER_POOL_CONTAINER_CPUS"])
            .filter(|value| safe_resource_value(value)),
        forward_env_keys: forwarded_worker_env_keys(),
        pids_limit: env_u64("CONTAINER_POOL_PIDS_LIMIT", 4096).clamp(16, 16384),
        nofile_limit: env_u64("CONTAINER_POOL_NOFILE_LIMIT", 65536).clamp(32, 262144),
        cap_drop_all: env_bool("CONTAINER_POOL_CAP_DROP_ALL", false),
        no_new_privileges: env_bool("CONTAINER_POOL_NO_NEW_PRIVILEGES", false),
        mount_source_allowlist: mount_source_allowlist(),
        allow_writable_mounts: env_bool("CONTAINER_POOL_ALLOW_WRITABLE_MOUNTS", false),
    }
}

// Absolute host-path prefixes under which pools may bind-mount code/binaries.
// Empty by default: only named volumes are permitted unless an operator opts in.
fn mount_source_allowlist() -> Vec<String> {
    first_env(&["CONTAINER_POOL_MOUNT_SOURCE_ALLOWLIST"])
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|prefix| prefix.starts_with('/') && safe_local_path(prefix))
        .map(str::to_string)
        .collect()
}

fn forwarded_worker_env_keys() -> Vec<String> {
    let configured = first_env(&["CONTAINER_POOL_FORWARD_ENV_KEYS"]).unwrap_or_else(|| {
        [
            "SERVER_AUTH_SECRET",
            "REMOTE_DEV_SERVER_SECRET",
            "GH_DEPLOY_KEY",
            "GH_PAT",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_API_KEYS_JSON",
            "CLAUDE_API_KEYS_JSON",
            "OPENAI_API_KEY",
            "OPENAI_API_KEYS_JSON",
            "GOOGLE_API_KEY",
            "GOOGLE_API_KEYS_JSON",
            "GEMINI_API_KEY",
            "GEMINI_API_KEYS_JSON",
            "OPENCODE_API_KEY",
            "OPENCODE_API_KEYS_JSON",
            "OPENCODE_BASE_URL",
            "OPENCODE_MODELS",
            "EVENT_INGEST_URL",
            "EVENT_INGEST_SECRET",
            "GLEAM_WORKER_WS_SECRET",
            "WORKER_FANOUT_WS_SECRET",
            "WORKER_FANOUT_WS_BASE_URL",
        ]
        .join(",")
    });
    configured
        .split(',')
        .map(str::trim)
        .filter(|key| safe_env_key(key))
        .map(str::to_string)
        .collect()
}

// An OCI runtime handler for `--runtime`: a short name (runc, crun, runsc), a
// containerd handler (io.containerd.kata.v2, io.containerd.runsc.v1), or an
// absolute path to a runtime binary. No whitespace / shell metacharacters.
pub(crate) fn safe_oci_runtime(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && (bytes[0].is_ascii_alphanumeric() || bytes[0] == b'/')
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
}

// Resolve CONTAINER_POOL_OCI_RUNTIME. Absent/empty -> None (engine default
// runtime). Valid -> Some. Set-but-invalid -> Err, so the caller warns loudly
// instead of silently dropping the operator's chosen runtime — for sandbox
// runtimes (gVisor/Kata) a silent fallback to runc is an isolation downgrade.
fn classify_oci_runtime(raw: Option<&str>) -> Result<Option<String>, String> {
    match raw {
        None => Ok(None),
        Some(value) if value.trim().is_empty() => Ok(None),
        Some(value) if safe_oci_runtime(value) => Ok(Some(value.to_string())),
        Some(value) => Err(format!(
            "ignoring invalid CONTAINER_POOL_OCI_RUNTIME={value:?}"
        )),
    }
}

fn engine_value_recognized(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "nerdctl" | "docker" | "podman"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oci_runtime_set_but_invalid_is_an_error_not_a_silent_downgrade() {
        // Absent / empty => engine default (None), no warning path.
        assert_eq!(classify_oci_runtime(None), Ok(None));
        assert_eq!(classify_oci_runtime(Some("")), Ok(None));
        assert_eq!(classify_oci_runtime(Some("   ")), Ok(None));
        // Valid handlers pass through.
        assert_eq!(
            classify_oci_runtime(Some("runsc")),
            Ok(Some("runsc".to_string()))
        );
        assert_eq!(
            classify_oci_runtime(Some("io.containerd.kata.v2")),
            Ok(Some("io.containerd.kata.v2".to_string()))
        );
        // Set-but-invalid must surface as an error so the caller warns instead of
        // silently running under the (weaker) default runtime.
        assert!(classify_oci_runtime(Some("runc; rm -rf /")).is_err());
        assert!(classify_oci_runtime(Some("two words")).is_err());
    }

    #[test]
    fn engine_value_recognition() {
        for ok in ["nerdctl", "Docker", " podman "] {
            assert!(engine_value_recognized(ok), "{ok} should be recognized");
        }
        for bad in ["dcoker", "lxd", "crio", "containerd-shim"] {
            assert!(
                !engine_value_recognized(bad),
                "{bad} should not be recognized"
            );
        }
        // Parser still falls back to nerdctl for unknown values.
        assert_eq!(EngineKind::parse("dcoker"), EngineKind::Nerdctl);
        assert_eq!(EngineKind::parse("podman"), EngineKind::Podman);
    }
}
