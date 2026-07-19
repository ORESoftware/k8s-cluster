//! fiducia.cloud (github.com/fiducia-cloud) coordination for concurrent builds.
//!
//! Two primitives are used, both over the fiducia HTTP/JSON API (the
//! sharded-multi-Raft data plane behind fiducia-load-balance):
//!
//! - **Locks** (`/v1/locks/acquire` / `/v1/locks/release`): one lock per image
//!   reference serializes concurrent builds of the same image across every
//!   build-server replica and any other fiducia tenant sharing the key space.
//!   Grants carry a monotonic u64 fencing token which is persisted on the job
//!   row and attached to lifecycle events.
//! - **Idempotency leases** (`/v1/idempotency/claim|complete|abandon`):
//!   first-writer dedupe for webhook deliveries and NATS request ids, so
//!   at-least-once delivery cannot double-build (Postgres remains the local
//!   dedupe guard; the lease covers multi-replica and DB-outage windows).
//!
//! Waiting follows the fiducia protocol: `wait:false` try-lock plus a
//! client-side retry budget (the server never holds requests open).
//!
//! Configuration (mirrors dd-contract-service):
//!   FIDUCIA_LOCK_URL     default http://fiducia-load-balance.fiducia.svc.cluster.local:8088
//!   FIDUCIA_API_KEY      optional bearer token
//!   BUILD_SERVER_COORDINATION_ENABLED   opt-in (default false)
//!   BUILD_SERVER_COORDINATION_REQUIRED  fail builds when fiducia is down (default false)

use serde_json::json;
use std::time::Duration;

use crate::Config;

#[derive(Debug, Clone)]
pub struct LockGrant {
    pub key: String,
    pub holder: String,
    pub fencing_token: u64,
}

#[derive(Debug)]
pub enum LockOutcome {
    /// Coordination disabled or not configured; caller proceeds with the
    /// local semaphore only.
    Disabled,
    Acquired(LockGrant),
    /// Retry budget exhausted while another holder kept the lock.
    Busy {
        key: String,
    },
    /// fiducia unreachable/errored. Fatal only when coordination is required.
    Unavailable {
        error: String,
    },
}

/// Reject URLs that would leak the bearer token to arbitrary hosts: https
/// anywhere, plain http only inside the cluster (dd-contract-service policy).
pub fn validate_lock_url(url: &str) -> Result<(), String> {
    if url.contains('@') {
        return Err("fiducia URL must not embed credentials".to_string());
    }
    if url.starts_with("https://") {
        return Ok(());
    }
    if url.starts_with("http://") {
        let host = url
            .trim_start_matches("http://")
            .split(['/', ':'])
            .next()
            .unwrap_or_default();
        if host.ends_with(".svc.cluster.local") || host == "localhost" || host == "127.0.0.1" {
            return Ok(());
        }
        return Err(
            "plain-http fiducia URL is only allowed for in-cluster .svc.cluster.local hosts"
                .to_string(),
        );
    }
    Err("fiducia URL must be http(s)".to_string())
}

fn request(
    http: &reqwest::Client,
    config: &Config,
    path: &str,
    body: serde_json::Value,
) -> reqwest::RequestBuilder {
    let url = format!("{}{path}", config.fiducia_url.trim_end_matches('/'));
    let mut builder = http.post(url).timeout(Duration::from_secs(10)).json(&body);
    if let Some(api_key) = config.fiducia_api_key.as_deref() {
        builder = builder.bearer_auth(api_key);
    }
    builder
}

async fn post_json(
    http: &reqwest::Client,
    config: &Config,
    path: &str,
    body: serde_json::Value,
) -> Result<(u16, serde_json::Value), String> {
    let response = request(http, config, path, body)
        .send()
        .await
        .map_err(|error| format!("fiducia request to {path} failed: {error}"))?;
    let status = response.status().as_u16();
    let value = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or(serde_json::Value::Null);
    Ok((status, value))
}

fn fencing_token(value: &serde_json::Value) -> Option<u64> {
    value
        .get("fencing_token")
        .or_else(|| value.get("fencingToken"))
        .and_then(serde_json::Value::as_u64)
}

fn explicitly_not_acquired(value: &serde_json::Value) -> bool {
    [value.get("acquired"), value.get("ok")]
        .into_iter()
        .flatten()
        .any(|flag| flag.as_bool() == Some(false))
}

