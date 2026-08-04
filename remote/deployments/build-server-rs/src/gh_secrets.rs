//! GitHub Actions secret sync.
//!
//! Source of truth stays AWS Secrets Manager → External Secrets Operator →
//! the pod's env (dd-agent-secrets et al.) — the cluster's cross-cloud
//! secrets backbone (the ClusterSecretStore has AWS/GCP/Hetzner providers).
//! This module pushes selected env-provided values OUT to GitHub Actions
//! repo secrets over the GitHub REST API, so GHA workflows and the cluster
//! share one secret source without hand-copying:
//!
//!   GET /repos/{repo}/actions/secrets/public-key   → libsodium sealed-box key
//!   PUT /repos/{repo}/actions/secrets/{name}       → {encrypted_value, key_id}
//!
//! Values are encrypted client-side with the repo's public key (crypto_box
//! sealed box, the same construction `gh secret set` uses). Only SHA-256
//! hashes of values are persisted for change detection — never the values.
//!
//! Config:
//!   BUILD_SERVER_GH_SYNC_ENABLED           opt-in (default false)
//!   GH_SECRETS_SYNC_TOKEN | GH_PAT | GITHUB_TOKEN   PAT with `repo` scope
//!   BUILD_SERVER_GH_SYNC_RULES / _PATH     JSON: [{"repo":"owner/name",
//!                                          "secrets":{"GH_NAME":{"fromEnv":"ENV_NAME"}}}]
//!   BUILD_SERVER_GH_SYNC_INTERVAL_SECONDS  periodic sync (0 = manual only)
//!
//! Trigger: POST /secrets/sync (operator auth) or the periodic loop; status
//! at GET /secrets/sync/status (hashes and outcomes only).

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, env, sync::atomic::Ordering, time::Duration};

use crate::{db, AppState};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncRule {
    /// GitHub `owner/name`.
    pub repo: String,
    /// GitHub secret name → source spec.
    pub secrets: BTreeMap<String, SecretSource>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretSource {
    /// Environment variable holding the value (populated by External Secrets
    /// from AWS Secrets Manager). The value itself never appears in rules.
    pub from_env: String,
}

/// Policy that constrains which repos may receive secrets and which process
/// env vars may be used as a source. Both are enforced at parse time so an
/// invalid rule set is rejected before any sync runs.
#[derive(Debug, Clone, Default)]
pub struct SyncPolicy {
    /// Repo owners (case-insensitive) allowed as a sync target. Empty = deny
    /// all: secret sync is fail-closed until an operator names the owners.
    pub allowed_owners: Vec<String>,
    /// Exact env-var names permitted as a `fromEnv` source. When empty, sources
    /// must instead carry the `GH_SYNC_` prefix convention.
    pub allowed_env: Vec<String>,
}

/// Env names (or substrings) that may NEVER be synced to GitHub, regardless of
/// allowlist — the cluster crown jewels. This is the structural block on the
/// "patch the rules ConfigMap to exfiltrate every secret" chain.
const FORBIDDEN_ENV_SUBSTRINGS: &[&str] = &[
    "AWS_SECRET",
    "AWS_ACCESS",
    "AWS_SESSION",
    "FIDUCIA_API_KEY",
    "SERVER_AUTH_SECRET",
    "DATABASE_URL",
    "GH_PAT",
    "GITHUB_TOKEN",
    "GH_SECRETS_SYNC_TOKEN",
    "RUNTIME_CONFIG_SERVER_SECRET",
    "WEBHOOK_SECRET",
    "PRIVATE_KEY",
];

fn repo_owner(repo: &str) -> Option<&str> {
    repo.split_once('/')
        .map(|(owner, _)| owner)
        .filter(|owner| {
            !owner.is_empty()
                && owner
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        })
}

fn env_source_allowed(from_env: &str, policy: &SyncPolicy) -> Result<(), String> {
    let upper = from_env.to_ascii_uppercase();
    if FORBIDDEN_ENV_SUBSTRINGS
        .iter()
        .any(|needle| upper.contains(needle))
    {
        return Err(format!(
            "gh sync source env {from_env:?} is a protected cluster secret and can never be synced"
        ));
    }
    if policy.allowed_env.is_empty() {
        if !from_env.starts_with("GH_SYNC_") {
            return Err(format!(
                "gh sync source env {from_env:?} must be listed in BUILD_SERVER_GH_SYNC_ALLOWED_ENV or use the GH_SYNC_ prefix"
            ));
        }
    } else if !policy.allowed_env.iter().any(|name| name == from_env) {
        return Err(format!(
            "gh sync source env {from_env:?} is not in BUILD_SERVER_GH_SYNC_ALLOWED_ENV"
        ));
    }
    Ok(())
}

pub fn parse_rules(raw: &str, policy: &SyncPolicy) -> Result<Vec<SyncRule>, String> {
    let rules = serde_json::from_str::<Vec<SyncRule>>(raw)
        .map_err(|error| format!("invalid gh secret sync rules JSON: {error}"))?;
    for rule in &rules {
        // Repo must be a strict `owner/name` with a clean charset — blocks the
        // `x/../../gists` dot-segment trick that reqwest would normalize into a
        // different GitHub endpoint.
        let owner = repo_owner(&rule.repo).ok_or_else(|| {
            format!(
                "gh sync rule repo {:?} must be a valid owner/name",
                rule.repo
            )
        })?;
        let (_, name) = rule.repo.split_once('/').unwrap_or_default();
        let name_ok = !name.is_empty()
            && !name.contains('/')
            && name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'));
        if !name_ok {
            return Err(format!(
                "gh sync rule repo {:?} must be a valid owner/name",
                rule.repo
            ));
        }
        // Owner allowlist — fail closed on an empty policy.
        if policy.allowed_owners.is_empty() {
            return Err(
                "gh secret sync is enabled but BUILD_SERVER_GH_SYNC_ALLOWED_OWNERS is empty; refusing all rules".to_string(),
            );
        }
        if !policy
            .allowed_owners
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(owner))
        {
            return Err(format!(
                "gh sync rule owner {owner:?} is not in BUILD_SERVER_GH_SYNC_ALLOWED_OWNERS"
            ));
        }
        for (name, source) in &rule.secrets {
            if name.is_empty()
                || !name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            {
                return Err(format!(
                    "gh secret name {name:?} is not a valid secret name"
                ));
            }
            env_source_allowed(&source.from_env, policy)?;
        }
    }
    Ok(rules)
}

