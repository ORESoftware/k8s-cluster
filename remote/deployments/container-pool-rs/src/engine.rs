use std::{sync::atomic::Ordering, time::Duration};

use serde_json::Value;
use tokio::{process::Command, time::timeout};

use crate::types::{AppState, EngineKind};

// Global flags that precede every subcommand. Only nerdctl needs the containerd
// namespace; docker/podman take none.
pub(crate) fn engine_global_args(engine: EngineKind, namespace: &str) -> Vec<String> {
    if engine.uses_namespace() {
        vec!["-n".to_string(), namespace.to_string()]
    } else {
        Vec::new()
    }
}

pub(crate) async fn remove_container(state: &AppState, name: &str) -> Result<(), String> {
    let mut args = engine_global_args(state.config.engine, &state.config.containerd_namespace);
    args.extend(["rm".to_string(), "-f".to_string(), name.to_string()]);
    run_command(
        &state.config.engine_bin,
        &args,
        state.config.command_timeout,
    )
    .await?;
    state
        .metrics
        .containers_removed_total
        .fetch_add(1, Ordering::Relaxed);
    Ok(())
}

pub(crate) async fn cleanup_managed_containers_on_start(state: &AppState) -> Result<(), String> {
    if !state.config.cleanup_on_start {
        return Ok(());
    }
    let mut list_args = engine_global_args(state.config.engine, &state.config.containerd_namespace);
    list_args.extend([
        "ps".to_string(),
        "-a".to_string(),
        "-q".to_string(),
        "--filter".to_string(),
        "label=dd.container-pool.managed=true".to_string(),
    ]);
    let output = run_command(
        &state.config.engine_bin,
        &list_args,
        state.config.command_timeout,
    )
    .await?;
    for id in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Err(error) = remove_container(state, id).await {
            tracing::error!("failed to remove stale managed container {id}: {error}");
        }
    }
    Ok(())
}

pub(crate) async fn run_command(
    program: &str,
    args: &[String],
    command_timeout: Duration,
) -> Result<String, String> {
    let output = timeout(command_timeout, Command::new(program).args(args).output())
        .await
        .map_err(|_| format!("{program} timed out after {}s", command_timeout.as_secs()))?
        .map_err(|error| format!("{program} failed to start: {error}"))?;
    if output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr_trimmed = stderr.trim();
        if !stderr_trimmed.is_empty() && args.iter().any(|arg| arg == "run" || arg == "inspect") {
            let stderr = stderr_trimmed.chars().take(1500).collect::<String>();
            if benign_success_stderr(&stderr) {
                tracing::debug!("{program} benign stderr (exit 0, args={args:?}): {stderr}");
            } else {
                tracing::warn!("{program} stderr (exit 0, args={args:?}): {stderr}");
            }
        }
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr)
        .chars()
        .take(2000)
        .collect::<String>();
    Err(format!(
        "{program} exited with {}: {stderr}",
        output.status.code().unwrap_or(-1)
    ))
}

fn benign_success_stderr(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("failed to inspect netns")
        && lower.contains("/proc/")
        && lower.contains("no such file or directory")
}

pub(crate) async fn inspect_container_running(state: &AppState, name: &str) -> Result<bool, String> {
    let inspect_timeout = state.config.command_timeout.min(Duration::from_secs(15));
    let mut args = engine_global_args(state.config.engine, &state.config.containerd_namespace);
    args.extend(["inspect".to_string(), name.to_string()]);
    let output = match run_command(&state.config.engine_bin, &args, inspect_timeout).await {
        Ok(output) => output,
        Err(error) => {
            let lower = error.to_ascii_lowercase();
            if lower.contains("not found") || lower.contains("no such") {
                return Ok(false);
            }
            return Err(error);
        }
    };
    let value = serde_json::from_str::<Value>(&output).map_err(|error| error.to_string())?;
    let Some(container) = value
        .as_array()
        .and_then(|items| items.first())
        .or(Some(&value))
    else {
        return Ok(false);
    };
    if let Some(running) = container
        .pointer("/State/Running")
        .and_then(Value::as_bool)
        .or_else(|| container.pointer("/State/running").and_then(Value::as_bool))
    {
        return Ok(running);
    }
    let status = container
        .pointer("/State/Status")
        .or_else(|| container.pointer("/State/status"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(status.eq_ignore_ascii_case("running"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_only_known_success_stderr_as_benign() {
        assert!(benign_success_stderr(
            "failed to inspect NetNS: failed to Statfs /proc/123/ns/net: no such file or directory"
        ));
        assert!(!benign_success_stderr(
            "permission denied opening containerd socket"
        ));
        assert!(!benign_success_stderr(
            "failed to inspect NetNS: operation not permitted"
        ));
    }
}