/// Try-lock with a client-side retry budget. `Busy`/`Unavailable` handling is
/// the caller's policy decision (required vs best-effort coordination).
pub async fn acquire_lock(
    http: &reqwest::Client,
    config: &Config,
    key: &str,
    holder: &str,
) -> LockOutcome {
    if !config.coordination_enabled {
        return LockOutcome::Disabled;
    }
    let deadline = tokio::time::Instant::now() + config.lock_wait_budget;
    let mut last_error: Option<String> = None;
    loop {
        let body = json!({
            "key": key,
            "holder": holder,
            "ttl_ms": config.lock_ttl.as_millis() as u64,
            "wait": false,
        });
        match post_json(http, config, "/v1/locks/acquire", body).await {
            Ok((status, value)) if (200..300).contains(&status) => {
                if let Some(token) = fencing_token(&value) {
                    if !explicitly_not_acquired(&value) {
                        return LockOutcome::Acquired(LockGrant {
                            key: key.to_string(),
                            holder: holder.to_string(),
                            fencing_token: token,
                        });
                    }
                }
                // 2xx without a grant = queued/busy; fall through to retry.
            }
            Ok((409, _)) | Ok((423, _)) => {}
            Ok((status, value)) => {
                last_error = Some(format!(
                    "fiducia lock acquire returned HTTP {status}: {}",
                    value.to_string().chars().take(300).collect::<String>()
                ));
            }
            Err(error) => last_error = Some(error),
        }
        if tokio::time::Instant::now() >= deadline {
            return match last_error {
                Some(error) => LockOutcome::Unavailable { error },
                None => LockOutcome::Busy {
                    key: key.to_string(),
                },
            };
        }
        tokio::time::sleep(config.lock_retry_interval).await;
    }
}

pub async fn release_lock(http: &reqwest::Client, config: &Config, grant: &LockGrant) {
    let body = json!({
        "key": grant.key,
        "holder": grant.holder,
        "fencing_token": grant.fencing_token,
    });
    match post_json(http, config, "/v1/locks/release", body).await {
        Ok((status, _)) if (200..300).contains(&status) => {}
        Ok((status, value)) => tracing::warn!(
            "fiducia lock release for {} returned HTTP {status}: {}",
            grant.key,
            value.to_string().chars().take(300).collect::<String>()
        ),
        Err(error) => tracing::warn!("fiducia lock release for {} failed: {error}", grant.key),
    }
}

/// First-writer idempotency claim. `Ok(true)` = we own the key and should do
/// the work; `Ok(false)` = someone already claimed/completed it. Errors are
/// returned so the caller can decide (fail-open with the Postgres dedupe as
/// the local guard).
pub async fn idempotency_claim(
    http: &reqwest::Client,
    config: &Config,
    key: &str,
    holder: &str,
) -> Result<bool, String> {
    if !config.coordination_enabled {
        return Ok(true);
    }
    let body = json!({
        "key": key,
        "holder": holder,
        "ttl_ms": config.idempotency_lease.as_millis() as u64,
        "retention_ms": config.idempotency_retention.as_millis() as u64,
    });
    let (status, value) = post_json(http, config, "/v1/idempotency/claim", body).await?;
    if (200..300).contains(&status) {
        if explicitly_not_acquired(&value)
            || value.get("state").and_then(serde_json::Value::as_str) == Some("completed")
            || value.get("duplicate").and_then(serde_json::Value::as_bool) == Some(true)
        {
            return Ok(false);
        }
        return Ok(true);
    }
    if status == 409 {
        return Ok(false);
    }
    Err(format!(
        "fiducia idempotency claim returned HTTP {status}: {}",
        value.to_string().chars().take(300).collect::<String>()
    ))
}

pub async fn idempotency_finish(
    http: &reqwest::Client,
    config: &Config,
    key: &str,
    holder: &str,
    succeeded: bool,
) {
    if !config.coordination_enabled {
        return;
    }
    let path = if succeeded {
        "/v1/idempotency/complete"
    } else {
        "/v1/idempotency/abandon"
    };
    let body = json!({ "key": key, "holder": holder });
    if let Err(error) = post_json(http, config, path, body).await {
        tracing::warn!("fiducia {path} for {key} failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_url_policy_allows_cluster_http_and_https_only() {
        assert!(validate_lock_url("https://locks.fiducia.cloud").is_ok());
        assert!(
            validate_lock_url("http://fiducia-load-balance.fiducia.svc.cluster.local:8088").is_ok()
        );
        assert!(validate_lock_url("http://example.com:8088").is_err());
        assert!(validate_lock_url("https://user:pass@locks.fiducia.cloud").is_err());
    }
}