#[derive(Debug, Deserialize)]
struct PublicKeyResponse {
    key_id: String,
    key: String,
}

fn seal_for_repo(public_key_b64: &str, value: &str) -> Result<String, String> {
    let key_bytes = BASE64
        .decode(public_key_b64)
        .map_err(|error| format!("repo public key is not valid base64: {error}"))?;
    let key_bytes: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| "repo public key must be 32 bytes".to_string())?;
    let public_key = crypto_box::PublicKey::from(key_bytes);
    let sealed = public_key
        .seal(&mut crypto_box::aead::OsRng, value.as_bytes())
        .map_err(|error| format!("sealed-box encryption failed: {error}"))?;
    Ok(BASE64.encode(sealed))
}

fn gh_headers(token: &str) -> Result<reqwest::header::HeaderMap, String> {
    use reqwest::header::{HeaderMap, HeaderValue};
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|error| error.to_string())?,
    );
    headers.insert(
        "accept",
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(
        "x-github-api-version",
        HeaderValue::from_static("2022-11-28"),
    );
    headers.insert(
        "user-agent",
        HeaderValue::from_static("dd-build-server-secret-sync"),
    );
    Ok(headers)
}

async fn repo_public_key(
    http: &reqwest::Client,
    token: &str,
    repo: &str,
) -> Result<PublicKeyResponse, String> {
    let response = http
        .get(format!(
            "https://api.github.com/repos/{repo}/actions/secrets/public-key"
        ))
        .headers(gh_headers(token)?)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|error| format!("failed to fetch repo public key: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "GitHub public-key request for {repo} failed with HTTP {}",
            status.as_u16()
        ));
    }
    response
        .json::<PublicKeyResponse>()
        .await
        .map_err(|error| format!("invalid public-key response: {error}"))
}

async fn put_secret(
    http: &reqwest::Client,
    token: &str,
    repo: &str,
    name: &str,
    key_id: &str,
    encrypted_value: &str,
) -> Result<(), String> {
    let response = http
        .put(format!(
            "https://api.github.com/repos/{repo}/actions/secrets/{name}"
        ))
        .headers(gh_headers(token)?)
        .timeout(Duration::from_secs(15))
        .json(&json!({ "encrypted_value": encrypted_value, "key_id": key_id }))
        .send()
        .await
        .map_err(|error| format!("failed to PUT secret: {error}"))?;
    let status = response.status();
    if status.as_u16() == 201 || status.as_u16() == 204 {
        Ok(())
    } else {
        Err(format!(
            "GitHub secret PUT failed with HTTP {}",
            status.as_u16()
        ))
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOutcome {
    pub repo: String,
    pub secret: String,
    pub status: String,
    pub detail: Option<String>,
}

/// Run one full sync pass. Returns per-secret outcomes (no values).
pub async fn sync_all(state: &AppState) -> Vec<SyncOutcome> {
    let mut outcomes = Vec::new();
    let Some(token) = state.config.gh_sync_token.as_deref() else {
        outcomes.push(SyncOutcome {
            repo: "-".to_string(),
            secret: "-".to_string(),
            status: "failed".to_string(),
            detail: Some("no GitHub token configured (GH_SECRETS_SYNC_TOKEN/GH_PAT)".to_string()),
        });
        return outcomes;
    };

    for rule in &state.config.gh_sync_rules {
        let public_key = match repo_public_key(&state.http, token, &rule.repo).await {
            Ok(key) => key,
            Err(error) => {
                state
                    .counters
                    .gh_secret_sync_failures
                    .fetch_add(1, Ordering::Relaxed);
                for name in rule.secrets.keys() {
                    outcomes.push(SyncOutcome {
                        repo: rule.repo.clone(),
                        secret: name.clone(),
                        status: "failed".to_string(),
                        detail: Some(error.clone()),
                    });
                }
                continue;
            }
        };

        for (name, source) in &rule.secrets {
            let Some(value) = env::var(&source.from_env)
                .ok()
                .filter(|value| !value.is_empty())
            else {
                let outcome = SyncOutcome {
                    repo: rule.repo.clone(),
                    secret: name.clone(),
                    status: "failed".to_string(),
                    detail: Some(format!("source env {} is unset or empty", source.from_env)),
                };
                if let Some(db) = state.db.as_ref() {
                    db::record_secret_sync(
                        db,
                        &rule.repo,
                        name,
                        "-",
                        "failed",
                        outcome.detail.as_deref(),
                    )
                    .await;
                }
                outcomes.push(outcome);
                continue;
            };

            let value_sha256 = hex::encode(Sha256::digest(value.as_bytes()));
            if let Some(db) = state.db.as_ref() {
                if db::last_synced_sha256(db, &rule.repo, name)
                    .await
                    .as_deref()
                    == Some(value_sha256.as_str())
                {
                    db::record_secret_sync(
                        db,
                        &rule.repo,
                        name,
                        &value_sha256,
                        "skipped-unchanged",
                        None,
                    )
                    .await;
                    outcomes.push(SyncOutcome {
                        repo: rule.repo.clone(),
                        secret: name.clone(),
                        status: "skipped-unchanged".to_string(),
                        detail: None,
                    });
                    continue;
                }
            }

            let result = match seal_for_repo(&public_key.key, &value) {
                Ok(encrypted) => {
                    put_secret(
                        &state.http,
                        token,
                        &rule.repo,
                        name,
                        &public_key.key_id,
                        &encrypted,
                    )
                    .await
                }
                Err(error) => Err(error),
            };

            let (status, detail) = match result {
                Ok(()) => {
                    state
                        .counters
                        .gh_secrets_synced
                        .fetch_add(1, Ordering::Relaxed);
                    ("synced".to_string(), None)
                }
                Err(error) => {
                    state
                        .counters
                        .gh_secret_sync_failures
                        .fetch_add(1, Ordering::Relaxed);
                    ("failed".to_string(), Some(error))
                }
            };
            if let Some(db) = state.db.as_ref() {
                db::record_secret_sync(
                    db,
                    &rule.repo,
                    name,
                    &value_sha256,
                    &status,
                    detail.as_deref(),
                )
                .await;
            }
            outcomes.push(SyncOutcome {
                repo: rule.repo.clone(),
                secret: name.clone(),
                status,
                detail,
            });
        }
    }
    outcomes
}

/// Periodic sync loop, serialized across replicas by a fiducia lock.
pub async fn run_periodic_sync(state: AppState) {
    let interval = state.config.gh_sync_interval;
    if interval.is_zero() {
        return;
    }
    loop {
        tokio::time::sleep(interval).await;
        let lock_key = "build-server/gh-secrets-sync";
        match crate::fiducia::acquire_lock(&state.http, &state.config, lock_key, &state.holder)
            .await
        {
            crate::fiducia::LockOutcome::Busy { .. } => continue,
            crate::fiducia::LockOutcome::Acquired(grant) => {
                let outcomes = sync_all(&state).await;
                tracing::info!("periodic gh secret sync ran: {} outcome(s)", outcomes.len());
                crate::fiducia::release_lock(&state.http, &state.config, &grant).await;
            }
            // Disabled or unavailable coordination: run anyway (single-replica
            // deployments; PUTs are idempotent).
            _ => {
                let outcomes = sync_all(&state).await;
                tracing::info!("periodic gh secret sync ran: {} outcome(s)", outcomes.len());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> SyncPolicy {
        SyncPolicy {
            allowed_owners: vec!["ORESoftware".to_string()],
            allowed_env: Vec::new(),
        }
    }

    #[test]
    fn sync_rules_validate_repo_and_secret_names() {
        // Allowed owner + GH_SYNC_-prefixed source is accepted.
        let good = r#"[{"repo":"ORESoftware/k8s-cluster","secrets":{"BUILD_TOKEN":{"fromEnv":"GH_SYNC_BUILD_TOKEN"}}}]"#;
        assert!(parse_rules(good, &policy()).is_ok());
        let bad_repo = r#"[{"repo":"nope","secrets":{}}]"#;
        assert!(parse_rules(bad_repo, &policy()).is_err());
        let bad_name =
            r#"[{"repo":"ORESoftware/b","secrets":{"BAD NAME":{"fromEnv":"GH_SYNC_X"}}}]"#;
        assert!(parse_rules(bad_name, &policy()).is_err());
    }

    #[test]
    fn sync_rules_block_owner_and_crown_jewel_exfiltration() {
        // Owner not in the allowlist is rejected.
        let other_owner = r#"[{"repo":"attacker/x","secrets":{"A":{"fromEnv":"GH_SYNC_A"}}}]"#;
        assert!(parse_rules(other_owner, &policy()).is_err());

        // Empty owner allowlist fails closed even for a plausible repo.
        let empty_policy = SyncPolicy::default();
        let ok_repo = r#"[{"repo":"ORESoftware/x","secrets":{"A":{"fromEnv":"GH_SYNC_A"}}}]"#;
        assert!(parse_rules(ok_repo, &empty_policy).is_err());

        // Crown-jewel env sources can never be synced, even to an allowed owner.
        for env in [
            "AWS_SECRET_ACCESS_KEY",
            "FIDUCIA_API_KEY",
            "SERVER_AUTH_SECRET",
            "GH_PAT",
            "BUILD_SERVER_DATABASE_URL",
        ] {
            let raw =
                format!(r#"[{{"repo":"ORESoftware/x","secrets":{{"S":{{"fromEnv":"{env}"}}}}}}]"#);
            assert!(
                parse_rules(&raw, &policy()).is_err(),
                "{env} must be blocked as a sync source"
            );
        }

        // A non-crown-jewel name without the GH_SYNC_ prefix is rejected when no
        // explicit allowed_env list is configured.
        let unprefixed = r#"[{"repo":"ORESoftware/x","secrets":{"S":{"fromEnv":"SOME_VALUE"}}}]"#;
        assert!(parse_rules(unprefixed, &policy()).is_err());

        // An explicit allowed_env entry permits an otherwise-unprefixed name.
        let explicit = SyncPolicy {
            allowed_owners: vec!["ORESoftware".to_string()],
            allowed_env: vec!["SOME_VALUE".to_string()],
        };
        assert!(parse_rules(unprefixed, &explicit).is_ok());
    }

    #[test]
    fn sealed_box_encrypts_for_a_32_byte_key() {
        let secret_key = crypto_box::SecretKey::generate(&mut crypto_box::aead::OsRng);
        let public_b64 = BASE64.encode(secret_key.public_key().as_bytes());
        let sealed = seal_for_repo(&public_b64, "value").expect("seals");
        let sealed_bytes = BASE64.decode(sealed).expect("base64");
        let opened = secret_key
            .unseal(&sealed_bytes)
            .expect("recipient can open");
        assert_eq!(opened, b"value");
        assert!(seal_for_repo("not-base64!!", "value").is_err());
    }
}
